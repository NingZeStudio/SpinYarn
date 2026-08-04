use spinyarn::api::build_router;
use spinyarn::config::Config;
use spinyarn::START_TIME;
use std::sync::atomic::Ordering;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let start_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    START_TIME.store(start_ts, Ordering::Relaxed);

    let config = Config::load();
    tracing::info!("Bundled mappings dir: {}", config.maven.mappings_dir);
    tracing::info!(
        "cache: enabled={} max_entries={} high_watermark={} low_watermark={}",
        config.cache.enabled,
        config.cache.max_entries,
        config.cache.high_watermark,
        config.cache.low_watermark
    );
    let app = build_router(config.clone());

    // Bind with port-auto-increment: if the configured/default port is taken,
    // try the next one until a free port is found.
    let host = config.server.host.clone();
    let mut port = config.server.port;
    let (listener, addr) = loop {
        let addr = std::net::SocketAddr::new(
            host.parse().unwrap_or_else(|_| panic!("invalid host: {}", host)),
            port,
        );
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => break (listener, addr),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                let Some(next) = port.checked_add(1) else {
                    tracing::error!("no free port after {}", port);
                    std::process::exit(1);
                };
                tracing::warn!("port {} in use, falling back to {}", port, next);
                port = next;
            }
            Err(e) => panic!("failed to bind {}: {}", addr, e),
        }
    };
    tracing::info!("SpinYarn listening on {}", addr);

    axum::serve(listener, app).await.unwrap();
}
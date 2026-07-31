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
    std::env::set_var("SPINYARN_MAPPINGS_DIR", &config.maven.mappings_dir);
    tracing::info!(
        "Bundled mappings dir: {}",
        config.maven.mappings_dir
    );
    let app = build_router(config.clone());

    let addr = std::net::SocketAddr::new(config.server.host.parse().unwrap(), config.server.port);
    tracing::info!("SpinYarn listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
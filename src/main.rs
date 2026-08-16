use spinyarn::api::build_router;
use spinyarn::config::Config;
use spinyarn::mapping::download::{ensure_mapping, ensure_vanilla_mapping, mappings_dir_empty};
use spinyarn::START_TIME;
use std::sync::atomic::Ordering;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Spawn a background bootstrap download of the configured version list (both
/// Yarn and Vanilla families) when `maven.auto_download` is on and the
/// mappings directory is empty. Runs off the request path; the server keeps
/// serving while it fills in. Vanilla is skipped for versions without official
/// mappings (e.g. 1.14.3 and earlier) by `ensure_vanilla_mapping`.
fn bootstrap_mappings(config: &Config) {
    if !config.maven.auto_download {
        return;
    }
    if !mappings_dir_empty(&config.maven.mappings_dir) {
        tracing::debug!("mappings dir already populated, skipping bootstrap");
        return;
    }
    let versions = config.maven.bootstrap_versions.clone();
    let mappings_dir = config.maven.mappings_dir.clone();
    tokio::spawn(async move {
        tracing::info!(
            "bootstrap: mappings dir empty, downloading {} version(s)",
            versions.len()
        );
        for version in versions {
            let dir = mappings_dir.clone();
            let v = version.clone();
            let yarn = tokio::task::spawn_blocking(move || ensure_mapping(&v, &dir, false))
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or(false);
            let dir = mappings_dir.clone();
            let v = version.clone();
            let vanilla =
                tokio::task::spawn_blocking(move || ensure_vanilla_mapping(&v, &dir, false))
                    .await
                    .ok()
                    .and_then(|r| r.ok())
                    .unwrap_or(false);
            if yarn {
                tracing::info!("bootstrap: {} yarn ready", version);
            } else {
                tracing::warn!("bootstrap: {} yarn failed or unsupported", version);
            }
            if vanilla {
                tracing::info!("bootstrap: {} vanilla ready", version);
            }
        }
        tracing::info!("bootstrap: done");
    });
}

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
    bootstrap_mappings(&config);
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
use spinyarn::api::build_router;
use spinyarn::config::Config;
use spinyarn::mapping::dispatcher::{self, MappingType};
use spinyarn::mapping::download::{ensure_mapping, ensure_vanilla_mapping};
use spinyarn::START_TIME;
use std::path::Path;
use std::sync::atomic::Ordering;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Ensure a mapping file exists locally, downloading it if missing (both Yarn
/// and Vanilla families). Runs inside a `spawn_blocking` task; missing files
/// are backfilled, existing ones are left untouched.
fn ensure_one(version: &str, mappings_dir: &str) {
    let yarn = dispatcher::local_path(version, mappings_dir, MappingType::Yarn);
    if !Path::new(&yarn).exists() {
        match ensure_mapping(version, mappings_dir, false) {
            Ok(true) => tracing::info!("bootstrap: {} yarn ready", version),
            _ => tracing::warn!("bootstrap: {} yarn failed or unsupported", version),
        }
    }

    let vanilla = dispatcher::local_path(version, mappings_dir, MappingType::Vanilla);
    if !Path::new(&vanilla).exists() {
        match ensure_vanilla_mapping(version, mappings_dir, false) {
            Ok(true) => tracing::info!("bootstrap: {} vanilla ready", version),
            Ok(false) => tracing::debug!("bootstrap: {} vanilla unavailable (no official mapping)", version),
            Err(e) => tracing::warn!("bootstrap: {} vanilla failed: {}", version, e),
        }
    }
}

/// Spawn a background bootstrap that backfills every missing mapping from the
/// configured version list (both Yarn and Vanilla families) when
/// `maven.auto_download` is on. Runs off the request path; the server keeps
/// serving while it fills in.
fn bootstrap_mappings(config: &Config) {
    if !config.maven.auto_download {
        return;
    }
    let versions = config.maven.bootstrap_versions.clone();
    let mappings_dir = config.maven.mappings_dir.clone();
    tokio::spawn(async move {
        tracing::info!("bootstrap: checking {} version(s)", versions.len());
        for version in versions {
            let dir = mappings_dir.clone();
            let v = version.clone();
            tokio::task::spawn_blocking(move || ensure_one(&v, &dir))
                .await
                .ok();
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
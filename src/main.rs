use spinyarn::api::build_router;
use spinyarn::config::{Config, MAX_BODY_SIZE_LIMIT};
use spinyarn::{Spinyarn, START_TIME};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let start_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    START_TIME.store(start_ts, Ordering::Relaxed);

    let mut config: Config = Config::load();
    // Clamp the concurrency gate: 0 would deadlock every load, huge values
    // would allow unbounded ~10MB table sets (OOM). Warn and fix up.
    if config.server.max_concurrency == 0 {
        tracing::warn!("max_concurrency is 0, falling back to default 32");
        config.server.max_concurrency = 32;
    } else if config.server.max_concurrency > 1024 {
        tracing::warn!(
            "max_concurrency {} exceeds 1024, clamping to 1024",
            config.server.max_concurrency
        );
        config.server.max_concurrency = 1024;
    }
    // Upper bound for request bodies (256MB); larger values risk OOM.
    if config.server.max_body_size > MAX_BODY_SIZE_LIMIT {
        tracing::warn!(
            "max_body_size {} exceeds 256MB, falling back to 256MB",
            config.server.max_body_size
        );
        config.server.max_body_size = MAX_BODY_SIZE_LIMIT;
    }
    tracing::info!("External mappings dir: {}", config.maven.mappings_dir);
    // Web API parses mappings on demand with no local cache; the shared
    // (Redis) cache only exists in C ABI / PHP embeds. Do not log
    // `cache.enabled=true` as if the Web API cached — it misleads operators.
    tracing::info!(
        "Web API 按需解析、无本地缓存 (max_concurrency={}, max_body_size={})",
        config.server.max_concurrency,
        config.server.max_body_size
    );

    let spinyarn = Arc::new(Spinyarn::new(&config));
    let app = build_router(config.clone(), spinyarn);

    // Bind with port-auto-increment: if the configured/default port is taken,
    // try the next one until a free port is found.
    let host = config.server.host.clone();
    let mut port = config.server.port;
    let (listener, addr) = loop {
        let parsed: std::net::IpAddr = match host.parse() {
            Ok(ip) => ip,
            Err(e) => {
                tracing::error!("invalid host {}: {}", host, e);
                std::process::exit(1);
            }
        };
        let addr = std::net::SocketAddr::new(parsed, port);
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
            Err(e) => {
                tracing::error!("failed to bind {}: {}", addr, e);
                std::process::exit(1);
            }
        }
    };
    tracing::info!("SpinYarn listening on {}", addr);

    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!("server error: {}", e);
        std::process::exit(1);
    }
}

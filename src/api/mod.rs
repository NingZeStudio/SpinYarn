use axum::{extract::DefaultBodyLimit, Router};
use tower_http::cors::{Any, CorsLayer};

use crate::config::Config;

mod deobfuscate;
mod health;
mod response;

pub fn build_router(_config: Config) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let api_routes = Router::new()
        .route(
            "/api/v1/deobfuscate",
            axum::routing::post(deobfuscate::handler)
                .layer(DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
        .route("/api/v1/health", axum::routing::get(health::handler));

    Router::new().merge(api_routes).layer(cors)
}

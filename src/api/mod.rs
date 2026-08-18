use axum::{extract::DefaultBodyLimit, response::Response, Json, Router};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing::{debug_span, info_span, Span};
use utoipa::OpenApi;

use crate::config::Config;
use crate::Spinyarn;

mod deobfuscate;
mod health;
mod mappings;
mod response;

/// OpenAPI 3.0 document for all endpoints.
#[derive(OpenApi)]
#[openapi(
    paths(
        deobfuscate::handler,
        deobfuscate::handler_plain,
        health::handler,
        mappings::load_mapping,
        mappings::load_mapping_local,
        mappings::list_mappings,
        mappings::mapping_stats,
        mappings::unload_mapping,
    ),
    components(schemas(
        deobfuscate::DeobfuscateRequest,
        deobfuscate::DeobfuscateResponse,
        deobfuscate::DeobfuscateStats,
        health::HealthResponse,
        mappings::LoadRequest,
        mappings::LoadLocalRequest,
        mappings::LoadedInfo,
        mappings::MappingsList,
        mappings::MappingStats,
        mappings::UnloadInfo,
        crate::cache::CacheStats,
    )),
    info(
        title = "SpinYarn API",
        description = "Minecraft 日志反混淆服务：Fabric(Yarn) 与 Vanilla(Mojang official) 映射支持。",
        // utoipa's macro only accepts a literal here; keep in sync with Cargo.toml.
        version = "0.9.0"
    )
)]
struct ApiDoc;

/// Router-wide state: the deobfuscation concurrency gate and the shared engine
/// (which owns the mappings dir, auto-download toggle, and LRU cache).
#[derive(Clone)]
pub struct AppState {
    pub gate: Arc<Semaphore>,
    pub spinyarn: Arc<Spinyarn>,
}

fn make_info_span(request: &axum::extract::Request) -> Span {
    info_span!(
        "http_request",
        method = %request.method(),
        uri = %request.uri(),
        status = tracing::field::Empty
    )
}

// Health is probed frequently; log it at DEBUG to keep the access log quiet.
fn make_debug_span(request: &axum::extract::Request) -> Span {
    debug_span!(
        "http_request",
        method = %request.method(),
        uri = %request.uri(),
        status = tracing::field::Empty
    )
}

fn on_response_info(response: &Response, latency: Duration, span: &Span) {
    span.record("status", response.status().as_u16());
    tracing::info!(
        parent: span,
        latency_ms = latency.as_millis() as u64,
        "request finished"
    );
}

fn on_response_debug(response: &Response, latency: Duration, span: &Span) {
    span.record("status", response.status().as_u16());
    tracing::debug!(
        parent: span,
        latency_ms = latency.as_millis() as u64,
        "request finished"
    );
}

/// GET /api/v1/openapi.json — OpenAPI 3.0 规范文档。
async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    let mut api = ApiDoc::openapi();
    // utoipa's `info(version = ...)` only accepts a literal, so stamp the real
    // version here at runtime to guarantee it never drifts from Cargo.toml.
    let info = &mut api.info;
    info.version = env!("CARGO_PKG_VERSION").to_string();
    Json(api)
}

pub fn build_router(config: Config, spinyarn: Arc<Spinyarn>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let info_trace = TraceLayer::new_for_http()
        .make_span_with(make_info_span)
        .on_response(on_response_info);
    let debug_trace = TraceLayer::new_for_http()
        .make_span_with(make_debug_span)
        .on_response(on_response_debug);

    let deobfuscate_route: axum::routing::MethodRouter<AppState> =
        axum::routing::post(deobfuscate::handler)
            .layer(DefaultBodyLimit::max(config.server.max_body_size));
    let deobfuscate_route: axum::routing::MethodRouter<AppState> =
        deobfuscate_route.layer(info_trace.clone());

    let deobfuscate_plain_route: axum::routing::MethodRouter<AppState> =
        axum::routing::post(deobfuscate::handler_plain)
            .layer(DefaultBodyLimit::max(config.server.max_body_size));
    let deobfuscate_plain_route: axum::routing::MethodRouter<AppState> =
        deobfuscate_plain_route.layer(info_trace.clone());

    let load_route: axum::routing::MethodRouter<AppState> = axum::routing::post(mappings::load_mapping);
    let load_local_route: axum::routing::MethodRouter<AppState> =
        axum::routing::post(mappings::load_mapping_local);
    let list_route: axum::routing::MethodRouter<AppState> = axum::routing::get(mappings::list_mappings);
    let stats_route: axum::routing::MethodRouter<AppState> = axum::routing::get(mappings::mapping_stats);
    let unload_route: axum::routing::MethodRouter<AppState> = axum::routing::delete(mappings::unload_mapping);

    let api_routes = Router::new()
        .route("/api/v1/deobfuscate", deobfuscate_route)
        .route("/api/v1/deobfuscate/plain", deobfuscate_plain_route)
        .route("/api/v1/mappings/load", load_route)
        .route("/api/v1/mappings/load/local", load_local_route)
        .route("/api/v1/mappings", list_route)
        .route("/api/v1/mappings/:type/:version", stats_route)
        .route("/api/v1/mappings/:version", unload_route)
        .route("/api/v1/openapi.json", axum::routing::get(openapi_json))
        .route(
            "/api/v1/health",
            axum::routing::get(health::handler).layer(debug_trace),
        )
        .with_state(AppState {
            gate: Arc::new(Semaphore::new(config.server.max_concurrency)),
            spinyarn,
        });
    Router::new().merge(api_routes).layer(cors)
}
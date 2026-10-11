use axum::{
    extract::DefaultBodyLimit,
    http::{HeaderValue, Method},
    response::Response,
    Json, Router,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tower_http::{
    cors::{AllowOrigin, Any, CorsLayer},
    trace::TraceLayer,
};
use tracing::{debug_span, info_span, Span};
use utoipa::OpenApi;

use crate::config::Config;
use crate::error::ApiError;
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
        mappings::LoadInfo,
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
        version = "1.0.0"
    )
)]
struct ApiDoc;

/// Router-wide state: the deobfuscation concurrency gate, the shared engine
/// (which owns the mappings dir and optional Redis cache), the admin token
/// for write endpoints, and the per-key singleflight table.
#[derive(Clone)]
pub struct AppState {
    pub gate: Arc<Semaphore>,
    pub spinyarn: Arc<Spinyarn>,
    pub admin_token: String,
    /// In-flight per `(type, version)` mutexes. With a shared cache this
    /// de-duplicates concurrent loads of the same key (second waiter hits the
    /// cache after the first fills it); without a cache it serializes same-key
    /// loads so a burst of identical versions cannot OOM with N parallel
    /// ~10MB table parses.
    pub inflight: Arc<tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

/// Constant-time token comparison (no `subtle` dependency).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Extract the caller token from `X-Admin-Token` or `Authorization: Bearer`.
fn extract_admin_token(headers: &axum::http::HeaderMap) -> Option<String> {
    if let Some(v) = headers.get("x-admin-token") {
        if let Ok(s) = v.to_str() {
            let s = s.trim();
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    if let Some(v) = headers.get(axum::http::header::AUTHORIZATION) {
        if let Ok(s) = v.to_str() {
            let s = s.trim();
            if let Some(tok) = s.strip_prefix("Bearer ") {
                let tok = tok.trim();
                if !tok.is_empty() {
                    return Some(tok.to_string());
                }
            }
        }
    }
    None
}

/// Guard for the write mapping endpoints (`load`, `load/local`, `unload`).
/// Reads (`deobfuscate`, `list`, `stats`, `health`, `openapi`) stay open.
/// Token source: `[admin] token` in config.toml,
/// overridden by `SPINYARN_ADMIN_TOKEN` (env wins, applied in
/// `Config::load`). Empty/unconfigured → 403 + warn (fail closed).
pub fn require_admin(headers: &axum::http::HeaderMap, expected: &str) -> Result<(), ApiError> {
    if expected.is_empty() {
        tracing::warn!("admin token not configured; rejecting write operation");
        return Err(ApiError::Forbidden(
            "admin token not configured".to_string(),
        ));
    }
    match extract_admin_token(headers) {
        Some(got) if constant_time_eq(got.as_bytes(), expected.as_bytes()) => Ok(()),
        _ => {
            tracing::warn!("forbidden write operation: bad admin token");
            Err(ApiError::Forbidden("forbidden".to_string()))
        }
    }
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

/// Permissive CORS for public (read/open) routes. These endpoints take no
/// side effects, so cross-origin browser calls are harmless.
fn public_cors() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
}

/// Restrictive CORS for write (admin) routes: only `server.cors_origins`
/// origins are allowed. Default (empty whitelist) emits no
/// `Access-Control-Allow-Origin`, i.e. browsers reject cross-origin writes.
fn admin_cors(origins: &[String]) -> CorsLayer {
    let methods = [Method::GET, Method::POST, Method::DELETE, Method::OPTIONS];
    if origins.is_empty() {
        CorsLayer::new().allow_methods(methods).allow_headers(Any)
    } else {
        let list: Vec<HeaderValue> = origins.iter().filter_map(|s| s.parse().ok()).collect();
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(list))
            .allow_methods(methods)
            .allow_headers(Any)
    }
}

pub fn build_router(config: Config, spinyarn: Arc<Spinyarn>) -> Router {
    let info_trace = TraceLayer::new_for_http()
        .make_span_with(make_info_span)
        .on_response(on_response_info);
    let debug_trace = TraceLayer::new_for_http()
        .make_span_with(make_debug_span)
        .on_response(on_response_debug);

    let state = AppState {
        gate: Arc::new(Semaphore::new(config.server.max_concurrency)),
        spinyarn,
        admin_token: config.admin.token.clone(),
        inflight: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
    };

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

    let load_route: axum::routing::MethodRouter<AppState> =
        axum::routing::post(mappings::load_mapping);
    let load_local_route: axum::routing::MethodRouter<AppState> =
        axum::routing::post(mappings::load_mapping_local);
    let unload_route: axum::routing::MethodRouter<AppState> =
        axum::routing::delete(mappings::unload_mapping);

    let list_route: axum::routing::MethodRouter<AppState> =
        axum::routing::get(mappings::list_mappings);
    let stats_route: axum::routing::MethodRouter<AppState> =
        axum::routing::get(mappings::mapping_stats);

    // Public (open) routes keep the permissive CORS policy.
    let public_routes = Router::new()
        .route("/api/v1/deobfuscate", deobfuscate_route)
        .route("/api/v1/deobfuscate/plain", deobfuscate_plain_route)
        .route("/api/v1/mappings", list_route)
        .route("/api/v1/mappings/:type/:version", stats_route)
        .route("/api/v1/openapi.json", axum::routing::get(openapi_json))
        .route(
            "/api/v1/health",
            axum::routing::get(health::handler).layer(debug_trace),
        )
        .layer(public_cors());

    // Write (admin) routes use the whitelist CORS policy.
    let admin_routes = Router::new()
        .route("/api/v1/mappings/load", load_route)
        .route("/api/v1/mappings/load/local", load_local_route)
        .route("/api/v1/mappings/:version", unload_route)
        .layer(admin_cors(&config.server.cors_origins));

    Router::new()
        .merge(public_routes)
        .merge(admin_routes)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with(token: &str) -> axum::http::HeaderMap {
        let mut h = axum::http::HeaderMap::new();
        h.insert("x-admin-token", token.parse().unwrap());
        h
    }

    #[test]
    fn test_require_admin_unconfigured_is_forbidden() {
        let h = headers_with("anything");
        assert!(require_admin(&h, "").is_err());
        let empty = axum::http::HeaderMap::new();
        assert!(require_admin(&empty, "").is_err());
    }

    #[test]
    fn test_require_admin_accepts_both_headers() {
        assert!(require_admin(&headers_with("s3cret"), "s3cret").is_ok());
        let mut h = axum::http::HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer s3cret".parse().unwrap(),
        );
        assert!(require_admin(&h, "s3cret").is_ok());
        assert!(require_admin(&headers_with("wrong"), "s3cret").is_err());
        let empty = axum::http::HeaderMap::new();
        assert!(require_admin(&empty, "s3cret").is_err());
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }
}

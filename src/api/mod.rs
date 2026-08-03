use axum::{extract::DefaultBodyLimit, response::Response, Router};
use std::time::Duration;
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing::{debug_span, info_span, Span};

use crate::config::Config;

mod deobfuscate;
mod health;
mod response;

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

pub fn build_router(_config: Config) -> Router {
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

    let deobfuscate_route: axum::routing::MethodRouter = axum::routing::post(deobfuscate::handler)
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024));
    let deobfuscate_route: axum::routing::MethodRouter = deobfuscate_route.layer(info_trace.clone());

    let deobfuscate_plain_route: axum::routing::MethodRouter =
        axum::routing::post(deobfuscate::handler_plain)
            .layer(DefaultBodyLimit::max(64 * 1024 * 1024));
    let deobfuscate_plain_route: axum::routing::MethodRouter =
        deobfuscate_plain_route.layer(info_trace.clone());

    let api_routes = Router::new()
        .route("/api/v1/deobfuscate", deobfuscate_route)
        .route("/api/v1/deobfuscate/plain", deobfuscate_plain_route)
        .route(
            "/api/v1/health",
            axum::routing::get(health::handler).layer(debug_trace),
        );

    Router::new().merge(api_routes).layer(cors)
}
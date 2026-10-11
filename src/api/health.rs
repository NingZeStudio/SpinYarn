use axum::extract::State;
use axum::Json;
use serde::Serialize;
use std::sync::atomic::Ordering;
use utoipa::ToSchema;

use crate::api::AppState;
use crate::cache::CacheStats;

#[derive(Serialize, ToSchema)]
pub struct HealthResponse {
    pub status: &'static str,
    pub uptime_seconds: u64,
    /// Shared-cache stats. The Web API runs on-demand with no local cache, so
    /// this stays `None` in the default deployment; only Redis-backed embeds
    /// report `Some`. Kept for backward compatibility.
    pub cache: Option<CacheStats>,
    /// Cumulative mapping load failures (corrupt/unreadable files).
    pub load_errors: u64,
}

#[utoipa::path(
    get,
    path = "/api/v1/health",
    responses((status = 200, body = crate::api::response::ApiResponse<HealthResponse>))
)]
#[axum::debug_handler]
pub async fn handler(
    state: State<AppState>,
) -> Json<crate::api::response::ApiResponse<HealthResponse>> {
    let start_secs = crate::START_TIME.load(Ordering::Relaxed);
    let uptime = if start_secs > 0 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(start_secs)
    } else {
        0
    };

    let cache = state.spinyarn.cache_stats();
    let load_errors = state.spinyarn.load_errors();

    Json(crate::api::response::ApiResponse::success(HealthResponse {
        status: "healthy",
        uptime_seconds: uptime,
        cache,
        load_errors,
    }))
}

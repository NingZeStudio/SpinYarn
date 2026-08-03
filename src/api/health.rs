use axum::extract::State;
use axum::Json;
use serde::Serialize;
use std::sync::atomic::Ordering;

use crate::api::AppState;

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub uptime_seconds: u64,
}

#[axum::debug_handler]
pub async fn handler(
    _state: State<AppState>,
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

    Json(crate::api::response::ApiResponse::success(HealthResponse {
        status: "healthy",
        uptime_seconds: uptime,
    }))
}

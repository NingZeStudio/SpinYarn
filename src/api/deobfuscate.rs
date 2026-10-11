use axum::{
    extract::{rejection::JsonRejection, State},
    http::{header, HeaderMap, HeaderValue},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use utoipa::ToSchema;

use crate::api::AppState;
use crate::error::ApiError;
use crate::mapping::dispatcher::MappingType;
use crate::{DeobfuscateOutput, Spinyarn};

#[derive(Debug, Deserialize, ToSchema)]
pub struct DeobfuscateRequest {
    pub content: String,
    pub version: String,
    /// Mapping family: `yarn` (default) or `vanilla` (case-insensitive).
    #[serde(default)]
    pub mapping_type: String,
}

#[derive(serde::Serialize, ToSchema)]
pub struct DeobfuscateResponse {
    pub deobfuscated: String,
    pub stats: DeobfuscateStats,
}

#[derive(serde::Serialize, ToSchema)]
pub struct DeobfuscateStats {
    pub version: String,
    pub classes_mapped: usize,
    pub methods_mapped: usize,
    pub fields_mapped: usize,
    pub total_time_ms: f64,
}

struct DeobfuscateOutcome {
    text: String,
    stats: DeobfuscateStats,
}

fn passthrough_stats(version: String) -> DeobfuscateStats {
    DeobfuscateStats {
        version,
        classes_mapped: 0,
        methods_mapped: 0,
        fields_mapped: 0,
        total_time_ms: 0.0,
    }
}

fn outcome_from(output: DeobfuscateOutput, version: String) -> DeobfuscateOutcome {
    DeobfuscateOutcome {
        text: output.deobfuscated,
        stats: DeobfuscateStats {
            version,
            classes_mapped: output.classes_mapped,
            methods_mapped: output.methods_mapped,
            fields_mapped: output.fields_mapped,
            total_time_ms: output.total_time_ms,
        },
    }
}

async fn remove_inflight(state: &AppState, key: &str) {
    state.inflight.lock().await.remove(key);
}

/// Shared pipeline for both JSON and plain-text handlers, delegating to the
/// `Spinyarn` facade. The concurrency gate wraps only the load step; cache hits
/// and the ensure step stay gate-free (nothing loads on a hit).
async fn process(
    req: DeobfuscateRequest,
    state: &AppState,
) -> Result<DeobfuscateOutcome, ApiError> {
    let mtype = MappingType::parse_result(&req.mapping_type).ok_or_else(|| {
        ApiError::BadRequest(format!("unknown mapping_type: {:?}", req.mapping_type))
    })?;
    let spinyarn = state.spinyarn.clone();

    // 1. Ensure the mapping file is available locally, off the gate.
    let version = req.version.clone();
    let engine = spinyarn.clone();
    let available = tokio::task::spawn_blocking(move || engine.ensure_available(&version, mtype))
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !available {
        return Ok(DeobfuscateOutcome {
            text: req.content,
            stats: passthrough_stats(req.version),
        });
    }

    // 2. Fast-path cache hit: deobfuscate directly, no gate (nothing loads).
    if let Some(shared) = spinyarn.get_cached(&req.version, mtype) {
        let content = req.content;
        let deobfuscated =
            tokio::task::spawn_blocking(move || Spinyarn::deobfuscate_loaded(&shared, &content))
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
        return Ok(outcome_from(deobfuscated, req.version));
    }

    // 3. Per-key singleflight: with a shared cache this de-duplicates
    // concurrent loads of the same key (waiters re-check the cache below and
    // hit); without a cache it serializes same-key loads so a burst of
    // identical versions cannot hold N parallel ~10MB table sets (OOM guard).
    let key = mtype.cache_key(&req.version);
    let key_mutex = {
        let mut table = state.inflight.lock().await;
        table
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    let _key_guard = key_mutex.lock().await;

    // Re-check after acquiring the per-key lock: another in-flight task may
    // have filled the cache while we waited.
    if let Some(shared) = spinyarn.get_cached(&req.version, mtype) {
        let content = req.content;
        let deobfuscated =
            tokio::task::spawn_blocking(move || Spinyarn::deobfuscate_loaded(&shared, &content))
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
        drop(_key_guard);
        remove_inflight(state, &key).await;
        return Ok(outcome_from(deobfuscated, req.version));
    }

    // 4. Bound peak memory: at most N in-flight mapping table sets. Wait at
    // most 30s, then shed load with 429 + `Retry-After: 30`.
    let permit = tokio::time::timeout(Duration::from_secs(30), state.gate.acquire()).await;
    let _permit = match permit {
        Err(_) => {
            drop(_key_guard);
            remove_inflight(state, &key).await;
            return Err(ApiError::TooManyRequests(
                "server busy, retry later".to_string(),
            ));
        }
        Ok(Err(e)) => {
            drop(_key_guard);
            remove_inflight(state, &key).await;
            return Err(ApiError::Internal(e.to_string()));
        }
        Ok(Ok(p)) => p,
    };

    // 5. CPU-bound: load + parse, then cache and deobfuscate.
    let version = req.version.clone();
    let content = req.content;
    let engine = spinyarn.clone();
    let loaded = tokio::task::spawn_blocking(move || engine.load(&version, mtype))
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let Some(loaded) = loaded else {
        drop(_key_guard);
        remove_inflight(state, &key).await;
        return Ok(DeobfuscateOutcome {
            text: content,
            stats: passthrough_stats(req.version),
        });
    };

    let shared = std::sync::Arc::new(loaded);
    spinyarn.insert_cached(&req.version, mtype, &shared);

    let deobfuscated =
        tokio::task::spawn_blocking(move || Spinyarn::deobfuscate_loaded(&shared, &content))
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

    drop(_key_guard);
    remove_inflight(state, &key).await;
    Ok(outcome_from(deobfuscated, req.version))
}

#[utoipa::path(
    post,
    path = "/api/v1/deobfuscate",
    request_body = DeobfuscateRequest,
    responses(
        (status = 200, body = crate::api::response::ApiResponse<DeobfuscateResponse>),
        (status = 400, description = "unknown mapping_type or malformed JSON"),
        (status = 429, description = "concurrency gate timeout, retry after 30s")
    )
)]
#[axum::debug_handler]
pub async fn handler(
    State(state): State<AppState>,
    req: Result<Json<DeobfuscateRequest>, JsonRejection>,
) -> Result<Json<crate::api::response::ApiResponse<DeobfuscateResponse>>, ApiError> {
    let Json(req) = req?;
    let outcome = process(req, &state).await?;
    Ok(Json(crate::api::response::ApiResponse::success(
        DeobfuscateResponse {
            deobfuscated: outcome.text,
            stats: outcome.stats,
        },
    )))
}

#[utoipa::path(
    post,
    path = "/api/v1/deobfuscate/plain",
    request_body = DeobfuscateRequest,
    responses(
        (status = 200, content_type = "text/plain"),
        (status = 400, description = "unknown mapping_type or malformed JSON"),
        (status = 429, description = "concurrency gate timeout, retry after 30s")
    )
)]
#[axum::debug_handler]
pub async fn handler_plain(
    State(state): State<AppState>,
    req: Result<Json<DeobfuscateRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(req) = req?;
    let outcome = process(req, &state).await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    Ok((headers, outcome.text).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unknown_mapping_type_is_none() {
        assert!(MappingType::parse_result("srg").is_none());
        assert!(MappingType::parse_result("MOJANG").is_none());
    }
}

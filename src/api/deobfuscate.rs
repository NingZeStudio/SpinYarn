use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;

use crate::{
    api::AppState,
    deobfuscator::LineEngine,
    error::ApiError,
    mapping::{download::{is_version_supported, load_mappings}, Mappings},
};

#[derive(Debug, Deserialize)]
pub struct DeobfuscateRequest {
    pub content: String,
    pub version: String,
}

#[derive(serde::Serialize)]
pub struct DeobfuscateResponse {
    pub deobfuscated: String,
    pub stats: DeobfuscateStats,
}

#[derive(serde::Serialize)]
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

/// Shared pipeline for both JSON and plain-text handlers:
/// passthrough check -> concurrency gate -> load mappings -> deobfuscate.
async fn process(req: DeobfuscateRequest, state: &AppState) -> Result<DeobfuscateOutcome, ApiError> {
    if !is_version_supported(&req.version, &state.mappings_dir) {
        return Ok(DeobfuscateOutcome {
            text: req.content,
            stats: passthrough_stats(req.version),
        });
    }

    // Bound peak memory: at most N in-flight mapping table sets.
    let _permit = state
        .gate
        .acquire()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let version = req.version.clone();
    let content = req.content;
    let mappings_dir = state.mappings_dir.clone();

    // CPU-bound: gzip decompress + parse.
    let loaded: Result<Option<Mappings>, ApiError> = tokio::task::spawn_blocking(move || {
        load_mappings(&version, &mappings_dir).map_err(|e| ApiError::Internal(e.to_string()))
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    let mappings = match loaded {
        Ok(Some(m)) => m,
        Ok(None) => {
            // Version declared supported but no mapping available -> pass through.
            return Ok(DeobfuscateOutcome {
                text: content,
                stats: passthrough_stats(req.version),
            });
        }
        Err(e) => return Err(e),
    };

    let deobfuscated = tokio::task::spawn_blocking(move || {
        let engine = LineEngine::new(mappings);
        engine.deobfuscate(&content)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(DeobfuscateOutcome {
        text: deobfuscated.text,
        stats: DeobfuscateStats {
            version: req.version,
            classes_mapped: deobfuscated.classes_mapped,
            methods_mapped: deobfuscated.methods_mapped,
            fields_mapped: deobfuscated.fields_mapped,
            total_time_ms: deobfuscated.total_time_ms,
        },
    })
}

#[axum::debug_handler]
pub async fn handler(
    State(state): State<AppState>,
    Json(req): Json<DeobfuscateRequest>,
) -> Result<Json<crate::api::response::ApiResponse<DeobfuscateResponse>>, ApiError> {
    let outcome = process(req, &state).await?;
    Ok(Json(crate::api::response::ApiResponse::success(
        DeobfuscateResponse {
            deobfuscated: outcome.text,
            stats: outcome.stats,
        },
    )))
}

#[axum::debug_handler]
pub async fn handler_plain(
    State(state): State<AppState>,
    Json(req): Json<DeobfuscateRequest>,
) -> Result<Response, ApiError> {
    let outcome = process(req, &state).await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    Ok((headers, outcome.text).into_response())
}

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
    mapping::download::{ensure_mapping, is_downloadable_version, is_version_supported, load_mappings},
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
/// passthrough check -> (auto-download) -> cache lookup -> gate/load -> deobfuscate.
async fn process(req: DeobfuscateRequest, state: &AppState) -> Result<DeobfuscateOutcome, ApiError> {
    if !is_version_supported(&req.version, &state.mappings_dir) {
        // Missing locally: if it's a downloadable 1.x token and auto-download is
        // enabled, try to fetch it once; otherwise fall through to passthrough.
        if state.auto_download && is_downloadable_version(&req.version) {
            let version = req.version.clone();
            let mappings_dir = state.mappings_dir.clone();
            let ready = tokio::task::spawn_blocking(move || ensure_mapping(&version, &mappings_dir))
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?
                .map_err(|e| ApiError::Internal(e.to_string()))?;
            if !ready {
                return Ok(DeobfuscateOutcome {
                    text: req.content,
                    stats: passthrough_stats(req.version),
                });
            }
        } else {
            return Ok(DeobfuscateOutcome {
                text: req.content,
                stats: passthrough_stats(req.version),
            });
        }
    }

    // Cache hit: share the already-parsed table, skip the gate (nothing loads).
    if let Some(cache) = &state.cache {
        if let Some(shared) = cache.get(&req.version) {
            let content = req.content;
            let deobfuscated = tokio::task::spawn_blocking(move || {
                LineEngine::from_arc(shared).deobfuscate(&content)
            })
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
            return Ok(DeobfuscateOutcome {
                text: deobfuscated.text,
                stats: DeobfuscateStats {
                    version: req.version,
                    classes_mapped: deobfuscated.classes_mapped,
                    methods_mapped: deobfuscated.methods_mapped,
                    fields_mapped: deobfuscated.fields_mapped,
                    total_time_ms: deobfuscated.total_time_ms,
                },
            });
        }
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
    let loaded = tokio::task::spawn_blocking(move || load_mappings(&version, &mappings_dir))
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let mappings = match loaded {
        Some(m) => m,
        None => {
            // Version declared supported but no mapping available -> pass through.
            return Ok(DeobfuscateOutcome {
                text: content,
                stats: passthrough_stats(req.version),
            });
        }
    };

    // Populate the cache with the parsed table, sharing the same Arc we deobfuscate with.
    let shared = std::sync::Arc::new(mappings);
    if let Some(cache) = &state.cache {
        cache.insert(&req.version, std::sync::Arc::clone(&shared));
    }

    let deobfuscated = tokio::task::spawn_blocking(move || {
        LineEngine::from_arc(shared).deobfuscate(&content)
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

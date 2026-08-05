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
    mapping::{
        dispatcher::{self, MappingType},
        download::{ensure_mapping, is_downloadable_version},
    },
};

#[derive(Debug, Deserialize)]
pub struct DeobfuscateRequest {
    pub content: String,
    pub version: String,
    /// Mapping family: `yarn` (default) or `vanilla`.
    #[serde(default)]
    pub mapping_type: String,
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

fn outcome_from(result: crate::deobfuscator::DeobfuscateResult, version: String) -> DeobfuscateOutcome {
    DeobfuscateOutcome {
        text: result.text,
        stats: DeobfuscateStats {
            version,
            classes_mapped: result.classes_mapped,
            methods_mapped: result.methods_mapped,
            fields_mapped: result.fields_mapped,
            total_time_ms: result.total_time_ms,
        },
    }
}

/// Shared pipeline for both JSON and plain-text handlers:
/// passthrough check -> (auto-download) -> cache lookup -> gate/load -> deobfuscate.
async fn process(req: DeobfuscateRequest, state: &AppState) -> Result<DeobfuscateOutcome, ApiError> {
    let mtype = MappingType::parse(&req.mapping_type);

    if !dispatcher::is_supported(&req.version, &state.mappings_dir, mtype) {
        // Auto-download only applies to Yarn (Vanilla data source not wired yet).
        if mtype == MappingType::Yarn
            && state.auto_download
            && is_downloadable_version(&req.version)
        {
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

    let cache_key = mtype.cache_key(&req.version);

    // Cache hit: share the already-parsed table, skip the gate (nothing loads).
    if let Some(cache) = &state.cache {
        if let Some(shared) = cache.get(&cache_key) {
            let content = req.content;
            let deobfuscated = tokio::task::spawn_blocking(move || {
                dispatcher::deobfuscate(&*shared, &content)
            })
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
            return Ok(outcome_from(deobfuscated, req.version));
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

    // CPU-bound: load + parse the requested mapping family.
    let loaded = tokio::task::spawn_blocking(move || dispatcher::load(&version, &mappings_dir, mtype))
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .map_err(ApiError::Internal)?;

    let Some(loaded) = loaded else {
        // Declared supported but no mapping available -> pass through.
        return Ok(DeobfuscateOutcome {
            text: content,
            stats: passthrough_stats(req.version),
        });
    };

    // Populate the cache with the parsed table, sharing the same Arc we deobfuscate with.
    let shared = std::sync::Arc::new(loaded);
    if let Some(cache) = &state.cache {
        cache.insert(&cache_key, std::sync::Arc::clone(&shared));
    }

    let deobfuscated = tokio::task::spawn_blocking(move || {
        dispatcher::deobfuscate(&*shared, &content)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(outcome_from(deobfuscated, req.version))
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

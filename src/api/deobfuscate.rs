use axum::Json;
use serde::Deserialize;

use crate::{
    deobfuscator::LineEngine,
    error::ApiError,
    mapping::{download::load_mappings, Mappings},
};

#[derive(Debug, Deserialize)]
pub struct DeobfuscateRequest {
    pub content: String,
    pub version: String,
}

#[derive(serde::Serialize)]
pub struct DeobfuscateResponse {
    pub original: String,
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

fn passthrough(req: DeobfuscateRequest) -> DeobfuscateResponse {
    DeobfuscateResponse {
        original: req.content.clone(),
        deobfuscated: req.content,
        stats: DeobfuscateStats {
            version: req.version,
            classes_mapped: 0,
            methods_mapped: 0,
            fields_mapped: 0,
            total_time_ms: 0.0,
        },
    }
}

#[axum::debug_handler]
pub async fn handler(
    Json(req): Json<DeobfuscateRequest>,
) -> Result<Json<crate::api::response::ApiResponse<DeobfuscateResponse>>, ApiError> {
    if !crate::config::SUPPORTED_VERSIONS.contains(req.version.as_str()) {
        return Ok(Json(crate::api::response::ApiResponse::success(passthrough(req))));
    }

    let version = req.version.clone();
    let content = req.content.clone();

    // CPU-bound: gzip decompress + parse.
    let loaded: Result<Option<Mappings>, ApiError> = tokio::task::spawn_blocking(move || {
        load_mappings(&version).map_err(|e| ApiError::Internal(e.to_string()))
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    let mappings = match loaded {
        Ok(Some(m)) => m,
        Ok(None) => {
            // Version declared supported but no mapping available -> pass through.
            return Ok(Json(crate::api::response::ApiResponse::success(passthrough(req))));
        }
        Err(e) => return Err(e),
    };

    let deobfuscated = tokio::task::spawn_blocking(move || {
        let engine = LineEngine::new(mappings);
        engine.deobfuscate(&content)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(crate::api::response::ApiResponse::success(
        DeobfuscateResponse {
            original: req.content,
            deobfuscated: deobfuscated.text,
            stats: DeobfuscateStats {
                version: req.version,
                classes_mapped: deobfuscated.classes_mapped,
                methods_mapped: deobfuscated.methods_mapped,
                fields_mapped: deobfuscated.fields_mapped,
                total_time_ms: deobfuscated.total_time_ms,
            },
        },
    )))
}

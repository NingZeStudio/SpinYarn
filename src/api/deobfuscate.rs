use axum::Json;
use once_cell::sync::Lazy;
use serde::Deserialize;
use tokio::sync::Semaphore;

use crate::{
    deobfuscator::LineEngine,
    error::ApiError,
    mapping::{download::load_mappings, Mappings},
};

/// Concurrency gate for the heavy load+deobfuscate path.
///
/// There is no mapping cache by design: each concurrent request holds a full
/// per-version table set (~30MB). On memory-constrained hosts, unbounded
/// `spawn_blocking` would multiply that per in-flight request. The semaphore
/// pins peak memory to `SPINYARN_MAX_CONCURRENCY` x one version.
///
/// Default 8: LogShare's real traffic is ~1600 RPM (~27 req/s), which at
/// ~150ms/request means a steady-state concurrency of ~4. A limit of 8 never
/// engages in steady state; it only converts burst OOM risk (~30MB per
/// in-flight request) into short queueing.
static GATE: Lazy<Semaphore> = Lazy::new(|| {
    let n = std::env::var("SPINYARN_MAX_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    Semaphore::new(n)
});

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

fn passthrough(req: DeobfuscateRequest) -> DeobfuscateResponse {
    DeobfuscateResponse {
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

    // Bound peak memory: at most N in-flight mapping table sets.
    let _permit = GATE
        .acquire()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let version = req.version.clone();

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

    let content = req.content;
    let deobfuscated = tokio::task::spawn_blocking(move || {
        let engine = LineEngine::new(mappings);
        engine.deobfuscate(&content)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(crate::api::response::ApiResponse::success(
        DeobfuscateResponse {
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

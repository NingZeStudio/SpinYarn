use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use std::path::{Path as FsPath, PathBuf};
use utoipa::ToSchema;

use crate::api::response::ApiResponse;
use crate::api::AppState;
use crate::error::ApiError;
use crate::mapping::dispatcher::{self, MappingType};
use crate::mapping::download::{ensure_mapping, ensure_vanilla_mapping};

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoadRequest {
    pub version: String,
    #[serde(default)]
    pub mapping_type: Option<String>,
    #[serde(default)]
    pub refresh: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoadLocalRequest {
    pub version: String,
    #[serde(default)]
    pub mapping_type: Option<String>,
    /// Path relative to the mappings dir (e.g. `./tmp/1.21.4.tiny.gz`).
    pub path: String,
}

#[derive(Serialize, ToSchema)]
pub struct LoadedInfo {
    pub version: String,
    pub mapping_type: String,
    pub ok: bool,
    pub path: String,
    pub bytes: u64,
}

/// POST /api/v1/mappings/load — fetch/refresh a version's mapping from its source.
#[utoipa::path(
    post,
    path = "/api/v1/mappings/load",
    request_body = LoadRequest,
    responses((status = 200, body = ApiResponse<LoadedInfo>))
)]
pub async fn load_mapping(
    State(state): State<AppState>,
    Json(req): Json<LoadRequest>,
) -> Result<Json<ApiResponse<LoadedInfo>>, ApiError> {
    let mtype = MappingType::parse(req.mapping_type.as_deref().unwrap_or(""));
    let force = req.refresh.unwrap_or(false);

    let version = req.version.clone();
    let mappings_dir = state.mappings_dir.clone();
    let ready = tokio::task::spawn_blocking(move || match mtype {
        MappingType::Yarn => ensure_mapping(&version, &mappings_dir, force),
        MappingType::Vanilla => ensure_vanilla_mapping(&version, &mappings_dir, force),
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    let path = dispatcher::local_path(&req.version, &state.mappings_dir, mtype);
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    Ok(Json(ApiResponse::success(LoadedInfo {
        version: req.version,
        mapping_type: mtype.as_str().to_string(),
        ok: ready,
        path: path.to_string_lossy().into_owned(),
        bytes,
    })))
}

/// POST /api/v1/mappings/load/local — load a mapping file from the local
/// filesystem into the standard location. The path is resolved relative to the
/// mappings dir and hardened against path traversal.
#[utoipa::path(
    post,
    path = "/api/v1/mappings/load/local",
    request_body = LoadLocalRequest,
    responses((status = 200, body = ApiResponse<LoadedInfo>))
)]
pub async fn load_mapping_local(
    State(state): State<AppState>,
    Json(req): Json<LoadLocalRequest>,
) -> Result<Json<ApiResponse<LoadedInfo>>, ApiError> {
    let mtype = MappingType::parse(req.mapping_type.as_deref().unwrap_or(""));

    // Resolve + canonicalize inside the mappings dir; reject any traversal.
    let source = safe_local_path(&state.mappings_dir, &req.path)?;

    let target = dispatcher::local_path(&req.version, &state.mappings_dir, mtype);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ApiError::Internal(format!("create dir: {e}")))?;
    }
    std::fs::copy(&source, &target)
        .map_err(|e| ApiError::Internal(format!("copy {} -> {}: {}", source.display(), target.display(), e)))?;

    Ok(Json(ApiResponse::success(LoadedInfo {
        version: req.version,
        mapping_type: mtype.as_str().to_string(),
        ok: true,
        path: target.to_string_lossy().into_owned(),
        bytes: std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0),
    })))
}

/// GET /api/v1/mappings — list locally cached mappings, grouped by type.
#[utoipa::path(get, path = "/api/v1/mappings", responses((status = 200, body = ApiResponse<MappingsList>)))]
pub async fn list_mappings(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<MappingsList>>, ApiError> {
    let base = FsPath::new(&state.mappings_dir);
    let mut yarn = Vec::new();
    if let Ok(entries) = std::fs::read_dir(base) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(v) = name.strip_suffix(".tiny.gz") {
                yarn.push(v.to_string());
            }
        }
    }
    let mut vanilla = Vec::new();
    if let Ok(entries) = std::fs::read_dir(base.join("vanilla")) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(v) = name.strip_suffix(".txt") {
                vanilla.push(v.to_string());
            }
        }
    }
    yarn.sort();
    vanilla.sort();
    Ok(Json(ApiResponse::success(MappingsList { yarn, vanilla })))
}

/// GET /api/v1/mappings/{type}/{version} — statistics for one mapping.
#[utoipa::path(
    get,
    path = "/api/v1/mappings/{type}/{version}",
    params(("type" = String, description = "yarn | vanilla"), ("version" = String, description = "Minecraft version")),
    responses((status = 200, body = ApiResponse<MappingStats>))
)]
pub async fn mapping_stats(
    State(state): State<AppState>,
    Path((mtype, version)): Path<(String, String)>,
) -> Result<Json<ApiResponse<MappingStats>>, ApiError> {
    let mtype = MappingType::parse(&mtype);
    let loaded = tokio::task::spawn_blocking({
        let version = version.clone();
        let dir = state.mappings_dir.clone();
        move || dispatcher::load(&version, &dir, mtype)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    let Some(loaded) = loaded else {
        return Err(ApiError::NotFound(format!(
            "{}:{} mapping not available locally",
            mtype.as_str(),
            version
        )));
    };

    let (classes, methods, fields, extra) = match &loaded {
        dispatcher::LoadedMappings::Yarn(m) => (
            m.classes.len(),
            m.methods.len(),
            m.fields.len(),
            format!("nested={}", m.nested.len()),
        ),
        dispatcher::LoadedMappings::Vanilla(m) => {
            let (c, me, f, idx) = m.stats();
            (c, me, f, format!("method_index={}", idx))
        }
    };

    Ok(Json(ApiResponse::success(MappingStats {
        version,
        mapping_type: mtype.as_str().to_string(),
        classes,
        methods,
        fields,
        extra,
    })))
}

/// DELETE /api/v1/mappings/{version} — unload a version: remove local files
/// (both families) and drop cache entries.
#[utoipa::path(
    delete,
    path = "/api/v1/mappings/{version}",
    params(("version" = String, description = "Minecraft version")),
    responses((status = 200, body = ApiResponse<UnloadInfo>))
)]
pub async fn unload_mapping(
    State(state): State<AppState>,
    Path(version): Path<String>,
) -> Result<Json<ApiResponse<UnloadInfo>>, ApiError> {
    let removed_files = dispatcher::remove_all_local(&version, &state.mappings_dir);
    let mut removed_cache = Vec::new();
    if let Some(cache) = &state.cache {
        for mt in [MappingType::Yarn, MappingType::Vanilla] {
            if cache.remove(&mt.cache_key(&version)) {
                removed_cache.push(mt.as_str().to_string());
            }
        }
    }
    if removed_files.is_empty() && removed_cache.is_empty() {
        return Err(ApiError::NotFound(format!("mapping {} not cached", version)));
    }
    Ok(Json(ApiResponse::success(UnloadInfo {
        version,
        removed_files,
        removed_cache,
    })))
}

#[derive(Serialize, ToSchema)]
pub struct MappingsList {
    pub yarn: Vec<String>,
    pub vanilla: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct MappingStats {
    pub version: String,
    pub mapping_type: String,
    pub classes: usize,
    pub methods: usize,
    pub fields: usize,
    pub extra: String,
}

#[derive(Serialize, ToSchema)]
pub struct UnloadInfo {
    pub version: String,
    pub removed_files: Vec<String>,
    pub removed_cache: Vec<String>,
}

/// Resolve a user-supplied local path against the mappings dir and ensure the
/// canonical result stays inside it (path traversal protection).
fn safe_local_path(mappings_dir: &str, rel: &str) -> Result<PathBuf, ApiError> {
    if rel.is_empty() {
        return Err(ApiError::BadRequest("path is required".to_string()));
    }
    if rel.contains("..") {
        return Err(ApiError::BadRequest(
            "path must not contain '..'".to_string(),
        ));
    }
    let p = FsPath::new(rel);
    if p.is_absolute() {
        return Err(ApiError::BadRequest(
            "path must be relative to the mappings dir".to_string(),
        ));
    }

    let base = FsPath::new(mappings_dir)
        .canonicalize()
        .map_err(|e| ApiError::Internal(format!("canonicalize mappings dir: {e}")))?;
    let candidate = base.join(p);
    let canon = candidate
        .canonicalize()
        .map_err(|_| ApiError::NotFound("local mapping file not found".to_string()))?;
    if !canon.starts_with(&base) {
        return Err(ApiError::BadRequest(
            "path escapes the mappings dir".to_string(),
        ));
    }
    Ok(canon)
}

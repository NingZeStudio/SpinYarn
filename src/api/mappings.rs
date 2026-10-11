use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::HeaderMap,
    Json,
};
use serde::{Deserialize, Serialize};
use std::path::{Path as FsPath, PathBuf};
use utoipa::ToSchema;

use crate::api::response::ApiResponse;
use crate::api::{require_admin, AppState};
use crate::error::ApiError;
use crate::mapping::dispatcher::{self, MappingType};
use crate::mapping::local::is_valid_version;

/// Reject a version that would escape the mappings dir when used in a path.
/// The `dispatcher::local_path` helpers join the version verbatim, so every
/// user-supplied version must pass the same token validation used by load.
fn validate_version(version: &str) -> Result<(), ApiError> {
    if !is_valid_version(version) {
        return Err(ApiError::BadRequest(format!(
            "invalid version token: {:?}",
            version
        )));
    }
    Ok(())
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoadLocalRequest {
    pub version: String,
    #[serde(default)]
    pub mapping_type: Option<String>,
    /// Path relative to the mappings dir (e.g. `./incoming/1.21.4.tiny.gz`).
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

/// POST /api/v1/mappings/load — availability probe for one mapping.
///
/// Reports whether `version`/`mapping_type` is available locally. This engine
/// never downloads mappings (they are host-provided), so "load" here only
/// checks presence; use `load/local` to install a file. Requires admin token.
#[utoipa::path(
    post,
    path = "/api/v1/mappings/load",
    request_body = LoadRequest,
    responses(
        (status = 200, body = ApiResponse<LoadInfo>),
        (status = 400, description = "invalid version or mapping_type"),
        (status = 403, description = "missing or bad admin token"),
        (status = 422, description = "malformed JSON")
    )
)]
pub async fn load_mapping(
    State(state): State<AppState>,
    headers: HeaderMap,
    req: Result<Json<LoadRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<LoadInfo>>, ApiError> {
    require_admin(&headers, &state.admin_token)?;
    let Json(req) = req?;
    let mtype =
        MappingType::parse_result(req.mapping_type.as_deref().unwrap_or("")).ok_or_else(|| {
            ApiError::BadRequest(format!(
                "unknown mapping_type: {:?}",
                req.mapping_type.as_deref().unwrap_or("")
            ))
        })?;
    validate_version(&req.version)?;

    let spinyarn = state.spinyarn.clone();
    let version = req.version.clone();
    let available = tokio::task::spawn_blocking(move || spinyarn.ensure_available(&version, mtype))
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(ApiResponse::success(LoadInfo {
        version: req.version,
        mapping_type: mtype.as_str().to_string(),
        available,
    })))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoadRequest {
    pub version: String,
    #[serde(default)]
    pub mapping_type: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct LoadInfo {
    pub version: String,
    pub mapping_type: String,
    pub available: bool,
}

/// POST /api/v1/mappings/load/local — load a mapping file from the local
/// filesystem into the standard location. The path is resolved relative to the
/// mappings dir and hardened against path traversal. Requires admin token.
#[utoipa::path(
    post,
    path = "/api/v1/mappings/load/local",
    request_body = LoadLocalRequest,
    responses(
        (status = 200, body = ApiResponse<LoadedInfo>),
        (status = 400, description = "invalid version, mapping_type, path, or corrupt mapping"),
        (status = 403, description = "missing or bad admin token"),
        (status = 404, description = "source file not found"),
        (status = 422, description = "malformed JSON")
    )
)]
pub async fn load_mapping_local(
    State(state): State<AppState>,
    headers: HeaderMap,
    req: Result<Json<LoadLocalRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<LoadedInfo>>, ApiError> {
    require_admin(&headers, &state.admin_token)?;
    let Json(req) = req?;
    let mtype =
        MappingType::parse_result(req.mapping_type.as_deref().unwrap_or("")).ok_or_else(|| {
            ApiError::BadRequest(format!(
                "unknown mapping_type: {:?}",
                req.mapping_type.as_deref().unwrap_or("")
            ))
        })?;
    validate_version(&req.version)?;

    let spinyarn = state.spinyarn.clone();
    let version = req.version.clone();
    let rel = req.path.clone();
    let out = tokio::task::spawn_blocking(move || {
        install_local_mapping(spinyarn.mappings_dir(), &version, mtype, &rel)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;

    Ok(Json(ApiResponse::success(LoadedInfo {
        version: req.version,
        mapping_type: mtype.as_str().to_string(),
        ok: true,
        path: out.0,
        bytes: out.1,
    })))
}

/// Copy a validated source file into its canonical location atomically:/// same-dir temp file + fsync + rename. Rejects symlinks on both ends (no
/// `libc` dependency, so `symlink_metadata` checks instead of `O_NOFOLLOW`),
/// and re-validates the installed file parses (corrupt → delete + 400).
/// Returns `(relative_path, bytes)`.
fn install_local_mapping(
    mappings_dir: &str,
    version: &str,
    mtype: MappingType,
    rel: &str,
) -> Result<(String, u64), ApiError> {
    // The mappings dir may not exist on first run; create it before resolving.
    std::fs::create_dir_all(mappings_dir)
        .map_err(|e| ApiError::Internal(format!("create mappings dir: {e}")))?;
    // Resolve + canonicalize inside the mappings dir; reject any traversal.
    let source = safe_local_path(mappings_dir, rel)?;

    // Source must not be a symlink (canonicalize follows links, so check the
    // link itself to block in-dir symlinks pointing elsewhere).
    if std::fs::symlink_metadata(&source)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(ApiError::BadRequest(
            "source must not be a symlink".to_string(),
        ));
    }

    let target = dispatcher::local_path(version, mappings_dir, mtype);
    // Refuse to clobber an existing symlink (would redirect the write outside).
    if let Ok(meta) = std::fs::symlink_metadata(&target) {
        if meta.file_type().is_symlink() {
            return Err(ApiError::BadRequest(
                "target is a symlink, refusing to overwrite".to_string(),
            ));
        }
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ApiError::Internal(format!("create dir: {e}")))?;
    }

    // Same-dir temp file so `rename` is atomic on one filesystem.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_name = format!(
        ".tmp.{}-{}.{}",
        std::process::id(),
        nanos,
        target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "mapping".to_string())
    );
    let tmp = target
        .parent()
        .map(|p| p.join(&tmp_name))
        .unwrap_or_else(|| std::path::PathBuf::from(tmp_name));
    let cleanup_tmp = |tmp: &std::path::Path| {
        let _ = std::fs::remove_file(tmp);
    };

    if let Err(e) = std::fs::copy(&source, &tmp) {
        cleanup_tmp(&tmp);
        return Err(ApiError::Internal(format!(
            "copy {} -> temp: {}",
            source.display(),
            e
        )));
    }
    // Fsync the temp file before the atomic rename.
    if let Err(e) = std::fs::File::open(&tmp).and_then(|f| f.sync_all()) {
        cleanup_tmp(&tmp);
        return Err(ApiError::Internal(format!("fsync temp: {e}")));
    }
    // Re-check: no symlink swap between the first check and rename.
    if let Ok(meta) = std::fs::symlink_metadata(&target) {
        if meta.file_type().is_symlink() {
            cleanup_tmp(&tmp);
            return Err(ApiError::BadRequest(
                "target is a symlink, refusing to overwrite".to_string(),
            ));
        }
    }
    if let Err(e) = std::fs::rename(&tmp, &target) {
        cleanup_tmp(&tmp);
        return Err(ApiError::Internal(format!("rename temp: {e}")));
    }
    // Post-rename: target must be a regular file, never a symlink.
    if std::fs::symlink_metadata(&target)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(true)
    {
        let _ = std::fs::remove_file(&target);
        return Err(ApiError::Internal(
            "installed target is a symlink".to_string(),
        ));
    }

    // Best-effort parse validation: corrupt gzip/text must not stay installed.
    if let Err(e) = dispatcher::load(version, mappings_dir, mtype) {
        let _ = std::fs::remove_file(&target);
        return Err(ApiError::BadRequest(format!(
            "mapping failed to parse: {e}"
        )));
    }

    let bytes = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
    // Only ever return the install-relative path, never the server absolute.
    let rel_path = match mtype {
        MappingType::Yarn => format!("{}.tiny.gz", version),
        MappingType::Vanilla => format!("vanilla/{}.txt", version),
    };
    Ok((rel_path, bytes))
}

/// GET /api/v1/mappings — list locally cached mappings, grouped by type.
#[utoipa::path(
    get,
    path = "/api/v1/mappings",
    responses(
        (status = 200, body = ApiResponse<MappingsList>),
    )
)]
pub async fn list_mappings(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<MappingsList>>, ApiError> {
    let dir = state.spinyarn.mappings_dir().to_string();
    let (yarn, vanilla) = tokio::task::spawn_blocking(move || {
        let base = FsPath::new(&dir);
        let mut yarn = Vec::new();
        if let Ok(entries) = std::fs::read_dir(base) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if let Some(v) = name.strip_suffix(".tiny.gz") {
                    // Skip temp files from atomic installs.
                    if !v.starts_with(".tmp.") {
                        yarn.push(v.to_string());
                    }
                }
            }
        }
        let mut vanilla = Vec::new();
        if let Ok(entries) = std::fs::read_dir(base.join("vanilla")) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if let Some(v) = name.strip_suffix(".txt") {
                    if !v.starts_with(".tmp.") {
                        vanilla.push(v.to_string());
                    }
                }
            }
        }
        yarn.sort();
        vanilla.sort();
        (yarn, vanilla)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(ApiResponse::success(MappingsList { yarn, vanilla })))
}

/// GET /api/v1/mappings/{type}/{version} — statistics for one mapping.
#[utoipa::path(
    get,
    path = "/api/v1/mappings/{type}/{version}",
    params(("type" = String, description = "yarn | vanilla"), ("version" = String, description = "Minecraft version")),
    responses(
        (status = 200, body = ApiResponse<MappingStats>),
        (status = 400, description = "invalid version or mapping_type"),
        (status = 404, description = "mapping not available locally")
    )
)]
pub async fn mapping_stats(
    State(state): State<AppState>,
    Path((mtype, version)): Path<(String, String)>,
) -> Result<Json<ApiResponse<MappingStats>>, ApiError> {
    validate_version(&version)?;
    let mtype = MappingType::parse_result(&mtype)
        .ok_or_else(|| ApiError::BadRequest(format!("unknown mapping_type: {:?}", mtype)))?;
    let spinyarn = state.spinyarn.clone();
    let loaded = tokio::task::spawn_blocking({
        let version = version.clone();
        move || spinyarn.load(&version, mtype)
    })
    .await
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
/// (both families) and drop cache entries. Requires admin token.
#[utoipa::path(
    delete,
    path = "/api/v1/mappings/{version}",
    params(("version" = String, description = "Minecraft version")),
    responses(
        (status = 200, body = ApiResponse<UnloadInfo>),
        (status = 400, description = "invalid version"),
        (status = 403, description = "missing or bad admin token"),
        (status = 404, description = "mapping not cached")
    )
)]
pub async fn unload_mapping(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(version): Path<String>,
) -> Result<Json<ApiResponse<UnloadInfo>>, ApiError> {
    require_admin(&headers, &state.admin_token)?;
    validate_version(&version)?;
    let (removed_files, removed_cache) = state.spinyarn.unload(&version);
    if removed_files.is_empty() && removed_cache.is_empty() {
        return Err(ApiError::NotFound(format!(
            "mapping {} not cached",
            version
        )));
    }
    // Never leak server absolute paths: strip the mappings dir prefix.
    let base = FsPath::new(state.spinyarn.mappings_dir());
    let removed_files: Vec<String> = removed_files
        .into_iter()
        .map(|p| {
            FsPath::new(&p)
                .strip_prefix(base)
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_else(|_| {
                    FsPath::new(&p)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or(p)
                })
        })
        .collect();
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
///
/// Note: no `..` substring precheck — the canonicalize + starts_with guard
/// below is authoritative and also tolerates legitimate names containing `..`.
fn safe_local_path(mappings_dir: &str, rel: &str) -> Result<PathBuf, ApiError> {
    if rel.is_empty() {
        return Err(ApiError::BadRequest("path is required".to_string()));
    }
    let p = FsPath::new(rel);
    if p.is_absolute() {
        return Err(ApiError::BadRequest(
            "path must be relative to the mappings dir".to_string(),
        ));
    }
    // Lexical traversal check before touching the filesystem: a `..` that
    // pops past the base escapes regardless of whether the target exists
    // (canonicalize alone would report a missing escape target as 404 and
    // mask the traversal as "not found").
    {
        use std::path::Component::*;
        let mut depth = 0usize;
        for comp in p.components() {
            match comp {
                Prefix(_) | RootDir => {
                    return Err(ApiError::BadRequest(
                        "path must be relative to the mappings dir".to_string(),
                    ))
                }
                CurDir => {}
                ParentDir => {
                    if depth == 0 {
                        return Err(ApiError::BadRequest(
                            "path escapes the mappings dir".to_string(),
                        ));
                    }
                    depth -= 1;
                }
                Normal(_) => depth += 1,
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spinyarn-api-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_validate_version_rejects_traversal() {
        assert!(validate_version("1.21.9").is_ok());
        assert!(validate_version("1.18.2-pre1").is_ok());
        assert!(validate_version("../../etc/passwd").is_err());
        assert!(validate_version("a/b").is_err());
        assert!(validate_version("..").is_err());
        assert!(validate_version("").is_err());
    }

    #[test]
    fn test_safe_local_path_rejects_traversal() {
        let dir = tmp_dir("traversal");
        // A file inside the dir resolves fine.
        std::fs::write(dir.join("1.21.9.tiny.gz"), b"x").unwrap();
        let ok = safe_local_path(dir.to_str().unwrap(), "1.21.9.tiny.gz").unwrap();
        assert!(ok.starts_with(dir.canonicalize().unwrap()));

        // `..` escapes -> rejected as 400 even when the target is missing
        // (lexical check runs before the existence canonicalize).
        assert!(matches!(
            safe_local_path(dir.to_str().unwrap(), "../escape.tiny.gz"),
            Err(ApiError::BadRequest(_))
        ));

        // Missing in-dir file -> 404, not 400.
        assert!(matches!(
            safe_local_path(dir.to_str().unwrap(), "nope.tiny.gz"),
            Err(ApiError::NotFound(_))
        ));

        // Absolute path -> rejected.
        let abs = dir.join("1.21.9.tiny.gz");
        assert!(safe_local_path(dir.to_str().unwrap(), abs.to_str().unwrap()).is_err());

        // Empty -> rejected.
        assert!(safe_local_path(dir.to_str().unwrap(), "").is_err());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_safe_local_path_accepts_dotdot_in_name() {
        // Legitimate filename containing `..` (not a path separator) is allowed.
        let dir = tmp_dir("dotdot");
        std::fs::write(dir.join("foo..bar.tiny.gz"), b"x").unwrap();
        let ok = safe_local_path(dir.to_str().unwrap(), "foo..bar.tiny.gz").unwrap();
        assert!(ok.starts_with(dir.canonicalize().unwrap()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_unknown_mapping_type_rejected() {
        assert!(MappingType::parse_result("srg").is_none());
        assert!(MappingType::parse_result("MOJANG").is_none());
        assert_eq!(
            MappingType::parse_result("VANILLA"),
            Some(MappingType::Vanilla)
        );
    }

    #[test]
    fn test_install_refuses_symlink_target() {
        let dir = tmp_dir("symlink-target");
        std::fs::write(dir.join("src.tiny.gz"), b"x").unwrap();
        // Pre-create the target as a symlink -> install must refuse.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("src.tiny.gz"), dir.join("1.21.9.tiny.gz"))
                .unwrap();
            let r = install_local_mapping(
                dir.to_str().unwrap(),
                "1.21.9",
                MappingType::Yarn,
                "src.tiny.gz",
            );
            assert!(r.is_err());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_install_returns_relative_path() {
        // Build a minimal valid yarn mapping so parse validation passes.
        let dir = tmp_dir("relpath");
        let tiny = "v1\tofficial\tintermediary\tnamed\nCLASS\tnet/minecraft/class_1\tnet/minecraft/class_1\tcom/Example\n";
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut enc, tiny.as_bytes()).unwrap();
        let bytes = enc.finish().unwrap();
        std::fs::write(dir.join("src.tiny.gz"), &bytes).unwrap();
        let (rel, n) = install_local_mapping(
            dir.to_str().unwrap(),
            "1.21.9",
            MappingType::Yarn,
            "src.tiny.gz",
        )
        .unwrap();
        assert_eq!(rel, "1.21.9.tiny.gz");
        assert!(n > 0);
        assert!(!rel.starts_with('/'));
        assert!(dir.join(&rel).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

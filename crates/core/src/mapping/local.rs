use std::io::Read;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;

use crate::mapping::{parse, Mappings};

/// Mapping files are provided externally (downloaded/shipped by the host
/// application into the mappings dir). This module only loads and validates
/// them locally — it never performs network access.
#[derive(Debug, thiserror::Error)]
pub enum MappingLoadError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parse error: {0}")]
    Parse(String),
}

/// Path of an external gzipped tiny file: `<mappings_dir>/<version>.tiny.gz`
fn bundled_path(mappings_dir: &str, version: &str) -> PathBuf {
    Path::new(mappings_dir).join(format!("{}.tiny.gz", version))
}

/// A version used in a file path must be a plain Minecraft version token.
/// Rejects path separators and `..` to prevent path traversal, e.g.
/// `version = "../../etc/passwd"`. Call before any path built from a
/// user-supplied version (API endpoints and the mapping dispatcher).
pub fn is_valid_version(version: &str) -> bool {
    let first = version.as_bytes().first();
    !version.is_empty()
        && version.len() <= 64
        && matches!(first, Some(b) if b.is_ascii_alphanumeric())
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
        && !version.contains("..")
}

/// Whether a mapping file is available for a version in the external
/// `<mappings_dir>/<version>.tiny.gz` directory.
pub fn is_version_supported(version: &str, mappings_dir: &str) -> bool {
    is_valid_version(version) && bundled_path(mappings_dir, version).exists()
}

/// Load mappings for a version from the external mappings directory.
/// Returns `Ok(None)` when no file provides the version.
pub fn load_mappings(
    version: &str,
    mappings_dir: &str,
) -> Result<Option<Mappings>, MappingLoadError> {
    if !is_valid_version(version) {
        return Ok(None);
    }
    let path = bundled_path(mappings_dir, version);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path)?;
    parse_gz(&bytes).map(Some)
}

fn parse_gz(bytes: &[u8]) -> Result<Mappings, MappingLoadError> {
    // M2：带上限解压防 zip bomb。正常单版本映射约 10MB，256MB 已远超合理值；
    // 先 `take(LIMIT + 1)`，超限即拒绝，避免无界 `read_to_end` 直接 OOM。
    const MAX_DECOMPRESSED_BYTES: u64 = 256 * 1024 * 1024;
    let decoder = GzDecoder::new(bytes);
    let mut limited = decoder.take(MAX_DECOMPRESSED_BYTES + 1);
    let mut out = Vec::new();
    limited
        .read_to_end(&mut out)
        .map_err(MappingLoadError::Io)?;
    if out.len() as u64 > MAX_DECOMPRESSED_BYTES {
        return Err(MappingLoadError::Parse(
            "decompressed mapping exceeds 256MB limit".to_string(),
        ));
    }
    parse(&out).map_err(|e| MappingLoadError::Parse(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_version_accepts_minecraft_tokens() {
        for v in [
            "1.21.9",
            "1.18.2-pre1",
            "1.21.11-rc2",
            "25w44a",
            "26.1",
            "26.1-Snapshot-1",
        ] {
            assert!(is_valid_version(v), "should accept {}", v);
        }
    }

    #[test]
    fn test_is_valid_version_rejects_traversal() {
        for v in [
            "../../etc/passwd",
            "../secret",
            "a/b",
            "a\\b",
            "..",
            "",
            ".",
            "a..b",
        ] {
            assert!(!is_valid_version(v), "should reject {:?}", v);
        }
    }

    #[test]
    fn test_is_valid_version_rejects_overlong() {
        assert!(!is_valid_version(&"x".repeat(65)));
    }

    #[test]
    fn test_load_mappings_ignores_invalid_version() {
        let dir = std::env::temp_dir();
        assert!(load_mappings("../../etc/passwd", dir.to_str().unwrap())
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_parse_gz_rejects_non_gzip_gracefully() {
        // M2：非 gzip/损坏输入应走原错误体系，不 panic。
        let r = parse_gz(b"not a gzip stream");
        assert!(r.is_err());
    }

    #[test]
    fn test_parse_gz_roundtrip_small_mapping() {
        // M2：正常小映射仍可解压解析（上限不应误伤正常路径）。
        use std::io::Write;
        let tiny = "v1\tofficial\tintermediary\tnamed\nCLASS\tnet/minecraft/class_1\tnet/minecraft/class_1\tcom/Example\n";
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(tiny.as_bytes()).unwrap();
        let bytes = enc.finish().unwrap();
        let m = parse_gz(&bytes).unwrap();
        assert_eq!(m.classes_len(), 1);
    }
}

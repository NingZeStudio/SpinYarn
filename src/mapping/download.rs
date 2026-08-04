use std::io::Read;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;

use crate::mapping::{parse, Mappings};

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
/// `version = "../../etc/passwd"`.
fn is_valid_version(version: &str) -> bool {
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
/// `<mappings_dir>/<version>.tiny.gz` directory (mappings are no longer
/// embedded into the binary; they ship alongside it).
pub fn is_version_supported(version: &str, mappings_dir: &str) -> bool {
    is_valid_version(version) && bundled_path(mappings_dir, version).exists()
}

/// Load mappings for a version from the external mappings directory.
/// Returns `Ok(None)` when no file provides the version.
pub fn load_mappings(version: &str, mappings_dir: &str) -> Result<Option<Mappings>, MappingLoadError> {
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
    let mut decoder = GzDecoder::new(bytes);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(MappingLoadError::Io)?;
    parse(&out).map_err(|e| MappingLoadError::Parse(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_version_accepts_minecraft_tokens() {
        for v in ["1.21.9", "1.18.2-pre1", "1.21.11-rc2", "25w44a", "26.1", "26.1-Snapshot-1"] {
            assert!(is_valid_version(v), "should accept {}", v);
        }
    }

    #[test]
    fn test_is_valid_version_rejects_traversal() {
        for v in ["../../etc/passwd", "../secret", "a/b", "a\\b", "..", "", ".", "a..b", "x".repeat(65).as_str()] {
            assert!(!is_valid_version(v), "should reject {:?}", v);
        }
    }

    #[test]
    fn test_load_mappings_ignores_invalid_version() {
        let dir = std::env::temp_dir();
        assert!(load_mappings("../../etc/passwd", dir.to_str().unwrap())
            .unwrap()
            .is_none());
    }
}

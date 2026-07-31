use std::io::Read;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;

use crate::mapping::{parse, Mappings};

use super::embedded;

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

/// Load mappings for a version.
///
/// Priority:
/// 1. Embedded table (compiled into the binary via `build.rs`).
/// 2. External `mappings/` directory override (if it contains the version).
///
/// Returns `Ok(None)` when no source provides the version.
pub fn load_mappings(version: &str) -> Result<Option<Mappings>, MappingLoadError> {
    if let Some(bytes) = embedded::get(version) {
        return parse_gz(bytes).map(Some);
    }

    let dir = std::env::var("SPINYARN_MAPPINGS_DIR").unwrap_or_else(|_| "./mappings".to_string());
    let path = bundled_path(&dir, version);
    if path.exists() {
        let bytes = std::fs::read(&path)?;
        return parse_gz(&bytes).map(Some);
    }

    Ok(None)
}

fn parse_gz(bytes: &[u8]) -> Result<Mappings, MappingLoadError> {
    let mut decoder = GzDecoder::new(bytes);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(MappingLoadError::Io)?;
    parse(&out).map_err(|e| MappingLoadError::Parse(e.to_string()))
}

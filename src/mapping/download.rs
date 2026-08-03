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

/// Whether a mapping file is available for a version in the external
/// `<mappings_dir>/<version>.tiny.gz` directory (mappings are no longer
/// embedded into the binary; they ship alongside it).
pub fn is_version_supported(version: &str, mappings_dir: &str) -> bool {
    bundled_path(mappings_dir, version).exists()
}

/// Load mappings for a version from the external mappings directory.
/// Returns `Ok(None)` when no file provides the version.
pub fn load_mappings(version: &str, mappings_dir: &str) -> Result<Option<Mappings>, MappingLoadError> {
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

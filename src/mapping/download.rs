use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use flate2::read::GzDecoder;
use once_cell::sync::Lazy;

use crate::mapping::{parse, Mappings};

/// A mapping file cached on disk is considered fresh for this long before a
/// refresh is attempted.
const CACHE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
const MAVEN_METADATA_URL: &str = "https://maven.fabricmc.net/net/fabricmc/yarn/maven-metadata.xml";

#[derive(Debug, thiserror::Error)]
pub enum MappingLoadError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Network error: {0}")]
    Network(String),
}

impl From<ureq::Error> for MappingLoadError {
    fn from(e: ureq::Error) -> Self {
        MappingLoadError::Network(e.to_string())
    }
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

/// Candidate Minecraft version token for runtime auto-download: the `1.x`
/// series (including `-pre`/`-rc`), which is the only one that ships Yarn
/// mappings. Snapshots (`25w44a`) and the 26.x era (`YY.D.H`, unobfuscated)
/// are rejected.
static DOWNLOADABLE_VERSION: Lazy<regex::Regex> = Lazy::new(|| {
    regex::Regex::new(r"^1\.\d{1,2}(\.\d{1,2})?(-(pre|rc)\d+)?$").unwrap()
});

pub fn is_downloadable_version(version: &str) -> bool {
    DOWNLOADABLE_VERSION.is_match(version)
}

/// Whether the on-disk mapping file is within the 7-day TTL.
fn mapping_fresh(path: &Path) -> bool {
    path.metadata()
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().map(|e| e < CACHE_TTL).unwrap_or(false))
        .unwrap_or(false)
}

/// Find the latest `+build.N` for a version in `maven-metadata.xml` content.
/// Returns the full Maven version string like `1.18.2-pre1+build.6`.
fn find_latest_build(version: &str, metadata: &str) -> Option<String> {
    let re = regex::Regex::new(&format!(
        r"<version>{}(?:\+build)?\.(\d+)</version>",
        regex::escape(version)
    ))
    .ok()?;
    let mut best: Option<(u64, String)> = None;
    for caps in re.captures_iter(metadata) {
        let build_str = caps.get(1)?.as_str();
        let build: u64 = build_str.parse().ok()?;
        let full = format!("{}+build.{}", version, build_str);
        if best.as_ref().map(|(b, _)| build > *b).unwrap_or(true) {
            best = Some((build, full));
        }
    }
    best.map(|(_, full)| full)
}

/// Fetch a URL into memory with a bounded timeout.
fn http_get(url: &str) -> Result<Vec<u8>, MappingLoadError> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .build();
    let resp = agent.get(url).call()?;
    let mut body = Vec::new();
    resp.into_reader().read_to_end(&mut body)?;
    Ok(body)
}

/// Download the mapping for `version` from Fabric Maven into
/// `<mappings_dir>/<version>.tiny.gz` (temp file + atomic rename).
fn download_mapping(version: &str, mappings_dir: &str) -> Result<bool, MappingLoadError> {
    let metadata = String::from_utf8(http_get(MAVEN_METADATA_URL)?)
        .map_err(|e| MappingLoadError::Parse(e.to_string()))?;
    let Some(mvn_version) = find_latest_build(version, &metadata) else {
        return Ok(false);
    };

    // `+` must be percent-encoded in the URL path.
    let encoded = mvn_version.replace('+', "%2B");
    let url = format!(
        "https://maven.fabricmc.net/net/fabricmc/yarn/{}/yarn-{}-tiny.gz",
        encoded, encoded
    );
    let bytes = http_get(&url)?;

    std::fs::create_dir_all(mappings_dir)?;
    let target = bundled_path(mappings_dir, version);
    let tmp = target.with_extension("tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &target)?;
    Ok(true)
}

/// Ensure a downloadable version's mapping exists locally (respecting the
/// 7-day TTL). Returns whether the mapping is ready to use.
pub fn ensure_mapping(version: &str, mappings_dir: &str) -> Result<bool, MappingLoadError> {
    if !is_valid_version(version) || !is_downloadable_version(version) {
        return Ok(false);
    }
    let path = bundled_path(mappings_dir, version);
    if path.exists() && mapping_fresh(&path) {
        return Ok(true);
    }
    // Missing or stale: try to (re)download.
    let ok = download_mapping(version, mappings_dir)?;
    if ok {
        return Ok(true);
    }
    // Download failed; fall back to a stale file if one exists.
    Ok(path.exists())
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
        for v in ["../../etc/passwd", "../secret", "a/b", "a\\b", "..", "", ".", "a..b"] {
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
    fn test_is_downloadable_version() {
        for v in ["1.21.9", "1.21.12", "1.18.2-pre1", "1.21.11-rc2", "1.14"] {
            assert!(is_downloadable_version(v), "should accept {}", v);
        }
        for v in ["25w44a", "26.1", "26.1-Snapshot-1", "1.13.2.1", "x1.0"] {
            assert!(!is_downloadable_version(v), "should reject {}", v);
        }
    }

    #[test]
    fn test_find_latest_build() {
        let meta = "<version>1.18.2-pre1+build.4</version>\n<version>1.18.2-pre1+build.6</version>\n<version>1.18.2-pre1+build.5</version>";
        assert_eq!(
            find_latest_build("1.18.2-pre1", meta).as_deref(),
            Some("1.18.2-pre1+build.6")
        );
        assert_eq!(find_latest_build("1.21.9", meta), None);
    }
}

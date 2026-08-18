use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;
use once_cell::sync::Lazy;

use crate::mapping::{parse, Mappings};

/// A mapping file cached on disk is considered fresh for this long before a
/// refresh is attempted.
const CACHE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
const MAVEN_METADATA_URL: &str = "https://maven.fabricmc.net/net/fabricmc/yarn/maven-metadata.xml";
/// The launcher version manifest and per-version JSONs change rarely; cache the
/// manifest in-process so a 43-version bootstrap does not refetch it per version.
const VANILLA_MANIFEST_TTL: Duration = Duration::from_secs(10 * 60);

/// Reusable HTTP agent: keeps a connection pool across downloads instead of
/// building a fresh agent per request.
static HTTP_AGENT: Lazy<ureq::Agent> = Lazy::new(|| {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .build()
});

/// Cached `version -> version_json_url` entries from the launcher manifest.
type ManifestEntries = Vec<(String, String)>;
static VANILLA_MANIFEST_CACHE: Lazy<Mutex<Option<(Instant, ManifestEntries)>>> =
    Lazy::new(|| Mutex::new(None));

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

/// Fetch a URL into memory with a bounded timeout, reusing the pooled agent.
fn http_get(url: &str) -> Result<Vec<u8>, MappingLoadError> {
    let resp = HTTP_AGENT.get(url).call()?;
    let mut body = Vec::new();
    resp.into_reader().read_to_end(&mut body)?;
    Ok(body)
}

/// Monotonic per-process sequence to disambiguate temp files created within
/// the same nanosecond (a nano-second timestamp alone could collide under
/// extreme concurrency).
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// A unique temporary file path next to `target`, so concurrent downloads of
/// the same version (e.g. startup bootstrap racing a request) never collide on
/// the same `.tmp` file. The final artifact is renamed over atomically.
fn unique_tmp(target: &Path) -> PathBuf {
    let mut file_name = target.file_name().unwrap_or_default().to_owned();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    file_name.push(format!(".tmp.{}.{}", nanos, seq));
    target.with_file_name(file_name)
}

/// Download the mapping for `version` from Fabric Maven into
/// `<mappings_dir>/<version>.tiny.gz` (temp file + atomic rename).
fn download_mapping(version: &str, mappings_dir: &str) -> Result<bool, MappingLoadError> {
    let metadata = String::from_utf8(http_get(MAVEN_METADATA_URL)?)
        .map_err(|e| MappingLoadError::Parse(e.to_string()))?;
    let Some(mvn_version) = find_latest_build(version, &metadata) else {
        tracing::debug!("maven build not found for version {}", version);
        return Ok(false);
    };

    // `+` must be percent-encoded in the URL path.
    let encoded = mvn_version.replace('+', "%2B");
    let url = format!(
        "https://maven.fabricmc.net/net/fabricmc/yarn/{}/yarn-{}-tiny.gz",
        encoded, encoded
    );
    tracing::info!("mapping download: {} ({})", version, mvn_version);
    let bytes = http_get(&url)?;
    tracing::info!("mapping downloaded: {} ({} bytes)", version, bytes.len());

    std::fs::create_dir_all(mappings_dir)?;
    let target = bundled_path(mappings_dir, version);
    let tmp = unique_tmp(&target);
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &target)?;
    Ok(true)
}

/// Ensure a downloadable version's mapping exists locally (respecting the
/// 7-day TTL unless `force`). Returns whether the mapping is ready to use.
///
/// Download is atomic (temp file + rename): on failure the previous file is
/// kept even in `force` mode.
pub fn ensure_mapping(
    version: &str,
    mappings_dir: &str,
    force: bool,
) -> Result<bool, MappingLoadError> {
    if !is_valid_version(version) || !is_downloadable_version(version) {
        return Ok(false);
    }
    let path = bundled_path(mappings_dir, version);
    if !force && path.exists() && mapping_fresh(&path) {
        tracing::debug!("mapping fresh (cached): {}", version);
        return Ok(true);
    }
    // Missing, stale or forced: try to (re)download.
    let ok = download_mapping(version, mappings_dir)?;
    if ok {
        return Ok(true);
    }
    // Download failed; fall back to a stale file if one exists.
    if path.exists() {
        tracing::warn!(
            "mapping download failed, falling back to stale file: {}",
            version
        );
        Ok(true)
    } else {
        tracing::error!(
            "mapping download failed and no stale file to fall back to: {} (auto-download failed)",
            version
        );
        Ok(false)
    }
}

/// Path of a Vanilla (TSRG) mapping file: `<mappings_dir>/vanilla/<version>.txt`
fn vanilla_path(mappings_dir: &str, version: &str) -> PathBuf {
    Path::new(mappings_dir)
        .join("vanilla")
        .join(format!("{}.txt", version))
}

/// Fetch + parse the launcher version manifest into `(version id, version json
/// url)` entries.
fn fetch_launcher_manifest() -> Result<Vec<(String, String)>, MappingLoadError> {
    let manifest: serde_json::Value = serde_json::from_slice(&http_get(
        "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json",
    )?)
    .map_err(|e| MappingLoadError::Parse(e.to_string()))?;
    manifest
        .get("versions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let id = v.get("id")?.as_str()?.to_string();
                    let url = v.get("url")?.as_str()?.to_string();
                    Some((id, url))
                })
                .collect()
        })
        .ok_or_else(|| MappingLoadError::Parse("malformed launcher manifest".to_string()))
}

/// The launcher version manifest entries `(version id, version json url)`,
/// cached in-process for `VANILLA_MANIFEST_TTL` to avoid one HTTP fetch per
/// version during a full bootstrap.
///
/// The network fetch happens **outside** the cache lock: a stale/empty cache
/// releases the lock before `fetch_launcher_manifest` (up to 20s), so concurrent
/// vanilla downloads don't all block on one holder. A poisoned lock falls back
/// to an uncached fetch.
fn launcher_manifest() -> Result<Vec<(String, String)>, MappingLoadError> {
    let now = Instant::now();
    {
        let cache = match VANILLA_MANIFEST_CACHE.lock() {
            Ok(guard) => guard,
            Err(_) => return fetch_launcher_manifest(), // poisoned lock
        };
        if let Some((at, entries)) = cache.as_ref() {
            if at.elapsed() < VANILLA_MANIFEST_TTL {
                return Ok(entries.clone());
            }
        }
    }
    // Fetch outside the lock, then briefly re-acquire to store the result.
    let entries = fetch_launcher_manifest()?;
    if let Ok(mut cache) = VANILLA_MANIFEST_CACHE.lock() {
        *cache = Some((now, entries.clone()));
    }
    Ok(entries)
}

/// Locate the Mojang official `client_mappings` URL for a version via the
/// launcher version manifest.
fn find_vanilla_mapping_url(version: &str) -> Result<Option<String>, MappingLoadError> {
    let entries = launcher_manifest()?;
    let version_url = entries
        .iter()
        .find(|(id, _)| id == version)
        .map(|(_, url)| url.clone())
        .ok_or_else(|| MappingLoadError::Parse("version not found in launcher manifest".to_string()))?;

    let version_json: serde_json::Value = serde_json::from_slice(&http_get(&version_url)?)
        .map_err(|e| MappingLoadError::Parse(e.to_string()))?;

    Ok(version_json
        .get("downloads")
        .and_then(|d| d.get("client_mappings"))
        .and_then(|cm| cm.get("url"))
        .and_then(|u| u.as_str())
        .map(str::to_string))
}

/// Download the Vanilla (Mojang official) mapping for `version` into
/// `<mappings_dir>/vanilla/<version>.txt` (temp file + atomic rename).
fn download_vanilla_mapping(version: &str, mappings_dir: &str) -> Result<bool, MappingLoadError> {
    let Some(url) = find_vanilla_mapping_url(version)? else {
        tracing::debug!("vanilla mapping not found for version {}", version);
        return Ok(false);
    };
    tracing::info!("vanilla mapping download: {}", version);
    let bytes = http_get(&url)?;
    tracing::info!(
        "vanilla mapping downloaded: {} ({} bytes)",
        version,
        bytes.len()
    );

    let dir = Path::new(mappings_dir).join("vanilla");
    std::fs::create_dir_all(&dir)?;
    let target = dir.join(format!("{}.txt", version));
    let tmp = unique_tmp(&target);
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &target)?;
    Ok(true)
}

/// Ensure a downloadable Vanilla version's mapping exists locally (7-day TTL
/// unless `force`). Returns whether the mapping is ready to use.
pub fn ensure_vanilla_mapping(
    version: &str,
    mappings_dir: &str,
    force: bool,
) -> Result<bool, MappingLoadError> {
    if !is_valid_version(version) || !is_downloadable_version(version) {
        return Ok(false);
    }
    let path = vanilla_path(mappings_dir, version);
    if !force && path.exists() && mapping_fresh(&path) {
        tracing::debug!("vanilla mapping fresh (cached): {}", version);
        return Ok(true);
    }
    let ok = download_vanilla_mapping(version, mappings_dir)?;
    if ok {
        return Ok(true);
    }
    // Download failed; fall back to a stale file if one exists.
    let stale = path.exists();
    if !stale {
        tracing::warn!(
            "vanilla mapping unavailable (no official Mojang mapping for this version): {}",
            version
        );
    }
    Ok(stale)
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

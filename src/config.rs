use serde::Deserialize;
use std::path::PathBuf;

/// Default request body limit (64MB).
pub const DEFAULT_MAX_BODY_SIZE: usize = 64 * 1024 * 1024;

/// Directory containing the running executable. Default locations for the
/// config file and the external mappings dir are resolved relative to it,
/// so the binary can be run from anywhere with its mappings/ next to it.
fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub maven: MavenConfig,
    #[serde(default)]
    pub cache: CacheConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_max_body_size")]
    pub max_body_size: usize,
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MavenConfig {
    #[serde(default = "default_mappings_dir")]
    pub mappings_dir: String,
    #[serde(default = "default_auto_download")]
    pub auto_download: bool,
    /// Versions fetched at startup when `auto_download` is on and the mappings
    /// directory is empty. Both the Yarn (`<version>.tiny.gz`) and Vanilla
    /// (`vanilla/<version>.txt`) families are fetched for each listed version
    /// (Vanilla skips versions without official mappings). Defaults to the full
    /// 1.14–1.21.11 Yarn set shipped by `scripts/download_mappings.sh`; override
    /// in `config.toml` to trim or extend it.
    #[serde(default = "default_bootstrap_versions")]
    pub bootstrap_versions: Vec<String>,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}
fn default_port() -> u16 {
    14523
}
fn default_max_body_size() -> usize {
    DEFAULT_MAX_BODY_SIZE
}
/// Concurrency default falls back to `SPINYARN_MAX_CONCURRENCY`, then 32.
fn default_max_concurrency() -> usize {
    std::env::var("SPINYARN_MAX_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(32)
}
/// Mappings dir default: `SPINYARN_MAPPINGS_DIR` override, else
/// `<exe_dir>/mappings` (shipped alongside the binary).
fn default_mappings_dir() -> String {
    std::env::var("SPINYARN_MAPPINGS_DIR")
        .ok()
        .unwrap_or_else(|| exe_dir().join("mappings").to_string_lossy().into_owned())
}

/// Auto-download missing mapping files from Fabric Maven at runtime (default on).
fn default_auto_download() -> bool {
    true
}

/// Default startup bootstrap list: every Yarn version bundled by
/// `scripts/download_mappings.sh` (1.14 – 1.21.11).
///
/// KEEP IN SYNC with `scripts/download_mappings.sh`'s `VERSIONS` array: when a
/// new version ships there, add it here too (or override via `config.toml`'s
/// `maven.bootstrap_versions`).
fn default_bootstrap_versions() -> Vec<String> {
    [
        "1.14", "1.14.1", "1.14.2", "1.14.3", "1.14.4",
        "1.15", "1.15.1", "1.15.2",
        "1.16", "1.16.1", "1.16.2", "1.16.3", "1.16.4", "1.16.5",
        "1.17", "1.17.1",
        "1.18", "1.18.1", "1.18.2",
        "1.19", "1.19.1", "1.19.2", "1.19.3", "1.19.4",
        "1.20", "1.20.1", "1.20.2", "1.20.3", "1.20.4", "1.20.5", "1.20.6",
        "1.21", "1.21.1", "1.21.2", "1.21.3", "1.21.4", "1.21.5",
        "1.21.6", "1.21.7", "1.21.8", "1.21.9", "1.21.10", "1.21.11",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// In-memory LRU cache of parsed mappings with watermark eviction.
#[derive(Debug, Deserialize, Clone)]
pub struct CacheConfig {
    #[serde(default = "default_cache_enabled")]
    pub enabled: bool,
    #[serde(default = "default_cache_max_entries")]
    pub max_entries: usize,
    #[serde(default = "default_cache_high")]
    pub high_watermark: usize,
    #[serde(default = "default_cache_low")]
    pub low_watermark: usize,
}

fn default_cache_enabled() -> bool {
    true
}
fn default_cache_max_entries() -> usize {
    44
}
fn default_cache_high() -> usize {
    40
}
fn default_cache_low() -> usize {
    30
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: default_cache_enabled(),
            max_entries: default_cache_max_entries(),
            high_watermark: default_cache_high(),
            low_watermark: default_cache_low(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            max_body_size: default_max_body_size(),
            max_concurrency: default_max_concurrency(),
        }
    }
}

impl Default for MavenConfig {
    fn default() -> Self {
        Self {
            mappings_dir: default_mappings_dir(),
            auto_download: default_auto_download(),
            bootstrap_versions: default_bootstrap_versions(),
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let config_paths = [
            exe_dir().join("config.toml"),
            PathBuf::from("config.toml"),
            PathBuf::from("SpinYarn.toml"),
            PathBuf::from("/etc/spinyarn/config.toml"),
        ];

        for path in &config_paths {
            if path.exists() {
                match std::fs::read_to_string(path) {
                    Ok(content) => match toml::from_str(&content) {
                        Ok(config) => {
                            tracing::info!("Loaded config from {}", path.display());
                            return config;
                        }
                        Err(e) => {
                            tracing::warn!("Failed to parse {}: {}", path.display(), e);
                        }
                    },
                    Err(e) => {
                        tracing::warn!("Failed to read {}: {}", path.display(), e);
                    }
                }
            }
        }

        tracing::info!("No config file found, using defaults");
        Self::default()
    }
}

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
    10
}
fn default_cache_high() -> usize {
    8
}
fn default_cache_low() -> usize {
    4
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

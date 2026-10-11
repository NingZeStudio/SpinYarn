use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Default request body limit (64MB).
pub const DEFAULT_MAX_BODY_SIZE: usize = 64 * 1024 * 1024;

/// Upper bound for the request body limit (256MB).
pub const MAX_BODY_SIZE_LIMIT: usize = 256 * 1024 * 1024;

/// Directory containing the running executable. Default locations for the
/// config file and the external mappings dir are resolved relative to it,
/// so the binary can be run from anywhere with its mappings/ next to it.
fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub maven: MavenConfig,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub admin: AdminConfig,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_max_body_size")]
    pub max_body_size: usize,
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
    /// CORS whitelist for the write (admin) routes. Empty = deny cross-origin
    /// (no `Access-Control-Allow-Origin` emitted). Public routes keep a
    /// permissive `Any` policy; see `src/api/mod.rs`.
    #[serde(default)]
    pub cors_origins: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct AdminConfig {
    /// Admin token guarding the write mapping endpoints. Empty = writes
    /// disabled (all return 403). Overridden by `SPINYARN_ADMIN_TOKEN` (env wins).
    #[serde(default)]
    pub token: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MavenConfig {
    #[serde(default = "default_mappings_dir")]
    pub mappings_dir: String,
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

/// Reserved cache bounds (kept for config-file compatibility).
/// The local LRU was removed in the Redis migration: the Web API currently
/// parses mappings on demand, and shared caching lives behind
/// `Spinyarn::from_redis_settings` (PHP/C ABI `spinyarn_init` 5th arg).
/// The fields still parse/serialize so old `config.toml` files keep working.
#[derive(Debug, Deserialize, Serialize, Clone)]
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
            cors_origins: Vec::new(),
        }
    }
}

impl Default for MavenConfig {
    fn default() -> Self {
        Self {
            mappings_dir: default_mappings_dir(),
        }
    }
}

impl Config {
    /// Apply environment overrides (env wins over file). Currently only the
    /// admin token: `SPINYARN_ADMIN_TOKEN` replaces `[admin] token` when set.
    fn apply_env_overrides(mut self) -> Self {
        if let Ok(v) = std::env::var("SPINYARN_ADMIN_TOKEN") {
            self.admin.token = v;
        }
        self
    }

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
                            let config: Self = config;
                            return config.apply_env_overrides();
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

        // No config file found: write a default config next to the executable
        // so first-run operators get a visible, editable starting point.
        let default = Self::default();
        let target = &config_paths[0];
        match toml::to_string_pretty(&default) {
            Ok(content) => {
                if let Err(e) = std::fs::write(target, content) {
                    tracing::warn!(
                        "failed to write default config to {}: {}",
                        target.display(),
                        e
                    );
                } else {
                    tracing::info!("no config found, generated default at {}", target.display());
                }
            }
            Err(e) => tracing::warn!("failed to serialize default config: {}", e),
        }

        tracing::info!("No config file found, using defaults");
        default.apply_env_overrides()
    }
}

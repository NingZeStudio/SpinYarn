use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

/// Built-in supported versions (for reference). Versions outside this set are
/// passed through unchanged rather than being deobfuscated.
pub static SUPPORTED_VERSIONS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        // 1.14 (5)
        "1.14", "1.14.1", "1.14.2", "1.14.3", "1.14.4",
        // 1.15 (3)
        "1.15", "1.15.1", "1.15.2",
        // 1.16 (6)
        "1.16", "1.16.1", "1.16.2", "1.16.3", "1.16.4", "1.16.5",
        // 1.17 (2)
        "1.17", "1.17.1",
        // 1.18 (3)
        "1.18", "1.18.1", "1.18.2",
        // 1.19 (5)
        "1.19", "1.19.1", "1.19.2", "1.19.3", "1.19.4",
        // 1.20 (7)
        "1.20", "1.20.1", "1.20.2", "1.20.3", "1.20.4", "1.20.5", "1.20.6",
        // 1.21 (12)
        "1.21", "1.21.1", "1.21.2", "1.21.3", "1.21.4", "1.21.5",
        "1.21.6", "1.21.7", "1.21.8", "1.21.9", "1.21.10", "1.21.11",
    ]
    .into()
});

#[derive(Debug, Deserialize, Clone, Default)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub maven: MavenConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MavenConfig {
    #[serde(default = "default_mappings_dir")]
    pub mappings_dir: String,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}
fn default_port() -> u16 {
    8080
}
fn default_mappings_dir() -> String {
    "./mappings".to_string()
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
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
    pub fn load() -> Self {
        let config_paths = ["config.toml", "SpinYarn.toml", "/etc/spinyarn/config.toml"];

        for path in &config_paths {
            if Path::new(path).exists() {
                match std::fs::read_to_string(path) {
                    Ok(content) => match toml::from_str(&content) {
                        Ok(config) => {
                            tracing::info!("Loaded config from {}", path);
                            return config;
                        }
                        Err(e) => {
                            tracing::warn!("Failed to parse {}: {}", path, e);
                        }
                    },
                    Err(e) => {
                        tracing::warn!("Failed to read {}: {}", path, e);
                    }
                }
            }
        }

        tracing::info!("No config file found, using defaults");
        Self::default()
    }
}

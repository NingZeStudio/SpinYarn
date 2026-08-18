//! SpinYarn core library: Minecraft log deobfuscation engine with no web
//! framework dependency. Consumed by the Axum binary and the C ABI (cdylib).

pub mod cache;
pub mod config;
pub mod deobfuscator;
pub mod mapping;

use std::sync::Arc;

use deobfuscator::DeobfuscateResult;
use mapping::dispatcher::{self, MappingType};
use mapping::download::{
    ensure_mapping, ensure_vanilla_mapping, is_downloadable_version, MappingLoadError,
};

/// A self-contained deobfuscation engine instance.
///
/// Owns the mappings directory, auto-download toggle, and (optionally) the LRU
/// cache. This is the synchronous facade used by both the C ABI and any other
/// embedding host (the Axum binary layers async/HTTP concerns on top).
pub struct Spinyarn {
    mappings_dir: String,
    auto_download: bool,
    cache: Option<Arc<cache::Cache>>,
}

/// Statistics for one deobfuscation pass (mirrors the JSON API shape).
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeobfuscateOutput {
    pub deobfuscated: String,
    pub classes_mapped: usize,
    pub methods_mapped: usize,
    pub fields_mapped: usize,
    pub total_time_ms: f64,
}

impl Spinyarn {
    /// Build an engine from a loaded [`config::Config`].
    pub fn new(config: &config::Config) -> Self {
        let cache = if config.cache.enabled {
            Some(Arc::new(cache::Cache::new(config.cache.clone())))
        } else {
            None
        };
        Spinyarn {
            mappings_dir: config.maven.mappings_dir.clone(),
            auto_download: config.maven.auto_download,
            cache,
        }
    }

    /// Build an engine from explicit settings (no config file). Used by the
    /// C ABI, where the host process executable is not SpinYarn and a config
    /// file is unnecessary: the caller supplies the mappings dir and the
    /// auto-download toggle directly. The LRU cache uses its defaults (on).
    pub fn from_settings(mappings_dir: &str, auto_download: bool) -> Self {
        Spinyarn {
            mappings_dir: mappings_dir.to_string(),
            auto_download,
            cache: Some(Arc::new(cache::Cache::new(config::CacheConfig::default()))),
        }
    }

    /// Deobfuscate `content` against `version`/`mapping_type`.
    ///
    /// Pass-through behaviour matches the HTTP API: an unsupported/unavailable
    /// version returns the input unchanged with zero counters (never an error).
    pub fn deobfuscate(
        &self,
        content: &str,
        version: &str,
        mapping_type: MappingType,
    ) -> DeobfuscateOutput {
        let mtype = mapping_type;

        if !dispatcher::is_supported(version, &self.mappings_dir, mtype) {
            if self.auto_download && is_downloadable_version(version) {
                let ready = match mtype {
                    MappingType::Yarn => ensure_mapping(version, &self.mappings_dir, false),
                    MappingType::Vanilla => ensure_vanilla_mapping(version, &self.mappings_dir, false),
                };
                match ready {
                    Ok(true) => {}
                    Ok(false) | Err(_) => {
                        return Self::passthrough(content);
                    }
                }
            } else {
                return Self::passthrough(content);
            }
        }

        let cache_key = mtype.cache_key(version);

        if let Some(cache) = &self.cache {
            if let Some(shared) = cache.get(&cache_key) {
                return Self::output_from(dispatcher::deobfuscate(&shared, content));
            }
        }

        let loaded = match dispatcher::load(version, &self.mappings_dir, mtype) {
            Ok(Some(loaded)) => loaded,
            Ok(None) => return Self::passthrough(content),
            Err(e) => {
                tracing::warn!("deobfuscate load error for {} {}: {}", mtype.as_str(), version, e);
                return Self::passthrough(content);
            }
        };

        let shared = Arc::new(loaded);
        if let Some(cache) = &self.cache {
            cache.insert(&cache_key, Arc::clone(&shared));
        }

        Self::output_from(dispatcher::deobfuscate(&shared, content))
    }

    /// Load/refresh a version's mapping file from its source.
    pub fn load_mapping(
        &self,
        version: &str,
        mapping_type: MappingType,
        force: bool,
    ) -> Result<bool, MappingLoadError> {
        match mapping_type {
            MappingType::Yarn => ensure_mapping(version, &self.mappings_dir, force),
            MappingType::Vanilla => ensure_vanilla_mapping(version, &self.mappings_dir, force),
        }
    }

    /// Whether a version/type mapping file exists locally.
    pub fn has_mapping(&self, version: &str, mapping_type: MappingType) -> bool {
        dispatcher::is_supported(version, &self.mappings_dir, mapping_type)
    }

    /// Cache statistics (None when caching is disabled).
    pub fn cache_stats(&self) -> Option<cache::CacheStats> {
        self.cache.as_ref().map(|c| c.stats())
    }

    fn passthrough(content: &str) -> DeobfuscateOutput {
        DeobfuscateOutput {
            deobfuscated: content.to_string(),
            classes_mapped: 0,
            methods_mapped: 0,
            fields_mapped: 0,
            total_time_ms: 0.0,
        }
    }

    fn output_from(result: DeobfuscateResult) -> DeobfuscateOutput {
        DeobfuscateOutput {
            deobfuscated: result.text,
            classes_mapped: result.classes_mapped,
            methods_mapped: result.methods_mapped,
            fields_mapped: result.fields_mapped,
            total_time_ms: result.total_time_ms,
        }
    }
}

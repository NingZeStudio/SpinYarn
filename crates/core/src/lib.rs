//! SpinYarn core library: Minecraft log deobfuscation engine with no web
//! framework dependency. Consumed by the Axum binary and the C ABI (cdylib).

pub mod cache;
pub mod config;
pub mod deobfuscator;
pub mod mapping;

use std::sync::Arc;

use deobfuscator::DeobfuscateResult;
use mapping::dispatcher::{self, LoadedMappings, MappingType};
use mapping::download::{
    ensure_mapping, ensure_vanilla_mapping, is_downloadable_version, MappingLoadError,
};

/// A self-contained deobfuscation engine instance.
///
/// Owns the mappings directory, auto-download toggle, and (optionally) the LRU
/// cache. This is the synchronous facade used by both the C ABI and any other
/// embedding host; it also exposes the deobfuscation pipeline as granular steps
/// so the Axum binary can wrap the load step with a concurrency gate while
/// keeping cache hits gate-free.
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
    /// Build an engine from a loaded [`config::Config`], honouring every cache
    /// field (bound + watermarks) exactly as configured.
    pub fn new(config: &config::Config) -> Self {
        Self::from_full_settings(
            &config.maven.mappings_dir,
            config.maven.auto_download,
            if config.cache.enabled {
                config.cache.max_entries
            } else {
                0
            },
            config.cache.high_watermark,
            config.cache.low_watermark,
        )
    }

    /// Build an engine from explicit settings (no config file), with the
    /// default LRU cache configuration. Used by the C ABI's short init.
    pub fn from_settings(mappings_dir: &str, auto_download: bool) -> Self {
        let d = config::CacheConfig::default();
        Self::from_full_settings(
            mappings_dir,
            auto_download,
            d.max_entries,
            d.high_watermark,
            d.low_watermark,
        )
    }

    /// Build an engine from the full explicit settings set (no config file),
    /// MySQLi-style positional configuration.
    ///
    /// - `cache_max_entries`: 0 = disable the LRU cache; a positive value caps
    ///   the cache at that many entries.
    /// - `cache_high_watermark` / `cache_low_watermark`: 0 = auto (derived from
    ///   the cap); otherwise the explicit watermark values are used.
    pub fn from_full_settings(
        mappings_dir: &str,
        auto_download: bool,
        cache_max_entries: usize,
        cache_high_watermark: usize,
        cache_low_watermark: usize,
    ) -> Self {
        let cache = if cache_max_entries == 0 {
            None
        } else {
            let high_watermark = if cache_high_watermark == 0 {
                cache_max_entries.max(1)
            } else {
                cache_high_watermark
            };
            let low_watermark = if cache_low_watermark == 0 {
                (cache_max_entries * 3 / 4).max(1)
            } else {
                cache_low_watermark
            };
            let cfg = config::CacheConfig {
                enabled: true,
                max_entries: cache_max_entries,
                high_watermark,
                low_watermark,
            };
            Some(Arc::new(cache::Cache::new(cfg)))
        };
        Spinyarn {
            mappings_dir: mappings_dir.to_string(),
            auto_download,
            cache,
        }
    }

    /// The mappings directory this engine reads from.
    pub fn mappings_dir(&self) -> &str {
        &self.mappings_dir
    }

    /// Whether on-demand mapping download is enabled.
    pub fn auto_download(&self) -> bool {
        self.auto_download
    }

    /// Make sure a mapping for `version`/`mapping_type` is available locally,
    /// downloading it on demand when enabled. Returns `false` when the caller
    /// should pass the input through unchanged (unsupported or download failed).
    pub fn ensure_available(&self, version: &str, mtype: MappingType) -> bool {
        if dispatcher::is_supported(version, &self.mappings_dir, mtype) {
            return true;
        }
        if !(self.auto_download && is_downloadable_version(version)) {
            return false;
        }
        let result = match mtype {
            MappingType::Yarn => ensure_mapping(version, &self.mappings_dir, false),
            MappingType::Vanilla => ensure_vanilla_mapping(version, &self.mappings_dir, false),
        };
        match result {
            Ok(true) => true,
            Ok(false) => false,
            Err(e) => {
                tracing::warn!(
                    "auto-download failed for {} {}: {}",
                    mtype.as_str(),
                    version,
                    e
                );
                false
            }
        }
    }

    /// Look up an already-parsed mapping in the cache (no load performed).
    pub fn get_cached(&self, version: &str, mtype: MappingType) -> Option<Arc<LoadedMappings>> {
        self.cache.as_ref()?.get(&mtype.cache_key(version))
    }

    /// Load a mapping set from disk (no cache interaction).
    pub fn load(&self, version: &str, mtype: MappingType) -> Option<LoadedMappings> {
        match dispatcher::load(version, &self.mappings_dir, mtype) {
            Ok(loaded) => loaded,
            Err(e) => {
                tracing::warn!("load error for {} {}: {}", mtype.as_str(), version, e);
                None
            }
        }
    }

    /// Store a freshly loaded mapping set in the cache (shared via the same Arc).
    pub fn insert_cached(&self, version: &str, mtype: MappingType, shared: &Arc<LoadedMappings>) {
        if let Some(cache) = &self.cache {
            cache.insert(&mtype.cache_key(version), Arc::clone(shared));
        }
    }

    /// Deobfuscate `content` against an already-loaded mapping set (zero-copy).
    pub fn deobfuscate_loaded(shared: &LoadedMappings, content: &str) -> DeobfuscateOutput {
        Self::output_from(dispatcher::deobfuscate(shared, content))
    }

    /// Full deobfuscation pipeline (used by the C ABI and simple embeds):
    /// ensure available -> cache lookup -> load -> cache -> deobfuscate.
    ///
    /// Pass-through behaviour matches the HTTP API: an unsupported/unavailable
    /// version returns the input unchanged with zero counters (never an error).
    pub fn deobfuscate(
        &self,
        content: &str,
        version: &str,
        mapping_type: MappingType,
    ) -> DeobfuscateOutput {
        if !self.ensure_available(version, mapping_type) {
            return Self::passthrough(content);
        }

        if let Some(shared) = self.get_cached(version, mapping_type) {
            return Self::deobfuscate_loaded(&shared, content);
        }

        let Some(loaded) = self.load(version, mapping_type) else {
            return Self::passthrough(content);
        };

        let shared = Arc::new(loaded);
        self.insert_cached(version, mapping_type, &shared);
        Self::deobfuscate_loaded(&shared, content)
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

    /// Remove a version's local files (both families) and drop cache entries.
    /// Returns `(removed_file_paths, removed_cache_types)`.
    pub fn unload(&self, version: &str) -> (Vec<String>, Vec<String>) {
        let removed_files = dispatcher::remove_all_local(version, &self.mappings_dir);
        let mut removed_cache = Vec::new();
        if let Some(cache) = &self.cache {
            for mt in [MappingType::Yarn, MappingType::Vanilla] {
                if cache.remove(&mt.cache_key(version)) {
                    removed_cache.push(mt.as_str().to_string());
                }
            }
        }
        (removed_files, removed_cache)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_from_full_settings_cache_disabled() {
        let s = Spinyarn::from_full_settings("/tmp", false, 0, 0, 0);
        assert!(s.cache.is_none());
    }

    #[test]
    fn test_from_full_settings_custom_bound() {
        let s = Spinyarn::from_full_settings("/tmp", false, 8, 8, 4);
        let cache = s.cache.as_ref().expect("cache enabled");
        let stats = cache.stats();
        assert!(stats.enabled);
    }

    #[test]
    fn test_from_full_settings_auto_watermarks() {
        // high=0, low=0 -> derived from the cap (high = cap, low = 3/4 cap).
        let s = Spinyarn::from_full_settings("/tmp", false, 10, 0, 0);
        let cache = s.cache.as_ref().expect("cache enabled");
        let stats = cache.stats();
        assert!(stats.enabled);
    }

    #[test]
    fn test_unload_removes_files_and_cache() {
        let dir = std::env::temp_dir().join(format!("spinyarn-unload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(dir.join("vanilla")).unwrap();
        let mut f = std::fs::File::create(dir.join("vanilla").join("1.21.4.txt")).unwrap();
        f.write_all(b"com.example.Main -> a:\n    0:10:void init() -> b\n").unwrap();

        let s = Spinyarn::from_settings(dir.to_str().unwrap(), false);
        // Populate the cache by loading once.
        let loaded = s.load("1.21.4", MappingType::Vanilla).expect("load");
        let shared = Arc::new(loaded);
        s.insert_cached("1.21.4", MappingType::Vanilla, &shared);

        let (files, cache_types) = s.unload("1.21.4");
        assert!(files.iter().any(|p| p.contains("1.21.4.txt")));
        assert_eq!(cache_types, vec!["vanilla"]);
        assert!(s.get_cached("1.21.4", MappingType::Vanilla).is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_passthrough_unsupported_version() {
        let s = Spinyarn::from_settings("/tmp/nonexistent", false);
        // Snapshot version is not downloadable -> passthrough.
        let out = s.deobfuscate("at a.b(X.java:1)", "25w44a", MappingType::Yarn);
        assert_eq!(out.deobfuscated, "at a.b(X.java:1)");
        assert_eq!(out.classes_mapped, 0);
    }
}

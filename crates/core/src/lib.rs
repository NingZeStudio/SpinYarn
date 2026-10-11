//! SpinYarn core library: Minecraft log deobfuscation engine with no web
//! framework dependency. Consumed by the Axum binary and the C ABI (cdylib).

pub mod cache;
pub mod config;
pub mod deobfuscator;
pub mod mapping;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use deobfuscator::DeobfuscateResult;
use mapping::dispatcher::{self, LoadedMappings, MappingType};

/// A self-contained deobfuscation engine instance.
///
/// Owns the mappings directory and (optionally) the Redis-backed shared
/// cache. This is the synchronous facade used by both the C ABI and any other
/// embedding host; it also exposes the deobfuscation pipeline as granular steps
/// so the Axum binary can wrap the load step with a concurrency gate while
/// keeping cache hits gate-free.
///
/// Caching exists only in the Redis form: without a `redis_url` the engine
/// parses mappings from disk on demand (no local LRU).
pub struct Spinyarn {
    mappings_dir: String,
    cache: Option<Arc<cache::Cache>>,
    load_errors: AtomicU64,
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
            if config.cache.enabled {
                config.cache.max_entries
            } else {
                0
            },
            config.cache.high_watermark,
            config.cache.low_watermark,
        )
    }

    /// Build an engine from explicit settings (no config file) without a
    /// cache: mappings are parsed from disk on demand. Used by the C ABI's
    /// short init.
    pub fn from_settings(mappings_dir: &str) -> Self {
        Self {
            mappings_dir: mappings_dir.to_string(),
            cache: None,
            load_errors: AtomicU64::new(0),
        }
    }

    /// Build a Redis-backed engine from explicit settings (no config file),
    /// MySQLi-style positional configuration.
    ///
    /// - `redis_url`: e.g. `redis://host:6379/0`; empty = no cache.
    /// - `cache_max_entries`: 0 = disable the cache; a positive value caps
    ///   the shared pool at that many entries.
    /// - `cache_high_watermark` / `cache_low_watermark`: 0 = auto (derived from
    ///   the cap); otherwise the explicit watermark values are used.
    ///
    /// An unreachable Redis also yields a cache-less engine (fail open);
    /// probing `cache_stats().is_none()` tells the two cases apart from a
    /// live cache. This matches the PHP host contract: LogShare passes its
    /// `spinyarn` water marks plus the URL derived from `cache.redis`, and
    /// falls back to [`from_full_settings`] when this comes back cache-less.
    pub fn from_redis_settings(
        mappings_dir: &str,
        redis_url: &str,
        cache_max_entries: usize,
        cache_high_watermark: usize,
        cache_low_watermark: usize,
    ) -> Self {
        let cache = if redis_url.is_empty() {
            None
        } else {
            cache::Cache::new(
                redis_url,
                cache_max_entries,
                cache_high_watermark,
                cache_low_watermark,
            )
            .map(Arc::new)
        };
        Self {
            mappings_dir: mappings_dir.to_string(),
            cache,
            load_errors: AtomicU64::new(0),
        }
    }

    /// Reserved MySQLi-style positional configuration (no config file).
    ///
    /// Accepted for C ABI / PHP signature compatibility only: the local LRU
    /// was removed in the Redis migration, so this always builds an
    /// on-demand (uncached) engine and ignores the bound arguments. Use
    /// [`from_redis_settings`] for shared caching.
    pub fn from_full_settings(
        mappings_dir: &str,
        cache_max_entries: usize,
        cache_high_watermark: usize,
        cache_low_watermark: usize,
    ) -> Self {
        let _ = (cache_max_entries, cache_high_watermark, cache_low_watermark);
        Spinyarn {
            mappings_dir: mappings_dir.to_string(),
            cache: None,
            load_errors: AtomicU64::new(0),
        }
    }

    /// The mappings directory this engine reads from.
    pub fn mappings_dir(&self) -> &str {
        &self.mappings_dir
    }

    /// Whether a mapping for `version`/`mapping_type` is available locally.
    /// Mapping files are provided externally (shipped/downloaded by the host);
    /// this engine never downloads them.
    pub fn ensure_available(&self, version: &str, mtype: MappingType) -> bool {
        dispatcher::is_supported(version, &self.mappings_dir, mtype)
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
                self.load_errors.fetch_add(1, Ordering::Relaxed);
                tracing::warn!("load error for {} {}: {}", mtype.as_str(), version, e);
                None
            }
        }
    }

    /// Store a freshly loaded mapping set in the cache (shared via the same Arc).
    pub fn insert_cached(&self, version: &str, mtype: MappingType, shared: &Arc<LoadedMappings>) {
        if let Some(cache) = &self.cache {
            cache.insert(&mtype.cache_key(version), shared);
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

    /// Whether a version/type mapping file exists locally.
    pub fn has_mapping(&self, version: &str, mapping_type: MappingType) -> bool {
        dispatcher::is_supported(version, &self.mappings_dir, mapping_type)
    }

    /// Cache statistics (None when caching is disabled).
    /// The Web API runs on-demand with no local cache, so this is normally
    /// `None`; only Redis-backed embeds report `Some`.
    pub fn cache_stats(&self) -> Option<cache::CacheStats> {
        self.cache.as_ref().map(|c| c.stats())
    }

    /// Cumulative count of mapping load failures (`dispatcher::load` Err).
    pub fn load_errors(&self) -> u64 {
        self.load_errors.load(Ordering::Relaxed)
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
        let s = Spinyarn::from_full_settings("/tmp", 0, 0, 0);
        assert!(s.cache.is_none());
    }

    #[test]
    fn test_from_full_settings_custom_bound() {
        let s = Spinyarn::from_full_settings("/tmp", 8, 8, 4);
        assert!(s.cache.is_none());
    }

    #[test]
    fn test_from_full_settings_auto_watermarks() {
        let s = Spinyarn::from_full_settings("/tmp", 10, 0, 0);
        assert!(s.cache.is_none());
    }

    #[test]
    fn test_from_redis_settings_disabled_or_unreachable_is_cacheless() {
        // 0 = disabled.
        let s = Spinyarn::from_redis_settings("/tmp", "redis://127.0.0.1:6379/", 0, 0, 0);
        assert!(s.cache.is_none());
        // Empty URL = no cache requested.
        let s = Spinyarn::from_redis_settings("/tmp", "", 10, 10, 7);
        assert!(s.cache.is_none());
        // Unreachable Redis fails open to a cache-less engine (host falls back
        // to on-demand parsing instead of running silently uncached).
        let s = Spinyarn::from_redis_settings("/tmp", "redis://127.0.0.1:1/", 10, 10, 7);
        assert!(s.cache.is_none());
        assert!(s.cache_stats().is_none());
    }

    #[test]
    fn test_unload_removes_files_and_cache() {
        let dir = std::env::temp_dir().join(format!("spinyarn-unload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(dir.join("vanilla")).unwrap();
        let mut f = std::fs::File::create(dir.join("vanilla").join("1.21.4.txt")).unwrap();
        f.write_all(b"com.example.Main -> a:\n    0:10:void init() -> b\n")
            .unwrap();

        let s = Spinyarn::from_settings(dir.to_str().unwrap());
        // Populate the cache by loading once.
        let loaded = s.load("1.21.4", MappingType::Vanilla).expect("load");
        let shared = Arc::new(loaded);
        s.insert_cached("1.21.4", MappingType::Vanilla, &shared);

        let (files, cache_types) = s.unload("1.21.4");
        assert!(files.iter().any(|p| p.contains("1.21.4.txt")));
        assert!(cache_types.is_empty());
        assert!(s.get_cached("1.21.4", MappingType::Vanilla).is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_passthrough_unsupported_version() {
        let s = Spinyarn::from_settings("/tmp/nonexistent");
        // Snapshot version has no mapping file -> passthrough.
        let out = s.deobfuscate("at a.b(X.java:1)", "25w44a", MappingType::Yarn);
        assert_eq!(out.deobfuscated, "at a.b(X.java:1)");
        assert_eq!(out.classes_mapped, 0);
    }

    #[test]
    fn test_load_errors_counter() {
        let dir = std::env::temp_dir().join(format!("spinyarn-load-err-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Corrupt gzip: file exists so `load` hits the Err branch.
        std::fs::write(dir.join("1.21.9.tiny.gz"), b"not-gzip").unwrap();
        let s = Spinyarn::from_settings(dir.to_str().unwrap());
        assert_eq!(s.load_errors(), 0);
        assert!(s.load("1.21.9", MappingType::Yarn).is_none());
        assert_eq!(s.load_errors(), 1);
        // Missing file is Ok(None), not an error.
        assert!(s.load("1.21.10", MappingType::Yarn).is_none());
        assert_eq!(s.load_errors(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

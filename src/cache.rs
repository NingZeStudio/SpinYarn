use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::config::CacheConfig;
use crate::mapping::Mappings;

/// In-memory LRU cache of parsed `Mappings` shared via `Arc`.
///
/// Bounded by `max_entries`; watermark eviction kicks in once `high_watermark`
/// entries are held, trimming back to `low_watermark` so the cache oscillates
/// between the two levels instead of idling at full capacity.
pub struct Cache {
    cfg: CacheConfig,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// version -> (shared mappings, monotonic access tick)
    map: HashMap<String, (Arc<Mappings>, u64)>,
    tick: u64,
    hits: u64,
    misses: u64,
    evictions: u64,
}

#[derive(Clone, serde::Serialize)]
pub struct CacheStats {
    pub enabled: bool,
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

impl Cache {
    pub fn new(cfg: CacheConfig) -> Self {
        Self {
            cfg,
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Look up a version, bumping its recency on a hit.
    pub fn get(&self, version: &str) -> Option<Arc<Mappings>> {
        let mut inner = self.inner.lock().unwrap();
        inner.tick += 1;
        let tick = inner.tick;
        let result = inner.map.get_mut(version).map(|(m, last)| {
            *last = tick;
            Arc::clone(m)
        });
        if result.is_some() {
            inner.hits += 1;
        } else {
            inner.misses += 1;
        }
        result
    }

    /// Insert a version's mappings, then trim down to the low watermark if the
    /// high watermark has been reached (evicting least-recently-used entries).
    pub fn insert(&self, version: &str, mappings: Arc<Mappings>) {
        let mut inner = self.inner.lock().unwrap();
        inner.tick += 1;
        let tick = inner.tick;
        inner.map.insert(version.to_string(), (mappings, tick));

        let max = self.cfg.max_entries.max(1);
        let high = self.cfg.high_watermark.clamp(1, max);
        let low = self.cfg.low_watermark.min(high);

        if inner.map.len() >= high {
            let remove = inner.map.len().saturating_sub(low);
            if remove > 0 {
                let mut oldest: Vec<(String, u64)> = inner
                    .map
                    .iter()
                    .map(|(k, (_, t))| (k.clone(), *t))
                    .collect();
                oldest.sort_by_key(|(_, t)| *t);
                for (k, _) in oldest.into_iter().take(remove) {
                    inner.map.remove(&k);
                    inner.evictions += 1;
                }
            }
        }
    }

    pub fn stats(&self) -> CacheStats {
        let inner = self.inner.lock().unwrap();
        CacheStats {
            enabled: self.cfg.enabled,
            entries: inner.map.len(),
            hits: inner.hits,
            misses: inner.misses,
            evictions: inner.evictions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(enabled: bool, max: usize, high: usize, low: usize) -> CacheConfig {
        CacheConfig {
            enabled,
            max_entries: max,
            high_watermark: high,
            low_watermark: low,
        }
    }

    fn mappings(id: &str) -> Arc<Mappings> {
        let mut m = Mappings::default();
        m.classes.insert(id.to_string(), id.to_string());
        Arc::new(m)
    }

    #[test]
    fn test_get_miss_then_hit() {
        let c = Cache::new(cfg(true, 10, 8, 4));
        assert!(c.get("1.21.9").is_none());
        c.insert("1.21.9", mappings("a"));
        assert!(c.get("1.21.9").is_some());
        let s = c.stats();
        assert_eq!(s.hits, 1);
        assert_eq!(s.misses, 1);
    }

    #[test]
    fn test_watermark_eviction_trims_to_low() {
        let c = Cache::new(cfg(true, 10, 5, 2));
        // Inserting v4 (5th) crosses high(5) and trims back to low(2), evicting
        // v0..v2; v5 then fills back up to 3 without another eviction.
        for i in 0..6 {
            c.insert(&format!("v{}", i), mappings("x"));
        }
        let s = c.stats();
        assert_eq!(s.entries, 3, "oscillates in (low, high]");
        assert_eq!(s.evictions, 3);
        // most recent survive, oldest evicted
        assert!(c.get("v4").is_some());
        assert!(c.get("v5").is_some());
        assert!(c.get("v0").is_none());
    }

    #[test]
    fn test_recency_keeps_recent_entries() {
        let c = Cache::new(cfg(true, 10, 5, 3));
        for i in 0..5 {
            c.insert(&format!("v{}", i), mappings("x"));
        }
        // v5th insert triggered an eviction of v0,v1 -> {v2,v3,v4}
        // Touch v2 to make it the most recent, then push two more.
        assert!(c.get("v2").is_some());
        c.insert("v5", mappings("x"));
        c.insert("v6", mappings("x")); // len 5 -> evict 2 oldest (v3,v4)
        assert!(c.get("v2").is_some(), "v2 was touched, must survive");
        assert!(c.get("v3").is_none(), "v3 untouched -> evicted");
        assert!(c.get("v6").is_some());
    }
}

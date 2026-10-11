use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::mapping::dispatcher::LoadedMappings;

/// Recency index for eviction (sorted set: member = full blob key, score = ms).
const LRU_KEY: &str = "spinyarn:mapping:lru";
/// Prefix for cached mapping blobs (`<prefix><type>:<version>`), namespaced so
/// the shared Redis DB never collides with host keys (e.g. LogShare's own).
const KEY_PREFIX: &str = "spinyarn:mapping:";

/// Millisecond clock for LRU ordering (second granularity ties same-second
/// inserts and makes eviction order undefined).
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Normalize MySQLi-style bounds (`0 = auto`), mirroring the old in-memory
/// LRU semantics: `high = 0` derives from `max`, `low = 0` derives to
/// `max * 3 / 4`. Returns `(max, high, low)` with `1 <= low <= high <= max`.
fn normalize_bounds(max_entries: usize, high: usize, low: usize) -> (usize, usize, usize) {
    let max = max_entries.max(1);
    let h = if high == 0 { max } else { high.clamp(1, max) };
    let l = if low == 0 {
        (max * 3 / 4).max(1).min(h)
    } else {
        low.min(h)
    };
    (max, h, l)
}

/// Redis-backed shared mapping cache (cross-process, e.g. PHP-FPM workers).
///
/// Parsed tables are stored as JSON blobs (7-day absolute TTL) under
/// `<prefix><type>:<version>`; recency lives in the [`LRU_KEY`] sorted set.
/// Eviction trims down to `low_watermark` once `high_watermark` is reached so
/// the pool oscillates between the two levels instead of idling at full.
///
/// Every op opens a short connection (`Client::get_connection`) rather than
/// holding a resident one: PHP-FPM forks workers after the client is built,
/// and a shared socket would not survive `fork()`. Inserts are rare (once per
/// version), hits cost one extra `ZADD` to protect hot entries.
pub struct Cache {
    client: redis::Client,
    high_watermark: usize,
    low_watermark: usize,
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
}

#[derive(Clone, serde::Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct CacheStats {
    pub enabled: bool,
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

fn redis_key(key: &str) -> String {
    format!("{}{}", KEY_PREFIX, key)
}

impl Cache {
    /// Connect to `url` with MySQLi-style bounds. Returns `None` when caching
    /// is disabled (`max_entries == 0`, empty URL) or Redis is unreachable at
    /// construction time, so callers can fail open (on-demand parsing) instead
    /// of running silently uncached.
    pub fn new(
        url: &str,
        max_entries: usize,
        high_watermark: usize,
        low_watermark: usize,
    ) -> Option<Self> {
        if max_entries == 0 || url.is_empty() {
            return None;
        }
        let client = redis::Client::open(url).ok()?;
        // Fail fast: `Client::open` only parses the URL, so PING once here.
        // Per-op failures later still degrade to on-demand loads.
        match client.get_connection() {
            Ok(mut con) => {
                if redis::cmd("PING").query::<String>(&mut con).is_err() {
                    tracing::warn!("redis cache unreachable, caching disabled");
                    return None;
                }
            }
            Err(_) => {
                tracing::warn!("redis cache unreachable, caching disabled");
                return None;
            }
        }
        let (max, high, low) = normalize_bounds(max_entries, high_watermark, low_watermark);
        tracing::info!(
            "redis cache enabled (max {}, high {} -> low {})",
            max,
            high,
            low
        );
        Some(Self {
            client,
            high_watermark: high,
            low_watermark: low,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        })
    }

    /// Look up a mapping blob, refreshing its LRU recency on a hit.
    /// On a miss the (possibly TTL-expired) index member is dropped so
    /// `ZCARD` stays honest.
    pub fn get(&self, key: &str) -> Option<std::sync::Arc<LoadedMappings>> {
        let full = redis_key(key);
        let Ok(mut con) = self.client.get_connection() else {
            tracing::debug!("cache get skipped (redis unreachable): {}", key);
            return None;
        };
        let raw: Option<Vec<u8>> = redis::Commands::get(&mut con, full.as_str()).unwrap_or(None);
        match raw {
            Some(bytes) => match serde_json::from_slice(&bytes) {
                Ok(mappings) => {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    // Hits must bump recency, otherwise the index degrades to
                    // insert-order FIFO and hot versions get evicted first.
                    let _: redis::RedisResult<()> =
                        redis::Commands::zadd(&mut con, LRU_KEY, full.as_str(), now_ms());
                    tracing::debug!("cache hit: {}", key);
                    Some(std::sync::Arc::new(mappings))
                }
                Err(e) => {
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!("cache corrupt entry, dropping: {} ({})", key, e);
                    let _: redis::RedisResult<()> = redis::Commands::del(&mut con, full.as_str());
                    let _: redis::RedisResult<()> =
                        redis::Commands::zrem(&mut con, LRU_KEY, full.as_str());
                    None
                }
            },
            None => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                let _: redis::RedisResult<()> =
                    redis::Commands::zrem(&mut con, LRU_KEY, full.as_str());
                tracing::debug!("cache miss: {}", key);
                None
            }
        }
    }

    /// Store a mapping blob (7-day TTL), then trim the pool down to the low
    /// watermark when the high watermark is reached (oldest first).
    pub fn insert(&self, key: &str, mappings: &LoadedMappings) {
        let full = redis_key(key);
        let Ok(mut con) = self.client.get_connection() else {
            tracing::debug!("cache insert skipped (redis unreachable): {}", key);
            return;
        };
        let Ok(value) = serde_json::to_vec(mappings) else {
            tracing::warn!("cache insert serialize failed: {}", key);
            return;
        };
        // 7-day absolute TTL (literal keeps the `set_ex` seconds type inferred,
        // as before); hot entries are re-parsed at most once per window.
        let stored: redis::RedisResult<()> =
            redis::Commands::set_ex(&mut con, full.as_str(), value, 604800);
        if stored.is_err() {
            tracing::warn!("cache insert store failed: {}", key);
            return;
        }
        // Only index blobs that are actually stored; otherwise eviction could
        // drop live entries to make room for a phantom member.
        let _: redis::RedisResult<()> =
            redis::Commands::zadd(&mut con, LRU_KEY, full.as_str(), now_ms());
        let count: usize = redis::Commands::zcard(&mut con, LRU_KEY).unwrap_or(0);
        if count >= self.high_watermark {
            let remove = count.saturating_sub(self.low_watermark);
            if remove > 0 {
                let keys: Vec<String> =
                    redis::Commands::zrange(&mut con, LRU_KEY, 0, remove as isize - 1)
                        .unwrap_or_default();
                if keys.is_empty() {
                    return;
                }
                // DEL + ZREM is idempotent: TTL-expired members clean up here
                // even if no reader ever re-requested them.
                for old_key in &keys {
                    let _: redis::RedisResult<()> =
                        redis::Commands::del(&mut con, old_key.as_str());
                    let _: redis::RedisResult<()> =
                        redis::Commands::zrem(&mut con, LRU_KEY, old_key.as_str());
                }
                self.evictions
                    .fetch_add(keys.len() as u64, Ordering::Relaxed);
                tracing::info!(
                    "cache evict: {} entries ({} >= high {} -> low {})",
                    keys.len(),
                    count,
                    self.high_watermark,
                    self.low_watermark
                );
            }
        } else {
            tracing::debug!("cache insert: {} (entries {})", key, count);
        }
    }

    pub fn stats(&self) -> CacheStats {
        let entries: usize = self
            .client
            .get_connection()
            .ok()
            .and_then(|mut con| redis::Commands::zcard(&mut con, LRU_KEY).ok())
            .unwrap_or(0);
        CacheStats {
            enabled: true,
            entries,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
        }
    }

    /// Drop a version's blob and its index member. Returns true when anything
    /// was removed (either counts: a TTL-expired blob may leave only the
    /// member behind and vice versa).
    pub fn remove(&self, key: &str) -> bool {
        let full = redis_key(key);
        let Ok(mut con) = self.client.get_connection() else {
            return false;
        };
        let del: bool = redis::Commands::del(&mut con, full.as_str()).unwrap_or(false);
        let zrem: bool = redis::Commands::zrem(&mut con, LRU_KEY, full.as_str()).unwrap_or(false);
        del || zrem
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_bounds_explicit() {
        // LogShare production shape (Config.inc.example.php).
        assert_eq!(normalize_bounds(10, 10, 7), (10, 10, 7));
        assert_eq!(normalize_bounds(8, 8, 4), (8, 8, 4));
    }

    #[test]
    fn test_normalize_bounds_auto() {
        // 0 = auto: high derives from max, low from max * 3 / 4.
        assert_eq!(normalize_bounds(10, 0, 0), (10, 10, 7));
        assert_eq!(normalize_bounds(44, 0, 0), (44, 44, 33));
        assert_eq!(normalize_bounds(1, 0, 0), (1, 1, 1));
    }

    #[test]
    fn test_normalize_bounds_clamped() {
        // Out-of-range inputs collapse to 1 <= low <= high <= max.
        assert_eq!(normalize_bounds(10, 99, 99), (10, 10, 10));
        assert_eq!(normalize_bounds(10, 2, 9), (10, 2, 2));
        assert_eq!(normalize_bounds(10, 0, 9), (10, 10, 9));
    }

    #[test]
    fn test_redis_key_namespaced() {
        assert_eq!(redis_key("yarn:1.21.4"), "spinyarn:mapping:yarn:1.21.4");
    }

    #[test]
    fn test_new_rejects_disabled() {
        assert!(Cache::new("redis://127.0.0.1:6379/", 0, 0, 0).is_none());
        assert!(Cache::new("", 10, 10, 7).is_none());
    }

    #[test]
    fn test_new_rejects_unreachable() {
        // Nothing listens on port 1: must fail fast, not hang.
        assert!(Cache::new("redis://127.0.0.1:1/", 10, 10, 7).is_none());
    }
}

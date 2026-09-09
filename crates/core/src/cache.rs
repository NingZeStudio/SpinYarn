use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use redis::Commands;

use crate::mapping::dispatcher::LoadedMappings;

/// Redis 侧版本条目高水位（`<type>:<version>` 计数）：insert 超过即批量淘汰最旧。
pub const VERSION_HIGH_WATERMARK: usize = 10;
/// 高水位触发后淘汰到达的低水位。
pub const VERSION_LOW_WATERMARK: usize = 8;

/// LRU 记账有序集合（member=cache key，score=最近使用时间秒）
const LRU_ZSET: &str = "spinyarn:mapping:lru";
/// 映射键与 LRU 集合的 TTL（秒）
const MAPPING_TTL: u64 = 604800;

/// Redis-backed mapping cache (single shared tier; no in-process copy).
///
/// v1.1.0 的两个问题在此修复：
/// - **不加速**：每次调用新建 TCP 连接 + JSON 全量反序列化（实测单次 37-54ms）。
///   改为常驻连接复用 + bincode 序列化（整块编解码，解析耗时降至毫秒级）。
/// - **不释放**：JSON 反序列化产生海量小字符串分配（HashMap key 等），glibc
///   堆高水位驻留不还。bincode 以整块 buffer 进出，超过 mmap 阈值的分配在
///   free 时直接 munmap 归还内核。
///
/// Redis 容量由版本高低水位控制：超过 10 个版本即按最近使用时间淘汰最旧
/// 的键，回落到 8。
pub struct Cache {
    client: redis::Client,
    conn: Mutex<Option<redis::Connection>>,
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
}

#[derive(Clone, serde::Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct CacheStats {
    pub enabled: bool,
    /// Redis 中当前缓存的版本条目数（LRU ZSET 计数）
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Encode with bincode; decode accepts bincode first and falls back to the
/// v1.1.0 JSON format (JSON objects start with `{`), so old cached entries
/// stay readable across the upgrade.
fn encode_mappings(mappings: &LoadedMappings) -> Option<Vec<u8>> {
    bincode::serialize(mappings).ok()
}

fn decode_mappings(bytes: Vec<u8>) -> Option<LoadedMappings> {
    if bytes.first() == Some(&b'{') {
        return serde_json::from_slice(&bytes).ok();
    }
    bincode::deserialize(&bytes).ok()
}

impl Cache {
    pub fn new(url: &str) -> Option<Self> {
        redis::Client::open(url).ok().map(|client| Self {
            client,
            conn: Mutex::new(None),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        })
    }

    /// 常驻连接：取出（必要时重建）并执行；命令失败时重连重试一次。
    /// 调用方为单线程宿主（PHP 扩展），Mutex 实际无竞争，仅守护内部状态。
    fn with_connection<F, T>(&self, f: F) -> Option<T>
    where
        F: Fn(&mut redis::Connection) -> redis::RedisResult<T>,
    {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        for attempt in 0..2 {
            if guard.is_none() {
                *guard = self.client.get_connection().ok();
            }
            let conn = guard.as_mut()?;
            match f(conn) {
                Ok(value) => return Some(value),
                Err(_) => {
                    *guard = None;
                    if attempt == 1 {
                        return None;
                    }
                }
            }
        }
        None
    }

    pub fn get(&self, key: &str) -> Option<std::sync::Arc<LoadedMappings>> {
        let hit = self.get_inner(key);
        match &hit {
            Some(_) => self.hits.fetch_add(1, Ordering::Relaxed),
            None => self.misses.fetch_add(1, Ordering::Relaxed),
        };
        hit
    }

    fn get_inner(&self, key: &str) -> Option<Arc<LoadedMappings>> {
        // with_connection 外层 Option（连接成败）套内层 Option（键是否存在）
        let value: Option<Vec<u8>> = self.with_connection(|c| c.get(key)).flatten();
        let mappings = decode_mappings(value?)?;
        // 刷新最近使用时间，防被水位淘汰（尽力而为）
        let _: Option<usize> = self.with_connection(|c| c.zadd(LRU_ZSET, key, now_unix()));
        Some(Arc::new(mappings))
    }

    pub fn insert(&self, key: &str, mappings: &LoadedMappings) {
        let Some(value) = encode_mappings(mappings) else {
            return;
        };
        let written = self
            .with_connection(|c| {
                let _: () = c.set_ex(key, &value, MAPPING_TTL)?;
                let _: () = c.zadd(LRU_ZSET, key, now_unix())?;
                let _: () = c.expire(LRU_ZSET, MAPPING_TTL as i64)?;
                Ok(())
            })
            .is_some();
        if written {
            self.evict_to_low_watermark();
        }
    }

    /// 版本数超过高水位时，按 LRU 逐个弹出最旧条目并删除其映射键，直至低水位。
    fn evict_to_low_watermark(&self) {
        loop {
            let Some(count) = self.with_connection(|c: &mut redis::Connection| -> redis::RedisResult<u64> { c.zcard(LRU_ZSET) }) else {
                return;
            };
            if count <= VERSION_HIGH_WATERMARK as u64 {
                return;
            }
            let popped: Vec<(String, f64)> = match self
                .with_connection(|c| c.zpopmin(LRU_ZSET, 1))
            {
                Some(v) => v,
                None => return,
            };
            let Some((member, _)) = popped.into_iter().next() else {
                return;
            };
            // 数据键可能已按 TTL 自行过期：DEL 幂等，计数仍记淘汰
            let _: Option<i32> = self.with_connection(|c| c.del(&member));
            self.evictions.fetch_add(1, Ordering::Relaxed);
            if count - 1 <= VERSION_LOW_WATERMARK as u64 {
                return;
            }
        }
    }

    pub fn stats(&self) -> CacheStats {
        let entries = self
            .with_connection(|c: &mut redis::Connection| -> redis::RedisResult<u64> { c.zcard(LRU_ZSET) })
            .unwrap_or(0) as usize;
        CacheStats {
            enabled: true,
            entries,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
        }
    }

    pub fn remove(&self, key: &str) -> bool {
        let removed: Option<i32> = self.with_connection(|c| {
            let n: i32 = c.del(key)?;
            let _: i32 = c.zrem(LRU_ZSET, key)?;
            Ok(n)
        });
        removed.unwrap_or(0) > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::tiny_v2::Mappings;
    use std::io::Write;

    fn sample_loaded() -> LoadedMappings {
        let mut m = Mappings::default();
        m.classes.insert("a".into(), "net.minecraft.client.MinecraftClient".into());
        LoadedMappings::Yarn(Arc::new(m))
    }

    #[test]
    fn bincode_roundtrip() {
        let bytes = bincode::serialize(&sample_loaded()).unwrap();
        let back = decode_mappings(bytes).expect("bincode decode");
        match back {
            LoadedMappings::Yarn(m) => assert_eq!(
                m.classes.get("a").map(String::as_str),
                Some("net.minecraft.client.MinecraftClient")
            ),
            LoadedMappings::Vanilla(_) => panic!("wrong family"),
        }
    }

    #[test]
    fn json_fallback_decodes_v1_1_0_entries() {
        // v1.1.0 存量键是 JSON 序列化，升级期间必须仍可读
        let json = serde_json::to_vec(&sample_loaded()).unwrap();
        assert!(json.starts_with(b"{"));
        assert!(decode_mappings(json).is_some());
    }

    #[test]
    fn cache_ops_never_panic_without_redis() {
        // 不可达 Redis：所有路径必须静默降级（缓存缺席，不影响正确性）
        let cache = Cache::new("redis://127.0.0.1:1").unwrap();
        cache.insert("yarn:1.21.9", &sample_loaded());
        assert!(cache.get("yarn:1.21.9").is_none());
        assert!(!cache.remove("yarn:1.21.9"));
        let stats = cache.stats();
        assert!(stats.enabled);
        assert_eq!(stats.entries, 0);
    }

    #[test]
    fn vanilla_family_roundtrip() {
        let mut f = std::io::Cursor::new(Vec::new());
        write!(f, "com.example.Main -> a:\n    0:10:void init() -> b\n").unwrap();
        let raw = f.into_inner();
        let vm = crate::mapping::vanilla::parse_tsrg(&String::from_utf8_lossy(&raw)).unwrap();
        let loaded = LoadedMappings::Vanilla(Arc::new(vm));
        let bytes = bincode::serialize(&loaded).unwrap();
        assert!(matches!(
            decode_mappings(bytes),
            Some(LoadedMappings::Vanilla(_))
        ));
    }
}

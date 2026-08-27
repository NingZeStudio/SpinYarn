use std::sync::atomic::{AtomicU64, Ordering};

use crate::mapping::dispatcher::LoadedMappings;

pub struct Cache {
    client: redis::Client,
    hits: AtomicU64,
    misses: AtomicU64,
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

impl Cache {
    pub fn new(url: &str) -> Option<Self> {
        redis::Client::open(url).ok().map(|client| Self {
            client,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        })
    }

    pub fn get(&self, key: &str) -> Option<std::sync::Arc<LoadedMappings>> {
        let mut connection = self.client.get_connection().ok()?;
        let value: Option<Vec<u8>> = redis::Commands::get(&mut connection, key).ok()?;
        match value.and_then(|bytes| serde_json::from_slice(&bytes).ok()) {
            Some(mappings) => {
                self.hits.fetch_add(1, Ordering::Relaxed);
                Some(std::sync::Arc::new(mappings))
            }
            None => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    pub fn insert(&self, key: &str, mappings: &LoadedMappings) {
        let Ok(mut connection) = self.client.get_connection() else {
            return;
        };
        let Ok(value) = serde_json::to_vec(mappings) else {
            return;
        };
        let _: redis::RedisResult<()> = redis::Commands::set_ex(&mut connection, key, value, 604800);
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            enabled: true,
            entries: 0,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: 0,
        }
    }

    pub fn remove(&self, key: &str) -> bool {
        let Ok(mut connection) = self.client.get_connection() else {
            return false;
        };
        redis::Commands::del(&mut connection, key).unwrap_or(false)
    }
}

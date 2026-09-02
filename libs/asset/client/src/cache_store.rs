//! Capability-based verified cache stores.
//!
//! The in-memory backend is intentionally page-lifetime only: its pins and
//! LRU survive for as long as the store value does, and it cannot resume a
//! partial download after a reload. [`CacheStoreStats`] reports both facts.

use crate::error::{ClientError, ClientResult};
use makepad_asset_data::Sha256;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlobContent {
    Bytes(Arc<[u8]>),
    #[cfg(all(not(target_arch = "wasm32"), not(feature = "web")))]
    VerifiedPath(std::path::PathBuf),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStoreStats {
    pub object_count: u64,
    pub total_bytes: u64,
    pub pinned_bytes: u64,
    pub partial_bytes: u64,
    pub evictions: u64,
    pub corruption_rejections: u64,
    /// True only when verified objects survive construction of a new store.
    pub persistent: bool,
    /// True only when incomplete transfers can continue after reconstruction.
    pub resumable_across_reload: bool,
}

pub trait CacheStore {
    fn get_verified(&mut self, digest: &[u8; 32]) -> ClientResult<Option<BlobContent>>;
    fn put_verified(&mut self, digest: &[u8; 32], bytes: &[u8]) -> ClientResult<()>;
    fn contains(&self, digest: &[u8; 32]) -> bool;
    fn len(&self) -> usize;
    fn pin(&mut self, digest: &[u8; 32]) -> ClientResult<()>;
    fn unpin(&mut self, digest: &[u8; 32]) -> ClientResult<()>;
    fn remove(&mut self, digest: &[u8; 32]) -> ClientResult<bool>;
    fn stats(&self) -> CacheStoreStats;
}

/// Digest-keyed, byte-budgeted LRU for portable/static clients.
///
/// Bytes are hashed before admission and held behind immutable `Arc` slices,
/// so a successful lookup needs no second trust boundary. Pins protect
/// entries only for this store's (normally one page's) lifetime.
pub struct MemoryCacheStore {
    entries: HashMap<[u8; 32], MemoryEntry>,
    pins: HashSet<[u8; 32]>,
    used_bytes: u64,
    byte_budget: u64,
    clock: u64,
    evictions: u64,
    corruption_rejections: u64,
}

struct MemoryEntry {
    bytes: Arc<[u8]>,
    last_used: u64,
}

impl MemoryCacheStore {
    pub fn new(byte_budget: u64) -> Self {
        Self {
            entries: HashMap::new(),
            pins: HashSet::new(),
            used_bytes: 0,
            byte_budget,
            clock: 0,
            evictions: 0,
            corruption_rejections: 0,
        }
    }

    pub fn byte_budget(&self) -> u64 {
        self.byte_budget
    }

    fn touch_clock(&mut self) -> u64 {
        self.clock = self.clock.saturating_add(1);
        self.clock
    }

    fn pinned_bytes(&self) -> u64 {
        self.pins
            .iter()
            .filter_map(|digest| self.entries.get(digest))
            .map(|entry| entry.bytes.len() as u64)
            .sum()
    }

    fn evict_for(&mut self, incoming: u64) -> ClientResult<()> {
        if incoming > self.byte_budget {
            return Err(ClientError::CacheAdmission {
                what: "memory object over total budget",
                limit: self.byte_budget,
                found: incoming,
            });
        }
        let immovable = self.pinned_bytes();
        if immovable.saturating_add(incoming) > self.byte_budget {
            return Err(ClientError::CacheAdmission {
                what: "memory pins leave no room",
                limit: self.byte_budget,
                found: immovable.saturating_add(incoming),
            });
        }
        while self.used_bytes.saturating_add(incoming) > self.byte_budget {
            let Some(victim) = self
                .entries
                .iter()
                .filter(|(digest, _)| !self.pins.contains(*digest))
                .min_by_key(|(digest, entry)| (entry.last_used, **digest))
                .map(|(digest, _)| *digest)
            else {
                return Err(ClientError::CacheAdmission {
                    what: "memory pins leave no room",
                    limit: self.byte_budget,
                    found: self.used_bytes.saturating_add(incoming),
                });
            };
            if let Some(entry) = self.entries.remove(&victim) {
                self.used_bytes = self.used_bytes.saturating_sub(entry.bytes.len() as u64);
                self.evictions = self.evictions.saturating_add(1);
            }
        }
        Ok(())
    }
}

impl CacheStore for MemoryCacheStore {
    fn get_verified(&mut self, digest: &[u8; 32]) -> ClientResult<Option<BlobContent>> {
        let clock = self.touch_clock();
        let Some(entry) = self.entries.get_mut(digest) else {
            return Ok(None);
        };
        entry.last_used = clock;
        Ok(Some(BlobContent::Bytes(Arc::clone(&entry.bytes))))
    }

    fn put_verified(&mut self, digest: &[u8; 32], bytes: &[u8]) -> ClientResult<()> {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let found = hasher.finalize();
        if &found != digest {
            self.corruption_rejections = self.corruption_rejections.saturating_add(1);
            return Err(ClientError::DigestMismatch {
                what: "memory cache admission",
                expected: *digest,
                found,
            });
        }
        let clock = self.touch_clock();
        if let Some(entry) = self.entries.get_mut(digest) {
            entry.last_used = clock;
            return Ok(());
        }
        let incoming = bytes.len() as u64;
        self.evict_for(incoming)?;
        self.entries.insert(
            *digest,
            MemoryEntry { bytes: Arc::from(bytes), last_used: clock },
        );
        self.used_bytes = self.used_bytes.saturating_add(incoming);
        Ok(())
    }

    fn contains(&self, digest: &[u8; 32]) -> bool {
        self.entries.contains_key(digest)
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn pin(&mut self, digest: &[u8; 32]) -> ClientResult<()> {
        self.pins.insert(*digest);
        Ok(())
    }

    fn unpin(&mut self, digest: &[u8; 32]) -> ClientResult<()> {
        self.pins.remove(digest);
        Ok(())
    }

    fn remove(&mut self, digest: &[u8; 32]) -> ClientResult<bool> {
        let Some(entry) = self.entries.remove(digest) else {
            return Ok(false);
        };
        self.used_bytes = self.used_bytes.saturating_sub(entry.bytes.len() as u64);
        Ok(true)
    }

    fn stats(&self) -> CacheStoreStats {
        CacheStoreStats {
            object_count: self.entries.len() as u64,
            total_bytes: self.used_bytes,
            pinned_bytes: self.pinned_bytes(),
            partial_bytes: 0,
            evictions: self.evictions,
            corruption_rejections: self.corruption_rejections,
            persistent: false,
            resumable_across_reload: false,
        }
    }
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "web")))]
pub struct FsCacheStore {
    cache: crate::cache::ContentCache,
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "web")))]
impl FsCacheStore {
    pub fn open(
        root: &std::path::Path,
        budgets: crate::cache::CacheBudgets,
    ) -> ClientResult<Self> {
        Ok(Self {
            cache: crate::cache::ContentCache::open(root, budgets, crate::util::now_ms())?,
        })
    }

    pub fn from_content_cache(cache: crate::cache::ContentCache) -> Self {
        Self { cache }
    }

    pub fn into_content_cache(self) -> crate::cache::ContentCache {
        self.cache
    }
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "web")))]
impl CacheStore for FsCacheStore {
    fn get_verified(&mut self, digest: &[u8; 32]) -> ClientResult<Option<BlobContent>> {
        self.cache
            .resolve(digest, crate::util::now_ms())
            .map(|path| path.map(BlobContent::VerifiedPath))
    }

    fn put_verified(&mut self, digest: &[u8; 32], bytes: &[u8]) -> ClientResult<()> {
        self.cache
            .put_bytes(bytes, Some(digest), crate::util::now_ms())
            .map(|_| ())
    }

    fn contains(&self, digest: &[u8; 32]) -> bool {
        self.cache.contains(digest)
    }

    fn len(&self) -> usize {
        self.cache.stats().object_count as usize
    }

    fn pin(&mut self, digest: &[u8; 32]) -> ClientResult<()> {
        self.cache.pin(digest)
    }

    fn unpin(&mut self, digest: &[u8; 32]) -> ClientResult<()> {
        self.cache.unpin(digest)
    }

    fn remove(&mut self, digest: &[u8; 32]) -> ClientResult<bool> {
        self.cache.remove(digest)
    }

    fn stats(&self) -> CacheStoreStats {
        let stats = self.cache.stats();
        CacheStoreStats {
            object_count: stats.object_count,
            total_bytes: stats.total_bytes,
            pinned_bytes: stats.pinned_bytes,
            partial_bytes: stats.partial_bytes,
            evictions: stats.evictions,
            corruption_rejections: stats.corruption_evictions,
            persistent: true,
            resumable_across_reload: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_asset_data::BlobId;

    fn digest(bytes: &[u8]) -> [u8; 32] {
        *BlobId::hash_of(bytes).as_bytes()
    }

    #[test]
    fn memory_budget_evicts_the_least_recently_used() {
        let mut cache = MemoryCacheStore::new(8);
        let (a, b, c) = (b"aaaa", b"bbbb", b"cccc");
        cache.put_verified(&digest(a), a).unwrap();
        cache.put_verified(&digest(b), b).unwrap();
        assert!(cache.get_verified(&digest(a)).unwrap().is_some());
        cache.put_verified(&digest(c), c).unwrap();
        assert!(cache.contains(&digest(a)));
        assert!(!cache.contains(&digest(b)));
        assert!(cache.contains(&digest(c)));
        assert_eq!(cache.stats().evictions, 1);
    }

    #[test]
    fn memory_pin_survives_pressure_for_the_store_lifetime() {
        let mut cache = MemoryCacheStore::new(8);
        let (a, b, c) = (b"aaaa", b"bbbb", b"cccc");
        cache.put_verified(&digest(a), a).unwrap();
        cache.pin(&digest(a)).unwrap();
        cache.put_verified(&digest(b), b).unwrap();
        cache.put_verified(&digest(c), c).unwrap();
        assert!(cache.contains(&digest(a)));
        assert!(cache.contains(&digest(c)));
        assert!(!cache.contains(&digest(b)));
        assert_eq!(cache.stats().pinned_bytes, 4);
        assert!(!cache.stats().persistent);
        assert!(!cache.stats().resumable_across_reload);
    }

    #[test]
    fn memory_hashes_before_admission() {
        let mut cache = MemoryCacheStore::new(64);
        let expected = digest(b"good");
        assert!(matches!(
            cache.put_verified(&expected, b"tampered"),
            Err(ClientError::DigestMismatch { what: "memory cache admission", .. })
        ));
        assert!(!cache.contains(&expected));
        assert_eq!(cache.stats().corruption_rejections, 1);
    }

    #[cfg(all(not(target_arch = "wasm32"), not(feature = "web")))]
    #[test]
    fn filesystem_adapter_preserves_verified_paths_pins_and_stats() {
        let root = std::env::temp_dir().join(format!(
            "mp-fs-cache-store-{}-{}",
            std::process::id(),
            crate::util::now_ms()
        ));
        let mut budgets = crate::cache::CacheBudgets::default_v1();
        budgets.max_total_bytes = 1024;
        budgets.max_object_bytes = 1024;
        let mut cache = FsCacheStore::open(&root, budgets).unwrap();
        let bytes = b"verified";
        let digest = digest(bytes);
        cache.put_verified(&digest, bytes).unwrap();
        cache.pin(&digest).unwrap();
        let content = cache.get_verified(&digest).unwrap().unwrap();
        assert!(matches!(content, BlobContent::VerifiedPath(_)));
        assert_eq!(cache.stats().pinned_bytes, bytes.len() as u64);
        assert!(cache.stats().persistent);
        assert!(cache.stats().resumable_across_reload);
        drop(cache);
        let _ = std::fs::remove_dir_all(root);
    }
}

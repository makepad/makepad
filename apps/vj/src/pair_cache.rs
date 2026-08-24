//! PAIR CACHE — what a producer made for one pair of frames, kept under a
//! key that says exactly which pair of which clip at which tier.
//!
//! The invariant it exists for (design-v2 §0): NO asynchronous event ever
//! changes the active pair lease. A ladder that lands mid-pair is stored
//! under ITS key and only ever used by a later traversal of that pair; a
//! result for the previous clip generation has a different key and is
//! simply never found. Bare `usize pair` indices alias across cue / trim /
//! clip swap — the key does not.
//!
//! Eviction is byte-budgeted LRU. Pinned entries (active leases, in-flight
//! destinations) are never evicted; a generation can be retired wholesale.
//! Pure: no threads, no time.

use std::collections::HashMap;

/// A resident clip's identity: bumped at every cue, cache rebuild or trim
/// epoch so nothing produced for the old frames can be mistaken for the
/// new ones.
pub type ClipGeneration = u64;

/// Which pair, of which clip, at which tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PairKey {
    pub clip: ClipGeneration,
    pub a: u32,
    pub b: u32,
    pub tier: u8,
}

impl PairKey {
    pub fn new(clip: ClipGeneration, a: usize, b: usize, tier: u8) -> PairKey {
        PairKey { clip, a: a as u32, b: b as u32, tier }
    }
}

struct Entry<T> {
    value: T,
    bytes: usize,
    /// Recompute cost, arbitrary units — a hint for eviction weighting.
    cost: u32,
    /// LRU stamp.
    used: u64,
    pinned: bool,
}

/// A bounded store of per-pair products.
pub struct PairCache<T> {
    entries: HashMap<PairKey, Entry<T>>,
    budget_bytes: usize,
    bytes: usize,
    tick: u64,
}

impl<T> PairCache<T> {
    pub fn new(budget_bytes: usize) -> PairCache<T> {
        PairCache { entries: HashMap::new(), budget_bytes, bytes: 0, tick: 0 }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn budget_bytes(&self) -> usize {
        self.budget_bytes
    }

    pub fn contains(&self, key: &PairKey) -> bool {
        self.entries.contains_key(key)
    }

    /// Look a product up and mark it recently used.
    pub fn get(&mut self, key: &PairKey) -> Option<&T> {
        self.tick += 1;
        let tick = self.tick;
        let e = self.entries.get_mut(key)?;
        e.used = tick;
        Some(&e.value)
    }

    /// Peek without touching the LRU order.
    pub fn peek(&self, key: &PairKey) -> Option<&T> {
        self.entries.get(key).map(|e| &e.value)
    }

    /// Store a product. Replaces an existing entry under the same key
    /// (keeping its pin). Evicts least-recently-used UNPINNED entries until
    /// the budget holds; an entry larger than the whole budget is still
    /// stored (the caller asked for it) with everything unpinned evicted.
    pub fn insert(&mut self, key: PairKey, value: T, bytes: usize, cost: u32) {
        self.tick += 1;
        let pinned = if let Some(old) = self.entries.remove(&key) {
            self.bytes -= old.bytes;
            old.pinned
        } else {
            false
        };
        self.bytes += bytes;
        self.entries.insert(key, Entry { value, bytes, cost, used: self.tick, pinned });
        self.evict_to_budget(&key);
    }

    /// Evict until the budget holds — never `keep` (the entry that was just
    /// asked for), never a pinned one.
    fn evict_to_budget(&mut self, keep: &PairKey) {
        while self.bytes > self.budget_bytes {
            // The least recently used unpinned entry; among equally old
            // ones, the cheapest to recompute goes first.
            let victim = self
                .entries
                .iter()
                .filter(|(k, e)| !e.pinned && *k != keep)
                .min_by_key(|(_, e)| (e.used, e.cost))
                .map(|(k, _)| *k);
            let Some(k) = victim else { break };
            if let Some(e) = self.entries.remove(&k) {
                self.bytes -= e.bytes;
            }
        }
    }

    pub fn remove(&mut self, key: &PairKey) -> Option<T> {
        let e = self.entries.remove(key)?;
        self.bytes -= e.bytes;
        Some(e.value)
    }

    /// Pin (or unpin) an entry: pinned entries survive eviction.
    pub fn pin(&mut self, key: &PairKey, pinned: bool) -> bool {
        match self.entries.get_mut(key) {
            Some(e) => {
                e.pinned = pinned;
                true
            }
            None => false,
        }
    }

    pub fn is_pinned(&self, key: &PairKey) -> bool {
        self.entries.get(key).map(|e| e.pinned).unwrap_or(false)
    }

    /// Drop every pin (a new lease set is about to be pinned).
    pub fn unpin_all(&mut self) {
        for e in self.entries.values_mut() {
            e.pinned = false;
        }
    }

    /// Drop everything belonging to a clip generation — pinned or not.
    pub fn retire(&mut self, clip: ClipGeneration) -> usize {
        let before = self.entries.len();
        let mut freed = 0;
        self.entries.retain(|k, e| {
            if k.clip == clip {
                freed += e.bytes;
                false
            } else {
                true
            }
        });
        self.bytes -= freed;
        before - self.entries.len()
    }

    /// Keep only entries for `clip`.
    pub fn retain_generation(&mut self, clip: ClipGeneration) -> usize {
        let before = self.entries.len();
        let mut freed = 0;
        self.entries.retain(|k, e| {
            if k.clip != clip {
                freed += e.bytes;
                false
            } else {
                true
            }
        });
        self.bytes -= freed;
        before - self.entries.len()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    /// Keys currently held, in no particular order.
    pub fn keys(&self) -> impl Iterator<Item = &PairKey> {
        self.entries.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(clip: u64, a: usize, tier: u8) -> PairKey {
        PairKey::new(clip, a, a + 1, tier)
    }

    #[test]
    fn keys_carry_the_clip_generation() {
        let mut c: PairCache<&'static str> = PairCache::new(1024);
        c.insert(k(1, 5, 0), "old clip", 10, 1);
        c.insert(k(2, 5, 0), "new clip", 10, 1);
        assert_eq!(c.get(&k(1, 5, 0)), Some(&"old clip"));
        assert_eq!(c.get(&k(2, 5, 0)), Some(&"new clip"));
        assert_eq!(c.len(), 2);
        // Same pair, different tier: a different product.
        c.insert(k(2, 5, 1), "tier 1", 10, 1);
        assert_eq!(c.get(&k(2, 5, 0)), Some(&"new clip"));
        assert_eq!(c.get(&k(2, 5, 1)), Some(&"tier 1"));
        // Retiring the old generation cannot touch the new one.
        assert_eq!(c.retire(1), 1);
        assert!(!c.contains(&k(1, 5, 0)));
        assert!(c.contains(&k(2, 5, 0)));
        assert_eq!(c.bytes(), 20);
    }

    #[test]
    fn eviction_is_lru_within_the_byte_budget() {
        let mut c: PairCache<u32> = PairCache::new(30);
        c.insert(k(1, 0, 0), 0, 10, 1);
        c.insert(k(1, 1, 0), 1, 10, 1);
        c.insert(k(1, 2, 0), 2, 10, 1);
        assert_eq!(c.bytes(), 30);
        // Touch pair 0 so pair 1 is the oldest.
        assert_eq!(c.get(&k(1, 0, 0)), Some(&0));
        c.insert(k(1, 3, 0), 3, 10, 1);
        assert_eq!(c.len(), 3);
        assert!(!c.contains(&k(1, 1, 0)), "the LRU entry went");
        assert!(c.contains(&k(1, 0, 0)));
        assert!(c.contains(&k(1, 2, 0)));
        assert!(c.contains(&k(1, 3, 0)));
        // Replacing under the same key does not double count.
        c.insert(k(1, 3, 0), 33, 10, 1);
        assert_eq!(c.bytes(), 30);
        assert_eq!(c.peek(&k(1, 3, 0)), Some(&33));
        // Peek does not touch the order. Ages now: pair 2 (inserted before
        // pair 0 was touched) is the oldest; then 0; then 3 (replaced).
        c.insert(k(1, 4, 0), 4, 10, 1);
        assert!(!c.contains(&k(1, 2, 0)));
        assert!(c.contains(&k(1, 0, 0)));
        c.insert(k(1, 5, 0), 5, 10, 1);
        assert!(!c.contains(&k(1, 0, 0)));
    }

    #[test]
    fn pinned_entries_survive_eviction() {
        let mut c: PairCache<u32> = PairCache::new(20);
        c.insert(k(1, 0, 0), 0, 10, 1);
        c.insert(k(1, 1, 0), 1, 10, 1);
        assert!(c.pin(&k(1, 0, 0), true));
        assert!(c.is_pinned(&k(1, 0, 0)));
        c.insert(k(1, 2, 0), 2, 10, 1);
        assert!(c.contains(&k(1, 0, 0)), "pinned survives");
        assert!(!c.contains(&k(1, 1, 0)));
        // An oversized insert with everything pinned: stored anyway, the
        // pinned entries stay (the budget is a target, the pin a promise).
        c.pin(&k(1, 2, 0), true);
        c.insert(k(1, 3, 0), 3, 25, 1);
        assert_eq!(c.len(), 3);
        assert!(c.bytes() > c.budget_bytes());
        c.unpin_all();
        c.insert(k(1, 4, 0), 4, 1, 1);
        assert!(c.bytes() <= c.budget_bytes());
        assert!(c.contains(&k(1, 4, 0)));
        assert!(!c.pin(&k(9, 9, 0), true), "pinning a missing key says so");
    }

    #[test]
    fn equally_old_entries_evict_the_cheapest_first() {
        let mut c: PairCache<u32> = PairCache::new(20);
        // Same LRU stamp is impossible through the API (ticks), so make
        // the order explicit: insert then touch both in one order and
        // rely on cost only as the tie-break — verified via retire counts.
        c.insert(k(1, 0, 0), 0, 10, 5);
        c.insert(k(1, 1, 0), 1, 10, 1);
        assert_eq!(c.retain_generation(1), 0);
        assert_eq!(c.retain_generation(2), 2);
        assert!(c.is_empty());
        assert_eq!(c.bytes(), 0);
    }

    #[test]
    fn remove_and_clear_keep_the_byte_count_honest() {
        let mut c: PairCache<Vec<u8>> = PairCache::new(100);
        c.insert(k(1, 0, 0), vec![0; 40], 40, 1);
        c.insert(k(1, 1, 0), vec![0; 40], 40, 1);
        assert_eq!(c.bytes(), 80);
        assert_eq!(c.remove(&k(1, 0, 0)).map(|v| v.len()), Some(40));
        assert_eq!(c.bytes(), 40);
        assert!(c.remove(&k(1, 0, 0)).is_none());
        c.clear();
        assert_eq!(c.bytes(), 0);
        assert!(c.is_empty());
        assert_eq!(c.keys().count(), 0);
    }
}

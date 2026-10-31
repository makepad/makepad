use super::{next_id, DepNodeIndex, Fingerprint, QueryEngine, QueryError, QueryId};
use crate::fxhash::{FxHasher, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::{OnceLock, RwLock};

const SHARDS: usize = 32;

/// Values dropped by `retire` and recomputed by a later `get` (a green node whose value was retired).
pub static RETIRED_VALUES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static RECOMPUTED_VALUES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct Slot<K, V> {
    key: K,
    value: Option<V>,
    verified_at: u64,
    // This index also addresses the engine's in-progress job slot.
    node: DepNodeIndex,
}

struct Shard<K, V> {
    keys: HashMap<K, u32>,
    slots: Vec<Slot<K, V>>,
}

/// A per-kind key interner, value cache and handle to in-progress state. Hits need
/// only a shared shard lock. Values should be cheap to clone (ids or Arc handles).
/// rustc_middle/src/query/caches.rs:16,42 separates typed values from dep-node indices;
/// rustc_middle/src/query/plumbing.rs:79 associates each query with its cache/state.
///
/// A table binds permanently to its first engine and exclusively owns its kind.
/// `new` is const so concrete queries can own static tables without generated code.
/// Key/value hashing functions must encode stable semantic identities with Sip128;
/// ordinary Hash is used only for the in-memory shard and interner.
/// Every call for a kind must use the same provider semantics and force dispatcher.
pub struct QueryTable<K, V> {
    kind: u16,
    key_fingerprint: fn(&K) -> Fingerprint,
    value_fingerprint: fn(&V) -> Fingerprint,
    owner: OnceLock<Result<u64, QueryError>>,
    shards: OnceLock<Vec<RwLock<Shard<K, V>>>>,
}

impl<K: Hash + Eq + Clone, V: Clone> QueryTable<K, V> {
    pub const fn new(kind: u16, key_fingerprint: fn(&K) -> Fingerprint, value_fingerprint: fn(&V) -> Fingerprint) -> Self {
        Self { kind, key_fingerprint, value_fingerprint, owner: OnceLock::new(), shards: OnceLock::new() }
    }

    fn bind(&self, engine: &QueryEngine) -> Result<(), QueryError> {
        let owner = self.owner.get_or_init(|| {
            engine.claim_kind(self.kind, next_id())?;
            Ok(engine.id)
        });
        match owner {
            Ok(id) if *id == engine.id => Ok(()),
            Ok(_) => Err(QueryError::WrongEngine),
            Err(error) => Err(error.clone()),
        }
    }

    fn shards(&self) -> &[RwLock<Shard<K, V>>] {
        self.shards.get_or_init(|| {
            let mut shards = Vec::with_capacity(SHARDS);
            for _ in 0..SHARDS {
                shards.push(RwLock::new(Shard { keys: HashMap::default(), slots: Vec::new() }));
            }
            shards
        })
    }

    fn shard(key: &K) -> usize {
        let mut hash = FxHasher::default();
        key.hash(&mut hash);
        (hash.finish() >> 40) as usize % SHARDS
    }

    /// rustc_middle/src/query/caches.rs:31: inspect completed query entries.
    /// The callback must not enter this table while its shard is read-locked.
    pub fn for_each(&self, f: &mut dyn FnMut(&K, &V, DepNodeIndex)) {
        for shard in self.shards() {
            let shard = shard.read().unwrap_or_else(|e| e.into_inner());
            for slot in &shard.slots {
                if let Some(value) = &slot.value { f(&slot.key, value, slot.node); }
            }
        }
    }

    /// memory report: (slots, bytes) with `deep` per completed value
    pub fn mem_stats(&self, deep: &dyn Fn(&K, &V) -> usize) -> (usize, usize) {
        let (mut n, mut b) = (0usize, 0usize);
        for shard in self.shards() {
            let shard = shard.read().unwrap_or_else(|e| e.into_inner());
            n += shard.slots.len();
            b += shard.slots.capacity() * std::mem::size_of::<Slot<K, V>>() + shard.keys.capacity() * (std::mem::size_of::<(K, u32)>() + 1) * 8 / 7;
            for s in &shard.slots { b += s.value.as_ref().map_or(0, |v| deep(&s.key, v)); }
        }
        (n, b)
    }

    /// Recover an interned key for the context's kind-dispatching force function.
    pub fn key(&self, index: u32) -> Option<K> {
        let shard = &self.shards()[index as usize % SHARDS];
        let shard = shard.read().unwrap_or_else(|e| e.into_inner());
        Some(shard.slots.get(index as usize / SHARDS)?.key.clone())
    }

    pub fn query_id(&self, key: &K) -> Option<QueryId> {
        let shard_index = Self::shard(key);
        let shard = self.shards()[shard_index].read().unwrap_or_else(|e| e.into_inner());
        let &slot = shard.keys.get(key)?;
        Some(QueryId { kind: self.kind, key: slot * SHARDS as u32 + shard_index as u32 })
    }

    pub fn get(
        &self,
        engine: &QueryEngine,
        key: K,
        compute: &mut dyn FnMut(K) -> Result<V, QueryError>,
        force: &dyn Fn(QueryId) -> Result<(), QueryError>,
    ) -> Result<V, QueryError> {
        self.bind(engine)?;
        let shard_index = Self::shard(&key);
        let shard_lock = &self.shards()[shard_index];
        let found = {
            let shard = shard_lock.read().unwrap_or_else(|e| e.into_inner());
            match shard.keys.get(&key) {
                Some(&slot_index) => {
                    let slot = &shard.slots[slot_index as usize];
                    if slot.verified_at == engine.revision() {
                        if let Some(value) = slot.value.as_ref() {
                            let value = value.clone();
                            engine.read_index(slot.node);
                            return Ok(value);
                        }
                    }
                    Some((slot_index, slot.node))
                }
                None => None,
            }
        };
        let (slot_index, node) = match found {
            Some(found) => found,
            None => {
                let fingerprint = (self.key_fingerprint)(&key);
                let mut shard = shard_lock.write().unwrap_or_else(|e| e.into_inner());
                match shard.keys.get(&key) {
                    Some(&index) => (index, shard.slots[index as usize].node),
                    None => {
                        let index = u32::try_from(shard.slots.len()).expect("query key limit");
                        let key_index =
                            index.checked_mul(SHARDS as u32).and_then(|v| v.checked_add(shard_index as u32)).expect("query key limit");
                        let node = engine.intern(QueryId { kind: self.kind, key: key_index }, fingerprint, false)?;
                        shard.slots.push(Slot { key: key.clone(), value: None, verified_at: 0, node });
                        shard.keys.insert(key.clone(), index);
                        (index, node)
                    }
                }
            }
        };
        engine.with_task(
            node,
            &mut || {
                let value = compute(key.clone())?;
                let fingerprint = (self.value_fingerprint)(&value);
                shard_lock.write().unwrap_or_else(|e| e.into_inner()).slots[slot_index as usize].value = Some(value);
                Ok(fingerprint)
            },
            force,
        )?;
        let value = {
            let mut shard = shard_lock.write().unwrap_or_else(|e| e.into_inner());
            let slot = &mut shard.slots[slot_index as usize];
            slot.verified_at = engine.revision();
            slot.value.clone()
        };
        // A retired value (`retire`) on a green node: the node's fingerprint stands, the value is
        // derived again from the same provider (its reads join the caller's, a superset).
        let value = match value {
            Some(value) => value,
            None => {
                let value = compute(key.clone())?;
                RECOMPUTED_VALUES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                shard_lock.write().unwrap_or_else(|e| e.into_inner()).slots[slot_index as usize].value = Some(value.clone());
                value
            }
        };
        engine.read_index(node);
        Ok(value)
    }

    /// Drop the completed values `keep` does not want; the nodes stay green and `get` derives a
    /// dropped value again on demand. Returns how many were dropped.
    pub fn retire(&self, keep: &dyn Fn(&K, &V) -> bool) -> usize {
        let mut n = 0;
        for shard in self.shards() {
            let mut shard = shard.write().unwrap_or_else(|e| e.into_inner());
            for slot in shard.slots.iter_mut() {
                if slot.value.as_ref().is_some_and(|v| !keep(&slot.key, v)) { slot.value = None; n += 1; }
            }
        }
        RETIRED_VALUES.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
        n
    }
}

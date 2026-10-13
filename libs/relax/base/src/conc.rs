//! Concurrent memo tables for the front end: a map split into lock shards by key hash, so
//! bodies type-checked on several threads read and fill the same process-wide caches
//! (rustc's sharded query caches, rustc_data_structures::sharded). Values are small and Clone.

use crate::fxhash::{FxHasher, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::RwLock;

const SHARDS: usize = 32;

pub struct ShardMap<K, V> {
    shards: Box<[RwLock<HashMap<K, V>>]>,
}

impl<K: Hash + Eq, V: Clone> Default for ShardMap<K, V> {
    fn default() -> Self {
        ShardMap { shards: (0..SHARDS).map(|_| RwLock::new(HashMap::default())).collect() }
    }
}

impl<K: Hash + Eq, V: Clone> ShardMap<K, V> {
    #[inline]
    fn shard<Q: Hash + ?Sized>(&self, k: &Q) -> &RwLock<HashMap<K, V>> {
        let mut h = FxHasher::default();
        k.hash(&mut h);
        // the high bits: the low ones also pick the bucket inside the shard's table
        &self.shards[(h.finish() >> 40) as usize % SHARDS]
    }

    #[inline]
    pub fn get(&self, k: &K) -> Option<V> {
        self.shard(k).read().unwrap_or_else(|e| e.into_inner()).get(k).cloned()
    }

    /// `get` by a borrowed form of the key (`&[T]` for an `Arc<[T]>` key: no allocation).
    #[inline]
    pub fn get_by<Q: Hash + Eq + ?Sized>(&self, k: &Q) -> Option<V> where K: std::borrow::Borrow<Q> {
        self.shard(k).read().unwrap_or_else(|e| e.into_inner()).get(k).cloned()
    }

    #[inline]
    pub fn insert(&self, k: K, v: V) {
        self.shard(&k).write().unwrap_or_else(|e| e.into_inner()).insert(k, v);
    }

    pub fn clear(&self) {
        for s in self.shards.iter() {
            s.write().unwrap_or_else(|e| e.into_inner()).clear();
        }
    }

    /// set membership (a ShardMap<K, ()> used as a set)
    pub fn contains(&self, k: &K) -> bool {
        self.contains_key(k)
    }

    pub fn contains_key(&self, k: &K) -> bool {
        self.shard(k).read().unwrap_or_else(|e| e.into_inner()).contains_key(k)
    }

    pub fn remove(&self, k: &K) -> Option<V> {
        self.shard(k).write().unwrap_or_else(|e| e.into_inner()).remove(k)
    }

    pub fn retain(&self, mut f: impl FnMut(&K, &mut V) -> bool) {
        for s in self.shards.iter() {
            s.write().unwrap_or_else(|e| e.into_inner()).retain(|k, v| f(k, v));
        }
    }

    pub fn len(&self) -> usize {
        self.shards.iter().map(|s| s.read().unwrap_or_else(|e| e.into_inner()).len()).sum()
    }

    /// Memory report: (entries, bytes): the shards' tables at capacity plus `deep` per entry.
    pub fn mem_stats(&self, deep: impl Fn(&K, &V) -> usize) -> (usize, usize) {
        let (mut n, mut b) = (0usize, 0usize);
        for s in self.shards.iter() {
            let m = s.read().unwrap_or_else(|e| e.into_inner());
            n += m.len();
            b += m.capacity() * (std::mem::size_of::<(K, V)>() + 1) * 8 / 7;
            b += m.iter().map(|(k, v)| deep(k, v)).sum::<usize>();
        }
        (n, b)
    }

    /// Copies the published entries. A coherent graph checkpoint excludes writers.
    pub fn snapshot(&self) -> HashMap<K, V> where K: Clone {
        self.shards.iter().flat_map(|s| {
            s.read().unwrap_or_else(|e| e.into_inner()).iter()
                .map(|(k, v)| (k.clone(), v.clone())).collect::<Vec<_>>()
        }).collect()
    }
}

/// A small dense id for the current thread (for per-thread entries in shared tables: a query's
/// in-progress marker is visible to its own thread only, so cycle cuts never leak to others).
pub fn thread_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    // no destructor (const-initialised Cell): usable during thread teardown
    thread_local! {
        static ID: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }
    ID.with(|x| {
        if x.get() == 0 {
            x.set(NEXT.fetch_add(1, Ordering::Relaxed));
        }
        x.get()
    })
}

/// A memo table read without locks: a fixed array of hash buckets whose chains are immutable
/// once published (a node is complete before the bucket head points at it, Release/Acquire);
/// inserts take one of a few stripe locks and prepend, so the newest value of a key wins.
/// `clear` swaps in a fresh table; the old one stays allocated (readers may still walk it)
/// until the map is dropped. For caches filled once and read many times from many threads,
/// where an RwLock's shared counter would bounce between cores.
pub struct MemoMap<K, V> {
    table: std::sync::atomic::AtomicPtr<MemoTable<K, V>>,
    graveyard: relax_session::sync::Mutex<Vec<*mut MemoTable<K, V>>>,
    locks: Box<[relax_session::sync::Mutex<()>]>,
    bits: u32,
    /// the table, its nodes and keys live in the kept heap (zone_alloc::kept), not the compiler arena
    kept: bool,
}

struct MemoTable<K, V> {
    buckets: Box<[std::sync::atomic::AtomicPtr<MemoNode<K, V>>]>,
}

struct MemoNode<K, V> {
    hash: u64,
    key: K,
    val: V,
    next: *mut MemoNode<K, V>,
}

unsafe impl<K: Send + Sync, V: Send + Sync> Send for MemoMap<K, V> {}
unsafe impl<K: Send + Sync, V: Send + Sync> Sync for MemoMap<K, V> {}

const MEMO_LOCKS: usize = 64;

impl<K, V> MemoTable<K, V> {
    fn new(bits: u32) -> *mut MemoTable<K, V> {
        // All-zero bytes are null pointers: a large fresh zeroed allocation's pages stay
        // untouched until a bucket is used (a Program/Tcx holds dozens of 2^16-bucket tables).
        let buckets = unsafe { Box::<[std::sync::atomic::AtomicPtr<MemoNode<K, V>>]>::new_zeroed_slice(1usize << bits).assume_init() };
        Box::into_raw(Box::new(MemoTable { buckets }))
    }

    unsafe fn free(t: *mut MemoTable<K, V>) {
        let t = Box::from_raw(t);
        for b in t.buckets.iter() {
            let mut p = b.load(std::sync::atomic::Ordering::Relaxed);
            while !p.is_null() {
                let n = Box::from_raw(p);
                p = n.next;
            }
        }
    }
}

impl<K, V> Drop for MemoMap<K, V> {
    fn drop(&mut self) {
        unsafe {
            MemoTable::free(*self.table.get_mut());
            for t in self.graveyard.get_mut().unwrap_or_else(|e| e.into_inner()).drain(..) {
                MemoTable::free(t);
            }
        }
    }
}

impl<K: Hash + Eq, V: Clone> Default for MemoMap<K, V> {
    fn default() -> Self {
        Self::with_bits(16)
    }
}

impl<K: Hash + Eq, V: Clone> MemoMap<K, V> {
    /// A table of 2^bits buckets (chains grow past that, gracefully).
    pub fn with_bits(bits: u32) -> Self {
        Self::build(bits, false)
    }

    /// `with_bits` for state that must outlive the compiler arena: every allocation the map
    /// makes itself (table, nodes) is in the kept heap. Keys and values the caller built before
    /// `insert` are the caller's to build in the kept phase.
    pub fn new_kept(bits: u32) -> Self {
        Self::build(bits, true)
    }

    fn build(bits: u32, kept: bool) -> Self {
        let _k = if kept { Some(crate::zone_alloc::kept()) } else { None };
        MemoMap {
            table: std::sync::atomic::AtomicPtr::new(MemoTable::new(bits)),
            graveyard: relax_session::sync::Mutex::new(Vec::new()),
            locks: (0..MEMO_LOCKS).map(|_| relax_session::sync::Mutex::new(())).collect(),
            bits,
            kept,
        }
    }

    #[inline]
    fn phase(&self) -> Option<crate::zone_alloc::PhaseGuard> {
        if self.kept { Some(crate::zone_alloc::kept()) } else { None }
    }

    #[inline]
    fn hash_of<Q: Hash + ?Sized>(k: &Q) -> u64 {
        let mut h = FxHasher::default();
        k.hash(&mut h);
        h.finish()
    }

    #[inline]
    fn bucket(&self, h: u64) -> usize {
        (h >> (64 - self.bits)) as usize
    }

    #[inline]
    fn find(&self, k: &K) -> Option<&V> {
        let h = Self::hash_of(k);
        let t = unsafe { &*self.table.load(std::sync::atomic::Ordering::Acquire) };
        let mut p = t.buckets[self.bucket(h)].load(std::sync::atomic::Ordering::Acquire);
        while !p.is_null() {
            let n = unsafe { &*p };
            if n.hash == h && n.key == *k {
                return Some(&n.val);
            }
            p = n.next;
        }
        None
    }

    #[inline]
    pub fn get(&self, k: &K) -> Option<V> {
        self.find(k).cloned()
    }

    #[inline]
    pub fn contains(&self, k: &K) -> bool {
        self.find(k).is_some()
    }

    #[inline]
    pub fn contains_key(&self, k: &K) -> bool {
        self.find(k).is_some()
    }

    /// Copies the current table, keeping the newest value for each key. Retired tables
    /// remain alive for readers; a coherent checkpoint additionally excludes writers.
    pub fn snapshot(&self) -> HashMap<K, V> where K: Clone {
        let mut out = HashMap::default();
        let table = unsafe { &*self.table.load(std::sync::atomic::Ordering::Acquire) };
        for bucket in &table.buckets {
            let mut ptr = bucket.load(std::sync::atomic::Ordering::Acquire);
            while !ptr.is_null() {
                let node = unsafe { &*ptr };
                out.entry(node.key.clone()).or_insert_with(|| node.val.clone());
                ptr = node.next;
            }
        }
        out
    }

    pub fn insert(&self, k: K, v: V) {
        let h = Self::hash_of(&k);
        let b = self.bucket(h);
        let _l = self.locks[b % MEMO_LOCKS].lock().unwrap_or_else(|e| e.into_inner());
        let t = unsafe { &*self.table.load(std::sync::atomic::Ordering::Acquire) };
        let head = t.buckets[b].load(std::sync::atomic::Ordering::Acquire);
        let _k = self.phase();
        let n = Box::into_raw(Box::new(MemoNode { hash: h, key: k, val: v, next: head }));
        t.buckets[b].store(n, std::sync::atomic::Ordering::Release);
    }

    /// Inserts `v` unless the key is already there; returns the value the map holds for it (the
    /// first one inserted: racing builders of one memo entry all get the same value).
    pub fn insert_first(&self, k: K, v: V) -> V {
        let h = Self::hash_of(&k);
        let b = self.bucket(h);
        let _l = self.locks[b % MEMO_LOCKS].lock().unwrap_or_else(|e| e.into_inner());
        if let Some(x) = self.find(&k) {
            return x.clone();
        }
        let t = unsafe { &*self.table.load(std::sync::atomic::Ordering::Acquire) };
        let head = t.buckets[b].load(std::sync::atomic::Ordering::Acquire);
        let _k = self.phase();
        let n = Box::into_raw(Box::new(MemoNode { hash: h, key: k, val: v.clone(), next: head }));
        t.buckets[b].store(n, std::sync::atomic::Ordering::Release);
        v
    }

    /// The kept-state scan (MAKEPAD_RUN_ZONE_CHECK): `own` sees each allocation the map owns (tables,
    /// bucket arrays, nodes), `entry` each live entry's key and value. Quiescent callers only.
    pub fn scan(&self, mut own: impl FnMut(*const u8), mut entry: impl FnMut(&K, &V)) {
        let tables: Vec<*mut MemoTable<K, V>> = {
            let g = self.graveyard.lock().unwrap_or_else(|e| e.into_inner());
            std::iter::once(self.table.load(std::sync::atomic::Ordering::Acquire)).chain(g.iter().copied()).collect()
        };
        own(self as *const Self as *const u8);
        for (ti, t) in tables.into_iter().enumerate() {
            own(t as *const u8);
            let t = unsafe { &*t };
            own(t.buckets.as_ptr() as *const u8);
            for bucket in t.buckets.iter() {
                let mut p = bucket.load(std::sync::atomic::Ordering::Acquire);
                while !p.is_null() {
                    own(p as *const u8);
                    let n = unsafe { &*p };
                    if ti == 0 {
                        entry(&n.key, &n.val);
                    }
                    p = n.next;
                }
            }
        }
    }

    /// Memory report: (entries, bytes) of the current table, `deep` adding each entry's own heap
    /// (nodes and buckets counted here). Reads like `get`: nodes are freed only under exclusion.
    pub fn mem_stats(&self, deep: impl Fn(&K, &V) -> usize) -> (usize, usize) {
        let t = unsafe { &*self.table.load(std::sync::atomic::Ordering::Acquire) };
        let (mut n, mut b) = (0usize, t.buckets.len() * 8);
        for bucket in t.buckets.iter() {
            let mut p = bucket.load(std::sync::atomic::Ordering::Acquire);
            while !p.is_null() {
                let node = unsafe { &*p };
                n += 1;
                b += std::mem::size_of::<MemoNode<K, V>>() + deep(&node.key, &node.val);
                p = node.next;
            }
        }
        (n, b)
    }

    pub fn clear(&self) {
        let _k = self.phase();
        let fresh = MemoTable::new(self.bits);
        let old = self.table.swap(fresh, std::sync::atomic::Ordering::AcqRel);
        self.graveyard.lock().unwrap_or_else(|e| e.into_inner()).push(old);
    }

    /// Removes memo entries and frees retired tables at a quiescent point.
    /// Safety: exclude ALL readers and writers (including suspended queries holding references).
    pub unsafe fn retain_exclusive(&self, mut keep: impl FnMut(&K, &V) -> bool) {
        let table = &*self.table.load(std::sync::atomic::Ordering::Relaxed);
        for bucket in table.buckets.iter() {
            let mut prev: *mut MemoNode<K, V> = std::ptr::null_mut();
            let mut node = bucket.load(std::sync::atomic::Ordering::Relaxed);
            while !node.is_null() {
                let next = (*node).next;
                if keep(&(*node).key, &(*node).val) {
                    prev = node;
                } else {
                    if prev.is_null() {
                        bucket.store(next, std::sync::atomic::Ordering::Relaxed);
                    } else {
                        (*prev).next = next;
                    }
                    drop(Box::from_raw(node));
                }
                node = next;
            }
        }
        self.reclaim();
    }

    /// `clear`, and the retired tables with it, handed back instead of freed: the entries are
    /// freed when the returned value drops (outside the caller's locks). Same safety as `reclaim`.
    pub unsafe fn detach(&self) -> Detached<K, V> {
        let _k = self.phase();
        let fresh = MemoTable::new(self.bits);
        let mut all = vec![self.table.swap(fresh, std::sync::atomic::Ordering::AcqRel)];
        all.extend(self.graveyard.lock().unwrap_or_else(|e| e.into_inner()).drain(..));
        Detached(all)
    }

    /// Frees the tables earlier clears retired. Safety: no reader may still be inside one of
    /// them (the caller excludes readers, e.g. holds the front-end write lock for a table only
    /// front-end code reads).
    pub unsafe fn reclaim(&self) {
        for t in self.graveyard.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
            MemoTable::free(t);
        }
    }
}

/// Tables a `MemoMap` gave up (`detach`): their entries are freed when this drops.
pub struct Detached<K, V>(Vec<*mut MemoTable<K, V>>);
unsafe impl<K: Send, V: Send> Send for Detached<K, V> {}
impl<K, V> Drop for Detached<K, V> {
    fn drop(&mut self) {
        for t in self.0.drain(..) { unsafe { MemoTable::free(t) } }
    }
}

/// An append-only vector whose entries never move: `push` takes `&self` (one short lock), reads
/// of published entries need no lock; `IndexMut` / `iter_mut` need `&mut self` (the jit's slot
/// table: entries are created by lowering on any thread, their build state is changed under the
/// unit lock).
pub struct AppendVec<T> {
    chunks: Box<[std::sync::atomic::AtomicPtr<T>]>,
    len: std::sync::atomic::AtomicUsize,
    grow: relax_session::sync::Mutex<()>,
    /// its chunk table and chunks live in the kept heap (zone_alloc::kept)
    kept: bool,
}

const AV_BITS: u32 = 12;
const AV_CHUNK: usize = 1 << AV_BITS;

unsafe impl<T: Send> Send for AppendVec<T> {}
unsafe impl<T: Send + Sync> Sync for AppendVec<T> {}

impl<T> AppendVec<T> {
    /// Room for `cap` entries (rounded up to whole chunks).
    pub fn with_capacity(cap: usize) -> Self {
        Self::build(cap, false)
    }

    /// `with_capacity` for state that must outlive the compiler arena: the chunk table and
    /// every chunk are allocated in the kept heap. Heap data the entries own is the pusher's to
    /// build in the kept phase.
    pub fn new_kept(cap: usize) -> Self {
        Self::build(cap, true)
    }

    fn build(cap: usize, kept: bool) -> Self {
        let _k = if kept { Some(crate::zone_alloc::kept()) } else { None };
        let n = (cap + AV_CHUNK - 1) / AV_CHUNK;
        // zeroed pages (null pointers) straight from the allocator: a table for every u32 index stays virtual until chunks are used
        AppendVec { chunks: unsafe { Box::<[std::sync::atomic::AtomicPtr<T>]>::new_zeroed_slice(n).assume_init() }, len: Default::default(), grow: Default::default(), kept }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Makes room for entries up to index `upto` (allocating their chunks now); `fresh(ptr,
    /// bytes)` sees each chunk allocated here (a caller about to fill many: prefault it).
    pub fn reserve(&self, upto: usize, fresh: impl Fn(*mut u8, usize)) {
        use std::sync::atomic::Ordering;
        let _g = self.grow.lock().unwrap_or_else(|e| e.into_inner());
        let _k = if self.kept { Some(crate::zone_alloc::kept()) } else { None };
        let last = upto.saturating_sub(1) >> AV_BITS;
        for ci in (self.len.load(Ordering::Relaxed) >> AV_BITS)..=last.min(self.chunks.len().saturating_sub(1)) {
            if self.chunks[ci].load(Ordering::Relaxed).is_null() {
                let mut v: Vec<std::mem::MaybeUninit<T>> = Vec::with_capacity(AV_CHUNK);
                unsafe { v.set_len(AV_CHUNK) };
                let c = Box::into_raw(v.into_boxed_slice()) as *mut T;
                fresh(c as *mut u8, AV_CHUNK * std::mem::size_of::<T>());
                self.chunks[ci].store(c, Ordering::Release);
            }
        }
    }

    /// Appends `v`, returns its index.
    pub fn push(&self, v: T) -> usize {
        use std::sync::atomic::Ordering;
        let _g = self.grow.lock().unwrap_or_else(|e| e.into_inner());
        let i = self.len.load(Ordering::Relaxed);
        let ci = i >> AV_BITS;
        assert!(ci < self.chunks.len(), "AppendVec full");
        let mut c = self.chunks[ci].load(Ordering::Relaxed);
        if c.is_null() {
            let _k = if self.kept { Some(crate::zone_alloc::kept()) } else { None };
            let mut v: Vec<std::mem::MaybeUninit<T>> = Vec::with_capacity(AV_CHUNK);
            unsafe { v.set_len(AV_CHUNK) };
            c = Box::into_raw(v.into_boxed_slice()) as *mut T;
            self.chunks[ci].store(c, Ordering::Release);
        }
        unsafe { c.add(i & (AV_CHUNK - 1)).write(v) };
        self.len.store(i + 1, Ordering::Release);
        i
    }

    #[inline]
    fn ptr(&self, i: usize) -> *mut T {
        assert!(i < self.len(), "AppendVec index {} out of bounds", i);
        let c = self.chunks[i >> AV_BITS].load(std::sync::atomic::Ordering::Acquire);
        unsafe { c.add(i & (AV_CHUNK - 1)) }
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> + '_ {
        let n = self.len();
        (0..n).map(move |i| unsafe { &*self.ptr(i) })
    }

    /// Every allocation the vector itself owns (the chunk table, each chunk): the kept-state
    /// scan (MAKEPAD_RUN_ZONE_CHECK) asks the compiler arena whether any of them lies in it.
    pub fn alloc_ptrs(&self, mut f: impl FnMut(*const u8)) {
        f(self.chunks.as_ptr() as *const u8);
        for c in self.chunks.iter() {
            let p = c.load(std::sync::atomic::Ordering::Acquire);
            if !p.is_null() {
                f(p as *const u8);
            }
        }
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> + '_ {
        let n = self.len();
        let me: &Self = self;
        (0..n).map(move |i| unsafe { &mut *me.ptr(i) })
    }
}

impl<T> std::ops::Index<usize> for AppendVec<T> {
    type Output = T;
    #[inline]
    fn index(&self, i: usize) -> &T {
        unsafe { &*self.ptr(i) }
    }
}

impl<T> std::ops::IndexMut<usize> for AppendVec<T> {
    #[inline]
    fn index_mut(&mut self, i: usize) -> &mut T {
        unsafe { &mut *self.ptr(i) }
    }
}

impl<T> Drop for AppendVec<T> {
    fn drop(&mut self) {
        let n = *self.len.get_mut();
        for (ci, c) in self.chunks.iter_mut().enumerate() {
            let p = *c.get_mut();
            if p.is_null() {
                continue;
            }
            let live = n.saturating_sub(ci * AV_CHUNK).min(AV_CHUNK);
            unsafe {
                for i in 0..live {
                    std::ptr::drop_in_place(p.add(i));
                }
                std::mem::drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(p as *mut std::mem::MaybeUninit<T>, AV_CHUNK)));
            }
        }
    }
}

#[cfg(test)]
mod settle_tests {
    use super::MemoMap;
    use std::sync::Arc;

    #[test]
    fn exclusive_retain_frees_removed_and_retired_values() {
        let map = MemoMap::with_bits(1); // exercise collisions and head/middle/tail removal
        let values: Vec<_> = (0..40).map(|_| Arc::new(vec![0u8; 100])).collect();
        for (i, v) in values.iter().enumerate() { map.insert(i, v.clone()); }
        unsafe { map.retain_exclusive(|k, _| k % 2 == 0); }
        for (i, v) in values.iter().enumerate() {
            assert_eq!(map.contains(&i), i % 2 == 0);
            assert_eq!(Arc::strong_count(v), if i % 2 == 0 { 2 } else { 1 });
        }
        map.clear();
        map.insert(41, values[1].clone());
        unsafe { map.retain_exclusive(|_, _| false); }
        assert!(values.iter().all(|v| Arc::strong_count(v) == 1));
    }

    #[test]
    fn detach_empties_the_map_and_frees_the_entries_when_dropped() {
        let map = MemoMap::with_bits(2);
        let values: Vec<_> = (0..30).map(|_| Arc::new(vec![0u8; 100])).collect();
        for (i, v) in values.iter().enumerate() { map.insert(i, v.clone()); }
        map.clear(); // a retired table too
        map.insert(99, values[0].clone());
        let gone = unsafe { map.detach() };
        assert!((0..30).all(|i| !map.contains(&i)) && !map.contains(&99));
        assert_eq!(Arc::strong_count(&values[0]), 3, "detached, not freed");
        map.insert(5, values[5].clone()); // the map works on
        assert!(map.contains(&5));
        drop(gone);
        assert_eq!(Arc::strong_count(&values[0]), 1);
        assert_eq!(Arc::strong_count(&values[5]), 2);
    }
}

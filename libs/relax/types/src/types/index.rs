//! The interners' hash index (types, lists, regions, solver inputs and results): open
//! addressing with linear probing over a power-of-two table of slots, 0 being empty. Lookups
//! read it without locks; claims and growth hold its lock. It doubles when a claim would make
//! it more than 3/4 full, so its size follows the number of entries: a small app's few 10^5
//! types take a table of a few MB, a check of a large app's dependencies (~10^7 types) a
//! proportionally larger one. A replaced table stays allocated until the interner is dropped or
//! mutably borrowed, since a lookup may still be probing it; every replaced table together is
//! smaller than the current one.
use relax_session::sync::{Mutex, MutexGuard};
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, Ordering};

/// A slot: a nonzero value names an entry (how is the interner's business).
pub(crate) trait Slot: Copy + Eq + 'static {
    type Atomic: Send + Sync;
    const EMPTY: Self;
    fn atomic(v: Self) -> Self::Atomic;
    fn load(a: &Self::Atomic) -> Self;
    fn store(a: &Self::Atomic, v: Self);
}
impl Slot for u32 {
    type Atomic = AtomicU32;
    const EMPTY: Self = 0;
    fn atomic(v: Self) -> AtomicU32 { AtomicU32::new(v) }
    // Acquire: a slot is stored (Release) after the entry it names is complete
    #[inline(always)]
    fn load(a: &AtomicU32) -> Self { a.load(Ordering::Acquire) }
    fn store(a: &AtomicU32, v: Self) { a.store(v, Ordering::Release) }
}
impl Slot for u64 {
    type Atomic = AtomicU64;
    const EMPTY: Self = 0;
    fn atomic(v: Self) -> AtomicU64 { AtomicU64::new(v) }
    #[inline(always)]
    fn load(a: &AtomicU64) -> Self { a.load(Ordering::Acquire) }
    fn store(a: &AtomicU64, v: Self) { a.store(v, Ordering::Release) }
}

/// The first table's size (log2).
const MIN_BITS: u32 = 10;

pub(crate) struct Table<S: Slot> {
    bits: u32,
    slots: Box<[S::Atomic]>,
}

impl<S: Slot> Table<S> {
    fn new(bits: u32) -> Box<Self> {
        Box::new(Table { bits, slots: (0..1usize << bits).map(|_| S::atomic(S::EMPTY)).collect() })
    }

    /// Where the probe for hash `h` starts: its high bits.
    #[inline(always)]
    fn home(&self, h: u32) -> usize {
        (h >> (32 - self.bits)) as usize
    }

    /// Stores `s` in the first empty slot from `h`'s home (the caller holds the lock).
    fn put(&self, h: u32, s: S) {
        let mask = self.slots.len() - 1;
        let mut i = self.home(h);
        while S::load(&self.slots[i]) != S::EMPTY {
            i = (i + 1) & mask;
        }
        S::store(&self.slots[i], s);
    }

    /// The slots as they are, `keep` deciding which stay (the others read as empty).
    pub(crate) fn snapshot(&self, keep: impl Fn(S) -> bool) -> Vec<S> {
        self.slots.iter().map(|a| S::load(a)).map(|s| if s != S::EMPTY && keep(s) { s } else { S::EMPTY }).collect()
    }

    /// The filled slots.
    pub(crate) fn filled(&self) -> impl Iterator<Item = S> + '_ {
        self.slots.iter().map(|a| S::load(a)).filter(|&s| s != S::EMPTY)
    }
}

/// What the lock guards: the entry count (the load) and the replaced tables.
pub(crate) struct Claims<S: Slot> {
    count: usize,
    retired: Vec<Box<Table<S>>>,
}

pub(crate) struct Index<S: Slot> {
    table: AtomicPtr<Table<S>>,
    lock: Mutex<Claims<S>>,
}

impl<S: Slot> Drop for Index<S> {
    fn drop(&mut self) {
        drop(unsafe { Box::from_raw(*self.table.get_mut()) });
    }
}

impl<S: Slot> Index<S> {
    pub(crate) fn new() -> Self {
        Self::of(Table::new(MIN_BITS), 0)
    }

    fn of(t: Box<Table<S>>, count: usize) -> Self {
        Index { table: AtomicPtr::new(Box::into_raw(t)), lock: Mutex::new(Claims { count, retired: Vec::new() }) }
    }

    /// The current table. It lives while `self` is shared (a replaced one is retired, not freed).
    #[inline(always)]
    pub(crate) fn table(&self) -> &Table<S> {
        // Acquire: a table is complete before its pointer is published
        unsafe { &*self.table.load(Ordering::Acquire) }
    }

    /// The first slot from `h`'s home that `hit` accepts, before an empty one.
    #[inline]
    pub(crate) fn find(&self, h: u32, mut hit: impl FnMut(S) -> bool) -> Option<S> {
        let t = self.table();
        let mask = t.slots.len() - 1;
        let mut i = t.home(h);
        loop {
            let s = S::load(unsafe { t.slots.get_unchecked(i) });
            if s == S::EMPTY {
                return None;
            }
            if hit(s) {
                return Some(s);
            }
            i = (i + 1) & mask;
        }
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Claims<S>> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Claims a slot for `s` (hash `h`), the caller holding the lock (`g`). When that would make
    /// the table more than 3/4 full, the table doubles instead: `all` yields every (hash, slot)
    /// the doubled table holds, `s` included.
    pub(crate) fn claim<I: Iterator<Item = (u32, S)>>(&self, g: &mut Claims<S>, h: u32, s: S, all: impl FnOnce() -> I) {
        g.count += 1;
        let t = self.table();
        if g.count <= (t.slots.len() >> 2) * 3 {
            t.put(h, s);
            return;
        }
        let new = Table::new(t.bits + 1);
        for (h, s) in all() {
            new.put(h, s);
        }
        let old = self.table.swap(Box::into_raw(new), Ordering::AcqRel);
        g.retired.push(unsafe { Box::from_raw(old) });
    }

    /// Entries claimed.
    pub(crate) fn count(&self) -> usize {
        self.lock().count
    }

    /// Bytes held: the current table and the replaced ones.
    pub(crate) fn bytes(&self) -> usize {
        let g = self.lock();
        let size = |t: &Table<S>| t.slots.len() * std::mem::size_of::<S>();
        size(self.table()) + g.retired.iter().map(|t| size(&**t)).sum::<usize>()
    }

    /// Frees the replaced tables (no lookup can be probing them: `self` is not shared).
    pub(crate) fn free_retired(&mut self) {
        self.lock.get_mut().unwrap_or_else(|e| e.into_inner()).retired.clear();
    }

    /// A table from an image's slots (`ok` checks each nonzero slot); None: not a table.
    pub(crate) fn restored(slots: &[S], ok: impl Fn(S) -> bool) -> Option<Self> {
        let bits = slots.len().trailing_zeros();
        if !slots.len().is_power_of_two() || bits < MIN_BITS || bits > 31 {
            return None;
        }
        let mut count = 0;
        for &s in slots {
            if s != S::EMPTY {
                if !ok(s) {
                    return None;
                }
                count += 1;
            }
        }
        Some(Self::of(Box::new(Table { bits, slots: slots.iter().map(|&s| S::atomic(s)).collect() }), count))
    }

    /// A copy of `live`'s current table (nothing claims while it is taken).
    pub(crate) fn copy_of(live: &Self) -> Self {
        let g = live.lock();
        let t = live.table();
        Self::of(Box::new(Table { bits: t.bits, slots: t.slots.iter().map(|a| S::atomic(S::load(a))).collect() }), g.count)
    }
}

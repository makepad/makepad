//! Open-addressing hash table storage shared by HashMap and HashSet.
//!
//! One control byte per bucket: EMPTY (0xff), DELETED (0x80, a tombstone) or FULL (the top 7
//! bits of the element's hash, 0x00..=0x7f). Buckets are a power of two; probing is linear
//! from `hash & mask`, comparing the control byte before touching the element, and stops at
//! the first EMPTY. Maximum load is 7/8 (all buckets for tables of < 8 buckets minus one).
//! A removal whose next bucket is EMPTY frees its bucket outright; otherwise it leaves a
//! tombstone, and tombstones are cleared by an in-place rebuild when they use up the growth
//! budget. The probe loops are non-generic (R8); only key comparison and hashing call back.

use crate::vec::Vec;
use crate::mem::MaybeUninit;

const EMPTY: u8 = 0xff;
const DELETED: u8 = 0x80;

#[inline]
fn h2(hash: u64) -> u8 {
    (hash >> 57) as u8
}

#[inline]
fn is_full(c: u8) -> bool {
    c & 0x80 == 0
}

/// Number of buckets for `cap` elements (0 for an unallocated table).
fn capacity_to_buckets(cap: usize) -> usize {
    if cap == 0 {
        return 0;
    }
    if cap < 4 {
        return 4;
    }
    if cap < 8 {
        return 8;
    }
    let adjusted = match cap.checked_mul(8) {
        Some(x) => x / 7,
        None => crate::panicking::panic_str("capacity overflow"),
    };
    adjusted.next_power_of_two()
}

/// Elements a table of `buckets` buckets holds before it must grow.
fn buckets_to_capacity(buckets: usize) -> usize {
    if buckets == 0 {
        0
    } else if buckets <= 8 {
        buckets - 1
    } else {
        buckets / 8 * 7
    }
}

/// The bucket holding an element for which `eq` returns true, if any.
fn probe_find(ctrl: &[u8], hash: u64, eq: &mut dyn FnMut(usize) -> bool) -> Option<usize> {
    let buckets = ctrl.len();
    if buckets == 0 {
        return None;
    }
    let mask = buckets - 1;
    let tag = h2(hash);
    let mut pos = (hash as usize) & mask;
    let mut probed = 0;
    while probed < buckets {
        let c = ctrl[pos];
        if c == EMPTY {
            return None;
        }
        if c == tag && eq(pos) {
            return Some(pos);
        }
        pos = (pos + 1) & mask;
        probed += 1;
    }
    None
}

/// The first EMPTY or DELETED bucket on `hash`'s probe sequence (the table has room).
fn probe_insert_slot(ctrl: &[u8], hash: u64) -> usize {
    let mask = ctrl.len() - 1;
    let mut pos = (hash as usize) & mask;
    loop {
        if !is_full(ctrl[pos]) {
            return pos;
        }
        pos = (pos + 1) & mask;
    }
}

/// Index of the next FULL bucket at or after `from`, or `ctrl.len()`.
pub(crate) fn next_full(ctrl: &[u8], from: usize) -> usize {
    let mut i = from;
    while i < ctrl.len() && !is_full(ctrl[i]) {
        i += 1;
    }
    i
}

pub(crate) struct RawTable<T> {
    ctrl: Vec<u8>,
    slots: Vec<MaybeUninit<T>>,
    items: usize,
    /// insertions possible before a resize/rebuild (EMPTY buckets beyond the load limit
    /// are not counted; tombstones consume it)
    growth_left: usize,
}

impl<T> RawTable<T> {
    pub(crate) const fn new() -> RawTable<T> {
        RawTable { ctrl: Vec::new(), slots: Vec::new(), items: 0, growth_left: 0 }
    }

    pub(crate) fn with_capacity(cap: usize) -> RawTable<T> {
        let mut t = RawTable::new();
        t.alloc_buckets(capacity_to_buckets(cap));
        t
    }

    fn alloc_buckets(&mut self, buckets: usize) {
        let mut ctrl = Vec::with_capacity(buckets);
        ctrl.resize(buckets, EMPTY);
        let mut slots: Vec<MaybeUninit<T>> = Vec::with_capacity(buckets);
        unsafe { slots.set_len(buckets) };
        self.ctrl = ctrl;
        self.slots = slots;
        self.items = 0;
        self.growth_left = buckets_to_capacity(buckets);
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.items
    }

    #[inline]
    pub(crate) fn buckets(&self) -> usize {
        self.ctrl.len()
    }

    pub(crate) fn capacity(&self) -> usize {
        self.items + self.growth_left
    }

    pub(crate) fn ctrl(&self) -> &[u8] {
        &self.ctrl
    }

    #[inline]
    pub(crate) fn is_full_at(&self, i: usize) -> bool {
        is_full(self.ctrl[i])
    }

    /// The element in a FULL bucket.
    #[inline]
    pub(crate) unsafe fn get(&self, i: usize) -> &T {
        self.slots[i].assume_init_ref()
    }

    #[inline]
    pub(crate) unsafe fn get_mut(&mut self, i: usize) -> &mut T {
        self.slots[i].assume_init_mut()
    }

    /// Raw pointer to bucket `i` (for iterators handing out disjoint `&mut`).
    #[inline]
    pub(crate) fn slot_ptr(&mut self, i: usize) -> *mut T {
        self.slots[i].as_mut_ptr()
    }

    pub(crate) fn find(&self, hash: u64, eq: &mut dyn FnMut(&T) -> bool) -> Option<usize> {
        let slots = &self.slots;
        probe_find(&self.ctrl, hash, &mut |i| eq(unsafe { slots[i].assume_init_ref() }))
    }

    /// Makes room for `additional` more elements; `hasher` rehashes moved elements.
    pub(crate) fn reserve(&mut self, additional: usize, hasher: &dyn Fn(&T) -> u64) {
        if additional <= self.growth_left {
            return;
        }
        let needed = match self.items.checked_add(additional) {
            Some(n) => n,
            None => crate::panicking::panic_str("capacity overflow"),
        };
        let full_cap = buckets_to_capacity(self.buckets());
        if needed <= full_cap / 2 {
            // mostly tombstones: rebuild at the same size
            self.resize_to(self.buckets(), hasher);
        } else {
            let want = if needed > full_cap + 1 { needed } else { full_cap + 1 };
            self.resize_to(capacity_to_buckets(want), hasher);
        }
    }

    /// Moves every element into a fresh table of `buckets` buckets.
    fn resize_to(&mut self, buckets: usize, hasher: &dyn Fn(&T) -> u64) {
        let old_ctrl = crate::mem::replace(&mut self.ctrl, Vec::new());
        let old_slots = crate::mem::replace(&mut self.slots, Vec::new());
        let items = self.items;
        self.alloc_buckets(buckets);
        let mut i = 0;
        while i < old_ctrl.len() {
            if is_full(old_ctrl[i]) {
                let v = unsafe { old_slots[i].assume_init_read() };
                let hash = hasher(&v);
                let slot = probe_insert_slot(&self.ctrl, hash);
                self.ctrl[slot] = h2(hash);
                self.slots[slot] = MaybeUninit::new(v);
            }
            i += 1;
        }
        self.items = items;
        self.growth_left -= items;
        // old_slots holds MaybeUninit: dropping it frees the buffer without dropping values
    }

    pub(crate) fn shrink_to(&mut self, min_capacity: usize, hasher: &dyn Fn(&T) -> u64) {
        let min = if min_capacity > self.items { min_capacity } else { self.items };
        let buckets = capacity_to_buckets(min);
        if buckets < self.buckets() {
            if buckets == 0 {
                self.ctrl = Vec::new();
                self.slots = Vec::new();
                self.growth_left = 0;
            } else {
                self.resize_to(buckets, hasher);
            }
        }
    }

    /// Bucket for a new element with `hash` (call `reserve(1, ..)` first). Returns the
    /// bucket; `insert_at` fills it.
    pub(crate) fn find_insert_slot(&self, hash: u64) -> usize {
        probe_insert_slot(&self.ctrl, hash)
    }

    /// Fills an EMPTY/DELETED bucket returned by `find_insert_slot`.
    pub(crate) fn insert_at(&mut self, slot: usize, hash: u64, value: T) -> usize {
        let old = self.ctrl[slot];
        if old == EMPTY {
            self.growth_left -= 1;
        }
        self.ctrl[slot] = h2(hash);
        self.slots[slot] = MaybeUninit::new(value);
        self.items += 1;
        slot
    }

    pub(crate) fn insert(&mut self, hash: u64, value: T, hasher: &dyn Fn(&T) -> u64) -> usize {
        self.reserve(1, hasher);
        let slot = probe_insert_slot(&self.ctrl, hash);
        self.insert_at(slot, hash, value)
    }

    /// Marks FULL bucket `i` free and returns its element.
    pub(crate) fn remove(&mut self, i: usize) -> T {
        let v = unsafe { self.slots[i].assume_init_read() };
        self.erase_ctrl(i);
        v
    }

    fn erase_ctrl(&mut self, i: usize) {
        let mask = self.buckets() - 1;
        // a probe chain through `i` would continue to i+1; if that is EMPTY, so can `i` be
        if self.ctrl[(i + 1) & mask] == EMPTY {
            self.ctrl[i] = EMPTY;
            self.growth_left += 1;
        } else {
            self.ctrl[i] = DELETED;
        }
        self.items -= 1;
    }

    /// Drops every element, keeps the buckets.
    pub(crate) fn clear(&mut self) {
        self.drop_elements();
        let mut i = 0;
        while i < self.ctrl.len() {
            self.ctrl[i] = EMPTY;
            i += 1;
        }
        self.items = 0;
        self.growth_left = buckets_to_capacity(self.buckets());
    }

    fn drop_elements(&mut self) {
        if crate::mem::needs_drop::<T>() && self.items != 0 {
            let mut i = 0;
            while i < self.ctrl.len() {
                if is_full(self.ctrl[i]) {
                    unsafe { self.slots[i].assume_init_drop() };
                }
                i += 1;
            }
        }
    }

    /// Keeps the elements for which `f` returns true.
    pub(crate) fn retain(&mut self, f: &mut dyn FnMut(&mut T) -> bool) {
        let mut i = 0;
        while i < self.ctrl.len() {
            if is_full(self.ctrl[i]) {
                let keep = f(unsafe { self.slots[i].assume_init_mut() });
                if !keep {
                    let v = self.remove(i);
                    drop(v);
                }
            }
            i += 1;
        }
    }

    /// Takes the element out of FULL bucket `i` without touching the counters' invariants
    /// beyond marking the bucket EMPTY (used by owning iterators, which finish with a reset).
    pub(crate) unsafe fn take_for_iter(&mut self, i: usize) -> T {
        self.ctrl[i] = EMPTY;
        self.items -= 1;
        self.slots[i].assume_init_read()
    }

    /// After an owning iteration emptied (or partly emptied) the table: drop the rest and
    /// reset every bucket to EMPTY.
    pub(crate) fn reset_after_drain(&mut self) {
        self.clear();
    }
}

impl<T: Clone> RawTable<T> {
    pub(crate) fn clone_table(&self) -> RawTable<T> {
        let mut t = RawTable::new();
        if self.buckets() == 0 {
            return t;
        }
        t.ctrl = self.ctrl.clone();
        let mut slots: Vec<MaybeUninit<T>> = Vec::with_capacity(self.buckets());
        unsafe { slots.set_len(self.buckets()) };
        let mut i = 0;
        while i < self.ctrl.len() {
            if is_full(self.ctrl[i]) {
                slots[i] = MaybeUninit::new(unsafe { self.slots[i].assume_init_ref() }.clone());
            }
            i += 1;
        }
        t.slots = slots;
        t.items = self.items;
        t.growth_left = self.growth_left;
        t
    }
}

impl<T> Drop for RawTable<T> {
    fn drop(&mut self) {
        self.drop_elements();
    }
}

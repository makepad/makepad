//! Shared HashMap/HashSet storage: port of hashbrown 0.14.5's raw table and 0.17.1's
//! control groups, as used by real std (MIT/Apache-2.0).
//!
//! One allocation holds buckets followed by controls and a trailing mirrored group.
//! Group matching stays non-generic and follows hashbrown per target: SSE2 (16 bytes) on
//! x86_64, NEON (8 bytes) on aarch64, else the generic SWAR group (8 bytes) -- under rustc and
//! under Relax alike (Relax lowers these core::arch intrinsics to vector ops). Triangular probe
//! loops and equality callbacks are monomorphized to inline key comparisons.

use crate::heap::{self, Layout};
use crate::marker::PhantomData;
use crate::{mem, ptr};

const EMPTY: u8 = 0xff;
const DELETED: u8 = 0x80;
// Group operations are non-generic. Only probing and user equality monomorphize.
// The generic SWAR group: targets without SSE2/NEON.
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
mod group {
    use super::*;
    const HIGH_BITS: u64 = 0x8080_8080_8080_8080;

    pub(super) struct BitMask(u64);

    impl BitMask {
        #[inline]
        pub(super) fn is_empty(&self) -> bool { self.0 == 0 }
        #[inline]
        pub(super) fn next(&mut self) -> Option<usize> {
            if self.0 == 0 { return None; }
            let bit = self.0.trailing_zeros() as usize / 8;
            self.0 &= self.0 - 1;
            Some(bit)
        }
        #[inline]
        pub(super) fn trailing_zeros(&self) -> usize { self.0.trailing_zeros() as usize / 8 }
        #[inline]
        pub(super) fn leading_zeros(&self) -> usize { self.0.leading_zeros() as usize / 8 }
    }

    pub(super) struct Group(u64);

    impl Group {
        #[inline]
        pub(super) unsafe fn store(&self, ctrl: *mut u8) { ptr::write(ctrl as *mut u64, self.0); }
        pub(super) const WIDTH: usize = 8;
        #[inline]
        pub(super) unsafe fn load(ctrl: *const u8) -> Group {
            Group(ptr::read_unaligned(ctrl as *const u64))
        }
        #[inline]
        pub(super) fn match_byte(&self, byte: u8) -> BitMask {
            let cmp = self.0 ^ (0x0101_0101_0101_0101 * byte as u64);
            // Borrow propagation can also match an adjacent tag differing in bit 0.
            // Equality filters these false positives, exactly as in hashbrown.
            BitMask((cmp.wrapping_sub(0x0101_0101_0101_0101) & !cmp & HIGH_BITS).to_le())
        }
        #[inline]
        pub(super) fn match_empty(&self) -> BitMask {
            BitMask((self.0 & (self.0 << 1) & HIGH_BITS).to_le())
        }
        #[inline]
        pub(super) fn match_empty_or_deleted(&self) -> BitMask {
            BitMask((self.0 & HIGH_BITS).to_le())
        }
        #[inline]
        pub(super) fn match_full(&self) -> BitMask {
            BitMask(self.match_empty_or_deleted().0 ^ HIGH_BITS)
        }
        #[inline]
        pub(super) fn prepare_rehash(&self) -> Group {
            let full = !self.0 & HIGH_BITS;
            Group(!full + (full >> 7))
        }
    }

}

#[cfg(target_arch = "x86_64")]
mod group {
    #[cfg(relax_check)]
    use core::arch::x86_64::*;
    #[cfg(not(relax_check))]
    use crate::arch::x86_64::*;
    pub(super) struct BitMask(u16);
    impl BitMask {
        #[inline]
        pub(super) fn is_empty(&self) -> bool { self.0 == 0 }
        #[inline]
        pub(super) fn next(&mut self) -> Option<usize> {
            if self.0 == 0 { return None; }
            let bit = self.0.trailing_zeros() as usize;
            self.0 &= self.0 - 1;
            Some(bit)
        }
        #[inline]
        pub(super) fn trailing_zeros(&self) -> usize { self.0.trailing_zeros() as usize }
        #[inline]
        pub(super) fn leading_zeros(&self) -> usize { self.0.leading_zeros() as usize }
    }
    pub(super) struct Group(__m128i);
    impl Group {
        pub(super) const WIDTH: usize = 16;
        #[inline]
        pub(super) unsafe fn load(ctrl: *const u8) -> Group {
            Group(_mm_loadu_si128(ctrl as *const __m128i))
        }
        #[inline]
        pub(super) unsafe fn store(&self, ctrl: *mut u8) {
            _mm_store_si128(ctrl as *mut __m128i, self.0);
        }
        #[inline]
        pub(super) fn match_byte(&self, byte: u8) -> BitMask {
            unsafe { BitMask(_mm_movemask_epi8(_mm_cmpeq_epi8(self.0, _mm_set1_epi8(byte as i8))) as u16) }
        }
        #[inline]
        pub(super) fn match_empty(&self) -> BitMask {
            self.match_byte(super::EMPTY)
        }
        #[inline]
        pub(super) fn match_empty_or_deleted(&self) -> BitMask {
            unsafe { BitMask(_mm_movemask_epi8(self.0) as u16) }
        }
        #[inline]
        pub(super) fn match_full(&self) -> BitMask {
            BitMask(!self.match_empty_or_deleted().0)
        }
        #[inline]
        pub(super) fn prepare_rehash(&self) -> Group {
            unsafe { Group(_mm_or_si128(_mm_cmpgt_epi8(_mm_setzero_si128(), self.0), _mm_set1_epi8(super::DELETED as i8))) }
        }
    }
}

#[cfg(target_arch = "aarch64")]
mod group {
    #[cfg(relax_check)]
    use core::arch::aarch64::*;
    #[cfg(not(relax_check))]
    use crate::arch::aarch64::*;
    pub(super) struct BitMask(u64);
    impl BitMask {
        #[inline]
        pub(super) fn is_empty(&self) -> bool { self.0 == 0 }
        #[inline]
        pub(super) fn next(&mut self) -> Option<usize> {
            // NEON comparisons set every bit of a matching byte; iterate only its MSB.
            self.0 &= 0x8080_8080_8080_8080;
            if self.0 == 0 { return None; }
            let bit = self.0.trailing_zeros() as usize / 8;
            self.0 &= self.0 - 1;
            Some(bit)
        }
        #[inline]
        pub(super) fn trailing_zeros(&self) -> usize { self.0.trailing_zeros() as usize / 8 }
        #[inline]
        pub(super) fn leading_zeros(&self) -> usize { self.0.leading_zeros() as usize / 8 }
    }
    pub(super) struct Group(uint8x8_t);
    impl Group {
        pub(super) const WIDTH: usize = 8;
        #[inline]
        pub(super) unsafe fn load(ctrl: *const u8) -> Group {
            Group(vld1_u8(ctrl))
        }
        #[inline]
        pub(super) unsafe fn store(&self, ctrl: *mut u8) {
            vst1_u8(ctrl, self.0);
        }
        #[inline]
        pub(super) fn match_byte(&self, byte: u8) -> BitMask {
            // rustc_resolve/src/lib.rs:2643 @8d1a764: same-crate calls use const syntax.
            unsafe { BitMask(vget_lane_u64::<0>(vreinterpret_u64_u8(vceq_u8(self.0, vdup_n_u8(byte))))) }
        }
        #[inline]
        pub(super) fn match_empty(&self) -> BitMask {
            self.match_byte(super::EMPTY)
        }
        #[inline]
        pub(super) fn match_empty_or_deleted(&self) -> BitMask {
            unsafe { BitMask(vget_lane_u64::<0>(vreinterpret_u64_u8(vcltz_s8(vreinterpret_s8_u8(self.0))))) }
        }
        #[inline]
        pub(super) fn match_full(&self) -> BitMask {
            unsafe { BitMask(vget_lane_u64::<0>(vreinterpret_u64_u8(vcgez_s8(vreinterpret_s8_u8(self.0))))) }
        }
        #[inline]
        pub(super) fn prepare_rehash(&self) -> Group {
            unsafe { Group(vorr_u8(vcltz_s8(vreinterpret_s8_u8(self.0)), vdup_n_u8(super::DELETED))) }
        }
    }
}

use self::group::Group;

struct ProbeSeq { pos: usize, stride: usize }
impl ProbeSeq {
    #[inline]
    fn new(hash: u64, mask: usize) -> ProbeSeq {
        ProbeSeq { pos: (hash as usize) & mask, stride: 0 }
    }
    #[inline]
    fn move_next(&mut self, mask: usize) {
        self.stride += Group::WIDTH;
        self.pos = (self.pos + self.stride) & mask;
    }
}

#[inline]
fn h2(hash: u64) -> u8 {
    let bytes = mem::size_of::<usize>();
    let bytes = if bytes < 8 { bytes } else { 8 };
    ((hash >> (bytes * 8 - 7)) & 0x7f) as u8
}
#[inline]
fn is_full(c: u8) -> bool { c & 0x80 == 0 }

fn capacity_to_buckets(cap: usize) -> usize {
    if cap == 0 { return 0; }
    if cap < 8 { return if cap < 4 { 4 } else { 8 }; }
    let adjusted = match cap.checked_mul(8) {
        Some(x) => x / 7,
        None => crate::panicking::panic_str("capacity overflow"),
    };
    adjusted.next_power_of_two()
}

fn bucket_mask_to_capacity(mask: usize) -> usize {
    if mask < 8 { mask } else { (mask + 1) / 8 * 7 }
}

// The control pointer is also the end of the bucket array. Buckets run backwards
// from it, so one pointer suffices for both regions, including over-aligned/ZST T.
fn allocation_layout(size: usize, align: usize, buckets: usize) -> (Layout, usize) {
    let align = if align > Group::WIDTH { align } else { Group::WIDTH };
    let offset = match size.checked_mul(buckets).and_then(|n| n.checked_add(align - 1)) {
        Some(n) => n & !(align - 1),
        None => crate::panicking::panic_str("capacity overflow"),
    };
    let len = match offset.checked_add(buckets).and_then(|n| n.checked_add(Group::WIDTH)) {
        Some(n) => n,
        None => crate::panicking::panic_str("capacity overflow"),
    };
    let layout = match Layout::from_size_align(len, align) {
        Ok(l) => l,
        Err(_) => crate::panicking::panic_str("capacity overflow"),
    };
    (layout, offset)
}

fn probe_find<F: FnMut(usize) -> bool>(ctrl: *const u8, buckets: usize, hash: u64, mut eq: F) -> Option<usize> {
    if buckets == 0 { return None; }
    let mask = buckets - 1;
    let mut probe = ProbeSeq::new(hash, mask);
    let tag = h2(hash);
    loop {
        let group = unsafe { Group::load(ctrl.add(probe.pos)) };
        let mut matches = group.match_byte(tag);
        while let Some(bit) = matches.next() {
            let i = (probe.pos + bit) & mask;
            if eq(i) { return Some(i); }
        }
        if !group.match_empty().is_empty() { return None; }
        probe.move_next(mask);
    }
}

fn probe_insert_slot(ctrl: *const u8, mask: usize, hash: u64) -> usize {
    let mut probe = ProbeSeq::new(hash, mask);
    loop {
        let group = unsafe { Group::load(ctrl.add(probe.pos)) };
        if let Some(bit) = group.match_empty_or_deleted().next() {
            let i = (probe.pos + bit) & mask;
            // Small tables have EMPTY padding between the real and mirrored bytes.
            // A match on padding can wrap onto a full slot: search from zero then.
            if unsafe { is_full(*ctrl.add(i)) } {
                return unsafe { Group::load(ctrl) }.match_empty_or_deleted().trailing_zeros();
            }
            return i;
        }
        probe.move_next(mask);
    }
}

/// Next full bucket, scanning complete groups without reading past the slice.
pub(crate) fn next_full(ctrl: &[u8], from: usize) -> usize {
    let mut i = from;
    while i + Group::WIDTH <= ctrl.len() {
        let mut full = unsafe { Group::load(ctrl.as_ptr().add(i)) }.match_full();
        if let Some(bit) = full.next() { return i + bit; }
        i += Group::WIDTH;
    }
    while i < ctrl.len() && !is_full(ctrl[i]) { i += 1; }
    i
}

pub(crate) struct RawTable<T> {
    ctrl: *mut u8,
    bucket_count: usize,
    items: usize,
    growth_left: usize,
    marker: PhantomData<T>,
}

unsafe impl<T: Send> Send for RawTable<T> {}
unsafe impl<T: Sync> Sync for RawTable<T> {}

impl<T> RawTable<T> {
    pub(crate) const fn new() -> RawTable<T> {
        RawTable { ctrl: ptr::null_mut(), bucket_count: 0, items: 0, growth_left: 0, marker: PhantomData }
    }
    pub(crate) fn with_capacity(cap: usize) -> RawTable<T> {
        let mut t = RawTable::new();
        t.alloc_buckets(capacity_to_buckets(cap));
        t
    }
    fn alloc_buckets(&mut self, buckets: usize) {
        if buckets == 0 { return; }
        let (layout, offset) = allocation_layout(mem::size_of::<T>(), mem::align_of::<T>(), buckets);
        let allocation = heap::alloc_or_abort(layout);
        self.ctrl = unsafe { allocation.add(offset) };
        self.bucket_count = buckets;
        self.growth_left = bucket_mask_to_capacity(buckets - 1);
        unsafe { ptr::write_bytes(self.ctrl, EMPTY, buckets + Group::WIDTH); }
    }
    #[inline]
    fn set_ctrl(&mut self, i: usize, c: u8) {
        let mirror = (i.wrapping_sub(Group::WIDTH) & (self.bucket_count - 1)) + Group::WIDTH;
        unsafe { *self.ctrl.add(i) = c; *self.ctrl.add(mirror) = c; }
    }
    #[inline]
    pub(crate) fn len(&self) -> usize { self.items }
    #[inline]
    pub(crate) fn buckets(&self) -> usize { self.bucket_count }
    pub(crate) fn capacity(&self) -> usize { self.items + self.growth_left }
    pub(crate) fn ctrl(&self) -> &[u8] {
        if self.bucket_count == 0 { return &[]; }
        unsafe { crate::slice::from_raw_parts(self.ctrl, self.bucket_count) }
    }
    #[inline]
    pub(crate) fn is_full_at(&self, i: usize) -> bool { unsafe { is_full(*self.ctrl.add(i)) } }
    #[inline]
    fn bucket_ptr(&self, i: usize) -> *mut T {
        unsafe { (self.ctrl as *mut T).sub(i + 1) }
    }
    #[inline]
    pub(crate) unsafe fn get(&self, i: usize) -> &T { &*self.bucket_ptr(i) }
    #[inline]
    pub(crate) unsafe fn get_mut(&mut self, i: usize) -> &mut T { &mut *self.bucket_ptr(i) }
    #[inline]
    pub(crate) fn slot_ptr(&mut self, i: usize) -> *mut T { self.bucket_ptr(i) }
    pub(crate) fn find<F: FnMut(&T) -> bool>(&self, hash: u64, mut eq: F) -> Option<usize> {
        probe_find(self.ctrl, self.bucket_count, hash, |i| eq(unsafe { self.get(i) }))
    }

    #[relax(keep_reachable)]
    pub(crate) fn reserve(&mut self, additional: usize, hasher: &dyn Fn(&T) -> u64) {
        if additional <= self.growth_left { return; }
        let needed = match self.items.checked_add(additional) {
            Some(n) => n,
            None => crate::panicking::panic_str("capacity overflow"),
        };
        let full_cap = if self.bucket_count == 0 { 0 } else { bucket_mask_to_capacity(self.bucket_count - 1) };
        if needed <= full_cap / 2 {
            self.rehash_in_place(hasher);
        } else {
            let want = if needed > full_cap + 1 { needed } else { full_cap + 1 };
            self.resize_to(capacity_to_buckets(want), hasher);
        }
    }

    #[relax(keep_reachable)]
    fn resize_to(&mut self, buckets: usize, hasher: &dyn Fn(&T) -> u64) {
        let mut new = RawTable::<T>::new();
        new.alloc_buckets(buckets);
        let mut i = 0;
        while i < self.bucket_count {
            if self.is_full_at(i) {
                let hash = hasher(unsafe { self.get(i) });
                let slot = new.find_insert_slot(hash);
                // Keep ownership in self until every hash succeeds. new.items == 0
                // makes new's unwinding destructor free only the allocation.
                unsafe { ptr::copy_nonoverlapping(self.bucket_ptr(i), new.bucket_ptr(slot), 1); }
                new.set_ctrl(slot, h2(hash));
            }
            i += 1;
        }
        new.items = self.items;
        new.growth_left -= self.items;
        self.items = 0;
        mem::swap(self, &mut new);
    }

    #[relax(keep_reachable)]
    fn rehash_in_place(&mut self, hasher: &dyn Fn(&T) -> u64) {
        let mut i = 0;
        while i < self.bucket_count {
            unsafe {
                let group = Group::load(self.ctrl.add(i)).prepare_rehash();
                group.store(self.ctrl.add(i));
            }
            i += Group::WIDTH;
        }
        // For small tables retain EMPTY padding; their converted real bytes are
        // mirrored starting at WIDTH, as in set_ctrl.
        let mirror = if self.bucket_count < Group::WIDTH { Group::WIDTH } else { self.bucket_count };
        let count = if self.bucket_count < Group::WIDTH { self.bucket_count } else { Group::WIDTH };
        unsafe { ptr::copy_nonoverlapping(self.ctrl, self.ctrl.add(mirror), count); }
        let guard = RehashGuard { table: self };
        let mask = guard.table.bucket_count - 1;
        i = 0;
        while i < guard.table.bucket_count {
            while unsafe { *guard.table.ctrl.add(i) == DELETED } {
                let hash = hasher(unsafe { guard.table.get(i) });
                let new_i = guard.table.find_insert_slot(hash);
                let start = (hash as usize) & mask;
                if ((i.wrapping_sub(start) & mask) / Group::WIDTH) == ((new_i.wrapping_sub(start) & mask) / Group::WIDTH) {
                    guard.table.set_ctrl(i, h2(hash));
                    break;
                }
                let old = unsafe { *guard.table.ctrl.add(new_i) };
                guard.table.set_ctrl(new_i, h2(hash));
                if old == EMPTY {
                    unsafe { ptr::copy_nonoverlapping(guard.table.bucket_ptr(i), guard.table.bucket_ptr(new_i), 1); }
                    guard.table.set_ctrl(i, EMPTY);
                    break;
                }
                unsafe { ptr::swap(guard.table.bucket_ptr(i), guard.table.bucket_ptr(new_i)); }
            }
            i += 1;
        }
        guard.table.growth_left = bucket_mask_to_capacity(mask) - guard.table.items;
        mem::forget(guard);
    }

    pub(crate) fn shrink_to(&mut self, min_capacity: usize, hasher: &dyn Fn(&T) -> u64) {
        let min = if min_capacity > self.items { min_capacity } else { self.items };
        let buckets = capacity_to_buckets(min);
        if buckets < self.bucket_count { self.resize_to(buckets, hasher); }
    }
    pub(crate) fn find_insert_slot(&self, hash: u64) -> usize {
        probe_insert_slot(self.ctrl, self.bucket_count - 1, hash)
    }
    pub(crate) fn insert_at(&mut self, slot: usize, hash: u64, value: T) -> usize {
        if unsafe { *self.ctrl.add(slot) == EMPTY } { self.growth_left -= 1; }
        unsafe { ptr::write(self.bucket_ptr(slot), value); }
        self.set_ctrl(slot, h2(hash));
        self.items += 1;
        slot
    }
    pub(crate) fn insert(&mut self, hash: u64, value: T, hasher: &dyn Fn(&T) -> u64) -> usize {
        self.reserve(1, hasher);
        let slot = self.find_insert_slot(hash);
        self.insert_at(slot, hash, value)
    }
    pub(crate) fn remove(&mut self, i: usize) -> T {
        self.erase_ctrl(i);
        unsafe { ptr::read(self.bucket_ptr(i)) }
    }
    fn erase_ctrl(&mut self, i: usize) {
        let before = i.wrapping_sub(Group::WIDTH) & (self.bucket_count - 1);
        let empty_before = unsafe { Group::load(self.ctrl.add(before)) }.match_empty();
        let empty_after = unsafe { Group::load(self.ctrl.add(i)) }.match_empty();
        let c = if empty_before.leading_zeros() + empty_after.trailing_zeros() >= Group::WIDTH {
            DELETED
        } else {
            self.growth_left += 1;
            EMPTY
        };
        self.set_ctrl(i, c);
        self.items -= 1;
    }
    pub(crate) fn clear(&mut self) {
        self.drop_elements();
        if self.bucket_count != 0 {
            unsafe { ptr::write_bytes(self.ctrl, EMPTY, self.bucket_count + Group::WIDTH); }
            self.growth_left = bucket_mask_to_capacity(self.bucket_count - 1);
        }
        self.items = 0;
    }
    fn drop_elements(&mut self) {
        if mem::needs_drop::<T>() && self.items != 0 {
            let mut i = 0;
            while i < self.bucket_count {
                if self.is_full_at(i) {
                    // Remove ownership before running user code.
                    self.erase_ctrl(i);
                    unsafe { ptr::drop_in_place(self.bucket_ptr(i)); }
                }
                i += 1;
            }
        }
    }
    pub(crate) fn retain<F: FnMut(&mut T) -> bool>(&mut self, mut f: F) {
        let mut i = 0;
        while i < self.bucket_count {
            if self.is_full_at(i) && !f(unsafe { self.get_mut(i) }) { drop(self.remove(i)); }
            i += 1;
        }
    }
    pub(crate) unsafe fn take_for_iter(&mut self, i: usize) -> T {
        self.set_ctrl(i, EMPTY);
        self.items -= 1;
        ptr::read(self.bucket_ptr(i))
    }
    pub(crate) fn reset_after_drain(&mut self) { self.clear(); }
}

// After a hashing panic, DELETED controls still own unprocessed values. Remove
// those values and retain the already reinserted portion of the table.
struct RehashGuard<'a, T> { table: &'a mut RawTable<T> }
impl<T> Drop for RehashGuard<'_, T> {
    fn drop(&mut self) {
        let mut i = 0;
        while i < self.table.bucket_count {
            if unsafe { *self.table.ctrl.add(i) == DELETED } {
                self.table.set_ctrl(i, EMPTY);
                self.table.items -= 1;
                unsafe { ptr::drop_in_place(self.table.bucket_ptr(i)); }
            }
            i += 1;
        }
        self.table.growth_left = bucket_mask_to_capacity(self.table.bucket_count - 1) - self.table.items;
    }
}

impl<T: Clone> RawTable<T> {
    pub(crate) fn clone_table(&self) -> RawTable<T> {
        let mut t = RawTable::new();
        t.alloc_buckets(self.bucket_count);
        let mut i = 0;
        while i < self.bucket_count {
            if self.is_full_at(i) {
                let value = unsafe { self.get(i) }.clone();
                unsafe { ptr::write(t.bucket_ptr(i), value); }
                t.items += 1;
            }
            t.set_ctrl(i, unsafe { *self.ctrl.add(i) });
            i += 1;
        }
        t.growth_left = self.growth_left;
        t
    }
}
unsafe impl<#[may_dangle] T> Drop for RawTable<T> {
    fn drop(&mut self) {
        self.drop_elements();
        if self.bucket_count != 0 {
            let (layout, offset) = allocation_layout(mem::size_of::<T>(), mem::align_of::<T>(), self.bucket_count);
            unsafe { heap::dealloc(self.ctrl.sub(offset), layout); }
        }
    }
}

// Threading support primitives for the multithreaded solver port.
//
// The C engine shares mutable world data across worker threads and guarantees
// disjoint access structurally (graph coloring, per-worker contexts, atomic
// block claiming). Rust cannot express "disjoint by construction" in the type
// system, so these wrappers carry the invariants as documented unsafe
// contracts. They are the only aliasing escape hatches the threaded code may
// use; every use site must be justifiable by one of the invariants below.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicI32, Ordering};

/// Send+Sync raw pointer wrapper for sharing a struct with worker tasks.
///
/// # Safety contract
/// The caller guarantees that for the lifetime of the sharing:
/// - the pointee outlives every task that can dereference the pointer
///   (parallel work must be joined before the pointee is dropped), and
/// - either only one thread dereferences it mutably at a time, or all
///   concurrent access is confined to fields that are themselves
///   synchronized (atomics).
#[derive(Clone, Copy)]
pub struct SyncPtr<T>(pub *mut T);

unsafe impl<T> Send for SyncPtr<T> {}
unsafe impl<T> Sync for SyncPtr<T> {}

impl<T> SyncPtr<T> {
    #[inline]
    pub fn new(reference: &mut T) -> SyncPtr<T> {
        SyncPtr(reference as *mut T)
    }

    /// # Safety
    /// See the struct-level contract: the pointee must be alive and the
    /// caller must guarantee no aliased mutable access.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get(&self) -> &mut T {
        &mut *self.0
    }
}

/// A shared view of a mutable slice that hands out disjoint `&mut` elements
/// across threads.
///
/// This is how the parallel solver stages materialize per-element mutable
/// access (body states by graph color, contacts by parallel-for block)
/// without holding an aliasing `&mut World` on more than one thread.
///
/// # Safety contract
/// For the lifetime `'a` of the view the caller guarantees:
/// - no two threads call `get_mut` with the same index concurrently
///   (index-disjointness — in the engine this comes from graph coloring and
///   from atomic block claiming handing each index to exactly one worker),
/// - the original `&mut [T]` is not used for anything else while the view
///   exists (the constructor takes it by `&mut` borrow, so the borrow checker
///   enforces this within one thread).
pub struct SyncSlice<'a, T> {
    slice: &'a [UnsafeCell<T>],
}

unsafe impl<'a, T: Send> Send for SyncSlice<'a, T> {}
unsafe impl<'a, T: Send> Sync for SyncSlice<'a, T> {}

impl<'a, T> SyncSlice<'a, T> {
    #[inline]
    pub fn new(slice: &'a mut [T]) -> SyncSlice<'a, T> {
        // SAFETY: UnsafeCell<T> has the same layout as T; the exclusive
        // borrow is re-exposed only through the documented disjoint-index
        // contract of get_mut.
        let cells = unsafe { &*(slice as *mut [T] as *const [UnsafeCell<T>]) };
        SyncSlice { slice: cells }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.slice.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.slice.is_empty()
    }

    /// # Safety
    /// See the struct-level contract: `i` must not be accessed by any other
    /// thread for as long as the returned reference lives.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get_mut(&self, i: usize) -> &mut T {
        &mut *self.slice[i].get()
    }

    /// Read-only access under the same disjointness contract as `get_mut`
    /// (no other thread may mutate `i` while the reference lives).
    ///
    /// # Safety
    /// See the struct-level contract.
    #[inline]
    pub unsafe fn get_ref(&self, i: usize) -> &T {
        &*self.slice[i].get()
    }
}

/// Port of the C b3AtomicInt. Every C shim (b3AtomicLoadInt/StoreInt/
/// FetchAddInt/CompareExchangeInt, platform.h) uses __ATOMIC_SEQ_CST, so this
/// wrapper is SeqCst throughout to match conservatively.
#[derive(Debug, Default)]
pub struct AtomicIndex {
    value: AtomicI32,
}

impl AtomicIndex {
    #[inline]
    pub fn new(value: i32) -> AtomicIndex {
        AtomicIndex { value: AtomicI32::new(value) }
    }

    /// C: b3AtomicLoadInt
    #[inline]
    pub fn load(&self) -> i32 {
        self.value.load(Ordering::SeqCst)
    }

    /// C: b3AtomicStoreInt
    #[inline]
    pub fn store(&self, value: i32) {
        self.value.store(value, Ordering::SeqCst)
    }

    /// C: b3AtomicFetchAddInt — returns the previous value.
    #[inline]
    pub fn fetch_add(&self, increment: i32) -> i32 {
        self.value.fetch_add(increment, Ordering::SeqCst)
    }

    /// C: b3AtomicCompareExchangeInt — returns true when the exchange happened.
    #[inline]
    pub fn compare_exchange(&self, expected: i32, desired: i32) -> bool {
        self.value
            .compare_exchange(expected, desired, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
}

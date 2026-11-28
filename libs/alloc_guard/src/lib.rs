//! A process-wide memory reserve for evaluations. On the first refusal, the allocator releases
//! the reserve so the request can retry and the evaluation can unwind and report the error.
//! Each refusal advances [`low_memory_epoch`], even when the reserve has already been spent.
//!
//! A binary opts in by wrapping its existing global allocator.
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOC: makepad_alloc_guard::Guarded<std::alloc::System> =
//!     makepad_alloc_guard::Guarded(std::alloc::System);
//! ```
//!
//! A successful allocation, zeroed allocation or reallocation costs one null test, and the
//! layout is kept across the inner call for the retry (a stack frame and one or two stores),
//! so the inner call is no longer a tail call. Deallocation goes straight through. Without
//! the guard installed, [`arm`] still holds the reserve, but it stays allocated for the life
//! of the process and the epoch never advances.
//!
//! Never install this guard in a process that hosts CEF. CEF replaces the process allocator
//! when it loads (see `makepad-alloc-epoch`), and the guard has not been proven beside that swap.

use std::alloc::{alloc, dealloc, GlobalAlloc, Layout};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

/// The 16 MiB reserve covers unguarded allocations between VM samples, unwinding and reporting
/// the diagnostic, and the app's allocations while an evaluation is suspended. The fault sweep
/// is to verify that allocations after release use less than a quarter of this reserve.
pub const BALLAST_BYTES: usize = 16 << 20;

const BALLAST_LAYOUT: Layout = match Layout::from_size_align(BALLAST_BYTES, 16) {
    Ok(layout) => layout,
    Err(_) => panic!("invalid ballast layout"),
};
static BALLAST: AtomicPtr<u8> = AtomicPtr::new(null_mut());
static EPOCH: AtomicUsize = AtomicUsize::new(0);

/// Whether the process currently holds a reserve.
pub fn armed() -> bool {
    !BALLAST.load(Ordering::Relaxed).is_null()
}

/// Hold a reserve for the next refusal. Its pages are left untouched.
pub fn arm() -> bool {
    if armed() {
        return true;
    }
    // SAFETY: the layout is nonempty. Either publish ownership or free this block here.
    let block = unsafe { alloc(BALLAST_LAYOUT) };
    if block.is_null() {
        return false;
    }
    if BALLAST.compare_exchange(null_mut(), block, Ordering::Release, Ordering::Relaxed).is_err() {
        unsafe { dealloc(block, BALLAST_LAYOUT) };
    }
    true
}

/// Advances once per refused request, including requests whose retry succeeds.
pub fn low_memory_epoch() -> usize {
    EPOCH.load(Ordering::Relaxed)
}

/// The epoch's own counter, for a reader that cannot call this crate but can load an
/// atomic by address, such as a script VM the loader provides.
pub fn epoch_counter() -> &'static AtomicUsize {
    &EPOCH
}

#[cold]
#[inline(never)]
fn release_on_refusal() {
    let block = BALLAST.swap(null_mut(), Ordering::Acquire);
    if !block.is_null() {
        // SAFETY: the swap took sole ownership. arm used the process global, even if the
        // Guarded instance that saw this refusal is a standalone allocator.
        unsafe { dealloc(block, BALLAST_LAYOUT) };
    }
    EPOCH.fetch_add(1, Ordering::Relaxed);
}

/// The allocator `A`, with one retry after releasing the process reserve.
pub struct Guarded<A>(pub A);

#[inline(always)]
fn retry_on_refusal(mut allocate: impl FnMut() -> *mut u8) -> *mut u8 {
    let block = allocate();
    if block.is_null() {
        release_on_refusal();
        return allocate();
    }
    block
}

// SAFETY: every operation forwards the caller's layout and pointer to the same allocator.
// A null realloc leaves the original block valid for the retry.
unsafe impl<A: GlobalAlloc> GlobalAlloc for Guarded<A> {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        retry_on_refusal(|| unsafe { self.0.alloc(layout) })
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        retry_on_refusal(|| unsafe { self.0.alloc_zeroed(layout) })
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        retry_on_refusal(|| unsafe { self.0.realloc(ptr, layout, new_size) })
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { self.0.dealloc(ptr, layout) }
    }
}

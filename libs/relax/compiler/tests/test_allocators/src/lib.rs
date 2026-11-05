//! `makepad test ta-app` (from this folder) passes when every test target is its own binary with its
//! own `#[global_allocator]`, as under `cargo test`: two targets that each define one are no conflict
//! ("cannot define multiple global allocators" is per crate), and each target's allocations go to its
//! own allocator, never to the other target's -- also when the targets run out of one test program
//! (makepad test, run.rs --subject). tests/plain_alloc.rs defines none: it allocates through neither.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The allocations of tests/first.rs's, tests/second.rs's and tests/alloc.rs's allocators.
pub static FIRST: AtomicUsize = AtomicUsize::new(0);
pub static SECOND: AtomicUsize = AtomicUsize::new(0);
pub static NAMED_ALLOC: AtomicUsize = AtomicUsize::new(0);

pub struct Counting(pub &'static AtomicUsize);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        self.0.fetch_add(1, Ordering::Relaxed);
        System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l)
    }
}

/// Allocates once and returns how far `counter` moved meanwhile.
pub fn counted_allocation(counter: &AtomicUsize) -> usize {
    let before = counter.load(Ordering::Relaxed);
    let b = std::hint::black_box(Box::new([7u8; 64]));
    assert_eq!(b[63], 7);
    counter.load(Ordering::Relaxed) - before
}

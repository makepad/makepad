//! `makepad test ta-app` (from this folder) passes when every test target is its own binary with its
//! own `#[global_allocator]`, as under `cargo test`: two targets that each define one are no conflict
//! ("cannot define multiple global allocators" is per crate), and each target's allocations go to its
//! own allocator, never to the other target's.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

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

//! Counts Rust allocations on the calling test thread.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
thread_local! {
    static COUNT: Cell<Option<usize>> = const { Cell::new(None) };
}
struct CountAllocator;
fn note() {
    COUNT.with(|c| {
        if let Some(n) = c.get() {
            c.set(Some(n + 1));
        }
    });
}
unsafe impl GlobalAlloc for CountAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        System.alloc(layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        System.alloc_zeroed(layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        note();
        System.realloc(ptr, layout, size)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}
#[global_allocator]
static ALLOCATOR: CountAllocator = CountAllocator;

pub(crate) fn measured(f: impl FnOnce()) -> usize {
    COUNT.with(|c| c.set(Some(0)));
    f();
    COUNT.with(|c| c.replace(None).unwrap())
}

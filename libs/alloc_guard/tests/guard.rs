use makepad_alloc_guard::{arm, armed, low_memory_epoch, Guarded, BALLAST_BYTES};
use std::alloc::{alloc, alloc_zeroed, dealloc, realloc, GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::ptr::null_mut;

struct Gate;

thread_local! {
    static REFUSE: Cell<u32> = const { Cell::new(0) };
    static BALLAST_ALLOCS: Cell<usize> = const { Cell::new(0) };
    static BALLAST_FREES: Cell<usize> = const { Cell::new(0) };
}

fn refused() -> bool {
    REFUSE.try_with(|n| {
        let left = n.get();
        n.set(left.saturating_sub(1));
        left != 0
    }).unwrap_or(false)
}

// SAFETY: a refusal leaves any old block intact. Successful operations and frees use System.
unsafe impl GlobalAlloc for Gate {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if refused() { return null_mut(); }
        let block = unsafe { System.alloc(layout) };
        if !block.is_null() && layout.size() == BALLAST_BYTES {
            BALLAST_ALLOCS.set(BALLAST_ALLOCS.get() + 1);
        }
        block
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if refused() { null_mut() } else { unsafe { System.alloc_zeroed(layout) } }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if refused() { null_mut() } else { unsafe { System.realloc(ptr, layout, size) } }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if layout.size() == BALLAST_BYTES {
            BALLAST_FREES.set(BALLAST_FREES.get() + 1);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: Guarded<Gate> = Guarded(Gate);

#[test]
fn a_refused_allocation_releases_the_reserve_and_retries_once() {
    let before = BALLAST_ALLOCS.get();
    let frees = BALLAST_FREES.get();
    assert!(arm() && armed(), "arming must hold a reserve");
    assert_eq!(BALLAST_ALLOCS.get(), before + 1, "the reserve must come from the process allocator");
    assert!(arm() && armed(), "an armed reserve must remain available");
    assert_eq!(BALLAST_ALLOCS.get(), before + 1, "arming twice must keep the original block");
    assert_eq!(BALLAST_FREES.get(), frees, "arming twice must leave the original block live");

    let epoch = low_memory_epoch();
    REFUSE.set(1);
    let mut bytes = Vec::<u8>::with_capacity(black_box(1024));
    bytes.resize(1024, 37);
    assert_eq!(black_box(&bytes)[1023], 37, "the retried vector allocation must be writable");
    assert!(!armed(), "the refused allocation must release the ballast");
    assert_eq!(BALLAST_FREES.get(), frees + 1, "the process allocator must actually free the reserve");
    assert_eq!(low_memory_epoch(), epoch.wrapping_add(1), "a successful retry must report the refusal once");

    let layout = Layout::from_size_align(32, 16).unwrap();
    let allocate = black_box(alloc as unsafe fn(Layout) -> *mut u8);
    let epoch = low_memory_epoch();
    REFUSE.set(2);
    let block = unsafe { allocate(layout) };
    assert!(block.is_null(), "a second refusal must return null to the caller");
    assert_eq!(REFUSE.get(), 0, "the allocator must retry once without a reserve");
    assert_eq!(low_memory_epoch(), epoch.wrapping_add(1), "two refusals of one request must advance the epoch once");

    let epoch = low_memory_epoch();
    REFUSE.set(1);
    let block = unsafe { alloc_zeroed(layout) };
    assert!(!block.is_null(), "a refused zeroed allocation must retry");
    // SAFETY: the successful zeroed allocation initialized layout.size() bytes.
    unsafe {
        assert!(std::slice::from_raw_parts(block, layout.size()).iter().all(|&b| b == 0),
            "the retry must preserve zero-initialization");
        block.write_bytes(0xa5, layout.size());
    }
    assert_eq!(low_memory_epoch(), epoch.wrapping_add(1), "the zeroed request must observe its refusal");

    let grown = Layout::from_size_align(2 * layout.size(), layout.align()).unwrap();
    let epoch = low_memory_epoch();
    REFUSE.set(1);
    let block = unsafe { realloc(block, layout, grown.size()) };
    assert!(!block.is_null(), "a refused reallocation must retry");
    // SAFETY: realloc preserves the initialized prefix and transfers ownership to this pointer.
    unsafe {
        assert!(std::slice::from_raw_parts(block, layout.size()).iter().all(|&b| b == 0xa5),
            "a retried reallocation must preserve the old bytes");
        dealloc(block, grown);
    }
    assert_eq!(low_memory_epoch(), epoch.wrapping_add(1), "the reallocation must observe its refusal");
    assert!(arm() && armed(), "the reserve must be available to arm again after a release");
}

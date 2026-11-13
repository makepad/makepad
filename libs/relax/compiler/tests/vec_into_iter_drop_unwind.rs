// vec::IntoIter's drop dropped the items left one at a time and then freed the buffer: a panicking
// item drop left the later items undropped and the buffer unfreed (relax issue 162). alloc drops the
// items left as one slice (the others still drop after a panic) and frees the buffer from a guard.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};

struct WatchAlloc;
static WATCHED: AtomicUsize = AtomicUsize::new(0);
static FREED: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static ALLOC: WatchAlloc = WatchAlloc;

unsafe impl GlobalAlloc for WatchAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if WATCHED.compare_exchange(ptr as usize, 0, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            FREED.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}

thread_local!(static DROPS: Cell<u32> = const { Cell::new(0) });

/// An item whose drop panics when its id is the bomb's.
struct Bomb {
    id: u32,
    bomb: u32,
}

impl Drop for Bomb {
    fn drop(&mut self) {
        DROPS.with(|d| d.set(d.get() + 1));
        if self.id == self.bomb {
            panic!("bomb {}", self.id);
        }
    }
}

/// Five items, `taken` of them taken by next() first; then the iterator's drop, in which item
/// `bomb`'s drop panics: (items dropped in all, the buffer freed).
fn drop_with_a_panic(bomb: u32, taken: usize) -> (u32, bool) {
    DROPS.with(|d| d.set(0));
    let v: Vec<Bomb> = (0..5).map(|id| Bomb { id, bomb }).collect();
    let buf = v.as_ptr() as usize;
    let mut it = v.into_iter();
    for _ in 0..taken {
        drop(it.next());
    }
    let freed = FREED.load(Ordering::SeqCst);
    WATCHED.store(buf, Ordering::SeqCst);
    assert!(catch_unwind(AssertUnwindSafe(|| drop(it))).is_err(), "the bomb's drop panics");
    WATCHED.store(0, Ordering::SeqCst);
    (DROPS.with(|d| d.get()), FREED.load(Ordering::SeqCst) > freed)
}

#[test]
fn the_items_left_and_the_buffer_are_dropped_when_one_drop_panics() {
    assert_eq!(drop_with_a_panic(0, 0), (5, true), "the first item's drop panics: the other four drop and the buffer is freed");
    assert_eq!(drop_with_a_panic(3, 2), (5, true), "after two next()s a later item's drop panics: the rest drop and the buffer is freed");
}

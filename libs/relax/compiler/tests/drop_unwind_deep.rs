// Shared recursive successors used to be walked again below every cycle cut.
// Keep Drop on the payload: Drop on each node would stop drop_may_unwind at the root.
// rustc_mir_transform/src/shim.rs: build_drop_shim; elaborate_drop.rs: open_drop_for_adt
// drops owned fields on both the normal and unwind paths.
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
        // Clear before freeing: another test may immediately reuse the address.
        if WATCHED.compare_exchange(ptr as usize, 0, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            FREED.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}

macro_rules! node {
    ($name:ident, $next:ident, $other:ident) => {
        enum $name<T> {
            End,
            Link {
                _next: Box<$next<T>>,
                _other: Vec<$other<T>>,
                _back: Option<Box<N00<T>>>,
                _value: T,
            },
        }
        impl<T> $name<T> {
            fn chain<F: FnMut() -> T>(depth: usize, make: &mut F) -> Self {
                if depth == 0 { return Self::End; }
                Self::Link {
                    _next: Box::new($next::chain(depth - 1, make)),
                    _other: Vec::new(),
                    _back: None,
                    _value: make(),
                }
            }
        }
    };
}

node!(N00, N01, N02);
node!(N01, N02, N03);
node!(N02, N03, N04);
node!(N03, N04, N05);
node!(N04, N05, N06);
node!(N05, N06, N07);
node!(N06, N07, N08);
node!(N07, N08, N09);
node!(N08, N09, N10);
node!(N09, N10, N11);
node!(N10, N11, N12);
node!(N11, N12, N13);
node!(N12, N13, N14);
node!(N13, N14, N15);
node!(N14, N15, N16);
node!(N15, N16, N17);
node!(N16, N17, N18);
node!(N17, N18, N19);
node!(N18, N19, N20);
node!(N19, N20, N21);
node!(N20, N21, N22);
node!(N21, N22, N23);
node!(N22, N23, N24);
node!(N23, N24, N25);
node!(N24, N25, N26);
node!(N25, N26, N27);
node!(N26, N27, N28);
node!(N27, N28, N29);
node!(N28, N29, N00);
node!(N29, N00, N01);

struct Count<'a>(&'a Cell<usize>, bool);
impl Drop for Count<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
        if self.1 { panic!("recursive drop"); }
    }
}

#[test]
fn struct_field_panic_drops_the_rest() {
    struct Pair<'a> { _first: Count<'a>, _second: Count<'a> }
    let drops = Cell::new(0);
    let result = catch_unwind(AssertUnwindSafe(|| {
        drop(Pair { _first: Count(&drops, true), _second: Count(&drops, false) });
    }));
    assert!(result.is_err());
    assert_eq!(drops.get(), 2);
}

#[test]
fn enum_field_panic_drops_the_rest() {
    enum Pair<'a> { Empty, Fields { _first: Count<'a>, _second: Count<'a> } }
    let drops = Cell::new(0);
    drop(Pair::Empty);
    let result = catch_unwind(AssertUnwindSafe(|| {
        drop(Pair::Fields { _first: Count(&drops, true), _second: Count(&drops, false) });
    }));
    assert!(result.is_err());
    assert_eq!(drops.get(), 2);
}

#[test]
fn enum_destructor_panic_drops_the_active_variant() {
    enum Value<'a> { Empty, Fields { _first: Count<'a>, _second: Count<'a> } }
    impl Drop for Value<'_> {
        fn drop(&mut self) { panic!("enum drop"); }
    }
    let drops = Cell::new(0);
    let empty = catch_unwind(AssertUnwindSafe(|| drop(Value::Empty)));
    assert!(empty.is_err());
    let result = catch_unwind(AssertUnwindSafe(|| {
        drop(Value::Fields { _first: Count(&drops, false), _second: Count(&drops, false) });
    }));
    assert!(result.is_err());
    assert_eq!(drops.get(), 2);
}

#[test]
fn slice_element_panic_drops_the_rest_and_the_caller_guard() {
    let drops = Cell::new(0);
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _guard = Count(&drops, false);
        let mut values = std::mem::ManuallyDrop::new([
            Count(&drops, true), Count(&drops, false), Count(&drops, false),
        ]);
        let slice: &mut [Count<'_>] = &mut *values;
        unsafe { std::ptr::drop_in_place(slice); }
    }));
    assert!(result.is_err());
    assert_eq!(drops.get(), 4);
}

#[test]
fn vec_element_panic_drops_the_rest() {
    for panic_at in 0..3 {
        let drops = Cell::new(0);
        let result = catch_unwind(AssertUnwindSafe(|| {
            drop(vec![Count(&drops, panic_at == 0), Count(&drops, panic_at == 1), Count(&drops, panic_at == 2)]);
        }));
        assert!(result.is_err());
        assert_eq!(drops.get(), 3);
    }
}

#[test]
fn panicking_heap_drops_free_the_allocation() {
    // rustc_mir_transform/src/elaborate_drop.rs: open_drop_for_adt frees Box on unwind;
    // library/alloc/src/vec/mod.rs: Vec::drop drops the slice, then RawVec frees the buffer.
    let drops = Cell::new(0);
    let boxed = Box::new(Count(&drops, true));
    WATCHED.store(&*boxed as *const Count<'_> as usize, Ordering::SeqCst);
    let result = catch_unwind(AssertUnwindSafe(|| drop(boxed)));
    assert!(result.is_err());
    assert_eq!(drops.get(), 1);
    assert_eq!(FREED.load(Ordering::SeqCst), 1);

    let values = vec![Count(&drops, true), Count(&drops, false), Count(&drops, false)];
    WATCHED.store(values.as_ptr() as usize, Ordering::SeqCst);
    let result = catch_unwind(AssertUnwindSafe(|| drop(values)));
    assert!(result.is_err());
    assert_eq!(drops.get(), 4);
    assert_eq!(FREED.load(Ordering::SeqCst), 2);

    let slice = vec![Count(&drops, true), Count(&drops, false), Count(&drops, false)].into_boxed_slice();
    WATCHED.store(slice.as_ptr() as usize, Ordering::SeqCst);
    let result = catch_unwind(AssertUnwindSafe(|| drop(slice)));
    assert!(result.is_err());
    assert_eq!(drops.get(), 7);
    assert_eq!(FREED.load(Ordering::SeqCst), 3);
}

#[test]
fn recursive_without_unwinding() {
    // An entire SCC with no unwinding payload still needs Box/Vec drop glue.
    let mut make = || ();
    drop(N00::chain(30, &mut make));
    drop(N15::chain(30, &mut make));
    println!("| recursive no unwind");
}

#[test]
fn recursive_drop_counts_and_unwinds() {
    let drops = Cell::new(0);
    let mut make = || Count(&drops, false);
    drop(N00::chain(30, &mut make));
    assert_eq!(drops.get(), 30);
    drop(N00::Link {
        _next: Box::new(N01::chain(3, &mut make)),
        _other: vec![N02::chain(3, &mut make)],
        _back: Some(Box::new(N00::chain(3, &mut make))),
        _value: make(),
    });
    assert_eq!(drops.get(), 40);

    // Query another member after the root. Its provisional false must not escape
    // into the cache: a panic in the deepest payload also drops every ancestor.
    let mut first = true;
    let mut make = || {
        let panic = first;
        first = false;
        Count(&drops, panic)
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _guard = Count(&drops, false);
        drop(N15::chain(30, &mut make));
    }));
    assert!(result.is_err());
    assert_eq!(drops.get(), 71);
    println!("| recursive drops {}", drops.get());
}

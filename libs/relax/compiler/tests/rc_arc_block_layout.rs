//! Rc and Arc ask the allocator for rustc's RcInner / ArcInner layout, the two counts and then the
//! value at its alignment, padded to the block's alignment, and free the block with that same
//! layout, also when a Weak frees it after the value is gone (relax issue 204: Relax asked for two
//! words more).
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fmt::Debug;
use std::rc::Rc;
use std::sync::Arc;

thread_local! {
    /// [allocations, last size, last align, deallocations, last size, last align] on this thread
    static SEEN: Cell<[usize; 6]> = const { Cell::new([0; 6]) };
}

fn note(at: usize, layout: Layout) {
    let _ = SEEN.try_with(|s| {
        let mut v = s.get();
        v[at] += 1;
        v[at + 1] = layout.size();
        v[at + 2] = layout.align();
        s.set(v);
    });
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(0, layout);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        note(3, layout);
        System.dealloc(p, layout)
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// rustc's block for a value: the two counts, then the value, padded to the alignment.
fn block(value: Layout) -> (usize, usize) {
    let l = Layout::new::<[usize; 2]>().extend(value).unwrap().0.pad_to_align();
    (l.size(), l.align())
}

/// Runs `f`, which allocates one block and frees it: (the layout asked for, the layout freed).
fn one(what: &str, f: impl FnOnce()) -> ((usize, usize), (usize, usize)) {
    let a = SEEN.with(Cell::get);
    f();
    let b = SEEN.with(Cell::get);
    assert_eq!((b[0] - a[0], b[3] - a[3]), (1, 1), "{what}: one block, freed");
    ((b[1], b[2]), (b[4], b[5]))
}

#[repr(align(32))]
struct Wide([u8; 40]);

#[test]
fn rc_and_arc_blocks_have_rustcs_layout() {
    // (an array: building the cases allocates nothing)
    let cases: [(&str, Layout, ((usize, usize), (usize, usize))); 8] = [
        ("Rc<[u8; 512]>", Layout::new::<[u8; 512]>(), one("rc", || drop(Rc::new(std::hint::black_box([7u8; 512]))))),
        ("Arc<[u8; 512]>", Layout::new::<[u8; 512]>(), one("arc", || drop(Arc::new(std::hint::black_box([7u8; 512]))))),
        ("Rc<u8> (padded)", Layout::new::<u8>(), one("rc u8", || drop(Rc::new(std::hint::black_box(1u8))))),
        ("Arc<Wide> (align 32)", Layout::new::<Wide>(), one("arc wide", || assert_eq!(Arc::new(std::hint::black_box(Wide([1; 40]))).0[39], 1))),
        ("Rc<str>", Layout::array::<u8>(3).unwrap(), one("rc str", || drop(std::hint::black_box(Rc::<str>::from("abc"))))),
        ("Arc<[u64]>", Layout::array::<u64>(3).unwrap(), one("arc slice", || drop(std::hint::black_box(Arc::<[u64]>::from(&[1u64, 2, 3][..]))))),
        ("Rc<dyn Debug>, freed by its Weak", Layout::new::<[u16; 5]>(), one("rc dyn", || {
            let r: Rc<dyn Debug> = Rc::new(std::hint::black_box([7u16; 5]));
            let w = Rc::downgrade(&r);
            assert!(w.upgrade().is_some());
            drop(r);
            assert!(w.upgrade().is_none());
            drop(w);
        })),
        ("Arc<[u32]>, freed by its Weak", Layout::array::<u32>(2).unwrap(), one("arc weak", || {
            let a: Arc<[u32]> = Arc::new(std::hint::black_box([5u32, 6]));
            let w = Arc::downgrade(&a);
            drop(a);
            assert!(w.upgrade().is_none());
            drop(w);
        })),
    ];
    for (what, value, (asked, freed)) in cases {
        assert_eq!(asked, block(value), "{what}: the layout asked for");
        assert_eq!(freed, asked, "{what}: the layout freed");
    }
    assert_eq!(block(Layout::new::<[u8; 512]>()).0, 512 + 2 * std::mem::size_of::<usize>(), "two counts before the value");
}

use makepad_stack::{usage, with_frame, Frame, StackSlice};
use std::cell::Cell;
use std::rc::Rc;

fn squares<'f>(f: &Frame<'f>, n: usize) -> StackSlice<'f, u64> {
    let mut v = f.vec();
    for i in 0..n {
        v.push((i * i) as u64);
    }
    v.into_slice()
}

#[test]
fn vec_on_the_stack_and_released() {
    let before = usage();
    with_frame(|f| {
        let mut v = f.vec::<u32>();
        for i in 0..10_000 {
            v.push(i);
        }
        assert!(!v.spilled());
        assert_eq!(v.iter().map(|&x| x as u64).sum::<u64>(), 49_995_000);
        assert!(usage().0 >= 40_000);
    });
    assert_eq!(usage(), before);
}

#[test]
fn returning_moves_result_into_caller_frame() {
    with_frame(|f| {
        let top0 = usage().0;
        let s = f.returning(|g| {
            // temporaries above the result
            let mut junk = g.vec::<u8>();
            junk.extend_from_slice(&[1u8; 4096]);
            let r = squares(g, 100);
            drop(junk);
            r
        });
        assert!(!s.spilled());
        assert_eq!(s.len(), 100);
        assert_eq!(s[99], 99 * 99);
        // only the result stays: the 4 KB of temporaries are released
        assert!(usage().0 - top0 <= 100 * 8 + 8);
        let t = squares(f, 3);
        assert_eq!(&*t, &[0, 1, 4]);
        assert_eq!(s[10], 100);
    });
}

#[test]
fn outer_frame_allocates_on_heap_while_inner_open() {
    with_frame(|outer| {
        let mut v = outer.vec::<u64>();
        v.push(1);
        outer.frame(|inner| {
            let w = inner.vec_with_capacity::<u64>(4);
            assert!(!w.spilled());
            // growing the outer vector now must not land in the inner frame
            for i in 0..1000 {
                v.push(i);
            }
            assert!(v.spilled());
        });
        assert_eq!(v.len(), 1001);
        assert_eq!(v[1000], 999);
    });
}

#[test]
fn destructors_run_once() {
    struct D(Rc<Cell<u32>>);
    impl Drop for D {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let n = Rc::new(Cell::new(0));
    with_frame(|f| {
        let mut v = f.vec();
        for _ in 0..100 {
            v.push(D(n.clone()));
        }
        v.truncate(60);
        assert_eq!(n.get(), 40);
        let s = f.returning(|g| {
            let mut w = g.vec();
            for _ in 0..10 {
                w.push(D(n.clone()));
            }
            w.into_slice()
        });
        assert_eq!(n.get(), 40);
        drop(s);
        assert_eq!(n.get(), 50);
        let b = f.alloc(D(n.clone()));
        drop(b);
        assert_eq!(n.get(), 51);
    });
    assert_eq!(n.get(), 111);
}

#[test]
fn unwinding_closes_the_frame() {
    let before = usage();
    let r = std::panic::catch_unwind(|| {
        with_frame(|f| {
            let mut v = f.vec::<u8>();
            v.extend_from_slice(&[0; 1000]);
            panic!("boom");
        })
    });
    assert!(r.is_err());
    assert_eq!(usage(), before);
}

#[test]
fn big_values_grow_the_region() {
    with_frame(|f| {
        let mut v = f.vec::<u8>();
        v.extend(std::iter::repeat(7u8).take(256 << 20));
        assert_eq!(v.len(), 256 << 20);
        assert_eq!(v[(256 << 20) - 1], 7);
    });
    // and the next use starts small again
    with_frame(|f| {
        let s = f.slice_copy(&[1, 2, 3]);
        assert_eq!(&*s, &[1, 2, 3]);
    });
}

#[test]
fn strings_and_zero_sized() {
    with_frame(|f| {
        let s = f.format(format_args!("{}-{}", 12, "ab"));
        assert_eq!(&*s, "12-ab");
        let mut z = f.vec::<()>();
        for _ in 0..1000 {
            z.push(());
        }
        assert_eq!(z.len(), 1000);
        let e = f.returning(|g| g.vec::<u32>().into_slice());
        assert!(e.is_empty());
    });
}

#[test]
fn threads_have_their_own_region() {
    with_frame(|f| {
        let s = squares(f, 50);
        std::thread::scope(|sc| {
            sc.spawn(|| {
                assert_eq!(s[7], 49);
                with_frame(|g| assert_eq!(squares(g, 5)[4], 16));
            });
        });
    });
}

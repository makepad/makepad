//! BinaryHeap, Rc and Arc vs real std.
mod coll_common;
use coll_common::*;
use rapid_std_check::arc::{Arc, Weak as AWeak};
use rapid_std_check::collections::BinaryHeap;
use rapid_std_check::rc::{Rc, Weak};
use std::cell::Cell;
use std::collections::BinaryHeap as SBinaryHeap;
use std::rc::Rc as StdRc;

/// Compares by key only, so equal keys with different tags expose the heap's tie order.
#[derive(Clone, Copy, Debug)]
struct Tagged(u32, u32);
impl PartialEq for Tagged {
    fn eq(&self, o: &Tagged) -> bool {
        self.0 == o.0
    }
}
impl Eq for Tagged {}
impl PartialOrd for Tagged {
    fn partial_cmp(&self, o: &Tagged) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Tagged {
    fn cmp(&self, o: &Tagged) -> std::cmp::Ordering {
        self.0.cmp(&o.0)
    }
}

fn hv<T: Clone>(v: &[T]) -> rapid_std_check::vec::Vec<T> {
    let mut o = rapid_std_check::vec::Vec::new();
    for x in v {
        o.push(x.clone());
    }
    o
}

fn tags(v: &[Tagged]) -> Vec<(u32, u32)> {
    v.iter().map(|t| (t.0, t.1)).collect()
}

#[test]
fn heap_random() {
    let mut rng = Rng(42);
    let mut a: BinaryHeap<Tagged> = BinaryHeap::new();
    let mut b: SBinaryHeap<Tagged> = SBinaryHeap::new();
    for i in 0..30_000u32 {
        match rng.below(6) {
            0 | 1 | 2 => {
                let t = Tagged(rng.below(50) as u32, i);
                a.push(t);
                b.push(t);
            }
            3 | 4 => {
                let (x, y) = (a.pop(), b.pop());
                assert_eq!(x.map(|t| (t.0, t.1)), y.map(|t| (t.0, t.1)));
            }
            _ => {
                if let (Some(mut pa), Some(mut pb)) = (a.peek_mut(), b.peek_mut()) {
                    let n = rng.below(50) as u32;
                    pa.0 = n;
                    pb.0 = n;
                }
            }
        }
        assert_eq!(a.peek().map(|t| (t.0, t.1)), b.peek().map(|t| (t.0, t.1)));
        assert_eq!(a.len(), b.len());
        if i % 5000 == 4999 {
            assert_eq!(tags(a.as_slice()), tags(b.as_slice()));
            let m = rng.below(3) as u32;
            a.retain(|t| t.1 % 3 != m);
            b.retain(|t| t.1 % 3 != m);
            assert_eq!(tags(a.as_slice()), tags(b.as_slice()));
            let extra: Vec<Tagged> = (0..rng.below(400) as u32).map(|j| Tagged(j % 17, 100_000 + j)).collect();
            let mut ea = BinaryHeap::from(hv(&extra));
            let mut eb = SBinaryHeap::from(extra);
            a.append(&mut ea);
            b.append(&mut eb);
            assert_eq!(tags(a.as_slice()), tags(b.as_slice()));
        }
    }
    let sa = a.clone().into_sorted_vec();
    let sb = b.clone().into_sorted_vec();
    assert_eq!(tags(&sa), tags(&sb));
    let va = a.into_vec();
    let vb = b.into_vec();
    assert_eq!(tags(&va), tags(&vb));
    let fa: BinaryHeap<u32> = (0..100u32).map(|i| (i * 37) % 101).collect();
    let fb: SBinaryHeap<u32> = (0..100u32).map(|i| (i * 37) % 101).collect();
    assert_eq!(fa.as_slice(), fb.as_slice());
    assert_eq!(hdebug(&BinaryHeap::from(hv(&[1u32, 5, 3])), false), format!("{:?}", SBinaryHeap::from(vec![1u32, 5, 3])));
}

#[test]
fn rc_basics() {
    let drops = StdRc::new(Cell::new(0usize));
    {
        let a = Rc::new(Counted { v: 5, drops: drops.clone() });
        assert_eq!(Rc::strong_count(&a), 1);
        let b = a.clone();
        assert_eq!(Rc::strong_count(&a), 2);
        let w = Rc::downgrade(&a);
        assert_eq!(Rc::weak_count(&a), 1);
        assert_eq!(w.strong_count(), 2);
        assert!(Rc::ptr_eq(&a, &b));
        drop(a);
        assert_eq!(w.upgrade().map(|r| r.v), Some(5));
        drop(b);
        assert!(w.upgrade().is_none());
        assert_eq!(w.strong_count(), 0);
        assert_eq!(w.weak_count(), 0);
        assert_eq!(drops.get(), 1);
    }
    let mut x = Rc::new(10u32);
    *Rc::get_mut(&mut x).unwrap() += 1;
    let y = x.clone();
    assert!(Rc::get_mut(&mut x).is_none());
    *Rc::make_mut(&mut x) += 1;
    assert_eq!((*x, *y), (12, 11));
    let w = Rc::downgrade(&x);
    *Rc::make_mut(&mut x) += 1;
    assert!(w.upgrade().is_none());
    assert_eq!(*x, 13);
    assert_eq!(Rc::try_unwrap(x).ok(), Some(13));
    let z = Rc::new(String::from("s"));
    let z2 = z.clone();
    assert!(Rc::try_unwrap(z).is_err());
    assert_eq!(Rc::into_inner(z2).as_deref(), Some("s"));
    let s: Rc<str> = Rc::from("héllo");
    assert_eq!(&*s, "héllo");
    let s2 = s.clone();
    assert_eq!(Rc::strong_count(&s2), 2);
    let sl: Rc<[u64]> = Rc::from(hv(&[1u64, 2, 3]));
    assert_eq!(&*sl, &[1, 2, 3]);
    let sl2: Rc<[String]> = Rc::from(&[String::from("a"), String::from("b")][..]);
    assert_eq!(sl2.len(), 2);
    let empty: Weak<u32> = Weak::new();
    assert!(empty.upgrade().is_none());
    let raw = Rc::into_raw(Rc::new(77u64));
    let back = unsafe { Rc::from_raw(raw) };
    assert_eq!(*back, 77);
    struct Node {
        me: Weak<Node>,
        v: u8,
    }
    let n = Rc::new_cyclic(|w| Node { me: w.clone(), v: 3 });
    assert_eq!(n.me.upgrade().unwrap().v, 3);
    assert_eq!(hdebug(&Rc::new(5u32), false), "5");
    // over-aligned values
    #[repr(align(32))]
    struct Big(u8);
    let b = Rc::new(Big(9));
    assert_eq!((&*b as *const Big as usize) % 32, 0);
    assert_eq!(b.0, 9);
}

#[test]
fn arc_threads() {
    let a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut hs = Vec::new();
    for _ in 0..8 {
        let a = a.clone();
        hs.push(std::thread::spawn(move || {
            for _ in 0..10_000 {
                let c = a.clone();
                c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let w = Arc::downgrade(&c);
                assert!(w.upgrade().is_some());
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
    assert_eq!(a.load(std::sync::atomic::Ordering::Relaxed), 80_000);
    assert_eq!(Arc::strong_count(&a), 1);
    assert_eq!(Arc::weak_count(&a), 0);
    let drops = StdRc::new(Cell::new(0usize));
    {
        let x = Arc::new(Counted { v: 1, drops: drops.clone() });
        let w: AWeak<Counted> = Arc::downgrade(&x);
        let y = x.clone();
        drop(x);
        assert_eq!(w.upgrade().map(|r| r.v), Some(1));
        drop(y);
        assert!(w.upgrade().is_none());
    }
    assert_eq!(drops.get(), 1);
    let mut m = Arc::new(vec![1u8]);
    Arc::make_mut(&mut m).push(2);
    let m2 = m.clone();
    Arc::make_mut(&mut m).push(3);
    assert_eq!((&m[..], &m2[..]), (&[1u8, 2, 3][..], &[1u8, 2][..]));
    let s: Arc<str> = Arc::from(rapid_std_check::string::String::from("abc"));
    assert_eq!(&*s, "abc");
    let v: Arc<[u32]> = (0..4u32).collect();
    assert_eq!(&*v, &[0, 1, 2, 3]);
    assert_eq!(Arc::try_unwrap(Arc::new(4u8)).ok(), Some(4));
    assert_eq!(Arc::into_inner(Arc::new(5u8)), Some(5));
}

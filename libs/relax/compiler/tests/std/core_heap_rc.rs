// BinaryHeap tie ordering and Rc/Arc ownership, mutation and threaded reference counts.
// Differential file: `| ` lines equal under rustc --test and makepad test.

use std::cell::Cell;
use std::collections::BinaryHeap;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Weak as AWeak};

struct D(u64, u32);
impl D {
    fn new() -> D {
        D(0xcbf29ce484222325, 0)
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= x as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
    fn add<T: std::fmt::Debug + ?Sized>(&mut self, value: &T) {
        use std::fmt::Write;
        write!(self, "{:?};", value).unwrap();
        self.1 += 1;
    }
    fn line(&self, tag: &str) {
        println!("| {} {} {:016x}", tag, self.1, self.0);
    }
}
impl std::fmt::Write for D {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.bytes(s.as_bytes());
        Ok(())
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

struct Counted {
    v: u32,
    drops: Rc<Cell<usize>>,
}
impl Drop for Counted {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

#[derive(Clone, Copy, Debug)]
struct Tagged(u32, u32);
impl PartialEq for Tagged {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for Tagged {}
impl PartialOrd for Tagged {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Tagged {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}
fn tags(values: &[Tagged]) -> Vec<(u32, u32)> {
    values.iter().map(|t| (t.0, t.1)).collect()
}

#[test]
fn a01_heap_random() {
    let mut rng = Rng(42);
    let mut d = D::new();
    let mut a: BinaryHeap<Tagged> = BinaryHeap::new();
    for i in 0..3_000u32 {
        match rng.below(6) {
            0..=2 => a.push(Tagged(rng.below(50) as u32, i)),
            3 | 4 => d.add(&a.pop().map(|t| (t.0, t.1))),
            _ => {
                if let Some(mut top) = a.peek_mut() {
                    top.0 = rng.below(50) as u32;
                }
            }
        }
        d.add(&a.peek().map(|t| (t.0, t.1)));
        d.add(&a.len());
        if i % 500 == 499 {
            d.add(&tags(a.as_slice()));
            let m = rng.below(3) as u32;
            a.retain(|t| t.1 % 3 != m);
            d.add(&tags(a.as_slice()));
            let extra: Vec<_> = (0..rng.below(400) as u32)
                .map(|j| Tagged(j % 17, 100_000 + j))
                .collect();
            let mut extra = BinaryHeap::from(extra);
            a.append(&mut extra);
            d.add(&tags(a.as_slice()));
        }
    }
    d.add(&tags(&a.clone().into_sorted_vec()));
    d.add(&tags(&a.into_vec()));
    let collected: BinaryHeap<u32> = (0..100u32).map(|i| (i * 37) % 101).collect();
    d.add(collected.as_slice());
    d.add(&format!("{:?}", BinaryHeap::from(vec![1u32, 5, 3])));
    d.line("heap random");
}

#[test]
fn a02_rc_basics() {
    let drops = Rc::new(Cell::new(0usize));
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
    let sl: Rc<[u64]> = Rc::from(vec![1u64, 2, 3]);
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
    assert_eq!(format!("{:?}", Rc::new(5u32)), "5");
    // over-aligned values
    #[repr(align(32))]
    struct Big(u8);
    let b = Rc::new(Big(9));
    assert_eq!((&*b as *const Big as usize) % 32, 0);
    assert_eq!(b.0, 9);
    println!(
        "| rc basics drops {} str {:?} slice {:?} cyclic {} aligned {}",
        drops.get(),
        &*s2,
        &*sl,
        n.me.upgrade().unwrap().v,
        b.0,
    );
}

#[test]
fn a03_arc_threads() {
    let a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut hs = Vec::new();
    for _ in 0..8 {
        let a = a.clone();
        hs.push(std::thread::spawn(move || {
            for _ in 0..1_000 {
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
    assert_eq!(a.load(std::sync::atomic::Ordering::Relaxed), 8_000);
    assert_eq!(Arc::strong_count(&a), 1);
    assert_eq!(Arc::weak_count(&a), 0);
    let drops = Rc::new(Cell::new(0usize));
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
    let s: Arc<str> = Arc::from(String::from("abc"));
    assert_eq!(&*s, "abc");
    let v: Arc<[u32]> = (0..4u32).collect();
    assert_eq!(&*v, &[0, 1, 2, 3]);
    assert_eq!(Arc::try_unwrap(Arc::new(4u8)).ok(), Some(4));
    assert_eq!(Arc::into_inner(Arc::new(5u8)), Some(5));
    println!(
        "| arc threads {} strong {} weak {} drops {} slices {:?} {:?} str {:?}",
        a.load(std::sync::atomic::Ordering::Relaxed),
        Arc::strong_count(&a),
        Arc::weak_count(&a),
        drops.get(),
        &m[..],
        &m2[..],
        &*s,
    );
}

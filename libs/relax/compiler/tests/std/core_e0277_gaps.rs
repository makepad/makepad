// Impls real std has that the frontend's E0277 check found missing: Rc/Arc::make_mut on unsized
// values (CloneToUninit), From<Ipv6Addr> for u128 (and the other Ip conversions), From<Box<T>> for
// Rc<T> / Arc<T> including unsized T. (RangeBounds for ranges of references is held: see
// local/agent_state/fast-rust/std-held/range_refs.patch and coord.md.)
use std::cell::Cell;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::rc::Rc;
use std::sync::Arc;
use std::collections::VecDeque;
use std::ops::{Bound, RangeBounds};

thread_local! {
    static DROPS: Cell<u32> = Cell::new(0);
}

#[derive(Debug)]
struct D(u32);
impl Clone for D {
    fn clone(&self) -> D {
        D(self.0 + 100)
    }
}
impl Drop for D {
    fn drop(&mut self) {
        DROPS.with(|d| d.set(d.get() + 1));
    }
}
fn drops() -> u32 {
    DROPS.with(|d| d.get())
}

#[test]
fn a01_make_mut_unsized() {
    let mut a: Arc<[u8]> = Arc::from(&[1u8, 2, 3][..]);
    let b = a.clone();
    Arc::make_mut(&mut a)[0] = 9;
    println!("| arc [u8] {:?} {:?} {} {}", a, b, Arc::strong_count(&a), Arc::ptr_eq(&a, &b));
    drop(b);
    let p = Arc::as_ptr(&a);
    Arc::make_mut(&mut a)[1] = 8;
    println!("| arc unique {:?} same block {}", a, std::ptr::eq(p, Arc::as_ptr(&a)));
    let w = Arc::downgrade(&a);
    Arc::make_mut(&mut a)[2] = 7;
    println!("| arc weak-only {:?} upgrade {:?}", a, w.upgrade());

    let mut s: Rc<str> = Rc::from("hello");
    let s2 = s.clone();
    Rc::make_mut(&mut s).make_ascii_uppercase();
    println!("| rc str {} {}", s, s2);

    let before = drops();
    let mut v: Rc<[D]> = Rc::from(vec![D(1), D(2)]);
    let v2 = v.clone();
    Rc::make_mut(&mut v)[1].0 += 5;
    println!("| rc [D] {:?} {:?} drops {}", v, v2, drops() - before);
    drop(v);
    drop(v2);
    println!("| rc [D] all dropped {}", drops() - before);

    let mut n = Arc::new(5u32);
    *Arc::make_mut(&mut n) += 1;
    println!("| sized {}", n);
}

fn bounds<R: RangeBounds<usize>>(r: R) -> (Bound<usize>, Bound<usize>) {
    (r.start_bound().cloned(), r.end_bound().cloned())
}

#[test]
fn a02_ranges_of_refs() {
    let (a, b) = (2usize, 5usize);
    println!("| from {:?}", bounds(&a..));
    println!("| to {:?}", bounds(..&b));
    println!("| range {:?}", bounds(&a..&b));
    println!("| incl {:?}", bounds(&a..=&b));
    println!("| to incl {:?}", bounds(..=&b));
    println!("| tuple {:?}", bounds((Bound::Excluded(&a), Bound::Included(&b))));
    let mut v: Vec<u32> = (0..10).collect();
    let start = &a;
    let d: Vec<u32> = v.drain(start..).collect();
    println!("| drain {:?} {:?}", v, d);
    let mut q: VecDeque<u32> = (0..6).collect();
    q.drain(&a..&b);
    println!("| deque {:?}", q);
    // literals stay integers: `RangeBounds<T> for Range<&T>` is no candidate for `1..4`
    let mut w: Vec<u32> = (0..10).collect();
    let e: Vec<u32> = w.drain(1..4).collect();
    let mut r: VecDeque<u32> = (0..6).collect();
    r.drain(1..=2);
    println!("| literal drain {:?} {:?} {:?}", w, e, r);
    println!("| contains {} {}", RangeBounds::<usize>::contains(&(&a..&b), &3), RangeBounds::<usize>::contains(&(&a..=&b), &6));
}

#[test]
fn a03_ip_conversions() {
    let v6 = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1);
    let bits: u128 = v6.into();
    println!("| v6 bits {:#x} back {}", bits, Ipv6Addr::from(bits));
    let v4: u32 = Ipv4Addr::new(10, 1, 2, 3).into();
    println!("| v4 bits {:#x}", v4);
    let ip: IpAddr = [0u16, 0, 0, 0, 0, 0xffff, 0x7f00, 1].into();
    println!("| ip from segments {}", ip);
}

#[test]
fn a04_from_box() {
    let before = drops();
    let r: Rc<D> = Rc::from(Box::new(D(4)));
    println!("| rc box sized {:?} drops {}", r, drops() - before);
    drop(r);
    println!("| after drop {}", drops() - before);
    let s: Arc<[u16]> = Arc::from(vec![1u16, 2, 3].into_boxed_slice());
    println!("| arc box slice {:?}", s);
    let t: Rc<str> = Rc::from(String::from("boxed").into_boxed_str());
    println!("| rc box str {}", t);
    let dynv: Arc<dyn std::fmt::Debug + Send + Sync> = Arc::from(Box::new(42u64) as Box<dyn std::fmt::Debug + Send + Sync>);
    println!("| arc box dyn {:?}", dynv);
    let z: Rc<[u8]> = Rc::from(Vec::<u8>::new().into_boxed_slice());
    println!("| empty {:?}", z);
}

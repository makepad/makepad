// diff_fast corpus program: functional record update from a place behind a reference,
// `S { v: self.v.clone(), ..*self }` (libs/model Body::clone_layout). Relax dropped the base's given
// fields as if `*self` were a temporary: self.v freed under its owner, a Drop field dropped twice.
// And from a field of a temporary, `Inner { swap: x, ..outer().inner }`: the temporary owns the whole
// value, the update moves the fields it does not give out of it, and the temporary drops the rest (the
// given field's old value, the siblings) at the statement's end. Relax dropped the moved fields again.
// rustc --test -O and `makepad test` (all tiers) must print identical D lines.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
struct Tracked(Rc<Cell<u32>>, u32);
impl Drop for Tracked { fn drop(&mut self) { self.0.set(self.0.get() + 1); } }
struct S { a: f64, v: Vec<[f64; 3]>, b: [f64; 3], t: Option<u8> }
impl S {
    #[inline(never)]
    fn copy(&self) -> S { S { v: self.v.clone(), ..*self } }
}
struct T { k: u32, d: Tracked, m: [u64; 4] }
impl T {
    #[inline(never)]
    fn copy(&self, drops: &Rc<Cell<u32>>) -> T { T { d: Tracked(drops.clone(), self.d.1 + 1), ..*self } }
}
type Log = Rc<RefCell<Vec<u32>>>;
struct Noisy(u32, Log);
impl Drop for Noisy { fn drop(&mut self) { self.1.borrow_mut().push(self.0); } }
struct Inner { keep: Noisy, swap: Noisy, buf: Vec<u8> }
struct Outer { inner: Inner, side: Noisy, name: String }
#[inline(never)]
fn outer(log: &Log, id: u32) -> Outer {
    let n = |k| Noisy(id + k, log.clone());
    Outer { inner: Inner { keep: n(1), swap: n(2), buf: vec![id as u8; 3] }, side: n(3), name: format!("outer{id}") }
}
#[inline(never)]
fn pair(log: &Log, id: u32) -> (Noisy, Outer) { (Noisy(id, log.clone()), outer(log, id)) }
fn temp_field_updates() {
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let a = Inner { swap: Noisy(4, log.clone()), ..outer(&log, 10).inner };
    d!("temp field: after the statement {:?}; keep={} swap={} buf={:?}", log.borrow(), a.keep.0, a.swap.0, a.buf);
    drop(a);
    d!("temp field: after its drop {:?}", log.borrow());
    log.borrow_mut().clear();
    let b = Outer { inner: Inner { swap: Noisy(5, log.clone()), ..outer(&log, 20).inner }, ..outer(&log, 30) };
    d!("nested: after the statement {:?}; {} {} {} {:?}", log.borrow(), b.name, b.side.0, b.inner.keep.0, b.inner.buf);
    drop(b);
    d!("nested: after its drop {:?}", log.borrow());
    log.borrow_mut().clear();
    let c = Inner { keep: Noisy(6, log.clone()), ..pair(&log, 40).1.inner };
    d!("tuple field path: after the statement {:?}; swap={} buf={:?}", log.borrow(), c.swap.0, c.buf);
    drop(c);
    d!("tuple field path: after its drop {:?}", log.borrow());
}
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

fn work() {
    let s = S { a: 1.5, v: vec![[1.0, 2.0, 3.0]; 5], b: [4.0, 5.0, 6.0], t: Some(7) };
    let c = s.copy();
    let c2 = s.copy();
    d!("s.v={:?} c.v={:?} c2.v[4]={:?} a={} b={:?} t={:?}", s.v[0], c.v[0], c2.v[4], c.a, c.b, c.t);
    d!("distinct buffers: {}", s.v.as_ptr() != c.v.as_ptr());
    let drops = Rc::new(Cell::new(0));
    let t = T { k: 9, d: Tracked(drops.clone(), 1), m: [1, 2, 3, 4] };
    let u = t.copy(&drops);
    d!("drops after copy={} k={} m={:?} d={} {}", drops.get(), u.k, u.m, t.d.1, u.d.1);
    drop(u);
    drop(t);
    d!("drops at end={}", drops.get());
    temp_field_updates();
}

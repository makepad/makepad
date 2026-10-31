// diff_fast corpus program: functional record update from a place behind a reference,
// `S { v: self.v.clone(), ..*self }` (libs/model Body::clone_layout). Relax dropped the base's given
// fields as if `*self` were a temporary: self.v freed under its owner, a Drop field dropped twice.
// rustc --test -O and `makepad test` (all tiers) must print identical D lines.
use std::cell::Cell;
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
}

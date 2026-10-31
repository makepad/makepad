// diff_fast corpus program: Vec::retain / retain_mut (Relax's std ports real alloc's two-loop form):
// kept order, drops of removed elements, and the Vec after a predicate panics midway (real std keeps
// the unvisited tail, shifted over the holes). rustc --test -O and `makepad test` (all tiers) must
// print identical D lines.
use std::cell::Cell;
use std::rc::Rc;
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

struct T(u32, Rc<Cell<u32>>);
impl Drop for T { fn drop(&mut self) { self.1.set(self.1.get() + 1); } }

fn work() {
    let mut a: Vec<u32> = (0..50).collect();
    a.retain(|x| x % 3 != 0);
    d!("retain u32 {} {:?}", a.len(), &a[..8]);
    let mut all: Vec<u32> = (0..10).collect();
    all.retain(|_| true);
    d!("keep all {:?}", all);
    let mut none: Vec<u32> = (0..10).collect();
    none.retain(|_| false);
    d!("keep none {}", none.len());
    let mut m: Vec<u32> = (0..20).collect();
    m.retain_mut(|x| { *x *= 2; *x % 4 == 0 });
    d!("retain_mut {:?}", m);
    let drops = Rc::new(Cell::new(0));
    let mut v: Vec<T> = (0..12).map(|i| T(i, drops.clone())).collect();
    v.retain(|t| t.0 % 2 == 1);
    d!("drops after retain {} left {:?}", drops.get(), v.iter().map(|t| t.0).collect::<Vec<_>>());
    drops.set(0);
    let mut p: Vec<T> = (0..12).map(|i| T(i, drops.clone())).collect();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        p.retain(|t| {
            if t.0 == 7 { panic!("stop") }
            t.0 % 3 != 0
        });
    }));
    d!("panicked {} drops {} vec {:?}", r.is_err(), drops.get(), p.iter().map(|t| t.0).collect::<Vec<_>>());
}

// diff_fast corpus program: `#[test] run` calls work() in rounds (the O3 tier installs between rounds); every
// result line is `D ...`. rustc --test -O and `makepad test` (baseline and O3) must print identical D lines.
// `const`-initialised thread-locals of types without drop glue (std: a `#[thread_local]` static; Relax: static
// TLS at a fixed offset from the thread pointer when the value starts as zeros, else the thread-local block
// with the value copied in on a thread's first access): zero and non-zero starts, pointers to read-only data
// and to a static, over-aligned values, many threads, each thread's own values, values read in a destructor.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

use std::cell::Cell;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(5);

#[derive(Clone, Copy)]
#[repr(align(32))]
struct Wide([u64; 4]);

thread_local! {
    static ZERO: Cell<u64> = const { Cell::new(0) };
    static SEVEN: Cell<u32> = const { Cell::new(7) };
    static FLAG: Cell<bool> = const { Cell::new(true) };
    static NAME: Cell<&'static str> = const { Cell::new("start") };
    static TARGET: Cell<&'static AtomicU32> = const { Cell::new(&COUNTER) };
    static WIDE: Cell<Wide> = const { Cell::new(Wide([1, 2, 3, 4])) };
    static ZWIDE: Cell<Wide> = const { Cell::new(Wide([0; 4])) };
    static TABLE: [Cell<u16>; 64] = const { [const { Cell::new(0) }; 64] };
    static OPT: Cell<Option<u32>> = const { Cell::new(None) };
}

struct Tail(u32);
impl Drop for Tail {
    fn drop(&mut self) {
        // a thread-local without drop glue stays readable while other destructors run
        let s = SEVEN.with(|c| c.get());
        let z = ZERO.with(|c| c.get());
        if s != 7 + self.0 || z != self.0 as u64 { println!("D tail {} sees seven={} zero={}", self.0, s, z); }
    }
}
thread_local! { static TAIL: std::cell::RefCell<Option<Tail>> = const { std::cell::RefCell::new(None) }; }

#[inline(never)]
fn touch(i: u32) -> u64 {
    ZERO.with(|c| c.set(c.get() + i as u64));
    SEVEN.with(|c| c.set(c.get().wrapping_mul(3).wrapping_add(i)));
    FLAG.with(|c| c.set(!c.get()));
    if i % 7 == 0 { NAME.with(|c| c.set(if c.get() == "start" { "seven" } else { "start" })); }
    TABLE.with(|t| t[(i % 64) as usize].set(t[(i % 64) as usize].get().wrapping_add(i as u16)));
    WIDE.with(|c| { let mut w = c.get(); w.0[(i % 4) as usize] += i as u64; c.set(w) });
    ZWIDE.with(|c| { let mut w = c.get(); w.0[(i % 4) as usize] ^= i as u64; c.set(w) });
    OPT.with(|c| c.set(Some(c.get().unwrap_or(0) + 1)));
    ZERO.with(|c| c.get())
}

fn report(tag: &str) -> String {
    let t: u64 = TABLE.with(|t| t.iter().map(|c| c.get() as u64).sum());
    let w = WIDE.with(|c| c.get().0);
    let z = ZWIDE.with(|c| c.get().0);
    let aligned = WIDE.with(|c| c as *const _ as usize % 32 == 0) && ZWIDE.with(|c| c as *const _ as usize % 32 == 0);
    format!("{} zero={} seven={} flag={} name={} target={} table={} wide={:?} zwide={:?} opt={:?} aligned={}", tag,
        ZERO.with(|c| c.get()), SEVEN.with(|c| c.get()), FLAG.with(|c| c.get()), NAME.with(|c| c.get()),
        std::ptr::eq(TARGET.with(|c| c.get()), &COUNTER), t, w, z, OPT.with(|c| c.get()), aligned)
}

fn work() {
    for c in [&ZERO] { c.with(|c| c.set(0)); }
    SEVEN.with(|c| c.set(7));
    d!("{}", report("fresh-main"));
    let mut s = 0u64;
    for i in 0..1000u32 { s = s.wrapping_add(touch(i)); }
    d!("main s={} {}", s, report("main"));
    let hs: Vec<_> = (0..6u32).map(|t| std::thread::spawn(move || {
        let fresh = report("fresh");
        let mut s = 0u64;
        for i in 0..(300 + 50 * t) { s = s.wrapping_add(touch(i)); }
        TARGET.with(|c| c.get().fetch_add(1, Ordering::Relaxed));
        ZERO.with(|c| c.set(t as u64));
        SEVEN.with(|c| c.set(7 + t));
        TAIL.with(|c| *c.borrow_mut() = Some(Tail(t)));
        (t, fresh, s, report("thread"))
    })).collect();
    for h in hs {
        let (t, fresh, s, r) = h.join().unwrap();
        d!("t{} {}", t, fresh);
        d!("t{} s={} {}", t, s, r);
    }
    d!("counter={}", COUNTER.load(Ordering::Relaxed) >= 5);
}

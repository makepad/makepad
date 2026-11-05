// diff_fast corpus program: `#[test] run` calls work() in rounds (the O3 tier installs between rounds); every
// result line is `D ...`. rustc --test -O and `makepad test` (baseline and O3) must print identical D lines.
// A thread started after another one ended: under Relax it may run on what the ended thread left (its program
// stack, fault stack and thread-local block are kept for the next thread). Its thread-locals start as a new
// thread's: lazy ones initialise again, const ones with a non-zero image read that image, never what the
// ended thread wrote. Threads one after another, and in waves of several at once.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

use std::cell::{Cell, RefCell};

static INITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

thread_local! {
    static SEVEN: Cell<u64> = const { Cell::new(7) };
    static ZERO: Cell<u64> = const { Cell::new(0) };
    static LAZY: RefCell<Vec<u64>> = RefCell::new({ INITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed); vec![1, 2, 3] });
    static NAME: RefCell<String> = RefCell::new(String::from("fresh"));
}

/// What a new thread sees, then leaves its thread-locals changed (and some stack used).
#[inline(never)]
fn body(t: u64) -> String {
    let seen = format!("seven={} zero={} lazy={:?} name={}", SEVEN.with(|c| c.get()), ZERO.with(|c| c.get()), LAZY.with(|v| v.borrow().clone()), NAME.with(|n| n.borrow().clone()));
    SEVEN.with(|c| c.set(1000 + t));
    ZERO.with(|c| c.set(2000 + t));
    LAZY.with(|v| v.borrow_mut().push(t));
    NAME.with(|n| n.borrow_mut().push_str("-used"));
    let mut buf = [0u64; 16384];
    for (i, b) in buf.iter_mut().enumerate() { *b = (i as u64).wrapping_mul(t + 3); }
    let sum = std::hint::black_box(&buf).iter().fold(0u64, |a, b| a.wrapping_add(*b));
    format!("{} sum={}", seen, sum)
}

fn work() {
    INITS.store(0, std::sync::atomic::Ordering::Relaxed);
    for t in 0..4u64 {
        d!("one {} {}", t, std::thread::spawn(move || body(t)).join().unwrap());
    }
    for wave in 0..3u64 {
        let hs: Vec<_> = (0..4u64).map(|t| std::thread::spawn(move || body(wave * 10 + t))).collect();
        for (t, h) in hs.into_iter().enumerate() { d!("wave {} {} {}", wave, t, h.join().unwrap()); }
    }
    d!("lazy inits {}", INITS.load(std::sync::atomic::Ordering::Relaxed));
}

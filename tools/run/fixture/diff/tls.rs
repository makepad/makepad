// diff_fast corpus program: `#[test] run` calls work() in rounds (the O3 tier installs between rounds); every
// result line is `D ...`. rustc --test -O and `makepad test` (baseline and O3) must print identical D lines.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

use std::cell::{Cell, RefCell};
thread_local! {
    static COUNT: Cell<u64> = const { Cell::new(0) };
    static LOG: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    static NAME: RefCell<String> = RefCell::new(String::from("main"));
}
#[inline(never)]
fn step(op: u8, v: u32) -> u64 {
    // a dense match (jump table) whose arms touch thread-locals
    match op {
        0 => { COUNT.with(|c| c.set(c.get() + 1)); 1 }
        1 => { LOG.with(|l| l.borrow_mut().push(v)); 2 }
        2 => { COUNT.with(|c| c.set(c.get().wrapping_mul(3))); 3 }
        3 => NAME.with(|n| { n.borrow_mut().push('a'); n.borrow().len() as u64 }),
        4 => { COUNT.with(|c| c.set(c.get() ^ v as u64)); 5 }
        5 => LOG.with(|l| l.borrow().len() as u64),
        6 => { COUNT.with(|c| c.set(c.get() + 7)); 8 }
        7 => COUNT.with(|c| c.get()),
        8 => { NAME.with(|n| n.borrow_mut().truncate(2)); 9 }
        _ => 0,
    }
}
fn work() {
    COUNT.with(|c| c.set(0));
    LOG.with(|l| l.borrow_mut().clear());
    let mut s = 0u64;
    for i in 0..3000u32 { s = s.wrapping_add(step((i % 11) as u8, i)); }
    d!("main s={} count={} log={} name={}", s, COUNT.with(|c| c.get()), LOG.with(|l| l.borrow().iter().map(|x| *x as u64).sum::<u64>()), NAME.with(|n| n.borrow().clone()));
    let hs: Vec<_> = (0..3u32).map(|t| std::thread::spawn(move || {
        let mut s = 0u64;
        for i in 0..2000u32 { s = s.wrapping_add(step(((i + t) % 11) as u8, i * (t + 1))); }
        (t, s, COUNT.with(|c| c.get()), LOG.with(|l| l.borrow().len()), NAME.with(|n| n.borrow().clone()))
    })).collect();
    for h in hs { d!("{:?}", h.join().unwrap()); }
}

// tests/live_invalid/run.py edits src/work.rs while this runs. A worker thread calls work::step in
// a loop. An edited step calls `park_here`, which blocks until the test says `free` on stdin: the
// edited step's frame stays on the worker's stack while the test makes its code invalid. Prints
// "out start", "out tick <n> <value>" when the value changes, "out parked <x>" when the worker
// blocks in park_here. "hold0" makes the next call of step block in park_here from its own first
// version (gen 0), whatever edits came since.
mod work;

use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

pub static FREE: AtomicBool = AtomicBool::new(false);
/// set by "hold0": the next step (its first version, gen 0) blocks in park_here
pub static HOLD0: AtomicBool = AtomicBool::new(false);

/// Blocks until the test frees it (its caller's frame stays live meanwhile).
#[inline(never)]
pub fn park_here(x: u64) -> u64 {
    println!("out parked {}", x);
    while !FREE.swap(false, SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    x + 1
}

fn main() {
    println!("out start");
    std::thread::spawn(|| {
        for line in std::io::stdin().lock().lines() {
            match line.as_deref().unwrap_or("quit").trim() {
                "free" => FREE.store(true, SeqCst),
                "hold0" => HOLD0.store(true, SeqCst),
                _ => std::process::exit(0),
            }
        }
        std::process::exit(0);
    });
    let worker = std::thread::spawn(|| {
        let st = work::leaked();
        let mut last = u64::MAX;
        let mut n: u64 = 0;
        loop {
            let v = work::step(st, n);
            if v != last {
                println!("out tick {} {}", n, v);
                last = v;
            }
            n += 1;
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    });
    let t0 = std::time::Instant::now();
    while t0.elapsed().as_secs() < 300 && !worker.is_finished() {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

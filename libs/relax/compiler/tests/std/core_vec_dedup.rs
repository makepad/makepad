// Vec deduplication results and ownership after predicate or destructor panics.
// Differential file: `| ` lines must match under rustc --test and makepad test.

use std::cell::Cell;
use std::panic::{catch_unwind, AssertUnwindSafe};

thread_local! {
    static DROPS: Cell<usize> = Cell::new(0);
    static PANIC_DROP: Cell<u32> = Cell::new(u32::MAX);
}

#[derive(Debug, PartialEq)]
struct Dropped(u32);

impl Drop for Dropped {
    fn drop(&mut self) {
        DROPS.with(|d| d.set(d.get() + 1));
        if PANIC_DROP.with(|p| p.get()) == self.0 {
            PANIC_DROP.with(|p| p.set(u32::MAX));
            panic!("drop boom");
        }
    }
}

fn drops() -> usize {
    DROPS.with(|d| d.get())
}

const INPUT: [u32; 11] = [1, 1, 2, 2, 3, 4, 4, 4, 5, 6, 6];

// Runs dedup_by with a same_bucket that panics on call `at` (or a Drop that panics for value
// `drop_panic`), returns (panicked, drops during, remaining values, total drops).
fn run(at: usize, drop_panic: u32) -> (bool, usize, Vec<u32>, usize) {
    let before = drops();
    let mut v: Vec<Dropped> = INPUT.iter().map(|&x| Dropped(x)).collect();
    let mut calls = 0usize;
    PANIC_DROP.with(|p| p.set(drop_panic));
    let r = catch_unwind(AssertUnwindSafe(|| {
        v.dedup_by(|a, b| {
            if calls == at {
                panic!("boom");
            }
            calls += 1;
            a.0 == b.0
        })
    }));
    PANIC_DROP.with(|p| p.set(u32::MAX));
    let during = drops() - before;
    let left: Vec<u32> = v.iter().map(|d| d.0).collect();
    drop(v);
    (r.is_err(), during, left, drops() - before)
}

#[test]
fn a01_dedup_by_panic_safety() {
    std::panic::set_hook(Box::new(|_| {}));
    for at in 0..12 {
        let result = run(at, u32::MAX);
        assert_eq!(result.3, INPUT.len());
        println!("| dedup predicate {} {:?}", at, result);
    }
    for dp in [1u32, 2, 4, 6] {
        let result = run(usize::MAX, dp);
        assert_eq!(result.3, INPUT.len());
        println!("| dedup drop {} {:?}", dp, result);
    }
}

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
}

#[test]
fn a02_dedup_results() {
    let mut x = 0x2545F4914F6CDD1Du64;
    let mut d = D::new();
    for len in 0..200usize {
        let mut a: Vec<u32> = Vec::new();
        for _ in 0..len {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            a.push((x % 4) as u32);
        }
        a.dedup();
        d.bytes(&(a.len() as u64).to_le_bytes());
        for value in a {
            d.bytes(&value.to_le_bytes());
        }
        d.1 += 1;
    }
    println!("| dedup results {} {:016x}", d.1, d.0);
}

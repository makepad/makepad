//! Relax std Vec::dedup_by vs real std: results and panic safety (a panicking same_bucket or Drop
//! leaves the Vec valid; every element is dropped exactly once).
use relax_std_check::vec::Vec as HVec;
use std::cell::Cell;
use std::panic::{catch_unwind, AssertUnwindSafe};

thread_local! {
    static DROPS: Cell<usize> = Cell::new(0);
    static PANIC_DROP: Cell<u32> = Cell::new(u32::MAX);
}

#[derive(Debug, PartialEq)]
struct D(u32);

impl Drop for D {
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
fn run_real(at: usize, drop_panic: u32) -> (bool, usize, Vec<u32>, usize) {
    let before = drops();
    let mut v: Vec<D> = INPUT.iter().map(|&x| D(x)).collect();
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

fn run_relax(at: usize, drop_panic: u32) -> (bool, usize, Vec<u32>, usize) {
    let before = drops();
    let mut v: HVec<D> = HVec::new();
    for &x in INPUT.iter() {
        v.push(D(x));
    }
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
fn dedup_by_panic_safety() {
    std::panic::set_hook(Box::new(|_| {}));
    for at in 0..12 {
        assert_eq!(run_relax(at, u32::MAX), run_real(at, u32::MAX), "same_bucket panics at call {}", at);
    }
    for dp in [1u32, 2, 4, 6] {
        assert_eq!(run_relax(usize::MAX, dp), run_real(usize::MAX, dp), "drop of {} panics", dp);
    }
    let _ = std::panic::take_hook();
}

#[test]
fn dedup_results() {
    let mut x = 0x2545F4914F6CDD1Du64;
    for len in 0..200usize {
        let mut a: Vec<u32> = Vec::new();
        for _ in 0..len {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            a.push((x % 4) as u32);
        }
        let mut h: HVec<u32> = HVec::new();
        for &e in a.iter() {
            h.push(e);
        }
        a.dedup();
        h.dedup();
        assert_eq!(&h[..], &a[..]);
    }
}

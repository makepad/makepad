// Vec::dedup / dedup_by / dedup_by_key: results and drop counts. (Panic safety -- same_bucket panicking
// part-way -- is checked in std/check/tests/vec_dedup.rs: Relax panics do not unwind, R11.)
use std::cell::Cell;

thread_local! {
    static DROPS: Cell<usize> = Cell::new(0);
}

#[derive(Debug, PartialEq)]
struct D(u32);

impl Drop for D {
    fn drop(&mut self) {
        DROPS.with(|d| d.set(d.get() + 1));
    }
}

fn drops() -> usize {
    DROPS.with(|d| d.get())
}

#[test]
fn a01_dedup() {
    let cases: [&[u32]; 7] = [&[], &[1], &[1, 1], &[1, 2, 3], &[1, 1, 2, 2, 2, 3, 1, 1], &[5, 5, 5, 5], &[1, 2, 2, 3, 3, 3, 4]];
    for c in cases.iter() {
        let mut v = c.to_vec();
        v.dedup();
        println!("| dedup {:?} -> {:?}", c, v);
        let mut w = c.to_vec();
        w.dedup_by_key(|x| *x / 2);
        println!("| dedup_by_key/2 {:?} -> {:?}", c, w);
        let mut u = c.to_vec();
        u.dedup_by(|a, b| a.abs_diff(*b) <= 1);
        println!("| dedup_by near {:?} -> {:?}", c, u);
    }
    let mut big: Vec<u64> = (0..100_000u64).map(|i| i / 3).collect();
    big.dedup();
    println!("| big {} {:?}", big.len(), big.last());
}

#[test]
fn a02_drops() {
    let before = drops();
    let mut v: Vec<D> = [1u32, 1, 2, 3, 3, 3, 4].iter().map(|&x| D(x)).collect();
    v.dedup();
    println!("| dedup drops {} left {:?}", drops() - before, v);
    drop(v);
    println!("| total drops {}", drops() - before);
}

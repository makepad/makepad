// std's sort picks its small-sort by `T: Freeze` (real core's specialization; Relax's
// is_freeze intrinsic per instance): comparison counts follow the same algorithm as rustc for
// Freeze non-Copy types (String), types with a Cell inside, and Copy types.
use std::cell::Cell;

fn count<T: Clone, F: Fn(&T, &T) -> std::cmp::Ordering>(v: &[T], f: F) -> usize {
    let n = Cell::new(0usize);
    let mut w = v.to_vec();
    w.sort_by(|a, b| {
        n.set(n.get() + 1);
        f(a, b)
    });
    let mut u = v.to_vec();
    u.sort_unstable_by(|a, b| {
        n.set(n.get() + 1000);
        f(a, b)
    });
    n.get()
}

#[derive(Clone)]
struct C(Cell<u32>, u32);

#[test]
fn f01_freeze_dispatch() {
    let mut seed = 12345u32;
    let mut next = || {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        (seed >> 8) % 1000
    };
    for len in [5usize, 17, 20, 33, 64] {
        let nums: Vec<u32> = (0..len).map(|_| next()).collect();
        let strs: Vec<String> = nums.iter().map(|x| format!("{:04}", x)).collect();
        let cells: Vec<C> = nums.iter().map(|x| C(Cell::new(0), *x)).collect();
        println!(
            "| {} {} {} {}",
            len,
            count(&nums, |a, b| a.cmp(b)),
            count(&strs, |a, b| a.cmp(b)),
            count(&cells, |a, b| {
                a.0.set(a.0.get() + 1);
                a.1.cmp(&b.1)
            })
        );
    }
}

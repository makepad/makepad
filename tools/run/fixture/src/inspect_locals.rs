//! makepad inspect `locals` against a live program (tools/run/fixture/inspect_locals.sh): a bounds
//! panic inside `pick`, which `outer` inlines once it runs optimised code, with locals of every
//! kind at the panic: an integer held across the panic call (`scale`), a struct whose address a
//! call takes (`p`, a stack slot), a value dead before the call (`gone`: optimised out), and the
//! inlined fn's own (`i`, `doubled`). A thread named `parked` waits in a futex the whole run
//! (`stacks thread=parked`: a blocked thread's program frames).

#[derive(Debug, Clone, Copy)]
pub struct Pt {
    pub x: i32,
    pub y: i32,
}

#[inline(never)]
pub fn sink(p: &Pt) -> i32 {
    p.x ^ p.y
}

#[inline]
pub fn step(x: u32) -> u32 {
    (x << 1) ^ (x >> 3)
}

#[inline]
pub fn pick(v: &[u32], i: usize, k: u32) -> u32 {
    let doubled = k.wrapping_mul(2);
    // (a branch on it, through black_box: it is computed before the index check at every tier.
    // A plain `doubled == 7` folds away at O3, an even number is never 7, and the product then
    // sinks past the check)
    if std::hint::black_box(doubled) == 7 {
        return 0;
    }
    v[i].wrapping_add(doubled).wrapping_add(doubled >> 3)
}

#[inline(never)]
pub fn outer(n: u32, at: usize) -> u32 {
    let table = [1u32, 2, 3, 4];
    let p = Pt { x: n as i32, y: 7 };
    let base = sink(&p);
    let scale = n.wrapping_mul(3).wrapping_add(1);
    let gone = n ^ 0x55;
    let mut s = gone.count_ones();
    let mut i = 0u32;
    while i < n {
        s = s.wrapping_add(step(i));
        i += 1;
    }
    let got = pick(&table, at, scale);
    s.wrapping_add(got).wrapping_add(scale).wrapping_add(base as u32).wrapping_add(n).wrapping_add(p.x as u32)
}

/// Parks for good: blocked in a futex, never running (no frame pointer in its walk).
#[inline(never)]
pub fn park_here() {
    loop {
        std::thread::park();
    }
}

fn main() {
    std::thread::Builder::new().name("parked".into()).spawn(park_here).unwrap();
    let trigger = std::env::var("PROBE_TRIGGER").unwrap_or_default();
    let secs: u64 = std::env::var("PROBE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(120);
    let mut total = 0u32;
    for _ in 0..secs * 50 {
        let mut at = 1usize;
        if !trigger.is_empty() && std::path::Path::new(&trigger).exists() {
            let _ = std::fs::remove_file(&trigger);
            at = 9;
        }
        for _ in 0..200 {
            total = total.wrapping_add(outer(1000, at));
            at = 1;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    println!("total {}", total);
}

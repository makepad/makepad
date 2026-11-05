// diff_fast corpus program: loads of a local struct that stays in the frame (built there by a call
// that keeps no copy, as `SegGrid::new`) read in each step of a loop that writes other memory --
// LICM takes them out of the loop (a local grid read by an inlined cell walk) -- next to the loops
// where they must stay: the struct handed to a call in the loop that changes it, and a struct whose
// address was kept elsewhere and changed through that copy. rustc --test -O and `makepad test`
// (all tiers) must print identical D lines.
use std::cell::Cell;
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

struct Grid {
    nx: i64,
    ny: i64,
    at: Vec<u32>,
    items: Vec<u32>,
}

#[inline(never)]
fn make(nx: i64, ny: i64) -> Grid {
    let mut at = Vec::new();
    let mut items = Vec::new();
    let mut k = 0u32;
    for c in 0..(nx * ny) as u32 {
        at.push(k);
        for j in 0..(c * 7 + 3) % 5 {
            items.push(c * 31 + j);
            k += 1;
        }
    }
    at.push(k);
    Grid { nx, ny, at, items }
}

#[inline(never)]
fn note(out: &mut Vec<u32>, v: u32) {
    if v % 3 == 0 {
        out.push(v);
    }
}

/// The cell walk: the grid is only read in the loops; `out` (other memory) is written.
#[inline(never)]
fn walk(nx: i64, ny: i64, out: &mut Vec<u32>) -> u64 {
    let g = make(nx, ny);
    let mut s = 0u64;
    for y in 0..g.ny {
        let mut x = 0;
        while x < g.nx {
            let c = (y * g.nx + x) as usize;
            for &i in &g.items[g.at[c] as usize..g.at[c + 1] as usize] {
                s = s.wrapping_mul(33).wrapping_add(i as u64);
                note(out, i);
            }
            x += 1;
        }
    }
    s
}

#[inline(never)]
fn shrink(g: &mut Grid, x: i64) {
    if x == 2 {
        g.nx -= 1;
    }
}

/// The loop hands the grid to a call that changes `nx`: the bound is read again each step.
#[inline(never)]
fn walk_changing(nx: i64) -> i64 {
    let mut g = make(nx, 1);
    let mut x = 0;
    let mut s = 0;
    while x < g.nx {
        s += x * 10 + g.nx;
        shrink(&mut g, x);
        x += 1;
    }
    s
}

struct Bound {
    nx: Cell<i64>,
}

struct Holder<'a> {
    b: &'a Bound,
}

#[inline(never)]
fn keep<'a>(h: &mut Holder<'a>, b: &'a Bound) {
    h.b = b;
}

/// The bound's address is kept elsewhere and the loop changes `nx` through that copy.
#[inline(never)]
fn walk_kept(nx: i64, other: &Bound) -> i64 {
    let b = Bound { nx: Cell::new(nx) };
    let mut h = Holder { b: other };
    keep(&mut h, &b);
    let h = std::hint::black_box(&h);
    let mut x = 0;
    let mut s = 0;
    while x < b.nx.get() {
        s += x * 10 + b.nx.get();
        if x == 2 {
            h.b.nx.set(h.b.nx.get() - 1);
        }
        x += 1;
    }
    s
}

fn work() {
    let other = Bound { nx: Cell::new(0) };
    let (mut sum, mut outs, mut ch, mut kept) = (0u64, 0usize, 0i64, 0i64);
    for round in 0..600 {
        let (nx, ny) = (3 + round % 7, 2 + round % 5);
        let mut out = Vec::new();
        sum = sum.wrapping_mul(7).wrapping_add(walk(nx, ny, &mut out));
        outs += out.len();
        let n = 4 + round % 3;
        ch += walk_changing(n);
        kept += walk_kept(n, &other);
    }
    d!("walk {sum} out {outs} changing {ch} kept {kept}");
}

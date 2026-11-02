// diff_fast corpus program: `let (a, b);` declared without an initializer and assigned later, the
// bindings in memory (their address is taken) and first assigned in a branch that does not run.
// Each binding's storage begins at the `let`; a lowering that allocated it at its first assignment
// handed the other branches an address that was never computed (csg_brep evaluate_geometric:
// wrong principal directions on x86-64, a fault at address 0x1 on arm64).
use std::hint::black_box;
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

#[derive(Debug, Clone, Copy, PartialEq)]
struct V { x: f64, y: f64, z: f64 }
impl core::ops::Neg for V { type Output = V; fn neg(self) -> V { V { x: -self.x, y: -self.y, z: -self.z } } }

#[inline(never)]
fn tuple(which: u32, a: V, b: V) -> (V, V) {
    let (mut v1, mut v2);
    if which == 0 {
        v1 = a;
        v2 = b;
    } else if which == 1 {
        v1 = b;
        v2 = a;
    } else {
        v1 = -a;
        v2 = -b;
    }
    if a.x > b.x {
        core::mem::swap(&mut v1, &mut v2);
    }
    if v1.z < 0.0 {
        v2 = -v2;
    }
    (v1, v2)
}

#[inline(never)]
fn nested(which: bool, a: V, n: u64) -> (V, u64, [u64; 3]) {
    let (mut p, (mut q, mut r));
    if which {
        p = a;
        q = n;
        r = [n; 3];
    } else {
        p = -a;
        q = n + 1;
        r = [1, 2, n];
    }
    let rp = &mut r;
    rp[0] += 10;
    core::mem::swap(&mut p.x, &mut p.y);
    let qp = &mut q;
    *qp *= 3;
    (p, q, r)
}

#[inline(never)]
fn in_loop(n: u32) -> f64 {
    let mut acc = 0.0;
    for i in 0..n {
        let (mut s, mut t);
        if i % 3 == 0 {
            s = V { x: i as f64, y: 1.0, z: 2.0 };
            t = V { x: 0.5, y: i as f64, z: -1.0 };
        } else {
            t = V { x: 1.5, y: 2.5, z: i as f64 };
            s = -t;
        }
        core::mem::swap(&mut s, &mut t);
        acc += s.x + 2.0 * s.y + 3.0 * s.z + 5.0 * t.x + 7.0 * t.y + 11.0 * t.z;
    }
    acc
}

fn work() {
    let a = V { x: 1.0, y: 2.0, z: -3.0 };
    let b = V { x: 4.0, y: -5.0, z: 6.0 };
    for w in 0..3 {
        d!("tuple {w} {:?}", tuple(black_box(w), black_box(a), black_box(b)));
        d!("tuple-rev {w} {:?}", tuple(black_box(w), black_box(b), black_box(a)));
    }
    for w in [true, false] {
        d!("nested {w} {:?}", nested(black_box(w), black_box(a), black_box(7)));
    }
    d!("loop {}", in_loop(black_box(10)));
}

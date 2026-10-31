// diff_fast corpus program: `#[test] run` calls work() in rounds (the O3 tier installs between rounds); every
// result line is `D ...`. rustc --test -O and `makepad test` (baseline and O3) must print identical D lines.
// Fused multiply-adds with NaN operands: f32/f64 `mul_add` in every negation form ((-a).mul_add(b, c),
// a.mul_add(b, -c), both), NaNs of both signs, quiet and signaling, one or several per call, inf * 0 and
// inf - inf. The bits are what rustc -O (no fma target feature) gives: x86-64 calls the C library's
// fmaf/fma (vfmadd213, the first NaN of b, a, c), AArch64 emits fmadd/fmsub/fnmsub/fnmadd (Arm's rule);
// a negated NaN operand's sign is flipped first. Relax's rule is relax_backend::rir::fma_nan_f32 / f64.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

use std::hint::black_box;

#[inline(never)]
fn f32_forms(a: f32, b: f32, c: f32) -> [u32; 4] {
    [a.mul_add(b, c).to_bits(), (-a).mul_add(b, c).to_bits(), a.mul_add(b, -c).to_bits(), (-a).mul_add(b, -c).to_bits()]
}
#[inline(never)]
fn f64_forms(a: f64, b: f64, c: f64) -> [u64; 4] {
    [a.mul_add(b, c).to_bits(), (-a).mul_add(b, c).to_bits(), a.mul_add(b, -c).to_bits(), (-a).mul_add(b, -c).to_bits()]
}
// an accumulation (the result in c's register: x64's 231 form) and a chain reusing b (the 132 form)
#[inline(never)]
fn f32_chain(xs: &[f32]) -> u64 {
    let mut acc = xs[0];
    let mut h = 0u64;
    for w in xs.windows(3) {
        acc = w[0].mul_add(w[1], acc);
        let e = w[2].mul_add(acc, w[0]);
        let g = (-w[1]).mul_add(e, w[2]);
        h = h.rotate_left(7) ^ acc.to_bits() as u64 ^ ((e.to_bits() as u64) << 32) ^ g.to_bits() as u64;
    }
    h
}
#[inline(never)]
fn f64_chain(xs: &[f64]) -> u64 {
    let mut acc = xs[0];
    let mut h = 0u64;
    for w in xs.windows(3) {
        acc = w[0].mul_add(w[1], acc);
        let e = w[2].mul_add(acc, w[0]);
        let g = (-w[1]).mul_add(e, w[2]);
        h = h.rotate_left(7) ^ acc.to_bits() ^ e.to_bits().rotate_left(17) ^ g.to_bits();
    }
    h
}

fn mix(h: u64, x: u64) -> u64 { (h ^ x).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(23) }

fn work() {
    let v32: Vec<f32> = [0x3f80_0000u32, 0xbfc0_0000, 0, 0x8000_0000, 0x7f80_0000, 0xff80_0000, 0x7fc0_0000, 0xffc0_0000, 0x7fc3_872e, 0xffc1_2345, 0x7f80_0567, 0xff80_0089, 0x7149_f2ca]
        .iter().map(|&x| f32::from_bits(x)).collect();
    let v64: Vec<f64> = [0x3ff0_0000_0000_0000u64, 0xbff8_0000_0000_0000, 0, 0x8000_0000_0000_0000, 0x7ff0_0000_0000_0000, 0xfff0_0000_0000_0000, 0x7ff8_0000_0000_0000, 0xfff8_0000_0000_0000, 0x7ff8_0000_0000_1234, 0xfff8_0000_0004_4321, 0x7ff0_0000_0000_0567, 0xfff0_0000_0000_0089, 0x7e37_e43c_8800_759c]
        .iter().map(|&x| f64::from_bits(x)).collect();
    // per form: one hash over the triples with at most one NaN operand, one over the rest
    let (mut one32, mut many32, mut one64, mut many64) = ([0u64; 4], [0u64; 4], [0u64; 4], [0u64; 4]);
    for &a in &v32 {
        for &b in &v32 {
            for &c in &v32 {
                let r = f32_forms(black_box(a), black_box(b), black_box(c));
                let nans = [a, b, c].iter().filter(|x| x.is_nan()).count();
                let h = if nans <= 1 { &mut one32 } else { &mut many32 };
                for k in 0..4 { h[k] = mix(h[k], r[k] as u64); }
            }
        }
    }
    for &a in &v64 {
        for &b in &v64 {
            for &c in &v64 {
                let r = f64_forms(black_box(a), black_box(b), black_box(c));
                let nans = [a, b, c].iter().filter(|x| x.is_nan()).count();
                let h = if nans <= 1 { &mut one64 } else { &mut many64 };
                for k in 0..4 { h[k] = mix(h[k], r[k]); }
            }
        }
    }
    d!("f32 one-nan {:016x?}", one32);
    d!("f32 many-nan {:016x?}", many32);
    d!("f64 one-nan {:016x?}", one64);
    d!("f64 many-nan {:016x?}", many64);
    // the single-NaN cases spelled out (sign flips of negated operands)
    let q = f32::from_bits(0x7fc3_872e);
    for (a, b, c) in [(q, 2.0f32, 1.0f32), (2.0, q, 1.0), (2.0, 1.0, q), (-q, 2.0, 1.0), (2.0, 1.0, -q)] {
        d!("f32 {:08x} {:08x} {:08x} -> {:08x?}", a.to_bits(), b.to_bits(), c.to_bits(), f32_forms(black_box(a), black_box(b), black_box(c)));
    }
    let ch32: Vec<f32> = (0..64).map(|i| if i % 5 == 2 { f32::from_bits(if i % 2 == 0 { 0xffc0_0000 } else { 0x7fc0_0000 } | i as u32) } else { i as f32 * 0.37 - 7.0 }).collect();
    let ch64: Vec<f64> = (0..64).map(|i| if i % 5 == 2 { f64::from_bits(if i % 2 == 0 { 0xfff8_0000_0000_0000 } else { 0x7ff8_0000_0000_0000 } | i as u64) } else { i as f64 * 0.37 - 7.0 }).collect();
    d!("chains {:016x} {:016x}", f32_chain(black_box(&ch32)), f64_chain(black_box(&ch64)));
}

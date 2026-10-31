// diff_fast corpus program: std::simd (portable_simd) -- arithmetic, compares and masks, select, swizzles,
// reductions, casts, gathers from slices, byte conversions, for f32/f64/i8..i64/u8..u64 and N = 2..64;
// every result line is `D ...`. (rustc side: RUSTC_BOOTSTRAP=1, set by diff_fast.sh for #![feature] programs.)
#![feature(portable_simd)]
use std::simd::prelude::*;
use std::simd::{Simd, StdFloat, Swizzle};

// (an explicit Swizzle impl: simd_swizzle!'s `impl Swizzle<{ $index.len() }>` hits a Relax CTFE finding,
// a false E0080 for a method call in an anon const of an impl header)
struct Mix;
impl Swizzle<8> for Mix {
    const INDEX: [usize; 8] = [0, 9, 2, 11, 4, 13, 6, 15];
}

macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[inline(never)]
fn floats(rng: &mut Rng) {
    let a: [f32; 8] = core::array::from_fn(|_| (rng.next() as i32 as f32) * 1e-7);
    let b: [f32; 8] = core::array::from_fn(|_| (rng.next() as i32 as f32) * 1e-8);
    let (va, vb) = (f32x8::from_array(a), f32x8::from_array(b));
    let s = va + vb * f32x8::splat(0.5) - va / vb.abs().simd_max(f32x8::splat(1.0));
    d!("f32x8 {:?}", s.to_array());
    d!("sum {} prod {} min {} max {}", s.reduce_sum(), (s * f32x8::splat(1e-3)).reduce_product(), s.reduce_min(), s.reduce_max());
    let m = va.simd_lt(vb);
    d!("mask {:?} bits {:#b} any {} all {}", m.to_array(), m.to_bitmask(), m.any(), m.all());
    let sel = m.select(va, vb);
    d!("select {:?}", sel.to_array());
    d!("floor {:?} sqrt {:?}", va.floor().to_array(), vb.abs().sqrt().to_array());
    d!("mul_add {:?}", va.mul_add(vb, va).to_array());
    let i: i32x8 = va.cast();
    d!("cast {:?}", i.to_array());
    let r = Mix::concat_swizzle(va, vb);
    d!("swizzle {:?} rev {:?}", r.to_array(), va.reverse().to_array());
    d!("rotate {:?}", va.rotate_elements_left::<3>().to_array());
    let (lo, hi) = va.interleave(vb);
    d!("interleave {:?} {:?}", lo.to_array(), hi.to_array());
    let dd = f64x4::from_array([a[0] as f64, a[1] as f64, f64::NAN, -0.0]);
    d!("f64x4 {:?} {:?}", (dd * dd).to_array().map(|x| if x.is_nan() { 0.0 } else { x }), dd.is_nan().to_array());
}

#[inline(never)]
fn ints(rng: &mut Rng) {
    let a: [i32; 4] = core::array::from_fn(|_| rng.next() as i32);
    let b: [i32; 4] = core::array::from_fn(|_| rng.next() as i32);
    let (va, vb) = (i32x4::from_array(a), i32x4::from_array(b));
    d!("i32x4 {:?} {:?} {:?}", (va + vb).to_array(), (va * vb).to_array(), (va ^ vb >> Simd::splat(3)).to_array());
    d!("sat {:?} {:?}", va.saturating_add(vb).to_array(), va.saturating_sub(vb).to_array());
    d!("min {:?} max {:?} abs {:?}", va.simd_min(vb).to_array(), va.simd_max(vb).to_array(), va.abs().to_array());
    d!("reduce {} {} {} {} {}", va.reduce_sum(), va.reduce_and(), va.reduce_or(), va.reduce_xor(), va.reduce_max());
    let u: u8x16 = u8x16::from_array(core::array::from_fn(|_| rng.next() as u8));
    d!("u8x16 {:?} {:?}", (u + u8x16::splat(200)).to_array(), u.simd_gt(u8x16::splat(127)).to_bitmask());
    d!("count {:?}", u.count_ones().to_array());
    let w: u64x2 = u64x2::from_array([rng.next(), rng.next()]);
    d!("u64x2 {:?} {:?}", (w * w).to_array(), w.leading_zeros().to_array());
    let big: Simd<u16, 32> = Simd::from_array(core::array::from_fn(|i| (rng.next() as u16) ^ i as u16));
    d!("u16x32 sum {} max {}", big.reduce_sum(), big.reduce_max());
    let data: Vec<i32> = (0..100).map(|i| i * 7 - 300).collect();
    let idx = usizex4::from_array([3, 99, 50, 120]);
    let g = Simd::gather_or_default(&data, idx);
    d!("gather {:?}", g.to_array());
    let bytes = va.to_le_bytes();
    d!("bytes {:?}", bytes.to_array());
    let wide: i64x4 = va.cast();
    d!("widen {:?}", (wide * wide).to_array());
}

fn work() {
    let mut rng = Rng(0x0123_4567_89ab_cdef);
    floats(&mut rng);
    ints(&mut rng);
}

// diff_fast corpus program: rustc's generic SIMD intrinsics (core::intrinsics::simd) on #[repr(simd)] types of
// every lane type and widths 8..32 bytes, edge and pseudo-random lanes; every result line is `D ...`.
// (rustc side: nightly features through RUSTC_BOOTSTRAP=1, set by diff_fast.sh for programs with #![feature].)
// NaN results print canonically (Rust leaves NaN payloads unspecified); minimum/maximum_number_nsz print
// zeros without sign (either zero may be returned).
#![feature(repr_simd, core_intrinsics)]
#![allow(non_camel_case_types, internal_features)]
use core::intrinsics::simd::*;

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

fn mix(h: &mut u64, x: u64) {
    *h = (*h ^ x).wrapping_mul(0x100000001b3).rotate_left(7);
}

const EDGE: [u64; 12] = [0, 1, 2, 7, u64::MAX, 0x8000000000000000, 0x7fffffffffffffff, 0x80, 0x7f, 0x8000, 0x7fff, 0x80000000];

macro_rules! ints {
    ($f:ident, $v:ident, $m:ident, $e:ty, $n:expr) => {
        #[repr(simd)]
        #[derive(Copy, Clone)]
        struct $v([$e; $n]);
        #[repr(simd)]
        #[derive(Copy, Clone)]
        struct $m([$e; $n]);
        #[inline(never)]
        fn $f(rng: &mut Rng) {
            let bits = <$e>::BITS as u64;
            let mut hs = [0u64; 20];
            for _ in 0..64 {
                let mut a = [0 as $e; $n];
                let mut b = [0 as $e; $n];
                let mut s = [0 as $e; $n];
                for k in 0..$n {
                    let r = rng.next();
                    a[k] = (if r % 3 == 0 { rng.next() } else { EDGE[(r >> 8) as usize % EDGE.len()] }) as $e;
                    let r = rng.next();
                    b[k] = (if r % 3 == 0 { rng.next() } else { EDGE[(r >> 8) as usize % EDGE.len()] }) as $e;
                    s[k] = (rng.next() % bits) as $e;
                }
                let (va, vb, vs) = ($v(a), $v(b), $v(s));
                let r: [$v; 13] = unsafe {
                    [
                        simd_add(va, vb), simd_sub(va, vb), simd_mul(va, vb), simd_and(va, vb), simd_or(va, vb),
                        simd_xor(va, vb), simd_shl(va, vs), simd_shr(va, vs), simd_sub(simd_xor(va, va), va),
                        simd_saturating_add(va, vb), simd_saturating_sub(va, vb),
                        simd_select(simd_lt::<$v, $m>(va, vb), va, vb), simd_select(simd_ge::<$v, $m>(va, vb), vb, va),
                    ]
                };
                let m: [$m; 6] = unsafe { [simd_eq(va, vb), simd_ne(va, vb), simd_lt(va, vb), simd_le(va, vb), simd_gt(va, vb), simd_ge(va, vb)] };
                for (i, v) in r.iter().enumerate() {
                    let l: [$e; $n] = unsafe { core::mem::transmute(*v) };
                    for x in l { mix(&mut hs[i], x as u64); }
                }
                for (i, v) in m.iter().enumerate() {
                    let l: [$e; $n] = unsafe { core::mem::transmute(*v) };
                    for x in l { mix(&mut hs[13 + i], x as u64); }
                }
            }
            d!("{} {:016x?}", stringify!($v), hs);
        }
    };
}

macro_rules! floats {
    ($f:ident, $v:ident, $m:ident, $e:ty, $mi:ty, $n:expr) => {
        #[repr(simd)]
        #[derive(Copy, Clone)]
        struct $v([$e; $n]);
        #[repr(simd)]
        #[derive(Copy, Clone)]
        struct $m([$mi; $n]);
        #[inline(never)]
        fn $f(rng: &mut Rng) {
            let edge: [$e; 16] = [0.0, -0.0, 1.0, -1.0, 0.5, 1.5, 2.5, -2.5, <$e>::INFINITY, <$e>::NEG_INFINITY, <$e>::NAN, <$e>::MAX, <$e>::MIN_POSITIVE, 1e-40 as $e, 0.49999997, 3.0e9];
            let canon = |x: $e, nsz: bool| -> u64 {
                if x.is_nan() { 1 } else if nsz && x == 0.0 { 0 } else { x.to_bits() as u64 }
            };
            let mut hs = [0u64; 21];
            for _ in 0..64 {
                let mut a = [0.0 as $e; $n];
                let mut b = [0.0 as $e; $n];
                for k in 0..$n {
                    let r = rng.next();
                    a[k] = if r % 4 == 0 { (rng.next() as i64 as $e) * 1e-12 } else { edge[(r >> 8) as usize % 16] };
                    let r = rng.next();
                    b[k] = if r % 4 == 0 { (rng.next() as i64 as $e) * 1e-15 } else { edge[(r >> 8) as usize % 16] };
                }
                let (va, vb) = ($v(a), $v(b));
                let r: [$v; 13] = unsafe {
                    [
                        simd_add(va, vb), simd_sub(va, vb), simd_mul(va, vb), simd_div(va, vb), simd_neg(va), simd_fabs(va),
                        simd_fsqrt(va), simd_floor(va), simd_ceil(va), simd_trunc(va), simd_round(va),
                        simd_select(simd_lt::<$v, $m>(va, vb), va, vb), simd_select(simd_ne::<$v, $m>(va, vb), vb, va),
                    ]
                };
                let mm: [$v; 2] = unsafe { [simd_minimum_number_nsz(va, vb), simd_maximum_number_nsz(va, vb)] };
                let m: [$m; 6] = unsafe { [simd_eq(va, vb), simd_ne(va, vb), simd_lt(va, vb), simd_le(va, vb), simd_gt(va, vb), simd_ge(va, vb)] };
                for (i, v) in r.iter().enumerate() {
                    let l: [$e; $n] = unsafe { core::mem::transmute(*v) };
                    for x in l { mix(&mut hs[i], canon(x, false)); }
                }
                for (i, v) in mm.iter().enumerate() {
                    let l: [$e; $n] = unsafe { core::mem::transmute(*v) };
                    for x in l { mix(&mut hs[13 + i], canon(x, true)); }
                }
                for (i, v) in m.iter().enumerate() {
                    let l: [$mi; $n] = unsafe { core::mem::transmute(*v) };
                    for x in l { mix(&mut hs[15 + i], x as u64); }
                }
            }
            d!("{} {:016x?}", stringify!($v), hs);
        }
    };
}

ints!(t_i8x16, i8x16, m8x16, i8, 16);
ints!(t_u8x16, u8x16, mu8x16, u8, 16);
ints!(t_u8x8, u8x8, mu8x8, u8, 8);
ints!(t_i16x8, i16x8, m16x8, i16, 8);
ints!(t_u16x16, u16x16, mu16x16, u16, 16);
ints!(t_i32x4, i32x4, m32x4, i32, 4);
ints!(t_u32x2, u32x2, mu32x2, u32, 2);
ints!(t_u32x8, u32x8, mu32x8, u32, 8);
ints!(t_i64x2, i64x2, m64x2, i64, 2);
ints!(t_u64x4, u64x4, mu64x4, u64, 4);
floats!(t_f32x4, f32x4, mf32x4, f32, i32, 4);
floats!(t_f32x2, f32x2, mf32x2, f32, i32, 2);
floats!(t_f32x8, f32x8, mf32x8, f32, i32, 8);
floats!(t_f64x2, f64x2, mf64x2, f64, i64, 2);
floats!(t_f64x4, f64x4, mf64x4, f64, i64, 4);

fn work() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    t_i8x16(&mut rng);
    t_u8x16(&mut rng);
    t_u8x8(&mut rng);
    t_i16x8(&mut rng);
    t_u16x16(&mut rng);
    t_i32x4(&mut rng);
    t_u32x2(&mut rng);
    t_u32x8(&mut rng);
    t_i64x2(&mut rng);
    t_u64x4(&mut rng);
    t_f32x4(&mut rng);
    t_f32x2(&mut rng);
    t_f32x8(&mut rng);
    t_f64x2(&mut rng);
    t_f64x4(&mut rng);
}

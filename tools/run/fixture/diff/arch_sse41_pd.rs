// diff_fast corpus program: the SSE3 / SSE4.1 double-precision intrinsics of core::arch::x86_64 (addsub, hadd,
// hsub, movedup, loaddup, blend, blendv, dp, round/floor/ceil _pd and _sd) on the edge lanes of arch_sse2_pd
// (zeros, infinities, limits, subnormals, NaNs with payloads, rounding ties) and random bits, bit-exact, one
// hash per intrinsic (and immediate), `D <name> <hash>`; the MXCSR-direction rounding under all four modes.
// Runs where the CPU has SSE4.1 (`D no sse4.1` otherwise); x86-64 only. ARCH_DETAIL=<name> prints every case.
#![allow(unused_unsafe, deprecated, non_snake_case)]
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

#[cfg(not(target_arch = "x86_64"))]
fn work() {
    d!("sse2 pd: x86-64 only");
}

#[cfg(target_arch = "x86_64")]
fn work() {
    if !(std::is_x86_feature_detected!("sse3") && std::is_x86_feature_detected!("sse4.1")) {
        d!("no sse4.1");
        return;
    }
    unsafe { x::all() };
}

#[cfg(target_arch = "x86_64")]
mod x {
    use core::arch::x86_64::*;
    use std::hint::black_box;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    const EDGE64: [u64; 40] = [
        0x0000_0000_0000_0000, // 0.0
        0x8000_0000_0000_0000, // -0.0
        0x3ff0_0000_0000_0000, // 1.0
        0xbff0_0000_0000_0000, // -1.0
        0x3fe0_0000_0000_0000, // 0.5
        0xbfe0_0000_0000_0000, // -0.5
        0x3ff8_0000_0000_0000, // 1.5
        0x4004_0000_0000_0000, // 2.5
        0xc004_0000_0000_0000, // -2.5
        0x3fdf_ffff_ffff_ffff, // 0.49999999999999994
        0x7ff0_0000_0000_0000, // inf
        0xfff0_0000_0000_0000, // -inf
        0x7fef_ffff_ffff_ffff, // f64::MAX
        0xffef_ffff_ffff_ffff, // f64::MIN
        0x0010_0000_0000_0000, // MIN_POSITIVE
        0x0000_0000_0000_0001, // smallest subnormal
        0x800f_ffff_ffff_ffff, // largest negative subnormal
        0x41df_ffff_ffe0_0000, // 2147483647.5
        0x41e0_0000_0000_0000, // 2147483648.0
        0xc1e0_0000_0010_0000, // -2147483648.5
        0xc1e0_0000_0020_0000, // -2147483649.0
        0x41df_ffff_ffc0_0000, // 2147483647.0
        0xc1e0_0000_0000_0000, // -2147483648.0
        0x43e0_0000_0000_0000, // 9223372036854775808.0
        0xc3e0_0000_0000_0000, // -9223372036854775808.0
        0x43df_ffff_ffff_ffff, // 9223372036854774784.0
        0x47ef_ffff_e000_0000, // f32::MAX
        0x47ef_ffff_f000_0000, // f32::MAX + half ulp (rounds to inf in f32)
        0x3690_0000_0000_0000, // ~1.0e-45 range: f32 subnormal territory
        0x3ff0_0000_1000_0000, // 1 + 2^-24: an f32 rounding tie
        0x4330_0000_0000_0001, // 2^52 + 1
        0x7ff8_0000_0000_0000, // qNaN
        0xfff8_0000_0000_0000, // -qNaN (Intel default NaN)
        0x7ff0_0000_0000_0001, // sNaN
        0xfff4_0000_1234_5678, // -sNaN with payload
        0x7ff8_0000_1234_5678, // qNaN with payload
        0x7fff_ffff_ffff_ffff, // qNaN all payload bits
        0x4024_0000_0000_0000, // 10.0
        0xc059_0000_0000_0000, // -100.0
        0x3fb9_9999_9999_999a, // 0.1
    ];
    #[allow(dead_code)]
    const EDGE32: [u32; 20] = [
        0x0000_0000, 0x8000_0000, 0x3f80_0000, 0xbf80_0000, 0x3fc0_0000, 0x4020_0000, 0xc020_0000, 0x7f80_0000,
        0xff80_0000, 0x7f7f_ffff, 0x0000_0001, 0x4f00_0000, 0xcf00_0000, 0x4eff_ffff, 0xcf00_0001, 0x7fc0_0000,
        0xffc0_0000, 0x7f80_0001, 0xffa0_1234, 0x7fc0_5678,
    ];
    #[allow(dead_code)]
    const EDGEI32: [i32; 8] = [0, 1, -1, i32::MAX, i32::MIN, 16777217, -16777217, 123456789];
    #[allow(dead_code)]
    const EDGEI64: [i64; 8] = [0, 1, -1, i64::MAX, i64::MIN, 9007199254740993, -9007199254740993, 1234567890123456789];

    fn mix(h: &mut u64, x: u64) {
        *h = (*h ^ x).wrapping_mul(0x100000001b3).rotate_left(7);
    }
    fn pd(lo: u64, hi: u64) -> __m128d {
        black_box(unsafe { core::mem::transmute::<[u64; 2], __m128d>([lo, hi]) })
    }
    fn bits_pd(v: __m128d) -> [u64; 2] {
        unsafe { core::mem::transmute(v) }
    }

    /// One intrinsic's results: a hash, or every case when ARCH_DETAIL names it.
    struct Out {
        name: &'static str,
        h: u64,
        n: u64,
        detail: bool,
    }
    impl Out {
        fn new(name: &'static str) -> Out {
            let detail = std::env::var("ARCH_DETAIL").map(|v| v == name).unwrap_or(false);
            Out { name, h: 0xcbf29ce484222325, n: 0, detail }
        }
        fn put(&mut self, input: &[u64], r: [u64; 2]) {
            if self.detail {
                println!("D {} {:016x?} -> {:016x} {:016x}", self.name, input, r[1], r[0]);
            }
            mix(&mut self.h, r[0]);
            mix(&mut self.h, r[1]);
            self.n += 1;
        }
        fn end(self) {
            d!("{} n={} {:016x}", self.name, self.n, self.h);
        }
    }

    /// f64 lane values: every edge, then random bits and random finite values.
    fn lanes64(rng: &mut Rng) -> Vec<u64> {
        let mut v = EDGE64.to_vec();
        for _ in 0..24 {
            v.push(rng.next());
            let f = (rng.next() as i64 as f64) / ((rng.next() >> 40) as f64 + 1.0);
            v.push(f.to_bits());
        }
        v
    }

    macro_rules! bin {
        ($rng:ident, $($name:expr => |$a:ident, $b:ident| $e:expr;)*) => {$({
            #[inline(never)]
            #[target_feature(enable = "sse3,sse4.1")]
            unsafe fn op($a: __m128d, $b: __m128d) -> __m128d { $e }
            let mut o = Out::new($name);
            let v = lanes64($rng);
            for (i, &x) in v.iter().enumerate() {
                for (j, &y) in v.iter().enumerate() {
                    let (x2, y2) = (v[(i + 7) % v.len()], v[(j * 3 + 1) % v.len()]);
                    o.put(&[x, x2, y, y2], bits_pd(op(pd(x, x2), pd(y, y2))));
                }
            }
            o.end();
        })*};
    }
    macro_rules! un {
        ($rng:ident, $($name:expr => |$a:ident| $e:expr;)*) => {$({
            #[inline(never)]
            #[target_feature(enable = "sse3,sse4.1")]
            unsafe fn op($a: __m128d) -> __m128d { $e }
            let mut o = Out::new($name);
            let v = lanes64($rng);
            for (i, &x) in v.iter().enumerate() {
                let x2 = v[(i + 5) % v.len()];
                o.put(&[x, x2], bits_pd(op(pd(x, x2))));
            }
            o.end();
        })*};
    }

    #[target_feature(enable = "sse3,sse4.1")]
    pub unsafe fn all() {
        let rng = &mut Rng(0x2545_f491_4f6c_dd1d);
        bin!(rng,
            "_mm_addsub_pd" => |a, b| _mm_addsub_pd(a, b);
            "_mm_hadd_pd" => |a, b| _mm_hadd_pd(a, b);
            "_mm_hsub_pd" => |a, b| _mm_hsub_pd(a, b);
            "_mm_blend_pd::<0>" => |a, b| _mm_blend_pd::<0>(a, b);
            "_mm_blend_pd::<1>" => |a, b| _mm_blend_pd::<1>(a, b);
            "_mm_blend_pd::<2>" => |a, b| _mm_blend_pd::<2>(a, b);
            "_mm_blend_pd::<3>" => |a, b| _mm_blend_pd::<3>(a, b);
            "_mm_blendv_pd" => |a, b| _mm_blendv_pd(a, b, _mm_unpackhi_pd(b, a));
            "_mm_blendv_pd2" => |a, b| _mm_blendv_pd(b, a, _mm_mul_pd(a, b));
            "_mm_dp_pd::<0x31>" => |a, b| _mm_dp_pd::<0x31>(a, b);
            "_mm_dp_pd::<0x33>" => |a, b| _mm_dp_pd::<0x33>(a, b);
            "_mm_dp_pd::<0x12>" => |a, b| _mm_dp_pd::<0x12>(a, b);
            "_mm_dp_pd::<0x23>" => |a, b| _mm_dp_pd::<0x23>(a, b);
            "_mm_dp_pd::<0x00>" => |a, b| _mm_dp_pd::<0x00>(a, b);
            "_mm_dp_pd::<0xff>" => |a, b| _mm_dp_pd::<0xff>(a, b);
            "_mm_floor_sd" => |a, b| _mm_floor_sd(a, b);
            "_mm_ceil_sd" => |a, b| _mm_ceil_sd(a, b);
            "_mm_round_sd::<0>" => |a, b| _mm_round_sd::<0>(a, b);
            "_mm_round_sd::<9>" => |a, b| _mm_round_sd::<9>(a, b);
            "_mm_round_sd::<11>" => |a, b| _mm_round_sd::<11>(a, b);
        );
        un!(rng,
            "_mm_movedup_pd" => |a| _mm_movedup_pd(a);
            "_mm_floor_pd" => |a| _mm_floor_pd(a);
            "_mm_ceil_pd" => |a| _mm_ceil_pd(a);
            "_mm_round_pd::<0>" => |a| _mm_round_pd::<0>(a);
            "_mm_round_pd::<1>" => |a| _mm_round_pd::<1>(a);
            "_mm_round_pd::<2>" => |a| _mm_round_pd::<2>(a);
            "_mm_round_pd::<3>" => |a| _mm_round_pd::<3>(a);
            "_mm_round_pd::<8>" => |a| _mm_round_pd::<8>(a);
            "_mm_round_pd::<11>" => |a| _mm_round_pd::<11>(a);
        );
        {
            let mut o = Out::new("_mm_loaddup_pd");
            for &x in &lanes64(rng) {
                let m = [f64::from_bits(x), 3.0];
                o.put(&[x], bits_pd(_mm_loaddup_pd(black_box(m.as_ptr()))));
            }
            o.end();
        }
        let csr = _mm_getcsr();
        for rc in 0..4u32 {
            _mm_setcsr((csr & !0x6000) | rc << 13);
            un!(rng,
                "_mm_round_pd::<4> (MXCSR)" => |a| _mm_round_pd::<4>(a);
                "_mm_round_pd::<12> (MXCSR)" => |a| _mm_round_pd::<12>(a);
            );
            bin!(rng,
                "_mm_round_sd::<4> (MXCSR)" => |a, b| _mm_round_sd::<4>(a, b);
            );
            _mm_setcsr(csr);
        }
    }
}

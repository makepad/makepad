// diff_fast corpus program: the core::arch::x86_64 operations whose result follows the MXCSR rounding
// control (bits 13-14), under each of the four modes set with _mm_setcsr: f64 -> f32 and integer -> float
// conversions, float -> integer by the current mode, round with _MM_FROUND_CUR_DIRECTION, f32 -> f16
// (cvtps_ph by the current mode and by explicit modes, which must ignore MXCSR), sqrt, add / sub / mul /
// div and fused multiply-add. The inputs are made before any mode is set. Bit-exact, one hash per mode and
// intrinsic: `D <mode> <name> n=<cases> <hash>`. arch_sse2_pd / arch_sse41_pd cover the SSE2 f2i
// conversions and round_pd / round_sd under MXCSR; this file covers the rest. The AVX, FMA and F16C parts
// run where the CPU has them (`D no <feature>` otherwise); x86-64 only. ARCH_DETAIL=<name> prints every
// case of that intrinsic.
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
    d!("mxcsr: x86-64 only");
}

#[cfg(target_arch = "x86_64")]
fn work() {
    if !(std::is_x86_feature_detected!("sse3") && std::is_x86_feature_detected!("sse4.1")) {
        d!("no sse4.1");
        return;
    }
    let (avx, fma, f16c) = (std::is_x86_feature_detected!("avx"), std::is_x86_feature_detected!("fma"), std::is_x86_feature_detected!("f16c"));
    for (on, name) in [(avx, "avx"), (fma, "fma"), (f16c, "f16c")] {
        if !on {
            d!("no {name}");
        }
    }
    unsafe { x::all(avx, fma, f16c) };
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

    const EDGE64: [u64; 30] = [
        0x0000_0000_0000_0000, // 0.0
        0x8000_0000_0000_0000, // -0.0
        0x3ff0_0000_0000_0000, // 1.0
        0xbff0_0000_0000_0000, // -1.0
        0x3fe0_0000_0000_0000, // 0.5
        0x4004_0000_0000_0000, // 2.5
        0xc004_0000_0000_0000, // -2.5
        0x3fdf_ffff_ffff_ffff, // 0.49999999999999994
        0x7ff0_0000_0000_0000, // inf
        0xfff0_0000_0000_0000, // -inf
        0x7fef_ffff_ffff_ffff, // f64::MAX
        0x0000_0000_0000_0001, // smallest subnormal
        0x800f_ffff_ffff_ffff, // largest negative subnormal
        0x41df_ffff_ffe0_0000, // 2147483647.5
        0xc1e0_0000_0010_0000, // -2147483648.5
        0x47ef_ffff_e000_0000, // f32::MAX
        0x47ef_ffff_f000_0000, // f32::MAX + half ulp
        0xc7ef_ffff_f000_0001, // past -(f32::MAX + half ulp)
        0x36a0_0000_0000_0000, // 2^-149: the smallest f32 subnormal
        0x3690_0000_0000_0000, // 2^-150: half of it
        0xb698_0000_0000_0000, // -1.5 * 2^-150
        0x3ff0_0000_1000_0000, // 1 + 2^-24: an f32 tie
        0x3ff0_0000_1000_0001, // just past the tie
        0xbff0_0000_3000_0000, // -(1 + 3 * 2^-24): an odd tie
        0x3ff0_0000_0fff_ffff, // just below the tie
        0x4330_0000_0000_0001, // 2^52 + 1
        0x7ff8_0000_0000_0000, // qNaN
        0x7ff0_0000_0000_0001, // sNaN
        0xfff4_0000_1234_5678, // -sNaN with payload
        0x3fb9_9999_9999_999a, // 0.1
    ];
    const EDGE32: [u32; 22] = [
        0x0000_0000, 0x8000_0000, 0x3f80_0000, 0xbf80_0000, 0x4020_0000, 0xc020_0000, 0x7f80_0000, 0xff80_0000,
        0x7f7f_ffff, 0x0000_0001, 0x4eff_ffff, 0xcf00_0001, 0x7fc0_0000, 0x7f80_0001, 0xffa0_1234,
        0x3f80_1000, // 1 + 2^-11: an f16 tie
        0x3f80_3000, // 1 + 3 * 2^-11: an odd f16 tie
        0x477f_f000, // 65520: f16::MAX + half ulp
        0xc77f_efff, // just below -65520
        0x3380_0000, // 2^-24: the smallest f16 subnormal
        0x3300_0000, // 2^-25: half of it
        0x3dcc_cccd, // 0.1
    ];
    const EDGEI32: [i32; 10] = [0, 1, -1, i32::MAX, i32::MIN, 16777217, -16777217, 16777219, 123456789, -2147483583];
    const EDGEI64: [i64; 10] = [0, 1, -1, i64::MAX, i64::MIN, 9007199254740993, -9007199254740993, 9007199254740995, 1234567890123456789, -(1 << 62) - 1];

    fn mix(h: &mut u64, x: u64) {
        *h = (*h ^ x).wrapping_mul(0x100000001b3).rotate_left(7);
    }
    /// The 64-bit words of a 16- or 32-byte vector.
    fn words<T: Copy>(v: &T) -> &[u64] {
        unsafe { core::slice::from_raw_parts(v as *const T as *const u64, core::mem::size_of::<T>() / 8) }
    }
    fn cast<A: Copy, B: Copy>(a: A) -> B {
        assert_eq!(core::mem::size_of::<A>(), core::mem::size_of::<B>());
        black_box(unsafe { core::mem::transmute_copy(&a) })
    }

    /// One mode's results: a line per intrinsic, printed once MXCSR is back to its default.
    pub struct Out {
        mode: &'static str,
        lines: Vec<String>,
    }
    impl Out {
        fn run<T: Copy, R: Copy>(&mut self, name: &str, xs: &[T], f: impl Fn(T) -> R) {
            let detail = std::env::var("ARCH_DETAIL").map(|v| v == name).unwrap_or(false);
            let mut h = 0xcbf2_9ce4_8422_2325u64;
            for (i, &x) in xs.iter().enumerate() {
                let r = f(black_box(x));
                if detail {
                    self.lines.push(format!("{} {name} case {i} -> {:016x?}", self.mode, words(&r)));
                }
                for &w in words(&r) {
                    mix(&mut h, w);
                }
            }
            self.lines.push(format!("{} {name} n={} {h:016x}", self.mode, xs.len()));
        }
    }

    /// Each case: an `#[inline(never)]` function with the intrinsic's target features (so the native
    /// build cannot move it past the `_mm_setcsr` around it), run over one input list.
    macro_rules! cases {
        ($o:ident, $tf:literal, $xs:expr, |$x:ident: $t:ty| -> $r:ty { $($name:literal => $e:expr;)* }) => {$({
            #[inline(never)]
            #[target_feature(enable = $tf)]
            unsafe fn op($x: $t) -> $r { $e }
            $o.run($name, $xs, |x| unsafe { op(x) });
        })*};
    }

    /// The inputs, made under the default mode.
    pub struct In {
        pd: Vec<__m128d>,
        pd2: Vec<(__m128d, __m128d)>,
        pd3: Vec<(__m128d, __m128d, __m128d)>,
        ps: Vec<__m128>,
        ps2: Vec<(__m128, __m128)>,
        ps3: Vec<(__m128, __m128, __m128)>,
        ss_sd: Vec<(__m128, __m128d)>,
        sd_i64: Vec<(__m128d, i64)>,
        si: Vec<__m128i>,
        pd4: Vec<__m256d>,
        pd42: Vec<(__m256d, __m256d)>,
        pd43: Vec<(__m256d, __m256d, __m256d)>,
        ps8: Vec<__m256>,
        ps82: Vec<(__m256, __m256)>,
        ps83: Vec<(__m256, __m256, __m256)>,
        si8: Vec<__m256i>,
    }

    fn inputs() -> In {
        let rng = &mut Rng(0x9e37_79b9_7f4a_7c15);
        let mut f64s = EDGE64.to_vec();
        for _ in 0..24 {
            f64s.push(rng.next());
            f64s.push(((rng.next() as i64 as f64) / ((rng.next() >> 40) as f64 + 1.0)).to_bits());
            // near an f32 rounding boundary: the 29 bits below f32 precision are 0x0fff_ffff..0x1000_0001
            f64s.push((rng.next() & 0xc7ff_ffff_e000_0000) | 0x0fff_fffe | (rng.next() & 3));
        }
        let mut f32s = EDGE32.to_vec();
        for i in 0..f64s.len() {
            f32s.push((f64::from_bits(f64s[i]) as f32).to_bits());
        }
        for _ in 0..24 {
            let r = rng.next();
            f32s.push(r as u32);
            // f16 range and around it: exponents 2^-27 .. 2^17
            f32s.push(((r >> 32) as u32 & 0x807f_ffff) | ((100 + (r % 45) as u32) << 23));
        }
        let mut i32s = EDGEI32.to_vec();
        let mut i64s = EDGEI64.to_vec();
        for _ in 0..24 {
            let r = rng.next();
            i32s.push(r as i32);
            i32s.push((r >> 40) as i32 - (1 << 23));
            i64s.push(r as i64);
            i64s.push((r >> 8) as i64);
        }
        let (n64, n32, ni) = (f64s.len(), f32s.len(), i32s.len());
        let d = |i: usize| f64s[i % n64];
        let s = |i: usize| f32s[i % n32];
        let pd = |i: usize| -> __m128d { cast([d(i), d(i * 5 + 3)]) };
        let ps = |i: usize| -> __m128 { cast([s(i), s(i * 3 + 1), s(i * 7 + 2), s(i * 11 + 5)]) };
        let pd4 = |i: usize| -> __m256d { cast([d(i), d(i * 5 + 3), d(i * 7 + 1), d(i * 13 + 6)]) };
        let ps8 = |i: usize| -> __m256 { cast([s(i), s(i * 3 + 1), s(i * 7 + 2), s(i * 11 + 5), s(i * 2 + 9), s(i * 5 + 4), s(i * 13 + 8), s(i * 17 + 3)]) };
        let si = |i: usize| -> [i32; 8] { core::array::from_fn(|k| i32s[(i * (2 * k + 1) + k) % ni]) };
        In {
            pd: (0..n64).map(pd).collect(),
            pd2: (0..n64).map(|i| (pd(i), pd(i * 7 + 2))).collect(),
            pd3: (0..n64).map(|i| (pd(i), pd(i * 7 + 2), pd(i * 3 + 11))).collect(),
            ps: (0..n32).map(ps).collect(),
            ps2: (0..n32).map(|i| (ps(i), ps(i * 5 + 4))).collect(),
            ps3: (0..n32).map(|i| (ps(i), ps(i * 5 + 4), ps(i * 9 + 7))).collect(),
            ss_sd: (0..n64).map(|i| (ps(i), pd(i))).collect(),
            sd_i64: (0..i64s.len()).map(|i| (pd(i), i64s[i])).collect(),
            si: (0..ni).map(|i| cast::<[i32; 4], __m128i>(core::array::from_fn(|k| si(i)[k]))).collect(),
            pd4: (0..n64).map(pd4).collect(),
            pd42: (0..n64).map(|i| (pd4(i), pd4(i * 7 + 2))).collect(),
            pd43: (0..n64).map(|i| (pd4(i), pd4(i * 7 + 2), pd4(i * 3 + 11))).collect(),
            ps8: (0..n32).map(ps8).collect(),
            ps82: (0..n32).map(|i| (ps8(i), ps8(i * 5 + 4))).collect(),
            ps83: (0..n32).map(|i| (ps8(i), ps8(i * 5 + 4), ps8(i * 9 + 7))).collect(),
            si8: (0..ni).map(|i| cast(si(i))).collect(),
        }
    }

    fn sse(o: &mut Out, v: &In) {
        cases!(o, "sse2", &v.pd, |a: __m128d| -> __m128 {
            "_mm_cvtpd_ps" => _mm_cvtpd_ps(a);
        });
        cases!(o, "sse2", &v.ss_sd, |a: (__m128, __m128d)| -> __m128 {
            "_mm_cvtsd_ss" => _mm_cvtsd_ss(a.0, a.1);
        });
        cases!(o, "sse2", &v.si, |a: __m128i| -> __m128 {
            "_mm_cvtepi32_ps" => _mm_cvtepi32_ps(a);
        });
        cases!(o, "sse2", &v.sd_i64, |a: (__m128d, i64)| -> __m128d {
            "_mm_cvtsi64_sd" => _mm_cvtsi64_sd(a.0, a.1);
            "_mm_cvtsi64x_sd" => _mm_cvtsi64x_sd(a.0, a.1);
        });
        cases!(o, "sse2", &v.pd, |a: __m128d| -> __m128d {
            "_mm_sqrt_pd" => _mm_sqrt_pd(a);
        });
        cases!(o, "sse", &v.ps, |a: __m128| -> __m128 {
            "_mm_sqrt_ps" => _mm_sqrt_ps(a);
        });
        cases!(o, "sse4.1", &v.ps, |a: __m128| -> __m128 {
            "_mm_round_ps::<4>" => _mm_round_ps::<4>(a);
            "_mm_round_ps::<12>" => _mm_round_ps::<12>(a);
        });
        cases!(o, "sse2,sse3,sse4.1", &v.pd2, |a: (__m128d, __m128d)| -> __m128d {
            "_mm_sqrt_sd" => _mm_sqrt_sd(a.0, a.1);
            "_mm_add_pd" => _mm_add_pd(a.0, a.1);
            "_mm_sub_pd" => _mm_sub_pd(a.0, a.1);
            "_mm_mul_pd" => _mm_mul_pd(a.0, a.1);
            "_mm_div_pd" => _mm_div_pd(a.0, a.1);
            "_mm_add_sd" => _mm_add_sd(a.0, a.1);
            "_mm_sub_sd" => _mm_sub_sd(a.0, a.1);
            "_mm_mul_sd" => _mm_mul_sd(a.0, a.1);
            "_mm_div_sd" => _mm_div_sd(a.0, a.1);
            "_mm_addsub_pd" => _mm_addsub_pd(a.0, a.1);
            "_mm_hadd_pd" => _mm_hadd_pd(a.0, a.1);
            "_mm_dp_pd::<0x33>" => _mm_dp_pd::<0x33>(a.0, a.1);
        });
        cases!(o, "sse", &v.ps2, |a: (__m128, __m128)| -> __m128 {
            "_mm_add_ps" => _mm_add_ps(a.0, a.1);
            "_mm_sub_ps" => _mm_sub_ps(a.0, a.1);
            "_mm_mul_ps" => _mm_mul_ps(a.0, a.1);
            "_mm_div_ps" => _mm_div_ps(a.0, a.1);
            "_mm_add_ss" => _mm_add_ss(a.0, a.1);
        });
    }

    fn avx(o: &mut Out, v: &In) {
        cases!(o, "avx", &v.pd4, |a: __m256d| -> __m128 {
            "_mm256_cvtpd_ps" => _mm256_cvtpd_ps(a);
        });
        cases!(o, "avx", &v.pd4, |a: __m256d| -> __m128i {
            "_mm256_cvtpd_epi32" => _mm256_cvtpd_epi32(a);
        });
        cases!(o, "avx", &v.si8, |a: __m256i| -> __m256 {
            "_mm256_cvtepi32_ps" => _mm256_cvtepi32_ps(a);
        });
        cases!(o, "avx", &v.pd4, |a: __m256d| -> __m256d {
            "_mm256_sqrt_pd" => _mm256_sqrt_pd(a);
            "_mm256_round_pd::<4>" => _mm256_round_pd::<4>(a);
            "_mm256_round_pd::<12>" => _mm256_round_pd::<12>(a);
        });
        cases!(o, "avx", &v.ps8, |a: __m256| -> __m256 {
            "_mm256_sqrt_ps" => _mm256_sqrt_ps(a);
            "_mm256_round_ps::<4>" => _mm256_round_ps::<4>(a);
            "_mm256_round_ps::<12>" => _mm256_round_ps::<12>(a);
        });
        cases!(o, "avx", &v.ps82, |a: (__m256, __m256)| -> __m256 {
            "_mm256_add_ps" => _mm256_add_ps(a.0, a.1);
            "_mm256_sub_ps" => _mm256_sub_ps(a.0, a.1);
            "_mm256_mul_ps" => _mm256_mul_ps(a.0, a.1);
            "_mm256_div_ps" => _mm256_div_ps(a.0, a.1);
        });
        let _ = &v.pd42;
    }

    fn fma(o: &mut Out, v: &In) {
        cases!(o, "fma", &v.ps3, |a: (__m128, __m128, __m128)| -> __m128 {
            "_mm_fmadd_ps" => _mm_fmadd_ps(a.0, a.1, a.2);
            "_mm_fmsub_ps" => _mm_fmsub_ps(a.0, a.1, a.2);
            "_mm_fnmadd_ps" => _mm_fnmadd_ps(a.0, a.1, a.2);
        });
        cases!(o, "fma", &v.pd3, |a: (__m128d, __m128d, __m128d)| -> __m128d {
            "_mm_fmadd_pd" => _mm_fmadd_pd(a.0, a.1, a.2);
            "_mm_fmadd_sd" => _mm_fmadd_sd(a.0, a.1, a.2);
        });
        cases!(o, "fma", &v.ps83, |a: (__m256, __m256, __m256)| -> __m256 {
            "_mm256_fmadd_ps" => _mm256_fmadd_ps(a.0, a.1, a.2);
        });
        cases!(o, "fma", &v.pd43, |a: (__m256d, __m256d, __m256d)| -> __m256d {
            "_mm256_fmadd_pd" => _mm256_fmadd_pd(a.0, a.1, a.2);
            "_mm256_fmsub_pd" => _mm256_fmsub_pd(a.0, a.1, a.2);
        });
    }

    fn f16c(o: &mut Out, v: &In) {
        cases!(o, "f16c", &v.ps, |a: __m128| -> __m128i {
            "_mm_cvtps_ph::<4>" => _mm_cvtps_ph::<4>(a);
            "_mm_cvtps_ph::<0>" => _mm_cvtps_ph::<0>(a);
            "_mm_cvtps_ph::<1>" => _mm_cvtps_ph::<1>(a);
            "_mm_cvtps_ph::<2>" => _mm_cvtps_ph::<2>(a);
            "_mm_cvtps_ph::<3>" => _mm_cvtps_ph::<3>(a);
        });
        cases!(o, "f16c,avx", &v.ps8, |a: __m256| -> __m128i {
            "_mm256_cvtps_ph::<4>" => _mm256_cvtps_ph::<4>(a);
            "_mm256_cvtps_ph::<1>" => _mm256_cvtps_ph::<1>(a);
            "_mm256_cvtps_ph::<3>" => _mm256_cvtps_ph::<3>(a);
        });
    }

    pub unsafe fn all(has_avx: bool, has_fma: bool, has_f16c: bool) {
        let v = inputs();
        let csr = _mm_getcsr();
        for (mode, rc) in [("nearest", 0u32), ("down", 1), ("up", 2), ("zero", 3)] {
            let mut o = Out { mode, lines: Vec::new() };
            _mm_setcsr((csr & !0x6000) | rc << 13);
            sse(&mut o, &v);
            if has_avx {
                avx(&mut o, &v);
            }
            if has_fma {
                fma(&mut o, &v);
            }
            if has_f16c {
                f16c(&mut o, &v);
            }
            _mm_setcsr(csr);
            for l in o.lines {
                d!("{l}");
            }
        }
    }
}

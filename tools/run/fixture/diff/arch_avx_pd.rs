// diff_fast corpus program: the AVX / AVX2 / FMA double-precision intrinsics of core::arch::x86_64
// (_mm256_*_pd and the 128-bit AVX/FMA _pd/_sd forms: arithmetic, logic, compares with all 32 predicates,
// shuffles/permutes/blends with their immediates, broadcasts, masked loads/stores, gathers, casts, conversions,
// tests, fused multiply-adds) on the edge lanes of arch_sse2_pd and random bits, bit-exact, one hash per
// intrinsic (and immediate), `D <name> <hash>`. Runs where the CPU has AVX2 + FMA (`D no avx2/fma` otherwise);
// x86-64 only. ARCH_DETAIL=<name> prints every case.
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
    if !(std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma")) {
        d!("no avx2/fma");
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
    fn pd4(l: [u64; 4]) -> __m256d {
        black_box(unsafe { core::mem::transmute::<[u64; 4], __m256d>(l) })
    }
    fn bits4(v: __m256d) -> [[u64; 2]; 2] {
        unsafe { core::mem::transmute(v) }
    }
    fn si4(l: [u64; 4]) -> __m256i {
        black_box(unsafe { core::mem::transmute::<[u64; 4], __m256i>(l) })
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
        fn put4(&mut self, input: &[u64], r: __m256d) {
            let [a, b] = bits4(r);
            self.put(input, a);
            self.put(input, b);
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

    /// at most one NaN operand per lane for the fused ops: with several, the NaN rustc returns depends on the
    /// FMA instruction form LLVM picks (operand order), which Rust leaves unspecified
    fn one_nan(a: &mut [u64], b: &mut [u64], c: &mut [u64]) {
        let nan = |x: u64| f64::from_bits(x).is_nan();
        for i in 0..a.len() {
            if nan(a[i]) {
                if nan(b[i]) { b[i] = 0x3ff8_0000_0000_0000; }
                if nan(c[i]) { c[i] = 0xc004_0000_0000_0000; }
            } else if nan(b[i]) && nan(c[i]) {
                c[i] = 0x4024_0000_0000_0000;
            }
        }
    }
    // lane values of a 4-lane case k: edge pairs walked with different strides
    fn four(v: &[u64], i: usize, j: usize, s: usize) -> [u64; 4] {
        let n = v.len();
        [v[i % n], v[(i + s) % n], v[j % n], v[(j * 3 + s) % n]]
    }
    macro_rules! bin4 {
        ($rng:ident, $($name:expr => |$a:ident, $b:ident| $e:expr;)*) => {$({
            #[inline(never)]
            #[target_feature(enable = "avx,avx2,fma")]
            unsafe fn op($a: __m256d, $b: __m256d) -> __m256d { $e }
            let mut o = Out::new($name);
            let v = lanes64($rng);
            for i in 0..v.len() {
                for j in 0..v.len() {
                    let (x, y) = (four(&v, i, j, 5), four(&v, j, i, 11));
                    o.put4(&[x[0], y[0]], op(pd4(x), pd4(y)));
                }
            }
            o.end();
        })*};
    }
    macro_rules! un4 {
        ($rng:ident, $($name:expr => |$a:ident| $e:expr;)*) => {$({
            #[inline(never)]
            #[target_feature(enable = "avx,avx2,fma")]
            unsafe fn op($a: __m256d) -> __m256d { $e }
            let mut o = Out::new($name);
            let v = lanes64($rng);
            for i in 0..v.len() {
                for j in 0..8 {
                    let x = four(&v, i, j * 5 + i, 3);
                    o.put4(&[x[0]], op(pd4(x)));
                }
            }
            o.end();
        })*};
    }
    // any result type, from two 256-bit inputs, hashed through its bits
    macro_rules! any4 {
        ($rng:ident, $($name:expr => |$a:ident, $b:ident| -> $t:ty { $e:expr };)*) => {$({
            #[inline(never)]
            #[target_feature(enable = "avx,avx2,fma")]
            unsafe fn op($a: __m256d, $b: __m256d) -> $t { $e }
            let mut o = Out::new($name);
            let v = lanes64($rng);
            for i in 0..v.len() {
                for j in 0..v.len() {
                    let (x, y) = (four(&v, i, j, 5), four(&v, j, i, 11));
                    let r = op(pd4(x), pd4(y));
                    let mut w = [0u64; 4];
                    unsafe { core::ptr::copy_nonoverlapping(&r as *const $t as *const u8, w.as_mut_ptr() as *mut u8, core::mem::size_of::<$t>()) };
                    o.put(&[x[0], y[0]], [w[0] ^ w[2].rotate_left(13), w[1] ^ w[3].rotate_left(29)]);
                }
            }
            o.end();
        })*};
    }
    macro_rules! fma3 {
        ($rng:ident, $($name:expr => |$a:ident, $b:ident, $c:ident| $e:expr;)*) => {$({
            #[inline(never)]
            #[target_feature(enable = "avx,avx2,fma")]
            unsafe fn op($a: __m256d, $b: __m256d, $c: __m256d) -> __m256d { $e }
            let mut o = Out::new($name);
            let v = lanes64($rng);
            for i in 0..v.len() {
                for j in 0..v.len() {
                    let (mut x, mut y, mut z) = (four(&v, i, j, 5), four(&v, j, i, 11), four(&v, i + j, i * 7, 17));
                    one_nan(&mut x, &mut y, &mut z);
                    o.put4(&[x[0], y[0], z[0]], op(pd4(x), pd4(y), pd4(z)));
                }
            }
            o.end();
        })*};
    }
    macro_rules! fma2 {
        ($rng:ident, $($name:expr => |$a:ident, $b:ident, $c:ident| $e:expr;)*) => {$({
            #[inline(never)]
            #[target_feature(enable = "avx,avx2,fma")]
            unsafe fn op($a: __m128d, $b: __m128d, $c: __m128d) -> __m128d { $e }
            let mut o = Out::new($name);
            let v = lanes64($rng);
            for i in 0..v.len() {
                for j in 0..v.len() {
                    let (mut x, mut y, mut z) = (four(&v, i, j, 5), four(&v, j, i, 11), four(&v, i + j, i * 7, 17));
                    one_nan(&mut x[..2], &mut y[..2], &mut z[..2]);
                    o.put(&[x[0], y[0], z[0]], bits_pd(op(pd(x[0], x[1]), pd(y[0], y[1]), pd(z[0], z[1]))));
                }
            }
            o.end();
        })*};
    }
    // a 128-bit pair -> 128-bit
    macro_rules! bin2 {
        ($rng:ident, $($name:expr => |$a:ident, $b:ident| $e:expr;)*) => {$({
            #[inline(never)]
            #[target_feature(enable = "avx,avx2,fma")]
            unsafe fn op($a: __m128d, $b: __m128d) -> __m128d { $e }
            let mut o = Out::new($name);
            let v = lanes64($rng);
            for i in 0..v.len() {
                for j in 0..v.len() {
                    let (x, y) = (four(&v, i, j, 5), four(&v, j, i, 11));
                    o.put(&[x[0], y[0]], bits_pd(op(pd(x[0], x[1]), pd(y[0], y[1]))));
                }
            }
            o.end();
        })*};
    }

    #[target_feature(enable = "avx,avx2,fma")]
    pub unsafe fn all() {
        let rng = &mut Rng(0x0123_4567_89ab_cdef);
        bin4!(rng,
            "_mm256_add_pd" => |a, b| _mm256_add_pd(a, b);
            "_mm256_sub_pd" => |a, b| _mm256_sub_pd(a, b);
            "_mm256_mul_pd" => |a, b| _mm256_mul_pd(a, b);
            "_mm256_div_pd" => |a, b| _mm256_div_pd(a, b);
            "_mm256_min_pd" => |a, b| _mm256_min_pd(a, b);
            "_mm256_max_pd" => |a, b| _mm256_max_pd(a, b);
            "_mm256_and_pd" => |a, b| _mm256_and_pd(a, b);
            "_mm256_andnot_pd" => |a, b| _mm256_andnot_pd(a, b);
            "_mm256_or_pd" => |a, b| _mm256_or_pd(a, b);
            "_mm256_xor_pd" => |a, b| _mm256_xor_pd(a, b);
            "_mm256_addsub_pd" => |a, b| _mm256_addsub_pd(a, b);
            "_mm256_hadd_pd" => |a, b| _mm256_hadd_pd(a, b);
            "_mm256_hsub_pd" => |a, b| _mm256_hsub_pd(a, b);
            "_mm256_unpackhi_pd" => |a, b| _mm256_unpackhi_pd(a, b);
            "_mm256_unpacklo_pd" => |a, b| _mm256_unpacklo_pd(a, b);
            "_mm256_shuffle_pd::<0>" => |a, b| _mm256_shuffle_pd::<0>(a, b);
            "_mm256_shuffle_pd::<5>" => |a, b| _mm256_shuffle_pd::<5>(a, b);
            "_mm256_shuffle_pd::<10>" => |a, b| _mm256_shuffle_pd::<10>(a, b);
            "_mm256_shuffle_pd::<15>" => |a, b| _mm256_shuffle_pd::<15>(a, b);
            "_mm256_blend_pd::<0>" => |a, b| _mm256_blend_pd::<0>(a, b);
            "_mm256_blend_pd::<6>" => |a, b| _mm256_blend_pd::<6>(a, b);
            "_mm256_blend_pd::<9>" => |a, b| _mm256_blend_pd::<9>(a, b);
            "_mm256_blend_pd::<15>" => |a, b| _mm256_blend_pd::<15>(a, b);
            "_mm256_blendv_pd" => |a, b| _mm256_blendv_pd(a, b, _mm256_mul_pd(a, b));
            "_mm256_permute2f128_pd::<0x20>" => |a, b| _mm256_permute2f128_pd::<0x20>(a, b);
            "_mm256_permute2f128_pd::<0x31>" => |a, b| _mm256_permute2f128_pd::<0x31>(a, b);
            "_mm256_permute2f128_pd::<0x83>" => |a, b| _mm256_permute2f128_pd::<0x83>(a, b);
            "_mm256_permute2f128_pd::<0x12>" => |a, b| _mm256_permute2f128_pd::<0x12>(a, b);
            "_mm256_permutevar_pd" => |a, b| _mm256_permutevar_pd(a, _mm256_castpd_si256(b));
            "_mm256_cmp_pd::<0>" => |a, b| _mm256_cmp_pd::<0>(a, b);
            "_mm256_cmp_pd::<1>" => |a, b| _mm256_cmp_pd::<1>(a, b);
            "_mm256_cmp_pd::<2>" => |a, b| _mm256_cmp_pd::<2>(a, b);
            "_mm256_cmp_pd::<3>" => |a, b| _mm256_cmp_pd::<3>(a, b);
            "_mm256_cmp_pd::<4>" => |a, b| _mm256_cmp_pd::<4>(a, b);
            "_mm256_cmp_pd::<5>" => |a, b| _mm256_cmp_pd::<5>(a, b);
            "_mm256_cmp_pd::<6>" => |a, b| _mm256_cmp_pd::<6>(a, b);
            "_mm256_cmp_pd::<7>" => |a, b| _mm256_cmp_pd::<7>(a, b);
            "_mm256_cmp_pd::<8>" => |a, b| _mm256_cmp_pd::<8>(a, b);
            "_mm256_cmp_pd::<9>" => |a, b| _mm256_cmp_pd::<9>(a, b);
            "_mm256_cmp_pd::<10>" => |a, b| _mm256_cmp_pd::<10>(a, b);
            "_mm256_cmp_pd::<11>" => |a, b| _mm256_cmp_pd::<11>(a, b);
            "_mm256_cmp_pd::<12>" => |a, b| _mm256_cmp_pd::<12>(a, b);
            "_mm256_cmp_pd::<13>" => |a, b| _mm256_cmp_pd::<13>(a, b);
            "_mm256_cmp_pd::<14>" => |a, b| _mm256_cmp_pd::<14>(a, b);
            "_mm256_cmp_pd::<15>" => |a, b| _mm256_cmp_pd::<15>(a, b);
            "_mm256_cmp_pd::<16>" => |a, b| _mm256_cmp_pd::<16>(a, b);
            "_mm256_cmp_pd::<24>" => |a, b| _mm256_cmp_pd::<24>(a, b);
            "_mm256_cmp_pd::<28>" => |a, b| _mm256_cmp_pd::<28>(a, b);
            "_mm256_cmp_pd::<31>" => |a, b| _mm256_cmp_pd::<31>(a, b);
            "_mm256_insertf128_pd::<0>" => |a, b| _mm256_insertf128_pd::<0>(a, _mm256_castpd256_pd128(b));
            "_mm256_insertf128_pd::<1>" => |a, b| _mm256_insertf128_pd::<1>(a, _mm256_extractf128_pd::<1>(b));
        );
        un4!(rng,
            "_mm256_sqrt_pd" => |a| _mm256_sqrt_pd(a);
            "_mm256_movedup_pd" => |a| _mm256_movedup_pd(a);
            "_mm256_floor_pd" => |a| _mm256_floor_pd(a);
            "_mm256_ceil_pd" => |a| _mm256_ceil_pd(a);
            "_mm256_round_pd::<0>" => |a| _mm256_round_pd::<0>(a);
            "_mm256_round_pd::<1>" => |a| _mm256_round_pd::<1>(a);
            "_mm256_round_pd::<2>" => |a| _mm256_round_pd::<2>(a);
            "_mm256_round_pd::<3>" => |a| _mm256_round_pd::<3>(a);
            "_mm256_round_pd::<4>" => |a| _mm256_round_pd::<4>(a);
            "_mm256_permute_pd::<0>" => |a| _mm256_permute_pd::<0>(a);
            "_mm256_permute_pd::<5>" => |a| _mm256_permute_pd::<5>(a);
            "_mm256_permute_pd::<9>" => |a| _mm256_permute_pd::<9>(a);
            "_mm256_permute_pd::<15>" => |a| _mm256_permute_pd::<15>(a);
            "_mm256_permute4x64_pd::<0x1b>" => |a| _mm256_permute4x64_pd::<0x1b>(a);
            "_mm256_permute4x64_pd::<0xe4>" => |a| _mm256_permute4x64_pd::<0xe4>(a);
            "_mm256_permute4x64_pd::<0x4e>" => |a| _mm256_permute4x64_pd::<0x4e>(a);
            "_mm256_permute4x64_pd::<0x00>" => |a| _mm256_permute4x64_pd::<0x00>(a);
            "_mm256_permute4x64_pd::<0xb1>" => |a| _mm256_permute4x64_pd::<0xb1>(a);
            "_mm256_cvtps_pd" => |a| _mm256_cvtps_pd(_mm256_castps256_ps128(_mm256_castpd_ps(a)));
            "_mm256_cvtepi32_pd" => |a| _mm256_cvtepi32_pd(_mm256_castsi256_si128(_mm256_castpd_si256(a)));
            "_mm256_castpd128_pd256" => |a| _mm256_castpd128_pd256(_mm256_castpd256_pd128(a));
            "_mm256_zextpd128_pd256" => |a| _mm256_zextpd128_pd256(_mm256_extractf128_pd::<1>(a));
            "_mm256_broadcastsd_pd" => |a| _mm256_broadcastsd_pd(_mm256_extractf128_pd::<1>(a));
            "_mm256_castps_pd" => |a| _mm256_castps_pd(_mm256_castpd_ps(a));
            "_mm256_castsi256_pd" => |a| _mm256_castsi256_pd(_mm256_castpd_si256(a));
            "_mm256_undefined_pd/setzero" => |a| _mm256_or_pd(_mm256_setzero_pd(), _mm256_and_pd(a, _mm256_undefined_pd()));
        );
        any4!(rng,
            "_mm256_movemask_pd" => |a, b| -> i32 { _mm256_movemask_pd(_mm256_xor_pd(a, b)) };
            "_mm256_testz_pd" => |a, b| -> i32 { _mm256_testz_pd(a, b) };
            "_mm256_testc_pd" => |a, b| -> i32 { _mm256_testc_pd(a, b) };
            "_mm256_testnzc_pd" => |a, b| -> i32 { _mm256_testnzc_pd(a, b) };
            "_mm_testz_pd" => |a, b| -> i32 { _mm_testz_pd(_mm256_castpd256_pd128(a), _mm256_castpd256_pd128(b)) };
            "_mm_testc_pd" => |a, b| -> i32 { _mm_testc_pd(_mm256_castpd256_pd128(a), _mm256_castpd256_pd128(b)) };
            "_mm_testnzc_pd" => |a, b| -> i32 { _mm_testnzc_pd(_mm256_castpd256_pd128(a), _mm256_castpd256_pd128(b)) };
            "_mm256_cvtpd_ps" => |a, b| -> __m128 { _mm256_cvtpd_ps(a) };
            "_mm256_cvtpd_epi32" => |a, b| -> __m128i { _mm256_cvtpd_epi32(a) };
            "_mm256_cvttpd_epi32" => |a, b| -> __m128i { _mm256_cvttpd_epi32(b) };
            "_mm256_extractf128_pd::<0>" => |a, b| -> __m128d { _mm256_extractf128_pd::<0>(a) };
            "_mm256_extractf128_pd::<1>" => |a, b| -> __m128d { _mm256_extractf128_pd::<1>(a) };
            "_mm256_castpd_ps/si256" => |a, b| -> __m256i { _mm256_castps_si256(_mm256_castpd_ps(a)) };
        );
        bin2!(rng,
            "_mm_permute_pd::<0>" => |a, b| _mm_permute_pd::<0>(a);
            "_mm_permute_pd::<1>" => |a, b| _mm_permute_pd::<1>(a);
            "_mm_permute_pd::<2>" => |a, b| _mm_permute_pd::<2>(b);
            "_mm_permute_pd::<3>" => |a, b| _mm_permute_pd::<3>(b);
            "_mm_permutevar_pd" => |a, b| _mm_permutevar_pd(a, _mm_castpd_si128(b));
            "_mm_broadcastsd_pd" => |a, b| _mm_broadcastsd_pd(a);
            "_mm_cmp_pd::<0>" => |a, b| _mm_cmp_pd::<0>(a, b);
            "_mm_cmp_pd::<3>" => |a, b| _mm_cmp_pd::<3>(a, b);
            "_mm_cmp_pd::<8>" => |a, b| _mm_cmp_pd::<8>(a, b);
            "_mm_cmp_pd::<12>" => |a, b| _mm_cmp_pd::<12>(a, b);
            "_mm_cmp_pd::<15>" => |a, b| _mm_cmp_pd::<15>(a, b);
            "_mm_cmp_pd::<21>" => |a, b| _mm_cmp_pd::<21>(a, b);
            "_mm_cmp_sd::<1>" => |a, b| _mm_cmp_sd::<1>(a, b);
            "_mm_cmp_sd::<8>" => |a, b| _mm_cmp_sd::<8>(a, b);
            "_mm_cmp_sd::<12>" => |a, b| _mm_cmp_sd::<12>(a, b);
            "_mm_cmp_sd::<30>" => |a, b| _mm_cmp_sd::<30>(a, b);
        );
        fma2!(rng,
            "_mm_fmadd_pd" => |a, b, c| _mm_fmadd_pd(a, b, c);
            "_mm_fmsub_pd" => |a, b, c| _mm_fmsub_pd(a, b, c);
            "_mm_fnmadd_pd" => |a, b, c| _mm_fnmadd_pd(a, b, c);
            "_mm_fnmsub_pd" => |a, b, c| _mm_fnmsub_pd(a, b, c);
            "_mm_fmaddsub_pd" => |a, b, c| _mm_fmaddsub_pd(a, b, c);
            "_mm_fmsubadd_pd" => |a, b, c| _mm_fmsubadd_pd(a, b, c);
            "_mm_fmadd_sd" => |a, b, c| _mm_fmadd_sd(a, b, c);
            "_mm_fmsub_sd" => |a, b, c| _mm_fmsub_sd(a, b, c);
            "_mm_fnmadd_sd" => |a, b, c| _mm_fnmadd_sd(a, b, c);
            "_mm_fnmsub_sd" => |a, b, c| _mm_fnmsub_sd(a, b, c);
        );
        fma3!(rng,
            "_mm256_fmadd_pd" => |a, b, c| _mm256_fmadd_pd(a, b, c);
            "_mm256_fmsub_pd" => |a, b, c| _mm256_fmsub_pd(a, b, c);
            "_mm256_fnmadd_pd" => |a, b, c| _mm256_fnmadd_pd(a, b, c);
            "_mm256_fnmsub_pd" => |a, b, c| _mm256_fnmsub_pd(a, b, c);
            "_mm256_fmaddsub_pd" => |a, b, c| _mm256_fmaddsub_pd(a, b, c);
            "_mm256_fmsubadd_pd" => |a, b, c| _mm256_fmsubadd_pd(a, b, c);
        );
        memory(rng);
    }

    #[inline(never)]
    #[target_feature(enable = "avx,avx2,fma")]
    unsafe fn memory(rng: &mut Rng) {
        let v = lanes64(rng);
        let mut o = Out::new("256 loads/sets/broadcasts");
        for w in v.windows(5) {
            #[repr(C, align(32))]
            struct A([f64; 5]);
            let m = A(core::array::from_fn(|k| f64::from_bits(w[k])));
            let p = black_box(m.0.as_ptr());
            let h = pd(w[3], w[4]);
            let (x, y, z, t) = (black_box(m.0[0]), black_box(m.0[1]), black_box(m.0[2]), black_box(m.0[3]));
            for r in [
                _mm256_load_pd(p),
                _mm256_loadu_pd(p.add(1)),
                _mm256_broadcast_sd(&*p.add(2)),
                _mm256_broadcast_pd(&h),
                _mm256_set1_pd(x),
                _mm256_set_pd(x, y, z, t),
                _mm256_setr_pd(x, y, z, t),
                _mm256_setzero_pd(),
            ] {
                o.put4(w, r);
            }
            o.put(w, bits_pd(_mm_broadcastsd_pd(h)));
        }
        o.end();
        let mut o = Out::new("256 stores");
        for w in v.windows(4) {
            #[repr(C, align(32))]
            struct A([u64; 6]);
            let a = pd4([w[0], w[1], w[2], w[3]]);
            for k in 0..3 {
                let mut m = A([0x1111_1111_1111_1111; 6]);
                let p = black_box(m.0.as_mut_ptr() as *mut f64);
                match k {
                    0 => _mm256_store_pd(p, a),
                    1 => _mm256_storeu_pd(p.add(1), a),
                    _ => _mm256_stream_pd(p, a),
                }
                let m = black_box(m);
                o.put(w, [m.0[0] ^ m.0[1].rotate_left(7) ^ m.0[2].rotate_left(19), m.0[3] ^ m.0[4].rotate_left(11) ^ m.0[5].rotate_left(23)]);
            }
        }
        o.end();
        let mut o = Out::new("masked loads/stores");
        for (i, w) in v.windows(4).enumerate() {
            let src = [f64::from_bits(w[0]), f64::from_bits(w[1]), f64::from_bits(w[2]), f64::from_bits(w[3])];
            let r = rng.next();
            let mk = |k: u32| if r >> k & 1 != 0 { -1i64 - (r as i64 & 0xff) } else { r as i64 & 0x7fff_ffff };
            let m4 = si4([mk(0) as u64, mk(1) as u64, mk(2) as u64, mk(3) as u64]);
            let m2 = _mm256_castsi256_si128(m4);
            o.put4(w, _mm256_maskload_pd(black_box(src.as_ptr()), m4));
            o.put(w, bits_pd(_mm_maskload_pd(black_box(src.as_ptr()), m2)));
            let mut dst = [7.0f64; 4];
            _mm256_maskstore_pd(black_box(dst.as_mut_ptr()), m4, pd4([w[3], w[2], w[1], w[0]]));
            _mm_maskstore_pd(black_box(dst.as_mut_ptr().add(2)), m2, pd(w[(i + 1) % 4], w[0]));
            o.put(w, [dst[0].to_bits() ^ dst[1].to_bits().rotate_left(9), dst[2].to_bits() ^ dst[3].to_bits().rotate_left(21)]);
        }
        o.end();
        let mut o = Out::new("gathers");
        let table: Vec<f64> = v.iter().map(|&x| f64::from_bits(x)).collect();
        let base = black_box(table.as_ptr().add(20));
        for _ in 0..64 {
            let r = rng.next();
            let off = |k: u32| ((r >> (8 * k)) & 31) as i64 - 16;
            let o32 = _mm_setr_epi32(off(0) as i32, off(1) as i32, off(2) as i32, off(3) as i32);
            let o64 = si4([off(0) as u64, off(1) as u64, off(2) as u64, off(3) as u64]);
            let o64h = _mm256_castsi256_si128(o64);
            let msk = pd4([r << 3, r << 2, r << 1, r]);
            let src = pd4([rng.next(), 2, 3, 4]);
            o.put(&[r], bits_pd(_mm_i32gather_pd::<8>(base, o32)));
            o.put(&[r], bits_pd(_mm_i64gather_pd::<8>(base, o64h)));
            o.put(&[r], bits_pd(_mm_mask_i32gather_pd::<8>(_mm256_castpd256_pd128(src), base, o32, _mm256_castpd256_pd128(msk))));
            o.put(&[r], bits_pd(_mm_mask_i64gather_pd::<8>(_mm256_castpd256_pd128(src), base, o64h, _mm256_castpd256_pd128(msk))));
            o.put4(&[r], _mm256_i32gather_pd::<8>(base, o32));
            o.put4(&[r], _mm256_i64gather_pd::<8>(base, o64));
            o.put4(&[r], _mm256_mask_i32gather_pd::<8>(src, base, o32, msk));
            o.put4(&[r], _mm256_mask_i64gather_pd::<8>(src, base, o64, msk));
            // byte offsets with scale 1 (multiples of 8) and scale 4 (even)
            let ob = _mm_setr_epi32(off(0) as i32 * 8, off(1) as i32 * 8, 0, 0);
            let oq = _mm_setr_epi32(off(2) as i32 * 2, off(3) as i32 * 2, 0, 0);
            o.put(&[r], bits_pd(_mm_i32gather_pd::<1>(base, ob)));
            o.put(&[r], bits_pd(_mm_i32gather_pd::<4>(base, oq)));
        }
        o.end();
    }
}

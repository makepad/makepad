// diff_fast corpus program: the SSE2 double-precision intrinsics of core::arch::x86_64 (_pd / _sd, plus the
// pd/ps/epi32 conversions and casts) on edge lanes (both zeros, infinities, limits, subnormals, quiet and
// signaling NaNs of both signs with payloads, conversion boundaries) and random bits, bit-exact: every
// result's bits (NaN payloads, compare masks, -0.0) feed one hash per intrinsic, printed as `D <name> <hash>`.
// Float -> integer conversions also run under each MXCSR rounding mode. x86-64 only (other hosts print one
// line). ARCH_DETAIL=<name> prints that intrinsic's every case instead (both sides), for a diverging hash.
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
    x::all();
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
    const EDGE32: [u32; 20] = [
        0x0000_0000, 0x8000_0000, 0x3f80_0000, 0xbf80_0000, 0x3fc0_0000, 0x4020_0000, 0xc020_0000, 0x7f80_0000,
        0xff80_0000, 0x7f7f_ffff, 0x0000_0001, 0x4f00_0000, 0xcf00_0000, 0x4eff_ffff, 0xcf00_0001, 0x7fc0_0000,
        0xffc0_0000, 0x7f80_0001, 0xffa0_1234, 0x7fc0_5678,
    ];
    const EDGEI32: [i32; 8] = [0, 1, -1, i32::MAX, i32::MIN, 16777217, -16777217, 123456789];
    const EDGEI64: [i64; 8] = [0, 1, -1, i64::MAX, i64::MIN, 9007199254740993, -9007199254740993, 1234567890123456789];

    fn mix(h: &mut u64, x: u64) {
        *h = (*h ^ x).wrapping_mul(0x100000001b3).rotate_left(7);
    }
    fn pd(lo: u64, hi: u64) -> __m128d {
        black_box(unsafe { core::mem::transmute::<[u64; 2], __m128d>([lo, hi]) })
    }
    fn ps(l: [u32; 4]) -> __m128 {
        black_box(unsafe { core::mem::transmute::<[u32; 4], __m128>(l) })
    }
    fn si(l: [u32; 4]) -> __m128i {
        black_box(unsafe { core::mem::transmute::<[u32; 4], __m128i>(l) })
    }
    fn bits_pd(v: __m128d) -> [u64; 2] {
        unsafe { core::mem::transmute(v) }
    }
    fn bits_ps(v: __m128) -> [u64; 2] {
        unsafe { core::mem::transmute(v) }
    }
    fn bits_si(v: __m128i) -> [u64; 2] {
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

    // ---- packed f64 x f64 -> f64 (and masks): every (x, y) pair of lane values, both lanes
    macro_rules! bin_pd {
        ($rng:ident, $($f:ident)*) => {$({
            #[inline(never)]
            fn op(a: __m128d, b: __m128d) -> __m128d { unsafe { $f(a, b) } }
            let mut o = Out::new(stringify!($f));
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
    macro_rules! un_pd {
        ($rng:ident, $($f:ident)*) => {$({
            #[inline(never)]
            fn op(a: __m128d) -> __m128d { unsafe { $f(a) } }
            let mut o = Out::new(stringify!($f));
            let v = lanes64($rng);
            for (i, &x) in v.iter().enumerate() {
                let x2 = v[(i + 5) % v.len()];
                o.put(&[x, x2], bits_pd(op(pd(x, x2))));
            }
            o.end();
        })*};
    }
    // f64 pair -> i32 (comi, movemask)
    macro_rules! int_pd {
        ($rng:ident, $($f:ident)*) => {$({
            #[inline(never)]
            fn op(a: __m128d, b: __m128d) -> i32 { unsafe { $f(a, b) } }
            let mut o = Out::new(stringify!($f));
            let v = lanes64($rng);
            for (i, &x) in v.iter().enumerate() {
                for &y in v.iter() {
                    let x2 = v[(i + 3) % v.len()];
                    o.put(&[x, x2, y], [op(pd(x, x2), pd(y, x)) as u32 as u64, 0]);
                }
            }
            o.end();
        })*};
    }

    #[inline(never)]
    fn movemask(a: __m128d) -> i32 {
        unsafe { _mm_movemask_pd(a) }
    }
    #[inline(never)]
    fn shuffles(rng: &mut Rng) {
        macro_rules! sh {
            ($($k:literal)*) => {$({
                #[inline(never)]
                fn op(a: __m128d, b: __m128d) -> __m128d { unsafe { _mm_shuffle_pd::<$k>(a, b) } }
                let mut o = Out::new(concat!("_mm_shuffle_pd::<", stringify!($k), ">"));
                for _ in 0..16 {
                    let (a, b, c, e) = (rng.next(), rng.next(), rng.next(), rng.next());
                    o.put(&[a, b, c, e], bits_pd(op(pd(a, b), pd(c, e))));
                }
                o.end();
            })*};
        }
        sh!(0 1 2 3 4 255);
    }

    #[inline(never)]
    fn memory(rng: &mut Rng) {
        let v = lanes64(rng);
        let mut o = Out::new("loads");
        for w in v.windows(3) {
            #[repr(C, align(16))]
            struct A([f64; 3]);
            let m = A([f64::from_bits(w[0]), f64::from_bits(w[1]), f64::from_bits(w[2])]);
            let p = black_box(m.0.as_ptr());
            let a = pd(w[2], w[1]);
            unsafe {
                for r in [
                    _mm_load_pd(p),
                    _mm_loadu_pd(p.add(1)),
                    _mm_load_sd(p.add(1)),
                    _mm_load1_pd(p),
                    _mm_load_pd1(p.add(2)),
                    _mm_loadr_pd(p),
                    _mm_loadh_pd(a, p.add(1)),
                    _mm_loadl_pd(a, p.add(2)),
                ] {
                    o.put(w, bits_pd(r));
                }
            }
        }
        o.end();
        let mut o = Out::new("stores");
        for w in v.windows(2) {
            #[repr(C, align(16))]
            struct A([u64; 4]);
            let a = pd(w[0], w[1]);
            for k in 0..9 {
                let mut m = A([0x1111_1111_1111_1111; 4]);
                let p = black_box(m.0.as_mut_ptr() as *mut f64);
                unsafe {
                    match k {
                        0 => _mm_store_pd(p, a),
                        1 => _mm_storeu_pd(p.add(1), a),
                        2 => _mm_store_sd(p.add(1), a),
                        3 => _mm_store1_pd(p, a),
                        4 => _mm_store_pd1(p.add(2), a),
                        5 => _mm_storer_pd(p, a),
                        6 => _mm_storeh_pd(p.add(3), a),
                        7 => _mm_storel_pd(p, a),
                        _ => _mm_stream_pd(p.add(2), a),
                    }
                }
                let m = black_box(m);
                o.put(w, [m.0[0] ^ m.0[1].rotate_left(17), m.0[2] ^ m.0[3].rotate_left(29)]);
            }
        }
        o.end();
        let mut o = Out::new("sets");
        for w in v.windows(2) {
            let (x, y) = (black_box(f64::from_bits(w[0])), black_box(f64::from_bits(w[1])));
            unsafe {
                for r in [_mm_set1_pd(x), _mm_set_pd1(y), _mm_set_pd(x, y), _mm_setr_pd(x, y), _mm_set_sd(y), _mm_setzero_pd()] {
                    o.put(w, bits_pd(r));
                }
                o.put(w, [_mm_cvtsd_f64(pd(w[0], w[1])).to_bits(), 0]);
            }
        }
        o.end();
    }

    #[inline(never)]
    fn casts(rng: &mut Rng) {
        let mut o = Out::new("casts");
        for _ in 0..32 {
            let (x, y) = (rng.next(), rng.next());
            let l = [x as u32, (x >> 32) as u32, y as u32, (y >> 32) as u32];
            unsafe {
                o.put(&[x, y], bits_ps(_mm_castpd_ps(pd(x, y))));
                o.put(&[x, y], bits_si(_mm_castpd_si128(pd(x, y))));
                o.put(&[x, y], bits_pd(_mm_castps_pd(ps(l))));
                o.put(&[x, y], bits_si(_mm_castps_si128(ps(l))));
                o.put(&[x, y], bits_pd(_mm_castsi128_pd(si(l))));
                o.put(&[x, y], bits_ps(_mm_castsi128_ps(si(l))));
            }
        }
        o.end();
    }

    /// Float -> integer conversions under one MXCSR rounding mode, every other conversion once.
    #[inline(never)]
    fn conversions(rng: &mut Rng) {
        let v = lanes64(rng);
        let mut f32s: Vec<u32> = EDGE32.to_vec();
        for &x in &v {
            f32s.push((f64::from_bits(x) as f32).to_bits());
        }
        for _ in 0..16 {
            f32s.push(rng.next() as u32);
        }
        let csr = unsafe { _mm_getcsr() };
        for (mode, rc) in [("nearest", 0u32), ("down", 1), ("up", 2), ("zero", 3)] {
            let mut o = Out::new(match mode {
                "nearest" => "f2i nearest",
                "down" => "f2i down",
                "up" => "f2i up",
                _ => "f2i zero",
            });
            unsafe { _mm_setcsr((csr & !0x6000) | rc << 13) };
            for (i, &x) in v.iter().enumerate() {
                let a = pd(x, v[(i + 1) % v.len()]);
                unsafe {
                    o.put(&[x], bits_si(_mm_cvtpd_epi32(a)));
                    o.put(&[x], bits_si(_mm_cvttpd_epi32(a)));
                    o.put(&[x], [_mm_cvtsd_si32(a) as u32 as u64, _mm_cvttsd_si32(a) as u32 as u64]);
                    o.put(&[x], [_mm_cvtsd_si64(a) as u64, _mm_cvttsd_si64(a) as u64]);
                    o.put(&[x], [_mm_cvtsd_si64x(a) as u64, _mm_cvttsd_si64x(a) as u64]);
                }
            }
            for c in f32s.chunks(4) {
                if c.len() < 4 {
                    continue;
                }
                let a = ps([c[0], c[1], c[2], c[3]]);
                unsafe {
                    o.put(&[c[0] as u64, c[1] as u64], bits_si(_mm_cvtps_epi32(a)));
                    o.put(&[c[0] as u64, c[1] as u64], bits_si(_mm_cvttps_epi32(a)));
                }
            }
            unsafe { _mm_setcsr(csr) };
            o.end();
        }
        let mut o = Out::new("float conversions");
        for (i, &x) in v.iter().enumerate() {
            let y = v[(i + 11) % v.len()];
            let a = pd(x, y);
            let f = f32s[i % f32s.len()];
            let g = f32s[(i * 5 + 3) % f32s.len()];
            let s = ps([f, g, 0x4049_0fdb, 0xc2f6_e979]);
            unsafe {
                o.put(&[x, y], bits_ps(_mm_cvtpd_ps(a)));
                o.put(&[f as u64, g as u64], bits_pd(_mm_cvtps_pd(s)));
                o.put(&[x, y], bits_ps(_mm_cvtsd_ss(s, a)));
                o.put(&[f as u64, g as u64], bits_pd(_mm_cvtss_sd(a, s)));
            }
        }
        o.end();
        let mut o = Out::new("int conversions");
        for (i, &k) in EDGEI32.iter().enumerate() {
            let k2 = EDGEI32[(i + 3) % 8];
            let n = si([k as u32, k2 as u32, 0x7fff_ffff, 0x8000_0001]);
            let a = pd(v[i], v[i + 9]);
            unsafe {
                o.put(&[k as u64], bits_pd(_mm_cvtepi32_pd(n)));
                o.put(&[k as u64], bits_ps(_mm_cvtepi32_ps(n)));
                o.put(&[k as u64], bits_pd(_mm_cvtsi32_sd(a, black_box(k))));
            }
        }
        for (i, &k) in EDGEI64.iter().enumerate() {
            let a = pd(v[i + 2], v[i + 5]);
            unsafe {
                o.put(&[k as u64], bits_pd(_mm_cvtsi64_sd(a, black_box(k))));
                o.put(&[k as u64], bits_pd(_mm_cvtsi64x_sd(a, black_box(k))));
            }
        }
        for _ in 0..32 {
            let (x, y) = (rng.next(), rng.next());
            let n = si([x as u32, (x >> 32) as u32, y as u32, (y >> 32) as u32]);
            let a = pd(y, x);
            unsafe {
                o.put(&[x, y], bits_pd(_mm_cvtepi32_pd(n)));
                o.put(&[x, y], bits_ps(_mm_cvtepi32_ps(n)));
                o.put(&[x, y], bits_pd(_mm_cvtsi64_sd(a, x as i64 >> (y % 64))));
            }
        }
        o.end();
    }

    /// The fractal zoomer's inner step: z = z^2 + c on two lanes, escape by movemask.
    #[inline(never)]
    fn mandel() {
        let mut o = Out::new("mandel");
        unsafe {
            let four = _mm_set1_pd(4.0);
            let two = _mm_set1_pd(2.0);
            for row in 0..8 {
                let ci = _mm_set1_pd(-1.2 + row as f64 * 0.3);
                let cr = _mm_setr_pd(-2.0 + row as f64 * 0.31, -0.5 + row as f64 * 0.07);
                let (mut zr, mut zi) = (_mm_setzero_pd(), _mm_setzero_pd());
                let mut count = _mm_setzero_pd();
                for _ in 0..200 {
                    let zr2 = _mm_mul_pd(zr, zr);
                    let zi2 = _mm_mul_pd(zi, zi);
                    let mag = _mm_add_pd(zr2, zi2);
                    let inside = _mm_cmplt_pd(mag, four);
                    if _mm_movemask_pd(inside) == 0 {
                        break;
                    }
                    count = _mm_add_pd(count, _mm_and_pd(inside, _mm_set1_pd(1.0)));
                    zi = _mm_add_pd(_mm_mul_pd(_mm_mul_pd(two, zr), zi), ci);
                    zr = _mm_add_pd(_mm_sub_pd(zr2, zi2), cr);
                }
                let mut out = [0.0f64; 2];
                _mm_storeu_pd(out.as_mut_ptr(), _mm_div_pd(count, _mm_max_pd(_mm_sqrt_pd(_mm_add_pd(zr, two)), _mm_set1_pd(1.0))));
                o.put(&[row], [out[0].to_bits(), out[1].to_bits()]);
                o.put(&[row], [movemask(_mm_cmpgt_pd(zr, zi)) as u64, _mm_movemask_pd(_mm_andnot_pd(zr, _mm_or_pd(zi, _mm_xor_pd(zr, ci)))) as u64]);
            }
        }
        o.end();
    }

    pub fn all() {
        let rng = &mut Rng(0x9e37_79b9_7f4a_7c15);
        bin_pd!(rng, _mm_add_pd _mm_sub_pd _mm_mul_pd _mm_div_pd _mm_min_pd _mm_max_pd);
        bin_pd!(rng, _mm_and_pd _mm_andnot_pd _mm_or_pd _mm_xor_pd _mm_unpacklo_pd _mm_unpackhi_pd _mm_move_sd);
        bin_pd!(rng, _mm_cmpeq_pd _mm_cmpneq_pd _mm_cmplt_pd _mm_cmple_pd _mm_cmpgt_pd _mm_cmpge_pd);
        bin_pd!(rng, _mm_cmpord_pd _mm_cmpunord_pd _mm_cmpnlt_pd _mm_cmpnle_pd _mm_cmpngt_pd _mm_cmpnge_pd);
        bin_pd!(rng, _mm_add_sd _mm_sub_sd _mm_mul_sd _mm_div_sd _mm_min_sd _mm_max_sd _mm_sqrt_sd);
        bin_pd!(rng, _mm_cmpeq_sd _mm_cmpneq_sd _mm_cmplt_sd _mm_cmple_sd _mm_cmpgt_sd _mm_cmpge_sd);
        bin_pd!(rng, _mm_cmpord_sd _mm_cmpunord_sd _mm_cmpnlt_sd _mm_cmpnle_sd _mm_cmpngt_sd _mm_cmpnge_sd);
        un_pd!(rng, _mm_sqrt_pd);
        int_pd!(rng, _mm_comieq_sd _mm_comilt_sd _mm_comile_sd _mm_comigt_sd _mm_comige_sd _mm_comineq_sd);
        int_pd!(rng, _mm_ucomieq_sd _mm_ucomilt_sd _mm_ucomile_sd _mm_ucomigt_sd _mm_ucomige_sd _mm_ucomineq_sd);
        {
            let mut o = Out::new("_mm_movemask_pd");
            for &x in &lanes64(rng) {
                for &y in EDGE64.iter() {
                    o.put(&[x, y], [movemask(pd(x, y)) as u64, 0]);
                }
            }
            o.end();
        }
        shuffles(rng);
        memory(rng);
        casts(rng);
        conversions(rng);
        mandel();
    }
}

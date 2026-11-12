// The 25 aarch64 NEON intrinsics fearless_simd's Neon level calls that the std here lacked
// (vello_cpu renders through it): vbslq_f64, the conversions vcvtq_f32_u32, vcvtq_s64_f64 and
// vcvtq_u32_f32, vfmsq_f64, vrndnq_f64, vrndq_f64, the register shifts vshlq_s8 and vshlq_u8,
// vuzp1q and vuzp2q of f64 and of the 8, 16 and 32 bit integers, vzip1q_f64 and vzip2q_f64.
// A few values are worked by hand. Each intrinsic is also folded over edge and pseudo-random
// operands into one hash, and the expected hashes are what the real instructions give (this file
// built with rustc on an Apple M-series CPU), so the emulation has to agree with the CPU bit for
// bit. The "| " lines are equal under rustc --test and makepad test.
#![cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;
use std::mem::transmute;

fn next(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

/// Sixteen bytes: random, or one edge pattern in every lane (zero, all ones, sign bits, the
/// largest positive values, zero high bytes).
fn bits(s: &mut u64) -> [u64; 2] {
    let mode = next(s) % 8;
    let mut w = [0u64; 2];
    for x in w.iter_mut() {
        let r = next(s);
        *x = match mode {
            0 => 0,
            1 => u64::MAX,
            2 => 0x8000_8000_8000_8000 ^ (r & 0x0101_0101_0101_0101),
            3 => 0x7f7f_7fff_7fff_ffff ^ (r & 0x0100_0001_0000_0080),
            4 => r & 0x00ff_00ff_00ff_00ff,
            _ => r,
        };
    }
    w
}

/// f64 lanes worth a look: zeros, infinities, quiet and signalling NaNs of both signs, the ends of
/// the subnormal and normal ranges, values around one, halves (the rounding ties) and the ends of
/// the i64 range.
const F64S: [u64; 28] = [
    0, 0x8000_0000_0000_0000, 0x7ff0_0000_0000_0000, 0xfff0_0000_0000_0000,
    0x7ff8_0000_0000_0001, 0xfff8_1234_5678_9abc, 0x7ff0_0000_0000_0001, 0xfff0_1234_5678_9abc,
    1, 0x8000_0000_0000_0001, 0x000f_ffff_ffff_ffff, 0x0010_0000_0000_0000,
    0x7fef_ffff_ffff_ffff, 0xffef_ffff_ffff_ffff, 0x3ff0_0000_0000_0000, 0xbff0_0000_0000_0000,
    0x3ff0_0000_0000_0001, 0x3fef_ffff_ffff_ffff, 0x3fe0_0000_0000_0000, 0xbfe0_0000_0000_0000,
    0x3ff8_0000_0000_0000, 0xbff8_0000_0000_0000, 0x4004_0000_0000_0000, 0xc004_0000_0000_0000,
    0x43e0_0000_0000_0000, 0xc3e0_0000_0000_0000, 0x43df_ffff_ffff_ffff, 0xc3e0_0000_0000_0001,
];

/// The same for f32, with the ends of the u32 range.
const F32S: [u32; 24] = [
    0, 0x8000_0000, 0x7f80_0000, 0xff80_0000, 0x7fc0_0001, 0xffc1_2345, 0x7f80_0001, 0xff81_2345,
    1, 0x8000_0001, 0x007f_ffff, 0x0080_0000, 0x7f7f_ffff, 0xff7f_ffff, 0x3f80_0000, 0xbf80_0000,
    0x3f00_0000, 0xbf00_0000, 0x3fc0_0000, 0x4020_0000, 0x4f80_0000, 0x4f7f_ffff, 0x4f00_0000, 0xcf00_0000,
];

/// Operand `arg` of case `case` as two f64 lanes: the first 28^args cases walk every combination
/// of F64S through lane 0 (lane 1 runs along the table), the rest are `bits`.
fn f64s(s: &mut u64, case: usize, arg: u32, args: u32) -> float64x2_t {
    let n = F64S.len();
    let w = if case < n.pow(args) {
        let i = case / n.pow(arg) % n;
        [F64S[i], F64S[(i + case + arg as usize + 1) % n]]
    } else {
        bits(s)
    };
    unsafe { transmute(w) }
}

fn f32s(s: &mut u64, case: usize) -> float32x4_t {
    let n = F32S.len();
    if case < n {
        unsafe { transmute([F32S[case], F32S[(case + 1) % n], F32S[(case + 7) % n], F32S[(case + 13) % n]]) }
    } else {
        unsafe { transmute(bits(s)) }
    }
}

/// FNV-1a over the bytes of what `f` returns for each of `n` cases.
fn fold<T: Copy>(n: usize, mut f: impl FnMut(&mut u64, usize) -> T) -> u64 {
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for case in 0..n {
        let v = f(&mut s, case);
        let b = unsafe { std::slice::from_raw_parts(&v as *const T as *const u8, std::mem::size_of::<T>()) };
        for &x in b {
            h = (h ^ x as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}

fn expect(name: &str, hash: u64) {
    println!("| {name} {hash:016x}");
    let real = REAL.iter().find(|(n, _)| *n == name).expect(name).1;
    assert_eq!(hash, real, "{name}: the real instruction gives {real:016x}");
}

#[test]
fn a01_worked_values() {
    unsafe {
        let f = |a: [f64; 2]| -> float64x2_t { transmute(a) };
        let o = |v: float64x2_t| -> [f64; 2] { transmute(v) };
        // the mask picks bits, not lanes
        let m: uint64x2_t = transmute([u64::MAX, 0x8000_0000_0000_0000u64]);
        assert_eq!(o(vbslq_f64(m, f([1.5, -2.0]), f([7.0, 3.0]))), [1.5, -3.0]);
        // unsigned to float rounds to nearest even: 2^24 + 1 and 2^32 - 1 are not f32s
        let u: uint32x4_t = transmute([0u32, 16_777_217, u32::MAX, 0x8000_0080]);
        assert_eq!(transmute::<_, [f32; 4]>(vcvtq_f32_u32(u)), [0.0, 16_777_216.0, 4_294_967_296.0, 2_147_483_648.0]);
        // float to integer truncates and saturates; a NaN gives zero
        let c: [i64; 2] = transmute(vcvtq_s64_f64(f([-2.9, 1.0e300])));
        assert_eq!(c, [-2, i64::MAX]);
        let c: [i64; 2] = transmute(vcvtq_s64_f64(f([f64::NAN, f64::NEG_INFINITY])));
        assert_eq!(c, [0, i64::MIN]);
        let c: [u32; 4] = transmute(vcvtq_u32_f32(transmute([2.9f32, -1.0, 5.0e9, f32::NAN])));
        assert_eq!(c, [2, 0, u32::MAX, 0]);
        // a - b * c in one rounding: (1 + 2^-52) * (1 - 2^-53) is 1 + 2^-53 - 2^-105
        let e = f64::EPSILON;
        assert_eq!(o(vfmsq_f64(f([10.0, 1.0]), f([2.0, 1.0 + e]), f([3.0, 1.0 - e / 2.0]))), [4.0, -e / 2.0 + e * e / 2.0]);
        // to nearest even, and toward zero
        assert_eq!(o(vrndnq_f64(f([0.5, 1.5]))), [0.0, 2.0]);
        assert_eq!(o(vrndnq_f64(f([-2.5, 2.5000000000000004]))), [-2.0, 3.0]);
        assert_eq!(o(vrndq_f64(f([-2.9, 2.9]))), [-2.0, 2.0]);
        assert!(o(vrndq_f64(f([-0.9, 0.0])))[0].is_sign_negative());
        // a negative count shifts right (arithmetic when signed); eight or more shifts everything out
        let a: int8x16_t = transmute([1i8, 1, 1, -128, -128, -128, -1, 100, 100, 100, -100, -100, 3, 3, 3, 3]);
        let n: int8x16_t = transmute([0i8, 7, 8, 1, -7, -8, -128, -1, -2, 127, -1, -9, 2, -2, 6, -1]);
        assert_eq!(transmute::<_, [i8; 16]>(vshlq_s8(a, n)), [1, -128, 0, 0, -1, -1, -1, 50, 25, 0, -50, -1, 12, 0, -64, 1]);
        let a: uint8x16_t = transmute([1u8, 1, 1, 128, 128, 128, 255, 100, 100, 100, 200, 200, 3, 3, 3, 3]);
        assert_eq!(transmute::<_, [u8; 16]>(vshlq_u8(a, n)), [1, 128, 0, 0, 1, 0, 0, 50, 25, 0, 100, 0, 12, 0, 192, 1]);
        // unzip: the even lanes of a then b, or the odd ones
        let a: int16x8_t = transmute([0i16, 1, 2, 3, 4, 5, 6, 7]);
        let b: int16x8_t = transmute([10i16, 11, 12, 13, 14, 15, 16, 17]);
        assert_eq!(transmute::<_, [i16; 8]>(vuzp1q_s16(a, b)), [0, 2, 4, 6, 10, 12, 14, 16]);
        assert_eq!(transmute::<_, [i16; 8]>(vuzp2q_s16(a, b)), [1, 3, 5, 7, 11, 13, 15, 17]);
        let a: uint32x4_t = transmute([0u32, 1, 2, 3]);
        let b: uint32x4_t = transmute([10u32, 11, 12, 13]);
        assert_eq!(transmute::<_, [u32; 4]>(vuzp1q_u32(a, b)), [0, 2, 10, 12]);
        assert_eq!(transmute::<_, [u32; 4]>(vuzp2q_u32(a, b)), [1, 3, 11, 13]);
        let (a, b) = (f([1.0, 2.0]), f([3.0, 4.0]));
        assert_eq!(o(vuzp1q_f64(a, b)), [1.0, 3.0]);
        assert_eq!(o(vuzp2q_f64(a, b)), [2.0, 4.0]);
        assert_eq!(o(vzip1q_f64(a, b)), [1.0, 3.0]);
        assert_eq!(o(vzip2q_f64(a, b)), [2.0, 4.0]);
        println!("| worked values ok");
    }
}

#[test]
fn a02_select_and_convert() {
    unsafe {
        expect("vbslq_f64", fold(4096, |s, _| vbslq_f64(transmute(bits(s)), transmute(bits(s)), transmute(bits(s)))));
        // every bit length, so every position of the rounding
        expect(
            "vcvtq_f32_u32",
            fold(8192, |s, case| {
                let r: [u32; 4] = transmute(bits(s));
                let k = case as u32;
                let a = if case < 4096 { [r[0] >> (k % 32), 1 << (k % 32), (0x00ff_ffff_u32 + (k >> 5)) << (k % 8), !(r[3] >> (k % 32))] } else { r };
                vcvtq_f32_u32(transmute(a))
            }),
        );
        expect(
            "vcvtq_s64_f64",
            fold(8192, |s, case| {
                let w = bits(s);
                let k = case as u32;
                // integers with fractions at every magnitude, and both ends of the range
                let a: float64x2_t = if case < 28 {
                    f64s(s, case, 0, 1)
                } else if case < 4096 {
                    transmute([(w[0] as i64 >> (k % 64)) as f64 + 0.75, (w[1] as i64 >> (k % 11)) as f64 * if k & 1 == 0 { 1.0 } else { -1.0 }])
                } else {
                    transmute(w)
                };
                vcvtq_s64_f64(a)
            }),
        );
        expect(
            "vcvtq_u32_f32",
            fold(8192, |s, case| {
                let w: [u32; 4] = transmute(bits(s));
                let k = case as u32;
                let a: float32x4_t = if case < 24 {
                    f32s(s, case)
                } else if case < 4096 {
                    transmute([(w[0] >> (k % 32)) as f32 + 0.75, -((w[1] >> (k % 32)) as f32), w[2] as f32 * 1.5, (w[3] >> (k % 9)) as f32 * 0.999])
                } else {
                    transmute(w)
                };
                vcvtq_u32_f32(a)
            }),
        );
    }
}

fn nan64(x: u64) -> bool {
    x & 0x7fff_ffff_ffff_ffff > 0x7ff0_0000_0000_0000
}

#[test]
fn a03_fused_multiply_subtract() {
    const SIGN: u64 = 0x8000_0000_0000_0000;
    const QUIET: u64 = 0x0008_0000_0000_0000;
    let n = F64S.len().pow(3) + 4096;
    let h = fold(n, |s, case| unsafe {
        let (a, b, c) = (f64s(s, case, 0, 3), f64s(s, case, 1, 3), f64s(s, case, 2, 3));
        let mut r: [u64; 2] = transmute(vfmsq_f64(a, b, c));
        let (b, c): ([u64; 2], [u64; 2]) = (transmute(b), transmute(c));
        // A NaN of b or c comes back with either sign, and b's or c's when both are NaNs: stdarch
        // negates b and then fuses, an optimised build folds the negation into FMLS's first
        // factor, which LLVM makes c. Such a lane reads as that NaN without its sign, or as one
        // marker when both are NaNs.
        for l in 0..2 {
            let from_b = nan64(b[l]) && r[l] | SIGN == b[l] | SIGN | QUIET;
            let from_c = nan64(c[l]) && r[l] | SIGN == c[l] | SIGN | QUIET;
            if nan64(b[l]) && nan64(c[l]) && (from_b || from_c) {
                r[l] = 0x7ff8_0000_0000_0000;
            } else if from_b || from_c {
                r[l] &= !SIGN;
            }
        }
        r
    });
    expect("vfmsq_f64", h);
    // products that cancel a to the last place, where rounding the product first would lose bits
    let h = fold(4096, |s, _| unsafe {
        let w = bits(s);
        let b = 1.0 + (w[0] >> 12) as f64 * f64::EPSILON;
        let c = 1.0 - (w[1] >> 12) as f64 * f64::EPSILON / 2.0;
        vfmsq_f64(transmute([b * c, 1.0]), transmute([b, b]), transmute([c, c]))
    });
    expect("vfmsq_f64 cancelling", h);
}

#[test]
fn a04_round() {
    let operand = |s: &mut u64, case: usize| -> float64x2_t {
        let w = bits(s);
        let k = case as u32;
        unsafe {
            if case < 28 {
                f64s(s, case, 0, 1)
            } else if case < 4096 {
                // quarters at every magnitude up to 2^53, where every f64 is an integer
                transmute([(w[0] as i64 >> (k % 64)) as f64 * 0.25, (w[1] as i64 >> (k % 13 + 8)) as f64 * 0.5])
            } else {
                transmute(w)
            }
        }
    };
    unsafe {
        expect("vrndnq_f64", fold(8192, |s, case| vrndnq_f64(operand(s, case))));
        expect("vrndq_f64", fold(8192, |s, case| vrndq_f64(operand(s, case))));
    }
}

#[test]
fn a05_shift_by_register() {
    // every value by every count
    let operands = |case: usize| -> ([u8; 16], [u8; 16]) {
        let mut a = [0u8; 16];
        let mut n = [0u8; 16];
        for l in 0..16 {
            let k = case * 16 + l;
            a[l] = k as u8;
            n[l] = (k >> 8) as u8;
        }
        (a, n)
    };
    unsafe {
        expect(
            "vshlq_s8",
            fold(4096, |_, case| {
                let (a, n) = operands(case);
                vshlq_s8(transmute(a), transmute(n))
            }),
        );
        expect(
            "vshlq_u8",
            fold(4096, |_, case| {
                let (a, n) = operands(case);
                vshlq_u8(transmute(a), transmute(n))
            }),
        );
    }
}

#[test]
fn a06_unzip_and_zip() {
    macro_rules! each {
        ($($f:ident)*) => {$(
            expect(stringify!($f), fold(2048, |s, _| unsafe { $f(transmute(bits(s)), transmute(bits(s))) }));
        )*};
    }
    each! {
        vuzp1q_f64 vuzp1q_s16 vuzp1q_s32 vuzp1q_s8 vuzp1q_u16 vuzp1q_u32 vuzp1q_u8
        vuzp2q_f64 vuzp2q_s16 vuzp2q_s32 vuzp2q_s8 vuzp2q_u16 vuzp2q_u32 vuzp2q_u8
        vzip1q_f64 vzip2q_f64
    }
}

/// What the real instructions give: every hash above, from this file built with rustc.
const REAL: &[(&str, u64)] = &[
    ("vbslq_f64", 0xbf256bab0edecdd7),
    ("vcvtq_f32_u32", 0xe43ba9f99e9a8bbe),
    ("vcvtq_s64_f64", 0x978d4b0207df92a7),
    ("vcvtq_u32_f32", 0x47b707299b783128),
    ("vfmsq_f64", 0x22dcffd89d21c834),
    ("vfmsq_f64 cancelling", 0x78280efd0034fc53),
    ("vrndnq_f64", 0x98408b3b037c5c80),
    ("vrndq_f64", 0x1a60680846ae2da6),
    ("vshlq_s8", 0xd7ac1d26b99bada5),
    ("vshlq_u8", 0xa1925f0b0c56bfa5),
    ("vuzp1q_f64", 0x0bf030271c383ad3),
    ("vuzp1q_s16", 0x1efc97e0bac69964),
    ("vuzp1q_s32", 0x7cd4919383ca3b34),
    ("vuzp1q_s8", 0x796f304d82d33c23),
    ("vuzp1q_u16", 0x1efc97e0bac69964),
    ("vuzp1q_u32", 0x7cd4919383ca3b34),
    ("vuzp1q_u8", 0x796f304d82d33c23),
    ("vuzp2q_f64", 0x3d9774246f7fb0d3),
    ("vuzp2q_s16", 0x6cfe3607cdc93de0),
    ("vuzp2q_s32", 0xb45078832a5c63c4),
    ("vuzp2q_s8", 0xd13135980b5eebc5),
    ("vuzp2q_u16", 0x6cfe3607cdc93de0),
    ("vuzp2q_u32", 0xb45078832a5c63c4),
    ("vuzp2q_u8", 0xd13135980b5eebc5),
    ("vzip1q_f64", 0x0bf030271c383ad3),
    ("vzip2q_f64", 0x3d9774246f7fb0d3),
];

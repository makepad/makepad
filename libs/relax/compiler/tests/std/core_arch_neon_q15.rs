// The eight aarch64 NEON intrinsics moxcms's fixed-point paths call that the std here lacked
// ("cannot find function `vqrdmlah_s16` in this scope" and the like): the widening
// multiply-accumulate vmlal_s16, the saturating narrow vqmovun_s32, the FEAT_RDM rounding doubling
// multiply-accumulates vqrdmlah_s16, vqrdmlahq_s16, vqrdmlahq_s32, vqrdmlsh_s16 and
// vqrdmlshq_s16, and vcvtaq_u32_f32. A few values are worked by hand. Each intrinsic is also
// folded over edge and pseudo-random operands into one hash, and the expected hashes are what the
// real instructions give (this file built with rustc on an Apple M-series CPU), so the emulation
// has to agree with the CPU bit for bit. The "| " lines are equal under rustc --test and makepad
// test.
#![cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;
use std::mem::transmute;

const E16: [i16; 8] = [0, 1, -1, i16::MIN, i16::MAX, 0x4000, -0x4000, 0x7ffe];
const E32: [i32; 8] = [0, 1, -1, i32::MIN, i32::MAX, 0x4000_0000, -0x4000_0000, 0x0001_8000];
const EF: [f32; 16] = [
    0.0, -0.0, 0.5, -0.5, 1.5, 2.5, -1.5, 0.49999997, 8388609.0, 4294967040.0, 4294967296.0, 1.0e20, f32::INFINITY,
    f32::NEG_INFINITY, f32::NAN, 1.0e-40,
];

fn next(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

/// Operand `arg` of case `case`: the first 512 cases walk every triple of edge values through
/// lane 0 (the other lanes run along the table), the rest are random.
fn lanes16<const N: usize>(s: &mut u64, case: usize, arg: u32) -> [i16; N] {
    let mut r = [0i16; N];
    for (j, x) in r.iter_mut().enumerate() {
        *x = if case < 512 { E16[(case / 8usize.pow(arg) + j * (arg as usize + 1)) % 8] } else { next(s) as i16 };
    }
    r
}

fn lanes32(s: &mut u64, case: usize, arg: u32) -> [i32; 4] {
    let mut r = [0i32; 4];
    for (j, x) in r.iter_mut().enumerate() {
        *x = if case < 512 { E32[(case / 8usize.pow(arg) + j * (arg as usize + 1)) % 8] } else { next(s) as i32 };
    }
    r
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
        let v = |a: [i16; 4]| -> int16x4_t { transmute(a) };
        let o = |v: int16x4_t| -> [i16; 4] { transmute(v) };
        if std::arch::is_aarch64_feature_detected!("rdm") {
            // accumulate: the sum is rounded and saturated once, so MIN * MIN (+1.0 in Q15) on top
            // of -1.0 is 0
            let (b, c) = (v([i16::MIN, 0x4000, 0x4000, 0x2000]), v([i16::MIN, 0x4000, 0x4000, 4]));
            assert_eq!(o(vqrdmlah_s16(v([i16::MIN, 100, i16::MAX, -5]), b, c)), [0, 100 + 0x2000, i16::MAX, -4]);
            assert_eq!(o(vqrdmlsh_s16(v([i16::MAX, 100, i16::MIN, 5]), b, c)), [-1, 100 - 0x2000, i16::MIN, 4]);
            let a: int16x8_t = transmute([i16::MIN, 0, 0, 0, 1, 2, 3, 4]);
            let c: int16x8_t = transmute([i16::MIN, 0, 1, -1, 0, 0, 0, 0]);
            let q: [i16; 8] = transmute(vqrdmlahq_s16(a, transmute([i16::MIN; 8]), c));
            assert_eq!(q, [0, 0, -1, 1, 1, 2, 3, 4]);
            let q: [i16; 8] = transmute(vqrdmlshq_s16(a, transmute([i16::MIN; 8]), c));
            assert_eq!(q, [i16::MIN, 0, 1, -1, 1, 2, 3, 4]);
            let a: [i32; 4] = transmute(vqrdmlahq_s32(
                transmute([i32::MIN, 100, i32::MAX, -5]),
                transmute([i32::MIN, 0x4000_0000, 0x4000_0000, 2]),
                transmute([i32::MIN, 0x4000_0000, 0x4000_0000, 0x4000_0000]),
            ));
            assert_eq!(a, [0, 100 + 0x2000_0000, i32::MAX, -4]);
        }
        // the products are 32 bits wide and the sum wraps
        let w: [i32; 4] = transmute(vmlal_s16(transmute([1i32, -1, i32::MAX, 0]), v([2, -3, 1, i16::MIN]), v([5, 7, 1, i16::MIN])));
        assert_eq!(w, [11, -22, i32::MIN, 0x4000_0000]);
        let n: [u16; 4] = transmute(vqmovun_s32(transmute([-1i32, 0x1_0000, 0xffff, 12345])));
        assert_eq!(n, [0, 0xffff, 0xffff, 12345]);
        // ties go away from zero; a negative value and a NaN give zero
        let c: [u32; 4] = transmute(vcvtaq_u32_f32(transmute([0.5f32, 2.5, -0.5, 1.0e20])));
        assert_eq!(c, [1, 3, 0, u32::MAX]);
        let c: [u32; 4] = transmute(vcvtaq_u32_f32(transmute([0.49999997f32, f32::NAN, -7.5, 4294967040.0])));
        assert_eq!(c, [0, 0, 0, 4294967040]);
        println!("| worked values ok");
    }
}

#[test]
fn a02_rounding_doubling_multiply_accumulate() {
    if !std::arch::is_aarch64_feature_detected!("rdm") {
        return;
    }
    macro_rules! half {
        ($($f:ident)*) => {$(
            expect(stringify!($f), fold(2560, |s, case| unsafe {
                let (a, b, c) = (lanes16::<4>(s, case, 0), lanes16::<4>(s, case, 1), lanes16::<4>(s, case, 2));
                $f(transmute(a), transmute(b), transmute(c))
            }));
        )*};
    }
    macro_rules! quad {
        ($($f:ident)*) => {$(
            expect(stringify!($f), fold(2560, |s, case| unsafe {
                let (a, b, c) = (lanes16::<8>(s, case, 0), lanes16::<8>(s, case, 1), lanes16::<8>(s, case, 2));
                $f(transmute(a), transmute(b), transmute(c))
            }));
        )*};
    }
    half!(vqrdmlah_s16 vqrdmlsh_s16);
    quad!(vqrdmlahq_s16 vqrdmlshq_s16);
    expect(
        "vqrdmlahq_s32",
        fold(2560, |s, case| unsafe {
            let (a, b, c) = (lanes32(s, case, 0), lanes32(s, case, 1), lanes32(s, case, 2));
            vqrdmlahq_s32(transmute(a), transmute(b), transmute(c))
        }),
    );
}

#[test]
fn a03_widen_narrow_convert() {
    unsafe {
        expect(
            "vmlal_s16",
            fold(2560, |s, case| {
                let (a, b, c) = (lanes32(s, case, 0), lanes16::<4>(s, case, 1), lanes16::<4>(s, case, 2));
                vmlal_s16(transmute(a), transmute(b), transmute(c))
            }),
        );
        expect(
            "vqmovun_s32",
            fold(2560, |s, case| {
                let mut a = lanes32(s, case, 0);
                if case % 3 == 0 {
                    // around the 16-bit boundaries, where the saturation starts
                    a = [a[0] >> 15, a[1] >> 16, 0xffff + (a[2] >> 30), a[3] >> 14];
                }
                vqmovun_s32(transmute(a))
            }),
        );
        expect(
            "vcvtaq_u32_f32",
            fold(4096, |s, case| {
                let x: [f32; 4] = if case < 16 {
                    [EF[case], -EF[case], EF[(case + 5) % 16], EF[(case + 11) % 16] * 0.5]
                } else if case < 1536 {
                    // every quarter step around the ties
                    let q = (case - 16) as f32 * 0.25;
                    [q, -q, q + 4294965000.0, q * 2097152.0]
                } else {
                    [f32::from_bits(next(s) as u32), f32::from_bits(next(s) as u32), (next(s) % 100000) as f32 / 64.0, (next(s) >> 40) as f32 / 256.0]
                };
                vcvtaq_u32_f32(transmute(x))
            }),
        );
    }
}

/// What the real instructions give: every hash above, from this file built with rustc.
const REAL: &[(&str, u64)] = &[
    ("vqrdmlah_s16", 0xadda9ba597643292),
    ("vqrdmlsh_s16", 0x4c6fb2e250a47adc),
    ("vqrdmlahq_s16", 0xacd0b40374898e0a),
    ("vqrdmlshq_s16", 0x0775c725fe3d9294),
    ("vqrdmlahq_s32", 0x7579d597c192468c),
    ("vmlal_s16", 0x5aea8efdfaeab46f),
    ("vqmovun_s32", 0x2109e87dce11361d),
    ("vcvtaq_u32_f32", 0xaf74ca757a0bbe4d),
];

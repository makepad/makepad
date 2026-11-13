// The aarch64 NEON reciprocal square root estimate (vrsqrteq_f32: Arm's FRSQRTE, an 8-bit estimate
// from a table) and its Newton step (vrsqrtsq_f32: FRSQRTS, (3 - a * b) / 2 rounded once), the
// pair tiny-skia's f32x4 takes for 1/sqrt(x). Fixed values are asserted; every class of f32 (zeros,
// infinities, NaNs, negatives, subnormals, each fraction prefix the table reads at even and odd
// exponents, random bits) is folded into hashes, and the expected hashes are what the real
// instructions give (this file built with rustc on an Apple M-series CPU), so the emulation has to
// agree with the CPU bit for bit. The "| " lines are equal under rustc --test and makepad test.
#![cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;
use std::mem::transmute;

fn rsqrte(x: u32) -> u32 {
    unsafe {
        let r: [u32; 4] = transmute(vrsqrteq_f32(transmute([x; 4])));
        assert!(r.iter().all(|l| *l == r[0]), "lanes of {x:08x} differ: {r:08x?}");
        r[0]
    }
}

fn rsqrts(a: u32, b: u32) -> u32 {
    unsafe {
        let r: [u32; 4] = transmute(vrsqrtsq_f32(transmute([a; 4]), transmute([b; 4])));
        assert!(r.iter().all(|l| *l == r[0]), "lanes of {a:08x} {b:08x} differ: {r:08x?}");
        r[0]
    }
}

fn next(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
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
fn a01_estimate_classes() {
    // 1/sqrt(1) is 1 - 1/512, the table's top entry; an even exponent reads the other half
    assert_eq!(rsqrte(1.0f32.to_bits()), 0.998046875f32.to_bits());
    assert_eq!(rsqrte(4.0f32.to_bits()), 0.4990234375f32.to_bits());
    assert_eq!(rsqrte(0.25f32.to_bits()), 1.99609375f32.to_bits());
    assert_eq!(rsqrte(2.0f32.to_bits()), 0.705078125f32.to_bits());
    assert_eq!(rsqrte(0), f32::INFINITY.to_bits());
    assert_eq!(rsqrte(0x8000_0000), f32::NEG_INFINITY.to_bits());
    assert_eq!(rsqrte(f32::INFINITY.to_bits()), 0);
    assert_eq!(rsqrte(f32::NEG_INFINITY.to_bits()), 0x7fc0_0000, "the default NaN");
    assert_eq!(rsqrte((-1.0f32).to_bits()), 0x7fc0_0000);
    assert_eq!(rsqrte(0x8000_0001), 0x7fc0_0000, "a negative subnormal too");
    assert_eq!(rsqrte(0x7f80_0001), 0x7fc0_0001, "a signalling NaN comes back quieted");
    assert_eq!(rsqrte(0xffc0_1234), 0xffc0_1234, "a quiet NaN comes back as it was");
    assert_eq!(rsqrte(0xffa0_0005), 0xffe0_0005);
    let edges = [
        0x0000_0001, 0x0000_0002, 0x0000_0003, 0x0000_ffff, 0x0020_0000, 0x003f_ffff, 0x0040_0000, 0x007f_ffff,
        0x0080_0000, 0x0080_0001, 0x00ff_ffff, 0x0100_0000, 0x3e80_0000, 0x3f00_0000, 0x3f7f_ffff, 0x3f80_0000,
        0x3f80_ffff, 0x3f81_0000, 0x3fff_ffff, 0x4000_0000, 0x4000_7fff, 0x4000_8000, 0x4049_0fdb, 0x7e80_0000,
        0x7f00_0000, 0x7f7f_ffff, 0x8000_0001, 0x807f_ffff, 0xbf80_0000, 0xff7f_ffff,
    ];
    for x in edges {
        println!("| rsqrte {:08x} {:08x}", x, rsqrte(x));
    }
}

#[test]
fn a02_estimate_fractions_subnormals_and_random() {
    // every 8-bit fraction prefix the estimate reads, at even and odd exponents from the bottom
    // to the top, with the bits below the prefix clear, set and mixed
    expect(
        "vrsqrteq_f32 every prefix",
        fold(10 * 256 * 3, |_, case| {
            let exp = [1u32, 2, 3, 126, 127, 128, 129, 252, 253, 254][case / 768];
            let low = [0u32, 0x7fff, 0x4321][case % 3];
            rsqrte((exp << 23) | ((case as u32 / 3 % 256) << 15) | low)
        }),
    );
    // every subnormal's leading bit, alone, with the bits below it set, and with mixed ones
    expect(
        "vrsqrteq_f32 subnormals",
        fold(23 * 4, |s, case| {
            let top = 1u32 << (case / 4);
            let below = top - 1;
            rsqrte(top | [0, below, below & 0x2aaa_aaaa, below & next(s) as u32][case % 4])
        }),
    );
    // four unrelated lanes at once: any bits, then positive ones only
    expect("vrsqrteq_f32", fold(4096, |s, _| unsafe { vrsqrteq_f32(transmute([next(s), next(s)])) }));
    expect(
        "vrsqrteq_f32 positive",
        fold(4096, |s, _| unsafe { vrsqrteq_f32(transmute([next(s) & 0x7fff_ffff_7fff_ffff, next(s) & 0x7fff_ffff_7fff_ffff])) }),
    );
}

#[test]
fn a03_step() {
    assert_eq!(rsqrts(1.0f32.to_bits(), 1.0f32.to_bits()), 1.0f32.to_bits());
    assert_eq!(rsqrts(0.5f32.to_bits(), 2.0f32.to_bits()), 1.0f32.to_bits());
    assert_eq!(rsqrts(2.0f32.to_bits(), 2.0f32.to_bits()), (-0.5f32).to_bits());
    assert_eq!(rsqrts(3.0f32.to_bits(), 1.0f32.to_bits()), 0, "an exact zero is positive");
    // fused: (1 + 2^-23) * (1 - 2^-22) rounded first would give a tie, and 1 exactly
    assert_eq!(rsqrts(0x3f80_0001, 0x3f7f_fffc), 0x3f80_0001);
    // the product overflows and the halved sum does not
    assert_eq!(rsqrts(0x7f00_0000, 0x4000_0000), 0xff00_0000);
    assert_eq!(rsqrts(f32::INFINITY.to_bits(), 0), 1.5f32.to_bits(), "an infinity times a zero is 1.5");
    assert_eq!(rsqrts(0x8000_0000, f32::NEG_INFINITY.to_bits()), 1.5f32.to_bits());
    assert_eq!(rsqrts(f32::INFINITY.to_bits(), 1.0f32.to_bits()), f32::NEG_INFINITY.to_bits());
    assert_eq!(rsqrts(f32::NEG_INFINITY.to_bits(), 1.0f32.to_bits()), f32::INFINITY.to_bits());
    // the first operand is negated before the NaN rules: its NaN comes back with the other sign
    assert_eq!(rsqrts(0x7fc0_0001, 1.0f32.to_bits()), 0xffc0_0001);
    assert_eq!(rsqrts(1.0f32.to_bits(), 0x7fc0_0001), 0x7fc0_0001);
    assert_eq!(rsqrts(0x7f80_0001, 0x7fc0_0002), 0xffc0_0001, "a signalling NaN wins and is quieted");
    assert_eq!(rsqrts(0x7fc0_0002, 0xff80_0001), 0xffc0_0001);
    let v = [
        0u32, 0x8000_0000, 0x7f80_0000, 0xff80_0000, 0x7fc0_0000, 0x7f80_0001, 0xffc0_1234, 0x7fa0_0005, 0x0000_0001,
        0x807f_ffff, 0x3f80_0000, 0x3f80_0001, 0x3f7f_ffff, 0x4000_0000, 0x7f7f_ffff, 0x0080_0000, 0x3f00_0000,
        0xbfc0_0000, 0x5f00_0000, 0x2000_0000, 0x4040_0000, 0xc040_0000,
    ];
    for &a in &v {
        println!("| rsqrts {:08x} {:08x} {:08x} {:08x}", a, rsqrts(a, 0), rsqrts(a, 0x7f80_0000), rsqrts(a, 0x3f80_0001));
    }
    expect("vrsqrtsq_f32 special pairs", fold(v.len() * v.len(), |_, case| rsqrts(v[case / v.len()], v[case % v.len()])));
    expect("vrsqrtsq_f32", fold(4096, |s, _| unsafe { vrsqrtsq_f32(transmute([next(s), next(s)]), transmute([next(s), next(s)])) }));
    // products around 3, where the sum cancels, and around the overflow
    expect(
        "vrsqrtsq_f32 near three",
        fold(4096, |s, case| {
            let a = f32::from_bits(0x3f00_0000 + (next(s) as u32 % 0x0200_0000));
            let b = 3.0 / a;
            rsqrts(a.to_bits() ^ ((case as u32 & 1) << 31), b.to_bits().wrapping_add(next(s) as u32 % 9).wrapping_sub(4))
        }),
    );
    expect(
        "vrsqrtsq_f32 near overflow",
        fold(4096, |s, _| {
            let a = f32::from_bits(0x5f00_0000 + (next(s) as u32 % 0x2000_0000));
            let b = f32::from_bits(0x7f80_0000 - (next(s) as u32 % 0x0180_0000)) / a;
            rsqrts(a.to_bits(), b.to_bits() ^ (next(s) as u32 & 0x8000_0000))
        }),
    );
}

#[test]
fn a04_newton_refinement() {
    // 1/sqrt(x) as tiny-skia computes it: the estimate, then x * e * e against e
    let refine = |x: float32x4_t, e: float32x4_t| unsafe { vmulq_f32(vrsqrtsq_f32(x, vmulq_f32(e, e)), e) };
    let mut nans = 0;
    let mut worst = [0.0f64; 3];
    let hash = fold(4096, |s, case| unsafe {
        // normal numbers whose square root's reciprocal stays normal, and a few fixed ones
        let mut x = [0f32; 4];
        for (j, l) in x.iter_mut().enumerate() {
            *l = f32::from_bits(0x0080_0000 + (next(s) as u32 % 0x7e80_0000));
            if case < 8 {
                *l = [1.0, 2.0, 0.5, 4.0, 1.0e-30, 3.0e30, 16777216.0, 0.1][(case + j) % 8];
            }
        }
        let e0 = vrsqrteq_f32(transmute(x));
        let e1 = refine(transmute(x), e0);
        let e2 = refine(transmute(x), e1);
        for (step, e) in [e0, e1, e2].into_iter().enumerate() {
            let e: [f32; 4] = transmute(e);
            for j in 0..4 {
                nans += e[j].is_nan() as u32;
                let exact = 1.0 / (x[j] as f64).sqrt();
                worst[step] = worst[step].max(((e[j] as f64 - exact) / exact).abs());
            }
        }
        [e0, e1, e2]
    });
    assert_eq!(nans, 0);
    // the estimate is good to 8 bits, and each step roughly doubles them
    assert!(worst[0] < 1.0 / 256.0 && worst[1] < 1.0 / 32768.0 && worst[2] < 1.0 / 4194304.0, "{worst:?}");
    expect("newton refinement", hash);
}

/// What the real instructions give: every hash above, from this file built with rustc.
const REAL: &[(&str, u64)] = &[
    ("vrsqrteq_f32 every prefix", 0xaec7a83ba57f4809),
    ("vrsqrteq_f32 subnormals", 0x4aadb6d032334f52),
    ("vrsqrteq_f32", 0x6413bd4b6d162151),
    ("vrsqrteq_f32 positive", 0x70df29e19d29e44b),
    ("vrsqrtsq_f32 special pairs", 0xb1d31c35d7b13951),
    ("vrsqrtsq_f32", 0x168e4a7451f3bc4f),
    ("vrsqrtsq_f32 near three", 0xb162bd9e3df9bc00),
    ("vrsqrtsq_f32 near overflow", 0xb1e44fa6d92b925d),
    ("newton refinement", 0x2f2e4415e1c55292),
];

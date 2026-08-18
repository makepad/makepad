//! Deterministic f32 math for the Arcade sim (game.md §Engine design).
//!
//! IEEE-754 requires bit-exact results across architectures for add, sub, mul,
//! div, sqrt and the exact ops (abs/floor/ceil/round/trunc) — but NOT for
//! transcendentals: Rust's `f32::sin` etc. lower to the platform libm, whose
//! last bits differ per OS/arch/version. Every function here is built only
//! from exactly-rounded ops and fixed polynomial kernels (evaluated in f64,
//! rounded once to f32), so results are bit-identical on every platform.
//! Rust never contracts `a*b + c` into an fma, so polynomial evaluation order
//! is stable.
//!
//! Kernels follow fdlibm/musl (sin/cos/atan/ln); exp uses a degree-7 Taylor on
//! the Cody-Waite-reduced argument. Accuracy is a few f32 ulp — game-grade,
//! not libm-replacement-grade. sin/cos accuracy degrades (deterministically)
//! for |x| beyond ~2^24; keep angles reduced.

// ---------------------------------------------------------------- sin / cos

const INVPIO2: f64 = 6.36619772367581382433e-01;
const PIO2_1: f64 = 1.57079631090164184570e+00;
const PIO2_1T: f64 = 1.58932547735281966916e-08;

fn k_sin(x: f64) -> f64 {
    const S1: f64 = -0.166666666416265235595;
    const S2: f64 = 0.0083333293858894631756;
    const S3: f64 = -0.000198393348360966317347;
    const S4: f64 = 0.0000027183114939898219064;
    let z = x * x;
    let w = z * z;
    let r = S3 + z * S4;
    let s = z * x;
    (x + s * (S1 + z * S2)) + s * w * r
}

fn k_cos(x: f64) -> f64 {
    const C0: f64 = -0.499999997251031003120;
    const C1: f64 = 0.0416666233237390631894;
    const C2: f64 = -0.00138867637746099294692;
    const C3: f64 = 0.0000243904487962774090654;
    let z = x * x;
    let w = z * z;
    let r = C2 + z * C3;
    ((1.0 + z * C0) + w * C1) + (w * z) * r
}

fn reduce_pio2(x: f64) -> (i64, f64) {
    let n = (x * INVPIO2).round();
    let r = x - n * PIO2_1 - n * PIO2_1T;
    (n as i64 & 3, r)
}

pub fn sin(x: f32) -> f32 {
    if !x.is_finite() {
        return f32::NAN;
    }
    let (q, r) = reduce_pio2(x as f64);
    (match q {
        0 => k_sin(r),
        1 => k_cos(r),
        2 => -k_sin(r),
        _ => -k_cos(r),
    }) as f32
}

pub fn cos(x: f32) -> f32 {
    if !x.is_finite() {
        return f32::NAN;
    }
    let (q, r) = reduce_pio2(x as f64);
    (match q {
        0 => k_cos(r),
        1 => -k_sin(r),
        2 => -k_cos(r),
        _ => k_sin(r),
    }) as f32
}

pub fn sincos(x: f32) -> (f32, f32) {
    if !x.is_finite() {
        return (f32::NAN, f32::NAN);
    }
    let (q, r) = reduce_pio2(x as f64);
    let (s, c) = (k_sin(r), k_cos(r));
    let (s, c) = match q {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    };
    (s as f32, c as f32)
}

/// sin/cos quotient; deterministic, accuracy degrades near the poles.
pub fn tan(x: f32) -> f32 {
    let (s, c) = sincos(x);
    s / c
}

// ------------------------------------------------------------- atan / atan2

const ATANHI: [f64; 4] = [
    4.63647609000806093515e-01,
    7.85398163397448278999e-01,
    9.82793723247329054082e-01,
    1.57079632679489655800e+00,
];
const ATANLO: [f64; 4] = [
    2.26987774529616870924e-17,
    3.06161699786838301793e-17,
    1.39033110312309984516e-17,
    6.12323399573676603587e-17,
];
const AT: [f64; 11] = [
    3.33333333333329318027e-01,
    -1.99999999998764832476e-01,
    1.42857142725034663711e-01,
    -1.11111104054623557880e-01,
    9.09088713343650656196e-02,
    -7.69187620504482999495e-02,
    6.66107313738753120669e-02,
    -5.83357013379057348645e-02,
    4.97687799461593236017e-02,
    -3.65315727442169155270e-02,
    1.62858201153657823623e-02,
];

fn atan_d(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    let sign = x.is_sign_negative();
    let ax = x.abs();
    let (id, xr): (i32, f64) = if ax < 0.4375 {
        if ax < 1e-8 {
            return x;
        }
        (-1, x)
    } else if ax < 0.6875 {
        (0, (2.0 * ax - 1.0) / (2.0 + ax))
    } else if ax < 1.1875 {
        (1, (ax - 1.0) / (ax + 1.0))
    } else if ax < 2.4375 {
        (2, (ax - 1.5) / (1.0 + 1.5 * ax))
    } else if ax < 1e18 {
        (3, -1.0 / ax)
    } else {
        let v = ATANHI[3];
        return if sign { -v } else { v };
    };
    let z = xr * xr;
    let w = z * z;
    let s1 = z * (AT[0] + w * (AT[2] + w * (AT[4] + w * (AT[6] + w * (AT[8] + w * AT[10])))));
    let s2 = w * (AT[1] + w * (AT[3] + w * (AT[5] + w * (AT[7] + w * AT[9]))));
    if id < 0 {
        xr - xr * (s1 + s2)
    } else {
        let v = ATANHI[id as usize] - ((xr * (s1 + s2) - ATANLO[id as usize]) - xr);
        if sign {
            -v
        } else {
            v
        }
    }
}

fn atan2_d(y: f64, x: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    const PI: f64 = core::f64::consts::PI;
    const FRAC_PI_2: f64 = core::f64::consts::FRAC_PI_2;
    const FRAC_PI_4: f64 = core::f64::consts::FRAC_PI_4;
    if x.is_infinite() && y.is_infinite() {
        // fdlibm corner values, kept for determinism rather than any game need
        return match (x > 0.0, y > 0.0) {
            (true, true) => FRAC_PI_4,
            (true, false) => -FRAC_PI_4,
            (false, true) => 3.0 * FRAC_PI_4,
            (false, false) => -3.0 * FRAC_PI_4,
        };
    }
    if x == 0.0 {
        return if y > 0.0 {
            FRAC_PI_2
        } else if y < 0.0 {
            -FRAC_PI_2
        } else {
            0.0
        };
    }
    let a = atan_d(y / x);
    if x > 0.0 {
        a
    } else if y >= 0.0 {
        a + PI
    } else {
        a - PI
    }
}

pub fn atan(x: f32) -> f32 {
    atan_d(x as f64) as f32
}

pub fn atan2(y: f32, x: f32) -> f32 {
    atan2_d(y as f64, x as f64) as f32
}

pub fn asin(x: f32) -> f32 {
    if !(-1.0..=1.0).contains(&x) {
        return f32::NAN;
    }
    let xd = x as f64;
    atan2_d(xd, (1.0 - xd * xd).sqrt()) as f32
}

pub fn acos(x: f32) -> f32 {
    if !(-1.0..=1.0).contains(&x) {
        return f32::NAN;
    }
    let xd = x as f64;
    atan2_d((1.0 - xd * xd).sqrt(), xd) as f32
}

// ----------------------------------------------------------------- exp / ln

const LN2_HI: f64 = 6.93147180369123816490e-01;
const LN2_LO: f64 = 1.90821492927058770002e-10;
const INV_LN2: f64 = 1.44269504088896338700e+00;

fn exp_d(xd: f64) -> f64 {
    let k = (xd * INV_LN2).round();
    let r = xd - k * LN2_HI - k * LN2_LO;
    // degree-7 Taylor on |r| <= ln2/2: max rel err ~5e-9, far below f32 ulp
    let p = 1.0
        + r * (1.0
            + r * (0.5
                + r * (1.0 / 6.0
                    + r * (1.0 / 24.0
                        + r * (1.0 / 120.0 + r * (1.0 / 720.0 + r * (1.0 / 5040.0)))))));
    let scale = f64::from_bits(((1023 + k as i64) as u64) << 52);
    p * scale
}

pub fn exp(x: f32) -> f32 {
    if x.is_nan() {
        return f32::NAN;
    }
    if x > 88.8 {
        return f32::INFINITY;
    }
    if x < -104.0 {
        return 0.0;
    }
    exp_d(x as f64) as f32
}

const LG: [f64; 7] = [
    6.666666666666735130e-01,
    3.999999999940941908e-01,
    2.857142874366239149e-01,
    2.222219843214978396e-01,
    1.818357216161805012e-01,
    1.531383769920937332e-01,
    1.479819860511658591e-01,
];

fn ln_d(xd: f64) -> f64 {
    let bits = xd.to_bits();
    let mut k = ((bits >> 52) & 0x7ff) as i64 - 1023;
    let mut m = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | (1023u64 << 52));
    if m > core::f64::consts::SQRT_2 {
        m *= 0.5;
        k += 1;
    }
    let f = m - 1.0;
    let s = f / (2.0 + f);
    let z = s * s;
    let w = z * z;
    let t1 = w * (LG[1] + w * (LG[3] + w * LG[5]));
    let t2 = z * (LG[0] + w * (LG[2] + w * (LG[4] + w * LG[6])));
    let r = t1 + t2;
    let hfsq = 0.5 * f * f;
    let kd = k as f64;
    kd * LN2_HI - ((hfsq - (s * (hfsq + r) + kd * LN2_LO)) - f)
}

pub fn ln(x: f32) -> f32 {
    if x.is_nan() || x < 0.0 {
        return f32::NAN;
    }
    if x == 0.0 {
        return f32::NEG_INFINITY;
    }
    if x == f32::INFINITY {
        return f32::INFINITY;
    }
    ln_d(x as f64) as f32
}

/// exp(y·ln x) with the usual edge cases; negative base allowed for integer y.
pub fn pow(x: f32, y: f32) -> f32 {
    if y == 0.0 || x == 1.0 {
        return 1.0;
    }
    if x.is_nan() || y.is_nan() {
        return f32::NAN;
    }
    if x == 0.0 {
        return if y > 0.0 { 0.0 } else { f32::INFINITY };
    }
    let yd = y as f64;
    if x < 0.0 {
        if yd.fract() != 0.0 {
            return f32::NAN;
        }
        let odd = (yd as i64) & 1 == 1;
        let m = pow_pos(-x, yd);
        return if odd { -m } else { m };
    }
    pow_pos(x, yd)
}

fn pow_pos(x: f32, yd: f64) -> f32 {
    let t = yd * ln_d(x as f64);
    if t > 128.0 * core::f64::consts::LN_2 {
        return f32::INFINITY;
    }
    if t < -150.0 * core::f64::consts::LN_2 {
        return 0.0;
    }
    exp_d(t) as f32
}

// ------------------------------------------------------------- exact & misc

/// IEEE sqrt is exactly rounded on every platform — passthrough, documented.
#[inline]
pub fn sqrt(x: f32) -> f32 {
    x.sqrt()
}

/// sqrt(x² + y²) with exact f64 squares (f32 products are exact in f64):
/// one rounding for the sum, exact sqrt, one rounding to f32. No overflow.
pub fn hypot(x: f32, y: f32) -> f32 {
    let (xd, yd) = (x as f64, y as f64);
    (xd * xd + yd * yd).sqrt() as f32
}

/// Floor-division float modulo: result has the sign of `b`, in [0, b) for
/// b > 0. Built from exact ops only.
pub fn fmod_floor(a: f32, b: f32) -> f32 {
    if b == 0.0 || !a.is_finite() || !b.is_finite() {
        return f32::NAN;
    }
    let (ad, bd) = (a as f64, b as f64);
    (ad - (ad / bd).floor() * bd) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- accuracy vs std (tolerances cover std's own cross-platform ulp) --

    fn sweep(n: usize, lo: f32, hi: f32) -> impl Iterator<Item = f32> {
        let step = ((hi - lo) as f64) / (n as f64);
        (0..n).map(move |i| (lo as f64 + step * i as f64) as f32)
    }

    #[test]
    fn sin_cos_match_std_within_tolerance() {
        for x in sweep(200_000, -1000.0, 1000.0) {
            assert!((sin(x) - x.sin()).abs() < 1e-5, "sin({x})");
            assert!((cos(x) - x.cos()).abs() < 1e-5, "cos({x})");
        }
        for x in sweep(100_000, -0.1, 0.1) {
            assert!((sin(x) - x.sin()).abs() < 1e-7, "sin({x})");
            assert!((cos(x) - x.cos()).abs() < 1e-7, "cos({x})");
        }
    }

    #[test]
    fn sincos_identity_and_quadrants() {
        for x in sweep(100_000, -50.0, 50.0) {
            let (s, c) = sincos(x);
            assert_eq!(s, sin(x));
            assert_eq!(c, cos(x));
            assert!((s * s + c * c - 1.0).abs() < 1e-6);
        }
        assert_eq!(sin(0.0), 0.0);
        assert_eq!(cos(0.0), 1.0);
    }

    #[test]
    fn atan_atan2_match_std() {
        for x in sweep(200_000, -100.0, 100.0) {
            assert!((atan(x) - x.atan()).abs() < 1e-6, "atan({x})");
        }
        for y in sweep(450, -10.0, 10.0) {
            for x in sweep(450, -10.0, 10.0) {
                let got = atan2(y, x);
                let want = y.atan2(x);
                // std atan2(±0, -x) returns ±π; we return +π at y == 0 exactly
                if y == 0.0 && x < 0.0 {
                    continue;
                }
                assert!((got - want).abs() < 1e-6, "atan2({y},{x}) {got} {want}");
            }
        }
        assert_eq!(atan2(0.0, 1.0), 0.0);
        assert!((atan2(1.0, 0.0) - core::f32::consts::FRAC_PI_2).abs() < 1e-7);
    }

    #[test]
    fn asin_acos_match_std() {
        for x in sweep(100_000, -1.0, 1.0) {
            assert!((asin(x) - x.asin()).abs() < 1e-6, "asin({x})");
            assert!((acos(x) - x.acos()).abs() < 1e-6, "acos({x})");
        }
        assert!(asin(1.5).is_nan());
        assert!(acos(-1.5).is_nan());
    }

    #[test]
    fn exp_ln_pow_match_std() {
        for x in sweep(100_000, -80.0, 80.0) {
            let got = exp(x);
            let want = x.exp();
            let rel = ((got - want) / want).abs();
            assert!(rel < 1e-6, "exp({x}) {got} {want}");
        }
        for x in sweep(100_000, 1e-4, 1e4) {
            let got = ln(x);
            let want = x.ln();
            assert!((got - want).abs() < 1e-5, "ln({x}) {got} {want}");
        }
        for x in sweep(300, 0.01, 40.0) {
            for y in sweep(300, -8.0, 8.0) {
                let got = pow(x, y);
                let want = x.powf(y);
                let rel = ((got - want) / want).abs();
                assert!(rel < 1e-5, "pow({x},{y}) {got} {want}");
            }
        }
        assert_eq!(exp(0.0), 1.0);
        assert_eq!(ln(1.0), 0.0);
        assert_eq!(pow(2.0, 10.0), 1024.0);
        assert_eq!(pow(-2.0, 3.0), -8.0);
        assert_eq!(pow(-2.0, 2.0), 4.0);
        assert!(pow(-2.0, 0.5).is_nan());
        assert_eq!(ln(0.0), f32::NEG_INFINITY);
    }

    #[test]
    fn misc_ops() {
        assert_eq!(hypot(3.0, 4.0), 5.0);
        assert_eq!(fmod_floor(7.5, 2.0), 1.5);
        assert_eq!(fmod_floor(-0.5, 2.0), 1.5);
        assert!(fmod_floor(1.0, 0.0).is_nan());
        assert!((tan(0.5) - 0.5f32.tan()).abs() < 1e-6);
    }

    #[test]
    fn non_finite_inputs_are_handled() {
        assert!(sin(f32::INFINITY).is_nan());
        assert!(cos(f32::NAN).is_nan());
        assert!(atan2(f32::NAN, 1.0).is_nan());
        assert_eq!(atan(f32::INFINITY), core::f32::consts::FRAC_PI_2);
        assert_eq!(exp(f32::NEG_INFINITY), 0.0);
        assert_eq!(exp(1000.0), f32::INFINITY);
    }

    // ---- cross-arch bit-parity goldens ------------------------------------
    // These hashes were recorded on aarch64; the SAME constants must hold on
    // x86_64 and every other target — that equality IS the determinism claim.
    // If a kernel changes intentionally, re-record with `print_parity_hashes`.

    const FNV_OFFSET: u64 = 0xcbf29ce484222325;

    fn fnv(h: u64, v: u32) -> u64 {
        (h ^ v as u64).wrapping_mul(0x100000001b3)
    }

    fn hash_unary(f: fn(f32) -> f32) -> u64 {
        let mut h = FNV_OFFSET;
        for i in 0..200_000u32 {
            let x = (i as f64 * 0.01 - 1000.0) as f32;
            h = fnv(h, f(x).to_bits());
            let x = (i as f64 * 1e-6 - 0.1) as f32;
            h = fnv(h, f(x).to_bits());
        }
        h
    }

    fn hash_binary(f: fn(f32, f32) -> f32, alo: f32, ahi: f32, blo: f32, bhi: f32) -> u64 {
        let mut h = FNV_OFFSET;
        for i in 0..450u32 {
            let a = (alo as f64 + (ahi - alo) as f64 * i as f64 / 450.0) as f32;
            for j in 0..450u32 {
                let b = (blo as f64 + (bhi - blo) as f64 * j as f64 / 450.0) as f32;
                h = fnv(h, f(a, b).to_bits());
            }
        }
        h
    }

    fn parity_hashes() -> [(&'static str, u64); 8] {
        [
            ("sin", hash_unary(sin)),
            ("cos", hash_unary(cos)),
            ("atan", hash_unary(atan)),
            ("exp", hash_unary(exp)),
            ("ln", hash_unary(|x| ln(x.abs() + 1e-6))),
            ("atan2", hash_binary(atan2, -10.0, 10.0, -10.0, 10.0)),
            ("pow", hash_binary(|x, y| pow(x.abs() + 0.01, y), 0.0, 40.0, -8.0, 8.0)),
            ("hypot", hash_binary(hypot, -100.0, 100.0, -100.0, 100.0)),
        ]
    }

    #[test]
    #[ignore = "run manually to (re)record the golden hashes below"]
    fn print_parity_hashes() {
        for (name, h) in parity_hashes() {
            println!("(\"{name}\", 0x{h:016x}),");
        }
    }

    #[test]
    fn parity_hashes_match_golden() {
        const GOLDEN: [(&str, u64); 8] = [
            ("sin", 0xdaa54d37d0373775),
            ("cos", 0xd46f1ce6ffd67b76),
            ("atan", 0x2637c650a558b697),
            ("exp", 0x6028f9ecf672fec0),
            ("ln", 0x83e8747eb068923e),
            ("atan2", 0xd36c7eec6c051ed4),
            ("pow", 0x9a53615fa6888cc0),
            ("hypot", 0x1602cd23e5fd4803),
        ];
        for ((name, got), (gname, want)) in parity_hashes().iter().zip(GOLDEN.iter()) {
            assert_eq!(name, gname);
            assert_eq!(
                got, want,
                "{name}: parity hash changed — cross-arch determinism broken or kernel edited"
            );
        }
    }
}

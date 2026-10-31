//! Portable transcendental functions: the same bits on every platform.
//!
//! IEEE 754 fixes +, -, *, / and sqrt to the correctly rounded result, but
//! not sin, cos, exp, ln, pow, hypot and friends: Rust's `f64::sin` lowers
//! to the platform libm, and Apple's and glibc's disagree in the last bit.
//! Geometry built with them hashes differently on a Mac and on Linux, so a
//! content-addressed model store splits and every cache built on one
//! machine misses on the other.
//!
//! Everything here uses only exactly rounded operations and fixed fdlibm
//! polynomial kernels (Rust never contracts `a*b + c` into an fma, so the
//! evaluation order is fixed). Accuracy is about one ulp for arguments of
//! modelling size; sin/cos degrade (still deterministically) past ~1e6
//! radians. `f32` versions evaluate in `f64` and round once.
//!
//! Use the [`PortableFloat`] methods (`x.psin()`, `y.patan2(x)`, ...) in any
//! code whose output is content: geometry, texture pixels, serialized
//! numbers.

// fdlibm split constants and coefficients: their exact spellings are the kernel.
#![allow(clippy::approx_constant, clippy::excessive_precision)]

const INVPIO2: f64 = 6.36619772367581382433e-01;
const PIO2_1: f64 = 1.57079632673412561417e+00;
const PIO2_2: f64 = 6.07710050630396597660e-11;
const PIO2_2T: f64 = 2.02226624879595063154e-21;

fn k_sin(x: f64) -> f64 {
    const S1: f64 = -1.66666666666666324348e-01;
    const S2: f64 = 8.33333333332248946124e-03;
    const S3: f64 = -1.98412698298579493134e-04;
    const S4: f64 = 2.75573137070700676789e-06;
    const S5: f64 = -2.50507602534068634195e-08;
    const S6: f64 = 1.58969099521155010221e-10;
    let z = x * x;
    let v = z * x;
    let r = S2 + z * (S3 + z * (S4 + z * (S5 + z * S6)));
    x + v * (S1 + z * r)
}

fn k_cos(x: f64) -> f64 {
    const C1: f64 = 4.16666666666666019037e-02;
    const C2: f64 = -1.38888888888741095749e-03;
    const C3: f64 = 2.48015872894767294178e-05;
    const C4: f64 = -2.75573143513906633035e-07;
    const C5: f64 = 2.08757232129817482790e-09;
    const C6: f64 = -1.13596475577881948265e-11;
    let z = x * x;
    let w = z * z;
    let r = z * (C1 + z * (C2 + z * C3)) + w * w * (C4 + z * (C5 + z * C6));
    let hz = 0.5 * z;
    let w = 1.0 - hz;
    w + (((1.0 - w) - hz) + z * r)
}

/// x = n·π/2 + r, |r| ≤ π/4 (three-part Cody-Waite), n mod 4.
fn reduce(x: f64) -> (u8, f64) {
    if x.abs() <= core::f64::consts::FRAC_PI_4 {
        return (0, x);
    }
    let n = (x * INVPIO2).round();
    let y = x - n * PIO2_1;
    let w = n * PIO2_2;
    let r = y - w;
    let w2 = n * PIO2_2T - ((y - r) - w);
    ((n.rem_euclid(4.0)) as u8, r - w2)
}

pub fn sin_cos(x: f64) -> (f64, f64) {
    if !x.is_finite() {
        return (f64::NAN, f64::NAN);
    }
    let (q, r) = reduce(x);
    let (s, c) = (k_sin(r), k_cos(r));
    match q {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    }
}

pub fn sin(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    let (q, r) = reduce(x);
    match q {
        0 => k_sin(r),
        1 => k_cos(r),
        2 => -k_sin(r),
        _ => -k_cos(r),
    }
}

pub fn cos(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    let (q, r) = reduce(x);
    match q {
        0 => k_cos(r),
        1 => -k_sin(r),
        2 => -k_cos(r),
        _ => k_sin(r),
    }
}

pub fn tan(x: f64) -> f64 {
    let (s, c) = sin_cos(x);
    s / c
}

const ATANHI: [f64; 4] = [4.63647609000806093515e-01, 7.85398163397448278999e-01, 9.82793723247329054082e-01, 1.57079632679489655800e+00];
const ATANLO: [f64; 4] = [2.26987774529616870924e-17, 3.06161699786838301793e-17, 1.39033110312309984516e-17, 6.12323399573676603587e-17];
const AT: [f64; 11] = [
    3.33333333333329318027e-01, -1.99999999998764832476e-01, 1.42857142725034663711e-01, -1.11111104054623557880e-01,
    9.09088713343650656196e-02, -7.69187620504482999495e-02, 6.66107313738753120669e-02, -5.83357013379057348645e-02,
    4.97687799461593236017e-02, -3.65315727442169155270e-02, 1.62858201153657823623e-02,
];

pub fn atan(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    let sign = x.is_sign_negative();
    let ax = x.abs();
    let (id, xr): (i32, f64) = if ax < 0.4375 {
        if ax < 1e-27 {
            return x;
        }
        (-1, x)
    } else if ax < 0.6875 {
        (0, (2.0 * ax - 1.0) / (2.0 + ax))
    } else if ax < 1.1875 {
        (1, (ax - 1.0) / (ax + 1.0))
    } else if ax < 2.4375 {
        (2, (ax - 1.5) / (1.0 + 1.5 * ax))
    } else if ax < 2f64.powi(66) {
        (3, -1.0 / ax)
    } else {
        return if sign { -ATANHI[3] } else { ATANHI[3] };
    };
    let z = xr * xr;
    let w = z * z;
    let s1 = z * (AT[0] + w * (AT[2] + w * (AT[4] + w * (AT[6] + w * (AT[8] + w * AT[10])))));
    let s2 = w * (AT[1] + w * (AT[3] + w * (AT[5] + w * (AT[7] + w * AT[9]))));
    if id < 0 {
        return xr - xr * (s1 + s2);
    }
    let v = ATANHI[id as usize] - ((xr * (s1 + s2) - ATANLO[id as usize]) - xr);
    if sign {
        -v
    } else {
        v
    }
}

pub fn atan2(y: f64, x: f64) -> f64 {
    use core::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    let neg_y = y.is_sign_negative();
    let signed = |v: f64| if neg_y { -v } else { v };
    if y == 0.0 {
        return if x.is_sign_negative() { signed(PI) } else { y };
    }
    if x == 0.0 {
        return signed(FRAC_PI_2);
    }
    if x.is_infinite() {
        return match (x > 0.0, y.is_infinite()) {
            (true, true) => signed(FRAC_PI_4),
            (false, true) => signed(3.0 * FRAC_PI_4),
            (true, false) => signed(0.0),
            (false, false) => signed(PI),
        };
    }
    if y.is_infinite() {
        return signed(FRAC_PI_2);
    }
    let a = atan((y / x).abs());
    match (x > 0.0, neg_y) {
        (true, false) => a,
        (true, true) => -a,
        (false, false) => PI - a,
        (false, true) => a - PI,
    }
}

pub fn asin(x: f64) -> f64 {
    if !(-1.0..=1.0).contains(&x) {
        return f64::NAN;
    }
    atan2(x, ((1.0 - x) * (1.0 + x)).sqrt())
}

pub fn acos(x: f64) -> f64 {
    if !(-1.0..=1.0).contains(&x) {
        return f64::NAN;
    }
    atan2(((1.0 - x) * (1.0 + x)).sqrt(), x)
}

const LN2_HI: f64 = 6.93147180369123816490e-01;
const LN2_LO: f64 = 1.90821492927058770002e-10;
const INV_LN2: f64 = 1.44269504088896338700e+00;

/// y·2^k by exact power-of-two steps (no libm `ldexp`).
fn scalbn(mut y: f64, mut k: i32) -> f64 {
    let step = |e: i32| f64::from_bits(((1023 + e) as u64) << 52);
    while k > 1023 {
        y *= step(1023);
        k -= 1023;
    }
    while k < -1022 {
        y *= step(-1022);
        k += 1022;
    }
    y * step(k)
}

pub fn exp(x: f64) -> f64 {
    const P1: f64 = 1.66666666666666019037e-01;
    const P2: f64 = -2.77777777770155933842e-03;
    const P3: f64 = 6.61375632143793436117e-05;
    const P4: f64 = -1.65339022054652515390e-06;
    const P5: f64 = 4.13813679705723846039e-08;
    if x.is_nan() {
        return f64::NAN;
    }
    if x > 709.782712893383973096 {
        return f64::INFINITY;
    }
    if x < -745.13321910194110842 {
        return 0.0;
    }
    if x.abs() < 2f64.powi(-28) {
        return 1.0 + x;
    }
    let k = (x * INV_LN2).round();
    let hi = x - k * LN2_HI;
    let lo = k * LN2_LO;
    let r = hi - lo;
    let t = r * r;
    let c = r - t * (P1 + t * (P2 + t * (P3 + t * (P4 + t * P5))));
    let y = 1.0 - ((lo - (r * c) / (2.0 - c)) - hi);
    scalbn(y, k as i32)
}

pub fn ln(x: f64) -> f64 {
    const LG: [f64; 7] = [
        6.666666666666735130e-01, 3.999999999940941908e-01, 2.857142874366239149e-01, 2.222219843214978396e-01,
        1.818357216161805012e-01, 1.531383769920937332e-01, 1.479819860511658591e-01,
    ];
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x.is_infinite() {
        return f64::INFINITY;
    }
    // Subnormals: scale into the normal range first (exact).
    let (x, bias) = if x < f64::MIN_POSITIVE { (x * 2f64.powi(54), -54) } else { (x, 0) };
    let bits = x.to_bits();
    let mut k = ((bits >> 52) & 0x7ff) as i64 - 1023 + bias;
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

/// x^y. Integer exponents up to 64 are repeated multiplication (exact where
/// the product is); the rest is exp(y·ln x).
pub fn powf(x: f64, y: f64) -> f64 {
    if y == 0.0 || x == 1.0 {
        return 1.0;
    }
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if y.fract() == 0.0 && y.abs() <= 64.0 {
        let mut n = y.abs() as u32;
        let (mut base, mut acc) = (x, 1.0);
        while n > 0 {
            if n & 1 == 1 {
                acc *= base;
            }
            base *= base;
            n >>= 1;
        }
        return if y < 0.0 { 1.0 / acc } else { acc };
    }
    if x == 0.0 {
        return if y > 0.0 { 0.0 } else { f64::INFINITY };
    }
    if x < 0.0 {
        if y.fract() != 0.0 {
            return f64::NAN;
        }
        let odd = y.abs() < 2f64.powi(53) && (y as i64) & 1 == 1;
        let m = exp(y * ln(-x));
        return if odd { -m } else { m };
    }
    exp(y * ln(x))
}

pub fn hypot(x: f64, y: f64) -> f64 {
    let (a, b) = (x.abs(), y.abs());
    if a.is_infinite() || b.is_infinite() {
        return f64::INFINITY;
    }
    let big = a.max(b);
    // Scale by an exact power of two away from overflow and underflow.
    let scale = if big > 2f64.powi(500) { 2f64.powi(-600) } else if big < 2f64.powi(-500) && big > 0.0 { 2f64.powi(600) } else { 1.0 };
    let (a, b) = (a * scale, b * scale);
    (a * a + b * b).sqrt() / scale
}

pub fn cbrt(x: f64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let a = x.abs();
    let mut y = exp(ln(a) / 3.0);
    y -= (y * y * y - a) / (3.0 * y * y);
    if x < 0.0 {
        -y
    } else {
        y
    }
}

/// Method spellings of the functions above, for `f64` and `f32` (the `f32`
/// forms evaluate in `f64` and round once).
pub trait PortableFloat: Copy {
    fn psin(self) -> Self;
    fn pcos(self) -> Self;
    fn psin_cos(self) -> (Self, Self);
    fn ptan(self) -> Self;
    fn patan(self) -> Self;
    fn patan2(self, x: Self) -> Self;
    fn pasin(self) -> Self;
    fn pacos(self) -> Self;
    fn pexp(self) -> Self;
    fn pln(self) -> Self;
    fn ppowf(self, y: Self) -> Self;
    fn phypot(self, y: Self) -> Self;
    fn pcbrt(self) -> Self;
}

impl PortableFloat for f64 {
    fn psin(self) -> f64 { sin(self) }
    fn pcos(self) -> f64 { cos(self) }
    fn psin_cos(self) -> (f64, f64) { sin_cos(self) }
    fn ptan(self) -> f64 { tan(self) }
    fn patan(self) -> f64 { atan(self) }
    fn patan2(self, x: f64) -> f64 { atan2(self, x) }
    fn pasin(self) -> f64 { asin(self) }
    fn pacos(self) -> f64 { acos(self) }
    fn pexp(self) -> f64 { exp(self) }
    fn pln(self) -> f64 { ln(self) }
    fn ppowf(self, y: f64) -> f64 { powf(self, y) }
    fn phypot(self, y: f64) -> f64 { hypot(self, y) }
    fn pcbrt(self) -> f64 { cbrt(self) }
}

impl PortableFloat for f32 {
    fn psin(self) -> f32 { sin(self as f64) as f32 }
    fn pcos(self) -> f32 { cos(self as f64) as f32 }
    fn psin_cos(self) -> (f32, f32) { let (s, c) = sin_cos(self as f64); (s as f32, c as f32) }
    fn ptan(self) -> f32 { tan(self as f64) as f32 }
    fn patan(self) -> f32 { atan(self as f64) as f32 }
    fn patan2(self, x: f32) -> f32 { atan2(self as f64, x as f64) as f32 }
    fn pasin(self) -> f32 { asin(self as f64) as f32 }
    fn pacos(self) -> f32 { acos(self as f64) as f32 }
    fn pexp(self) -> f32 { exp(self as f64) as f32 }
    fn pln(self) -> f32 { ln(self as f64) as f32 }
    fn ppowf(self, y: f32) -> f32 { powf(self as f64, y as f64) as f32 }
    fn phypot(self, y: f32) -> f32 { hypot(self as f64, y as f64) as f32 }
    fn pcbrt(self) -> f32 { cbrt(self as f64) as f32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sweep(n: usize, lo: f64, hi: f64) -> impl Iterator<Item = f64> {
        (0..=n).map(move |i| lo + (hi - lo) * i as f64 / n as f64)
    }

    /// Relative-or-absolute closeness to the platform libm (which is itself
    /// off by up to an ulp; the point is portability, not a new libm).
    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    #[test]
    fn trig_matches_libm_to_a_few_ulp() {
        for x in sweep(200_000, -100.0, 100.0) {
            assert!(close(sin(x), x.sin(), 4e-16), "sin({x})");
            assert!(close(cos(x), x.cos(), 4e-16), "cos({x})");
            let (s, c) = sin_cos(x);
            assert_eq!((s, c), (sin(x), cos(x)));
        }
        for x in sweep(20_000, -30.0, 30.0) {
            assert!(close(atan(x), x.atan(), 4e-16), "atan({x})");
            assert!(close(atan2(x, 1.3), x.atan2(1.3), 4e-16) && close(atan2(x, -0.7), x.atan2(-0.7), 4e-16), "atan2({x})");
        }
        for x in sweep(20_000, -1.0, 1.0) {
            assert!(close(asin(x), x.asin(), 2e-15), "asin({x})");
            assert!(close(acos(x), x.acos(), 2e-15), "acos({x})");
        }
        assert_eq!(atan2(0.0, -1.0), core::f64::consts::PI);
        assert_eq!(atan2(-0.0, -1.0), -core::f64::consts::PI);
        assert_eq!(sin(0.0), 0.0);
        assert_eq!(cos(0.0), 1.0);
    }

    #[test]
    fn exp_ln_pow_hypot_cbrt_match_libm() {
        for x in sweep(50_000, -700.0, 700.0) {
            assert!(close(exp(x), x.exp(), 4e-16) || (exp(x) / x.exp() - 1.0).abs() < 4e-16, "exp({x})");
        }
        for x in sweep(50_000, 1e-6, 1e6) {
            assert!(close(ln(x), x.ln(), 4e-16), "ln({x})");
            assert!((powf(x, 0.37) / x.powf(0.37) - 1.0).abs() < 4e-15, "pow({x})");
            assert!((cbrt(x) / x.cbrt() - 1.0).abs() < 4e-16, "cbrt({x})");
            assert!((hypot(x, 3.0) / x.hypot(3.0) - 1.0).abs() < 4e-16, "hypot({x})");
        }
        assert_eq!(powf(2.0, 10.0), 1024.0);
        assert_eq!(powf(-3.0, 3.0), -27.0);
        assert_eq!(cbrt(-8.0), -2.0);
        assert_eq!(hypot(3.0, 4.0), 5.0);
        assert_eq!(ln(1.0), 0.0);
        assert_eq!(exp(0.0), 1.0);
        assert!(ln(f64::MIN_POSITIVE / 8.0).is_finite());
    }

    /// Bit patterns recorded on one machine; any platform must reproduce them.
    #[test]
    fn results_are_bit_stable() {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for x in sweep(10_000, -20.0, 20.0) {
            for v in [sin(x), cos(x), atan(x), exp(x * 0.5), ln(x.abs() + 1e-3), powf(x.abs() + 0.5, 1.7), hypot(x, 0.3), cbrt(x)] {
                h = (h ^ v.to_bits()).wrapping_mul(0x100_0000_01b3);
            }
        }
        assert_eq!(h, PARITY, "portable math changed bits: {h:#018x}");
    }
    const PARITY: u64 = 0x32bf_ff2b_42f3_9a8d;
}

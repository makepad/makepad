//! The node kernels' math, op for op, so local tiles are bit-identical to
//! the nodes' (the hub's Splash kernels, `libs/ai/hub/src/kernels`).
//!
//! Splash lowers `ln` and `atan2` on f64 to its portable kernels
//! (platform/script/compute/src/lower/portable.rs: `p_ln`, `p_atan2`) and
//! `log2` to its f32 log (lower.rs `log`, times LOG2_E in f32, the argument
//! rounded once to f32). Rust's own `ln`/`log2`/`atan2` come from the
//! platform's libm and differ from those in the last bit now and then, which
//! moves a smooth count or a distance code by one step.

/// Splash's f64 `ln` (`p_ln`).
pub fn ln(x: f64) -> f64 {
    const LG: [f64; 7] = [
        6.666666666666735130e-01, 3.999999999940941908e-01, 2.857142874366239149e-01, 2.222219843214978396e-01,
        1.818357216161805012e-01, 1.531383769920937332e-01, 1.479819860511658591e-01,
    ];
    const LN2_HI: f64 = 6.93147180369123816490e-01;
    const LN2_LO: f64 = 1.90821492927058770002e-10;
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x == f64::INFINITY {
        return f64::INFINITY;
    }
    let sub = x < f64::MIN_POSITIVE;
    let xs = if sub { x * 2f64.powi(54) } else { x };
    let bias = if sub { -54 } else { 0 };
    let bits = xs.to_bits();
    let hi = (bits >> 32) as u32;
    let lo = bits as u32;
    let e = ((hi >> 20) & 0x7ff) as i32;
    let mut k = e - 1023 + bias;
    let mh = (hi & 0x000f_ffff) | 0x3ff0_0000;
    let mut m = f64::from_bits(((mh as u64) << 32) | lo as u64);
    if m > std::f64::consts::SQRT_2 {
        m *= 0.5;
        k += 1;
    }
    let f = m - 1.0;
    let s = f / (2.0 + f);
    let z = s * s;
    let w = z * z;
    let t1 = w * horner(w, &[LG[5], LG[3], LG[1]]);
    let t2 = z * horner(w, &[LG[6], LG[4], LG[2], LG[0]]);
    let r = t1 + t2;
    let hfsq = 0.5 * f * f;
    let kd = k as f64;
    kd * LN2_HI - ((hfsq - (s * (hfsq + r) + kd * LN2_LO)) - f)
}

/// `horner_d`: c0, then acc = c + z * acc for the rest.
fn horner(z: f64, inner_first: &[f64]) -> f64 {
    let mut acc = inner_first[0];
    for c in &inner_first[1..] {
        acc = c + z * acc;
    }
    acc
}

/// Splash's f32 natural log (lower.rs `log`): x <= 0 gives -inf.
fn log_f32(x: f32) -> f32 {
    const P: [f32; 9] = [
        7.037_683_6e-2, -1.151_461e-1, 1.167_699_9e-1, -1.242_014_1e-1, 1.424_932_3e-1, -1.666_805_8e-1, 2.000_071_4e-1, -2.499_999_4e-1,
        3.333_333e-1,
    ];
    let bits = x.to_bits() as i32;
    let mut e = (bits >> 23) - 126;
    let m = f32::from_bits(((bits & 0x807f_ffffu32 as i32) | 0x3f00_0000) as u32);
    let small = m < std::f32::consts::FRAC_1_SQRT_2;
    if small {
        e -= 1;
    }
    let x1 = if small { (m + m) + -1.0 } else { m + -1.0 };
    let z = x1 * x1;
    let mut p = P[0];
    for c in &P[1..] {
        p = p * x1 + c;
    }
    let p = p * x1;
    let y = p * z;
    let fe = e as f32;
    let y = y + fe * -2.121_944_4e-4;
    let y = y - z * 0.5;
    let r = x1 + y;
    let r = r + fe * 0.693_359_4;
    if x > 0.0 {
        r
    } else {
        f32::NEG_INFINITY
    }
}

/// Splash's `log2` of an f64: in f32, the argument rounded once.
pub fn log2(x: f64) -> f32 {
    log_f32(x as f32) * std::f32::consts::LOG2_E
}

/// Splash's f64 `atan` (`p_atan`).
fn atan(x: f64) -> f64 {
    const ATANHI: [f64; 4] = [4.63647609000806093515e-01, 7.85398163397448278999e-01, 9.82793723247329054082e-01, 1.57079632679489655800e+00];
    const ATANLO: [f64; 4] = [2.26987774529616870924e-17, 3.06161699786838301793e-17, 1.39033110312309984516e-17, 6.12323399573676603587e-17];
    const AT: [f64; 11] = [
        3.33333333333329318027e-01, -1.99999999998764832476e-01, 1.42857142725034663711e-01, -1.11111104054623557880e-01,
        9.09088713343650656196e-02, -7.69187620504482999495e-02, 6.66107313738753120669e-02, -5.83357013379057348645e-02,
        4.97687799461593236017e-02, -3.65315727442169155270e-02, 1.62858201153657823623e-02,
    ];
    if x.is_nan() {
        return f64::NAN;
    }
    let sign = x.is_sign_negative();
    let ax = x.abs();
    if ax < 1e-27 {
        return x;
    }
    if ax >= 2f64.powi(66) {
        return if sign { -ATANHI[3] } else { ATANHI[3] };
    }
    let (xr, id) = if ax < 0.4375 {
        (x, -1)
    } else if ax < 0.6875 {
        ((2.0 * ax - 1.0) / (2.0 + ax), 0)
    } else if ax < 1.1875 {
        ((ax - 1.0) / (ax + 1.0), 1)
    } else if ax < 2.4375 {
        ((ax - 1.5) / (1.0 + 1.5 * ax), 2)
    } else {
        (-1.0 / ax, 3)
    };
    let z = xr * xr;
    let w = z * z;
    let s1 = z * horner(w, &[AT[10], AT[8], AT[6], AT[4], AT[2], AT[0]]);
    let s2 = w * horner(w, &[AT[9], AT[7], AT[5], AT[3], AT[1]]);
    let xs = xr * (s1 + s2);
    if id < 0 {
        return xr - xs;
    }
    let v = ATANHI[id as usize] - ((xs - ATANLO[id as usize]) - xr);
    if sign {
        -v
    } else {
        v
    }
}

/// Splash's f64 `atan2` (`p_atan2`).
pub fn atan2(y: f64, x: f64) -> f64 {
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};
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
    let yinf = y.is_infinite();
    if x.is_infinite() {
        return if x > 0.0 {
            if yinf { signed(FRAC_PI_4) } else { signed(0.0) }
        } else if yinf {
            signed(3.0 * FRAC_PI_4)
        } else {
            signed(PI)
        };
    }
    if yinf {
        return signed(FRAC_PI_2);
    }
    let a = atan((y / x).abs());
    if x > 0.0 {
        if neg_y { -a } else { a }
    } else if neg_y {
        a - PI
    } else {
        PI - a
    }
}

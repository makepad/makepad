//! f64 maths with the same bits on every platform, built only from exactly-rounded operations.
//! Ported from musl's maths library by way of rust-lang/libm 0.2.16 (`libs/rapier/vendor/libm`).

// Copyright (c) 2018 Jorge Aparicio
// Copyright © 2005-2020 Rich Felker, et al.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
//
// Much of the math library code (src/math/* and src/complex/*) is
// Copyright © 1993,2004 Sun Microsystems or
// Copyright © 2003-2011 David Schultz or
// Copyright © 2003-2009 Steven G. Kargl or
// Copyright © 2003-2009 Bruce D. Evans or
// Copyright © 2008 Stephen L. Moshier or
// Copyright © 2017-2018 Arm Limited
// and labelled as such in comments in the individual source files. All
// have been licensed under extremely permissive terms.

#[inline]
fn get_high_word(x: f64) -> u32 {
    (x.to_bits() >> 32) as u32
}

#[inline]
fn get_low_word(x: f64) -> u32 {
    x.to_bits() as u32
}

#[inline]
fn with_set_high_word(f: f64, hi: u32) -> f64 {
    let mut tmp = f.to_bits();
    tmp &= 0x00000000_ffffffff;
    tmp |= (hi as u64) << 32;
    f64::from_bits(tmp)
}

#[inline]
fn with_set_low_word(f: f64, lo: u32) -> f64 {
    let mut tmp = f.to_bits();
    tmp &= 0xffffffff_00000000;
    tmp |= lo as u64;
    f64::from_bits(tmp)
}

#[inline]
fn combine_words(hi: u32, lo: u32) -> f64 {
    f64::from_bits(((hi as u64) << 32) | lo as u64)
}

/// Scale the exponent.
///
/// From N3220:
///
/// > The scalbn and scalbln functions compute `x * b^n`, where `b = FLT_RADIX` if the return type
/// > of the function is a standard floating type, or `b = 10` if the return type of the function
/// > is a decimal floating type. A range error occurs for some finite x, depending on n.
/// >
/// > [...]
/// >
/// > * `scalbn(±0, n)` returns `±0`.
/// > * `scalbn(x, 0)` returns `x`.
/// > * `scalbn(±∞, n)` returns `±∞`.
/// >
/// > If the calculation does not overflow or underflow, the returned value is exact and
/// > independent of the current rounding direction mode.
#[inline]
fn scalbn(mut x: f64, mut n: i32) -> f64 {
    // Bits including the implicit bit
    let sig_total_bits = 52 + 1;

    // Maximum and minimum values when biased
    let exp_max = 1023;
    let exp_min = -1022;

    // 2 ^ Emax, maximum positive with null significand (0x1p1023 for f64)
    let f_exp_max = f64::from_bits(0x7fe0_0000_0000_0000); // 2^1023

    // 2 ^ Emin, minimum positive normal with null significand (0x1p-1022 for f64)
    let f_exp_min = f64::from_bits(0x0010_0000_0000_0000); // 2^-1022

    // 2 ^ sig_total_bits, moltiplier to normalize subnormals (0x1p53 for f64)
    let f_pow_subnorm = f64::from_bits(0x4340_0000_0000_0000); // 2^53

    /*
     * The goal is to multiply `x` by a scale factor that applies `n`. However, there are cases
     * where `2^n` is not representable by `f64` but the result should be, e.g. `x = 2^Emin` with
     * `n = -EMin + 2` (one out of range of 2^Emax). To get around this, reduce the magnitude of
     * the final scale operation by prescaling by the max/min power representable by `f64`.
     */

    if n > exp_max {
        // Worse case positive `n`: `x`  is the minimum subnormal value, the result is `f64::MAX`.
        // This can be reached by three scaling multiplications (two here and one final).
        debug_assert!(-exp_min + 52 + exp_max <= exp_max * 3);

        x *= f_exp_max;
        n -= exp_max;
        if n > exp_max {
            x *= f_exp_max;
            n -= exp_max;
            if n > exp_max {
                n = exp_max;
            }
        }
    } else if n < exp_min {
        // When scaling toward 0, the prescaling is limited to a value that does not allow `x` to
        // go subnormal. This avoids double rounding.
        // `mul` s.t. `!(x * mul).is_subnormal() ∀ x`
        let mul = f_exp_min * f_pow_subnorm;
        let add = -exp_min - sig_total_bits;

        // Worse case negative `n`: `x`  is the maximum positive value, the result is `f64::MIN`.
        // This must be reachable by three scaling multiplications (two here and one final).
        debug_assert!(-exp_min + 52 + exp_max <= add * 2 + -exp_min);

        x *= mul;
        n += add;

        if n < exp_min {
            x *= mul;
            n += add;

            if n < exp_min {
                n = exp_min;
            }
        }
    }

    let scale = f64::from_bits(((1023 + n) as u64) << 52); // 2^n
    x * scale
}

// origin: FreeBSD /usr/src/lib/msun/src/k_sin.c
//
// ====================================================
// Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
//
// Developed at SunSoft, a Sun Microsystems, Inc. business.
// Permission to use, copy, modify, and distribute this
// software is freely granted, provided that this notice
// is preserved.
// ====================================================

const S1: f64 = -1.66666666666666324348e-01; /* 0xBFC55555, 0x55555549 */
const S2: f64 = 8.33333333332248946124e-03; /* 0x3F811111, 0x1110F8A6 */
const S3: f64 = -1.98412698298579493134e-04; /* 0xBF2A01A0, 0x19C161D5 */
const S4: f64 = 2.75573137070700676789e-06; /* 0x3EC71DE3, 0x57B1FE7D */
const S5: f64 = -2.50507602534068634195e-08; /* 0xBE5AE5E6, 0x8A2B9CEB */
const S6: f64 = 1.58969099521155010221e-10; /* 0x3DE5D93A, 0x5ACFD57C */

// kernel sin function on ~[-pi/4, pi/4] (except on -0), pi/4 ~ 0.7854
// Input x is assumed to be bounded by ~pi/4 in magnitude.
// Input y is the tail of x.
// Input iy indicates whether y is 0. (if iy=0, y assume to be 0).
//
// Algorithm
//      1. Since sin(-x) = -sin(x), we need only to consider positive x.
//      2. Callers must return sin(-0) = -0 without calling here since our
//         odd polynomial is not evaluated in a way that preserves -0.
//         Callers may do the optimization sin(x) ~ x for tiny x.
//      3. sin(x) is approximated by a polynomial of degree 13 on
//         [0,pi/4]
//                               3            13
//              sin(x) ~ x + S1*x + ... + S6*x
//         where
//
//      |sin(x)         2     4     6     8     10     12  |     -58
//      |----- - (1+S1*x +S2*x +S3*x +S4*x +S5*x  +S6*x   )| <= 2
//      |  x                                               |
//
//      4. sin(x+y) = sin(x) + sin'(x')*y
//                  ~ sin(x) + (1-x*x/2)*y
//         For better accuracy, let
//                   3      2      2      2      2
//              r = x *(S2+x *(S3+x *(S4+x *(S5+x *S6))))
//         then                   3    2
//              sin(x) = x + (S1*x + (x *(r-y/2)+y))
fn k_sin(x: f64, y: f64, iy: i32) -> f64 {
    let z = x * x;
    let w = z * z;
    let r = S2 + z * (S3 + z * S4) + z * w * (S5 + z * S6);
    let v = z * x;
    if iy == 0 {
        x + v * (S1 + z * r)
    } else {
        x - ((z * (0.5 * y - v * r) - y) - v * S1)
    }
}

// origin: FreeBSD /usr/src/lib/msun/src/k_cos.c
//
// ====================================================
// Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
//
// Developed at SunSoft, a Sun Microsystems, Inc. business.
// Permission to use, copy, modify, and distribute this
// software is freely granted, provided that this notice
// is preserved.
// ====================================================

const C1: f64 = 4.16666666666666019037e-02; /* 0x3FA55555, 0x5555554C */
const C2: f64 = -1.38888888888741095749e-03; /* 0xBF56C16C, 0x16C15177 */
const C3: f64 = 2.48015872894767294178e-05; /* 0x3EFA01A0, 0x19CB1590 */
const C4: f64 = -2.75573143513906633035e-07; /* 0xBE927E4F, 0x809C52AD */
const C5: f64 = 2.08757232129817482790e-09; /* 0x3E21EE9E, 0xBDB4B1C4 */
const C6: f64 = -1.13596475577881948265e-11; /* 0xBDA8FAE9, 0xBE8838D4 */

// kernel cos function on [-pi/4, pi/4], pi/4 ~ 0.785398164
// Input x is assumed to be bounded by ~pi/4 in magnitude.
// Input y is the tail of x.
//
// Algorithm
//      1. Since cos(-x) = cos(x), we need only to consider positive x.
//      2. if x < 2^-27 (hx<0x3e400000 0), return 1 with inexact if x!=0.
//      3. cos(x) is approximated by a polynomial of degree 14 on
//         [0,pi/4]
//                                       4            14
//              cos(x) ~ 1 - x*x/2 + C1*x + ... + C6*x
//         where the remez error is
//
//      |              2     4     6     8     10    12     14 |     -58
//      |cos(x)-(1-.5*x +C1*x +C2*x +C3*x +C4*x +C5*x  +C6*x  )| <= 2
//      |                                                      |
//
//                     4     6     8     10    12     14
//      4. let r = C1*x +C2*x +C3*x +C4*x +C5*x  +C6*x  , then
//             cos(x) ~ 1 - x*x/2 + r
//         since cos(x+y) ~ cos(x) - sin(x)*y
//                        ~ cos(x) - x*y,
//         a correction term is necessary in cos(x) and hence
//              cos(x+y) = 1 - (x*x/2 - (r - x*y))
//         For better accuracy, rearrange to
//              cos(x+y) ~ w + (tmp + (r-x*y))
//         where w = 1 - x*x/2 and tmp is a tiny correction term
//         (1 - x*x/2 == w + tmp exactly in infinite precision).
//         The exactness of w + tmp in infinite precision depends on w
//         and tmp having the same precision as x.  If they have extra
//         precision due to compiler bugs, then the extra precision is
//         only good provided it is retained in all terms of the final
//         expression for cos().  Retention happens in all cases tested
//         under FreeBSD, so don't pessimize things by forcibly clipping
//         any extra precision in w.
fn k_cos(x: f64, y: f64) -> f64 {
    let z = x * x;
    let w = z * z;
    let r = z * (C1 + z * (C2 + z * C3)) + w * w * (C4 + z * (C5 + z * C6));
    let hz = 0.5 * z;
    let w = 1.0 - hz;
    w + (((1.0 - w) - hz) + (z * r - x * y))
}

// origin: FreeBSD /usr/src/lib/msun/src/k_tan.c */
//
// ====================================================
// Copyright 2004 Sun Microsystems, Inc.  All Rights Reserved.
//
// Permission to use, copy, modify, and distribute this
// software is freely granted, provided that this notice
// is preserved.
// ====================================================

// kernel tan function on ~[-pi/4, pi/4] (except on -0), pi/4 ~ 0.7854
// Input x is assumed to be bounded by ~pi/4 in magnitude.
// Input y is the tail of x.
// Input odd indicates whether tan (if odd = 0) or -1/tan (if odd = 1) is returned.
//
// Algorithm
//      1. Since tan(-x) = -tan(x), we need only to consider positive x.
//      2. Callers must return tan(-0) = -0 without calling here since our
//         odd polynomial is not evaluated in a way that preserves -0.
//         Callers may do the optimization tan(x) ~ x for tiny x.
//      3. tan(x) is approximated by a odd polynomial of degree 27 on
//         [0,0.67434]
//                               3             27
//              tan(x) ~ x + T1*x + ... + T13*x
//         where
//
//              |tan(x)         2     4            26   |     -59.2
//              |----- - (1+T1*x +T2*x +.... +T13*x    )| <= 2
//              |  x                                    |
//
//         Note: tan(x+y) = tan(x) + tan'(x)*y
//                        ~ tan(x) + (1+x*x)*y
//         Therefore, for better accuracy in computing tan(x+y), let
//                   3      2      2       2       2
//              r = x *(T2+x *(T3+x *(...+x *(T12+x *T13))))
//         then
//                                  3    2
//              tan(x+y) = x + (T1*x + (x *(r+y)+y))
//
//      4. For x in [0.67434,pi/4],  let y = pi/4 - x, then
//              tan(x) = tan(pi/4-y) = (1-tan(y))/(1+tan(y))
//                     = 1 - 2*(tan(y) - (tan(y)^2)/(1+tan(y)))
static T: [f64; 13] = [
    3.33333333333334091986e-01,  /* 3FD55555, 55555563 */
    1.33333333333201242699e-01,  /* 3FC11111, 1110FE7A */
    5.39682539762260521377e-02,  /* 3FABA1BA, 1BB341FE */
    2.18694882948595424599e-02,  /* 3F9664F4, 8406D637 */
    8.86323982359930005737e-03,  /* 3F8226E3, E96E8493 */
    3.59207910759131235356e-03,  /* 3F6D6D22, C9560328 */
    1.45620945432529025516e-03,  /* 3F57DBC8, FEE08315 */
    5.88041240820264096874e-04,  /* 3F4344D8, F2F26501 */
    2.46463134818469906812e-04,  /* 3F3026F7, 1A8D1068 */
    7.81794442939557092300e-05,  /* 3F147E88, A03792A6 */
    7.14072491382608190305e-05,  /* 3F12B80F, 32F0A7E9 */
    -1.85586374855275456654e-05, /* BEF375CB, DB605373 */
    2.59073051863633712884e-05,  /* 3EFB2A70, 74BF7AD4 */
];
const PIO4: f64 = 7.85398163397448278999e-01; /* 3FE921FB, 54442D18 */
const PIO4_LO: f64 = 3.06161699786838301793e-17; /* 3C81A626, 33145C07 */

fn k_tan(mut x: f64, mut y: f64, odd: i32) -> f64 {
    let hx = (f64::to_bits(x) >> 32) as u32;
    let big = (hx & 0x7fffffff) >= 0x3FE59428; /* |x| >= 0.6744 */
    if big {
        let sign = hx >> 31;
        if sign != 0 {
            x = -x;
            y = -y;
        }
        x = (PIO4 - x) + (PIO4_LO - y);
        y = 0.0;
    }
    let z = x * x;
    let w = z * z;
    /*
     * Break x^5*(T[1]+x^2*T[2]+...) into
     * x^5(T[1]+x^4*T[3]+...+x^20*T[11]) +
     * x^5(x^2*(T[2]+x^4*T[4]+...+x^22*[T12]))
     */
    let r = T[1] + w * (T[3] + w * (T[5] + w * (T[7] + w * (T[9] + w * T[11]))));
    let v = z * (T[2] + w * (T[4] + w * (T[6] + w * (T[8] + w * (T[10] + w * T[12])))));
    let s = z * x;
    let r = y + z * (s * (r + v) + y) + s * T[0];
    let w = x + r;
    if big {
        let sign = hx >> 31;
        let s = 1.0 - 2.0 * odd as f64;
        let v = s - 2.0 * (x + (r - w * w / (w + s)));
        return if sign != 0 { -v } else { v };
    }
    if odd == 0 {
        return w;
    }
    /* -1.0/(x+r) has up to 2ulp error, so compute it accurately */
    let w0 = zero_low_word(w);
    let v = r - (w0 - x); /* w0+v = r+x */
    let a = -1.0 / w;
    let a0 = zero_low_word(a);
    a0 + a * (1.0 + a0 * w0 + a0 * v)
}

fn zero_low_word(x: f64) -> f64 {
    f64::from_bits(f64::to_bits(x) & 0xFFFF_FFFF_0000_0000)
}

// origin: FreeBSD /usr/src/lib/msun/src/e_rem_pio2.c
//
// ====================================================
// Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
//
// Developed at SunPro, a Sun Microsystems, Inc. business.
// Permission to use, copy, modify, and distribute this
// software is freely granted, provided that this notice
// is preserved.
// ====================================================
//
// Optimized by Bruce D. Evans. */
// #if FLT_EVAL_METHOD==0 || FLT_EVAL_METHOD==1
// #define EPS DBL_EPSILON
const EPS: f64 = 2.2204460492503131e-16;
// #elif FLT_EVAL_METHOD==2
// #define EPS LDBL_EPSILON
// #endif

// TODO: Support FLT_EVAL_METHOD?

const TO_INT: f64 = 1.5 / EPS;
/// 53 bits of 2/pi
const INV_PIO2: f64 = 6.36619772367581382433e-01; /* 0x3FE45F30, 0x6DC9C883 */
/// first 33 bits of pi/2
const PIO2_1: f64 = 1.57079632673412561417e+00; /* 0x3FF921FB, 0x54400000 */
/// pi/2 - PIO2_1
const PIO2_1T: f64 = 6.07710050650619224932e-11; /* 0x3DD0B461, 0x1A626331 */
/// second 33 bits of pi/2
const PIO2_2: f64 = 6.07710050630396597660e-11; /* 0x3DD0B461, 0x1A600000 */
/// pi/2 - (PIO2_1+PIO2_2)
const PIO2_2T: f64 = 2.02226624879595063154e-21; /* 0x3BA3198A, 0x2E037073 */
/// third 33 bits of pi/2
const PIO2_3: f64 = 2.02226624871116645580e-21; /* 0x3BA3198A, 0x2E000000 */
/// pi/2 - (PIO2_1+PIO2_2+PIO2_3)
const PIO2_3T: f64 = 8.47842766036889956997e-32; /* 0x397B839A, 0x252049C1 */

// return the remainder of x rem pi/2 in y[0]+y[1]
// use rem_pio2_large() for large x
//
// caller must handle the case when reduction is not needed: |x| ~<= pi/4 */
fn rem_pio2(x: f64) -> (i32, f64, f64) {
    let x1p24 = f64::from_bits(0x4170000000000000);

    let sign = (f64::to_bits(x) >> 63) as i32;
    let ix = (f64::to_bits(x) >> 32) as u32 & 0x7fffffff;

    fn medium(x: f64, ix: u32) -> (i32, f64, f64) {
        /* rint(x/(pi/2)), Assume round-to-nearest. */
        let tmp = x * INV_PIO2 + TO_INT;
        let f_n = tmp - TO_INT;
        let n = f_n as i32;
        let mut r = x - f_n * PIO2_1;
        let mut w = f_n * PIO2_1T; /* 1st round, good to 85 bits */
        let mut y0 = r - w;
        let ui = f64::to_bits(y0);
        let ey = (ui >> 52) as i32 & 0x7ff;
        let ex = (ix >> 20) as i32;
        if ex - ey > 16 {
            /* 2nd round, good to 118 bits */
            let t = r;
            w = f_n * PIO2_2;
            r = t - w;
            w = f_n * PIO2_2T - ((t - r) - w);
            y0 = r - w;
            let ey = (f64::to_bits(y0) >> 52) as i32 & 0x7ff;
            if ex - ey > 49 {
                /* 3rd round, good to 151 bits, covers all cases */
                let t = r;
                w = f_n * PIO2_3;
                r = t - w;
                w = f_n * PIO2_3T - ((t - r) - w);
                y0 = r - w;
            }
        }
        let y1 = (r - y0) - w;
        (n, y0, y1)
    }

    if ix <= 0x400f6a7a {
        /* |x| ~<= 5pi/4 */
        if (ix & 0xfffff) == 0x921fb {
            /* |x| ~= pi/2 or 2pi/2 */
            return medium(x, ix); /* cancellation -- use medium case */
        }
        if ix <= 0x4002d97c {
            /* |x| ~<= 3pi/4 */
            if sign == 0 {
                let z = x - PIO2_1; /* one round good to 85 bits */
                let y0 = z - PIO2_1T;
                let y1 = (z - y0) - PIO2_1T;
                return (1, y0, y1);
            } else {
                let z = x + PIO2_1;
                let y0 = z + PIO2_1T;
                let y1 = (z - y0) + PIO2_1T;
                return (-1, y0, y1);
            }
        } else if sign == 0 {
            let z = x - 2.0 * PIO2_1;
            let y0 = z - 2.0 * PIO2_1T;
            let y1 = (z - y0) - 2.0 * PIO2_1T;
            return (2, y0, y1);
        } else {
            let z = x + 2.0 * PIO2_1;
            let y0 = z + 2.0 * PIO2_1T;
            let y1 = (z - y0) + 2.0 * PIO2_1T;
            return (-2, y0, y1);
        }
    }
    if ix <= 0x401c463b {
        /* |x| ~<= 9pi/4 */
        if ix <= 0x4015fdbc {
            /* |x| ~<= 7pi/4 */
            if ix == 0x4012d97c {
                /* |x| ~= 3pi/2 */
                return medium(x, ix);
            }
            if sign == 0 {
                let z = x - 3.0 * PIO2_1;
                let y0 = z - 3.0 * PIO2_1T;
                let y1 = (z - y0) - 3.0 * PIO2_1T;
                return (3, y0, y1);
            } else {
                let z = x + 3.0 * PIO2_1;
                let y0 = z + 3.0 * PIO2_1T;
                let y1 = (z - y0) + 3.0 * PIO2_1T;
                return (-3, y0, y1);
            }
        } else {
            if ix == 0x401921fb {
                /* |x| ~= 4pi/2 */
                return medium(x, ix);
            }
            if sign == 0 {
                let z = x - 4.0 * PIO2_1;
                let y0 = z - 4.0 * PIO2_1T;
                let y1 = (z - y0) - 4.0 * PIO2_1T;
                return (4, y0, y1);
            } else {
                let z = x + 4.0 * PIO2_1;
                let y0 = z + 4.0 * PIO2_1T;
                let y1 = (z - y0) + 4.0 * PIO2_1T;
                return (-4, y0, y1);
            }
        }
    }
    if ix < 0x413921fb {
        /* |x| ~< 2^20*(pi/2), medium size */
        return medium(x, ix);
    }
    /*
     * all other (large) arguments
     */
    if ix >= 0x7ff00000 {
        /* x is inf or NaN */
        let y0 = x - x;
        let y1 = y0;
        return (0, y0, y1);
    }
    /* set z = scalbn(|x|,-ilogb(x)+23) */
    let mut ui = f64::to_bits(x);
    ui &= (!1) >> 12;
    ui |= (0x3ff + 23) << 52;
    let mut z = f64::from_bits(ui);
    let mut tx = [0.0; 3];
    for i in 0..2 {
        tx[i] = z as i32 as f64;
        z = (z - tx[i]) * x1p24;
    }
    tx[2] = z;
    /* skip zero terms, first term is non-zero */
    let mut i = 2;
    while i != 0 && tx[i] == 0.0 {
        i -= 1;
    }
    let mut ty = [0.0; 3];
    let n = rem_pio2_large(&tx[..=i], &mut ty, ((ix as i32) >> 20) - (0x3ff + 23), 1);
    if sign != 0 {
        return (-n, -ty[0], -ty[1]);
    }
    (n, ty[0], ty[1])
}

/* origin: FreeBSD /usr/src/lib/msun/src/k_rem_pio2.c */
/*
 * ====================================================
 * Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
 *
 * Developed at SunSoft, a Sun Microsystems, Inc. business.
 * Permission to use, copy, modify, and distribute this
 * software is freely granted, provided that this notice
 * is preserved.
 * ====================================================
 */

// initial value for jk
const INIT_JK: [usize; 4] = [3, 4, 4, 6];

// Table of constants for 2/pi, 396 Hex digits (476 decimal) of 2/pi
//
//              integer array, contains the (24*i)-th to (24*i+23)-th
//              bit of 2/pi after binary point. The corresponding
//              floating value is
//
//                      ipio2[i] * 2^(-24(i+1)).
//
// NB: This table must have at least (e0-3)/24 + jk terms.
//     For quad precision (e0 <= 16360, jk = 6), this is 686.
#[cfg(any(target_pointer_width = "32", target_pointer_width = "16"))]
const IPIO2: [i32; 66] = [
    0xA2F983, 0x6E4E44, 0x1529FC, 0x2757D1, 0xF534DD, 0xC0DB62, 0x95993C, 0x439041, 0xFE5163,
    0xABDEBB, 0xC561B7, 0x246E3A, 0x424DD2, 0xE00649, 0x2EEA09, 0xD1921C, 0xFE1DEB, 0x1CB129,
    0xA73EE8, 0x8235F5, 0x2EBB44, 0x84E99C, 0x7026B4, 0x5F7E41, 0x3991D6, 0x398353, 0x39F49C,
    0x845F8B, 0xBDF928, 0x3B1FF8, 0x97FFDE, 0x05980F, 0xEF2F11, 0x8B5A0A, 0x6D1F6D, 0x367ECF,
    0x27CB09, 0xB74F46, 0x3F669E, 0x5FEA2D, 0x7527BA, 0xC7EBE5, 0xF17B3D, 0x0739F7, 0x8A5292,
    0xEA6BFB, 0x5FB11F, 0x8D5D08, 0x560330, 0x46FC7B, 0x6BABF0, 0xCFBC20, 0x9AF436, 0x1DA9E3,
    0x91615E, 0xE61B08, 0x659985, 0x5F14A0, 0x68408D, 0xFFD880, 0x4D7327, 0x310606, 0x1556CA,
    0x73A8C9, 0x60E27B, 0xC08C6B,
];

#[cfg(target_pointer_width = "64")]
const IPIO2: [i32; 690] = [
    0xA2F983, 0x6E4E44, 0x1529FC, 0x2757D1, 0xF534DD, 0xC0DB62, 0x95993C, 0x439041, 0xFE5163,
    0xABDEBB, 0xC561B7, 0x246E3A, 0x424DD2, 0xE00649, 0x2EEA09, 0xD1921C, 0xFE1DEB, 0x1CB129,
    0xA73EE8, 0x8235F5, 0x2EBB44, 0x84E99C, 0x7026B4, 0x5F7E41, 0x3991D6, 0x398353, 0x39F49C,
    0x845F8B, 0xBDF928, 0x3B1FF8, 0x97FFDE, 0x05980F, 0xEF2F11, 0x8B5A0A, 0x6D1F6D, 0x367ECF,
    0x27CB09, 0xB74F46, 0x3F669E, 0x5FEA2D, 0x7527BA, 0xC7EBE5, 0xF17B3D, 0x0739F7, 0x8A5292,
    0xEA6BFB, 0x5FB11F, 0x8D5D08, 0x560330, 0x46FC7B, 0x6BABF0, 0xCFBC20, 0x9AF436, 0x1DA9E3,
    0x91615E, 0xE61B08, 0x659985, 0x5F14A0, 0x68408D, 0xFFD880, 0x4D7327, 0x310606, 0x1556CA,
    0x73A8C9, 0x60E27B, 0xC08C6B, 0x47C419, 0xC367CD, 0xDCE809, 0x2A8359, 0xC4768B, 0x961CA6,
    0xDDAF44, 0xD15719, 0x053EA5, 0xFF0705, 0x3F7E33, 0xE832C2, 0xDE4F98, 0x327DBB, 0xC33D26,
    0xEF6B1E, 0x5EF89F, 0x3A1F35, 0xCAF27F, 0x1D87F1, 0x21907C, 0x7C246A, 0xFA6ED5, 0x772D30,
    0x433B15, 0xC614B5, 0x9D19C3, 0xC2C4AD, 0x414D2C, 0x5D000C, 0x467D86, 0x2D71E3, 0x9AC69B,
    0x006233, 0x7CD2B4, 0x97A7B4, 0xD55537, 0xF63ED7, 0x1810A3, 0xFC764D, 0x2A9D64, 0xABD770,
    0xF87C63, 0x57B07A, 0xE71517, 0x5649C0, 0xD9D63B, 0x3884A7, 0xCB2324, 0x778AD6, 0x23545A,
    0xB91F00, 0x1B0AF1, 0xDFCE19, 0xFF319F, 0x6A1E66, 0x615799, 0x47FBAC, 0xD87F7E, 0xB76522,
    0x89E832, 0x60BFE6, 0xCDC4EF, 0x09366C, 0xD43F5D, 0xD7DE16, 0xDE3B58, 0x929BDE, 0x2822D2,
    0xE88628, 0x4D58E2, 0x32CAC6, 0x16E308, 0xCB7DE0, 0x50C017, 0xA71DF3, 0x5BE018, 0x34132E,
    0x621283, 0x014883, 0x5B8EF5, 0x7FB0AD, 0xF2E91E, 0x434A48, 0xD36710, 0xD8DDAA, 0x425FAE,
    0xCE616A, 0xA4280A, 0xB499D3, 0xF2A606, 0x7F775C, 0x83C2A3, 0x883C61, 0x78738A, 0x5A8CAF,
    0xBDD76F, 0x63A62D, 0xCBBFF4, 0xEF818D, 0x67C126, 0x45CA55, 0x36D9CA, 0xD2A828, 0x8D61C2,
    0x77C912, 0x142604, 0x9B4612, 0xC459C4, 0x44C5C8, 0x91B24D, 0xF31700, 0xAD43D4, 0xE54929,
    0x10D5FD, 0xFCBE00, 0xCC941E, 0xEECE70, 0xF53E13, 0x80F1EC, 0xC3E7B3, 0x28F8C7, 0x940593,
    0x3E71C1, 0xB3092E, 0xF3450B, 0x9C1288, 0x7B20AB, 0x9FB52E, 0xC29247, 0x2F327B, 0x6D550C,
    0x90A772, 0x1FE76B, 0x96CB31, 0x4A1679, 0xE27941, 0x89DFF4, 0x9794E8, 0x84E6E2, 0x973199,
    0x6BED88, 0x365F5F, 0x0EFDBB, 0xB49A48, 0x6CA467, 0x427271, 0x325D8D, 0xB8159F, 0x09E5BC,
    0x25318D, 0x3974F7, 0x1C0530, 0x010C0D, 0x68084B, 0x58EE2C, 0x90AA47, 0x02E774, 0x24D6BD,
    0xA67DF7, 0x72486E, 0xEF169F, 0xA6948E, 0xF691B4, 0x5153D1, 0xF20ACF, 0x339820, 0x7E4BF5,
    0x6863B2, 0x5F3EDD, 0x035D40, 0x7F8985, 0x295255, 0xC06437, 0x10D86D, 0x324832, 0x754C5B,
    0xD4714E, 0x6E5445, 0xC1090B, 0x69F52A, 0xD56614, 0x9D0727, 0x50045D, 0xDB3BB4, 0xC576EA,
    0x17F987, 0x7D6B49, 0xBA271D, 0x296996, 0xACCCC6, 0x5414AD, 0x6AE290, 0x89D988, 0x50722C,
    0xBEA404, 0x940777, 0x7030F3, 0x27FC00, 0xA871EA, 0x49C266, 0x3DE064, 0x83DD97, 0x973FA3,
    0xFD9443, 0x8C860D, 0xDE4131, 0x9D3992, 0x8C70DD, 0xE7B717, 0x3BDF08, 0x2B3715, 0xA0805C,
    0x93805A, 0x921110, 0xD8E80F, 0xAF806C, 0x4BFFDB, 0x0F9038, 0x761859, 0x15A562, 0xBBCB61,
    0xB989C7, 0xBD4010, 0x04F2D2, 0x277549, 0xF6B6EB, 0xBB22DB, 0xAA140A, 0x2F2689, 0x768364,
    0x333B09, 0x1A940E, 0xAA3A51, 0xC2A31D, 0xAEEDAF, 0x12265C, 0x4DC26D, 0x9C7A2D, 0x9756C0,
    0x833F03, 0xF6F009, 0x8C402B, 0x99316D, 0x07B439, 0x15200C, 0x5BC3D8, 0xC492F5, 0x4BADC6,
    0xA5CA4E, 0xCD37A7, 0x36A9E6, 0x9492AB, 0x6842DD, 0xDE6319, 0xEF8C76, 0x528B68, 0x37DBFC,
    0xABA1AE, 0x3115DF, 0xA1AE00, 0xDAFB0C, 0x664D64, 0xB705ED, 0x306529, 0xBF5657, 0x3AFF47,
    0xB9F96A, 0xF3BE75, 0xDF9328, 0x3080AB, 0xF68C66, 0x15CB04, 0x0622FA, 0x1DE4D9, 0xA4B33D,
    0x8F1B57, 0x09CD36, 0xE9424E, 0xA4BE13, 0xB52333, 0x1AAAF0, 0xA8654F, 0xA5C1D2, 0x0F3F0B,
    0xCD785B, 0x76F923, 0x048B7B, 0x721789, 0x53A6C6, 0xE26E6F, 0x00EBEF, 0x584A9B, 0xB7DAC4,
    0xBA66AA, 0xCFCF76, 0x1D02D1, 0x2DF1B1, 0xC1998C, 0x77ADC3, 0xDA4886, 0xA05DF7, 0xF480C6,
    0x2FF0AC, 0x9AECDD, 0xBC5C3F, 0x6DDED0, 0x1FC790, 0xB6DB2A, 0x3A25A3, 0x9AAF00, 0x9353AD,
    0x0457B6, 0xB42D29, 0x7E804B, 0xA707DA, 0x0EAA76, 0xA1597B, 0x2A1216, 0x2DB7DC, 0xFDE5FA,
    0xFEDB89, 0xFDBE89, 0x6C76E4, 0xFCA906, 0x70803E, 0x156E85, 0xFF87FD, 0x073E28, 0x336761,
    0x86182A, 0xEABD4D, 0xAFE7B3, 0x6E6D8F, 0x396795, 0x5BBF31, 0x48D784, 0x16DF30, 0x432DC7,
    0x356125, 0xCE70C9, 0xB8CB30, 0xFD6CBF, 0xA200A4, 0xE46C05, 0xA0DD5A, 0x476F21, 0xD21262,
    0x845CB9, 0x496170, 0xE0566B, 0x015299, 0x375550, 0xB7D51E, 0xC4F133, 0x5F6E13, 0xE4305D,
    0xA92E85, 0xC3B21D, 0x3632A1, 0xA4B708, 0xD4B1EA, 0x21F716, 0xE4698F, 0x77FF27, 0x80030C,
    0x2D408D, 0xA0CD4F, 0x99A520, 0xD3A2B3, 0x0A5D2F, 0x42F9B4, 0xCBDA11, 0xD0BE7D, 0xC1DB9B,
    0xBD17AB, 0x81A2CA, 0x5C6A08, 0x17552E, 0x550027, 0xF0147F, 0x8607E1, 0x640B14, 0x8D4196,
    0xDEBE87, 0x2AFDDA, 0xB6256B, 0x34897B, 0xFEF305, 0x9EBFB9, 0x4F6A68, 0xA82A4A, 0x5AC44F,
    0xBCF82D, 0x985AD7, 0x95C7F4, 0x8D4D0D, 0xA63A20, 0x5F57A4, 0xB13F14, 0x953880, 0x0120CC,
    0x86DD71, 0xB6DEC9, 0xF560BF, 0x11654D, 0x6B0701, 0xACB08C, 0xD0C0B2, 0x485551, 0x0EFB1E,
    0xC37295, 0x3B06A3, 0x3540C0, 0x7BDC06, 0xCC45E0, 0xFA294E, 0xC8CAD6, 0x41F3E8, 0xDE647C,
    0xD8649B, 0x31BED9, 0xC397A4, 0xD45877, 0xC5E369, 0x13DAF0, 0x3C3ABA, 0x461846, 0x5F7555,
    0xF5BDD2, 0xC6926E, 0x5D2EAC, 0xED440E, 0x423E1C, 0x87C461, 0xE9FD29, 0xF3D6E7, 0xCA7C22,
    0x35916F, 0xC5E008, 0x8DD7FF, 0xE26A6E, 0xC6FDB0, 0xC10893, 0x745D7C, 0xB2AD6B, 0x9D6ECD,
    0x7B723E, 0x6A11C6, 0xA9CFF7, 0xDF7329, 0xBAC9B5, 0x5100B7, 0x0DB2E2, 0x24BA74, 0x607DE5,
    0x8AD874, 0x2C150D, 0x0C1881, 0x94667E, 0x162901, 0x767A9F, 0xBEFDFD, 0xEF4556, 0x367ED9,
    0x13D9EC, 0xB9BA8B, 0xFC97C4, 0x27A831, 0xC36EF1, 0x36C594, 0x56A8D8, 0xB5A8B4, 0x0ECCCF,
    0x2D8912, 0x34576F, 0x89562C, 0xE3CE99, 0xB920D6, 0xAA5E6B, 0x9C2A3E, 0xCC5F11, 0x4A0BFD,
    0xFBF4E1, 0x6D3B8E, 0x2C86E2, 0x84D4E9, 0xA9B4FC, 0xD1EEEF, 0xC9352E, 0x61392F, 0x442138,
    0xC8D91B, 0x0AFC81, 0x6A4AFB, 0xD81C2F, 0x84B453, 0x8C994E, 0xCC2254, 0xDC552A, 0xD6C6C0,
    0x96190B, 0xB8701A, 0x649569, 0x605A26, 0xEE523F, 0x0F117F, 0x11B5F4, 0xF5CBFC, 0x2DBC34,
    0xEEBC34, 0xCC5DE8, 0x605EDD, 0x9B8E67, 0xEF3392, 0xB817C9, 0x9B5861, 0xBC57E1, 0xC68351,
    0x103ED8, 0x4871DD, 0xDD1C2D, 0xA118AF, 0x462C21, 0xD7F359, 0x987AD9, 0xC0549E, 0xFA864F,
    0xFC0656, 0xAE79E5, 0x362289, 0x22AD38, 0xDC9367, 0xAAE855, 0x382682, 0x9BE7CA, 0xA40D51,
    0xB13399, 0x0ED7A9, 0x480569, 0xF0B265, 0xA7887F, 0x974C88, 0x36D1F9, 0xB39221, 0x4A827B,
    0x21CF98, 0xDC9F40, 0x5547DC, 0x3A74E1, 0x42EB67, 0xDF9DFE, 0x5FD45E, 0xA4677B, 0x7AACBA,
    0xA2F655, 0x23882B, 0x55BA41, 0x086E59, 0x862A21, 0x834739, 0xE6E389, 0xD49EE5, 0x40FB49,
    0xE956FF, 0xCA0F1C, 0x8A59C5, 0x2BFA94, 0xC5C1D3, 0xCFC50F, 0xAE5ADB, 0x86C547, 0x624385,
    0x3B8621, 0x94792C, 0x876110, 0x7B4C2A, 0x1A2C80, 0x12BF43, 0x902688, 0x893C78, 0xE4C4A8,
    0x7BDBE5, 0xC23AC4, 0xEAF426, 0x8A67F7, 0xBF920D, 0x2BA365, 0xB1933D, 0x0B7CBD, 0xDC51A4,
    0x63DD27, 0xDDE169, 0x19949A, 0x9529A8, 0x28CE68, 0xB4ED09, 0x209F44, 0xCA984E, 0x638270,
    0x237C7E, 0x32B90F, 0x8EF5A7, 0xE75614, 0x08F121, 0x2A9DB5, 0x4D7E6F, 0x5119A5, 0xABF9B5,
    0xD6DF82, 0x61DD96, 0x023616, 0x9F3AC4, 0xA1A283, 0x6DED72, 0x7A8D39, 0xA9B882, 0x5C326B,
    0x5B2746, 0xED3400, 0x7700D2, 0x55F4FC, 0x4D5901, 0x8071E0,
];

const PIO2: [f64; 8] = [
    1.57079625129699707031e+00, /* 0x3FF921FB, 0x40000000 */
    7.54978941586159635335e-08, /* 0x3E74442D, 0x00000000 */
    5.39030252995776476554e-15, /* 0x3CF84698, 0x80000000 */
    3.28200341580791294123e-22, /* 0x3B78CC51, 0x60000000 */
    1.27065575308067607349e-29, /* 0x39F01B83, 0x80000000 */
    1.22933308981111328932e-36, /* 0x387A2520, 0x40000000 */
    2.73370053816464559624e-44, /* 0x36E38222, 0x80000000 */
    2.16741683877804819444e-51, /* 0x3569F31D, 0x00000000 */
];

// fn rem_pio2_large(x : &[f64], y : &mut [f64], e0 : i32, prec : usize) -> i32
//
// Input parameters:
//      x[]     The input value (must be positive) is broken into nx
//              pieces of 24-bit integers in double precision format.
//              x[i] will be the i-th 24 bit of x. The scaled exponent
//              of x[0] is given in input parameter e0 (i.e., x[0]*2^e0
//              match x's up to 24 bits.
//
//              Example of breaking a double positive z into x[0]+x[1]+x[2]:
//                      e0 = ilogb(z)-23
//                      z  = scalbn(z,-e0)
//              for i = 0,1,2
//                      x[i] = floor(z)
//                      z    = (z-x[i])*2**24
//
//      y[]     output result in an array of double precision numbers.
//              The dimension of y[] is:
//                      24-bit  precision       1
//                      53-bit  precision       2
//                      64-bit  precision       2
//                      113-bit precision       3
//              The actual value is the sum of them. Thus for 113-bit
//              precison, one may have to do something like:
//
//              long double t,w,r_head, r_tail;
//              t = (long double)y[2] + (long double)y[1];
//              w = (long double)y[0];
//              r_head = t+w;
//              r_tail = w - (r_head - t);
//
//      e0      The exponent of x[0]. Must be <= 16360 or you need to
//              expand the ipio2 table.
//
//      prec    an integer indicating the precision:
//                      0       24  bits (single)
//                      1       53  bits (double)
//                      2       64  bits (extended)
//                      3       113 bits (quad)
//
// Here is the description of some local variables:
//
//      jk      jk+1 is the initial number of terms of ipio2[] needed
//              in the computation. The minimum and recommended value
//              for jk is 3,4,4,6 for single, double, extended, and quad.
//              jk+1 must be 2 larger than you might expect so that our
//              recomputation test works. (Up to 24 bits in the integer
//              part (the 24 bits of it that we compute) and 23 bits in
//              the fraction part may be lost to cancelation before we
//              recompute.)
//
//      jz      local integer variable indicating the number of
//              terms of ipio2[] used.
//
//      jx      nx - 1
//
//      jv      index for pointing to the suitable ipio2[] for the
//              computation. In general, we want
//                      ( 2^e0*x[0] * ipio2[jv-1]*2^(-24jv) )/8
//              is an integer. Thus
//                      e0-3-24*jv >= 0 or (e0-3)/24 >= jv
//              Hence jv = max(0,(e0-3)/24).
//
//      jp      jp+1 is the number of terms in PIo2[] needed, jp = jk.
//
//      q[]     double array with integral value, representing the
//              24-bits chunk of the product of x and 2/pi.
//
//      q0      the corresponding exponent of q[0]. Note that the
//              exponent for q[i] would be q0-24*i.
//
//      PIo2[]  double precision array, obtained by cutting pi/2
//              into 24 bits chunks.
//
//      f[]     ipio2[] in floating point
//
//      iq[]    integer array by breaking up q[] in 24-bits chunk.
//
//      fq[]    final product of x*(2/pi) in fq[0],..,fq[jk]
//
//      ih      integer. If >0 it indicates q[] is >= 0.5, hence
//              it also indicates the *sign* of the result.

/// Return the last three digits of N with y = x - N*pi/2
/// so that |y| < pi/2.
///
/// The method is to compute the integer (mod 8) and fraction parts of
/// (2/pi)*x without doing the full multiplication. In general we
/// skip the part of the product that are known to be a huge integer (
/// more accurately, = 0 mod 8 ). Thus the number of operations are
/// independent of the exponent of the input.
fn rem_pio2_large(x: &[f64], y: &mut [f64], e0: i32, prec: usize) -> i32 {
    let x1p24 = f64::from_bits(0x4170000000000000); // 0x1p24 === 2 ^ 24
    let x1p_24 = f64::from_bits(0x3e70000000000000); // 0x1p_24 === 2 ^ (-24)

    if cfg!(target_pointer_width = "64") {
        debug_assert!(e0 <= 16360);
    }

    let nx = x.len();

    let mut fw: f64;
    let mut n: i32;
    let mut ih: i32;
    let mut z: f64;
    let mut f: [f64; 20] = [0.; 20];
    let mut fq: [f64; 20] = [0.; 20];
    let mut q: [f64; 20] = [0.; 20];
    let mut iq: [i32; 20] = [0; 20];

    /* initialize jk*/
    let jk = INIT_JK[prec];
    let jp = jk;

    /* determine jx,jv,q0, note that 3>q0 */
    let jx = nx - 1;
    let mut jv = (e0 - 3) / 24;
    if jv < 0 {
        jv = 0;
    }
    let mut q0 = e0 - 24 * (jv + 1);
    let jv = jv as usize;

    /* set up f[0] to f[jx+jk] where f[jx+jk] = ipio2[jv+jk] */
    let mut j = (jv as i32) - (jx as i32);
    let m = jx + jk;
    for i in 0..=m {
        f[i] = if j < 0 {
            0.
        } else {
            IPIO2[j as usize] as f64
        };
        j += 1;
    }

    /* compute q[0],q[1],...q[jk] */
    for i in 0..=jk {
        fw = 0f64;
        for j in 0..=jx {
            fw += x[j] * f[jx + i - j];
        }
        q[i] = fw;
    }

    let mut jz = jk;

    'recompute: loop {
        /* distill q[] into iq[] reversingly */
        let mut i = 0i32;
        z = q[jz];
        for j in (1..=jz).rev() {
            fw = (x1p_24 * z) as i32 as f64;
            iq[i as usize] = (z - x1p24 * fw) as i32;
            z = q[j - 1] + fw;
            i += 1;
        }

        /* compute n */
        z = scalbn(z, q0); /* actual value of z */
        z -= 8.0 * f64::floor(z * 0.125); /* trim off integer >= 8 */
        n = z as i32;
        z -= n as f64;
        ih = 0;
        if q0 > 0 {
            /* need iq[jz-1] to determine n */
            i = iq[jz - 1] >> (24 - q0);
            n += i;
            iq[jz - 1] -= i << (24 - q0);
            ih = iq[jz - 1] >> (23 - q0);
        } else if q0 == 0 {
            ih = iq[jz - 1] >> 23;
        } else if z >= 0.5 {
            ih = 2;
        }

        if ih > 0 {
            /* q > 0.5 */
            n += 1;
            let mut carry = 0i32;
            for i in 0..jz {
                /* compute 1-q */
                let j = iq[i];
                if carry == 0 {
                    if j != 0 {
                        carry = 1;
                        iq[i] = 0x1000000 - j;
                    }
                } else {
                    iq[i] = 0xffffff - j;
                }
            }
            if q0 > 0 {
                /* rare case: chance is 1 in 12 */
                match q0 {
                    1 => {
                        iq[jz - 1] &= 0x7fffff;
                    }
                    2 => {
                        iq[jz - 1] &= 0x3fffff;
                    }
                    _ => {}
                }
            }
            if ih == 2 {
                z = 1. - z;
                if carry != 0 {
                    z -= scalbn(1., q0);
                }
            }
        }

        /* check if recomputation is needed */
        if z == 0. {
            let mut j = 0;
            for i in (jk..=jz - 1).rev() {
                j |= iq[i];
            }
            if j == 0 {
                /* need recomputation */
                let mut k = 1;
                while iq[jk - k] == 0 {
                    k += 1; /* k = no. of terms needed */
                }

                for i in (jz + 1)..=(jz + k) {
                    /* add q[jz+1] to q[jz+k] */
                    f[jx + i] = IPIO2[jv + i] as f64;
                    fw = 0f64;
                    for j in 0..=jx {
                        fw += x[j] * f[jx + i - j];
                    }
                    q[i] = fw;
                }
                jz += k;
                continue 'recompute;
            }
        }

        break;
    }

    /* chop off zero terms */
    if z == 0. {
        jz -= 1;
        q0 -= 24;
        while iq[jz] == 0 {
            jz -= 1;
            q0 -= 24;
        }
    } else {
        /* break z into 24-bit if necessary */
        z = scalbn(z, -q0);
        if z >= x1p24 {
            fw = (x1p_24 * z) as i32 as f64;
            iq[jz] = (z - x1p24 * fw) as i32;
            jz += 1;
            q0 += 24;
            iq[jz] = fw as i32;
        } else {
            iq[jz] = z as i32;
        }
    }

    /* convert integer "bit" chunk to floating-point value */
    fw = scalbn(1., q0);
    for i in (0..=jz).rev() {
        q[i] = fw * (iq[i] as f64);
        fw *= x1p_24;
    }

    /* compute PIo2[0,...,jp]*q[jz,...,0] */
    for i in (0..=jz).rev() {
        fw = 0f64;
        let mut k = 0;
        while (k <= jp) && (k <= jz - i) {
            fw += PIO2[k] * q[i + k];
            k += 1;
        }
        fq[jz - i] = fw;
    }

    /* compress fq[] into y[] */
    match prec {
        0 => {
            fw = 0f64;
            for i in (0..=jz).rev() {
                fw += fq[i];
            }
            y[0] = if ih == 0 { fw } else { -fw };
        }
        1 | 2 => {
            fw = 0f64;
            for i in (0..=jz).rev() {
                fw += fq[i];
            }
            y[0] = if ih == 0 { fw } else { -fw };
            fw = fq[0] - fw;
            for i in 1..=jz {
                fw += fq[i];
            }
            y[1] = if ih == 0 { fw } else { -fw };
        }
        3 => {
            /* painful */
            for i in (1..=jz).rev() {
                fw = fq[i - 1] + fq[i];
                fq[i] += fq[i - 1] - fw;
                fq[i - 1] = fw;
            }
            for i in (2..=jz).rev() {
                fw = fq[i - 1] + fq[i];
                fq[i] += fq[i - 1] - fw;
                fq[i - 1] = fw;
            }
            fw = 0f64;
            for i in (2..=jz).rev() {
                fw += fq[i];
            }
            if ih == 0 {
                y[0] = fq[0];
                y[1] = fq[1];
                y[2] = fw;
            } else {
                y[0] = -fq[0];
                y[1] = -fq[1];
                y[2] = -fw;
            }
        }
        #[cfg(debug_assertions)]
        _ => unreachable!(),
        #[cfg(not(debug_assertions))]
        _ => {}
    }
    n & 7
}

// origin: FreeBSD /usr/src/lib/msun/src/s_sin.c */
//
// ====================================================
// Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
//
// Developed at SunPro, a Sun Microsystems, Inc. business.
// Permission to use, copy, modify, and distribute this
// software is freely granted, provided that this notice
// is preserved.
// ====================================================

// sin(x)
// Return sine function of x.
//
// kernel function:
//      k_sin            ... sine function on [-pi/4,pi/4]
//      k_cos            ... cose function on [-pi/4,pi/4]
//      rem_pio2         ... argument reduction routine
//
// Method.
//      Let S,C and T denote the sin, cos and tan respectively on
//      [-PI/4, +PI/4]. Reduce the argument x to y1+y2 = x-k*pi/2
//      in [-pi/4 , +pi/4], and let n = k mod 4.
//      We have
//
//          n        sin(x)      cos(x)        tan(x)
//     ----------------------------------------------------------
//          0          S           C             T
//          1          C          -S            -1/T
//          2         -S          -C             T
//          3         -C           S            -1/T
//     ----------------------------------------------------------
//
// Special cases:
//      Let trig be any of sin, cos, or tan.
//      trig(+-INF)  is NaN, with signals;
//      trig(NaN)    is that NaN;
//
// Accuracy:
//      TRIG(x) returns trig(x) nearly rounded

/// The sine of `x` (f64).
///
/// `x` is specified in radians.
pub fn sin(x: f64) -> f64 {
    let _x1p120 = f64::from_bits(0x4770000000000000); // 0x1p120f === 2 ^ 120

    /* High word of x. */
    let ix = (f64::to_bits(x) >> 32) as u32 & 0x7fffffff;

    /* |x| ~< pi/4 */
    if ix <= 0x3fe921fb {
        if ix < 0x3e500000 {
            /* |x| < 2**-26 */
            /* raise inexact if x != 0 and underflow if subnormal*/
            if ix < 0x00100000 {
                // Floating-point exception flags are unused.
            } else {
                // Floating-point exception flags are unused.
            }
            return x;
        }
        return k_sin(x, 0.0, 0);
    }

    /* sin(Inf or NaN) is NaN */
    if ix >= 0x7ff00000 {
        return x - x;
    }

    /* argument reduction needed */
    let (n, y0, y1) = rem_pio2(x);
    match n & 3 {
        0 => k_sin(y0, y1, 1),
        1 => k_cos(y0, y1),
        2 => -k_sin(y0, y1, 1),
        _ => -k_cos(y0, y1),
    }
}

// origin: FreeBSD /usr/src/lib/msun/src/s_cos.c */
//
// ====================================================
// Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
//
// Developed at SunPro, a Sun Microsystems, Inc. business.
// Permission to use, copy, modify, and distribute this
// software is freely granted, provided that this notice
// is preserved.
// ====================================================

// cos(x)
// Return cosine function of x.
//
// kernel function:
//      k_sin           ... sine function on [-pi/4,pi/4]
//      k_cos           ... cosine function on [-pi/4,pi/4]
//      rem_pio2        ... argument reduction routine
//
// Method.
//      Let S,C and T denote the sin, cos and tan respectively on
//      [-PI/4, +PI/4]. Reduce the argument x to y1+y2 = x-k*pi/2
//      in [-pi/4 , +pi/4], and let n = k mod 4.
//      We have
//
//          n        sin(x)      cos(x)        tan(x)
//     ----------------------------------------------------------
//          0          S           C             T
//          1          C          -S            -1/T
//          2         -S          -C             T
//          3         -C           S            -1/T
//     ----------------------------------------------------------
//
// Special cases:
//      Let trig be any of sin, cos, or tan.
//      trig(+-INF)  is NaN, with signals;
//      trig(NaN)    is that NaN;
//
// Accuracy:
//      TRIG(x) returns trig(x) nearly rounded
//

/// The cosine of `x` (f64).
///
/// `x` is specified in radians.
pub fn cos(x: f64) -> f64 {
    let ix = (f64::to_bits(x) >> 32) as u32 & 0x7fffffff;

    /* |x| ~< pi/4 */
    if ix <= 0x3fe921fb {
        if ix < 0x3e46a09e {
            /* if x < 2**-27 * sqrt(2) */
            /* raise inexact if x != 0 */
            if x as i32 == 0 {
                return 1.0;
            }
        }
        return k_cos(x, 0.0);
    }

    /* cos(Inf or NaN) is NaN */
    if ix >= 0x7ff00000 {
        return x - x;
    }

    /* argument reduction needed */
    let (n, y0, y1) = rem_pio2(x);
    match n & 3 {
        0 => k_cos(y0, y1),
        1 => -k_sin(y0, y1, 1),
        2 => -k_cos(y0, y1),
        _ => k_sin(y0, y1, 1),
    }
}

/* origin: FreeBSD /usr/src/lib/msun/src/s_sin.c */
/*
 * ====================================================
 * Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
 *
 * Developed at SunPro, a Sun Microsystems, Inc. business.
 * Permission to use, copy, modify, and distribute this
 * software is freely granted, provided that this notice
 * is preserved.
 * ====================================================
 */

/// Both the sine and cosine of `x` (f64).
///
/// `x` is specified in radians and the return value is (sin(x), cos(x)).
pub fn sincos(x: f64) -> (f64, f64) {
    let s: f64;
    let c: f64;
    let mut ix: u32;

    ix = get_high_word(x);
    ix &= 0x7fffffff;

    /* |x| ~< pi/4 */
    if ix <= 0x3fe921fb {
        /* if |x| < 2**-27 * sqrt(2) */
        if ix < 0x3e46a09e {
            /* raise inexact if x!=0 and underflow if subnormal */
            let _x1p120 = f64::from_bits(0x4770000000000000); // 0x1p120 == 2^120
            if ix < 0x00100000 {
                // Floating-point exception flags are unused.
            } else {
                // Floating-point exception flags are unused.
            }
            return (x, 1.0);
        }
        return (k_sin(x, 0.0, 0), k_cos(x, 0.0));
    }

    /* sincos(Inf or NaN) is NaN */
    if ix >= 0x7ff00000 {
        let rv = x - x;
        return (rv, rv);
    }

    /* argument reduction needed */
    let (n, y0, y1) = rem_pio2(x);
    s = k_sin(y0, y1, 1);
    c = k_cos(y0, y1);
    match n & 3 {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        3 => (-c, s),
        #[cfg(debug_assertions)]
        _ => unreachable!(),
        #[cfg(not(debug_assertions))]
        _ => (0.0, 1.0),
    }
}

// origin: FreeBSD /usr/src/lib/msun/src/s_tan.c */
//
// ====================================================
// Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
//
// Developed at SunPro, a Sun Microsystems, Inc. business.
// Permission to use, copy, modify, and distribute this
// software is freely granted, provided that this notice
// is preserved.
// ====================================================

// tan(x)
// Return tangent function of x.
//
// kernel function:
//      k_tan           ... tangent function on [-pi/4,pi/4]
//      rem_pio2        ... argument reduction routine
//
// Method.
//      Let S,C and T denote the sin, cos and tan respectively on
//      [-PI/4, +PI/4]. Reduce the argument x to y1+y2 = x-k*pi/2
//      in [-pi/4 , +pi/4], and let n = k mod 4.
//      We have
//
//          n        sin(x)      cos(x)        tan(x)
//     ----------------------------------------------------------
//          0          S           C             T
//          1          C          -S            -1/T
//          2         -S          -C             T
//          3         -C           S            -1/T
//     ----------------------------------------------------------
//
// Special cases:
//      Let trig be any of sin, cos, or tan.
//      trig(+-INF)  is NaN, with signals;
//      trig(NaN)    is that NaN;
//
// Accuracy:
//      TRIG(x) returns trig(x) nearly rounded

/// The tangent of `x` (f64).
///
/// `x` is specified in radians.
pub fn tan(x: f64) -> f64 {
    let _x1p120 = f32::from_bits(0x7b800000); // 0x1p120f === 2 ^ 120

    let ix = (f64::to_bits(x) >> 32) as u32 & 0x7fffffff;
    /* |x| ~< pi/4 */
    if ix <= 0x3fe921fb {
        if ix < 0x3e400000 {
            /* |x| < 2**-27 */
            /* raise inexact if x!=0 and underflow if subnormal */
            // Floating-point exception flags are unused.
            return x;
        }
        return k_tan(x, 0.0, 0);
    }

    /* tan(Inf or NaN) is NaN */
    if ix >= 0x7ff00000 {
        return x - x;
    }

    /* argument reduction */
    let (n, y0, y1) = rem_pio2(x);
    k_tan(y0, y1, n & 1)
}

/* origin: FreeBSD /usr/src/lib/msun/src/e_asin.c */
/*
 * ====================================================
 * Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
 *
 * Developed at SunSoft, a Sun Microsystems, Inc. business.
 * Permission to use, copy, modify, and distribute this
 * software is freely granted, provided that this notice
 * is preserved.
 * ====================================================
 */
/* asin(x)
 * Method :
 *      Since  asin(x) = x + x^3/6 + x^5*3/40 + x^7*15/336 + ...
 *      we approximate asin(x) on [0,0.5] by
 *              asin(x) = x + x*x^2*R(x^2)
 *      where
 *              R(x^2) is a rational approximation of (asin(x)-x)/x^3
 *      and its remez error is bounded by
 *              |(asin(x)-x)/x^3 - R(x^2)| < 2^(-58.75)
 *
 *      For x in [0.5,1]
 *              asin(x) = pi/2-2*asin(sqrt((1-x)/2))
 *      Let y = (1-x), z = y/2, s := sqrt(z), and pio2_hi+pio2_lo=pi/2;
 *      then for x>0.98
 *              asin(x) = pi/2 - 2*(s+s*z*R(z))
 *                      = pio2_hi - (2*(s+s*z*R(z)) - pio2_lo)
 *      For x<=0.98, let pio4_hi = pio2_hi/2, then
 *              f = hi part of s;
 *              c = sqrt(z) - f = (z-f*f)/(s+f)         ...f+c=sqrt(z)
 *      and
 *              asin(x) = pi/2 - 2*(s+s*z*R(z))
 *                      = pio4_hi+(pio4-2s)-(2s*z*R(z)-pio2_lo)
 *                      = pio4_hi+(pio4-2f)-(2s*z*R(z)-(pio2_lo+2c))
 *
 * Special cases:
 *      if x is NaN, return x itself;
 *      if |x|>1, return NaN with invalid signal.
 *
 */

const PIO2_HI: f64 = 1.57079632679489655800e+00; /* 0x3FF921FB, 0x54442D18 */
const PIO2_LO: f64 = 6.12323399573676603587e-17; /* 0x3C91A626, 0x33145C07 */
/* coefficients for R(x^2) */
const P_S0: f64 = 1.66666666666666657415e-01; /* 0x3FC55555, 0x55555555 */
const P_S1: f64 = -3.25565818622400915405e-01; /* 0xBFD4D612, 0x03EB6F7D */
const P_S2: f64 = 2.01212532134862925881e-01; /* 0x3FC9C155, 0x0E884455 */
const P_S3: f64 = -4.00555345006794114027e-02; /* 0xBFA48228, 0xB5688F3B */
const P_S4: f64 = 7.91534994289814532176e-04; /* 0x3F49EFE0, 0x7501B288 */
const P_S5: f64 = 3.47933107596021167570e-05; /* 0x3F023DE1, 0x0DFDF709 */
const Q_S1: f64 = -2.40339491173441421878e+00; /* 0xC0033A27, 0x1C8A2D4B */
const Q_S2: f64 = 2.02094576023350569471e+00; /* 0x40002AE5, 0x9C598AC8 */
const Q_S3: f64 = -6.88283971605453293030e-01; /* 0xBFE6066C, 0x1B8D0159 */
const Q_S4: f64 = 7.70381505559019352791e-02; /* 0x3FB3B8C5, 0xB12E9282 */

fn comp_r(z: f64) -> f64 {
    let p = z * (P_S0 + z * (P_S1 + z * (P_S2 + z * (P_S3 + z * (P_S4 + z * P_S5)))));
    let q = 1.0 + z * (Q_S1 + z * (Q_S2 + z * (Q_S3 + z * Q_S4)));
    p / q
}

/// Arcsine (f64)
///
/// Computes the inverse sine (arc sine) of the argument `x`.
/// Arguments to asin must be in the range -1 to 1.
/// Returns values in radians, in the range of -pi/2 to pi/2.
pub fn asin(mut x: f64) -> f64 {
    let z: f64;
    let r: f64;
    let s: f64;
    let hx: u32;
    let ix: u32;

    hx = get_high_word(x);
    ix = hx & 0x7fffffff;
    /* |x| >= 1 or nan */
    if ix >= 0x3ff00000 {
        let lx: u32;
        lx = get_low_word(x);
        if ((ix - 0x3ff00000) | lx) == 0 {
            /* asin(1) = +-pi/2 with inexact */
            return x * PIO2_HI + f64::from_bits(0x3870000000000000);
        } else {
            return 0.0 / (x - x);
        }
    }
    /* |x| < 0.5 */
    if ix < 0x3fe00000 {
        /* if 0x1p-1022 <= |x| < 0x1p-26, avoid raising underflow */
        if (0x00100000..0x3e500000).contains(&ix) {
            return x;
        } else {
            return x + x * comp_r(x * x);
        }
    }
    /* 1 > |x| >= 0.5 */
    z = (1.0 - f64::abs(x)) * 0.5;
    s = f64::sqrt(z);
    r = comp_r(z);
    if ix >= 0x3fef3333 {
        /* if |x| > 0.975 */
        x = PIO2_HI - (2. * (s + s * r) - PIO2_LO);
    } else {
        let f: f64;
        let c: f64;
        /* f+c = sqrt(z) */
        f = with_set_low_word(s, 0);
        c = (z - f * f) / (s + f);
        x = 0.5 * PIO2_HI - (2.0 * s * r - (PIO2_LO - 2.0 * c) - (0.5 * PIO2_HI - 2.0 * f));
    }
    if hx >> 31 != 0 { -x } else { x }
}

/* origin: FreeBSD /usr/src/lib/msun/src/e_acos.c */
/*
 * ====================================================
 * Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
 *
 * Developed at SunSoft, a Sun Microsystems, Inc. business.
 * Permission to use, copy, modify, and distribute this
 * software is freely granted, provided that this notice
 * is preserved.
 * ====================================================
 */
/* acos(x)
 * Method :
 *      acos(x)  = pi/2 - asin(x)
 *      acos(-x) = pi/2 + asin(x)
 * For |x|<=0.5
 *      acos(x) = pi/2 - (x + x*x^2*R(x^2))     (see asin.c)
 * For x>0.5
 *      acos(x) = pi/2 - (pi/2 - 2asin(sqrt((1-x)/2)))
 *              = 2asin(sqrt((1-x)/2))
 *              = 2s + 2s*z*R(z)        ...z=(1-x)/2, s=sqrt(z)
 *              = 2f + (2c + 2s*z*R(z))
 *     where f=hi part of s, and c = (z-f*f)/(s+f) is the correction term
 *     for f so that f+c ~ sqrt(z).
 * For x<-0.5
 *      acos(x) = pi - 2asin(sqrt((1-|x|)/2))
 *              = pi - 0.5*(s+s*z*R(z)), where z=(1-|x|)/2,s=sqrt(z)
 *
 * Special cases:
 *      if x is NaN, return x itself;
 *      if |x|>1, return NaN with invalid signal.
 *
 * Function needed: sqrt
 */

/// Arccosine (f64)
///
/// Computes the inverse cosine (arc cosine) of the input value.
/// Arguments must be in the range -1 to 1.
/// Returns values in radians, in the range of 0 to pi.
pub fn acos(x: f64) -> f64 {
    const PIO2_HI: f64 = 1.57079632679489655800e+00; /* 0x3FF921FB, 0x54442D18 */
    const PIO2_LO: f64 = 6.12323399573676603587e-17; /* 0x3C91A626, 0x33145C07 */
    const PS0: f64 = 1.66666666666666657415e-01; /* 0x3FC55555, 0x55555555 */
    const PS1: f64 = -3.25565818622400915405e-01; /* 0xBFD4D612, 0x03EB6F7D */
    const PS2: f64 = 2.01212532134862925881e-01; /* 0x3FC9C155, 0x0E884455 */
    const PS3: f64 = -4.00555345006794114027e-02; /* 0xBFA48228, 0xB5688F3B */
    const PS4: f64 = 7.91534994289814532176e-04; /* 0x3F49EFE0, 0x7501B288 */
    const PS5: f64 = 3.47933107596021167570e-05; /* 0x3F023DE1, 0x0DFDF709 */
    const QS1: f64 = -2.40339491173441421878e+00; /* 0xC0033A27, 0x1C8A2D4B */
    const QS2: f64 = 2.02094576023350569471e+00; /* 0x40002AE5, 0x9C598AC8 */
    const QS3: f64 = -6.88283971605453293030e-01; /* 0xBFE6066C, 0x1B8D0159 */
    const QS4: f64 = 7.70381505559019352791e-02; /* 0x3FB3B8C5, 0xB12E9282 */

    fn r(z: f64) -> f64 {
        let p: f64 = z * (PS0 + z * (PS1 + z * (PS2 + z * (PS3 + z * (PS4 + z * PS5)))));
        let q: f64 = 1.0 + z * (QS1 + z * (QS2 + z * (QS3 + z * QS4)));
        p / q
    }

    let x1p_120f = f64::from_bits(0x3870000000000000); // 0x1p-120 === 2 ^ -120
    let z: f64;
    let w: f64;
    let s: f64;
    let c: f64;
    let df: f64;
    let hx: u32;
    let ix: u32;

    hx = (x.to_bits() >> 32) as u32;
    ix = hx & 0x7fffffff;
    /* |x| >= 1 or nan */
    if ix >= 0x3ff00000 {
        let lx: u32 = x.to_bits() as u32;

        if ((ix - 0x3ff00000) | lx) == 0 {
            /* acos(1)=0, acos(-1)=pi */
            if (hx >> 31) != 0 {
                return 2. * PIO2_HI + x1p_120f;
            }
            return 0.;
        }
        return 0. / (x - x);
    }
    /* |x| < 0.5 */
    if ix < 0x3fe00000 {
        if ix <= 0x3c600000 {
            /* |x| < 2**-57 */
            return PIO2_HI + x1p_120f;
        }
        return PIO2_HI - (x - (PIO2_LO - x * r(x * x)));
    }
    /* x < -0.5 */
    if (hx >> 31) != 0 {
        z = (1.0 + x) * 0.5;
        s = f64::sqrt(z);
        w = r(z) * s - PIO2_LO;
        return 2. * (PIO2_HI - (s + w));
    }
    /* x > 0.5 */
    z = (1.0 - x) * 0.5;
    s = f64::sqrt(z);
    // Set the low 4 bytes to zero
    df = f64::from_bits(s.to_bits() & 0xff_ff_ff_ff_00_00_00_00);

    c = (z - df * df) / (s + df);
    w = r(z) * s + c;
    2. * (df + w)
}

/* origin: FreeBSD /usr/src/lib/msun/src/s_atan.c */
/*
 * ====================================================
 * Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
 *
 * Developed at SunPro, a Sun Microsystems, Inc. business.
 * Permission to use, copy, modify, and distribute this
 * software is freely granted, provided that this notice
 * is preserved.
 * ====================================================
 */
/* atan(x)
 * Method
 *   1. Reduce x to positive by atan(x) = -atan(-x).
 *   2. According to the integer k=4t+0.25 chopped, t=x, the argument
 *      is further reduced to one of the following intervals and the
 *      arctangent of t is evaluated by the corresponding formula:
 *
 *      [0,7/16]      atan(x) = t-t^3*(a1+t^2*(a2+...(a10+t^2*a11)...)
 *      [7/16,11/16]  atan(x) = atan(1/2) + atan( (t-0.5)/(1+t/2) )
 *      [11/16.19/16] atan(x) = atan( 1 ) + atan( (t-1)/(1+t) )
 *      [19/16,39/16] atan(x) = atan(3/2) + atan( (t-1.5)/(1+1.5t) )
 *      [39/16,INF]   atan(x) = atan(INF) + atan( -1/t )
 *
 * Constants:
 * The hexadecimal values are the intended ones for the following
 * constants. The decimal values may be used, provided that the
 * compiler will convert from decimal to binary accurately enough
 * to produce the hexadecimal values shown.
 */

const ATANHI: [f64; 4] = [
    4.63647609000806093515e-01, /* atan(0.5)hi 0x3FDDAC67, 0x0561BB4F */
    7.85398163397448278999e-01, /* atan(1.0)hi 0x3FE921FB, 0x54442D18 */
    9.82793723247329054082e-01, /* atan(1.5)hi 0x3FEF730B, 0xD281F69B */
    1.57079632679489655800e+00, /* atan(inf)hi 0x3FF921FB, 0x54442D18 */
];

const ATANLO: [f64; 4] = [
    2.26987774529616870924e-17, /* atan(0.5)lo 0x3C7A2B7F, 0x222F65E2 */
    3.06161699786838301793e-17, /* atan(1.0)lo 0x3C81A626, 0x33145C07 */
    1.39033110312309984516e-17, /* atan(1.5)lo 0x3C700788, 0x7AF0CBBD */
    6.12323399573676603587e-17, /* atan(inf)lo 0x3C91A626, 0x33145C07 */
];

const AT: [f64; 11] = [
    3.33333333333329318027e-01,  /* 0x3FD55555, 0x5555550D */
    -1.99999999998764832476e-01, /* 0xBFC99999, 0x9998EBC4 */
    1.42857142725034663711e-01,  /* 0x3FC24924, 0x920083FF */
    -1.11111104054623557880e-01, /* 0xBFBC71C6, 0xFE231671 */
    9.09088713343650656196e-02,  /* 0x3FB745CD, 0xC54C206E */
    -7.69187620504482999495e-02, /* 0xBFB3B0F2, 0xAF749A6D */
    6.66107313738753120669e-02,  /* 0x3FB10D66, 0xA0D03D51 */
    -5.83357013379057348645e-02, /* 0xBFADDE2D, 0x52DEFD9A */
    4.97687799461593236017e-02,  /* 0x3FA97B4B, 0x24760DEB */
    -3.65315727442169155270e-02, /* 0xBFA2B444, 0x2C6A6C2F */
    1.62858201153657823623e-02,  /* 0x3F90AD3A, 0xE322DA11 */
];

/// Arctangent (f64)
///
/// Computes the inverse tangent (arc tangent) of the input value.
/// Returns a value in radians, in the range of -pi/2 to pi/2.
pub fn atan(x: f64) -> f64 {
    let mut x = x;
    let mut ix = (x.to_bits() >> 32) as u32;
    let sign = ix >> 31;
    ix &= 0x7fff_ffff;
    if ix >= 0x4410_0000 {
        if x.is_nan() {
            return x;
        }

        let z = ATANHI[3] + f64::from_bits(0x0380_0000); // 0x1p-120f
        return if sign != 0 { -z } else { z };
    }

    let id = if ix < 0x3fdc_0000 {
        /* |x| < 0.4375 */
        if ix < 0x3e40_0000 {
            /* |x| < 2^-27 */
            if ix < 0x0010_0000 {
                /* raise underflow for subnormal x */
                // Floating-point exception flags are unused.
            }

            return x;
        }

        -1
    } else {
        x = f64::abs(x);
        if ix < 0x3ff30000 {
            /* |x| < 1.1875 */
            if ix < 0x3fe60000 {
                /* 7/16 <= |x| < 11/16 */
                x = (2. * x - 1.) / (2. + x);
                0
            } else {
                /* 11/16 <= |x| < 19/16 */
                x = (x - 1.) / (x + 1.);
                1
            }
        } else if ix < 0x40038000 {
            /* |x| < 2.4375 */
            x = (x - 1.5) / (1. + 1.5 * x);
            2
        } else {
            /* 2.4375 <= |x| < 2^66 */
            x = -1. / x;
            3
        }
    };

    let z = x * x;
    let w = z * z;
    /* break sum from i=0 to 10 AT[i]z**(i+1) into odd and even poly */
    let s1 = z * (AT[0] + w * (AT[2] + w * (AT[4] + w * (AT[6] + w * (AT[8] + w * AT[10])))));
    let s2 = w * (AT[1] + w * (AT[3] + w * (AT[5] + w * (AT[7] + w * AT[9]))));

    if id < 0 {
        return x - x * (s1 + s2);
    }

    let z = ATANHI[id as usize] - (x * (s1 + s2) - ATANLO[id as usize] - x);

    if sign != 0 { -z } else { z }
}

/* origin: FreeBSD /usr/src/lib/msun/src/e_atan2.c */
/*
 * ====================================================
 * Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
 *
 * Developed at SunSoft, a Sun Microsystems, Inc. business.
 * Permission to use, copy, modify, and distribute this
 * software is freely granted, provided that this notice
 * is preserved.
 * ====================================================
 *
 */
/* atan2(y,x)
 * Method :
 *      1. Reduce y to positive by atan2(y,x)=-atan2(-y,x).
 *      2. Reduce x to positive by (if x and y are unexceptional):
 *              ARG (x+iy) = arctan(y/x)           ... if x > 0,
 *              ARG (x+iy) = pi - arctan[y/(-x)]   ... if x < 0,
 *
 * Special cases:
 *
 *      ATAN2((anything), NaN ) is NaN;
 *      ATAN2(NAN , (anything) ) is NaN;
 *      ATAN2(+-0, +(anything but NaN)) is +-0  ;
 *      ATAN2(+-0, -(anything but NaN)) is +-pi ;
 *      ATAN2(+-(anything but 0 and NaN), 0) is +-pi/2;
 *      ATAN2(+-(anything but INF and NaN), +INF) is +-0 ;
 *      ATAN2(+-(anything but INF and NaN), -INF) is +-pi;
 *      ATAN2(+-INF,+INF ) is +-pi/4 ;
 *      ATAN2(+-INF,-INF ) is +-3pi/4;
 *      ATAN2(+-INF, (anything but,0,NaN, and INF)) is +-pi/2;
 *
 * Constants:
 * The hexadecimal values are the intended ones for the following
 * constants. The decimal values may be used, provided that the
 * compiler will convert from decimal to binary accurately enough
 * to produce the hexadecimal values shown.
 */

const PI: f64 = 3.1415926535897931160E+00; /* 0x400921FB, 0x54442D18 */
const PI_LO: f64 = 1.2246467991473531772E-16; /* 0x3CA1A626, 0x33145C07 */

/// Arctangent of y/x (f64)
///
/// Computes the inverse tangent (arc tangent) of `y/x`.
/// Produces the correct result even for angles near pi/2 or -pi/2 (that is, when `x` is near 0).
/// Returns a value in radians, in the range of -pi to pi.
pub fn atan2(y: f64, x: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return x + y;
    }
    let mut ix = (x.to_bits() >> 32) as u32;
    let lx = x.to_bits() as u32;
    let mut iy = (y.to_bits() >> 32) as u32;
    let ly = y.to_bits() as u32;
    if ((ix.wrapping_sub(0x3ff00000)) | lx) == 0 {
        /* x = 1.0 */
        return atan(y);
    }
    let m = ((iy >> 31) & 1) | ((ix >> 30) & 2); /* 2*sign(x)+sign(y) */
    ix &= 0x7fffffff;
    iy &= 0x7fffffff;

    /* when y = 0 */
    if (iy | ly) == 0 {
        return match m {
            0 | 1 => y, /* atan(+-0,+anything)=+-0 */
            2 => PI,    /* atan(+0,-anything) = PI */
            _ => -PI,   /* atan(-0,-anything) =-PI */
        };
    }
    /* when x = 0 */
    if (ix | lx) == 0 {
        return if m & 1 != 0 { -PI / 2.0 } else { PI / 2.0 };
    }
    /* when x is INF */
    if ix == 0x7ff00000 {
        if iy == 0x7ff00000 {
            return match m {
                0 => PI / 4.0,        /* atan(+INF,+INF) */
                1 => -PI / 4.0,       /* atan(-INF,+INF) */
                2 => 3.0 * PI / 4.0,  /* atan(+INF,-INF) */
                _ => -3.0 * PI / 4.0, /* atan(-INF,-INF) */
            };
        } else {
            return match m {
                0 => 0.0,  /* atan(+...,+INF) */
                1 => -0.0, /* atan(-...,+INF) */
                2 => PI,   /* atan(+...,-INF) */
                _ => -PI,  /* atan(-...,-INF) */
            };
        }
    }
    /* |y/x| > 0x1p64 */
    if ix.wrapping_add(64 << 20) < iy || iy == 0x7ff00000 {
        return if m & 1 != 0 { -PI / 2.0 } else { PI / 2.0 };
    }

    /* z = atan(|y/x|) without spurious underflow */
    let z = if (m & 2 != 0) && iy.wrapping_add(64 << 20) < ix {
        /* |y/x| < 0x1p-64, x<0 */
        0.0
    } else {
        atan(f64::abs(y / x))
    };
    match m {
        0 => z,                /* atan(+,+) */
        1 => -z,               /* atan(-,+) */
        2 => PI - (z - PI_LO), /* atan(+,-) */
        _ => (z - PI_LO) - PI, /* atan(-,-) */
    }
}

/* origin: FreeBSD /usr/src/lib/msun/src/e_exp.c */
/*
 * ====================================================
 * Copyright (C) 2004 by Sun Microsystems, Inc. All rights reserved.
 *
 * Permission to use, copy, modify, and distribute this
 * software is freely granted, provided that this notice
 * is preserved.
 * ====================================================
 */
/* exp(x)
 * Returns the exponential of x.
 *
 * Method
 *   1. Argument reduction:
 *      Reduce x to an r so that |r| <= 0.5*ln2 ~ 0.34658.
 *      Given x, find r and integer k such that
 *
 *               x = k*ln2 + r,  |r| <= 0.5*ln2.
 *
 *      Here r will be represented as r = hi-lo for better
 *      accuracy.
 *
 *   2. Approximation of exp(r) by a special rational function on
 *      the interval [0,0.34658]:
 *      Write
 *          R(r**2) = r*(exp(r)+1)/(exp(r)-1) = 2 + r*r/6 - r**4/360 + ...
 *      We use a special Remez algorithm on [0,0.34658] to generate
 *      a polynomial of degree 5 to approximate R. The maximum error
 *      of this polynomial approximation is bounded by 2**-59. In
 *      other words,
 *          R(z) ~ 2.0 + P1*z + P2*z**2 + P3*z**3 + P4*z**4 + P5*z**5
 *      (where z=r*r, and the values of P1 to P5 are listed below)
 *      and
 *          |                  5          |     -59
 *          | 2.0+P1*z+...+P5*z   -  R(z) | <= 2
 *          |                             |
 *      The computation of exp(r) thus becomes
 *                              2*r
 *              exp(r) = 1 + ----------
 *                            R(r) - r
 *                                 r*c(r)
 *                     = 1 + r + ----------- (for better accuracy)
 *                                2 - c(r)
 *      where
 *                              2       4             10
 *              c(r) = r - (P1*r  + P2*r  + ... + P5*r   ).
 *
 *   3. Scale back to obtain exp(x):
 *      From step 1, we have
 *         exp(x) = 2^k * exp(r)
 *
 * Special cases:
 *      exp(INF) is INF, exp(NaN) is NaN;
 *      exp(-INF) is 0, and
 *      for finite argument, only exp(0)=1 is exact.
 *
 * Accuracy:
 *      according to an error analysis, the error is always less than
 *      1 ulp (unit in the last place).
 *
 * Misc. info.
 *      For IEEE double
 *          if x >  709.782712893383973096 then exp(x) overflows
 *          if x < -745.133219101941108420 then exp(x) underflows
 */

const HALF: [f64; 2] = [0.5, -0.5];
const LN2HI: f64 = 6.93147180369123816490e-01; /* 0x3fe62e42, 0xfee00000 */
const LN2LO: f64 = 1.90821492927058770002e-10; /* 0x3dea39ef, 0x35793c76 */
const INVLN2: f64 = 1.44269504088896338700e+00; /* 0x3ff71547, 0x652b82fe */
const P1: f64 = 1.66666666666666019037e-01; /* 0x3FC55555, 0x5555553E */
const P2: f64 = -2.77777777770155933842e-03; /* 0xBF66C16C, 0x16BEBD93 */
const P3: f64 = 6.61375632143793436117e-05; /* 0x3F11566A, 0xAF25DE2C */
const P4: f64 = -1.65339022054652515390e-06; /* 0xBEBBBD41, 0xC5D26BF1 */
const P5: f64 = 4.13813679705723846039e-08; /* 0x3E663769, 0x72BEA4D0 */

/// Exponential, base *e* (f64)
///
/// Calculate the exponential of `x`, that is, *e* raised to the power `x`
/// (where *e* is the base of the natural system of logarithms, approximately 2.71828).
pub fn exp(mut x: f64) -> f64 {
    let x1p1023 = f64::from_bits(0x7fe0000000000000); // 0x1p1023 === 2 ^ 1023
    let _x1p_149 = f64::from_bits(0x36a0000000000000); // 0x1p-149 === 2 ^ -149

    let hi: f64;
    let lo: f64;
    let c: f64;
    let xx: f64;
    let y: f64;
    let k: i32;
    let sign: i32;
    let mut hx: u32;

    hx = (x.to_bits() >> 32) as u32;
    sign = (hx >> 31) as i32;
    hx &= 0x7fffffff; /* high word of |x| */

    /* special cases */
    if hx >= 0x4086232b {
        /* if |x| >= 708.39... */
        if x.is_nan() {
            return x;
        }
        if x > 709.782712893383973096 {
            /* overflow if x!=inf */
            x *= x1p1023;
            return x;
        }
        if x < -708.39641853226410622 {
            /* underflow if x!=-inf */
            // Floating-point exception flags are unused.
            if x < -745.13321910194110842 {
                return 0.;
            }
        }
    }

    /* argument reduction */
    if hx > 0x3fd62e42 {
        /* if |x| > 0.5 ln2 */
        if hx >= 0x3ff0a2b2 {
            /* if |x| >= 1.5 ln2 */
            k = (INVLN2 * x + HALF[sign as usize]) as i32;
        } else {
            k = 1 - sign - sign;
        }
        hi = x - k as f64 * LN2HI; /* k*ln2hi is exact here */
        lo = k as f64 * LN2LO;
        x = hi - lo;
    } else if hx > 0x3e300000 {
        /* if |x| > 2**-28 */
        k = 0;
        hi = x;
        lo = 0.;
    } else {
        /* inexact if x!=0 */
        // Floating-point exception flags are unused.
        return 1. + x;
    }

    /* x is now in primary range */
    xx = x * x;
    c = x - xx * (P1 + xx * (P2 + xx * (P3 + xx * (P4 + xx * P5))));
    y = 1. + (x * c / (2. - c) - lo + hi);
    if k == 0 { y } else { scalbn(y, k) }
}

/* origin: FreeBSD /usr/src/lib/msun/src/s_expm1.c */
/*
 * ====================================================
 * Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
 *
 * Developed at SunPro, a Sun Microsystems, Inc. business.
 * Permission to use, copy, modify, and distribute this
 * software is freely granted, provided that this notice
 * is preserved.
 * ====================================================
 */

/// Exponential, base *e*, of x-1 (f64)
///
/// Calculates the exponential of `x` and subtract 1, that is, *e* raised
/// to the power `x` minus 1 (where *e* is the base of the natural
/// system of logarithms, approximately 2.71828).
/// The result is accurate even for small values of `x`,
/// where using `exp(x)-1` would lose many significant digits.
pub fn exp_m1(mut x: f64) -> f64 {
    const O_THRESHOLD: f64 = 7.09782712893383973096e+02; /* 0x40862E42, 0xFEFA39EF */
    const LN2_HI: f64 = 6.93147180369123816490e-01; /* 0x3fe62e42, 0xfee00000 */
    const LN2_LO: f64 = 1.90821492927058770002e-10; /* 0x3dea39ef, 0x35793c76 */
    const INVLN2: f64 = 1.44269504088896338700e+00; /* 0x3ff71547, 0x652b82fe */
    /* Scaled Q's: Qn_here = 2**n * Qn_above, for R(2*z) where z = hxs = x*x/2: */
    const Q1: f64 = -3.33333333333331316428e-02; /* BFA11111 111110F4 */
    const Q2: f64 = 1.58730158725481460165e-03; /* 3F5A01A0 19FE5585 */
    const Q3: f64 = -7.93650757867487942473e-05; /* BF14CE19 9EAADBB7 */
    const Q4: f64 = 4.00821782732936239552e-06; /* 3ED0CFCA 86E65239 */
    const Q5: f64 = -2.01099218183624371326e-07; /* BE8AFDB7 6E09C32D */

    let hi: f64;
    let lo: f64;
    let k: i32;
    let c: f64;
    let mut t: f64;
    let mut y: f64;

    let mut ui = x.to_bits();
    let hx = ((ui >> 32) & 0x7fffffff) as u32;
    let sign = (ui >> 63) as i32;

    /* filter out huge and non-finite argument */
    if hx >= 0x4043687A {
        /* if |x|>=56*ln2 */
        if x.is_nan() {
            return x;
        }
        if sign != 0 {
            return -1.0;
        }
        if x > O_THRESHOLD {
            x *= f64::from_bits(0x7fe0000000000000);
            return x;
        }
    }

    /* argument reduction */
    if hx > 0x3fd62e42 {
        /* if  |x| > 0.5 ln2 */
        if hx < 0x3FF0A2B2 {
            /* and |x| < 1.5 ln2 */
            if sign == 0 {
                hi = x - LN2_HI;
                lo = LN2_LO;
                k = 1;
            } else {
                hi = x + LN2_HI;
                lo = -LN2_LO;
                k = -1;
            }
        } else {
            k = (INVLN2 * x + if sign != 0 { -0.5 } else { 0.5 }) as i32;
            t = k as f64;
            hi = x - t * LN2_HI; /* t*ln2_hi is exact here */
            lo = t * LN2_LO;
        }
        x = hi - lo;
        c = (hi - x) - lo;
    } else if hx < 0x3c900000 {
        /* |x| < 2**-54, return x */
        if hx < 0x00100000 {
            // Floating-point exception flags are unused.
        }
        return x;
    } else {
        c = 0.0;
        k = 0;
    }

    /* x is now in primary range */
    let hfx = 0.5 * x;
    let hxs = x * hfx;
    let r1 = 1.0 + hxs * (Q1 + hxs * (Q2 + hxs * (Q3 + hxs * (Q4 + hxs * Q5))));
    t = 3.0 - r1 * hfx;
    let mut e = hxs * ((r1 - t) / (6.0 - x * t));
    if k == 0 {
        /* c is 0 */
        return x - (x * e - hxs);
    }
    e = x * (e - c) - c;
    e -= hxs;
    /* exp(x) ~ 2^k (x_reduced - e + 1) */
    if k == -1 {
        return 0.5 * (x - e) - 0.5;
    }
    if k == 1 {
        if x < -0.25 {
            return -2.0 * (e - (x + 0.5));
        }
        return 1.0 + 2.0 * (x - e);
    }
    ui = ((0x3ff + k) as u64) << 52; /* 2^k */
    let twopk = f64::from_bits(ui);
    if !(0..=56).contains(&k) {
        /* suffice to return exp(x)-1 */
        y = x - e + 1.0;
        if k == 1024 {
            y = y * 2.0 * f64::from_bits(0x7fe0000000000000);
        } else {
            y = y * twopk;
        }
        return y - 1.0;
    }
    ui = ((0x3ff - k) as u64) << 52; /* 2^-k */
    let uf = f64::from_bits(ui);
    if k < 20 {
        y = (x - e + (1.0 - uf)) * twopk;
    } else {
        y = (x - (e + uf) + 1.0) * twopk;
    }
    y
}

// origin: FreeBSD /usr/src/lib/msun/src/s_exp2.c */
//-
// Copyright (c) 2005 David Schultz <das@FreeBSD.ORG>
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions
// are met:
// 1. Redistributions of source code must retain the above copyright
//    notice, this list of conditions and the following disclaimer.
// 2. Redistributions in binary form must reproduce the above copyright
//    notice, this list of conditions and the following disclaimer in the
//    documentation and/or other materials provided with the distribution.
//
// THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND
// ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
// ARE DISCLAIMED.  IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE
// FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
// DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
// OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
// HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
// LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
// OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
// SUCH DAMAGE.

const TBLSIZE: usize = 256;

#[rustfmt::skip]
static TBL: [u64; TBLSIZE * 2] = [
    //  exp2(z + eps)          eps
    0x3fe6a09e667f3d5d, 0x3d39880000000000,
    0x3fe6b052fa751744, 0x3cd8000000000000,
    0x3fe6c012750bd9fe, 0xbd28780000000000,
    0x3fe6cfdcddd476bf, 0x3d1ec00000000000,
    0x3fe6dfb23c651a29, 0xbcd8000000000000,
    0x3fe6ef9298593ae3, 0xbcbc000000000000,
    0x3fe6ff7df9519386, 0xbd2fd80000000000,
    0x3fe70f7466f42da3, 0xbd2c880000000000,
    0x3fe71f75e8ec5fc3, 0x3d13c00000000000,
    0x3fe72f8286eacf05, 0xbd38300000000000,
    0x3fe73f9a48a58152, 0xbd00c00000000000,
    0x3fe74fbd35d7ccfc, 0x3d2f880000000000,
    0x3fe75feb564267f1, 0x3d03e00000000000,
    0x3fe77024b1ab6d48, 0xbd27d00000000000,
    0x3fe780694fde5d38, 0xbcdd000000000000,
    0x3fe790b938ac1d00, 0x3ce3000000000000,
    0x3fe7a11473eb0178, 0xbced000000000000,
    0x3fe7b17b0976d060, 0x3d20400000000000,
    0x3fe7c1ed0130c133, 0x3ca0000000000000,
    0x3fe7d26a62ff8636, 0xbd26900000000000,
    0x3fe7e2f336cf4e3b, 0xbd02e00000000000,
    0x3fe7f3878491c3e8, 0xbd24580000000000,
    0x3fe80427543e1b4e, 0x3d33000000000000,
    0x3fe814d2add1071a, 0x3d0f000000000000,
    0x3fe82589994ccd7e, 0xbd21c00000000000,
    0x3fe8364c1eb942d0, 0x3d29d00000000000,
    0x3fe8471a4623cab5, 0x3d47100000000000,
    0x3fe857f4179f5bbc, 0x3d22600000000000,
    0x3fe868d99b4491af, 0xbd32c40000000000,
    0x3fe879cad931a395, 0xbd23000000000000,
    0x3fe88ac7d98a65b8, 0xbd2a800000000000,
    0x3fe89bd0a4785800, 0xbced000000000000,
    0x3fe8ace5422aa223, 0x3d33280000000000,
    0x3fe8be05bad619fa, 0x3d42b40000000000,
    0x3fe8cf3216b54383, 0xbd2ed00000000000,
    0x3fe8e06a5e08664c, 0xbd20500000000000,
    0x3fe8f1ae99157807, 0x3d28280000000000,
    0x3fe902fed0282c0e, 0xbd1cb00000000000,
    0x3fe9145b0b91ff96, 0xbd05e00000000000,
    0x3fe925c353aa2ff9, 0x3cf5400000000000,
    0x3fe93737b0cdc64a, 0x3d17200000000000,
    0x3fe948b82b5f98ae, 0xbd09000000000000,
    0x3fe95a44cbc852cb, 0x3d25680000000000,
    0x3fe96bdd9a766f21, 0xbd36d00000000000,
    0x3fe97d829fde4e2a, 0xbd01000000000000,
    0x3fe98f33e47a23a3, 0x3d2d000000000000,
    0x3fe9a0f170ca0604, 0xbd38a40000000000,
    0x3fe9b2bb4d53ff89, 0x3d355c0000000000,
    0x3fe9c49182a3f15b, 0x3d26b80000000000,
    0x3fe9d674194bb8c5, 0xbcec000000000000,
    0x3fe9e86319e3238e, 0x3d17d00000000000,
    0x3fe9fa5e8d07f302, 0x3d16400000000000,
    0x3fea0c667b5de54d, 0xbcf5000000000000,
    0x3fea1e7aed8eb8f6, 0x3d09e00000000000,
    0x3fea309bec4a2e27, 0x3d2ad80000000000,
    0x3fea42c980460a5d, 0xbd1af00000000000,
    0x3fea5503b23e259b, 0x3d0b600000000000,
    0x3fea674a8af46213, 0x3d38880000000000,
    0x3fea799e1330b3a7, 0x3d11200000000000,
    0x3fea8bfe53c12e8d, 0x3d06c00000000000,
    0x3fea9e6b5579fcd2, 0xbd29b80000000000,
    0x3feab0e521356fb8, 0x3d2b700000000000,
    0x3feac36bbfd3f381, 0x3cd9000000000000,
    0x3fead5ff3a3c2780, 0x3ce4000000000000,
    0x3feae89f995ad2a3, 0xbd2c900000000000,
    0x3feafb4ce622f367, 0x3d16500000000000,
    0x3feb0e07298db790, 0x3d2fd40000000000,
    0x3feb20ce6c9a89a9, 0x3d12700000000000,
    0x3feb33a2b84f1a4b, 0x3d4d470000000000,
    0x3feb468415b747e7, 0xbd38380000000000,
    0x3feb59728de5593a, 0x3c98000000000000,
    0x3feb6c6e29f1c56a, 0x3d0ad00000000000,
    0x3feb7f76f2fb5e50, 0x3cde800000000000,
    0x3feb928cf22749b2, 0xbd04c00000000000,
    0x3feba5b030a10603, 0xbd0d700000000000,
    0x3febb8e0b79a6f66, 0x3d0d900000000000,
    0x3febcc1e904bc1ff, 0x3d02a00000000000,
    0x3febdf69c3f3a16f, 0xbd1f780000000000,
    0x3febf2c25bd71db8, 0xbd10a00000000000,
    0x3fec06286141b2e9, 0xbd11400000000000,
    0x3fec199bdd8552e0, 0x3d0be00000000000,
    0x3fec2d1cd9fa64ee, 0xbd09400000000000,
    0x3fec40ab5fffd02f, 0xbd0ed00000000000,
    0x3fec544778fafd15, 0x3d39660000000000,
    0x3fec67f12e57d0cb, 0xbd1a100000000000,
    0x3fec7ba88988c1b6, 0xbd58458000000000,
    0x3fec8f6d9406e733, 0xbd1a480000000000,
    0x3feca3405751c4df, 0x3ccb000000000000,
    0x3fecb720dcef9094, 0x3d01400000000000,
    0x3feccb0f2e6d1689, 0x3cf0200000000000,
    0x3fecdf0b555dc412, 0x3cf3600000000000,
    0x3fecf3155b5bab3b, 0xbd06900000000000,
    0x3fed072d4a0789bc, 0x3d09a00000000000,
    0x3fed1b532b08c8fa, 0xbd15e00000000000,
    0x3fed2f87080d8a85, 0x3d1d280000000000,
    0x3fed43c8eacaa203, 0x3d01a00000000000,
    0x3fed5818dcfba491, 0x3cdf000000000000,
    0x3fed6c76e862e6a1, 0xbd03a00000000000,
    0x3fed80e316c9834e, 0xbd0cd80000000000,
    0x3fed955d71ff6090, 0x3cf4c00000000000,
    0x3feda9e603db32ae, 0x3cff900000000000,
    0x3fedbe7cd63a8325, 0x3ce9800000000000,
    0x3fedd321f301b445, 0xbcf5200000000000,
    0x3fede7d5641c05bf, 0xbd1d700000000000,
    0x3fedfc97337b9aec, 0xbd16140000000000,
    0x3fee11676b197d5e, 0x3d0b480000000000,
    0x3fee264614f5a3e7, 0x3d40ce0000000000,
    0x3fee3b333b16ee5c, 0x3d0c680000000000,
    0x3fee502ee78b3fb4, 0xbd09300000000000,
    0x3fee653924676d68, 0xbce5000000000000,
    0x3fee7a51fbc74c44, 0xbd07f80000000000,
    0x3fee8f7977cdb726, 0xbcf3700000000000,
    0x3feea4afa2a490e8, 0x3ce5d00000000000,
    0x3feeb9f4867ccae4, 0x3d161a0000000000,
    0x3feecf482d8e680d, 0x3cf5500000000000,
    0x3feee4aaa2188514, 0x3cc6400000000000,
    0x3feefa1bee615a13, 0xbcee800000000000,
    0x3fef0f9c1cb64106, 0xbcfa880000000000,
    0x3fef252b376bb963, 0xbd2c900000000000,
    0x3fef3ac948dd7275, 0x3caa000000000000,
    0x3fef50765b6e4524, 0xbcf4f00000000000,
    0x3fef6632798844fd, 0x3cca800000000000,
    0x3fef7bfdad9cbe38, 0x3cfabc0000000000,
    0x3fef91d802243c82, 0xbcd4600000000000,
    0x3fefa7c1819e908e, 0xbd0b0c0000000000,
    0x3fefbdba3692d511, 0xbcc0e00000000000,
    0x3fefd3c22b8f7194, 0xbd10de8000000000,
    0x3fefe9d96b2a23ee, 0x3cee430000000000,
    0x3ff0000000000000, 0x0,
    0x3ff00b1afa5abcbe, 0xbcb3400000000000,
    0x3ff0163da9fb3303, 0xbd12170000000000,
    0x3ff02168143b0282, 0x3cba400000000000,
    0x3ff02c9a3e77806c, 0x3cef980000000000,
    0x3ff037d42e11bbca, 0xbcc7400000000000,
    0x3ff04315e86e7f89, 0x3cd8300000000000,
    0x3ff04e5f72f65467, 0xbd1a3f0000000000,
    0x3ff059b0d315855a, 0xbd02840000000000,
    0x3ff0650a0e3c1f95, 0x3cf1600000000000,
    0x3ff0706b29ddf71a, 0x3d15240000000000,
    0x3ff07bd42b72a82d, 0xbce9a00000000000,
    0x3ff0874518759bd0, 0x3ce6400000000000,
    0x3ff092bdf66607c8, 0xbd00780000000000,
    0x3ff09e3ecac6f383, 0xbc98000000000000,
    0x3ff0a9c79b1f3930, 0x3cffa00000000000,
    0x3ff0b5586cf988fc, 0xbcfac80000000000,
    0x3ff0c0f145e46c8a, 0x3cd9c00000000000,
    0x3ff0cc922b724816, 0x3d05200000000000,
    0x3ff0d83b23395dd8, 0xbcfad00000000000,
    0x3ff0e3ec32d3d1f3, 0x3d1bac0000000000,
    0x3ff0efa55fdfa9a6, 0xbd04e80000000000,
    0x3ff0fb66affed2f0, 0xbd0d300000000000,
    0x3ff1073028d7234b, 0x3cf1500000000000,
    0x3ff11301d0125b5b, 0x3cec000000000000,
    0x3ff11edbab5e2af9, 0x3d16bc0000000000,
    0x3ff12abdc06c31d5, 0x3ce8400000000000,
    0x3ff136a814f2047d, 0xbd0ed00000000000,
    0x3ff1429aaea92de9, 0x3ce8e00000000000,
    0x3ff14e95934f3138, 0x3ceb400000000000,
    0x3ff15a98c8a58e71, 0x3d05300000000000,
    0x3ff166a45471c3df, 0x3d03380000000000,
    0x3ff172b83c7d5211, 0x3d28d40000000000,
    0x3ff17ed48695bb9f, 0xbd05d00000000000,
    0x3ff18af9388c8d93, 0xbd1c880000000000,
    0x3ff1972658375d66, 0x3d11f00000000000,
    0x3ff1a35beb6fcba7, 0x3d10480000000000,
    0x3ff1af99f81387e3, 0xbd47390000000000,
    0x3ff1bbe084045d54, 0x3d24e40000000000,
    0x3ff1c82f95281c43, 0xbd0a200000000000,
    0x3ff1d4873168b9b2, 0x3ce3800000000000,
    0x3ff1e0e75eb44031, 0x3ceac00000000000,
    0x3ff1ed5022fcd938, 0x3d01900000000000,
    0x3ff1f9c18438cdf7, 0xbd1b780000000000,
    0x3ff2063b88628d8f, 0x3d2d940000000000,
    0x3ff212be3578a81e, 0x3cd8000000000000,
    0x3ff21f49917ddd41, 0x3d2b340000000000,
    0x3ff22bdda2791323, 0x3d19f80000000000,
    0x3ff2387a6e7561e7, 0xbd19c80000000000,
    0x3ff2451ffb821427, 0x3d02300000000000,
    0x3ff251ce4fb2a602, 0xbd13480000000000,
    0x3ff25e85711eceb0, 0x3d12700000000000,
    0x3ff26b4565e27d16, 0x3d11d00000000000,
    0x3ff2780e341de00f, 0x3d31ee0000000000,
    0x3ff284dfe1f5633e, 0xbd14c00000000000,
    0x3ff291ba7591bb30, 0xbd13d80000000000,
    0x3ff29e9df51fdf09, 0x3d08b00000000000,
    0x3ff2ab8a66d10e9b, 0xbd227c0000000000,
    0x3ff2b87fd0dada3a, 0x3d2a340000000000,
    0x3ff2c57e39771af9, 0xbd10800000000000,
    0x3ff2d285a6e402d9, 0xbd0ed00000000000,
    0x3ff2df961f641579, 0xbcf4200000000000,
    0x3ff2ecafa93e2ecf, 0xbd24980000000000,
    0x3ff2f9d24abd8822, 0xbd16300000000000,
    0x3ff306fe0a31b625, 0xbd32360000000000,
    0x3ff31432edeea50b, 0xbd70df8000000000,
    0x3ff32170fc4cd7b8, 0xbd22480000000000,
    0x3ff32eb83ba8e9a2, 0xbd25980000000000,
    0x3ff33c08b2641766, 0x3d1ed00000000000,
    0x3ff3496266e3fa27, 0xbcdc000000000000,
    0x3ff356c55f929f0f, 0xbd30d80000000000,
    0x3ff36431a2de88b9, 0x3d22c80000000000,
    0x3ff371a7373aaa39, 0x3d20600000000000,
    0x3ff37f26231e74fe, 0xbd16600000000000,
    0x3ff38cae6d05d838, 0xbd0ae00000000000,
    0x3ff39a401b713ec3, 0xbd44720000000000,
    0x3ff3a7db34e5a020, 0x3d08200000000000,
    0x3ff3b57fbfec6e95, 0x3d3e800000000000,
    0x3ff3c32dc313a8f2, 0x3cef800000000000,
    0x3ff3d0e544ede122, 0xbd17a00000000000,
    0x3ff3dea64c1234bb, 0x3d26300000000000,
    0x3ff3ec70df1c4ecc, 0xbd48a60000000000,
    0x3ff3fa4504ac7e8c, 0xbd3cdc0000000000,
    0x3ff40822c367a0bb, 0x3d25b80000000000,
    0x3ff4160a21f72e95, 0x3d1ec00000000000,
    0x3ff423fb27094646, 0xbd13600000000000,
    0x3ff431f5d950a920, 0x3d23980000000000,
    0x3ff43ffa3f84b9eb, 0x3cfa000000000000,
    0x3ff44e0860618919, 0xbcf6c00000000000,
    0x3ff45c2042a7d201, 0xbd0bc00000000000,
    0x3ff46a41ed1d0016, 0xbd12800000000000,
    0x3ff4786d668b3326, 0x3d30e00000000000,
    0x3ff486a2b5c13c00, 0xbd2d400000000000,
    0x3ff494e1e192af04, 0x3d0c200000000000,
    0x3ff4a32af0d7d372, 0xbd1e500000000000,
    0x3ff4b17dea6db801, 0x3d07800000000000,
    0x3ff4bfdad53629e1, 0xbd13800000000000,
    0x3ff4ce41b817c132, 0x3d00800000000000,
    0x3ff4dcb299fddddb, 0x3d2c700000000000,
    0x3ff4eb2d81d8ab96, 0xbd1ce00000000000,
    0x3ff4f9b2769d2d02, 0x3d19200000000000,
    0x3ff508417f4531c1, 0xbd08c00000000000,
    0x3ff516daa2cf662a, 0xbcfa000000000000,
    0x3ff5257de83f51ea, 0x3d4a080000000000,
    0x3ff5342b569d4eda, 0xbd26d80000000000,
    0x3ff542e2f4f6ac1a, 0xbd32440000000000,
    0x3ff551a4ca5d94db, 0x3d483c0000000000,
    0x3ff56070dde9116b, 0x3d24b00000000000,
    0x3ff56f4736b529de, 0x3d415a0000000000,
    0x3ff57e27dbe2c40e, 0xbd29e00000000000,
    0x3ff58d12d497c76f, 0xbd23080000000000,
    0x3ff59c0827ff0b4c, 0x3d4dec0000000000,
    0x3ff5ab07dd485427, 0xbcc4000000000000,
    0x3ff5ba11fba87af4, 0x3d30080000000000,
    0x3ff5c9268a59460b, 0xbd26c80000000000,
    0x3ff5d84590998e3f, 0x3d469a0000000000,
    0x3ff5e76f15ad20e1, 0xbd1b400000000000,
    0x3ff5f6a320dcebca, 0x3d17700000000000,
    0x3ff605e1b976dcb8, 0x3d26f80000000000,
    0x3ff6152ae6cdf715, 0x3d01000000000000,
    0x3ff6247eb03a5531, 0xbd15d00000000000,
    0x3ff633dd1d1929b5, 0xbd12d00000000000,
    0x3ff6434634ccc313, 0xbcea800000000000,
    0x3ff652b9febc8efa, 0xbd28600000000000,
    0x3ff6623882553397, 0x3d71fe0000000000,
    0x3ff671c1c708328e, 0xbd37200000000000,
    0x3ff68155d44ca97e, 0x3ce6800000000000,
    0x3ff690f4b19e9471, 0xbd29780000000000,
];

// exp2(x): compute the base 2 exponential of x
//
// Accuracy: Peak error < 0.503 ulp for normalized results.
//
// Method: (accurate tables)
//
//   Reduce x:
//     x = k + y, for integer k and |y| <= 1/2.
//     Thus we have exp2(x) = 2**k * exp2(y).
//
//   Reduce y:
//     y = i/TBLSIZE + z - eps[i] for integer i near y * TBLSIZE.
//     Thus we have exp2(y) = exp2(i/TBLSIZE) * exp2(z - eps[i]),
//     with |z - eps[i]| <= 2**-9 + 2**-39 for the table used.
//
//   We compute exp2(i/TBLSIZE) via table lookup and exp2(z - eps[i]) via
//   a degree-5 minimax polynomial with maximum error under 1.3 * 2**-61.
//   The values in exp2t[] and eps[] are chosen such that
//   exp2t[i] = exp2(i/TBLSIZE + eps[i]), and eps[i] is a small offset such
//   that exp2t[i] is accurate to 2**-64.
//
//   Note that the range of i is +-TBLSIZE/2, so we actually index the tables
//   by i0 = i + TBLSIZE/2.  For cache efficiency, exp2t[] and eps[] are
//   virtual tables, interleaved in the real table tbl[].
//
//   This method is due to Gal, with many details due to Gal and Bachelis:
//
//      Gal, S. and Bachelis, B.  An Accurate Elementary Mathematical Library
//      for the IEEE Floating Point Standard.  TOMS 17(1), 26-46 (1991).

/// Exponential, base 2 (f64)
///
/// Calculate `2^x`, that is, 2 raised to the power `x`.
pub fn exp2(mut x: f64) -> f64 {
    let redux = f64::from_bits(0x4338000000000000) / TBLSIZE as f64;
    let p1 = f64::from_bits(0x3fe62e42fefa39ef);
    let p2 = f64::from_bits(0x3fcebfbdff82c575);
    let p3 = f64::from_bits(0x3fac6b08d704a0a6);
    let p4 = f64::from_bits(0x3f83b2ab88f70400);
    let p5 = f64::from_bits(0x3f55d88003875c74);

    // double_t r, t, z;
    // uint32_t ix, i0;
    // union {double f; uint64_t i;} u = {x};
    // union {uint32_t u; int32_t i;} k;
    let x1p1023 = f64::from_bits(0x7fe0000000000000);
    let x1p52 = f64::from_bits(0x4330000000000000);
    let _0x1p_149 = f64::from_bits(0xb6a0000000000000);

    /* Filter out exceptional cases. */
    let ui = f64::to_bits(x);
    let ix = (ui >> 32) & 0x7fffffff;
    if ix >= 0x408ff000 {
        /* |x| >= 1022 or nan */
        if ix >= 0x40900000 && ui >> 63 == 0 {
            /* x >= 1024 or nan */
            /* overflow */
            x *= x1p1023;
            return x;
        }
        if ix >= 0x7ff00000 {
            /* -inf or -nan */
            return -1.0 / x;
        }
        if ui >> 63 != 0 {
            /* x <= -1022 */
            /* underflow */
            if x <= -1075.0 || x - x1p52 + x1p52 != x {
                // Floating-point exception flags are unused.
            }
            if x <= -1075.0 {
                return 0.0;
            }
        }
    } else if ix < 0x3c900000 {
        /* |x| < 0x1p-54 */
        return 1.0 + x;
    }

    /* Reduce x, computing z, i0, and k. */
    let ui = f64::to_bits(x + redux);
    let mut i0 = ui as u32;
    i0 = i0.wrapping_add(TBLSIZE as u32 / 2);
    let ku = i0 / TBLSIZE as u32 * TBLSIZE as u32;
    let ki = (ku as i32) / (TBLSIZE as i32);
    i0 %= TBLSIZE as u32;
    let uf = f64::from_bits(ui) - redux;
    let mut z = x - uf;

    /* Compute r = exp2(y) = exp2t[i0] * p(z - eps[i]). */
    let t = f64::from_bits(TBL[2 * i0 as usize]); /* exp2t[i0] */
    z -= f64::from_bits(TBL[2 * i0 as usize + 1]); /* eps[i0]   */
    let r = t + t * z * (p1 + z * (p2 + z * (p3 + z * (p4 + z * p5))));

    scalbn(r, ki)
}

/* origin: FreeBSD /usr/src/lib/msun/src/e_log.c */
/*
 * ====================================================
 * Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
 *
 * Developed at SunSoft, a Sun Microsystems, Inc. business.
 * Permission to use, copy, modify, and distribute this
 * software is freely granted, provided that this notice
 * is preserved.
 * ====================================================
 */
/* log(x)
 * Return the logarithm of x
 *
 * Method :
 *   1. Argument Reduction: find k and f such that
 *                      x = 2^k * (1+f),
 *         where  sqrt(2)/2 < 1+f < sqrt(2) .
 *
 *   2. Approximation of log(1+f).
 *      Let s = f/(2+f) ; based on log(1+f) = log(1+s) - log(1-s)
 *               = 2s + 2/3 s**3 + 2/5 s**5 + .....,
 *               = 2s + s*R
 *      We use a special Remez algorithm on [0,0.1716] to generate
 *      a polynomial of degree 14 to approximate R The maximum error
 *      of this polynomial approximation is bounded by 2**-58.45. In
 *      other words,
 *                      2      4      6      8      10      12      14
 *          R(z) ~ Lg1*s +Lg2*s +Lg3*s +Lg4*s +Lg5*s  +Lg6*s  +Lg7*s
 *      (the values of Lg1 to Lg7 are listed in the program)
 *      and
 *          |      2          14          |     -58.45
 *          | Lg1*s +...+Lg7*s    -  R(z) | <= 2
 *          |                             |
 *      Note that 2s = f - s*f = f - hfsq + s*hfsq, where hfsq = f*f/2.
 *      In order to guarantee error in log below 1ulp, we compute log
 *      by
 *              log(1+f) = f - s*(f - R)        (if f is not too large)
 *              log(1+f) = f - (hfsq - s*(hfsq+R)).     (better accuracy)
 *
 *      3. Finally,  log(x) = k*ln2 + log(1+f).
 *                          = k*ln2_hi+(f-(hfsq-(s*(hfsq+R)+k*ln2_lo)))
 *         Here ln2 is split into two floating point number:
 *                      ln2_hi + ln2_lo,
 *         where n*ln2_hi is always exact for |n| < 2000.
 *
 * Special cases:
 *      log(x) is NaN with signal if x < 0 (including -INF) ;
 *      log(+INF) is +INF; log(0) is -INF with signal;
 *      log(NaN) is that NaN with no signal.
 *
 * Accuracy:
 *      according to an error analysis, the error is always less than
 *      1 ulp (unit in the last place).
 *
 * Constants:
 * The hexadecimal values are the intended ones for the following
 * constants. The decimal values may be used, provided that the
 * compiler will convert from decimal to binary accurately enough
 * to produce the hexadecimal values shown.
 */

/// The natural logarithm of `x` (f64).
pub fn ln(mut x: f64) -> f64 {
    const LN2_HI: f64 = 6.93147180369123816490e-01; /* 3fe62e42 fee00000 */
    const LN2_LO: f64 = 1.90821492927058770002e-10; /* 3dea39ef 35793c76 */
    const LG1: f64 = 6.666666666666735130e-01; /* 3FE55555 55555593 */
    const LG2: f64 = 3.999999999940941908e-01; /* 3FD99999 9997FA04 */
    const LG3: f64 = 2.857142874366239149e-01; /* 3FD24924 94229359 */
    const LG4: f64 = 2.222219843214978396e-01; /* 3FCC71C5 1D8E78AF */
    const LG5: f64 = 1.818357216161805012e-01; /* 3FC74664 96CB03DE */
    const LG6: f64 = 1.531383769920937332e-01; /* 3FC39A09 D078C69F */
    const LG7: f64 = 1.479819860511658591e-01; /* 3FC2F112 DF3E5244 */

    let x1p54 = f64::from_bits(0x4350000000000000); // 0x1p54 === 2 ^ 54

    let mut ui = x.to_bits();
    let mut hx: u32 = (ui >> 32) as u32;
    let mut k: i32 = 0;

    if (hx < 0x00100000) || ((hx >> 31) != 0) {
        /* x < 2**-126  */
        if ui << 1 == 0 {
            return -1. / (x * x); /* log(+-0)=-inf */
        }
        if hx >> 31 != 0 {
            return (x - x) / 0.0; /* log(-#) = NaN */
        }
        /* subnormal number, scale x up */
        k -= 54;
        x *= x1p54;
        ui = x.to_bits();
        hx = (ui >> 32) as u32;
    } else if hx >= 0x7ff00000 {
        return x;
    } else if hx == 0x3ff00000 && ui << 32 == 0 {
        return 0.;
    }

    /* reduce x into [sqrt(2)/2, sqrt(2)] */
    hx += 0x3ff00000 - 0x3fe6a09e;
    k += ((hx >> 20) as i32) - 0x3ff;
    hx = (hx & 0x000fffff) + 0x3fe6a09e;
    ui = ((hx as u64) << 32) | (ui & 0xffffffff);
    x = f64::from_bits(ui);

    let f: f64 = x - 1.0;
    let hfsq: f64 = 0.5 * f * f;
    let s: f64 = f / (2.0 + f);
    let z: f64 = s * s;
    let w: f64 = z * z;
    let t1: f64 = w * (LG2 + w * (LG4 + w * LG6));
    let t2: f64 = z * (LG1 + w * (LG3 + w * (LG5 + w * LG7)));
    let r: f64 = t2 + t1;
    let dk: f64 = k as f64;
    s * (hfsq + r) + dk * LN2_LO - hfsq + f + dk * LN2_HI
}

/* origin: FreeBSD /usr/src/lib/msun/src/s_log1p.c */
/*
 * ====================================================
 * Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
 *
 * Developed at SunPro, a Sun Microsystems, Inc. business.
 * Permission to use, copy, modify, and distribute this
 * software is freely granted, provided that this notice
 * is preserved.
 * ====================================================
 */
/* double log1p(double x)
 * Return the natural logarithm of 1+x.
 *
 * Method :
 *   1. Argument Reduction: find k and f such that
 *                      1+x = 2^k * (1+f),
 *         where  sqrt(2)/2 < 1+f < sqrt(2) .
 *
 *      Note. If k=0, then f=x is exact. However, if k!=0, then f
 *      may not be representable exactly. In that case, a correction
 *      term is need. Let u=1+x rounded. Let c = (1+x)-u, then
 *      log(1+x) - log(u) ~ c/u. Thus, we proceed to compute log(u),
 *      and add back the correction term c/u.
 *      (Note: when x > 2**53, one can simply return log(x))
 *
 *   2. Approximation of log(1+f): See log.c
 *
 *   3. Finally, log1p(x) = k*ln2 + log(1+f) + c/u. See log.c
 *
 * Special cases:
 *      log1p(x) is NaN with signal if x < -1 (including -INF) ;
 *      log1p(+INF) is +INF; log1p(-1) is -INF with signal;
 *      log1p(NaN) is that NaN with no signal.
 *
 * Accuracy:
 *      according to an error analysis, the error is always less than
 *      1 ulp (unit in the last place).
 *
 * Constants:
 * The hexadecimal values are the intended ones for the following
 * constants. The decimal values may be used, provided that the
 * compiler will convert from decimal to binary accurately enough
 * to produce the hexadecimal values shown.
 *
 * Note: Assuming log() return accurate answer, the following
 *       algorithm can be used to compute log1p(x) to within a few ULP:
 *
 *              u = 1+x;
 *              if(u==1.0) return x ; else
 *                         return log(u)*(x/(u-1.0));
 *
 *       See HP-15C Advanced Functions Handbook, p.193.
 */

/// The natural logarithm of 1+`x` (f64).
pub fn ln_1p(x: f64) -> f64 {
    const LN2_HI: f64 = 6.93147180369123816490e-01; /* 3fe62e42 fee00000 */
    const LN2_LO: f64 = 1.90821492927058770002e-10; /* 3dea39ef 35793c76 */
    const LG1: f64 = 6.666666666666735130e-01; /* 3FE55555 55555593 */
    const LG2: f64 = 3.999999999940941908e-01; /* 3FD99999 9997FA04 */
    const LG3: f64 = 2.857142874366239149e-01; /* 3FD24924 94229359 */
    const LG4: f64 = 2.222219843214978396e-01; /* 3FCC71C5 1D8E78AF */
    const LG5: f64 = 1.818357216161805012e-01; /* 3FC74664 96CB03DE */
    const LG6: f64 = 1.531383769920937332e-01; /* 3FC39A09 D078C69F */
    const LG7: f64 = 1.479819860511658591e-01; /* 3FC2F112 DF3E5244 */

    let mut ui: u64 = x.to_bits();
    let hfsq: f64;
    let mut f: f64 = 0.;
    let mut c: f64 = 0.;
    let s: f64;
    let z: f64;
    let r: f64;
    let w: f64;
    let t1: f64;
    let t2: f64;
    let dk: f64;
    let hx: u32;
    let mut hu: u32;
    let mut k: i32;

    hx = (ui >> 32) as u32;
    k = 1;
    if hx < 0x3fda827a || (hx >> 31) > 0 {
        /* 1+x < sqrt(2)+ */
        if hx >= 0xbff00000 {
            /* x <= -1.0 */
            if x == -1. {
                return x / 0.0; /* log1p(-1) = -inf */
            }
            return (x - x) / 0.0; /* log1p(x<-1) = NaN */
        }
        if hx << 1 < 0x3ca00000 << 1 {
            /* |x| < 2**-53 */
            /* underflow if subnormal */
            if (hx & 0x7ff00000) == 0 {
                // Floating-point exception flags are unused.
            }
            return x;
        }
        if hx <= 0xbfd2bec4 {
            /* sqrt(2)/2- <= 1+x < sqrt(2)+ */
            k = 0;
            c = 0.;
            f = x;
        }
    } else if hx >= 0x7ff00000 {
        return x;
    }
    if k > 0 {
        ui = (1. + x).to_bits();
        hu = (ui >> 32) as u32;
        hu += 0x3ff00000 - 0x3fe6a09e;
        k = (hu >> 20) as i32 - 0x3ff;
        /* correction term ~ log(1+x)-log(u), avoid underflow in c/u */
        if k < 54 {
            c = if k >= 2 {
                1. - (f64::from_bits(ui) - x)
            } else {
                x - (f64::from_bits(ui) - 1.)
            };
            c /= f64::from_bits(ui);
        } else {
            c = 0.;
        }
        /* reduce u into [sqrt(2)/2, sqrt(2)] */
        hu = (hu & 0x000fffff) + 0x3fe6a09e;
        ui = ((hu as u64) << 32) | (ui & 0xffffffff);
        f = f64::from_bits(ui) - 1.;
    }
    hfsq = 0.5 * f * f;
    s = f / (2.0 + f);
    z = s * s;
    w = z * z;
    t1 = w * (LG2 + w * (LG4 + w * LG6));
    t2 = z * (LG1 + w * (LG3 + w * (LG5 + w * LG7)));
    r = t2 + t1;
    dk = k as f64;
    s * (hfsq + r) + (dk * LN2_LO + c) - hfsq + f + dk * LN2_HI
}

/* origin: FreeBSD /usr/src/lib/msun/src/e_log2.c */
/*
 * ====================================================
 * Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
 *
 * Developed at SunSoft, a Sun Microsystems, Inc. business.
 * Permission to use, copy, modify, and distribute this
 * software is freely granted, provided that this notice
 * is preserved.
 * ====================================================
 */
/*
 * Return the base 2 logarithm of x.  See log.c for most comments.
 *
 * Reduce x to 2^k (1+f) and calculate r = log(1+f) - f + f*f/2
 * as in log.c, then combine and scale in extra precision:
 *    log2(x) = (f - f*f/2 + r)/log(2) + k
 */

/// The base 2 logarithm of `x` (f64).
pub fn log2(mut x: f64) -> f64 {
    const IVLN2HI: f64 = 1.44269504072144627571e+00; /* 0x3ff71547, 0x65200000 */
    const IVLN2LO: f64 = 1.67517131648865118353e-10; /* 0x3de705fc, 0x2eefa200 */
    const LG1: f64 = 6.666666666666735130e-01; /* 3FE55555 55555593 */
    const LG2: f64 = 3.999999999940941908e-01; /* 3FD99999 9997FA04 */
    const LG3: f64 = 2.857142874366239149e-01; /* 3FD24924 94229359 */
    const LG4: f64 = 2.222219843214978396e-01; /* 3FCC71C5 1D8E78AF */
    const LG5: f64 = 1.818357216161805012e-01; /* 3FC74664 96CB03DE */
    const LG6: f64 = 1.531383769920937332e-01; /* 3FC39A09 D078C69F */
    const LG7: f64 = 1.479819860511658591e-01; /* 3FC2F112 DF3E5244 */

    let x1p54 = f64::from_bits(0x4350000000000000); // 0x1p54 === 2 ^ 54

    let mut ui: u64 = x.to_bits();
    let hfsq: f64;
    let f: f64;
    let s: f64;
    let z: f64;
    let r: f64;
    let mut w: f64;
    let t1: f64;
    let t2: f64;
    let y: f64;
    let mut hi: f64;
    let lo: f64;
    let mut val_hi: f64;
    let mut val_lo: f64;
    let mut hx: u32;
    let mut k: i32;

    hx = (ui >> 32) as u32;
    k = 0;
    if hx < 0x00100000 || (hx >> 31) > 0 {
        if ui << 1 == 0 {
            return -1. / (x * x); /* log(+-0)=-inf */
        }
        if (hx >> 31) > 0 {
            return (x - x) / 0.0; /* log(-#) = NaN */
        }
        /* subnormal number, scale x up */
        k -= 54;
        x *= x1p54;
        ui = x.to_bits();
        hx = (ui >> 32) as u32;
    } else if hx >= 0x7ff00000 {
        return x;
    } else if hx == 0x3ff00000 && ui << 32 == 0 {
        return 0.;
    }

    /* reduce x into [sqrt(2)/2, sqrt(2)] */
    hx += 0x3ff00000 - 0x3fe6a09e;
    k += (hx >> 20) as i32 - 0x3ff;
    hx = (hx & 0x000fffff) + 0x3fe6a09e;
    ui = ((hx as u64) << 32) | (ui & 0xffffffff);
    x = f64::from_bits(ui);

    f = x - 1.0;
    hfsq = 0.5 * f * f;
    s = f / (2.0 + f);
    z = s * s;
    w = z * z;
    t1 = w * (LG2 + w * (LG4 + w * LG6));
    t2 = z * (LG1 + w * (LG3 + w * (LG5 + w * LG7)));
    r = t2 + t1;

    /* hi+lo = f - hfsq + s*(hfsq+R) ~ log(1+f) */
    hi = f - hfsq;
    ui = hi.to_bits();
    ui &= (-1i64 as u64) << 32;
    hi = f64::from_bits(ui);
    lo = f - hi - hfsq + s * (hfsq + r);

    val_hi = hi * IVLN2HI;
    val_lo = (lo + hi) * IVLN2LO + lo * IVLN2HI;

    /* spadd(val_hi, val_lo, y), except for not using double_t: */
    y = k.into();
    w = y + val_hi;
    val_lo += (y - w) + val_hi;
    val_hi = w;

    val_lo + val_hi
}

/* origin: FreeBSD /usr/src/lib/msun/src/e_pow.c */
/*
 * ====================================================
 * Copyright (C) 2004 by Sun Microsystems, Inc. All rights reserved.
 *
 * Permission to use, copy, modify, and distribute this
 * software is freely granted, provided that this notice
 * is preserved.
 * ====================================================
 */

// pow(x,y) return x**y
//
//                    n
// Method:  Let x =  2   * (1+f)
//      1. Compute and return log2(x) in two pieces:
//              log2(x) = w1 + w2,
//         where w1 has 53-24 = 29 bit trailing zeros.
//      2. Perform y*log2(x) = n+y' by simulating multi-precision
//         arithmetic, where |y'|<=0.5.
//      3. Return x**y = 2**n*exp(y'*log2)
//
// Special cases:
//      1.  (anything) ** 0  is 1
//      2.  1 ** (anything)  is 1
//      3.  (anything except 1) ** NAN is NAN
//      4.  NAN ** (anything except 0) is NAN
//      5.  +-(|x| > 1) **  +INF is +INF
//      6.  +-(|x| > 1) **  -INF is +0
//      7.  +-(|x| < 1) **  +INF is +0
//      8.  +-(|x| < 1) **  -INF is +INF
//      9.  -1          ** +-INF is 1
//      10. +0 ** (+anything except 0, NAN)               is +0
//      11. -0 ** (+anything except 0, NAN, odd integer)  is +0
//      12. +0 ** (-anything except 0, NAN)               is +INF, raise divbyzero
//      13. -0 ** (-anything except 0, NAN, odd integer)  is +INF, raise divbyzero
//      14. -0 ** (+odd integer) is -0
//      15. -0 ** (-odd integer) is -INF, raise divbyzero
//      16. +INF ** (+anything except 0,NAN) is +INF
//      17. +INF ** (-anything except 0,NAN) is +0
//      18. -INF ** (+odd integer) is -INF
//      19. -INF ** (anything) = -0 ** (-anything), (anything except odd integer)
//      20. (anything) ** 1 is (anything)
//      21. (anything) ** -1 is 1/(anything)
//      22. (-anything) ** (integer) is (-1)**(integer)*(+anything**integer)
//      23. (-anything except 0 and inf) ** (non-integer) is NAN
//
// Accuracy:
//      pow(x,y) returns x**y nearly rounded. In particular
//                      pow(integer,integer)
//      always returns the correct integer provided it is
//      representable.
//
// Constants :
// The hexadecimal values are the intended ones for the following
// constants. The decimal values may be used, provided that the
// compiler will convert from decimal to binary accurately enough
// to produce the hexadecimal values shown.
//
/// Returns `x` to the power of `y` (f64).
pub fn pow(x: f64, y: f64) -> f64 {
    const BP: [f64; 2] = [1.0, 1.5];
    const DP_H: [f64; 2] = [0.0, 5.84962487220764160156e-01]; /* 0x3fe2b803_40000000 */
    const DP_L: [f64; 2] = [0.0, 1.35003920212974897128e-08]; /* 0x3E4CFDEB, 0x43CFD006 */
    const TWO53: f64 = 9007199254740992.0; /* 0x43400000_00000000 */
    const HUGE: f64 = 1.0e300;
    const TINY: f64 = 1.0e-300;

    // poly coefs for (3/2)*(log(x)-2s-2/3*s**3:
    const L1: f64 = 5.99999999999994648725e-01; /* 0x3fe33333_33333303 */
    const L2: f64 = 4.28571428578550184252e-01; /* 0x3fdb6db6_db6fabff */
    const L3: f64 = 3.33333329818377432918e-01; /* 0x3fd55555_518f264d */
    const L4: f64 = 2.72728123808534006489e-01; /* 0x3fd17460_a91d4101 */
    const L5: f64 = 2.30660745775561754067e-01; /* 0x3fcd864a_93c9db65 */
    const L6: f64 = 2.06975017800338417784e-01; /* 0x3fca7e28_4a454eef */
    const P1: f64 = 1.66666666666666019037e-01; /* 0x3fc55555_5555553e */
    const P2: f64 = -2.77777777770155933842e-03; /* 0xbf66c16c_16bebd93 */
    const P3: f64 = 6.61375632143793436117e-05; /* 0x3f11566a_af25de2c */
    const P4: f64 = -1.65339022054652515390e-06; /* 0xbebbbd41_c5d26bf1 */
    const P5: f64 = 4.13813679705723846039e-08; /* 0x3e663769_72bea4d0 */
    const LG2: f64 = 6.93147180559945286227e-01; /* 0x3fe62e42_fefa39ef */
    const LG2_H: f64 = 6.93147182464599609375e-01; /* 0x3fe62e43_00000000 */
    const LG2_L: f64 = -1.90465429995776804525e-09; /* 0xbe205c61_0ca86c39 */
    const OVT: f64 = 8.0085662595372944372e-017; /* -(1024-log2(ovfl+.5ulp)) */
    const CP: f64 = 9.61796693925975554329e-01; /* 0x3feec709_dc3a03fd =2/(3ln2) */
    const CP_H: f64 = 9.61796700954437255859e-01; /* 0x3feec709_e0000000 =(float)cp */
    const CP_L: f64 = -7.02846165095275826516e-09; /* 0xbe3e2fe0_145b01f5 =tail of cp_h*/
    const IVLN2: f64 = 1.44269504088896338700e+00; /* 0x3ff71547_652b82fe =1/ln2 */
    const IVLN2_H: f64 = 1.44269502162933349609e+00; /* 0x3ff71547_60000000 =24b 1/ln2*/
    const IVLN2_L: f64 = 1.92596299112661746887e-08; /* 0x3e54ae0b_f85ddf44 =1/ln2 tail*/

    let t1: f64;
    let t2: f64;

    let (hx, lx): (i32, u32) = ((x.to_bits() >> 32) as i32, x.to_bits() as u32);
    let (hy, ly): (i32, u32) = ((y.to_bits() >> 32) as i32, y.to_bits() as u32);

    let mut ix: i32 = hx & 0x7fffffff_i32;
    let iy: i32 = hy & 0x7fffffff_i32;

    /* x**0 = 1, even if x is NaN */
    if ((iy as u32) | ly) == 0 {
        return 1.0;
    }

    /* 1**y = 1, even if y is NaN */
    if hx == 0x3ff00000 && lx == 0 {
        return 1.0;
    }

    /* NaN if either arg is NaN */
    if ix > 0x7ff00000
        || (ix == 0x7ff00000 && lx != 0)
        || iy > 0x7ff00000
        || (iy == 0x7ff00000 && ly != 0)
    {
        return x + y;
    }

    /* determine if y is an odd int when x < 0
     * yisint = 0       ... y is not an integer
     * yisint = 1       ... y is an odd int
     * yisint = 2       ... y is an even int
     */
    let mut yisint: i32 = 0;
    let mut k: i32;
    let mut j: i32;
    if hx < 0 {
        if iy >= 0x43400000 {
            yisint = 2; /* even integer y */
        } else if iy >= 0x3ff00000 {
            k = (iy >> 20) - 0x3ff; /* exponent */

            if k > 20 {
                j = (ly >> (52 - k)) as i32;

                if (j << (52 - k)) == (ly as i32) {
                    yisint = 2 - (j & 1);
                }
            } else if ly == 0 {
                j = iy >> (20 - k);

                if (j << (20 - k)) == iy {
                    yisint = 2 - (j & 1);
                }
            }
        }
    }

    if ly == 0 {
        /* special value of y */
        if iy == 0x7ff00000 {
            /* y is +-inf */

            return if ((ix - 0x3ff00000) | (lx as i32)) == 0 {
                /* (-1)**+-inf is 1 */
                1.0
            } else if ix >= 0x3ff00000 {
                /* (|x|>1)**+-inf = inf,0 */
                if hy >= 0 { y } else { 0.0 }
            } else {
                /* (|x|<1)**+-inf = 0,inf */
                if hy >= 0 { 0.0 } else { -y }
            };
        }

        if iy == 0x3ff00000 {
            /* y is +-1 */
            return if hy >= 0 { x } else { 1.0 / x };
        }

        if hy == 0x40000000 {
            /* y is 2 */
            return x * x;
        }

        if hy == 0x3fe00000 {
            /* y is 0.5 */
            if hx >= 0 {
                /* x >= +0 */
                return f64::sqrt(x);
            }
        }
    }

    let mut ax: f64 = f64::abs(x);
    if lx == 0 {
        /* special value of x */
        if ix == 0x7ff00000 || ix == 0 || ix == 0x3ff00000 {
            /* x is +-0,+-inf,+-1 */
            let mut z: f64 = ax;

            if hy < 0 {
                /* z = (1/|x|) */
                z = 1.0 / z;
            }

            if hx < 0 {
                if ((ix - 0x3ff00000) | yisint) == 0 {
                    z = (z - z) / (z - z); /* (-1)**non-int is NaN */
                } else if yisint == 1 {
                    z = -z; /* (x<0)**odd = -(|x|**odd) */
                }
            }

            return z;
        }
    }

    let mut s: f64 = 1.0; /* sign of result */
    if hx < 0 {
        if yisint == 0 {
            /* (x<0)**(non-int) is NaN */
            return (x - x) / (x - x);
        }

        if yisint == 1 {
            /* (x<0)**(odd int) */
            s = -1.0;
        }
    }

    /* |y| is HUGE */
    if iy > 0x41e00000 {
        /* if |y| > 2**31 */
        if iy > 0x43f00000 {
            /* if |y| > 2**64, must o/uflow */
            if ix <= 0x3fefffff {
                return if hy < 0 { HUGE * HUGE } else { TINY * TINY };
            }

            if ix >= 0x3ff00000 {
                return if hy > 0 { HUGE * HUGE } else { TINY * TINY };
            }
        }

        /* over/underflow if x is not close to one */
        if ix < 0x3fefffff {
            return if hy < 0 {
                s * HUGE * HUGE
            } else {
                s * TINY * TINY
            };
        }
        if ix > 0x3ff00000 {
            return if hy > 0 {
                s * HUGE * HUGE
            } else {
                s * TINY * TINY
            };
        }

        /* now |1-x| is TINY <= 2**-20, suffice to compute
        log(x) by x-x^2/2+x^3/3-x^4/4 */
        let t: f64 = ax - 1.0; /* t has 20 trailing zeros */
        let w: f64 = (t * t) * (0.5 - t * (0.3333333333333333333333 - t * 0.25));
        let u: f64 = IVLN2_H * t; /* ivln2_h has 21 sig. bits */
        let v: f64 = t * IVLN2_L - w * IVLN2;
        t1 = with_set_low_word(u + v, 0);
        t2 = v - (t1 - u);
    } else {
        // double ss,s2,s_h,s_l,t_h,t_l;
        let mut n: i32 = 0;

        if ix < 0x00100000 {
            /* take care subnormal number */
            ax *= TWO53;
            n -= 53;
            ix = get_high_word(ax) as i32;
        }

        n += (ix >> 20) - 0x3ff;
        j = ix & 0x000fffff;

        /* determine interval */
        let k: i32;
        ix = j | 0x3ff00000; /* normalize ix */
        if j <= 0x3988E {
            /* |x|<sqrt(3/2) */
            k = 0;
        } else if j < 0xBB67A {
            /* |x|<sqrt(3)   */
            k = 1;
        } else {
            k = 0;
            n += 1;
            ix -= 0x00100000;
        }
        ax = with_set_high_word(ax, ix as u32);

        /* compute ss = s_h+s_l = (x-1)/(x+1) or (x-1.5)/(x+1.5) */
        let u: f64 = ax - BP[k as usize]; /* bp[0]=1.0, bp[1]=1.5 */
        let v: f64 = 1.0 / (ax + BP[k as usize]);
        let ss: f64 = u * v;
        let s_h = with_set_low_word(ss, 0);

        /* t_h=ax+bp[k] High */
        let t_h: f64 = with_set_high_word(
            0.0,
            ((ix as u32 >> 1) | 0x20000000) + 0x00080000 + ((k as u32) << 18),
        );
        let t_l: f64 = ax - (t_h - BP[k as usize]);
        let s_l: f64 = v * ((u - s_h * t_h) - s_h * t_l);

        /* compute log(ax) */
        let s2: f64 = ss * ss;
        let mut r: f64 = s2 * s2 * (L1 + s2 * (L2 + s2 * (L3 + s2 * (L4 + s2 * (L5 + s2 * L6)))));
        r += s_l * (s_h + ss);
        let s2: f64 = s_h * s_h;
        let t_h: f64 = with_set_low_word(3.0 + s2 + r, 0);
        let t_l: f64 = r - ((t_h - 3.0) - s2);

        /* u+v = ss*(1+...) */
        let u: f64 = s_h * t_h;
        let v: f64 = s_l * t_h + t_l * ss;

        /* 2/(3log2)*(ss+...) */
        let p_h: f64 = with_set_low_word(u + v, 0);
        let p_l = v - (p_h - u);
        let z_h: f64 = CP_H * p_h; /* cp_h+cp_l = 2/(3*log2) */
        let z_l: f64 = CP_L * p_h + p_l * CP + DP_L[k as usize];

        /* log2(ax) = (ss+..)*2/(3*log2) = n + dp_h + z_h + z_l */
        let t: f64 = n as f64;
        t1 = with_set_low_word(((z_h + z_l) + DP_H[k as usize]) + t, 0);
        t2 = z_l - (((t1 - t) - DP_H[k as usize]) - z_h);
    }

    /* split up y into y1+y2 and compute (y1+y2)*(t1+t2) */
    let y1: f64 = with_set_low_word(y, 0);
    let p_l: f64 = (y - y1) * t1 + y * t2;
    let mut p_h: f64 = y1 * t1;
    let z: f64 = p_l + p_h;
    let mut j: i32 = (z.to_bits() >> 32) as i32;
    let i: i32 = z.to_bits() as i32;
    // let (j, i): (i32, i32) = ((z.to_bits() >> 32) as i32, z.to_bits() as i32);

    if j >= 0x40900000 {
        /* z >= 1024 */
        if (j - 0x40900000) | i != 0 {
            /* if z > 1024 */
            return s * HUGE * HUGE; /* overflow */
        }

        if p_l + OVT > z - p_h {
            return s * HUGE * HUGE; /* overflow */
        }
    } else if (j & 0x7fffffff) >= 0x4090cc00 {
        /* z <= -1075 */
        // FIXME: instead of abs(j) use unsigned j

        if (((j as u32) - 0xc090cc00) | (i as u32)) != 0 {
            /* z < -1075 */
            return s * TINY * TINY; /* underflow */
        }

        if p_l <= z - p_h {
            return s * TINY * TINY; /* underflow */
        }
    }

    /* compute 2**(p_h+p_l) */
    let i: i32 = j & 0x7fffffff_i32;
    k = (i >> 20) - 0x3ff;
    let mut n: i32 = 0;

    if i > 0x3fe00000 {
        /* if |z| > 0.5, set n = [z+0.5] */
        n = j + (0x00100000 >> (k + 1));
        k = ((n & 0x7fffffff) >> 20) - 0x3ff; /* new k for n */
        let t: f64 = with_set_high_word(0.0, (n & !(0x000fffff >> k)) as u32);
        n = ((n & 0x000fffff) | 0x00100000) >> (20 - k);
        if j < 0 {
            n = -n;
        }
        p_h -= t;
    }

    let t: f64 = with_set_low_word(p_l + p_h, 0);
    let u: f64 = t * LG2_H;
    let v: f64 = (p_l - (t - p_h)) * LG2 + t * LG2_L;
    let mut z: f64 = u + v;
    let w: f64 = v - (z - u);
    let t: f64 = z * z;
    let t1: f64 = z - t * (P1 + t * (P2 + t * (P3 + t * (P4 + t * P5))));
    let r: f64 = (z * t1) / (t1 - 2.0) - (w + z * w);
    z = 1.0 - (r - z);
    j = get_high_word(z) as i32;
    j += n << 20;

    if (j >> 20) <= 0 {
        /* subnormal output */
        z = scalbn(z, n);
    } else {
        z = with_set_high_word(z, j as u32);
    }

    s * z
}

/* k is such that k*ln2 has minimal relative error and x - kln2 > log(FLT_MIN) */
const K: i32 = 2043;

/* expf(x)/2 for x >= log(FLT_MAX), slightly better than 0.5f*expf(x/2)*expf(x/2) */
fn k_expo2(x: f64) -> f64 {
    let k_ln2 = f64::from_bits(0x40962066151add8b);
    /* note that k is odd and scale*scale overflows */
    let scale = f64::from_bits(((((0x3ff + K / 2) as u32) << 20) as u64) << 32);
    /* exp(x - k ln2) * 2**(k-1) */
    exp(x - k_ln2) * scale * scale
}

/* exp(x)/2 for x >= log(DBL_MAX), slightly better than 0.5*exp(x/2)*exp(x/2) */
fn expo2(x: f64) -> f64 {
    /* k is such that k*ln2 has minimal relative error and x - kln2 > log(DBL_MIN) */
    const K: i32 = 2043;
    let kln2 = f64::from_bits(0x40962066151add8b);

    /* note that k is odd and scale*scale overflows */
    let scale = combine_words(((0x3ff + K / 2) as u32) << 20, 0);
    /* exp(x - k ln2) * 2**(k-1) */
    exp(x - kln2) * scale * scale
}

// sinh(x) = (exp(x) - 1/exp(x))/2
//         = (exp(x)-1 + (exp(x)-1)/exp(x))/2
//         = x + x^3/6 + o(x^5)
//

/// The hyperbolic sine of `x` (f64).
pub fn sinh(x: f64) -> f64 {
    // union {double f; uint64_t i;} u = {.f = x};
    // uint32_t w;
    // double t, h, absx;

    let mut uf: f64 = x;
    let mut ui: u64 = f64::to_bits(uf);
    let w: u32;
    let t: f64;
    let mut h: f64;
    let absx: f64;

    h = 0.5;
    if ui >> 63 != 0 {
        h = -h;
    }
    /* |x| */
    ui &= !1 / 2;
    uf = f64::from_bits(ui);
    absx = uf;
    w = (ui >> 32) as u32;

    /* |x| < log(DBL_MAX) */
    if w < 0x40862e42 {
        t = exp_m1(absx);
        if w < 0x3ff00000 {
            if w < 0x3ff00000 - (26 << 20) {
                /* note: inexact and underflow are raised by expm1 */
                /* note: this branch avoids spurious underflow */
                return x;
            }
            return h * (2.0 * t - t * t / (t + 1.0));
        }
        /* note: |x|>log(0x1p26)+eps could be just h*exp(x) */
        return h * (t + t / (t + 1.0));
    }

    /* |x| > log(DBL_MAX) or nan */
    /* note: the result is stored to handle overflow */
    t = 2.0 * h * expo2(absx);
    t
}

/// Hyperbolic cosine (f64)
///
/// Computes the hyperbolic cosine of the argument x.
/// Is defined as `(exp(x) + exp(-x))/2`
/// Angles are specified in radians.
pub fn cosh(mut x: f64) -> f64 {
    /* |x| */
    let mut ix = x.to_bits();
    ix &= 0x7fffffffffffffff;
    x = f64::from_bits(ix);
    let w = ix >> 32;

    /* |x| < log(2) */
    if w < 0x3fe62e42 {
        if w < 0x3ff00000 - (26 << 20) {
            let _x1p120 = f64::from_bits(0x4770000000000000);
            // Floating-point exception flags are unused.
            return 1.;
        }
        let t = exp_m1(x); // exponential minus 1
        return 1. + t * t / (2. * (1. + t));
    }

    /* |x| < log(DBL_MAX) */
    if w < 0x40862e42 {
        let t = exp(x);
        /* note: if x>log(0x1p26) then the 1/t is not needed */
        return 0.5 * (t + 1. / t);
    }

    /* |x| > log(DBL_MAX) or nan */
    k_expo2(x)
}

/* tanh(x) = (exp(x) - exp(-x))/(exp(x) + exp(-x))
 *         = (exp(2*x) - 1)/(exp(2*x) - 1 + 2)
 *         = (1 - exp(-2*x))/(exp(-2*x) - 1 + 2)
 */

/// The hyperbolic tangent of `x` (f64).
///
/// `x` is specified in radians.
pub fn tanh(mut x: f64) -> f64 {
    let mut uf: f64 = x;
    let mut ui: u64 = f64::to_bits(uf);

    let w: u32;
    let sign: bool;
    let mut t: f64;

    /* x = |x| */
    sign = ui >> 63 != 0;
    ui &= !1 / 2;
    uf = f64::from_bits(ui);
    x = uf;
    w = (ui >> 32) as u32;

    if w > 0x3fe193ea {
        /* |x| > log(3)/2 ~= 0.5493 or nan */
        if w > 0x40340000 {
            /* |x| > 20 or nan */
            /* note: this branch avoids raising overflow */
            t = 1.0 - 0.0 / x;
        } else {
            t = exp_m1(2.0 * x);
            t = 1.0 - 2.0 / (t + 2.0);
        }
    } else if w > 0x3fd058ae {
        /* |x| > log(5/3)/2 ~= 0.2554 */
        t = exp_m1(2.0 * x);
        t = t / (t + 2.0);
    } else if w >= 0x00100000 {
        /* |x| >= 0x1p-1022, up to 2ulp error in [0.1,0.2554] */
        t = exp_m1(-2.0 * x);
        t = -t / (t + 2.0);
    } else {
        /* |x| is subnormal */
        /* note: the branch above would not raise underflow in [0x1p-1023,0x1p-1022) */
        // Floating-point exception flags are unused.
        t = x;
    }

    if sign { -t } else { t }
}

const LN2: f64 = 0.693147180559945309417232121458176568; /* 0x3fe62e42,  0xfefa39ef*/

/* asinh(x) = sign(x)*log(|x|+sqrt(x*x+1)) ~= x - x^3/6 + o(x^5) */
/// Inverse hyperbolic sine (f64)
///
/// Calculates the inverse hyperbolic sine of `x`.
/// Is defined as `sgn(x)*log(|x|+sqrt(x*x+1))`.
pub fn asinh(mut x: f64) -> f64 {
    let mut u = x.to_bits();
    let e = ((u >> 52) as usize) & 0x7ff;
    let sign = (u >> 63) != 0;

    /* |x| */
    u &= (!0) >> 1;
    x = f64::from_bits(u);

    if e >= 0x3ff + 26 {
        /* |x| >= 0x1p26 or inf or nan */
        x = ln(x) + LN2;
    } else if e >= 0x3ff + 1 {
        /* |x| >= 2 */
        x = ln(2.0 * x + 1.0 / (f64::sqrt(x * x + 1.0) + x));
    } else if e >= 0x3ff - 26 {
        /* |x| >= 0x1p-26, up to 1.6ulp error in [0.125,0.5] */
        x = ln_1p(x + x * x / (f64::sqrt(x * x + 1.0) + 1.0));
    } else {
        /* |x| < 0x1p-26, raise inexact if x != 0 */
        let _x1p120 = f64::from_bits(0x4770000000000000);
        // Floating-point exception flags are unused.
    }

    if sign { -x } else { x }
}

/// Inverse hyperbolic cosine (f64)
///
/// Calculates the inverse hyperbolic cosine of `x`.
/// Is defined as `log(x + sqrt(x*x-1))`.
/// `x` must be a number greater than or equal to 1.
pub fn acosh(x: f64) -> f64 {
    const LN2: f64 = 0.693147180559945309417232121458176568; /* 0x3fe62e42,  0xfefa39ef*/

    let u = x.to_bits();
    let e = ((u >> 52) as usize) & 0x7ff;

    /* x < 1 domain error is handled in the called functions */

    if e < 0x3ff + 1 {
        /* |x| < 2, up to 2ulp error in [1,1.125] */
        return ln_1p(x - 1.0 + f64::sqrt((x - 1.0) * (x - 1.0) + 2.0 * (x - 1.0)));
    }
    if e < 0x3ff + 26 {
        /* |x| < 0x1p26 */
        return ln(2.0 * x - 1.0 / (x + f64::sqrt(x * x - 1.0)));
    }
    /* |x| >= 0x1p26 or nan */
    return ln(x) + LN2;
}

/* atanh(x) = log((1+x)/(1-x))/2 = log1p(2x/(1-x))/2 ~= x + x^3/3 + o(x^5) */
/// Inverse hyperbolic tangent (f64)
///
/// Calculates the inverse hyperbolic tangent of `x`.
/// Is defined as `log((1+x)/(1-x))/2 = log1p(2x/(1-x))/2`.
pub fn atanh(x: f64) -> f64 {
    let u = x.to_bits();
    let e = ((u >> 52) as usize) & 0x7ff;
    let sign = (u >> 63) != 0;

    /* |x| */
    let mut y = f64::from_bits(u & 0x7fff_ffff_ffff_ffff);

    if e < 0x3ff - 1 {
        if e < 0x3ff - 32 {
            /* handle underflow */
            if e == 0 {
                // Floating-point exception flags are unused.
            }
        } else {
            /* |x| < 0.5, up to 1.7ulp error */
            y = 0.5 * ln_1p(2.0 * y + 2.0 * y * y / (1.0 - y));
        }
    } else {
        /* avoid overflow */
        y = 0.5 * ln_1p(2.0 * (y / (1.0 - y)));
    }

    if sign { -y } else { y }
}

#[cfg(test)]
mod tests {
    use crate::deterministic_f64::*;

    struct Samples(u64);

    impl Samples {
        fn new() -> Self {
            Self(0x4d59_5df4_d0f3_3173)
        }

        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0
        }

        fn unit(&mut self) -> f64 {
            // The midpoint of a 52-bit bin is exactly representable and excludes both endpoints.
            ((self.next() >> 12) as f64 + 0.5) / (1u64 << 52) as f64
        }

        fn between(&mut self, lo: f64, hi: f64) -> f64 {
            lo + (hi - lo) * self.unit()
        }

        fn magnitude(&mut self, min_exp: i32, lo: f64) -> f64 {
            // Draw the binary exponent and significand independently, including subnormals.
            // 1e300 lies in the bin with exponent 996.
            loop {
                let exponent = min_exp + ((self.next() >> 32) % (997 - min_exp) as u64) as i32;
                let scale = if exponent < -1022 {
                    f64::from_bits(1u64 << (exponent + 1074))
                } else {
                    f64::from_bits(((exponent + 1023) as u64) << 52)
                };
                let x = scale * (1.0 + self.unit());
                if x >= lo && x <= 1e300 {
                    return x;
                }
            }
        }
    }

    #[derive(Clone, Copy)]
    enum Domain {
        Angle,
        Linear(f64, f64),
        PositiveLog,
    }

    use Domain::{Angle, Linear, PositiveLog};

    impl Domain {
        fn huge_angles(self) -> usize {
            if matches!(self, Angle) { 64 } else { 0 }
        }

        fn inputs(self, total: usize) -> impl Iterator<Item = f64> {
            let mut rng = Samples::new();
            (0..total).map(move |i| match self {
                Angle if i >= total - self.huge_angles() => rng.magnitude(49, 1e15),
                Angle => rng.between(-1e4, 1e4),
                Linear(lo, hi) => rng.between(lo, hi),
                PositiveLog => rng.magnitude(-1074, f64::from_bits(1)),
            })
        }
    }

    type Unary = fn(f64) -> f64;
    const UNARY: &[(&str, Unary, Unary, Domain, u64)] = &[
        ("sin", sin, f64::sin, Angle, 1),
        ("cos", cos, f64::cos, Angle, 1),
        // tan: 2 ulps from the M-series Mac's libm on 75 of 4160 inputs (glibc is within 1).
        ("tan", tan, f64::tan, Angle, 2),
        ("asin", asin, f64::asin, Linear(-1.0, 1.0), 1),
        ("acos", acos, f64::acos, Linear(-1.0, 1.0), 1),
        ("atan", atan, f64::atan, Linear(-50.0, 50.0), 1),
        ("exp", exp, f64::exp, Linear(-700.0, 700.0), 1),
        ("exp_m1", exp_m1, f64::exp_m1, Linear(-700.0, 700.0), 1),
        ("exp2", exp2, f64::exp2, Linear(-1000.0, 1000.0), 1),
        ("ln", ln, f64::ln, PositiveLog, 1),
        ("ln_1p", ln_1p, f64::ln_1p, Linear(-1.0, 1e6), 1),
        ("log2", log2, f64::log2, PositiveLog, 1),
        ("sinh", sinh, f64::sinh, Linear(-50.0, 50.0), 2),
        ("cosh", cosh, f64::cosh, Linear(-700.0, 700.0), 2),
        ("tanh", tanh, f64::tanh, Linear(-50.0, 50.0), 2),
        ("asinh", asinh, f64::asinh, Linear(-50.0, 50.0), 2),
        ("acosh", acosh, f64::acosh, Linear(1.0, 1e6), 2),
        ("atanh", atanh, atanh_reference, Linear(-1.0, 1.0), 2),
    ];

    /// std's `f64::atanh` is `0.5 * ln_1p(2x / (1 - x))`, which loses up to about 120 ulps near ±1.
    /// There, `(1 + x) / (1 - x)` has one exact side (Sterbenz) and the ln of it is accurate.
    fn atanh_reference(x: f64) -> f64 {
        if x.abs() >= 0.5 { 0.5 * ((1.0 + x) / (1.0 - x)).ln() } else { x.atanh() }
    }

    type Binary = fn(f64, f64) -> f64;
    type Range = (f64, f64);
    const BINARY: &[(&str, Binary, Binary, Range, Range)] = &[
        ("atan2", atan2, f64::atan2, (-1e3, 1e3), (-1e3, 1e3)),
        ("pow", pow, f64::powf, (0.0, 1e3), (-50.0, 50.0)),
    ];

    fn pairs(a: Range, b: Range, total: usize) -> impl Iterator<Item = (f64, f64)> {
        let mut rng = Samples::new();
        (0..total).map(move |_| (rng.between(a.0, a.1), rng.between(b.0, b.1)))
    }

    /// Distance in ulps: 0 for equal values (NaN against NaN included), u64::MAX when only one
    /// side is finite or only one is NaN.
    fn ulps(got: f64, want: f64) -> u64 {
        if got == want || (got.is_nan() && want.is_nan()) {
            return 0;
        }
        if !(got.is_finite() && want.is_finite()) {
            return u64::MAX;
        }
        fn ordered(value: f64) -> u64 {
            let bits = value.to_bits();
            if value.is_sign_negative() { bits.wrapping_neg() } else { bits | (1 << 63) }
        }
        ordered(got).abs_diff(ordered(want))
    }

    #[test]
    fn matches_std_within_tolerance() {
        // Every result beyond its function's tolerance, so one run shows each function's misses.
        let mut misses = Vec::new();
        let mut check = |name: &str, inputs: &[f64], got: f64, want: f64, tolerance: u64| {
            let distance = ulps(got, want);
            if distance > tolerance {
                misses.push(format!("{name}{inputs:?} got {got:?}, expected {want:?}, {distance} ulps (limit {tolerance})"));
            }
        };
        for &(name, kernel, reference, domain, tolerance) in UNARY {
            for x in domain.inputs(4096 + domain.huge_angles()) {
                check(name, &[x], kernel(x), reference(x), tolerance);
            }
        }
        for x in Angle.inputs(4096 + Angle.huge_angles()) {
            let (s, c) = sincos(x);
            let (want_s, want_c) = x.sin_cos();
            check("sincos.sin", &[x], s, want_s, 1);
            check("sincos.cos", &[x], c, want_c, 1);
        }
        for &(name, kernel, reference, a, b) in BINARY {
            for (x, y) in pairs(a, b, 4096) {
                check(name, &[x, y], kernel(x, y), reference(x, y), 1);
            }
        }
        assert!(misses.is_empty(), "{} results beyond tolerance against std:\n{}", misses.len(), misses.join("\n"));
    }

    fn exact(name: &str, inputs: &[f64], got: f64, want: f64) {
        assert_eq!(got.to_bits(), want.to_bits(), "{name}{inputs:?} got {got:?}, expected {want:?}");
    }

    #[test]
    fn special_values() {
        let nan = f64::NAN;
        let inf = f64::INFINITY;
        for &(name, kernel, _, _, _) in UNARY {
            assert!(kernel(nan).is_nan(), "{name}(NaN)");
        }
        let (s, c) = sincos(nan);
        assert!(s.is_nan() && c.is_nan(), "sincos(NaN) got ({s}, {c})");
        for &(name, kernel, _, _, _) in BINARY {
            assert!(kernel(nan, 2.0).is_nan(), "{name}(NaN, 2)");
            assert!(kernel(2.0, nan).is_nan(), "{name}(2, NaN)");
        }

        let signed_zero: &[(&str, Unary)] = &[
            ("sin", sin), ("tan", tan), ("asin", asin), ("atan", atan),
            ("sinh", sinh), ("tanh", tanh), ("asinh", asinh), ("atanh", atanh),
            ("exp_m1", exp_m1), ("ln_1p", ln_1p),
        ];
        for z in [0.0_f64, -0.0] {
            for &(name, kernel) in signed_zero {
                exact(name, &[z], kernel(z), z);
            }
            exact("cos", &[z], cos(z), 1.0);
            exact("exp", &[z], exp(z), 1.0);
            exact("cosh", &[z], cosh(z), 1.0);
            let (s, c) = sincos(z);
            exact("sincos.sin", &[z], s, z);
            exact("sincos.cos", &[z], c, 1.0);
        }

        exact("exp", &[-inf], exp(-inf), 0.0);
        exact("exp", &[inf], exp(inf), inf);
        exact("exp_m1", &[-inf], exp_m1(-inf), -1.0);
        exact("ln", &[0.0], ln(0.0), -inf);
        assert!(ln(-1.0).is_nan(), "ln(-1)");
        exact("ln", &[inf], ln(inf), inf);
        assert!(sin(inf).is_nan(), "sin(inf)");
        for x in [inf, -inf] {
            exact("atan", &[x], atan(x), core::f64::consts::FRAC_PI_2.copysign(x));
            exact("tanh", &[x], tanh(x), 1.0_f64.copysign(x));
        }

        for x in [f64::from_bits(1), 1e-310] {
            for (name, got) in [
                ("sin", sin(x)), ("tan", tan(x)), ("atan", atan(x)), ("exp_m1", exp_m1(x)),
                ("ln_1p", ln_1p(x)), ("sinh", sinh(x)), ("tanh", tanh(x)),
            ] {
                exact(name, &[x], got, x);
            }
            exact("ln", &[x], ln(x), x.ln());
            exact("log2", &[x], log2(x), x.log2());
        }

        let quadrants = [0.0, -0.0, 1.0, -1.0, inf, -inf];
        for y in quadrants {
            for x in quadrants {
                exact("atan2", &[y, x], atan2(y, x), y.atan2(x));
            }
        }
        for v in [nan, inf, -inf, 0.0, -0.0, 1.0, -1.0, 3.5, -3.5, f64::from_bits(1), f64::MAX] {
            for z in [0.0, -0.0] {
                exact("pow", &[v, z], pow(v, z), 1.0);
            }
            exact("pow", &[1.0, v], pow(1.0, v), 1.0);
        }
        for y in [inf, -inf] {
            exact("pow", &[-1.0, y], pow(-1.0, y), 1.0);
        }
        for z in [0.0, -0.0] {
            exact("pow", &[z, -3.0], pow(z, -3.0), inf.copysign(z));
            exact("pow", &[z, 3.0], pow(z, 3.0), z);
        }
        assert!(pow(-8.0, 1.0 / 3.0).is_nan(), "pow(-8, 1/3)");
        exact("pow", &[-2.0, 3.0], pow(-2.0, 3.0), -8.0);
        exact("pow", &[2.0, -1074.0], pow(2.0, -1074.0), f64::from_bits(1));
    }

    #[test]
    fn parity_hashes_match_golden() {
        // The same on a Linux box (x86-64) and an M-series Mac (aarch64). A change means the port
        // was edited or a platform computes different bits.
        const GOLDEN: &[(&str, u64)] = &[
            ("sin", 0x346980c6c2e28224), ("cos", 0x530bcc90ae8e9041), ("tan", 0x19fa748f4f6b8eb6),
            ("asin", 0xd222ef920dbe8343), ("acos", 0xd959e51d5d39c393), ("atan", 0xcbb757166413f00b),
            ("exp", 0x40c1021106f8cde0), ("exp_m1", 0xe46cfc3b7c47c2e3), ("exp2", 0xd8bbfb1ff657884d),
            ("ln", 0xe3b7a50259a11186), ("ln_1p", 0x5a5012468ca7a2c2), ("log2", 0x4a2635a2acd66534),
            ("sinh", 0x28e7f15db492737d), ("cosh", 0x813fdd1f0a83aae5), ("tanh", 0x4238cf26f1690f7d),
            ("asinh", 0xa6cc1348f53d150c), ("acosh", 0xf38e61847b6d3732), ("atanh", 0x9cd836fb8b4cfc85),
            ("sincos", 0x2c4f5d4858973526), ("atan2", 0x3f6c08e883259a43), ("pow", 0x11b4bd35e50ea8de),
        ];
        const OFFSET: u64 = 0xcbf29ce484222325;
        fn hash(h: u64, x: f64) -> u64 {
            (h ^ x.to_bits()).wrapping_mul(0x100000001b3)
        }

        let mut hashes = Vec::with_capacity(GOLDEN.len());
        for &(name, kernel, _, domain, _) in UNARY {
            let h = domain.inputs(2048).fold(OFFSET, |h, x| hash(h, kernel(x)));
            hashes.push((name, h));
        }
        let h = Angle.inputs(2048).fold(OFFSET, |h, x| {
            let (s, c) = sincos(x);
            hash(hash(h, s), c)
        });
        hashes.push(("sincos", h));
        for &(name, kernel, _, a, b) in BINARY {
            let h = pairs(a, b, 2048).fold(OFFSET, |h, (x, y)| hash(h, kernel(x, y)));
            hashes.push((name, h));
        }
        assert_eq!(hashes.as_slice(), GOLDEN, "parity hashes over 2048 inputs: {hashes:#018x?}");
    }
}

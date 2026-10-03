//! Exact float -> decimal digit generation (Steele & White / Dragon4 as in real core's
//! flt2dec::strategy::dragon; core's Grisu fast path produces identical digits).

use super::bignum::Big;
use super::Decoded;
use crate::cmp::Ordering;

/// Upper bound of significant digits shortest mode produces.
pub(crate) const MAX_SIG_DIGITS: usize = 17;

/// `k` with 10^(k-1) <= v < 10^(k+1) (underestimates by at most one).
fn estimate_scaling_factor(mant: u64, exp: i16) -> i16 {
    let nbits = 64 - (mant - 1).leading_zeros() as i64;
    // 1292913986 = floor(2^32 * log10(2))
    (((nbits + exp as i64) * 1292913986) >> 32) as i16
}

/// Upper bound of the significant digits of the exact decimal expansion.
pub(crate) fn estimate_max_buf_len(exp: i16) -> usize {
    21 + ((if exp < 0 { -12 } else { 5 } * exp as i32) as usize >> 4)
}

/// Increments the decimal digits in `d`; returns the extra digit when all were '9'.
pub(crate) fn round_up(d: &mut [u8]) -> Option<u8> {
    let mut i = d.len();
    while i > 0 {
        i -= 1;
        if d[i] != b'9' {
            d[i] += 1;
            let mut j = i + 1;
            while j < d.len() {
                d[j] = b'0';
                j += 1;
            }
            return None;
        }
    }
    if !d.is_empty() {
        d[0] = b'1';
        let mut j = 1;
        while j < d.len() {
            d[j] = b'0';
            j += 1;
        }
        Some(b'0')
    } else {
        Some(b'1')
    }
}

/// `x < y` when not inclusive, `x <= y` when inclusive.
fn below(o: Ordering, inclusive: bool) -> bool {
    if inclusive {
        o != Ordering::Greater
    } else {
        o == Ordering::Less
    }
}

fn ge(a: &Big, b: &Big) -> bool {
    a.cmp(b) != Ordering::Less
}

/// Shortest digits that round-trip: returns (number of digits in buf, k) with
/// v = 0.d1d2..dn * 10^k.
pub(crate) fn format_shortest(d: &Decoded, buf: &mut [u8]) -> (usize, i16) {
    let inclusive = d.inclusive;
    let mut k = estimate_scaling_factor(d.mant + d.plus, d.exp);

    let mut mant = Big::from_u64(d.mant);
    let mut minus = Big::from_u64(d.minus);
    let mut plus = Big::from_u64(d.plus);
    let mut scale = Big::from_small(1);
    if d.exp < 0 {
        scale.mul_pow2((-d.exp) as usize);
    } else {
        mant.mul_pow2(d.exp as usize);
        minus.mul_pow2(d.exp as usize);
        plus.mul_pow2(d.exp as usize);
    }
    if k >= 0 {
        scale.mul_pow10(k as usize);
    } else {
        mant.mul_pow10((-k) as usize);
        minus.mul_pow10((-k) as usize);
        plus.mul_pow10((-k) as usize);
    }

    // fixup when we underestimated k
    let mut mp = mant;
    mp.add(&plus);
    if below(scale.cmp(&mp), inclusive) {
        k += 1;
    } else {
        mant.mul_small(10);
        minus.mul_small(10);
        plus.mul_small(10);
    }

    let mut scale2 = scale;
    scale2.mul_pow2(1);
    let mut scale4 = scale;
    scale4.mul_pow2(2);
    let mut scale8 = scale;
    scale8.mul_pow2(3);

    let mut down;
    let mut up;
    let mut i = 0;
    loop {
        let mut digit = 0u8;
        if ge(&mant, &scale8) {
            mant.sub(&scale8);
            digit += 8;
        }
        if ge(&mant, &scale4) {
            mant.sub(&scale4);
            digit += 4;
        }
        if ge(&mant, &scale2) {
            mant.sub(&scale2);
            digit += 2;
        }
        if ge(&mant, &scale) {
            mant.sub(&scale);
            digit += 1;
        }
        buf[i] = b'0' + digit;
        i += 1;

        down = below(mant.cmp(&minus), inclusive);
        let mut mp = mant;
        mp.add(&plus);
        up = below(scale.cmp(&mp), inclusive);
        if down || up {
            break;
        }
        mant.mul_small(10);
        minus.mul_small(10);
        plus.mul_small(10);
    }

    // round up if we stopped in the middle of the digits
    if up {
        let round = if !down {
            true
        } else {
            let mut m2 = mant;
            m2.mul_pow2(1);
            ge(&m2, &scale)
        };
        if round {
            if let Some(c) = round_up(&mut buf[..i]) {
                buf[i] = c;
                i += 1;
                k += 1;
            }
        }
    }
    (i, k)
}

/// `x / (2 * 10^n)`.
fn div_2pow10(x: &mut Big, n: usize) {
    let mut n = n;
    while n > 9 {
        x.div_rem_small(1_000_000_000);
        n -= 9;
    }
    let mut p = 1u32;
    while n > 0 {
        p *= 10;
        n -= 1;
    }
    x.div_rem_small(p << 1);
}

/// Exactly rounded digits: at most `buf.len()` digits and none at or below 10^limit.
/// Returns (digits in buf, k) with v ~ 0.d1d2..dn * 10^k.
pub(crate) fn format_exact(d: &Decoded, buf: &mut [u8], limit: i16) -> (usize, i16) {
    let mut k = estimate_scaling_factor(d.mant, d.exp);

    let mut mant = Big::from_u64(d.mant);
    let mut scale = Big::from_small(1);
    if d.exp < 0 {
        scale.mul_pow2((-d.exp) as usize);
    } else {
        mant.mul_pow2(d.exp as usize);
    }
    if k >= 0 {
        scale.mul_pow10(k as usize);
    } else {
        mant.mul_pow10((-k) as usize);
    }

    // fixup when `mant + scale / (2 * 10^buf.len()) >= scale`
    let mut half = scale;
    div_2pow10(&mut half, buf.len());
    half.add(&mant);
    if ge(&half, &scale) {
        k += 1;
    } else {
        mant.mul_small(10);
    }

    let mut len = if k < limit {
        0
    } else if ((k as i32 - limit as i32) as usize) < buf.len() {
        (k as i32 - limit as i32) as usize
    } else {
        buf.len()
    };

    if len > 0 {
        let mut scale2 = scale;
        scale2.mul_pow2(1);
        let mut scale4 = scale;
        scale4.mul_pow2(2);
        let mut scale8 = scale;
        scale8.mul_pow2(3);
        let mut i = 0;
        while i < len {
            if mant.is_zero() {
                // the rest are zeros: fill, no rounding
                while i < len {
                    buf[i] = b'0';
                    i += 1;
                }
                return (len, k);
            }
            let mut digit = 0u8;
            if ge(&mant, &scale8) {
                mant.sub(&scale8);
                digit += 8;
            }
            if ge(&mant, &scale4) {
                mant.sub(&scale4);
                digit += 4;
            }
            if ge(&mant, &scale2) {
                mant.sub(&scale2);
                digit += 2;
            }
            if ge(&mant, &scale) {
                mant.sub(&scale);
                digit += 1;
            }
            buf[i] = b'0' + digit;
            mant.mul_small(10);
            i += 1;
        }
    }

    // round half to even on the remainder
    scale.mul_small(5);
    let order = mant.cmp(&scale);
    if order == Ordering::Greater || (order == Ordering::Equal && len > 0 && (buf[len - 1] & 1) == 1) {
        if let Some(c) = round_up(&mut buf[..len]) {
            k += 1;
            if k > limit && len < buf.len() {
                buf[len] = c;
                len += 1;
            }
        }
    }
    (len, k)
}

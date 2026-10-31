//! Decimal string -> f64/f32: port of real core's dec2flt pipeline (MIT/Apache-2.0).
//! Eight-digit parsing, Clinger's exact/disguised fast path, Eisel-Lemire scaling,
//! and Nigel Tao's simple decimal conversion for ambiguous rounding cases.
//! Accepts real core's syntax and preserves signed zero and special values.

use crate::fmt;
use super::dec2flt_table::{POWER_OF_FIVE_128, SMALLEST_POWER_OF_FIVE, LARGEST_POWER_OF_FIVE};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseFloatError {
    kind: FloatErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FloatErrorKind {
    Empty,
    Invalid,
}

impl ParseFloatError {
    fn description_str(&self) -> &'static str {
        match self.kind {
            FloatErrorKind::Empty => "cannot parse float from empty string",
            FloatErrorKind::Invalid => "invalid float literal",
        }
    }
}

impl fmt::Display for ParseFloatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.description_str(), f)
    }
}

#[cfg(not(relax_check))]
impl crate::error::Error for ParseFloatError {
    fn description(&self) -> &str {
        self.description_str()
    }
}

fn pfe_empty() -> ParseFloatError {
    ParseFloatError { kind: FloatErrorKind::Empty }
}

fn pfe_invalid() -> ParseFloatError {
    ParseFloatError { kind: FloatErrorKind::Invalid }
}

/// Binary float format parameters.
struct Format {
    mantissa_bits: u32,
    minimum_exponent: i32,
    infinite_power: i32,
    /// fast path: exponents `-fast_exp..=fast_exp` exact, mantissa <= 2 << mantissa_bits
    fast_exp: i64,
    /// fast path through an integer multiply up to this exponent
    disguised_exp: i64,
    smallest_power: i64,
    largest_power: i64,
    min_even: i64,
    max_even: i64,
}

const F64: Format = Format { mantissa_bits: 52, minimum_exponent: -1023, infinite_power: 0x7ff, fast_exp: 22, disguised_exp: 37, smallest_power: -342, largest_power: 308, min_even: -4, max_even: 23 };
const F32: Format = Format { mantissa_bits: 23, minimum_exponent: -127, infinite_power: 0xff, fast_exp: 10, disguised_exp: 17, smallest_power: -65, largest_power: 38, min_even: -17, max_even: 10 };

pub fn parse_f64(src: &str) -> Result<f64, ParseFloatError> {
    match parse(src, &F64) {
        Ok(Parsed::Bits(neg, bits)) => {
            let v = f64::from_bits(bits);
            Ok(if neg { -v } else { v })
        }
        Ok(Parsed::Nan(neg)) => Ok(if neg { -f64::NAN } else { f64::NAN }),
        Ok(Parsed::Inf(neg)) => Ok(if neg { f64::NEG_INFINITY } else { f64::INFINITY }),
        Ok(Parsed::Fast(neg, m, e)) => {
            let v = fast_f64(m, e);
            Ok(if neg { -v } else { v })
        }
        Err(e) => Err(e),
    }
}

pub fn parse_f32(src: &str) -> Result<f32, ParseFloatError> {
    match parse(src, &F32) {
        Ok(Parsed::Bits(neg, bits)) => {
            let v = f32::from_bits(bits as u32);
            Ok(if neg { -v } else { v })
        }
        Ok(Parsed::Nan(neg)) => Ok(if neg { -f32::NAN } else { f32::NAN }),
        Ok(Parsed::Inf(neg)) => Ok(if neg { f32::NEG_INFINITY } else { f32::INFINITY }),
        Ok(Parsed::Fast(neg, m, e)) => {
            let v = fast_f32(m, e);
            Ok(if neg { -v } else { v })
        }
        Err(e) => Err(e),
    }
}

enum Parsed {
    /// sign, IEEE bits of the magnitude
    Bits(bool, u64),
    Nan(bool),
    Inf(bool),
    /// sign, mantissa, decimal exponent: exact through the fast path
    Fast(bool, u64, i64),
}

fn eq_ignore_case(s: &[u8], word: &[u8]) -> bool {
    if s.len() != word.len() {
        return false;
    }
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        let lc = if c >= b'A' && c <= b'Z' { c + 32 } else { c };
        if lc != word[i] {
            return false;
        }
        i += 1;
    }
    true
}

fn parse(src: &str, fmt: &Format) -> Result<Parsed, ParseFloatError> {
    let s = src.as_bytes();
    if s.is_empty() {
        return Err(pfe_empty());
    }
    let negative = s[0] == b'-';
    let s = if s[0] == b'-' || s[0] == b'+' { &s[1..] } else { s };
    if s.is_empty() {
        return Err(pfe_invalid());
    }

    // syntax: digits [. digits] [e [+-] digits]
    let mut i = 0;
    let int_start = i;
    let mut mantissa = 0;
    parse_digits(s, &mut i, &mut mantissa);
    let int_end = i;
    let mut frac_start = i;
    let mut frac_end = i;
    if i < s.len() && s[i] == b'.' {
        i += 1;
        frac_start = i;
        parse_digits(s, &mut i, &mut mantissa);
        frac_end = i;
    }
    let n_digits = (int_end - int_start) + (frac_end - frac_start);
    if n_digits == 0 {
        return special(s, negative);
    }
    let mut exp_number: i64 = 0;
    if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
        i += 1;
        let mut exp_neg = false;
        if i < s.len() && (s[i] == b'-' || s[i] == b'+') {
            exp_neg = s[i] == b'-';
            i += 1;
        }
        let digits_start = i;
        while i < s.len() && s[i].is_ascii_digit() {
            if exp_number < 0x10000 {
                exp_number = 10 * exp_number + (s[i] - b'0') as i64;
            }
            i += 1;
        }
        if i == digits_start {
            return Err(pfe_invalid());
        }
        if exp_neg {
            exp_number = -exp_number;
        }
    }
    if i != s.len() {
        return Err(pfe_invalid());
    }

    let mut exponent = exp_number - (frac_end - frac_start) as i64;
    let mut many_digits = false;
    if n_digits > 19 {
        // Leading zeros do not count towards the 19 significant-digit limit.
        let mut significant = n_digits;
        let mut p = 0;
        while p < frac_end {
            if s[p] == b'0' {
                significant -= 1;
            } else if s[p] != b'.' {
                break;
            }
            p += 1;
        }
        if significant > 19 {
            many_digits = true;
            mantissa = 0;
            p = 0;
            parse_19digits(s, &mut p, &mut mantissa);
            if mantissa >= 1_000_000_000_000_000_000 {
                exponent = (int_end - p) as i64;
            } else {
                p = frac_start;
                parse_19digits(s, &mut p, &mut mantissa);
                exponent = -((p - frac_start) as i64);
            }
            exponent += exp_number;
        }
    }

    let max_mant = 2u64 << fmt.mantissa_bits;
    if !many_digits && mantissa <= max_mant && exponent >= -fmt.fast_exp && exponent <= fmt.disguised_exp {
        if exponent <= fmt.fast_exp {
            return Ok(Parsed::Fast(negative, mantissa, exponent));
        }
        // Disguised Clinger path: move excess powers into the integer first.
        let shift = (exponent - fmt.fast_exp) as usize;
        if let Some(m) = mantissa.checked_mul(INT_POW10[shift]) {
            if m <= max_mant {
                return Ok(Parsed::Fast(negative, m, fmt.fast_exp));
            }
        }
    }

    let fp = compute_float(exponent, mantissa, fmt);
    // Truncation represents an interval [mantissa, mantissa + 1]. Only use
    // its approximation if both endpoints round to the same machine float.
    if fp.1 >= 0 && (!many_digits || fp == compute_float(exponent, mantissa + 1, fmt)) {
        return Ok(Parsed::Bits(negative, fp.0 | ((fp.1 as u64) << fmt.mantissa_bits)));
    }

    let mut d = Decimal::new();
    d.parse(&s[..i], int_start, int_end, frac_start, frac_end, exp_number);
    let (f, e) = parse_long_mantissa(&mut d, fmt);
    Ok(Parsed::Bits(negative, f | ((e as u64) << fmt.mantissa_bits)))
}

fn special(s: &[u8], negative: bool) -> Result<Parsed, ParseFloatError> {
    if eq_ignore_case(s, b"nan") {
        Ok(Parsed::Nan(negative))
    } else if eq_ignore_case(s, b"inf") || eq_ignore_case(s, b"infinity") {
        Ok(Parsed::Inf(negative))
    } else {
        Err(pfe_invalid())
    }
}

// Clinger powers are exactly representable in their corresponding float format.
const INT_POW10: [u64; 16] = [1, 10, 100, 1000, 10000, 100000, 1000000, 10000000,
    100000000, 1000000000, 10000000000, 100000000000, 1000000000000,
    10000000000000, 100000000000000, 1000000000000000];
const F64_POW10: [f64; 23] = [1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8,
    1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16, 1e17, 1e18, 1e19, 1e20, 1e21, 1e22];
const F32_POW10: [f32; 11] = [1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10];

fn fast_f64(m: u64, e: i64) -> f64 {
    if e < 0 { (m as f64) / F64_POW10[(-e) as usize] }
    else { (m as f64) * F64_POW10[e as usize] }
}

fn fast_f32(m: u64, e: i64) -> f32 {
    if e < 0 { (m as f32) / F32_POW10[(-e) as usize] }
    else { (m as f32) * F32_POW10[e as usize] }
}

#[inline]
fn parse_8digits(mut v: u64) -> u64 {
    v -= 0x3030_3030_3030_3030;
    v = v * 10 + (v >> 8);
    let v1 = (v & 0x0000_00ff_0000_00ff).wrapping_mul(0x000f_4240_0000_0064);
    let v2 = ((v >> 16) & 0x0000_00ff_0000_00ff).wrapping_mul(0x0000_2710_0000_0001);
    (v1.wrapping_add(v2) >> 32) as u32 as u64
}

#[inline]
fn parse_digits(s: &[u8], i: &mut usize, mantissa: &mut u64) {
    while s.len() - *i >= 8 {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&s[*i..*i + 8]);
        let v = u64::from_le_bytes(bytes);
        let a = v.wrapping_add(0x4646_4646_4646_4646);
        let b = v.wrapping_sub(0x3030_3030_3030_3030);
        if (a | b) & 0x8080_8080_8080_8080 != 0 { break; }
        *mantissa = mantissa.wrapping_mul(100_000_000).wrapping_add(parse_8digits(v));
        *i += 8;
    }
    while *i < s.len() {
        let digit = s[*i].wrapping_sub(b'0');
        if digit >= 10 { break; }
        *mantissa = mantissa.wrapping_mul(10).wrapping_add(digit as u64);
        *i += 1;
    }
}

fn parse_19digits(s: &[u8], i: &mut usize, mantissa: &mut u64) {
    while *i < s.len() && *mantissa < 1_000_000_000_000_000_000 {
        let digit = s[*i].wrapping_sub(b'0');
        if digit >= 10 { break; }
        *mantissa = *mantissa * 10 + digit as u64;
        *i += 1;
    }
}

/// Compute w * 10^q using an extended-precision float representation.
///
/// Fast conversion of a the significant digits and decimal exponent
/// a float to an extended representation with a binary float. This
/// algorithm will accurately parse the vast majority of cases,
/// and uses a 128-bit representation (with a fallback 192-bit
/// representation).
///
/// This algorithm scales the exponent by the decimal exponent
/// using pre-computed powers-of-5, and calculates if the
/// representation can be unambiguously rounded to the nearest
/// machine float. Near-halfway cases are not handled here,
/// and are represented by a negative, biased binary exponent.
///
/// The algorithm is described in detail in "Daniel Lemire, Number Parsing
/// at a Gigabyte per Second" in section 5, "Fast Algorithm", and
/// section 6, "Exact Numbers And Ties", available online:
/// <https://arxiv.org/abs/2101.11408.pdf>.
fn compute_float(q: i64, mut w: u64, fmt: &Format) -> (u64, i32) {
    let fp_zero = (0, 0);
    let fp_inf = (0, fmt.infinite_power);
    let fp_error = (0, -1);

    // Short-circuit if the value can only be a literal 0 or infinity.
    if w == 0 || q < fmt.smallest_power {
        return fp_zero;
    } else if q > fmt.largest_power {
        return fp_inf;
    }
    // Normalize our significant digits, so the most-significant bit is set.
    let lz = w.leading_zeros();
    w <<= lz;
    let (lo, hi) = compute_product_approx(q, w, fmt.mantissa_bits as usize + 3);
    if lo == 0xFFFF_FFFF_FFFF_FFFF {
        // If we have failed to approximate w x 5^-q with our 128-bit value.
        // Since the addition of 1 could lead to an overflow which could then
        // round up over the half-way point, this can lead to improper rounding
        // of a float.
        //
        // However, this can only occur if q ∈ [-27, 55]. The upper bound of q
        // is 55 because 5^55 < 2^128, however, this can only happen if 5^q > 2^64,
        // since otherwise the product can be represented in 64-bits, producing
        // an exact result. For negative exponents, rounding-to-even can
        // only occur if 5^-q < 2^64.
        //
        // For detailed explanations of rounding for negative exponents, see
        // <https://arxiv.org/pdf/2101.11408.pdf#section.9.1>. For detailed
        // explanations of rounding for positive exponents, see
        // <https://arxiv.org/pdf/2101.11408.pdf#section.8>.
        let inside_safe_exponent = (q >= -27) && (q <= 55);
        if !inside_safe_exponent {
            return fp_error;
        }
    }
    let upperbit = (hi >> 63) as i32;
    let mut mantissa = hi >> (upperbit + 64 - fmt.mantissa_bits as i32 - 3);
    let mut power2 = power(q as i32) + upperbit - lz as i32 - (fmt.minimum_exponent + 1) + 1;
    if power2 <= 0 {
        if -power2 + 1 >= 64 {
            // Have more than 64 bits below the minimum exponent, must be 0.
            return fp_zero;
        }
        // Have a subnormal value.
        mantissa >>= -power2 + 1;
        mantissa += mantissa & 1;
        mantissa >>= 1;
        power2 = (mantissa >= (1_u64 << fmt.mantissa_bits)) as i32;
        return (mantissa, power2);
    }
    // Need to handle rounding ties. Normally, we need to round up,
    // but if we fall right in between and we have an even basis, we
    // need to round down.
    //
    // This will only occur if:
    //  1. The lower 64 bits of the 128-bit representation is 0.
    //      IE, 5^q fits in single 64-bit word.
    //  2. The least-significant bit prior to truncated mantissa is odd.
    //  3. All the bits truncated when shifting to mantissa bits + 1 are 0.
    //
    // Or, we may fall between two floats: we are exactly halfway.
    if lo <= 1
        && q >= fmt.min_even
        && q <= fmt.max_even
        && mantissa & 0b11 == 0b01
        && (mantissa << (upperbit + 64 - fmt.mantissa_bits as i32 - 3)) == hi
    {
        // Zero the lowest bit, so we don't round up.
        mantissa &= !1_u64;
    }
    // Round-to-even, then shift the significant digits into place.
    mantissa += mantissa & 1;
    mantissa >>= 1;
    if mantissa >= (2_u64 << fmt.mantissa_bits) {
        // Rounding up overflowed, so the carry bit is set. Set the
        // mantissa to 1 (only the implicit, hidden bit is set) and
        // increase the exponent.
        mantissa = 1_u64 << fmt.mantissa_bits;
        power2 += 1;
    }
    // Zero out the hidden bit.
    mantissa &= !(1_u64 << fmt.mantissa_bits);
    if power2 >= fmt.infinite_power {
        // Exponent is above largest normal value, must be infinite.
        return fp_inf;
    }
    (mantissa, power2)
}

/// Calculate a base 2 exponent from a decimal exponent.
/// This uses a pre-computed integer approximation for
/// log2(10), where 217706 / 2^16 is accurate for the
/// entire range of non-finite decimal exponents.
#[inline]
fn power(q: i32) -> i32 {
    (q.wrapping_mul(152_170 + 65536) >> 16) + 63
}

#[inline]
fn full_multiplication(a: u64, b: u64) -> (u64, u64) {
    let r = (a as u128) * (b as u128);
    (r as u64, (r >> 64) as u64)
}

// This will compute or rather approximate w * 5**q and return a pair of 64-bit words
// approximating the result, with the "high" part corresponding to the most significant
// bits and the low part corresponding to the least significant bits.
fn compute_product_approx(q: i64, w: u64, precision: usize) -> (u64, u64) {
    debug_assert!(q >= SMALLEST_POWER_OF_FIVE as i64);
    debug_assert!(q <= LARGEST_POWER_OF_FIVE as i64);
    debug_assert!(precision <= 64);

    let mask = if precision < 64 {
        0xFFFF_FFFF_FFFF_FFFF_u64 >> precision
    } else {
        0xFFFF_FFFF_FFFF_FFFF_u64
    };

    // 5^q < 2^64, then the multiplication always provides an exact value.
    // That means whenever we need to round ties to even, we always have
    // an exact value.
    let index = (q - SMALLEST_POWER_OF_FIVE as i64) as usize;
    let (lo5, hi5) = POWER_OF_FIVE_128[index];
    // Only need one multiplication as long as there is 1 zero but
    // in the explicit mantissa bits, +1 for the hidden bit, +1 to
    // determine the rounding direction, +1 for if the computed
    // product has a leading zero.
    let (mut first_lo, mut first_hi) = full_multiplication(w, lo5);
    if first_hi & mask == mask {
        // Need to do a second multiplication to get better precision
        // for the lower product. This will always be exact
        // where q is < 55, since 5^55 < 2^128. If this wraps,
        // then we need to round up the hi product.
        let (_, second_hi) = full_multiplication(w, hi5);
        first_lo = first_lo.wrapping_add(second_hi);
        if second_hi > first_lo {
            first_hi += 1;
        }
    }
    (first_lo, first_hi)
}

// ---------------------------------------------------------------- simple decimal conversion

const MAX_DIGITS: usize = 768;
const MAX_DIGITS_WITHOUT_OVERFLOW: usize = 19;
const DECIMAL_POINT_RANGE: i32 = 2047;

struct Decimal {
    num_digits: usize,
    decimal_point: i32,
    truncated: bool,
    digits: [u8; MAX_DIGITS],
}

impl Decimal {
    fn new() -> Decimal {
        Decimal { num_digits: 0, decimal_point: 0, truncated: false, digits: [0; MAX_DIGITS] }
    }

    fn try_add_digit(&mut self, digit: u8) {
        if self.num_digits < MAX_DIGITS {
            self.digits[self.num_digits] = digit;
        }
        self.num_digits += 1;
    }

    fn trim(&mut self) {
        while self.num_digits != 0 && self.digits[self.num_digits - 1] == 0 {
            self.num_digits -= 1;
        }
    }

    /// Core's parse_decimal over an already validated number.
    fn parse(&mut self, s: &[u8], int_start: usize, int_end: usize, frac_start: usize, frac_end: usize, exp: i64) {
        let mut i = int_start;
        while i < int_end && s[i] == b'0' {
            i += 1;
        }
        while i < int_end {
            self.try_add_digit(s[i] - b'0');
            i += 1;
        }
        let mut last = int_end;
        if frac_start > int_end {
            // there was a dot
            let mut j = frac_start;
            if self.num_digits == 0 {
                while j < frac_end && s[j] == b'0' {
                    j += 1;
                }
            }
            while j < frac_end {
                self.try_add_digit(s[j] - b'0');
                j += 1;
            }
            self.decimal_point = -((frac_end - frac_start) as i32);
            last = frac_end;
        }
        if self.num_digits != 0 {
            // trailing zeros (skipping the dot)
            let mut n_trailing_zeros = 0usize;
            let mut k = last;
            while k > int_start {
                k -= 1;
                let c = s[k];
                if c == b'0' {
                    n_trailing_zeros += 1;
                } else if c != b'.' {
                    break;
                }
            }
            self.decimal_point += n_trailing_zeros as i32;
            self.num_digits -= n_trailing_zeros;
            self.decimal_point += self.num_digits as i32;
            if self.num_digits > MAX_DIGITS {
                self.truncated = true;
                self.num_digits = MAX_DIGITS;
            }
        }
        self.decimal_point += exp as i32;
        let mut z = self.num_digits;
        while z < MAX_DIGITS_WITHOUT_OVERFLOW {
            self.digits[z] = 0;
            z += 1;
        }
    }

    /// The integer part, rounded half to even (the truncated flag counts as above half).
    fn round(&self) -> u64 {
        if self.num_digits == 0 || self.decimal_point < 0 {
            return 0;
        } else if self.decimal_point > 18 {
            return 0xFFFF_FFFF_FFFF_FFFF;
        }
        let dp = self.decimal_point as usize;
        let mut n = 0u64;
        let mut i = 0;
        while i < dp {
            n *= 10;
            if i < self.num_digits {
                n += self.digits[i] as u64;
            }
            i += 1;
        }
        let mut round_up = false;
        if dp < self.num_digits {
            round_up = self.digits[dp] >= 5;
            if self.digits[dp] == 5 && dp + 1 == self.num_digits {
                round_up = self.truncated || ((dp != 0) && (1 & self.digits[dp - 1] != 0));
            }
        }
        if round_up {
            n += 1;
        }
        n
    }

    /// Multiplies by 2^shift (shift <= 60).
    fn left_shift(&mut self, shift: usize) {
        if self.num_digits == 0 {
            return;
        }
        let mut tmp = [0u8; MAX_DIGITS + 32];
        let mut w = tmp.len();
        let mut n: u64 = 0;
        let mut r = self.num_digits;
        while r > 0 {
            r -= 1;
            n += (self.digits[r] as u64) << shift;
            let q = n / 10;
            w -= 1;
            tmp[w] = (n - 10 * q) as u8;
            n = q;
        }
        while n > 0 {
            let q = n / 10;
            w -= 1;
            tmp[w] = (n - 10 * q) as u8;
            n = q;
        }
        let total = tmp.len() - w;
        let num_new_digits = total - self.num_digits;
        let keep = if total < MAX_DIGITS { total } else { MAX_DIGITS };
        let mut i = 0;
        while i < keep {
            self.digits[i] = tmp[w + i];
            i += 1;
        }
        while i < total {
            if tmp[w + i] != 0 {
                self.truncated = true;
            }
            i += 1;
        }
        self.num_digits = keep;
        self.decimal_point += num_new_digits as i32;
        self.trim();
    }

    /// Divides by 2^shift (shift <= 60).
    fn right_shift(&mut self, shift: usize) {
        let mut read_index = 0;
        let mut write_index = 0;
        let mut n = 0u64;
        while (n >> shift) == 0 {
            if read_index < self.num_digits {
                n = (10 * n) + self.digits[read_index] as u64;
                read_index += 1;
            } else if n == 0 {
                return;
            } else {
                while (n >> shift) == 0 {
                    n *= 10;
                    read_index += 1;
                }
                break;
            }
        }
        self.decimal_point -= read_index as i32 - 1;
        if self.decimal_point < -DECIMAL_POINT_RANGE {
            self.num_digits = 0;
            self.decimal_point = 0;
            self.truncated = false;
            return;
        }
        let mask = (1u64 << shift) - 1;
        while read_index < self.num_digits {
            let new_digit = (n >> shift) as u8;
            n = (10 * (n & mask)) + self.digits[read_index] as u64;
            read_index += 1;
            self.digits[write_index] = new_digit;
            write_index += 1;
        }
        while n > 0 {
            let new_digit = (n >> shift) as u8;
            n = 10 * (n & mask);
            if write_index < MAX_DIGITS {
                self.digits[write_index] = new_digit;
                write_index += 1;
            } else if new_digit > 0 {
                self.truncated = true;
            }
        }
        self.num_digits = write_index;
        self.trim();
    }
}

fn get_shift(n: usize) -> usize {
    const POWERS: [u8; 19] = [0, 3, 6, 9, 13, 16, 19, 23, 26, 29, 33, 36, 39, 43, 46, 49, 53, 56, 59];
    if n < POWERS.len() {
        POWERS[n] as usize
    } else {
        60
    }
}

/// (explicit mantissa bits, biased exponent) of the correctly rounded value of `d`.
fn parse_long_mantissa(d: &mut Decimal, fmt: &Format) -> (u64, i32) {
    const MAX_SHIFT: usize = 60;
    if d.num_digits == 0 || d.decimal_point < -324 {
        return (0, 0);
    } else if d.decimal_point >= 310 {
        return (0, fmt.infinite_power);
    }
    let mut exp2 = 0i32;
    // shift right toward (1/2 .. 1]
    while d.decimal_point > 0 {
        let n = d.decimal_point as usize;
        let shift = get_shift(n);
        d.right_shift(shift);
        if d.decimal_point < -DECIMAL_POINT_RANGE {
            return (0, 0);
        }
        exp2 += shift as i32;
    }
    // shift left toward (1/2 .. 1]
    while d.decimal_point <= 0 {
        let shift = if d.decimal_point == 0 {
            let dg = d.digits[0];
            if dg >= 5 {
                break;
            }
            if dg == 0 || dg == 1 {
                2
            } else {
                1
            }
        } else {
            get_shift((-d.decimal_point) as usize)
        };
        d.left_shift(shift);
        if d.decimal_point > DECIMAL_POINT_RANGE {
            return (0, fmt.infinite_power);
        }
        exp2 -= shift as i32;
    }
    // [1/2 .. 1] -> [1 .. 2]
    exp2 -= 1;
    while (fmt.minimum_exponent + 1) > exp2 {
        let mut n = ((fmt.minimum_exponent + 1) - exp2) as usize;
        if n > MAX_SHIFT {
            n = MAX_SHIFT;
        }
        d.right_shift(n);
        exp2 += n as i32;
    }
    if (exp2 - fmt.minimum_exponent) >= fmt.infinite_power {
        return (0, fmt.infinite_power);
    }
    d.left_shift(fmt.mantissa_bits as usize + 1);
    let mut mantissa = d.round();
    if mantissa >= (1u64 << (fmt.mantissa_bits + 1)) {
        // rounding carried into a new bit
        d.right_shift(1);
        exp2 += 1;
        mantissa = d.round();
        if (exp2 - fmt.minimum_exponent) >= fmt.infinite_power {
            return (0, fmt.infinite_power);
        }
    }
    let mut power2 = exp2 - fmt.minimum_exponent;
    if mantissa < (1u64 << fmt.mantissa_bits) {
        power2 -= 1;
    }
    mantissa &= (1u64 << fmt.mantissa_bits) - 1;
    (mantissa, power2)
}

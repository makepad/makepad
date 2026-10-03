//! Decimal string -> f64/f32, correctly rounded, accepting exactly real core's syntax:
//! `[+-]` then `inf` | `infinity` | `nan` (any case) or `digits[.digits][(e|E)[+-]digits]`
//! with at least one mantissa digit. Exact small values take the fast path (one exact
//! multiply/divide); everything else goes through the simple decimal conversion
//! (Nigel Tao's, as real core's slow path), which is exact for any input length.

use crate::fmt;

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

#[cfg(not(rapid_check))]
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
}

const F64: Format = Format { mantissa_bits: 52, minimum_exponent: -1023, infinite_power: 0x7ff, fast_exp: 22, disguised_exp: 37 };
const F32: Format = Format { mantissa_bits: 23, minimum_exponent: -127, infinite_power: 0xff, fast_exp: 10, disguised_exp: 17 };

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
    while i < s.len() && s[i].is_ascii_digit() {
        i += 1;
    }
    let int_end = i;
    let mut frac_start = i;
    let mut frac_end = i;
    if i < s.len() && s[i] == b'.' {
        i += 1;
        frac_start = i;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
        }
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

    // fast path: at most 19 significant digits, small exponent
    if let Some((m, e)) = small_mantissa(s, int_start, int_end, frac_start, frac_end, exp_number) {
        if m == 0 {
            return Ok(Parsed::Bits(negative, 0));
        }
        let max_mant = 2u64 << fmt.mantissa_bits;
        if e >= -fmt.fast_exp && e <= fmt.fast_exp && m <= max_mant {
            return Ok(Parsed::Fast(negative, m, e));
        }
        if e > fmt.fast_exp && e <= fmt.disguised_exp {
            let mut mm = m;
            let mut k = e - fmt.fast_exp;
            let mut ok = true;
            while k > 0 {
                match mm.checked_mul(10) {
                    Some(v) => mm = v,
                    None => {
                        ok = false;
                        break;
                    }
                }
                k -= 1;
            }
            if ok && mm <= max_mant {
                return Ok(Parsed::Fast(negative, mm, fmt.fast_exp));
            }
        }
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

/// The significant digits as a u64 (None if more than 19) and the decimal exponent.
fn small_mantissa(s: &[u8], int_start: usize, int_end: usize, frac_start: usize, frac_end: usize, exp: i64) -> Option<(u64, i64)> {
    let mut m: u64 = 0;
    let mut count = 0;
    let mut i = int_start;
    while i < int_end {
        let dgt = (s[i] - b'0') as u64;
        if m != 0 || dgt != 0 {
            count += 1;
            if count > 19 {
                return None;
            }
        }
        m = m * 10 + dgt;
        i += 1;
    }
    let mut j = frac_start;
    while j < frac_end {
        let dgt = (s[j] - b'0') as u64;
        if m != 0 || dgt != 0 {
            count += 1;
            if count > 19 {
                return None;
            }
        }
        m = m * 10 + dgt;
        j += 1;
    }
    Some((m, exp - (frac_end - frac_start) as i64))
}

fn fast_f64(m: u64, e: i64) -> f64 {
    let v = m as f64;
    let mut p = 1.0f64;
    let mut k = if e < 0 { -e } else { e };
    while k > 0 {
        p *= 10.0;
        k -= 1;
    }
    if e < 0 {
        v / p
    } else {
        v * p
    }
}

fn fast_f32(m: u64, e: i64) -> f32 {
    let v = m as f32;
    let mut p = 1.0f32;
    let mut k = if e < 0 { -e } else { e };
    while k > 0 {
        p *= 10.0;
        k -= 1;
    }
    if e < 0 {
        v / p
    } else {
        v * p
    }
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

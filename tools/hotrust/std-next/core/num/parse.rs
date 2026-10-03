//! Integer parsing shared by the generated `from_str_radix` of every integer type. Error kinds
//! follow real core exactly, including which error wins (overflow is detected digit by digit,
//! so "300x" as u8 is PosOverflow, not InvalidDigit).

use super::{IntErrorKind, ParseIntError};

fn err(kind: IntErrorKind) -> ParseIntError {
    ParseIntError { kind }
}

#[track_caller]
fn check_radix(radix: u32) {
    if radix < 2 || radix > 36 {
        crate::panicking::panic_fmt(format_args!("from_ascii_radix: radix must lie in the range `[2, 36]` - found {}", radix));
    }
}

fn digit(c: u8, radix: u32) -> Option<u32> {
    let d = match c {
        b'0'..=b'9' => (c - b'0') as u32,
        b'a'..=b'z' => (c - b'a') as u32 + 10,
        b'A'..=b'Z' => (c - b'A') as u32 + 10,
        _ => return None,
    };
    if d < radix {
        Some(d)
    } else {
        None
    }
}

/// Splits the sign: (is_positive, digits). A lone sign is an invalid digit; '-' is a sign only
/// for signed types.
fn split_sign(src: &[u8], signed: bool) -> Result<(bool, &[u8]), ParseIntError> {
    if src.is_empty() {
        return Err(err(IntErrorKind::Empty));
    }
    match src[0] {
        b'+' | b'-' if src.len() == 1 => Err(err(IntErrorKind::InvalidDigit)),
        b'+' => Ok((true, &src[1..])),
        b'-' if signed => Ok((false, &src[1..])),
        _ => Ok((true, src)),
    }
}

/// Parses an unsigned value `<= max`.
#[track_caller]
pub fn parse_unsigned(src: &[u8], radix: u32, max: u64) -> Result<u64, ParseIntError> {
    check_radix(radix);
    let (_, digits) = split_sign(src, false)?;
    let mut v: u64 = 0;
    let mut i = 0;
    while i < digits.len() {
        let d = match digit(digits[i], radix) {
            Some(d) => d,
            None => return Err(err(IntErrorKind::InvalidDigit)),
        };
        v = match v.checked_mul(radix as u64) {
            Some(x) => x,
            None => return Err(err(IntErrorKind::PosOverflow)),
        };
        v = match v.checked_add(d as u64) {
            Some(x) => x,
            None => return Err(err(IntErrorKind::PosOverflow)),
        };
        if v > max {
            return Err(err(IntErrorKind::PosOverflow));
        }
        i += 1;
    }
    Ok(v)
}

/// Parses a signed value in [min, max].
#[track_caller]
pub fn parse_signed(src: &[u8], radix: u32, min: i64, max: i64) -> Result<i64, ParseIntError> {
    check_radix(radix);
    let (positive, digits) = split_sign(src, true)?;
    let limit: u64 = if positive { max as u64 } else { (min as u64).wrapping_neg() };
    let mut v: u64 = 0;
    let mut i = 0;
    while i < digits.len() {
        let d = match digit(digits[i], radix) {
            Some(d) => d,
            None => return Err(err(IntErrorKind::InvalidDigit)),
        };
        let over = if positive { IntErrorKind::PosOverflow } else { IntErrorKind::NegOverflow };
        v = match v.checked_mul(radix as u64) {
            Some(x) => x,
            None => return Err(err(over)),
        };
        v = match v.checked_add(d as u64) {
            Some(x) => x,
            None => return Err(err(over)),
        };
        if v > limit {
            return Err(err(over));
        }
        i += 1;
    }
    Ok(if positive { v as i64 } else { (v as i64).wrapping_neg() })
}

#[track_caller]
pub fn parse_u128(src: &[u8], radix: u32) -> Result<u128, ParseIntError> {
    check_radix(radix);
    let (_, digits) = split_sign(src, false)?;
    let mut v: u128 = 0;
    let mut i = 0;
    while i < digits.len() {
        let d = match digit(digits[i], radix) {
            Some(d) => d,
            None => return Err(err(IntErrorKind::InvalidDigit)),
        };
        v = match v.checked_mul(radix as u128) {
            Some(x) => x,
            None => return Err(err(IntErrorKind::PosOverflow)),
        };
        v = match v.checked_add(d as u128) {
            Some(x) => x,
            None => return Err(err(IntErrorKind::PosOverflow)),
        };
        i += 1;
    }
    Ok(v)
}

#[track_caller]
pub fn parse_i128(src: &[u8], radix: u32) -> Result<i128, ParseIntError> {
    check_radix(radix);
    let (positive, digits) = split_sign(src, true)?;
    let limit: u128 = if positive { i128::MAX as u128 } else { (i128::MAX as u128) + 1 };
    let over = if positive { IntErrorKind::PosOverflow } else { IntErrorKind::NegOverflow };
    let mut v: u128 = 0;
    let mut i = 0;
    while i < digits.len() {
        let d = match digit(digits[i], radix) {
            Some(d) => d,
            None => return Err(err(IntErrorKind::InvalidDigit)),
        };
        v = match v.checked_mul(radix as u128) {
            Some(x) => x,
            None => return Err(err(over)),
        };
        v = match v.checked_add(d as u128) {
            Some(x) => x,
            None => return Err(err(over)),
        };
        if v > limit {
            return Err(err(over));
        }
        i += 1;
    }
    Ok(if positive { v as i128 } else { (v as i128).wrapping_neg() })
}

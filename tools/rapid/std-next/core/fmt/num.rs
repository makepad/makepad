//! Integer formatting: Display (decimal), Debug ({:x?}/{:X?} aware), LowerHex, UpperHex,
//! Octal, Binary for every integer type, through Formatter::pad_integral as in real core.
//! Shared non-generic helpers here; the per-type impls are generated (num/impls.rs, by
//! check/src/gen_fmt.rs).

mod impls;

use super::{Formatter, Result};

fn ascii(b: &[u8]) -> &str {
    unsafe { crate::intrinsics_mem::str_from_raw(b.as_ptr(), b.len()) }
}

/// Decimal digits of `n` with the sign decided by `is_nonnegative`.
pub(crate) fn fmt_u64(n: u64, is_nonnegative: bool, f: &mut Formatter<'_>) -> Result {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    let mut n = n;
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    f.pad_integral(is_nonnegative, "", ascii(&buf[i..]))
}

pub(crate) fn fmt_u128(n: u128, is_nonnegative: bool, f: &mut Formatter<'_>) -> Result {
    let mut buf = [0u8; 40];
    let mut i = buf.len();
    let mut n = n;
    // 19-digit chunks through u64 arithmetic
    while n >= 10_000_000_000_000_000_000u128 {
        let mut low = (n % 10_000_000_000_000_000_000u128) as u64;
        n /= 10_000_000_000_000_000_000u128;
        let mut k = 0;
        while k < 19 {
            i -= 1;
            buf[i] = b'0' + (low % 10) as u8;
            low /= 10;
            k += 1;
        }
    }
    let mut n = n as u64;
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    f.pad_integral(is_nonnegative, "", ascii(&buf[i..]))
}

fn prefix(shift: u32) -> &'static str {
    match shift {
        1 => "0b",
        3 => "0o",
        _ => "0x",
    }
}

fn digit(d: u8, upper: bool) -> u8 {
    if d < 10 {
        b'0' + d
    } else if upper {
        b'A' + (d - 10)
    } else {
        b'a' + (d - 10)
    }
}

/// Radix 2^shift (binary 1, octal 3, hex 4) of the unsigned bits `x`.
pub(crate) fn fmt_radix_u64(x: u64, shift: u32, upper: bool, f: &mut Formatter<'_>) -> Result {
    let mut buf = [0u8; 64];
    let mut i = buf.len();
    let mask = (1u64 << shift) - 1;
    let mut x = x;
    loop {
        i -= 1;
        buf[i] = digit((x & mask) as u8, upper);
        x >>= shift;
        if x == 0 {
            break;
        }
    }
    f.pad_integral(true, prefix(shift), ascii(&buf[i..]))
}

pub(crate) fn fmt_radix_u128(x: u128, shift: u32, upper: bool, f: &mut Formatter<'_>) -> Result {
    let mut buf = [0u8; 128];
    let mut i = buf.len();
    let mask = (1u128 << shift) - 1;
    let mut x = x;
    loop {
        i -= 1;
        buf[i] = digit((x & mask) as u8, upper);
        x >>= shift;
        if x == 0 {
            break;
        }
    }
    f.pad_integral(true, prefix(shift), ascii(&buf[i..]))
}

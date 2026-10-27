//! Integer formatting: Display (decimal), Debug ({:x?}/{:X?} aware), LowerHex, UpperHex,
//! Octal, Binary for every integer type, through Formatter::pad_integral as in real core.
//! Shared non-generic helpers here; the per-type impls are generated (num/impls.rs, by
//! check/src/gen_fmt.rs).

mod impls;

use super::{Formatter, Result};

fn ascii(b: &[u8]) -> &str {
    unsafe { crate::intrinsics_mem::str_from_raw(b.as_ptr(), b.len()) }
}

// The string of all two-digit numbers in range 00..99 (real core's DECIMAL_PAIRS lookup table).
// A const, not real core's static: the same immutable bytes, read without a static's
// per-domain address lookup.
const DECIMAL_PAIRS: &[u8; 200] = b"\
      0001020304050607080910111213141516171819\
      2021222324252627282930313233343536373839\
      4041424344454647484950515253545556575859\
      6061626364656667686970717273747576777879\
      8081828384858687888990919293949596979899";

/// Port of real core's `_fmt_inner` (library/core/src/fmt/num.rs): four decimals per step from the pair
/// table, then two, then the last one, right-aligned in `buf`; returns the first written index.
/// `$Unsigned` arithmetic: u32 for values that fit (real core formats u8..u32 at their width), else u64.
/// Unchecked accesses where real core states the bounds (`assert_unchecked(offset >= 4)`, the pair
/// indices below 100 from `% 1_00_00 / 100` and `% 100`, which its optimiser proves).
macro_rules! dec_inner {
    ($name:ident, $T:ty) => {
        fn $name(n: $T, buf: &mut [u8; 20]) -> usize {
            let mut offset = buf.len();
            let mut remain = n;
            let pairs = DECIMAL_PAIRS.as_ptr();
            let out = buf.as_mut_ptr();
            while remain > 999 {
                offset -= 4;
                let quad = remain % 1_00_00;
                remain /= 1_00_00;
                let pair1 = (quad / 100) as usize;
                let pair2 = (quad % 100) as usize;
                // SAFETY: 20 bytes hold any u64's 20 decimals and the loop writes 4 more only
                // while at least 4 decimals remain (offset >= 4); pair1, pair2 < 100
                unsafe {
                    *out.add(offset) = *pairs.add(pair1 * 2);
                    *out.add(offset + 1) = *pairs.add(pair1 * 2 + 1);
                    *out.add(offset + 2) = *pairs.add(pair2 * 2);
                    *out.add(offset + 3) = *pairs.add(pair2 * 2 + 1);
                }
            }
            if remain > 9 {
                offset -= 2;
                let pair = (remain % 100) as usize;
                remain /= 100;
                // SAFETY: two more decimals fit (see above); pair < 100
                unsafe {
                    *out.add(offset) = *pairs.add(pair * 2);
                    *out.add(offset + 1) = *pairs.add(pair * 2 + 1);
                }
            }
            if remain != 0 || n == 0 {
                offset -= 1;
                let last = (remain & 15) as usize;
                // SAFETY: one more decimal fits; last < 10
                unsafe { *out.add(offset) = *pairs.add(last * 2 + 1) };
            }
            offset
        }
    };
}
dec_inner!(dec_u32, u32);
dec_inner!(dec_u64, u64);

/// Decimal digits of `n` with the sign decided by `is_nonnegative`.
pub(crate) fn fmt_u64(n: u64, is_nonnegative: bool, f: &mut Formatter<'_>) -> Result {
    let mut buf = [0u8; 20];
    let i = if n <= u32::MAX as u64 { dec_u32(n as u32, &mut buf) } else { dec_u64(n, &mut buf) };
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

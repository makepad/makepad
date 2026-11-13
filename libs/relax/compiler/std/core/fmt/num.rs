//! Integer formatting: Display (decimal), Debug ({:x?}/{:X?} aware), LowerHex, UpperHex,
//! Octal, Binary, LowerExp, UpperExp for every integer type, through Formatter::pad_integral
//! as in real core. Shared non-generic helpers here; the per-type impls are generated
//! (num/impls.rs, by gen/src/gen_fmt.rs).

mod impls;

use super::float::{pad_formatted_parts, Formatted, Part};
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

/// Port of real core's `impl_Exp` (library/core/src/fmt/num.rs): `n` as a coefficient in [1, 10)
/// and its power of ten, `1.234e3`. Without a precision the trailing zeros are dropped; a
/// precision below the digit count rounds the coefficient half to even, one above it appends
/// zeros. `$LEN`: the type's decimal digits, the point and `e99`.
macro_rules! exp_inner {
    ($name:ident, $T:ty, $LEN:expr) => {
        pub(crate) fn $name(n: $T, is_nonnegative: bool, upper: bool, f: &mut Formatter<'_>) -> Result {
            let mut exp = n.checked_ilog10().unwrap_or(0) as usize;
            // the digits as an integer, coef_prec of them after the point
            let mut coef = n;
            let mut coef_prec = exp;
            let more_prec = match f.precision {
                None => {
                    while coef_prec != 0 && coef % 10 == 0 {
                        coef /= 10;
                        coef_prec -= 1;
                    }
                    0
                }
                Some(fmt_prec) if fmt_prec >= coef_prec => fmt_prec - coef_prec,
                Some(fmt_prec) => {
                    // fewer than exp digits to drop: the power of ten stays below n
                    let scale = (10 as $T).pow((coef_prec - fmt_prec) as u32);
                    let floor = coef / scale;
                    let over = coef % scale;
                    let half = scale / 2;
                    let round_up = if over < half {
                        0
                    } else if over > half {
                        1
                    } else {
                        floor & 1
                    };
                    coef = floor + round_up;
                    coef_prec = fmt_prec;
                    // 9.95 to one decimal is 10.0: drop that zero for one more power of ten
                    if round_up != 0 && coef.checked_ilog10().unwrap_or(0) as usize > coef_prec {
                        coef /= 10;
                        exp += 1;
                    }
                    0
                }
            };
            let mut buf = [0u8; $LEN];
            let coef_len = if coef_prec == 0 && more_prec == 0 {
                1
            } else {
                buf[1] = b'.';
                let mut i = 2 + coef_prec;
                while i > 2 {
                    i -= 1;
                    buf[i] = b'0' + (coef % 10) as u8;
                    coef /= 10;
                }
                2 + coef_prec
            };
            buf[0] = b'0' + coef as u8;
            buf[coef_len] = if upper { b'E' } else { b'e' };
            let text_len = if exp < 10 {
                buf[coef_len + 1] = b'0' + exp as u8;
                coef_len + 2
            } else {
                buf[coef_len + 1] = b'0' + (exp / 10) as u8;
                buf[coef_len + 2] = b'0' + (exp % 10) as u8;
                coef_len + 3
            };
            if more_prec == 0 {
                return f.pad_integral(is_nonnegative, "", ascii(&buf[..text_len]));
            }
            let sign = if !is_nonnegative {
                "-"
            } else if f.sign_plus() {
                "+"
            } else {
                ""
            };
            let mut out = Formatted::new(sign);
            out.push(Part::Copy(&buf[..coef_len]));
            out.push(Part::Zero(more_prec));
            out.push(Part::Copy(&buf[coef_len..text_len]));
            pad_formatted_parts(f, &out)
        }
    };
}
exp_inner!(fmt_exp_u64, u64, 24);
exp_inner!(fmt_exp_u128, u128, 43);

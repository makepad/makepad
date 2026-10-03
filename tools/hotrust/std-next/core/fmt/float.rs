//! Display / Debug / LowerExp / UpperExp for f32 and f64, with real core's output:
//! shortest round-trip digits without a precision, exactly rounded (half to even on the
//! exact binary value) digits with one. Port of core::num::flt2dec's decoder and string
//! assembly over the Dragon digit generator in float/dragon.rs.

mod bignum;
mod dragon;

use super::{rt, Alignment, Debug, Display, Formatter, LowerExp, Result, UpperExp};

/// A finite non-zero float as `mant * 2^exp`, with its rounding interval
/// `(mant - minus) * 2^exp ..= (mant + plus) * 2^exp` (inclusive when mant is even).
pub(crate) struct Decoded {
    pub mant: u64,
    pub minus: u64,
    pub plus: u64,
    pub exp: i16,
    pub inclusive: bool,
}

pub(crate) enum FullDecoded {
    Nan,
    Infinite,
    Zero,
    Finite(Decoded),
}

/// (negative, decoded) from the IEEE fields of an f64/f32.
fn decode_parts(negative: bool, exp_field: u32, frac: u64, frac_bits: u32, bias_plus_bits: i16, exp_max: u32) -> (bool, FullDecoded) {
    if exp_field == exp_max {
        return (negative, if frac == 0 { FullDecoded::Infinite } else { FullDecoded::Nan });
    }
    if exp_field == 0 && frac == 0 {
        return (negative, FullDecoded::Zero);
    }
    // integer_decode: v = mant * 2^exp
    let (mant, exp) = if exp_field == 0 {
        (frac << 1, -bias_plus_bits)
    } else {
        (frac | (1u64 << frac_bits), exp_field as i16 - bias_plus_bits)
    };
    let even = (mant & 1) == 0;
    let d = if exp_field == 0 {
        Decoded { mant, minus: 1, plus: 1, exp, inclusive: even }
    } else if mant == (1u64 << frac_bits) {
        // power of two: the gap below is half the gap above
        Decoded { mant: mant << 2, minus: 1, plus: 2, exp: exp - 2, inclusive: even }
    } else {
        Decoded { mant: mant << 1, minus: 1, plus: 1, exp: exp - 1, inclusive: even }
    };
    (negative, FullDecoded::Finite(d))
}

pub(crate) fn decode_f64(v: f64) -> (bool, FullDecoded) {
    let bits = v.to_bits();
    decode_parts(
        (bits >> 63) != 0,
        ((bits >> 52) & 0x7ff) as u32,
        bits & 0xf_ffff_ffff_ffff,
        52,
        1023 + 52,
        0x7ff,
    )
}

pub(crate) fn decode_f32(v: f32) -> (bool, FullDecoded) {
    let bits = v.to_bits();
    decode_parts((bits >> 31) != 0, (bits >> 23) & 0xff, (bits & 0x7f_ffff) as u64, 23, 127 + 23, 0xff)
}

// ---------------------------------------------------------------- parts

#[derive(Clone, Copy)]
enum Part<'a> {
    /// n zeros
    Zero(usize),
    /// an exponent value (decimal)
    Num(u16),
    Copy(&'a [u8]),
}

impl<'a> Part<'a> {
    fn len(&self) -> usize {
        match *self {
            Part::Zero(n) => n,
            Part::Num(v) => {
                if v < 1_000 {
                    if v < 10 {
                        1
                    } else if v < 100 {
                        2
                    } else {
                        3
                    }
                } else if v < 10_000 {
                    4
                } else {
                    5
                }
            }
            Part::Copy(b) => b.len(),
        }
    }

    fn write(&self, f: &mut Formatter<'_>) -> Result {
        match *self {
            Part::Zero(n) => {
                const ZEROES: &str = "0000000000000000000000000000000000000000000000000000000000000000";
                let mut n = n;
                while n > ZEROES.len() {
                    f.buf.write_str(ZEROES)?;
                    n -= ZEROES.len();
                }
                f.buf.write_str(&ZEROES[..n])
            }
            Part::Num(v) => {
                let mut buf = [0u8; 5];
                let len = self.len();
                let mut v = v;
                let mut i = len;
                while i > 0 {
                    i -= 1;
                    buf[i] = b'0' + (v % 10) as u8;
                    v /= 10;
                }
                f.buf.write_str(ascii(&buf[..len]))
            }
            Part::Copy(b) => f.buf.write_str(ascii(b)),
        }
    }
}

/// The digit buffers only ever hold ASCII.
fn ascii(b: &[u8]) -> &str {
    unsafe { crate::intrinsics_mem::str_from_raw(b.as_ptr(), b.len()) }
}

/// A formatted number: sign plus up to six parts.
struct Formatted<'a> {
    sign: &'static str,
    parts: [Part<'a>; 6],
    n: usize,
}

impl<'a> Formatted<'a> {
    fn new(sign: &'static str) -> Formatted<'a> {
        Formatted { sign, parts: [Part::Zero(0); 6], n: 0 }
    }
    fn push(&mut self, p: Part<'a>) {
        self.parts[self.n] = p;
        self.n += 1;
    }
    fn len(&self) -> usize {
        let mut len = self.sign.len();
        let mut i = 0;
        while i < self.n {
            len += self.parts[i].len();
            i += 1;
        }
        len
    }
    fn write_parts(&self, f: &mut Formatter<'_>) -> Result {
        let mut i = 0;
        while i < self.n {
            self.parts[i].write(f)?;
            i += 1;
        }
        Ok(())
    }
}

/// Pads like core's Formatter::pad_formatted_parts (zero padding goes after the sign).
fn pad_formatted_parts(f: &mut Formatter<'_>, formatted: &Formatted<'_>) -> Result {
    let width = match f.width {
        Some(w) => w,
        None => {
            f.buf.write_str(formatted.sign)?;
            return formatted.write_parts(f);
        }
    };
    let mut width = width;
    let mut sign = formatted.sign;
    let old_fill = f.fill;
    let old_align = f.align;
    if f.sign_aware_zero_pad() {
        f.buf.write_str(sign)?;
        width = width.saturating_sub(sign.len());
        sign = "";
        f.fill = '0';
        f.align = rt::ALIGN_RIGHT;
    }
    let len = formatted.len() - formatted.sign.len() + sign.len();
    let ret = if width <= len {
        match f.buf.write_str(sign) {
            Ok(()) => formatted.write_parts(f),
            Err(e) => Err(e),
        }
    } else {
        match f.padding(width - len, Alignment::Right) {
            Ok(post) => match f.buf.write_str(sign) {
                Ok(()) => match formatted.write_parts(f) {
                    Ok(()) => post.write(f),
                    Err(e) => Err(e),
                },
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        }
    };
    f.fill = old_fill;
    f.align = old_align;
    ret
}

fn determine_sign(plus: bool, full: &FullDecoded, negative: bool) -> &'static str {
    match full {
        FullDecoded::Nan => "",
        _ => {
            if negative {
                "-"
            } else if plus {
                "+"
            } else {
                ""
            }
        }
    }
}

fn digits_to_dec_str<'a>(out: &mut Formatted<'a>, buf: &'a [u8], exp: i16, frac_digits: usize) {
    if exp <= 0 {
        let minus_exp = (-(exp as i32)) as usize;
        out.push(Part::Copy(b"0."));
        out.push(Part::Zero(minus_exp));
        out.push(Part::Copy(buf));
        if frac_digits > buf.len() && frac_digits - buf.len() > minus_exp {
            out.push(Part::Zero((frac_digits - buf.len()) - minus_exp));
        }
    } else {
        let exp = exp as usize;
        if exp < buf.len() {
            out.push(Part::Copy(&buf[..exp]));
            out.push(Part::Copy(b"."));
            out.push(Part::Copy(&buf[exp..]));
            if frac_digits > buf.len() - exp {
                out.push(Part::Zero(frac_digits - (buf.len() - exp)));
            }
        } else {
            out.push(Part::Copy(buf));
            out.push(Part::Zero(exp - buf.len()));
            if frac_digits > 0 {
                out.push(Part::Copy(b"."));
                out.push(Part::Zero(frac_digits));
            }
        }
    }
}

fn digits_to_exp_str<'a>(out: &mut Formatted<'a>, buf: &'a [u8], exp: i16, min_ndigits: usize, upper: bool) {
    out.push(Part::Copy(&buf[..1]));
    if buf.len() > 1 || min_ndigits > 1 {
        out.push(Part::Copy(b"."));
        out.push(Part::Copy(&buf[1..]));
        if min_ndigits > buf.len() {
            out.push(Part::Zero(min_ndigits - buf.len()));
        }
    }
    let exp = exp as i32 - 1;
    if exp < 0 {
        out.push(Part::Copy(if upper { b"E-" } else { b"e-" }));
        out.push(Part::Num((-exp) as u16));
    } else {
        out.push(Part::Copy(if upper { b"E" } else { b"e" }));
        out.push(Part::Num(exp as u16));
    }
}

/// Exact digits buffer: large enough for the exact expansion of any f64.
const EXACT_BUF: usize = 1024;

fn fmt_decimal_exact(f: &mut Formatter<'_>, negative: bool, full: FullDecoded, frac_digits: usize) -> Result {
    let sign = determine_sign(f.sign_plus(), &full, negative);
    let mut buf = [0u8; EXACT_BUF];
    let mut out = Formatted::new(sign);
    match full {
        FullDecoded::Nan => out.push(Part::Copy(b"NaN")),
        FullDecoded::Infinite => out.push(Part::Copy(b"inf")),
        FullDecoded::Zero => {
            if frac_digits > 0 {
                out.push(Part::Copy(b"0."));
                out.push(Part::Zero(frac_digits));
            } else {
                out.push(Part::Copy(b"0"));
            }
        }
        FullDecoded::Finite(ref d) => {
            let maxlen = dragon::estimate_max_buf_len(d.exp);
            let limit = if frac_digits < 0x8000 { -(frac_digits as i16) } else { i16::MIN };
            let (len, exp) = dragon::format_exact(d, &mut buf[..maxlen], limit);
            if exp <= limit {
                if frac_digits > 0 {
                    out.push(Part::Copy(b"0."));
                    out.push(Part::Zero(frac_digits));
                } else {
                    out.push(Part::Copy(b"0"));
                }
            } else {
                digits_to_dec_str(&mut out, &buf[..len], exp, frac_digits);
            }
        }
    }
    pad_formatted_parts(f, &out)
}

fn fmt_decimal_shortest(f: &mut Formatter<'_>, negative: bool, full: FullDecoded, frac_digits: usize) -> Result {
    let sign = determine_sign(f.sign_plus(), &full, negative);
    let mut buf = [0u8; dragon::MAX_SIG_DIGITS];
    let mut out = Formatted::new(sign);
    match full {
        FullDecoded::Nan => out.push(Part::Copy(b"NaN")),
        FullDecoded::Infinite => out.push(Part::Copy(b"inf")),
        FullDecoded::Zero => {
            if frac_digits > 0 {
                out.push(Part::Copy(b"0."));
                out.push(Part::Zero(frac_digits));
            } else {
                out.push(Part::Copy(b"0"));
            }
        }
        FullDecoded::Finite(ref d) => {
            let (len, exp) = dragon::format_shortest(d, &mut buf);
            digits_to_dec_str(&mut out, &buf[..len], exp, frac_digits);
        }
    }
    pad_formatted_parts(f, &out)
}

fn fmt_exp_exact(f: &mut Formatter<'_>, negative: bool, full: FullDecoded, ndigits: usize, upper: bool) -> Result {
    let sign = determine_sign(f.sign_plus(), &full, negative);
    let mut buf = [0u8; EXACT_BUF];
    let mut out = Formatted::new(sign);
    match full {
        FullDecoded::Nan => out.push(Part::Copy(b"NaN")),
        FullDecoded::Infinite => out.push(Part::Copy(b"inf")),
        FullDecoded::Zero => {
            if ndigits > 1 {
                out.push(Part::Copy(b"0."));
                out.push(Part::Zero(ndigits - 1));
                out.push(Part::Copy(if upper { b"E0" } else { b"e0" }));
            } else {
                out.push(Part::Copy(if upper { b"0E0" } else { b"0e0" }));
            }
        }
        FullDecoded::Finite(ref d) => {
            let maxlen = dragon::estimate_max_buf_len(d.exp);
            let trunc = if ndigits < maxlen { ndigits } else { maxlen };
            let (len, exp) = dragon::format_exact(d, &mut buf[..trunc], i16::MIN);
            digits_to_exp_str(&mut out, &buf[..len], exp, ndigits, upper);
        }
    }
    pad_formatted_parts(f, &out)
}

fn fmt_exp_shortest(f: &mut Formatter<'_>, negative: bool, full: FullDecoded, upper: bool) -> Result {
    let sign = determine_sign(f.sign_plus(), &full, negative);
    let mut buf = [0u8; dragon::MAX_SIG_DIGITS];
    let mut out = Formatted::new(sign);
    match full {
        FullDecoded::Nan => out.push(Part::Copy(b"NaN")),
        FullDecoded::Infinite => out.push(Part::Copy(b"inf")),
        FullDecoded::Zero => out.push(Part::Copy(if upper { b"0E0" } else { b"0e0" })),
        FullDecoded::Finite(ref d) => {
            let (len, exp) = dragon::format_shortest(d, &mut buf);
            digits_to_exp_str(&mut out, &buf[..len], exp, 0, upper);
        }
    }
    pad_formatted_parts(f, &out)
}

fn fmt_display(f: &mut Formatter<'_>, negative: bool, full: FullDecoded) -> Result {
    match f.precision {
        Some(p) => fmt_decimal_exact(f, negative, full, p),
        None => fmt_decimal_shortest(f, negative, full, 0),
    }
}

/// `{:?}`: like Display but with at least one fractional digit, and in exponent form
/// when `use_exp` (|v| != 0 and |v| < 1e-4 or |v| >= 1e16).
fn fmt_debug(f: &mut Formatter<'_>, negative: bool, full: FullDecoded, use_exp: bool) -> Result {
    match f.precision {
        Some(p) => fmt_decimal_exact(f, negative, full, p),
        None => {
            if use_exp {
                fmt_exp_shortest(f, negative, full, false)
            } else {
                fmt_decimal_shortest(f, negative, full, 1)
            }
        }
    }
}

fn fmt_exp(f: &mut Formatter<'_>, negative: bool, full: FullDecoded, upper: bool) -> Result {
    match f.precision {
        Some(p) => fmt_exp_exact(f, negative, full, p + 1, upper),
        None => fmt_exp_shortest(f, negative, full, upper),
    }
}

impl Display for f64 {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        let (neg, full) = decode_f64(*self);
        fmt_display(f, neg, full)
    }
}

impl Debug for f64 {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        let abs = self.abs();
        let use_exp = (abs != 0.0 && abs < 1e-4) || abs >= 1e16;
        let (neg, full) = decode_f64(*self);
        fmt_debug(f, neg, full, use_exp)
    }
}

impl LowerExp for f64 {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        let (neg, full) = decode_f64(*self);
        fmt_exp(f, neg, full, false)
    }
}

impl UpperExp for f64 {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        let (neg, full) = decode_f64(*self);
        fmt_exp(f, neg, full, true)
    }
}

impl Display for f32 {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        let (neg, full) = decode_f32(*self);
        fmt_display(f, neg, full)
    }
}

impl Debug for f32 {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        let abs = self.abs();
        let use_exp = (abs != 0.0 && abs < 1e-4) || abs >= 1e16;
        let (neg, full) = decode_f32(*self);
        fmt_debug(f, neg, full, use_exp)
    }
}

impl LowerExp for f32 {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        let (neg, full) = decode_f32(*self);
        fmt_exp(f, neg, full, false)
    }
}

impl UpperExp for f32 {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        let (neg, full) = decode_f32(*self);
        fmt_exp(f, neg, full, true)
    }
}

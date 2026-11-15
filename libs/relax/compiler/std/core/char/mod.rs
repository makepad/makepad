//! char: conversions, UTF-8/16 coding, properties, case mapping and escapes.
//! The `impl char` methods (char/inherent.rs) forward to the free fns here.

mod inherent;
mod convert_impls;
// What `char`'s methods are made of, outside `core::char` itself: real core has no such free
// functions there, so `use std::char; char::is_ascii_hexdigit` is the primitive's method
// (rustc_resolve late.rs resolve_qpath falls back to the primitive when the module has no item).
pub(crate) mod methods;

use crate::fmt;
use crate::fmt::Write as _;
use crate::iter::{DoubleEndedIterator, ExactSizeIterator, FusedIterator, IntoIterator, Iterator};
use crate::option::Option::{self, None, Some};
use crate::result::Result::{self, Err, Ok};

pub const MAX: char = '\u{10FFFF}';
pub const REPLACEMENT_CHARACTER: char = '\u{FFFD}';
pub const UNICODE_VERSION: (u8, u8, u8) = crate::unicode::UNICODE_VERSION;

// ---------------------------------------------------------------- conversions

pub const fn from_u32(i: u32) -> Option<char> {
    if (i > 0x10FFFF) || (i >= 0xD800 && i <= 0xDFFF) {
        None
    } else {
        Some(unsafe { from_u32_unchecked(i) })
    }
}

pub const unsafe fn from_u32_unchecked(i: u32) -> char {
    crate::intrinsics_mem::transmute::<u32, char>(i)
}

pub const fn from_digit(num: u32, radix: u32) -> Option<char> {
    if radix > 36 {
        panic!("from_digit: radix is too high (maximum 36)");
    }
    if num < radix {
        let num = num as u8;
        if num < 10 {
            Some((b'0' + num) as char)
        } else {
            Some((b'a' + num - 10) as char)
        }
    } else {
        None
    }
}

// ---------------------------------------------------------------- UTF-8 / UTF-16

/// Encodes `code` into `dst`, returning the byte count; panics like real core when `dst`
/// is too short.
#[track_caller]
pub fn encode_utf8_raw(code: u32, dst: &mut [u8]) -> usize {
    let len = methods::len_utf8(code);
    if dst.len() < len {
        panic!(
            "encode_utf8: need {} bytes to encode U+{:04X} but buffer has just {}",
            len,
            code,
            dst.len()
        );
    }
    if len == 1 {
        dst[0] = code as u8;
    } else if len == 2 {
        dst[0] = (code >> 6 & 0x1F) as u8 | 0xC0;
        dst[1] = (code & 0x3F) as u8 | 0x80;
    } else if len == 3 {
        dst[0] = (code >> 12 & 0x0F) as u8 | 0xE0;
        dst[1] = (code >> 6 & 0x3F) as u8 | 0x80;
        dst[2] = (code & 0x3F) as u8 | 0x80;
    } else {
        dst[0] = (code >> 18 & 0x07) as u8 | 0xF0;
        dst[1] = (code >> 12 & 0x3F) as u8 | 0x80;
        dst[2] = (code >> 6 & 0x3F) as u8 | 0x80;
        dst[3] = (code & 0x3F) as u8 | 0x80;
    }
    len
}

#[track_caller]
pub fn encode_utf16_raw(code: u32, dst: &mut [u16]) -> usize {
    let len = methods::len_utf16(code);
    if dst.len() < len {
        panic!(
            "encode_utf16: need {} bytes to encode U+{:04X} but buffer has just {}",
            len,
            code,
            dst.len()
        );
    }
    if len == 1 {
        dst[0] = code as u16;
    } else {
        let c = code - 0x1_0000;
        dst[0] = 0xD800 | ((c >> 10) as u16);
        dst[1] = 0xDC00 | ((c as u16) & 0x3FF);
    }
    len
}

// ---------------------------------------------------------------- properties

// ---------------------------------------------------------------- case mapping

/// Up to three chars of a case mapping, iterated from both ends.
#[derive(Clone, Debug)]
pub struct CaseMappingIter {
    chars: [char; 3],
    start: usize,
    end: usize,
}

impl CaseMappingIter {
    fn new(chars: [char; 3]) -> CaseMappingIter {
        let end = if chars[1] == '\0' {
            1
        } else if chars[2] == '\0' {
            2
        } else {
            3
        };
        CaseMappingIter { chars, start: 0, end }
    }
    fn next(&mut self) -> Option<char> {
        if self.start < self.end {
            let c = self.chars[self.start];
            self.start += 1;
            Some(c)
        } else {
            None
        }
    }
    fn next_back(&mut self) -> Option<char> {
        if self.start < self.end {
            self.end -= 1;
            Some(self.chars[self.end])
        } else {
            None
        }
    }
    fn len(&self) -> usize {
        self.end - self.start
    }
    fn write_to(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut i = self.start;
        while i < self.end {
            f.write_char(self.chars[i])?;
            i += 1;
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ToLowercase(CaseMappingIter);

#[derive(Clone, Debug)]
pub struct ToUppercase(CaseMappingIter);

impl Iterator for ToLowercase {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        self.0.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.0.len();
        (n, Some(n))
    }
}
impl DoubleEndedIterator for ToLowercase {
    fn next_back(&mut self) -> Option<char> {
        self.0.next_back()
    }
}
impl ExactSizeIterator for ToLowercase {}
impl FusedIterator for ToLowercase {}
impl fmt::Display for ToLowercase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.write_to(f)
    }
}

impl Iterator for ToUppercase {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        self.0.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.0.len();
        (n, Some(n))
    }
}
impl DoubleEndedIterator for ToUppercase {
    fn next_back(&mut self) -> Option<char> {
        self.0.next_back()
    }
}
impl ExactSizeIterator for ToUppercase {}
impl FusedIterator for ToUppercase {}
impl fmt::Display for ToUppercase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.write_to(f)
    }
}

// ---------------------------------------------------------------- escapes

/// The chars of an escape sequence (at most `\u{10ffff}` = 10 chars) or one raw char.
#[derive(Clone, Debug)]
pub struct EscapeIter {
    chars: [char; 10],
    start: u8,
    end: u8,
}

impl EscapeIter {
    fn printable(c: char) -> EscapeIter {
        let mut chars = ['\0'; 10];
        chars[0] = c;
        EscapeIter { chars, start: 0, end: 1 }
    }
    fn backslash(c: char) -> EscapeIter {
        let mut chars = ['\0'; 10];
        chars[0] = '\\';
        chars[1] = c;
        EscapeIter { chars, start: 0, end: 2 }
    }
    fn unicode(c: char) -> EscapeIter {
        let mut chars = ['\0'; 10];
        chars[0] = '\\';
        chars[1] = 'u';
        chars[2] = '{';
        let code = c as u32;
        // number of hex digits, at least one
        let mut digits = 1;
        while digits < 8 && (code >> (4 * digits)) != 0 {
            digits += 1;
        }
        let mut i = 0;
        while i < digits {
            let nib = (code >> (4 * (digits - 1 - i))) & 0xf;
            chars[3 + i] = HEX[nib as usize] as char;
            i += 1;
        }
        chars[3 + digits] = '}';
        EscapeIter { chars, start: 0, end: (4 + digits) as u8 }
    }
    fn next(&mut self) -> Option<char> {
        if self.start < self.end {
            let c = self.chars[self.start as usize];
            self.start += 1;
            Some(c)
        } else {
            None
        }
    }
    fn len(&self) -> usize {
        (self.end - self.start) as usize
    }
    fn write_to(&self, w: &mut dyn fmt::Write) -> fmt::Result {
        let mut i = self.start;
        while i < self.end {
            w.write_char(self.chars[i as usize])?;
            i += 1;
        }
        Ok(())
    }
}

const HEX: [u8; 16] = *b"0123456789abcdef";

/// Options of real core's escape_debug_ext.
#[derive(Clone, Copy)]
pub(crate) struct EscapeDebugArgs {
    pub escape_grapheme_extender: bool,
    pub escape_single_quote: bool,
    pub escape_double_quote: bool,
}

pub(crate) const ESCAPE_ALL: EscapeDebugArgs =
    EscapeDebugArgs { escape_grapheme_extender: true, escape_single_quote: true, escape_double_quote: true };

pub(crate) fn escape_debug_ext(c: char, args: EscapeDebugArgs) -> EscapeIter {
    match c {
        '"' if args.escape_double_quote => EscapeIter::backslash('"'),
        '\'' if args.escape_single_quote => EscapeIter::backslash('\''),
        '\\' => EscapeIter::backslash('\\'),
        '\n' => EscapeIter::backslash('n'),
        '\t' => EscapeIter::backslash('t'),
        '\r' => EscapeIter::backslash('r'),
        '\0' => EscapeIter::backslash('0'),
        '\x20'..='\x7e' => EscapeIter::printable(c),
        _ => {
            if args.escape_grapheme_extender && crate::unicode::is_grapheme_extend_printable(c) {
                EscapeIter::unicode(c)
            } else if crate::unicode::is_printable(c) {
                EscapeIter::printable(c)
            } else {
                EscapeIter::unicode(c)
            }
        }
    }
}

pub(crate) fn escape_default_iter(c: char) -> EscapeIter {
    match c {
        '\t' => EscapeIter::backslash('t'),
        '\r' => EscapeIter::backslash('r'),
        '\n' => EscapeIter::backslash('n'),
        '\\' | '\'' | '"' => EscapeIter::backslash(c),
        '\x20'..='\x7e' => EscapeIter::printable(c),
        _ => EscapeIter::unicode(c),
    }
}

/// The body of `<char as Debug>::fmt` (between the quotes).
pub(crate) fn write_escape_debug_char(c: char, w: &mut dyn fmt::Write) -> fmt::Result {
    let args = EscapeDebugArgs { escape_grapheme_extender: true, escape_single_quote: true, escape_double_quote: false };
    escape_debug_ext(c, args).write_to(w)
}

#[derive(Clone, Debug)]
pub struct EscapeDebug(EscapeIter);

#[derive(Clone, Debug)]
pub struct EscapeDefault(EscapeIter);

#[derive(Clone, Debug)]
pub struct EscapeUnicode(EscapeIter);

pub(crate) fn escape_debug_with(c: char, args: EscapeDebugArgs) -> EscapeDebug {
    EscapeDebug(escape_debug_ext(c, args))
}

impl EscapeDebug {
    pub(crate) fn write_to(&self, w: &mut dyn fmt::Write) -> fmt::Result {
        self.0.write_to(w)
    }
}
impl EscapeDefault {
    pub(crate) fn write_to(&self, w: &mut dyn fmt::Write) -> fmt::Result {
        self.0.write_to(w)
    }
}
impl EscapeUnicode {
    pub(crate) fn write_to(&self, w: &mut dyn fmt::Write) -> fmt::Result {
        self.0.write_to(w)
    }
}

impl Iterator for EscapeDebug {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        self.0.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.0.len();
        (n, Some(n))
    }
}
impl ExactSizeIterator for EscapeDebug {}
impl FusedIterator for EscapeDebug {}
impl fmt::Display for EscapeDebug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut i = self.0.start;
        while i < self.0.end {
            f.write_char(self.0.chars[i as usize])?;
            i += 1;
        }
        Ok(())
    }
}

impl Iterator for EscapeDefault {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        self.0.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.0.len();
        (n, Some(n))
    }
}
impl ExactSizeIterator for EscapeDefault {}
impl FusedIterator for EscapeDefault {}
impl fmt::Display for EscapeDefault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut i = self.0.start;
        while i < self.0.end {
            f.write_char(self.0.chars[i as usize])?;
            i += 1;
        }
        Ok(())
    }
}

impl Iterator for EscapeUnicode {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        self.0.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.0.len();
        (n, Some(n))
    }
}
impl ExactSizeIterator for EscapeUnicode {}
impl FusedIterator for EscapeUnicode {}
impl fmt::Display for EscapeUnicode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut i = self.0.start;
        while i < self.0.end {
            f.write_char(self.0.chars[i as usize])?;
            i += 1;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- UTF-16 decoding

#[derive(Clone, Debug)]
pub struct DecodeUtf16<I: Iterator<Item = u16>> {
    iter: I,
    buf: Option<u16>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DecodeUtf16Error {
    code: u16,
}

pub fn decode_utf16<I: IntoIterator<Item = u16>>(iter: I) -> DecodeUtf16<I::IntoIter> {
    DecodeUtf16 { iter: iter.into_iter(), buf: None }
}

fn is_utf16_surrogate(u: u16) -> bool {
    u >= 0xD800 && u <= 0xDFFF
}

impl<I: Iterator<Item = u16>> Iterator for DecodeUtf16<I> {
    type Item = Result<char, DecodeUtf16Error>;

    fn next(&mut self) -> Option<Result<char, DecodeUtf16Error>> {
        let u = match self.buf.take() {
            Some(buf) => buf,
            None => self.iter.next()?,
        };
        if !is_utf16_surrogate(u) {
            Some(Ok(unsafe { from_u32_unchecked(u as u32) }))
        } else if u >= 0xDC00 {
            Some(Err(DecodeUtf16Error { code: u }))
        } else {
            let u2 = match self.iter.next() {
                Some(u2) => u2,
                None => return Some(Err(DecodeUtf16Error { code: u })),
            };
            if u2 < 0xDC00 || u2 > 0xDFFF {
                self.buf = Some(u2);
                return Some(Err(DecodeUtf16Error { code: u }));
            }
            let c = ((((u & 0x3ff) as u32) << 10) | (u2 & 0x3ff) as u32) + 0x1_0000;
            Some(Ok(unsafe { from_u32_unchecked(c) }))
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let (low, high) = self.iter.size_hint();
        let (low_buf, high_buf) = match self.buf {
            None => (0, 0),
            Some(u) if !is_utf16_surrogate(u) => (1, 1),
            Some(_) if high == Some(0) => (1, 1),
            Some(_) => (0, 1),
        };
        let low = (low / 2 + low % 2) + low_buf;
        let high = match high {
            Some(h) => h.checked_add(high_buf),
            None => None,
        };
        (low, high)
    }
}

impl DecodeUtf16Error {
    pub fn unpaired_surrogate(&self) -> u16 {
        self.code
    }
}

/// Lowercase hex of `v` without leading zeros (what `{:x}` prints).
pub(crate) fn write_hex(v: u32, w: &mut dyn fmt::Write) -> fmt::Result {
    let mut digits = 1;
    while digits < 8 && (v >> (4 * digits)) != 0 {
        digits += 1;
    }
    let mut i = 0;
    while i < digits {
        let nib = (v >> (4 * (digits - 1 - i))) & 0xf;
        w.write_char(HEX[nib as usize] as char)?;
        i += 1;
    }
    Ok(())
}

impl fmt::Display for DecodeUtf16Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("unpaired surrogate found: ")?;
        write_hex(self.code as u32, f)
    }
}

// ---------------------------------------------------------------- errors

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CharTryFromError(());

impl CharTryFromError {
    pub(crate) fn new() -> CharTryFromError {
        CharTryFromError(())
    }
}

impl fmt::Display for CharTryFromError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt("converted integer out of range for `char`", f)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct TryFromCharError(pub(crate) ());

impl fmt::Display for TryFromCharError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt("unicode code point out of range", f)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum CharErrorKind {
    EmptyString,
    TooManyChars,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseCharError {
    kind: CharErrorKind,
}

impl fmt::Display for ParseCharError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self.kind {
            CharErrorKind::EmptyString => "cannot parse char from empty string",
            CharErrorKind::TooManyChars => "too many characters in string",
        };
        fmt::Display::fmt(s, f)
    }
}

impl crate::str::FromStr for char {
    type Err = ParseCharError;
    fn from_str(s: &str) -> Result<char, ParseCharError> {
        methods::parse_char(s)
    }
}

impl crate::error::Error for CharTryFromError {}
impl crate::error::Error for TryFromCharError {}
impl crate::error::Error for ParseCharError {}
impl crate::error::Error for DecodeUtf16Error {}

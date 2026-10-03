//! Chars, CharIndices, Bytes, EncodeUtf16 and the str escape iterators.

use super::sub;
use super::validations::{decode_at, decode_before};
use crate::char::{EscapeDebug as CharEscapeDebug, EscapeDebugArgs};
use crate::fmt;
use crate::iter::{DoubleEndedIterator, ExactSizeIterator, FusedIterator, Iterator};
use crate::option::Option::{self, None, Some};

// ---------------------------------------------------------------- Chars

#[derive(Clone)]
pub struct Chars<'a> {
    s: &'a str,
    front: usize,
    back: usize,
}

pub fn chars(s: &str) -> Chars<'_> {
    Chars { s, front: 0, back: s.len() }
}

impl<'a> Chars<'a> {
    /// The rest of the string.
    pub fn as_str(&self) -> &'a str {
        unsafe { sub(self.s, self.front, self.back) }
    }
    pub(crate) fn front_offset(&self) -> usize {
        self.front
    }
}

impl<'a> Iterator for Chars<'a> {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        if self.front >= self.back {
            return None;
        }
        let (cp, w) = decode_at(self.s.as_bytes(), self.front);
        self.front += w;
        Some(unsafe { crate::char::from_u32_unchecked(cp) })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.back - self.front;
        ((len + 3) / 4, Some(len))
    }
    fn count(self) -> usize {
        let b = self.s.as_bytes();
        let mut n = 0;
        let mut i = self.front;
        while i < self.back {
            if super::validations::is_utf8_char_boundary(b[i]) {
                n += 1;
            }
            i += 1;
        }
        n
    }
    fn last(self) -> Option<char> {
        let mut s = self;
        s.next_back()
    }
}

impl<'a> DoubleEndedIterator for Chars<'a> {
    fn next_back(&mut self) -> Option<char> {
        if self.front >= self.back {
            return None;
        }
        let (cp, w) = decode_before(self.s.as_bytes(), self.back);
        self.back -= w;
        Some(unsafe { crate::char::from_u32_unchecked(cp) })
    }
}

impl<'a> FusedIterator for Chars<'a> {}

impl<'a> fmt::Debug for Chars<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Chars(")?;
        f.debug_list().entries(self.clone()).finish()?;
        f.write_str(")")
    }
}

// ---------------------------------------------------------------- CharIndices

#[derive(Clone)]
pub struct CharIndices<'a> {
    iter: Chars<'a>,
}

impl<'a> fmt::Debug for CharIndices<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CharIndices").field("front_offset", &self.iter.front).field("iter", &self.iter).finish()
    }
}

pub fn char_indices(s: &str) -> CharIndices<'_> {
    CharIndices { iter: chars(s) }
}

impl<'a> CharIndices<'a> {
    pub fn as_str(&self) -> &'a str {
        self.iter.as_str()
    }
    /// Byte position of the next char (or the length when exhausted).
    pub fn offset(&self) -> usize {
        self.iter.front
    }
}

impl<'a> Iterator for CharIndices<'a> {
    type Item = (usize, char);
    fn next(&mut self) -> Option<(usize, char)> {
        let i = self.iter.front;
        match self.iter.next() {
            Some(c) => Some((i, c)),
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
    fn count(self) -> usize {
        self.iter.count()
    }
    fn last(self) -> Option<(usize, char)> {
        let mut s = self;
        s.next_back()
    }
}

impl<'a> DoubleEndedIterator for CharIndices<'a> {
    fn next_back(&mut self) -> Option<(usize, char)> {
        match self.iter.next_back() {
            Some(c) => Some((self.iter.back, c)),
            None => None,
        }
    }
}

impl<'a> FusedIterator for CharIndices<'a> {}

// ---------------------------------------------------------------- Bytes

#[derive(Clone, Debug)]
pub struct Bytes<'a> {
    b: &'a [u8],
    front: usize,
    back: usize,
}

pub fn bytes(s: &str) -> Bytes<'_> {
    let b = s.as_bytes();
    Bytes { b, front: 0, back: b.len() }
}

impl<'a> Iterator for Bytes<'a> {
    type Item = u8;
    fn next(&mut self) -> Option<u8> {
        if self.front < self.back {
            let v = self.b[self.front];
            self.front += 1;
            Some(v)
        } else {
            None
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.back - self.front;
        (n, Some(n))
    }
    fn count(self) -> usize {
        self.back - self.front
    }
    fn nth(&mut self, n: usize) -> Option<u8> {
        if n < self.back - self.front {
            self.front += n;
            self.next()
        } else {
            self.front = self.back;
            None
        }
    }
    fn last(self) -> Option<u8> {
        if self.front < self.back {
            Some(self.b[self.back - 1])
        } else {
            None
        }
    }
}

impl<'a> DoubleEndedIterator for Bytes<'a> {
    fn next_back(&mut self) -> Option<u8> {
        if self.front < self.back {
            self.back -= 1;
            Some(self.b[self.back])
        } else {
            None
        }
    }
    fn nth_back(&mut self, n: usize) -> Option<u8> {
        if n < self.back - self.front {
            self.back -= n;
            self.next_back()
        } else {
            self.back = self.front;
            None
        }
    }
}

impl<'a> ExactSizeIterator for Bytes<'a> {}
impl<'a> FusedIterator for Bytes<'a> {}

// ---------------------------------------------------------------- EncodeUtf16

#[derive(Clone)]
pub struct EncodeUtf16<'a> {
    chars: Chars<'a>,
    extra: u16,
}

pub fn encode_utf16(s: &str) -> EncodeUtf16<'_> {
    EncodeUtf16 { chars: chars(s), extra: 0 }
}

impl<'a> fmt::Debug for EncodeUtf16<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EncodeUtf16 { .. }")
    }
}

impl<'a> Iterator for EncodeUtf16<'a> {
    type Item = u16;
    fn next(&mut self) -> Option<u16> {
        if self.extra != 0 {
            let tmp = self.extra;
            self.extra = 0;
            return Some(tmp);
        }
        let ch = self.chars.next()?;
        let mut buf = [0u16; 2];
        let n = crate::char::encode_utf16_raw(ch as u32, &mut buf);
        if n == 2 {
            self.extra = buf[1];
        }
        Some(buf[0])
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.chars.back - self.chars.front;
        let lo = len / 3 + if len % 3 != 0 { 1 } else { 0 };
        if self.extra == 0 {
            (lo, Some(len))
        } else {
            (lo + 1, Some(len + 1))
        }
    }
}

impl<'a> FusedIterator for EncodeUtf16<'a> {}

// ---------------------------------------------------------------- escapes

pub(crate) const ESCAPE_CONTINUE: EscapeDebugArgs =
    EscapeDebugArgs { escape_grapheme_extender: false, escape_single_quote: true, escape_double_quote: true };

/// `str::escape_debug`: the first char escapes grapheme extenders, the rest do not.
#[derive(Clone)]
pub struct EscapeDebug<'a> {
    chars: Chars<'a>,
    first: bool,
    cur: Option<CharEscapeDebug>,
}

pub fn escape_debug(s: &str) -> EscapeDebug<'_> {
    EscapeDebug { chars: chars(s), first: true, cur: None }
}

impl<'a> Iterator for EscapeDebug<'a> {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        loop {
            if let Some(e) = &mut self.cur {
                if let Some(c) = e.next() {
                    return Some(c);
                }
                self.cur = None;
            }
            let c = self.chars.next()?;
            let args = if self.first { crate::char::ESCAPE_ALL } else { ESCAPE_CONTINUE };
            self.first = false;
            self.cur = Some(crate::char::escape_debug_with(c, args));
        }
    }
}

impl<'a> FusedIterator for EscapeDebug<'a> {}

impl<'a> fmt::Display for EscapeDebug<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut it = self.clone();
        while let Some(c) = it.next() {
            f.write_char(c)?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct EscapeDefault<'a> {
    chars: Chars<'a>,
    cur: Option<crate::char::EscapeDefault>,
}

pub fn escape_default(s: &str) -> EscapeDefault<'_> {
    EscapeDefault { chars: chars(s), cur: None }
}

impl<'a> Iterator for EscapeDefault<'a> {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        loop {
            if let Some(e) = &mut self.cur {
                if let Some(c) = e.next() {
                    return Some(c);
                }
                self.cur = None;
            }
            let c = self.chars.next()?;
            self.cur = Some(crate::char::escape_default(c));
        }
    }
}

impl<'a> FusedIterator for EscapeDefault<'a> {}

impl<'a> fmt::Display for EscapeDefault<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut it = self.clone();
        while let Some(c) = it.next() {
            f.write_char(c)?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct EscapeUnicode<'a> {
    chars: Chars<'a>,
    cur: Option<crate::char::EscapeUnicode>,
}

pub fn escape_unicode(s: &str) -> EscapeUnicode<'_> {
    EscapeUnicode { chars: chars(s), cur: None }
}

impl<'a> Iterator for EscapeUnicode<'a> {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        loop {
            if let Some(e) = &mut self.cur {
                if let Some(c) = e.next() {
                    return Some(c);
                }
                self.cur = None;
            }
            let c = self.chars.next()?;
            self.cur = Some(crate::char::escape_unicode(c));
        }
    }
}

impl<'a> FusedIterator for EscapeUnicode<'a> {}

impl<'a> fmt::Display for EscapeUnicode<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut it = self.clone();
        while let Some(c) = it.next() {
            f.write_char(c)?;
        }
        Ok(())
    }
}

/// The body of `<str as Debug>::fmt` (between the quotes): runs of chars that need no
/// escaping are written as one slice.
pub(crate) fn write_escape_debug(s: &str, w: &mut dyn fmt::Write) -> fmt::Result {
    let args = EscapeDebugArgs { escape_grapheme_extender: true, escape_single_quote: false, escape_double_quote: true };
    let b = s.as_bytes();
    let mut run_start = 0;
    let mut i = 0;
    while i < b.len() {
        let x = b[i];
        if x >= 0x20 && x <= 0x7e && x != b'\\' && x != b'"' {
            i += 1;
            continue;
        }
        let (cp, n) = decode_at(b, i);
        let c = unsafe { crate::char::from_u32_unchecked(cp) };
        let esc = crate::char::escape_debug_with(c, args);
        if esc.len() != 1 {
            w.write_str(unsafe { sub(s, run_start, i) })?;
            esc.write_to(w)?;
            run_start = i + n;
        }
        i += n;
    }
    w.write_str(unsafe { sub(s, run_start, b.len()) })
}

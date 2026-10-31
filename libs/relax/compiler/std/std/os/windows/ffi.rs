//! std::os::windows::ffi: OsStr / OsString as UTF-16 (real std's os/windows/ffi.rs). An OsStr
//! holds WTF-8 bytes (UTF-8 plus lone surrogates, encoded as UTF-8 would encode them), so every
//! UTF-16 sequence, valid or not, round-trips.

use alloc::vec::Vec;

use crate::ffi::{OsStr, OsString};

pub trait OsStringExt {
    fn from_wide(wide: &[u16]) -> Self;
}

impl OsStringExt for OsString {
    fn from_wide(wide: &[u16]) -> OsString {
        let mut bytes = Vec::with_capacity(wide.len());
        let mut i = 0;
        while i < wide.len() {
            let u = wide[i] as u32;
            let c = if (0xd800..0xdc00).contains(&u) && i + 1 < wide.len() && (0xdc00..0xe000).contains(&(wide[i + 1] as u32)) {
                i += 1;
                0x10000 + ((u - 0xd800) << 10) + (wide[i] as u32 - 0xdc00)
            } else {
                u
            };
            push_wtf8(&mut bytes, c);
            i += 1;
        }
        unsafe { OsString::from_encoded_bytes_unchecked(bytes) }
    }
}

/// One code point (or lone surrogate) as (generalized) UTF-8.
fn push_wtf8(out: &mut Vec<u8>, c: u32) {
    if c < 0x80 {
        out.push(c as u8);
    } else if c < 0x800 {
        out.push(0xc0 | (c >> 6) as u8);
        out.push(0x80 | (c & 0x3f) as u8);
    } else if c < 0x10000 {
        out.push(0xe0 | (c >> 12) as u8);
        out.push(0x80 | ((c >> 6) & 0x3f) as u8);
        out.push(0x80 | (c & 0x3f) as u8);
    } else {
        out.push(0xf0 | (c >> 18) as u8);
        out.push(0x80 | ((c >> 12) & 0x3f) as u8);
        out.push(0x80 | ((c >> 6) & 0x3f) as u8);
        out.push(0x80 | (c & 0x3f) as u8);
    }
}

pub trait OsStrExt {
    fn encode_wide(&self) -> EncodeWide<'_>;
}

impl OsStrExt for OsStr {
    fn encode_wide(&self) -> EncodeWide<'_> {
        EncodeWide { bytes: self.as_encoded_bytes(), pos: 0, extra: 0 }
    }
}

/// The UTF-16 code units of an OsStr (real std's EncodeWide).
#[derive(Clone)]
pub struct EncodeWide<'a> {
    bytes: &'a [u8],
    pos: usize,
    extra: u16,
}

impl<'a> Iterator for EncodeWide<'a> {
    type Item = u16;
    fn next(&mut self) -> Option<u16> {
        if self.extra != 0 {
            let e = self.extra;
            self.extra = 0;
            return Some(e);
        }
        let b = self.bytes;
        let i = self.pos;
        if i >= b.len() {
            return None;
        }
        let b0 = b[i] as u32;
        let (c, n) = if b0 < 0x80 {
            (b0, 1)
        } else if b0 < 0xe0 {
            (((b0 & 0x1f) << 6) | (b[i + 1] as u32 & 0x3f), 2)
        } else if b0 < 0xf0 {
            (((b0 & 0x0f) << 12) | ((b[i + 1] as u32 & 0x3f) << 6) | (b[i + 2] as u32 & 0x3f), 3)
        } else {
            (((b0 & 0x07) << 18) | ((b[i + 1] as u32 & 0x3f) << 12) | ((b[i + 2] as u32 & 0x3f) << 6) | (b[i + 3] as u32 & 0x3f), 4)
        };
        self.pos = i + n;
        if c >= 0x10000 {
            let c = c - 0x10000;
            self.extra = 0xdc00 | (c & 0x3ff) as u16;
            Some(0xd800 | (c >> 10) as u16)
        } else {
            Some(c as u16)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.bytes.len() - self.pos;
        ((left + 2) / 3 + (self.extra != 0) as usize, Some(left + (self.extra != 0) as usize))
    }
}

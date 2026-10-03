//! UTF-8 validation and decoding (same algorithm and error reporting as real core).

use crate::fmt;
use crate::option::Option::{self, None, Some};
use crate::result::Result::{self, Err, Ok};

/// Why a byte slice is not UTF-8.
#[derive(Copy, Eq, PartialEq, Clone, Debug)]
pub struct Utf8Error {
    pub(crate) valid_up_to: usize,
    pub(crate) error_len: Option<u8>,
}

impl Utf8Error {
    pub fn valid_up_to(&self) -> usize {
        self.valid_up_to
    }
    pub fn error_len(&self) -> Option<usize> {
        match self.error_len {
            Some(len) => Some(len as usize),
            None => None,
        }
    }
}

impl fmt::Display for Utf8Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(error_len) = self.error_len {
            f.write_str("invalid utf-8 sequence of ")?;
            super::write_dec(error_len as usize, f)?;
            f.write_str(" bytes from index ")?;
            super::write_dec(self.valid_up_to, f)
        } else {
            f.write_str("incomplete utf-8 byte sequence from index ")?;
            super::write_dec(self.valid_up_to, f)
        }
    }
}

#[cfg(not(rapid_check))]
impl crate::error::Error for Utf8Error {}

/// Width of a UTF-8 sequence from its first byte (0 = not a valid first byte).
pub const fn utf8_char_width(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b < 0xC2 {
        0
    } else if b < 0xE0 {
        2
    } else if b < 0xF0 {
        3
    } else if b < 0xF5 {
        4
    } else {
        0
    }
}

pub(crate) const fn utf8_is_cont_byte(byte: u8) -> bool {
    (byte as i8) < -64
}

pub(crate) const fn is_utf8_char_boundary(b: u8) -> bool {
    (b as i8) >= -0x40
}

pub(crate) fn run_utf8_validation(v: &[u8]) -> Result<(), Utf8Error> {
    let mut index = 0;
    let len = v.len();
    while index < len {
        let old_offset = index;
        let first = v[index];
        if first >= 128 {
            let w = utf8_char_width(first);
            if w == 2 {
                index += 1;
                if index >= len {
                    return Err(Utf8Error { valid_up_to: old_offset, error_len: None });
                }
                if v[index] as i8 >= -64 {
                    return Err(Utf8Error { valid_up_to: old_offset, error_len: Some(1) });
                }
            } else if w == 3 {
                index += 1;
                if index >= len {
                    return Err(Utf8Error { valid_up_to: old_offset, error_len: None });
                }
                let second = v[index];
                let ok = match first {
                    0xE0 => second >= 0xA0 && second <= 0xBF,
                    0xE1..=0xEC => second >= 0x80 && second <= 0xBF,
                    0xED => second >= 0x80 && second <= 0x9F,
                    0xEE..=0xEF => second >= 0x80 && second <= 0xBF,
                    _ => false,
                };
                if !ok {
                    return Err(Utf8Error { valid_up_to: old_offset, error_len: Some(1) });
                }
                index += 1;
                if index >= len {
                    return Err(Utf8Error { valid_up_to: old_offset, error_len: None });
                }
                if v[index] as i8 >= -64 {
                    return Err(Utf8Error { valid_up_to: old_offset, error_len: Some(2) });
                }
            } else if w == 4 {
                index += 1;
                if index >= len {
                    return Err(Utf8Error { valid_up_to: old_offset, error_len: None });
                }
                let second = v[index];
                let ok = match first {
                    0xF0 => second >= 0x90 && second <= 0xBF,
                    0xF1..=0xF3 => second >= 0x80 && second <= 0xBF,
                    0xF4 => second >= 0x80 && second <= 0x8F,
                    _ => false,
                };
                if !ok {
                    return Err(Utf8Error { valid_up_to: old_offset, error_len: Some(1) });
                }
                index += 1;
                if index >= len {
                    return Err(Utf8Error { valid_up_to: old_offset, error_len: None });
                }
                if v[index] as i8 >= -64 {
                    return Err(Utf8Error { valid_up_to: old_offset, error_len: Some(2) });
                }
                index += 1;
                if index >= len {
                    return Err(Utf8Error { valid_up_to: old_offset, error_len: None });
                }
                if v[index] as i8 >= -64 {
                    return Err(Utf8Error { valid_up_to: old_offset, error_len: Some(3) });
                }
            } else {
                return Err(Utf8Error { valid_up_to: old_offset, error_len: Some(1) });
            }
            index += 1;
        } else {
            // ASCII run
            index += 1;
            while index < len && v[index] < 128 {
                index += 1;
            }
        }
    }
    Ok(())
}

/// Decodes the code point starting at byte `i` of valid UTF-8 `b`: (code point, width).
pub(crate) fn decode_at(b: &[u8], i: usize) -> (u32, usize) {
    let x = b[i];
    if x < 128 {
        return (x as u32, 1);
    }
    let init = (x & (0x7F >> 2)) as u32;
    let y = b[i + 1];
    let mut ch = (init << 6) | (y & 0x3F) as u32;
    if x >= 0xE0 {
        let z = b[i + 2];
        let y_z = (((y & 0x3F) as u32) << 6) | (z & 0x3F) as u32;
        ch = (init << 12) | y_z;
        if x >= 0xF0 {
            let w = b[i + 3];
            ch = ((init & 7) << 18) | (y_z << 6) | (w & 0x3F) as u32;
            return (ch, 4);
        }
        return (ch, 3);
    }
    (ch, 2)
}

/// Decodes the code point ending just before byte `end` of valid UTF-8 `b`: (code point, width).
pub(crate) fn decode_before(b: &[u8], end: usize) -> (u32, usize) {
    let w = b[end - 1];
    if w < 128 {
        return (w as u32, 1);
    }
    let z = b[end - 2];
    let mut ch = (z & (0x7F >> 2)) as u32;
    let mut n = 2;
    if utf8_is_cont_byte(z) {
        let y = b[end - 3];
        ch = (y & (0x7F >> 3)) as u32;
        n = 3;
        if utf8_is_cont_byte(y) {
            let x = b[end - 4];
            ch = (x & (0x7F >> 4)) as u32;
            ch = (ch << 6) | (y & 0x3F) as u32;
            n = 4;
        }
        ch = (ch << 6) | (z & 0x3F) as u32;
    }
    ch = (ch << 6) | (w & 0x3F) as u32;
    (ch, n)
}

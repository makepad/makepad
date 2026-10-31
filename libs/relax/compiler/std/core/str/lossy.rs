//! Utf8Chunks: valid runs and the invalid byte sequences between them (from_utf8_lossy).

use super::validations::utf8_char_width;
use crate::iter::{FusedIterator, Iterator};
use crate::option::Option::{self, None, Some};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Utf8Chunk<'a> {
    valid: &'a str,
    invalid: &'a [u8],
}

impl<'a> Utf8Chunk<'a> {
    pub fn valid(&self) -> &'a str {
        self.valid
    }
    pub fn invalid(&self) -> &'a [u8] {
        self.invalid
    }
}

#[derive(Clone)]
pub struct Utf8Chunks<'a> {
    source: &'a [u8],
}

pub fn utf8_chunks(v: &[u8]) -> Utf8Chunks<'_> {
    Utf8Chunks { source: v }
}

fn safe_get(xs: &[u8], i: usize) -> u8 {
    if i < xs.len() {
        xs[i]
    } else {
        0
    }
}

impl<'a> Iterator for Utf8Chunks<'a> {
    type Item = Utf8Chunk<'a>;
    fn next(&mut self) -> Option<Utf8Chunk<'a>> {
        if self.source.is_empty() {
            return None;
        }
        let src = self.source;
        let mut i = 0;
        let mut valid_up_to = 0;
        while i < src.len() {
            let byte = src[i];
            i += 1;
            if byte >= 128 {
                let w = utf8_char_width(byte);
                if w == 2 {
                    if safe_get(src, i) & 192 != 128 {
                        break;
                    }
                    i += 1;
                } else if w == 3 {
                    let s = safe_get(src, i);
                    let ok = match byte {
                        0xE0 => s >= 0xA0 && s <= 0xBF,
                        0xE1..=0xEC => s >= 0x80 && s <= 0xBF,
                        0xED => s >= 0x80 && s <= 0x9F,
                        0xEE..=0xEF => s >= 0x80 && s <= 0xBF,
                        _ => false,
                    };
                    if !ok {
                        break;
                    }
                    i += 1;
                    if safe_get(src, i) & 192 != 128 {
                        break;
                    }
                    i += 1;
                } else if w == 4 {
                    let s = safe_get(src, i);
                    let ok = match byte {
                        0xF0 => s >= 0x90 && s <= 0xBF,
                        0xF1..=0xF3 => s >= 0x80 && s <= 0xBF,
                        0xF4 => s >= 0x80 && s <= 0x8F,
                        _ => false,
                    };
                    if !ok {
                        break;
                    }
                    i += 1;
                    if safe_get(src, i) & 192 != 128 {
                        break;
                    }
                    i += 1;
                    if safe_get(src, i) & 192 != 128 {
                        break;
                    }
                    i += 1;
                } else {
                    break;
                }
            }
            valid_up_to = i;
        }
        let valid = unsafe { super::from_utf8_unchecked(&src[..valid_up_to]) };
        let invalid = &src[valid_up_to..i];
        self.source = &src[i..];
        Some(Utf8Chunk { valid, invalid })
    }
}

impl<'a> FusedIterator for Utf8Chunks<'a> {}

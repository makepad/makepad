//! Word-at-a-time byte search, port of real core's slice::memchr (MIT/Apache-2.0).
//! Original rust-memchr implementation copyright 2015 Andrew Gallant, bluss,
//! and Nicolas Koch. Scan aligned pairs of words, then resolve hits bytewise.

use crate::option::Option::{self, None, Some};

#[inline]
fn contains_zero_byte(x: usize) -> bool {
    let lo = usize::MAX / 255;
    x.wrapping_sub(lo) & !x & (lo * 128) != 0
}

#[inline]
pub fn memchr(x: u8, text: &[u8]) -> Option<usize> {
    let word = crate::mem::size_of::<usize>();
    let ptr = text.as_ptr();
    let len = text.len();
    let mut offset = 0;
    if len >= 2 * word {
        let prefix = (word - (ptr as usize & (word - 1))) & (word - 1);
        while offset < prefix {
            if text[offset] == x { return Some(offset); }
            offset += 1;
        }
        let repeated = (usize::MAX / 255) * x as usize;
        while offset <= len - 2 * word {
            // Both loads are aligned and entirely within the slice.
            unsafe {
                let u = *(ptr.add(offset) as *const usize);
                let v = *(ptr.add(offset + word) as *const usize);
                if contains_zero_byte(u ^ repeated) || contains_zero_byte(v ^ repeated) { break; }
            }
            offset += 2 * word;
        }
    }
    while offset < len {
        if text[offset] == x { return Some(offset); }
        offset += 1;
    }
    None
}

#[inline]
pub fn memrchr(x: u8, text: &[u8]) -> Option<usize> {
    let word = crate::mem::size_of::<usize>();
    let ptr = text.as_ptr();
    let mut offset = text.len();
    if offset >= 2 * word {
        // Resolve the unaligned tail before loading aligned pairs backwards.
        let tail = (ptr as usize + offset) & (word - 1);
        let aligned = offset - tail;
        while offset > aligned {
            offset -= 1;
            if text[offset] == x { return Some(offset); }
        }
        let repeated = (usize::MAX / 255) * x as usize;
        while offset >= 2 * word {
            unsafe {
                let u = *(ptr.add(offset - 2 * word) as *const usize);
                let v = *(ptr.add(offset - word) as *const usize);
                if contains_zero_byte(u ^ repeated) || contains_zero_byte(v ^ repeated) { break; }
            }
            offset -= 2 * word;
        }
    }
    while offset > 0 {
        offset -= 1;
        if text[offset] == x { return Some(offset); }
    }
    None
}

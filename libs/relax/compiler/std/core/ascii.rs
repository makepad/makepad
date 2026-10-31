//! Operations on ASCII bytes.

use crate::fmt;
use crate::iter::FusedIterator;

#[derive(Clone)]
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct EscapeDefault {
    bytes: [u8; 4],
    front: usize,
    back: usize,
}

/// Produces a printable ASCII byte, a quoted escape, or a lowercase `\xNN` escape.
pub fn escape_default(c: u8) -> EscapeDefault {
    let (bytes, len) = match c {
        b'\t' => ([b'\\', b't', 0, 0], 2),
        b'\r' => ([b'\\', b'r', 0, 0], 2),
        b'\n' => ([b'\\', b'n', 0, 0], 2),
        b'\'' | b'"' | b'\\' => ([b'\\', c, 0, 0], 2),
        0x20..=0x7e => ([c, 0, 0, 0], 1),
        _ => {
            let hex = b"0123456789abcdef";
            ([b'\\', b'x', hex[(c >> 4) as usize], hex[(c & 0xf) as usize]], 4)
        }
    };
    EscapeDefault { bytes, front: 0, back: len }
}

impl Iterator for EscapeDefault {
    type Item = u8;

    fn next(&mut self) -> Option<u8> {
        if self.front == self.back {
            return None;
        }
        let byte = self.bytes[self.front];
        self.front += 1;
        Some(byte)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.back - self.front;
        (n, Some(n))
    }
}

impl DoubleEndedIterator for EscapeDefault {
    fn next_back(&mut self) -> Option<u8> {
        if self.front == self.back {
            return None;
        }
        self.back -= 1;
        Some(self.bytes[self.back])
    }
}

impl ExactSizeIterator for EscapeDefault {}
impl FusedIterator for EscapeDefault {}

impl fmt::Display for EscapeDefault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut i = self.front;
        while i < self.back {
            f.write_char(self.bytes[i] as char)?;
            i += 1;
        }
        Ok(())
    }
}

impl fmt::Debug for EscapeDefault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EscapeDefault").finish_non_exhaustive()
    }
}

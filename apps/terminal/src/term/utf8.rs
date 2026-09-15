//! DFA-based, non-allocating, error-replacing UTF-8 decoder.
//!
//! Port of ghostty `src/terminal/UTF8Decoder.zig`, itself based on Bjoern
//! Hoehrmann's DFA decoder (http://bjoern.hoehrmann.de/utf-8/decoder/dfa,
//! MIT licensed), modified for U+FFFD error replacement.

#[rustfmt::skip]
const CHAR_CLASSES: [u8; 256] = [
    0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,  0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,  0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,  0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
    0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,  0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
    1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,  9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,
    7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,  7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
    8,8,2,2,2,2,2,2,2,2,2,2,2,2,2,2,  2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,
   10,3,3,3,3,3,3,3,3,3,3,3,3,4,3,3, 11,6,6,6,5,8,8,8,8,8,8,8,8,8,8,8,
];

#[rustfmt::skip]
const TRANSITIONS: [u8; 108] = [
     0,12,24,36,60,96,84,12,12,12,48,72, 12,12,12,12,12,12,12,12,12,12,12,12,
    12, 0,12,12,12,12,12, 0,12, 0,12,12, 12,24,12,12,12,12,12,24,12,24,12,12,
    12,12,12,12,12,12,12,24,12,12,12,12, 12,24,12,12,12,12,12,12,12,24,12,12,
    12,12,12,12,12,12,12,36,12,36,12,12, 12,36,12,12,12,12,12,36,12,36,12,12,
    12,36,12,12,12,12,12,12,12,12,12,12,
];

const ACCEPT_STATE: u8 = 0;
const REJECT_STATE: u8 = 12;

#[derive(Default)]
pub struct Utf8Decoder {
    accumulator: u32,
    state: u8,
}

impl Utf8Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when no partial sequence is buffered.
    pub fn is_ground(&self) -> bool {
        self.state == ACCEPT_STATE
    }

    pub fn reset(&mut self) {
        self.accumulator = 0;
        self.state = ACCEPT_STATE;
    }

    /// Feed the next byte. Returns `(codepoint, consumed)`. If `consumed`
    /// is false, an ill-formed sequence was rejected: a replacement char is
    /// returned and the SAME byte must be fed again before continuing.
    #[inline]
    pub fn next(&mut self, byte: u8) -> (Option<u32>, bool) {
        let char_class = CHAR_CLASSES[byte as usize];
        let initial_state = self.state;

        if self.state != ACCEPT_STATE {
            self.accumulator <<= 6;
            self.accumulator |= (byte & 0x3f) as u32;
        } else {
            self.accumulator = ((0xffu32) >> char_class) & (byte as u32);
        }

        self.state = TRANSITIONS[(self.state + char_class) as usize];

        if self.state == ACCEPT_STATE {
            let cp = self.accumulator;
            self.accumulator = 0;
            (Some(cp), true)
        } else if self.state == REJECT_STATE {
            self.accumulator = 0;
            self.state = ACCEPT_STATE;
            // If we rejected the first byte in a sequence it was consumed,
            // otherwise it was not.
            (Some(0xfffd), initial_state == ACCEPT_STATE)
        } else {
            (None, true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(bytes: &[u8]) -> Vec<u32> {
        let mut d = Utf8Decoder::new();
        let mut out = Vec::new();
        for &b in bytes {
            let (cp, consumed) = d.next(b);
            if let Some(cp) = cp {
                out.push(cp);
            }
            if !consumed {
                let (cp, consumed2) = d.next(b);
                assert!(consumed2);
                if let Some(cp) = cp {
                    out.push(cp);
                }
            }
        }
        out
    }

    #[test]
    fn ascii() {
        assert_eq!(decode_all(b"hi"), vec!['h' as u32, 'i' as u32]);
    }

    #[test]
    fn multibyte() {
        assert_eq!(decode_all("é€𐍈".as_bytes()), vec![0xe9, 0x20ac, 0x10348]);
    }

    #[test]
    fn ill_formed_replacement() {
        // Lone continuation byte.
        assert_eq!(decode_all(&[0x80]), vec![0xfffd]);
        // Truncated 3-byte sequence followed by ASCII: replacement + ASCII.
        assert_eq!(decode_all(&[0xe2, 0x82, b'x']), vec![0xfffd, 'x' as u32]);
        // Overlong encoding is rejected.
        assert_eq!(decode_all(&[0xc0, 0xaf]), vec![0xfffd, 0xfffd]);
    }
}

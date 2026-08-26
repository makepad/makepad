//! MSB-first bit reader, the order FLAC packs frame and subframe bits in.
//!
//! Every read is bounds-checked and returns `None` past the end rather than
//! panicking: this reads attacker-supplied files, so running off the end is a
//! normal outcome, not a bug to assert about.

pub struct BitReader<'a> {
    data: &'a [u8],
    /// Absolute bit position from the start of `data`.
    bit: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, bit: 0 }
    }

    pub fn bits_left(&self) -> usize {
        (self.data.len() * 8).saturating_sub(self.bit)
    }

    pub fn bit_pos(&self) -> usize {
        self.bit
    }

    /// Byte index of the current bit. When the cursor is byte-aligned this is
    /// the next unread byte; otherwise it is the byte the cursor sits inside.
    pub fn byte_pos(&self) -> usize {
        self.bit / 8
    }

    pub fn is_byte_aligned(&self) -> bool {
        self.bit % 8 == 0
    }

    /// Consume the bits through the next byte boundary and report whether
    /// every one was zero. FLAC requires these frame-padding bits to be zero.
    pub fn read_zero_padding(&mut self) -> Option<bool> {
        let n = (8 - self.bit % 8) % 8;
        if n == 0 {
            Some(true)
        } else {
            self.read(n as u32).map(|v| v == 0)
        }
    }

    /// Consume `n` bits. `false` when the stream is exhausted (and then nothing
    /// is consumed).
    pub fn skip(&mut self, n: u32) -> bool {
        if self.bits_left() < n as usize {
            return false;
        }
        self.bit += n as usize;
        true
    }

    /// The next `n` bits (n <= 32) without consuming them. `None` past the end
    /// rather than zero-padding: a FLAC frame that runs out of bits is corrupt,
    /// not silently short.
    #[inline]
    pub fn peek(&self, n: u32) -> Option<u32> {
        if n == 0 {
            return Some(0);
        }
        if n > 32 || self.bits_left() < n as usize {
            return None;
        }
        let byte = self.bit >> 3;
        let off = (self.bit & 7) as u32;
        // 32 bits at an offset of up to 7 need 5 bytes.
        let mut acc = 0u64;
        for i in 0..5 {
            let b = self.data.get(byte + i).copied().unwrap_or(0) as u64;
            acc |= b << (56 - 8 * i);
        }
        acc <<= off;
        Some((acc >> (64 - n)) as u32)
    }

    /// Read `n` bits (n <= 32), MSB-first. `None` if the stream is exhausted.
    #[inline]
    pub fn read(&mut self, n: u32) -> Option<u32> {
        let v = self.peek(n)?;
        self.bit += n as usize;
        Some(v)
    }

    /// Single bit as a bool.
    #[inline]
    pub fn read_bit(&mut self) -> Option<bool> {
        self.read(1).map(|v| v != 0)
    }

    /// `n`-bit two's-complement signed integer.
    pub fn read_signed(&mut self, n: u32) -> Option<i32> {
        if n == 0 {
            return Some(0);
        }
        if n > 32 {
            return None;
        }
        let u = self.read(n)?;
        if n == 32 {
            return Some(u as i32);
        }
        let shift = 32 - n;
        Some(((u << shift) as i32) >> shift)
    }

    /// `n`-bit two's-complement signed integer, including the 33-bit side
    /// channel used by stereo streams whose coded sample size is 32 bits.
    pub fn read_signed_i64(&mut self, n: u32) -> Option<i64> {
        if n == 0 {
            return Some(0);
        }
        if n > 63 {
            return None;
        }
        let u = if n <= 32 {
            self.read(n)? as u64
        } else {
            let high_bits = n - 32;
            ((self.read(high_bits)? as u64) << 32) | self.read(32)? as u64
        };
        let shift = 64 - n;
        Some(((u << shift) as i64) >> shift)
    }

    /// Unary (stop bit 1): the number of zero bits before the next one.
    /// Bounded by remaining input, so a million leading zeros in a tiny slice
    /// cannot spin forever.
    pub fn read_unary(&mut self) -> Option<u32> {
        let mut n = 0u32;
        loop {
            let byte_i = self.bit >> 3;
            let b = *self.data.get(byte_i)?;
            let bit_off = self.bit & 7;
            let shifted = b << bit_off;
            if shifted != 0 {
                let zeros = shifted.leading_zeros();
                self.bit += zeros as usize + 1;
                return n.checked_add(zeros);
            }
            let rest = 8 - bit_off;
            n = n.checked_add(rest as u32)?;
            self.bit += rest;
        }
    }
}

/// Why a UTF-8-coded frame/sample number could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Utf8Error {
    Truncated,
    Invalid,
}

/// UTF-8-style unsigned integer used for the frame/sample number. The code is
/// canonical (overlong forms are invalid), but the value is an integer rather
/// than a Unicode scalar and the format therefore permits up to seven bytes.
pub fn read_utf8_uint(data: &[u8], at: usize) -> Result<(u64, usize), Utf8Error> {
    let first = *data.get(at).ok_or(Utf8Error::Truncated)?;
    let (extra, mut val) = match first {
        0x00..=0x7F => (0usize, first as u64),
        0xC0..=0xDF => (1, (first & 0x1F) as u64),
        0xE0..=0xEF => (2, (first & 0x0F) as u64),
        0xF0..=0xF7 => (3, (first & 0x07) as u64),
        0xF8..=0xFB => (4, (first & 0x03) as u64),
        0xFC..=0xFD => (5, (first & 0x01) as u64),
        0xFE => (6, 0u64),
        _ => return Err(Utf8Error::Invalid),
    };
    let end = at.checked_add(1 + extra).ok_or(Utf8Error::Truncated)?;
    if end > data.len() {
        return Err(Utf8Error::Truncated);
    }
    for i in 0..extra {
        let c = data[at + 1 + i];
        if c & 0xC0 != 0x80 {
            return Err(Utf8Error::Invalid);
        }
        val = (val << 6) | (c & 0x3F) as u64;
    }
    const MIN: [u64; 7] = [0, 0x80, 0x800, 0x1_0000, 0x20_0000, 0x400_0000, 0x8000_0000];
    if val < MIN[extra] {
        return Err(Utf8Error::Invalid);
    }
    Ok((val, 1 + extra))
}

/// Write a UTF-8-style unsigned integer (the encoder used by tests).
pub fn write_utf8_uint(out: &mut Vec<u8>, n: u64) {
    if n < 0x80 {
        out.push(n as u8);
        return;
    }
    // Payload bits: 11, 16, 21, 26, 31, 36 for 2..=7 byte sequences.
    let extra = if n < (1 << 11) {
        1
    } else if n < (1 << 16) {
        2
    } else if n < (1 << 21) {
        3
    } else if n < (1 << 26) {
        4
    } else if n < (1 << 31) {
        5
    } else {
        6
    };
    let lead_ones = extra + 1;
    let lead = (0xFFu8 << (8 - lead_ones)) | ((n >> (6 * extra)) as u8);
    out.push(lead);
    for i in (0..extra).rev() {
        out.push(0x80 | ((n >> (6 * i)) as u8 & 0x3F));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_msb_first_across_byte_boundaries() {
        let data = [0b1010_1100, 0b0011_0101];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read(3), Some(0b101));
        assert_eq!(r.read(1), Some(0));
        assert_eq!(r.read(8), Some(0b1100_0011));
        assert_eq!(r.read(4), Some(0b0101));
        assert_eq!(r.read(1), None);
    }

    #[test]
    fn peek_does_not_consume() {
        let data = [0xF0u8, 0x0F];
        let mut r = BitReader::new(&data);
        assert_eq!(r.peek(4), Some(0xF));
        assert_eq!(r.peek(12), Some(0xF00));
        assert_eq!(r.read(4), Some(0xF));
        assert_eq!(r.bits_left(), 12);
    }

    #[test]
    fn signed_sign_extends() {
        let data = [0b1111_0000];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_signed(4), Some(-1));
        assert_eq!(r.read_signed(4), Some(0));
    }

    #[test]
    fn signed_32_is_two_complement() {
        let data = 0x8000_0000u32.to_be_bytes();
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_signed(32), Some(i32::MIN));
    }

    #[test]
    fn signed_33_preserves_stereo_side_range() {
        let data = [0x3f, 0xff, 0xff, 0xff, 0x80];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_signed_i64(33), Some(i32::MAX as i64));

        let data = [0xc0, 0x00, 0x00, 0x00, 0x00];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_signed_i64(33), Some(i32::MIN as i64));
    }

    #[test]
    fn unary_counts_zeros() {
        // 0010_0001 : unary 2, then unary 0 after the 1, then 5 zeros and we
        // run off the end looking for a stop bit.
        let data = [0b0010_0000];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_unary(), Some(2));
        assert_eq!(r.read_unary(), None);
    }

    #[test]
    fn unary_across_bytes() {
        let data = [0x00, 0x00, 0x80];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_unary(), Some(16));
    }

    #[test]
    fn exhaustion_returns_none_not_panic() {
        let mut r = BitReader::new(&[0xFF]);
        assert_eq!(r.read(8), Some(0xFF));
        assert_eq!(r.read(1), None);
        assert_eq!(r.read(32), None);
        assert!(!r.skip(1));
        assert_eq!(r.read(0), Some(0));
        let mut empty = BitReader::new(&[]);
        assert_eq!(empty.peek(8), None);
        assert_eq!(empty.read_unary(), None);
        assert_eq!(empty.read_signed(16), None);
    }

    #[test]
    fn utf8_roundtrip_and_spec_examples() {
        for n in [0u64, 1, 127, 128, 0x7FF, 0x800, 0xFFFF, 0x1_0000, (1 << 36) - 1] {
            let mut buf = Vec::new();
            write_utf8_uint(&mut buf, n);
            let (got, len) = read_utf8_uint(&buf, 0).expect("roundtrip");
            assert_eq!(got, n, "n={n} bytes={buf:?}");
            assert_eq!(len, buf.len());
        }
        // One-byte and two-byte fixtures.
        assert_eq!(read_utf8_uint(&[0x00], 0), Ok((0, 1)));
        assert_eq!(read_utf8_uint(&[0x7F], 0), Ok((127, 1)));
        assert_eq!(read_utf8_uint(&[0xC2, 0x80], 0), Ok((0x80, 2)));
        // Continuation as a start byte is refused.
        assert_eq!(read_utf8_uint(&[0x80], 0), Err(Utf8Error::Invalid));
        // Truncated two-byte sequence.
        assert_eq!(read_utf8_uint(&[0xC2], 0), Err(Utf8Error::Truncated));
        // 0xFF is not a lead byte.
        assert_eq!(read_utf8_uint(&[0xFF], 0), Err(Utf8Error::Invalid));
        // Empty / past the end.
        assert_eq!(read_utf8_uint(&[], 0), Err(Utf8Error::Truncated));
        assert_eq!(read_utf8_uint(&[0x00], 1), Err(Utf8Error::Truncated));
        assert_eq!(read_utf8_uint(&[0xC0, 0x80], 0), Err(Utf8Error::Invalid));
        assert_eq!(read_utf8_uint(&[0xE0, 0x80, 0x80], 0), Err(Utf8Error::Invalid));
    }

    #[test]
    fn zero_padding_is_checked() {
        let data = [0xFF, 0x00];
        let mut r = BitReader::new(&data);
        assert!(r.read(3).is_some());
        assert_eq!(r.read_zero_padding(), Some(false));
        assert!(r.is_byte_aligned());
        assert_eq!(r.read(8), Some(0x00));

        let mut r = BitReader::new(&[0b1010_0000]);
        assert_eq!(r.read(3), Some(0b101));
        assert_eq!(r.read_zero_padding(), Some(true));
    }
}

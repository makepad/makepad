//! Bit readers. Zstandard uses two bit orders:
//! - forward, little-endian, least significant bit first (FSE table descriptions);
//! - backward (FSE / Huffman payloads): the stream is written forward but read from its end. The last byte
//!   holds a sentinel 1 bit above the final payload bit; reading walks from just below the sentinel down to
//!   bit 0 of the first byte.

/// `n` bits (n <= 56) starting at absolute bit `start` (bit i = byte i/8, bit i%8). Bytes past the end read 0.
#[inline]
pub(crate) fn bits_at(data: &[u8], start: usize, n: u32) -> u64 {
    debug_assert!(n <= 56);
    if n == 0 {
        return 0;
    }
    let byte = start >> 3;
    let shift = (start & 7) as u32;
    let word = if byte + 8 <= data.len() {
        u64::from_le_bytes(data[byte..byte + 8].try_into().unwrap())
    } else {
        let mut w = 0u64;
        for i in 0..8 {
            if let Some(&b) = data.get(byte + i) {
                w |= (b as u64) << (8 * i);
            }
        }
        w
    };
    (word >> shift) & ((1u64 << n) - 1)
}

/// Reads a backward bitstream. Reading past the start yields zero bits and marks the stream overflowed.
pub(crate) struct BackReader<'a> {
    data: &'a [u8],
    /// Number of unread bits; bits [0, pos) are unread. Negative once over-read.
    pos: isize,
}

impl<'a> BackReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Result<Self, String> {
        let last = *data.last().ok_or("empty bitstream")?;
        if last == 0 {
            return Err("bitstream has no end marker".into());
        }
        let pos = (data.len() * 8) as isize - (last.leading_zeros() as isize + 1);
        Ok(BackReader { data, pos })
    }

    /// The next `n` bits (n <= 56) without consuming them; missing bits below the start read as 0.
    #[inline]
    pub(crate) fn peek(&self, n: u32) -> u64 {
        if n == 0 {
            return 0;
        }
        let n_i = n as isize;
        if self.pos >= n_i {
            bits_at(self.data, (self.pos - n_i) as usize, n)
        } else if self.pos > 0 {
            bits_at(self.data, 0, self.pos as u32) << (n_i - self.pos)
        } else {
            0
        }
    }

    #[inline]
    pub(crate) fn consume(&mut self, n: u32) {
        self.pos -= n as isize;
    }

    #[inline]
    pub(crate) fn read(&mut self, n: u32) -> u64 {
        let v = self.peek(n);
        self.consume(n);
        v
    }

    /// Unread bits; negative after over-reading.
    #[inline]
    pub(crate) fn remaining(&self) -> isize {
        self.pos
    }

    #[inline]
    pub(crate) fn overflowed(&self) -> bool {
        self.pos < 0
    }
}

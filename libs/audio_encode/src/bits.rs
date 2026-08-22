//! LSB-first bit writer: the mirror of the decoder's `BitReader`.
//!
//! Vorbis packs header fields and residue vectors LSB-first, but Huffman
//! codewords are read bit-by-bit starting at the codeword's MSB, so codewords
//! are stored pre-reversed in the encode tables and written like any other
//! field ([`reverse_bits`]).

pub struct BitWriter {
    bytes: Vec<u8>,
    /// Bits accumulated below 8, LSB-first.
    acc: u64,
    /// Number of valid bits in `acc` (< 8 after every push).
    fill: u32,
}

impl Default for BitWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl BitWriter {
    pub fn new() -> Self {
        Self { bytes: Vec::new(), acc: 0, fill: 0 }
    }

    pub fn with_capacity(bytes: usize) -> Self {
        Self { bytes: Vec::with_capacity(bytes), acc: 0, fill: 0 }
    }

    /// Append the low `n` bits of `v`, LSB-first. `n <= 32`.
    #[inline]
    pub fn push(&mut self, v: u32, n: u32) {
        debug_assert!(n <= 32);
        debug_assert!(n == 32 || (v as u64) < (1u64 << n), "value {v} does not fit {n} bits");
        self.acc |= (v as u64) << self.fill;
        self.fill += n;
        while self.fill >= 8 {
            self.bytes.push((self.acc & 0xff) as u8);
            self.acc >>= 8;
            self.fill -= 8;
        }
    }

    #[inline]
    pub fn push_bit(&mut self, b: bool) {
        self.push(b as u32, 1);
    }

    /// Bits written so far, including the unflushed tail.
    pub fn bit_len(&self) -> usize {
        self.bytes.len() * 8 + self.fill as usize
    }

    /// Pad the tail with zero bits to a byte boundary and return the bytes.
    pub fn finish(mut self) -> Vec<u8> {
        if self.fill > 0 {
            self.bytes.push((self.acc & 0xff) as u8);
            self.acc = 0;
            self.fill = 0;
        }
        self.bytes
    }
}

/// The low `len` bits of `code`, reversed. Canonical Huffman codes are defined
/// MSB-first; the stream is written LSB-first; a decoder reading one bit at a
/// time therefore sees the MSB first when the codeword is stored reversed.
pub fn reverse_bits(code: u32, len: u32) -> u32 {
    let mut out = 0u32;
    for i in 0..len {
        out |= ((code >> (len - 1 - i)) & 1) << i;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_audio_decode::vorbis::bits::BitReader;

    #[test]
    fn writer_round_trips_through_the_decoder_reader() {
        let fields: &[(u32, u32)] = &[
            (0, 1),
            (0x564342, 24),
            (511, 9),
            (1, 1),
            (0xffff_ffff, 32),
            (5, 3),
            (0, 0),
            (129, 8),
        ];
        let mut w = BitWriter::new();
        for &(v, n) in fields {
            w.push(v, n);
        }
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes);
        for &(v, n) in fields {
            assert_eq!(r.read(n), Some(v), "field {v}:{n}");
        }
    }

    #[test]
    fn bit_len_counts_the_unflushed_tail() {
        let mut w = BitWriter::new();
        assert_eq!(w.bit_len(), 0);
        w.push(1, 3);
        assert_eq!(w.bit_len(), 3);
        w.push(0, 13);
        assert_eq!(w.bit_len(), 16);
        assert_eq!(w.finish().len(), 2);
    }

    #[test]
    fn reversed_codewords_decode_msb_first() {
        // Writing reverse_bits(code, len) must make a bit-at-a-time reader see
        // the code MSB-first, which is how the decoder walks its Huffman tree.
        let (code, len) = (0b110u32, 3u32);
        let mut w = BitWriter::new();
        w.push(reverse_bits(code, len), len);
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes);
        let mut seen = 0u32;
        for _ in 0..len {
            seen = (seen << 1) | r.read(1).unwrap();
        }
        assert_eq!(seen, code);
    }
}

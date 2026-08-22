//! LSB-first bit reader, the order Vorbis packs its bitstream in, plus the
//! three small spec helpers that go with it.
//!
//! Adapted from the same author's decoder in the sandbox repo
//! (`apps/sandbox/libs/audio/src/bitread.rs`); it lives inside the Vorbis
//! module here because nothing else in this crate reads bits this way (MP3 is
//! MSB-first).
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

    /// The next `n` bits (n <= 32) without consuming them, zero-padded past the
    /// end of the packet. Padding is what lets a Huffman lookup table be
    /// indexed unconditionally; the caller still has to refuse a codeword
    /// longer than [`BitReader::bits_left`].
    #[inline]
    pub fn peek(&self, n: u32) -> u32 {
        if n == 0 || n > 32 {
            return 0;
        }
        let byte = self.bit >> 3;
        let off = (self.bit & 7) as u32;
        // 32 bits at an offset of up to 7 need 5 bytes.
        let mut acc: u64 = 0;
        if byte + 8 <= self.data.len() {
            let chunk: [u8; 8] = self.data[byte..byte + 8].try_into().unwrap_or([0; 8]);
            acc = u64::from_le_bytes(chunk);
        } else {
            for i in 0..5 {
                if let Some(&b) = self.data.get(byte + i) {
                    acc |= (b as u64) << (8 * i);
                }
            }
        }
        let mask = if n == 32 { u32::MAX as u64 } else { (1u64 << n) - 1 };
        ((acc >> off) & mask) as u32
    }

    /// Consume `n` bits. `false` when the stream is exhausted (and then nothing
    /// is consumed).
    #[inline]
    pub fn skip(&mut self, n: u32) -> bool {
        if self.bits_left() < n as usize {
            return false;
        }
        self.bit += n as usize;
        true
    }

    /// Read `n` bits (n <= 32), LSB-first. `None` if the stream is exhausted.
    #[inline]
    pub fn read(&mut self, n: u32) -> Option<u32> {
        if n == 0 {
            return Some(0);
        }
        if n > 32 || self.bits_left() < n as usize {
            return None;
        }
        let v = self.peek(n);
        self.bit += n as usize;
        Some(v)
    }

    /// Single bit as a bool.
    #[inline]
    pub fn read_bit(&mut self) -> Option<bool> {
        self.read(1).map(|v| v != 0)
    }

    /// Read up to 64 bits as two halves (only used for the rare wide field).
    pub fn read_u64(&mut self, n: u32) -> Option<u64> {
        if n <= 32 {
            return self.read(n).map(|v| v as u64);
        }
        if n > 64 {
            return None;
        }
        let lo = self.read(32)? as u64;
        let hi = self.read(n - 32)? as u64;
        Some(lo | (hi << 32))
    }
}

/// `ilog` from the Vorbis spec: position of the highest set bit, 1-based.
pub fn ilog(x: i32) -> u32 {
    if x <= 0 {
        return 0;
    }
    32 - (x as u32).leading_zeros()
}

/// Vorbis' packed float format (not IEEE 754).
pub fn float32_unpack(x: u32) -> f32 {
    let mantissa = (x & 0x001f_ffff) as f64;
    let sign = x & 0x8000_0000;
    let exponent = ((x & 0x7fe0_0000) >> 21) as i32;
    let m = if sign != 0 { -mantissa } else { mantissa };
    (m * (2.0f64).powi(exponent - 788)) as f32
}

/// Largest integer `r` with `r^dim <= entries` (the spec's `lookup1_values`),
/// by bisection rather than the spec's linear walk — `entries` reaches 2^24 and
/// a crafted `dim` of 1 would otherwise be sixteen million multiplications.
pub fn lookup1_values(entries: u32, dim: u32) -> u32 {
    if dim == 0 {
        return 0;
    }
    if dim == 1 {
        return entries;
    }
    let pow_le = |r: u64| -> bool {
        // r^dim <= entries, computed without overflow.
        let mut p: u64 = 1;
        for _ in 0..dim {
            p = p.saturating_mul(r);
            if p > entries as u64 {
                return false;
            }
        }
        true
    };
    let (mut lo, mut hi) = (0u64, entries as u64 + 1);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if pow_le(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lsb_first_across_byte_boundaries() {
        // 0b1010_1100, 0b0000_0011
        let data = [0xACu8, 0x03];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read(4), Some(0b1100));
        assert_eq!(r.read(4), Some(0b1010));
        assert_eq!(r.read(8), Some(0x03));
        assert_eq!(r.read(1), None);
    }

    #[test]
    fn peek_does_not_consume_and_zero_pads() {
        let data = [0xFFu8, 0x0F];
        let r = BitReader::new(&data);
        assert_eq!(r.peek(4), 0xF);
        assert_eq!(r.peek(12), 0xFFF);
        assert_eq!(r.peek(16), 0x0FFF);
        // Past the end pads with zeroes rather than failing.
        assert_eq!(r.peek(32), 0x0FFF);
        assert_eq!(r.bits_left(), 16);
    }

    #[test]
    fn peek_matches_read_at_every_offset() {
        let data: Vec<u8> = (0..40u8).map(|i| i.wrapping_mul(37) ^ 0x5A).collect();
        for start in 0..64 {
            for n in 1..=32u32 {
                let mut a = BitReader::new(&data);
                assert!(a.skip(start));
                let peeked = a.peek(n);
                let read = a.read(n).unwrap();
                assert_eq!(peeked, read, "start={start} n={n}");
            }
        }
    }

    #[test]
    fn exhaustion_returns_none_not_panic() {
        let data = [0xFFu8];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read(8), Some(0xFF));
        assert_eq!(r.read(1), None);
        assert_eq!(r.read(32), None);
        assert!(!r.skip(1));
        // Reading zero bits is always fine.
        assert_eq!(r.read(0), Some(0));
        // And so is peeking at nothing.
        assert_eq!(r.peek(8), 0);
    }

    #[test]
    fn empty_input_is_not_a_panic() {
        let mut r = BitReader::new(&[]);
        assert_eq!(r.peek(32), 0);
        assert_eq!(r.read(1), None);
        assert_eq!(r.bits_left(), 0);
    }

    #[test]
    fn oversized_read_refused() {
        let data = [0u8; 16];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read(33), None);
        assert_eq!(r.peek(33), 0);
        assert_eq!(r.read_u64(65), None);
        assert_eq!(r.read_u64(40), Some(0));
    }

    #[test]
    fn ilog_matches_spec_examples() {
        assert_eq!(ilog(0), 0);
        assert_eq!(ilog(1), 1);
        assert_eq!(ilog(2), 2);
        assert_eq!(ilog(3), 2);
        assert_eq!(ilog(4), 3);
        assert_eq!(ilog(7), 3);
        assert_eq!(ilog(-1), 0);
        assert_eq!(ilog(i32::MAX), 31);
    }

    #[test]
    fn lookup1_values_is_the_integer_root() {
        // 3^2 = 9 <= 10, 4^2 = 16 > 10
        assert_eq!(lookup1_values(10, 2), 3);
        assert_eq!(lookup1_values(9, 2), 3);
        assert_eq!(lookup1_values(8, 3), 2);
        assert_eq!(lookup1_values(27, 3), 3);
        assert_eq!(lookup1_values(1, 1), 1);
        assert_eq!(lookup1_values(0, 4), 0);
        assert_eq!(lookup1_values(1 << 24, 1), 1 << 24);
        // Against the spec's own linear walk, over a spread of shapes.
        for entries in [0u32, 1, 2, 5, 16, 17, 63, 64, 65, 255, 4096, 65_535] {
            for dim in 1..=8u32 {
                let mut want = 0u32;
                loop {
                    let next = want as u64 + 1;
                    let mut p: u64 = 1;
                    for _ in 0..dim {
                        p = p.saturating_mul(next);
                    }
                    if p > entries as u64 {
                        break;
                    }
                    want += 1;
                }
                assert_eq!(lookup1_values(entries, dim), want, "entries={entries} dim={dim}");
            }
        }
    }

    #[test]
    fn float32_unpack_round_numbers() {
        // mantissa 1, exponent 788 => 1.0
        let v = float32_unpack((788 << 21) | 1);
        assert!((v - 1.0).abs() < 1e-6, "{v}");
        let n = float32_unpack(0x8000_0000 | (788 << 21) | 1);
        assert!((n + 1.0).abs() < 1e-6, "{n}");
        assert_eq!(float32_unpack(0), 0.0);
    }
}

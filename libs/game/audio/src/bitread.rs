//! LSB-first bit reader, the order Vorbis packs its bitstream in.
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

    /// Read `n` bits (n <= 32), LSB-first. `None` if the stream is exhausted.
    pub fn read(&mut self, n: u32) -> Option<u32> {
        if n == 0 {
            return Some(0);
        }
        if n > 32 || self.bits_left() < n as usize {
            return None;
        }
        let mut out: u32 = 0;
        for i in 0..n {
            let byte = self.data[self.bit >> 3];
            let b = (byte >> (self.bit & 7)) & 1;
            out |= (b as u32) << i;
            self.bit += 1;
        }
        Some(out)
    }

    /// Single bit as a bool.
    pub fn read_bit(&mut self) -> Option<bool> {
        self.read(1).map(|v| v != 0)
    }

    /// Read 64 bits as two halves (only used for the rare wide field).
    pub fn read_u64(&mut self, n: u32) -> Option<u64> {
        if n <= 32 {
            return self.read(n).map(|v| v as u64);
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

/// Vorbis' packed float format (not IEEE754).
pub fn float32_unpack(x: u32) -> f32 {
    let mantissa = (x & 0x001f_ffff) as f64;
    let sign = x & 0x8000_0000;
    let exponent = ((x & 0x7fe0_0000) >> 21) as i32;
    let m = if sign != 0 { -mantissa } else { mantissa };
    (m * (2.0f64).powi(exponent - 788)) as f32
}

/// Largest integer `r` with `r^dim <= entries` (spec's `lookup1_values`).
pub fn lookup1_values(entries: u32, dim: u32) -> u32 {
    if dim == 0 {
        return 0;
    }
    let mut r: u32 = 0;
    loop {
        let next = r + 1;
        // next^dim, saturating — entries fits in 24 bits so this stays small.
        let mut p: u64 = 1;
        let mut overflow = false;
        for _ in 0..dim {
            p = p.saturating_mul(next as u64);
            if p > entries as u64 {
                overflow = true;
                break;
            }
        }
        if overflow || p > entries as u64 {
            return r;
        }
        r = next;
        if r > entries {
            return entries;
        }
    }
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
    fn exhaustion_returns_none_not_panic() {
        let data = [0xFFu8];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read(8), Some(0xFF));
        assert_eq!(r.read(1), None);
        assert_eq!(r.read(32), None);
        // Reading zero bits is always fine.
        assert_eq!(r.read(0), Some(0));
    }

    #[test]
    fn oversized_read_refused() {
        let data = [0u8; 16];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read(33), None);
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
    }

    #[test]
    fn lookup1_values_is_the_integer_root() {
        // 3^2 = 9 <= 10, 4^2 = 16 > 10
        assert_eq!(lookup1_values(10, 2), 3);
        assert_eq!(lookup1_values(9, 2), 3);
        assert_eq!(lookup1_values(8, 3), 2);
        assert_eq!(lookup1_values(27, 3), 3);
        assert_eq!(lookup1_values(1, 1), 1);
    }

    #[test]
    fn float32_unpack_round_numbers() {
        // mantissa 1, exponent 788 => 1.0
        let v = float32_unpack(788 << 21 | 1);
        assert!((v - 1.0).abs() < 1e-6, "{v}");
        // negative sign bit
        let n = float32_unpack(0x8000_0000 | (788 << 21) | 1);
        assert!((n + 1.0).abs() < 1e-6, "{n}");
    }
}

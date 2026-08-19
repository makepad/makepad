//! MSB-first bit reading over a byte slice.
//!
//! Layer III reads two different streams this way: the frame's side info,
//! which is bounded by the frame, and the *main data*, which lives in the bit
//! reservoir and legitimately straddles frames. Both are attacker-controlled,
//! so reads past the end return zeros and set [`BitReader::overran`] rather
//! than panicking: the standard itself lets the final `count1` quadruple run
//! a few bits past `part2_3_length`, and a decoder that treats that as fatal
//! rejects legal files.

/// Bit cursor over a byte slice. Never panics; never allocates.
pub struct BitReader<'a> {
    data: &'a [u8],
    /// Cursor, in bits from the start of `data`.
    pos: usize,
    overran: bool,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0, overran: false }
    }

    /// A reader positioned `bit_pos` bits into `data`.
    pub fn at(data: &'a [u8], bit_pos: usize) -> Self {
        let mut r = Self::new(data);
        r.seek(bit_pos);
        r
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn len_bits(&self) -> usize {
        self.data.len() * 8
    }

    /// True once any read has gone past the end of the slice.
    pub fn overran(&self) -> bool {
        self.overran
    }

    pub fn seek(&mut self, bit_pos: usize) {
        self.pos = bit_pos;
        if bit_pos > self.len_bits() {
            self.overran = true;
        }
    }

    pub fn skip(&mut self, bits: usize) {
        self.seek(self.pos.saturating_add(bits));
    }

    /// Next bit, zero past the end.
    #[inline]
    pub fn read_bit(&mut self) -> u32 {
        let byte = self.pos >> 3;
        let bit = match self.data.get(byte) {
            Some(b) => (b >> (7 - (self.pos & 7))) as u32 & 1,
            None => {
                self.overran = true;
                0
            }
        };
        self.pos += 1;
        bit
    }

    /// Next `n` bits (`n <= 25`), MSB first, zero-filled past the end.
    #[inline]
    pub fn read(&mut self, n: u32) -> u32 {
        let v = self.peek(n);
        self.pos += n as usize;
        if self.pos > self.len_bits() {
            self.overran = true;
        }
        v
    }

    /// Like [`Self::read`] but without moving the cursor.
    #[inline]
    pub fn peek(&self, n: u32) -> u32 {
        debug_assert!(n <= 25);
        if n == 0 {
            return 0;
        }
        let byte = self.pos >> 3;
        let shift = self.pos & 7;
        // Four bytes always cover 25 bits from any bit offset.
        let mut acc = 0u32;
        for i in 0..4 {
            acc = (acc << 8) | *self.data.get(byte + i).unwrap_or(&0) as u32;
        }
        (acc << shift) >> (32 - n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_msb_first_across_byte_boundaries() {
        let data = [0b1010_1100, 0b0011_0101, 0xff, 0x00];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read(3), 0b101);
        assert_eq!(r.read(1), 0);
        assert_eq!(r.read(8), 0b1100_0011);
        assert_eq!(r.pos(), 12);
        assert_eq!(r.peek(4), 0b0101);
        assert_eq!(r.read(4), 0b0101);
        assert_eq!(r.read(8), 0xff);
        assert!(!r.overran());
    }

    #[test]
    fn reads_past_the_end_are_zero_and_flagged() {
        let data = [0xff, 0xff];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read(16), 0xffff);
        assert!(!r.overran());
        assert_eq!(r.read(8), 0);
        assert!(r.overran());
        assert_eq!(r.read_bit(), 0);
    }

    #[test]
    fn empty_slice_is_total() {
        let mut r = BitReader::new(&[]);
        assert_eq!(r.read(25), 0);
        assert_eq!(r.read_bit(), 0);
        assert!(r.overran());
        assert_eq!(r.len_bits(), 0);
    }

    #[test]
    fn seek_and_skip_track_the_cursor() {
        let data = [0b1111_0000, 0b1010_1010];
        let mut r = BitReader::new(&data);
        r.skip(4);
        assert_eq!(r.read(4), 0);
        r.seek(8);
        assert_eq!(r.read(4), 0b1010);
        assert_eq!(r.pos(), 12);
    }

    #[test]
    fn peek_of_25_bits_spans_four_bytes() {
        // 25 bits from bit offset 7 end exactly at bit 31, so four bytes are
        // the most the window ever needs.
        let data = [0x12, 0x34, 0x56, 0x78, 0x9a];
        let r = BitReader::at(&data, 7);
        let expect = (u32::from_be_bytes([0x12, 0x34, 0x56, 0x78]) << 7) >> (32 - 25);
        assert_eq!(r.peek(25), expect);
    }
}

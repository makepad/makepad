//! MSB-first bit writer: MPEG audio fields and Huffman codewords are both
//! stored most significant bit first.

#[derive(Default)]
pub struct BitWriter {
    pub bytes: Vec<u8>,
    acc: u64,
    fill: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append the low `n` bits of `value` (n <= 32).
    #[inline]
    pub fn put(&mut self, value: u32, n: u32) {
        if n == 0 {
            return;
        }
        let v = value as u64 & ((1u64 << n) - 1);
        self.acc = (self.acc << n) | v;
        self.fill += n;
        while self.fill >= 8 {
            self.fill -= 8;
            self.bytes.push((self.acc >> self.fill) as u8);
        }
        self.acc &= (1u64 << self.fill) - 1;
    }

    pub fn len_bits(&self) -> usize {
        self.bytes.len() * 8 + self.fill as usize
    }

    /// Pad with zero bits to a byte boundary and return the bytes.
    pub fn finish(mut self) -> Vec<u8> {
        if self.fill > 0 {
            let pad = 8 - self.fill;
            self.put(0, pad);
        }
        self.bytes
    }
}

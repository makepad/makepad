//! torch's CPU generator engine, shared by the model families that must
//! reproduce a reference's seeded draws bit-exactly (TripoSplat's Sobol
//! anchors, YuE2's acoustic noise).

// torch's CPU MT19937 (ATen/core/MT19937RNGEngine.h)

const MT_N: usize = 624;
const MT_M: usize = 397;
const MT_MATRIX_A: u32 = 0x9908_b0df;
const MT_UMASK: u32 = 0x8000_0000;
const MT_LMASK: u32 = 0x7fff_ffff;

pub struct Mt19937 {
    state: [u32; MT_N],
    left: usize,
    next: usize,
}

impl Mt19937 {
    /// `mt19937::init_with_uint32` — the canonical `init_genrand`.
    pub fn new(seed: u64) -> Self {
        let mut state = [0u32; MT_N];
        state[0] = (seed & 0xffff_ffff) as u32;
        for j in 1..MT_N {
            let prev = state[j - 1];
            state[j] = 1_812_433_253u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(j as u32);
        }
        Self {
            state,
            left: 1,
            next: 0,
        }
    }

    fn mix_bits(u: u32, v: u32) -> u32 {
        (u & MT_UMASK) | (v & MT_LMASK)
    }

    fn twist(u: u32, v: u32) -> u32 {
        (Self::mix_bits(u, v) >> 1) ^ if v & 1 != 0 { MT_MATRIX_A } else { 0 }
    }

    fn next_state(&mut self) {
        self.left = MT_N;
        self.next = 0;
        for j in 0..(MT_N - MT_M) {
            self.state[j] = self.state[j + MT_M] ^ Self::twist(self.state[j], self.state[j + 1]);
        }
        for j in (MT_N - MT_M)..(MT_N - 1) {
            self.state[j] =
                self.state[j + MT_M - MT_N] ^ Self::twist(self.state[j], self.state[j + 1]);
        }
        self.state[MT_N - 1] =
            self.state[MT_M - 1] ^ Self::twist(self.state[MT_N - 1], self.state[0]);
    }

    /// One tempered 32-bit output.
    pub fn next_u32(&mut self) -> u32 {
        self.left -= 1;
        if self.left == 0 {
            self.next_state();
        }
        let mut y = self.state[self.next];
        self.next += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^= y >> 18;
        y
    }

    /// `torch.randint(high, ...)` for `high < 2^32`: `random() % high`.
    pub fn randint_below(&mut self, high: u32) -> u32 {
        self.next_u32() % high
    }
}


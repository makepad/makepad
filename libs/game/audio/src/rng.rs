//! Device-local randomness.
//!
//! Sound selection varies (so the same hit never repeats twice), and that
//! variation must never come from the world RNG: audio is Local tier, and a
//! footstep that advanced the simulation would desync two devices in a room
//! over which shoe hit the ground first. This is a separate stream the sim
//! cannot observe.

/// xorshift64*, seeded per device.
#[derive(Clone, Debug)]
pub struct LocalRng {
    state: u64,
}

impl Default for LocalRng {
    fn default() -> Self {
        Self::new(0x2545_F491_4F6C_DD1D)
    }
}

impl LocalRng {
    pub fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed },
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / 16_777_216.0)
    }

    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }

    /// Uniform integer in [0, n). Zero for n == 0.
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_gives_the_same_stream() {
        let mut a = LocalRng::new(7);
        let mut b = LocalRng::new(7);
        for _ in 0..500 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn zero_seed_does_not_stick_at_zero() {
        let mut r = LocalRng::new(0);
        assert_ne!(r.next_u64(), 0);
    }

    #[test]
    fn floats_stay_in_range() {
        let mut r = LocalRng::new(11);
        for _ in 0..2000 {
            let v = r.next_f32();
            assert!((0.0..1.0).contains(&v));
            let s = r.range(-3.0, 5.0);
            assert!((-3.0..=5.0).contains(&s));
        }
    }

    #[test]
    fn below_respects_its_bound_and_zero() {
        let mut r = LocalRng::new(3);
        assert_eq!(r.below(0), 0);
        for _ in 0..1000 {
            assert!(r.below(7) < 7);
        }
    }
}

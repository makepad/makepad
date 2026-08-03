//! Deterministic local RNG for generation.
//!
//! Generation must NEVER touch the world rng — that would make mesh
//! detail an input to the simulation and break replay. This is a private
//! stream seeded by the caller, exactly like the particle and bake systems.

/// xorshift64* — same algorithm the sim uses, deliberately a separate
/// instance so the two streams can never interleave.
#[derive(Clone, Debug)]
pub struct GenRng {
    state: u64,
}

impl GenRng {
    pub fn new(seed: u64) -> Self {
        // Zero is a fixed point for xorshift; splitmix the seed first so
        // that seed 0, 1, 2... produce well-separated streams rather than
        // near-identical ones.
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Self {
            state: if z == 0 { 0x9E37_79B9_7F4A_7C15 } else { z },
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

    /// Uniform in [0, 1) with 24 bits of mantissa — exact f32 arithmetic.
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / 16_777_216.0)
    }

    /// Uniform in [lo, hi).
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }

    /// Symmetric jitter in [-amount, amount].
    pub fn jitter(&mut self, amount: f32) -> f32 {
        self.range(-amount, amount)
    }

    /// Index in [0, n); returns 0 for n == 0 rather than panicking, because
    /// callers pick from preset tables that a bad knob could leave empty.
    pub fn index(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as usize
    }

    pub fn chance(&mut self, p: f32) -> bool {
        self.f32() < p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_stream() {
        let (mut a, mut b) = (GenRng::new(7), GenRng::new(7));
        for _ in 0..256 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn nearby_seeds_diverge_immediately() {
        // Without the splitmix, xorshift seeds 1 and 2 produce visibly
        // similar first draws, which would make seed+1 trees look alike.
        let (mut a, mut b) = (GenRng::new(1), GenRng::new(2));
        let (x, y) = (a.f32(), b.f32());
        assert!((x - y).abs() > 0.05, "seeds 1 and 2 too close: {x} {y}");
    }

    #[test]
    fn zero_seed_is_not_a_fixed_point() {
        let mut r = GenRng::new(0);
        assert_ne!(r.next_u64(), 0);
        for _ in 0..64 {
            let v = r.f32();
            assert!((0.0..1.0).contains(&v));
        }
    }

    #[test]
    fn index_handles_empty() {
        let mut r = GenRng::new(3);
        assert_eq!(r.index(0), 0);
        for _ in 0..32 {
            assert!(r.index(5) < 5);
        }
    }
}

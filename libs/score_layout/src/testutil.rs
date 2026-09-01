//! Deterministic pseudo-random numbers for property tests.
//!
//! A tiny 64-bit linear congruential generator (Knuth's MMIX multiplier)
//! with a fixed seed per test: reproducible on every platform, no external
//! dependency, no global state.

/// Deterministic LCG for test input generation.
pub struct Lcg(u64);

impl Lcg {
    /// Create with a fixed seed. Every test picks its own seed so failures
    /// reproduce exactly.
    pub fn new(seed: u64) -> Lcg {
        Lcg(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(1))
    }

    /// Next raw 64-bit value.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        // The high bits of an LCG are the well-distributed ones.
        self.0.rotate_left(21) ^ (self.0 >> 17)
    }

    /// Uniform `f64` in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

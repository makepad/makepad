//! A seeded sequence of random draws, the same in documents, scenes and
//! kernels: `rand_seq(seed, i)` is the i-th draw (from 0) in 0..1. Draw i
//! depends only on (seed, i), so a table of draws is a loop or one call
//! (`rand_seq(seed, 0, n)` in documents), and a kernel element takes its own.
//!
//! The generator is a 32-bit counter hashed by a mixing function (the
//! mulberry32 construction): draw i of seed s is the (i + 1)-th output of
//! that generator started at s, so a sequence written as "the next draw" of
//! it is `rand_seq(s, 0)`, `rand_seq(s, 1)`, …. Documents get all 32 bits;
//! kernels (the prelude's `rand_seq`) the top 24, which is the same draw
//! rounded down to f32's precision.

/// Draw `i` (from 0) of the sequence `seed`, in 0..1. Seed and index are
/// taken modulo 2^32 after truncation.
pub fn rand_seq(seed: f64, i: f64) -> f64 {
    let seed = seed.trunc() as i64 as u32;
    let step = (i.trunc() as i64).wrapping_add(1) as u32;
    let mut t = seed.wrapping_add(step.wrapping_mul(0x6D2B_79F5));
    t = (t ^ (t >> 15)).wrapping_mul(t | 1);
    t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
    (t ^ (t >> 14)) as f64 / 4_294_967_296.0
}

#[cfg(test)]
mod tests {
    use super::rand_seq;

    #[test]
    fn draws_are_the_counter_generators_outputs_in_order() {
        // The generator stepped by hand (the 1st, 2nd, 3rd and 1001st outputs).
        let cases: [(f64, [f64; 4]); 4] = [
            (0.0, [0.26642920868471265, 0.0003297457005828619, 0.2232720274478197, 0.7286102059297264]),
            (42.0, [0.6011037519201636, 0.44829055899754167, 0.8524657934904099, 0.3865283413324505]),
            (1337.0, [0.1844118325971067, 0.18998925131745636, 0.8104719922412187, 0.4615255924873054]),
            (4294967295.0, [0.8964226141106337, 0.189478256739676, 0.7156526781618595, 0.3559702388010919]),
        ];
        for (seed, want) in cases {
            let got = [rand_seq(seed, 0.0), rand_seq(seed, 1.0), rand_seq(seed, 2.0), rand_seq(seed, 1000.0)];
            assert_eq!(got, want, "seed {seed}");
        }
        assert_eq!(rand_seq(-1.0, 0.0), rand_seq(4294967295.0, 0.0), "seeds wrap modulo 2^32");
    }
}

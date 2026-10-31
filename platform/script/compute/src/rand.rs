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

/// A stateless hash of 1-4 numbers to 0..1: FNV-1a over `floor(x * 1000003)`
/// with murmur finalisers (integers and fractions alike), the same in
/// documents, scenes and the reference films' JavaScript `hash(...xs)`.
pub fn hashn(xs: &[f64]) -> f64 {
    let mut h: u32 = 2166136261;
    for &x in xs {
        h ^= ((x * 1000003.0).floor() as i64) as i32 as u32;
        h = h.wrapping_mul(16777619);
        h ^= h >> 13;
        h = h.wrapping_mul(0x5bd1e995);
        h ^= h >> 15;
    }
    h as f64 / 4294967296.0
}

/// Value noise of the plane in -1..1: `hashn(ix, iy, seed)` at the integer
/// lattice, blended by the quintic fade, in the order
/// `lerp(lerp(a, b, fx), lerp(c, d, fx), fy) * 2 - 1`.
pub fn vnoise(x: f64, y: f64, seed: f64) -> f64 {
    let (ix, iy) = (x.floor(), y.floor());
    let fade = |t: f64| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let (fx, fy) = (fade(x - ix), fade(y - iy));
    let a = hashn(&[ix, iy, seed]);
    let b = hashn(&[ix + 1.0, iy, seed]);
    let c = hashn(&[ix, iy + 1.0, seed]);
    let d = hashn(&[ix + 1.0, iy + 1.0, seed]);
    lerp(lerp(a, b, fx), lerp(c, d, fx), fy) * 2.0 - 1.0
}

#[cfg(test)]
mod tests {
    use super::{hashn, rand_seq, vnoise};

    #[test]
    fn hashn_and_vnoise_are_the_reference_hash_and_noise() {
        assert_eq!(hashn(&[3.0, 7.0]), 0.04503778298385441);
        assert_eq!(hashn(&[0.5]), 0.0866293553262949);
        assert_eq!(hashn(&[12.0, 1.0, 2.0, 3.0]), 0.1231098179705441);
        assert_eq!(hashn(&[-4.25, 9.0]), 0.1556121150497347);
        assert_eq!(vnoise(0.3, 0.7, 1.0), 0.36239171777528245);
        assert_eq!(vnoise(12.25, -3.5, 2.0), 0.004047073656238354);
    }

    #[test]
    fn hashn_is_the_fnv_murmur_hash() {
        // Values of the JS reference (x|0 of floor(x * 1000003), Math.imul).
        assert_eq!(hashn(&[]), 2166136261.0 / 4294967296.0);
        for (xs, want) in [(vec![3.0, 1.0], 0.08475184044800699), (vec![1.0, 3.0], 0.9324939236976206), (vec![0.5, 7.0], 0.4266202412545681), (vec![17.3, -2.1, 5.0], 0.6753230004105717), (vec![123456.0, 31.0], 0.5473707553464919)] {
            assert_eq!(hashn(&xs), want, "{xs:?}");
        }
    }

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

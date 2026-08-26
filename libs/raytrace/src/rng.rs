//! The sampler, on the CPU: bit-for-bit the same integer recipe the shader
//! runs (`gpu.rs`), so the CPU reference integrator and the GPU draw the
//! SAME random numbers for the same pixel/sample/dimension.
//!
//! Low discrepancy, not white noise: an Owen-scrambled Sobol (0,2)-sequence
//! (dimensions 0 and 1 — both closed-form, no direction-number table), padded
//! across dimensions by a per-dimension-pair hash scramble and decorrelated
//! across pixels by a per-pixel index shuffle. This is Burley's "practical
//! hash-based Owen scrambling" construction; it roughly halves time-to-clean
//! against independent random numbers because every pixel's samples stay
//! stratified in every 2D projection the integrator cares about (lens,
//! BSDF, light).

/// `lowbias32` (Wellons): a good 32-bit integer hash in 4 multiplies.
#[inline]
pub fn hash_u32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846ca68b);
    x ^= x >> 16;
    x
}

/// Combine two words into one hash.
#[inline]
pub fn hash2(a: u32, b: u32) -> u32 {
    hash_u32(a ^ hash_u32(b.wrapping_add(0x9e3779b9)))
}

/// Bit reversal (van der Corput / Sobol dimension 0).
#[inline]
pub fn reverse_bits(mut x: u32) -> u32 {
    x = ((x & 0x55555555) << 1) | ((x >> 1) & 0x55555555);
    x = ((x & 0x33333333) << 2) | ((x >> 2) & 0x33333333);
    x = ((x & 0x0f0f0f0f) << 4) | ((x >> 4) & 0x0f0f0f0f);
    x = ((x & 0x00ff00ff) << 8) | ((x >> 8) & 0x00ff00ff);
    (x << 16) | (x >> 16)
}

/// Nested uniform (Owen) scramble in the Laine–Karras form, hash-seeded.
#[inline]
pub fn owen_scramble(mut x: u32, seed: u32) -> u32 {
    x = reverse_bits(x);
    x ^= x.wrapping_mul(0x3d20adea);
    x = x.wrapping_add(seed);
    x = x.wrapping_mul((seed >> 16) | 1);
    x ^= x.wrapping_mul(0x05526c56);
    x ^= x.wrapping_mul(0x53a22864);
    reverse_bits(x)
}

/// Sobol dimension 1 (primitive polynomial x+1, all direction numbers 1):
/// `v_k = v_{k-1} ^ (v_{k-1} >> 1)` starting at `1<<31`.
#[inline]
pub fn sobol_dim1(mut index: u32) -> u32 {
    let mut v: u32 = 0x8000_0000;
    let mut r: u32 = 0;
    while index != 0 {
        if index & 1 != 0 {
            r ^= v;
        }
        index >>= 1;
        v ^= v >> 1;
    }
    r
}

/// u32 → [0,1) with 24 significant bits (what the shader's `f32` can carry).
#[inline]
pub fn u32_to_unit(x: u32) -> f32 {
    (x >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// One 2D low-discrepancy point: sample `index` of the pixel whose seed is
/// `pixel_seed`, for dimension pair `pair` (0 = lens/jitter, then 3 pairs per
/// bounce: bsdf, light, lobe/rr).
#[inline]
pub fn sobol_2d(index: u32, pixel_seed: u32, pair: u32) -> (f32, f32) {
    let idx = owen_scramble(index, hash2(pixel_seed, 0x51ed_270b));
    let sx = hash2(pixel_seed, pair.wrapping_mul(2));
    let sy = hash2(pixel_seed, pair.wrapping_mul(2).wrapping_add(1));
    let x = owen_scramble(reverse_bits(idx), sx);
    let y = owen_scramble(sobol_dim1(idx), sy);
    (u32_to_unit(x), u32_to_unit(y))
}

/// The per-pixel seed for a frame-seeded render (`seed` changes per render
/// so two renders of one scene differ; within a render it is constant so a
/// pixel's samples walk one sequence).
#[inline]
pub fn pixel_seed(px: u32, py: u32, seed: u32) -> u32 {
    hash2(hash2(px, py.wrapping_mul(0x9e37)), seed)
}

/// Dimension-pair allocation shared conceptually with the shader. Pair 0 is
/// primary jitter, pair 1 is the lens. Each bounce owns five fresh pairs:
/// lobe/Fresnel, BSDF, environment, emissive barycentrics, light-select/RR.
#[inline]
pub fn bounce_pairs(bounce: u32) -> [u32; 5] {
    let base = 2 + bounce * 5;
    [base, base + 1, base + 2, base + 3, base + 4]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_is_an_involution() {
        for x in [0u32, 1, 2, 0x8000_0000, 0xdead_beef, u32::MAX] {
            assert_eq!(reverse_bits(reverse_bits(x)), x);
        }
        assert_eq!(reverse_bits(1), 0x8000_0000);
    }

    #[test]
    fn owen_scramble_is_a_bijection_on_low_bits() {
        // A nested uniform scramble: output bit i depends only on input
        // bits at or above i. Inputs that differ only in their low 12 bits
        // therefore map to outputs distinct in their low 12 bits.
        let seed = 0x1234_5678;
        let mut seen = std::collections::HashSet::new();
        for i in 0..4096u32 {
            assert!(seen.insert(owen_scramble(i, seed) & 0xfff));
        }
        // And on the Sobol side (high bits vary) the high bits stay distinct.
        let mut seen = std::collections::HashSet::new();
        for i in 0..4096u32 {
            assert!(seen.insert(owen_scramble(reverse_bits(i), seed) >> 20));
        }
    }

    #[test]
    fn first_sobol_points_are_stratified() {
        // A (0,2)-sequence: every aligned 1/4 × 1/4 cell of the unit square
        // holds exactly one of the first 16 points of the UNscrambled
        // sequence, and Owen scrambling preserves that property.
        for &seed in &[0u32, 7, 0xabcdef] {
            let mut cells = [[0u32; 4]; 4];
            for i in 0..16 {
                let (x, y) = sobol_2d(i, seed, 3);
                cells[(x * 4.0) as usize][(y * 4.0) as usize] += 1;
            }
            for row in cells {
                for c in row {
                    assert_eq!(c, 1, "seed {seed}: {cells:?}");
                }
            }
        }
    }

    #[test]
    fn dims_are_decorrelated_between_pixels() {
        let a: Vec<f32> = (0..64).map(|i| sobol_2d(i, pixel_seed(3, 4, 1), 1).0).collect();
        let b: Vec<f32> = (0..64).map(|i| sobol_2d(i, pixel_seed(4, 4, 1), 1).0).collect();
        let corr: f32 = a.iter().zip(&b).map(|(x, y)| (x - 0.5) * (y - 0.5)).sum::<f32>() / 64.0;
        assert!(corr.abs() < 0.02, "corr {corr}");
    }

    #[test]
    fn dimension_allocation_never_reuses_a_pair() {
        let mut seen = std::collections::HashSet::from([0u32, 1u32]);
        for b in 0..16 {
            for pair in bounce_pairs(b) {
                assert!(seen.insert(pair), "dimension pair {pair} reused at bounce {b}");
            }
        }
        assert_eq!(seen.len(), 82);
    }
}

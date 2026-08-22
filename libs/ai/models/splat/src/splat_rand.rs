//! Quasi-random and pseudo-random helpers TripoSplat's weights were trained
//! against.
//!
//! Two of these are part of the MODEL, not the sampling policy, so they are
//! reproduced bit-exactly rather than replaced:
//!
//! * `SobolEngine(dimension=3, scramble=True, seed=123).draw(8192)` anchors
//!   the flow model's 8192 latent tokens in the unit cube
//!   (`LatentSeqMMFlowModel.pos_pe`). It is not a checkpoint tensor — the
//!   reference regenerates it from the seed at construction — so the whole
//!   engine is transcribed here: torch's MT19937 (`init_genrand` +
//!   `genrand_int32`), `torch.randint(2, ...)` (= `random() % 2`, serial in
//!   memory order), `_sobol_engine_initialize_state_`,
//!   `_sobol_engine_scramble_` and `_sobol_engine_draw`.
//! * `hammersley_sequence(3, i, 32)` builds
//!   `ElasticGaussianFixedlenDecoder.points_offset_perturbation`, the fixed
//!   32-point jitter each anchor's gaussians are spread over.
//!
//! The stochastic parts are NOT reproduced bit-exactly, and cannot be: the
//! reference draws the octree's systematic-resampling offsets and the final
//! sub-voxel jitter from torch's *default* CUDA generator, never from the
//! seeded one, so its own decode is not seed-reproducible. Seeded draws here
//! come from [`SplatRng`] (SplitMix64 + Box-Muller), which makes the whole
//! pipeline deterministic for a given seed — a stronger guarantee than the
//! reference gives, just not the same mapping from seed to output.

use crate::splat::GS_PER_POINT;

// ---------------------------------------------------------------------------
// torch's CPU MT19937 (ATen/core/MT19937RNGEngine.h)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// torch.quasirandom.SobolEngine
// ---------------------------------------------------------------------------

const SOBOL_MAXBIT: usize = 30;
/// Joe-Kuo primitive polynomials for the first three dimensions. Dimension 0
/// is the degenerate duplicate torch prepends (`poly = concat([[1], ...])`).
const SOBOL_POLY: [i64; 3] = [1, 3, 7];
/// Initial direction numbers, `MAXDEG` wide. Row 0 is never read (torch fills
/// dimension 0 with all-ones explicitly); rows 1 and 2 are the `m_i` of the
/// `new-joe-kuo-6` file's first two data lines (`d=2: 1`, `d=3: 1 3`).
const SOBOL_INIT: [[i64; 2]; 3] = [[1, 0], [1, 0], [1, 3]];

fn bit_length(mut n: i64) -> usize {
    let mut bits = 0;
    while n > 0 {
        n /= 2;
        bits += 1;
    }
    bits
}

/// Zero-indexed position of the rightmost zero bit (= count of trailing ones).
fn rightmost_zero(mut n: i64) -> usize {
    let mut i = 0;
    while n % 2 == 1 {
        n /= 2;
        i += 1;
    }
    i
}

/// `_sobol_engine_initialize_state_` for `dimension <= 3`.
fn sobol_initialize_state(dimension: usize) -> Vec<[i64; SOBOL_MAXBIT]> {
    let mut state = vec![[0i64; SOBOL_MAXBIT]; dimension];
    for m in 0..SOBOL_MAXBIT {
        state[0][m] = 1;
    }
    for d in 1..dimension {
        let p = SOBOL_POLY[d];
        let m = bit_length(p) - 1;
        for i in 0..m {
            state[d][i] = SOBOL_INIT[d][i];
        }
        for j in m..SOBOL_MAXBIT {
            let mut newv = state[d][j - m];
            let mut pow2: i64 = 1;
            for k in 0..m {
                pow2 <<= 1;
                if (p >> (m - 1 - k)) & 1 != 0 {
                    newv ^= pow2 * state[d][j - k - 1];
                }
            }
            state[d][j] = newv;
        }
    }
    // sobolstate * [2^(MAXBIT-1), ..., 2, 1]
    for row in state.iter_mut() {
        for (j, value) in row.iter_mut().enumerate() {
            *value *= 1i64 << (SOBOL_MAXBIT - 1 - j);
        }
    }
    state
}

/// `SobolEngine._scramble` + `_sobol_engine_scramble_`.
fn sobol_scramble(state: &mut [[i64; SOBOL_MAXBIT]], dimension: usize, seed: u64) -> Vec<i64> {
    let mut rng = Mt19937::new(seed);
    // shift_ints = randint(2, (dimension, MAXBIT)); shift = shift_ints @ 2^k
    let mut shift = vec![0i64; dimension];
    for d in 0..dimension {
        let mut acc = 0i64;
        for b in 0..SOBOL_MAXBIT {
            let bit = rng.randint_below(2) as i64;
            acc += bit * (1i64 << b);
        }
        shift[d] = acc;
    }
    // ltm = randint(2, (dimension, MAXBIT, MAXBIT)).tril(); every entry is
    // drawn, the upper triangle is then discarded. `_sobol_engine_scramble_`
    // forces the diagonal to 1 before packing each row into an integer with
    // weights 2^(MAXBIT-1-k).
    let mut ltm_dots = vec![[0i64; SOBOL_MAXBIT]; dimension];
    for d in 0..dimension {
        for p in 0..SOBOL_MAXBIT {
            let mut packed = 0i64;
            for k in 0..SOBOL_MAXBIT {
                let drawn = rng.randint_below(2) as i64;
                let bit = if k > p {
                    0
                } else if k == p {
                    1
                } else {
                    drawn
                };
                packed += bit * (1i64 << (SOBOL_MAXBIT - 1 - k));
            }
            ltm_dots[d][p] = packed;
        }
    }
    for d in 0..dimension {
        for j in 0..SOBOL_MAXBIT {
            let vdj = state[d][j];
            let mut l: i64 = 1;
            let mut t2: i64 = 0;
            for p in (0..SOBOL_MAXBIT).rev() {
                let lsmdp = ltm_dots[d][p];
                let mut t1 = 0i64;
                for k in 0..SOBOL_MAXBIT {
                    t1 += ((lsmdp >> k) & 1) * ((vdj >> k) & 1);
                }
                t1 %= 2;
                t2 += t1 * l;
                l <<= 1;
            }
            state[d][j] = t2;
        }
    }
    shift
}

/// Scrambled Sobol points, `(n, dimension)` row-major, matching
/// `SobolEngine(dimension, scramble=True, seed).draw(n)`.
pub fn sobol_draw(dimension: usize, n: usize, seed: u64) -> Vec<f32> {
    assert!((1..=3).contains(&dimension), "only dims 1..=3 are tabulated");
    let mut state = sobol_initialize_state(dimension);
    let shift = sobol_scramble(&mut state, dimension, seed);
    let recip = 1.0f32 / (1i64 << SOBOL_MAXBIT) as f32;
    let mut out = vec![0.0f32; n * dimension];
    if n == 0 {
        return out;
    }
    // Row 0 is `_first_point = quasi / 2**MAXBIT` with `quasi = shift`.
    for d in 0..dimension {
        out[d] = shift[d] as f32 / (1i64 << SOBOL_MAXBIT) as f32;
    }
    let mut quasi = shift;
    let mut num_generated: i64 = 0;
    for i in 1..n {
        let l = rightmost_zero(num_generated);
        for d in 0..dimension {
            quasi[d] ^= state[d][l];
            out[i * dimension + d] = quasi[d] as f32 * recip;
        }
        num_generated += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Hammersley / Halton (model.py)
// ---------------------------------------------------------------------------

const PRIMES: [usize; 16] = [2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53];

pub fn radical_inverse(base: usize, mut n: usize) -> f64 {
    let mut value = 0.0f64;
    let inv_base = 1.0 / base as f64;
    let mut inv_base_n = inv_base;
    while n > 0 {
        let digit = n % base;
        value += digit as f64 * inv_base_n;
        n /= base;
        inv_base_n *= inv_base;
    }
    value
}

/// `hammersley_sequence(dim, n, num_samples)` = `[n/num_samples] +
/// halton_sequence(dim-1, n)`.
pub fn hammersley_sequence(dim: usize, n: usize, num_samples: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(dim);
    out.push(n as f64 / num_samples as f64);
    for d in 0..dim - 1 {
        out.push(radical_inverse(PRIMES[d], n));
    }
    out
}

/// `points_offset_perturbation`: `atanh((hammersley*2 - 1) / perturbe_size)`
/// for the 32 gaussians of one anchor, stored `(32, 3)` row-major.
pub fn gaussian_offset_perturbation(perturbe_size: f32) -> Vec<f32> {
    let mut out = vec![0.0f32; GS_PER_POINT * 3];
    for i in 0..GS_PER_POINT {
        let point = hammersley_sequence(3, i, GS_PER_POINT);
        for (axis, value) in point.iter().enumerate() {
            // torch.tensor(...).float() rounds to f32 BEFORE atanh.
            let x = *value as f32;
            out[i * 3 + axis] = ((x * 2.0 - 1.0) / perturbe_size).atanh();
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Seeded pseudo-random draws (SplitMix64 + Box-Muller)
// ---------------------------------------------------------------------------

/// Deterministic uniform/normal source for the seeded parts of the pipeline.
#[derive(Clone)]
pub struct SplatRng {
    state: u64,
}

impl SplatRng {
    pub fn new(seed: u64) -> Self {
        // Mix the seed so adjacent seeds do not produce correlated streams.
        Self {
            state: seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0xda3e_39cb_94b9_5bdb,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)` with 53 bits of mantissa, returned as f32.
    pub fn uniform(&mut self) -> f32 {
        ((self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)) as f32
    }

    /// Standard normal (Box-Muller; the second variate is kept implicit so a
    /// draw count always advances the stream by two words).
    pub fn normal(&mut self) -> f32 {
        let mut u1 = self.uniform();
        if u1 <= f32::MIN_POSITIVE {
            u1 = f32::MIN_POSITIVE;
        }
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
    }

    pub fn fill_normal(&mut self, out: &mut [f32]) {
        for value in out.iter_mut() {
            *value = self.normal();
        }
    }

    pub fn normal_vec(&mut self, len: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; len];
        self.fill_normal(&mut out);
        out
    }
}

// ---------------------------------------------------------------------------
// sample_probs (model.py) — systematic resampling
// ---------------------------------------------------------------------------

/// One row of `sample_probs(probs, counts, algo="systematic")`: distributes
/// `count` samples over `probs` by drawing `u0 ~ U[0, 1/count)` and taking the
/// inverse CDF at `u0 + k/count`. Rows summing to zero become uniform, exactly
/// like the reference's `zero_mask` branch.
pub fn sample_probs_row(probs: &[f32], count: usize, rng: &mut SplatRng, out: &mut [i64]) {
    let p = probs.len();
    out.iter_mut().for_each(|value| *value = 0);
    if count == 0 || p == 0 {
        return;
    }
    let mut norm = vec![0.0f32; p];
    let mut sum = 0.0f32;
    for (dst, src) in norm.iter_mut().zip(probs) {
        *dst = src.max(0.0);
        sum += *dst;
    }
    if sum == 0.0 {
        let uniform = 1.0 / p as f32;
        norm.iter_mut().for_each(|value| *value = uniform);
    } else {
        // The reference divides by `row_sums.clamp_min_(1)`, so rows that sum
        // to less than 1 are NOT renormalized upward.
        let divisor = sum.max(1.0);
        norm.iter_mut().for_each(|value| *value /= divisor);
    }
    let mut cdf = vec![0.0f32; p];
    let mut acc = 0.0f32;
    for (dst, value) in cdf.iter_mut().zip(&norm) {
        acc += *value;
        *dst = acc.min(1.0 - 1e-12);
    }
    let u0 = rng.uniform() / count as f32;
    for k in 0..count {
        let u = (u0 + k as f32 / count as f32).min(1.0 - 1e-12);
        // searchsorted(cdf, u) with the default 'left' side.
        let mut lo = 0usize;
        let mut hi = p;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if cdf[mid] < u {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        out[lo.min(p - 1)] += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mt19937_matches_the_canonical_init_genrand_vector() {
        // The reference MT19937 output for init_genrand(5489), which is what
        // torch's `mt19937::init_with_uint32` implements.
        let mut rng = Mt19937::new(5489);
        let expect = [
            3_499_211_612u32,
            581_869_302,
            3_890_346_734,
            3_586_334_585,
            545_404_204,
            4_161_255_391,
            3_922_919_429,
            949_333_985,
        ];
        for want in expect {
            assert_eq!(rng.next_u32(), want);
        }
    }

    #[test]
    fn unscrambled_sobol_matches_the_documented_prefix() {
        // torch's own docstring: SobolEngine(dimension=5).draw(3) starts
        // 0,0,0 / .5,.5,.5 / .75,.25,.25 — the first three columns are what
        // the 3-dimensional engine produces.
        let mut state = sobol_initialize_state(3);
        let shift = vec![0i64; 3];
        let recip = 1.0f32 / (1i64 << SOBOL_MAXBIT) as f32;
        let mut quasi = shift;
        let mut rows = vec![[0.0f32; 3]];
        let mut num_generated: i64 = 0;
        for _ in 0..2 {
            let l = rightmost_zero(num_generated);
            let mut row = [0.0f32; 3];
            for d in 0..3 {
                quasi[d] ^= state[d][l];
                row[d] = quasi[d] as f32 * recip;
            }
            rows.push(row);
            num_generated += 1;
        }
        assert_eq!(rows[0], [0.0, 0.0, 0.0]);
        assert_eq!(rows[1], [0.5, 0.5, 0.5]);
        assert_eq!(rows[2], [0.75, 0.25, 0.25]);
        // sanity: the direction numbers are the Bratley-Fox recurrence.
        assert_eq!(state[1][0] >> (SOBOL_MAXBIT - 1), 1);
        assert_eq!(state[1][1] >> (SOBOL_MAXBIT - 2), 3);
        assert_eq!(state[2][1] >> (SOBOL_MAXBIT - 2), 3);
    }

    #[test]
    fn scrambled_sobol_is_stratified_and_deterministic() {
        let points = sobol_draw(3, 4096, super::super::splat::FLOW_SOBOL_SEED);
        assert_eq!(points.len(), 4096 * 3);
        assert!(points.iter().all(|v| (0.0..1.0).contains(v)));
        // A scrambled Sobol net of 4096 points is balanced over any 8^3
        // dyadic partition: exactly 8 points per cell.
        let mut cells = vec![0usize; 512];
        for point in points.chunks_exact(3) {
            let ix = (point[0] * 8.0) as usize;
            let iy = (point[1] * 8.0) as usize;
            let iz = (point[2] * 8.0) as usize;
            cells[(ix.min(7) * 8 + iy.min(7)) * 8 + iz.min(7)] += 1;
        }
        assert!(cells.iter().all(|c| *c == 8), "not a balanced (0, m, 3)-net");
        // Deterministic across calls, and the seed actually matters.
        assert_eq!(points, sobol_draw(3, 4096, super::super::splat::FLOW_SOBOL_SEED));
        assert_ne!(points, sobol_draw(3, 4096, 124));
    }

    #[test]
    fn hammersley_first_points() {
        // n = 0 -> [0, 0, 0]; n = 1 -> [1/32, 1/2, 1/3].
        assert_eq!(hammersley_sequence(3, 0, 32), vec![0.0, 0.0, 0.0]);
        let p1 = hammersley_sequence(3, 1, 32);
        assert!((p1[0] - 1.0 / 32.0).abs() < 1e-15);
        assert!((p1[1] - 0.5).abs() < 1e-15);
        assert!((p1[2] - 1.0 / 3.0).abs() < 1e-15);
        // radical_inverse(2, 5) = 0b101 reversed = .101 = 5/8
        assert!((radical_inverse(2, 5) - 0.625).abs() < 1e-15);
    }

    #[test]
    fn offset_perturbation_is_finite_and_centered() {
        let perturbation = gaussian_offset_perturbation(GS_PERTURB_SIZE_TEST);
        assert_eq!(perturbation.len(), 32 * 3);
        assert!(perturbation.iter().all(|v| v.is_finite()));
        // hammersley[0] = (0,0,0) -> atanh(-1/1.5) for every axis.
        let want = (-1.0f32 / 1.5).atanh();
        for axis in 0..3 {
            assert!((perturbation[axis] - want).abs() < 1e-6);
        }
    }
    const GS_PERTURB_SIZE_TEST: f32 = 1.5;

    #[test]
    fn systematic_resampling_preserves_the_count_and_follows_the_mass() {
        let mut rng = SplatRng::new(7);
        let mut out = vec![0i64; 4];
        sample_probs_row(&[0.7, 0.1, 0.1, 0.1], 10, &mut rng, &mut out);
        assert_eq!(out.iter().sum::<i64>(), 10);
        // Systematic sampling of a 0.7 bin with 10 draws lands 7 there.
        assert_eq!(out[0], 7);
        // A degenerate (all-zero) row becomes uniform, not empty.
        let mut out = vec![0i64; 4];
        sample_probs_row(&[0.0, 0.0, 0.0, 0.0], 8, &mut rng, &mut out);
        assert_eq!(out, vec![2, 2, 2, 2]);
        // Zero draws leave the row empty.
        let mut out = vec![0i64; 4];
        sample_probs_row(&[1.0, 0.0, 0.0, 0.0], 0, &mut rng, &mut out);
        assert_eq!(out.iter().sum::<i64>(), 0);
    }

    #[test]
    fn seeded_rng_is_deterministic_and_roughly_normal() {
        let a = SplatRng::new(42).normal_vec(4096);
        let b = SplatRng::new(42).normal_vec(4096);
        assert_eq!(a, b);
        assert_ne!(a, SplatRng::new(43).normal_vec(4096));
        let mean = a.iter().sum::<f32>() / a.len() as f32;
        let var = a.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / a.len() as f32;
        assert!(mean.abs() < 0.06, "mean {mean}");
        assert!((var - 1.0).abs() < 0.1, "var {var}");
    }
}

//! Seeded, deterministic CPU noise shared by the HDRI layers: the 2D cloud
//! layer now, and volumetric clouds, gobos and imperfections in phase 1b.
//!
//! Everything here is a pure function of its inputs. The same coordinates
//! and seed give the same bits on every platform, because only integer
//! hashing, floor and f32 arithmetic are used (no transcendental functions).
//! That is what keeps a preset's clouds identical from one run, and one
//! machine, to the next.
//!
//! The patterns follow the repository's own: the multiply-xorshift integer
//! hash of `libs/rtsmap/src/rng.rs`, and the quintic fade and gradient table
//! of `libs/model/src/surface_pattern.rs`, rewritten for f32, three
//! dimensions and a seed.

use makepad_draw::*;

/// Most octaves an fbm sums. Past 12 the finest octave is below f32
/// resolution for typical coordinates and only costs time.
const FBM_MAX_OCTAVES: u32 = 12;

/// An orthonormal matrix (unit rows, mutually perpendicular) that turns the
/// 3D fbm's octaves against each other, so their lattices never line up.
const OCTAVE_TURN: [[f32; 3]; 3] = [[0.0, 0.8, 0.6], [-0.8, 0.36, -0.48], [-0.6, -0.48, 0.64]];

/// A 32-bit integer hash with full avalanche ("lowbias32" from Chris Wellons'
/// hash prospector): every input bit flips about half the output bits, so
/// neighbouring lattice cells get unrelated values. It is a bijection, so
/// distinct inputs never collide.
pub fn hash_u32(x: u32) -> u32 {
    let mut x = x;
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// The seed folded through the hash once. The golden-ratio xor keeps seed 0
/// from starting the chain at hash(0) = 0.
fn seed_key(seed: u32) -> u32 {
    hash_u32(seed ^ 0x9e37_79b9)
}

/// Chains one lattice coordinate into a running hash. Chaining (rather than
/// xoring all the coordinates together) keeps (1, 2) and (2, 1) unrelated.
fn mix_in(h: u32, coordinate: i32) -> u32 {
    hash_u32(h ^ coordinate as u32)
}

/// The top 24 bits as a float in [0, 1): exact in f32, and never 1.
fn unit(h: u32) -> f32 {
    (h >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// Perlin's quintic fade 6t^5 - 15t^4 + 10t^3: zero first and second
/// derivatives at the lattice, so the noise shows no creases along cell edges.
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Lattice cell and position inside it. `floor`, not truncation, so negative
/// coordinates land in the right cell.
fn cell(v: f32) -> (i32, f32) {
    let floor = v.floor();
    (floor as i32, v - floor)
}

/// A uniform value in [0, 1) for a lattice point and seed.
pub fn hash01(ix: i32, iy: i32, iz: i32, seed: u32) -> f32 {
    unit(mix_in(mix_in(mix_in(seed_key(seed), ix), iy), iz))
}

/// 2D value noise in [0, 1]: hashed lattice values blended with the quintic
/// fade. At integer points it equals `hash01(ix, iy, 0, seed)`.
pub fn value2(x: f32, y: f32, seed: u32) -> f32 {
    let (ix, fx) = cell(x);
    let (iy, fy) = cell(y);
    let (tx, ty) = (fade(fx), fade(fy));
    let s = seed_key(seed);
    // The x step of the chain is shared by the two corners in each column.
    let (hx0, hx1) = (mix_in(s, ix), mix_in(s, ix.wrapping_add(1)));
    let corner = |hx: u32, j: i32| unit(mix_in(mix_in(hx, j), 0));
    let iy1 = iy.wrapping_add(1);
    let bottom = mix_f32(corner(hx0, iy), corner(hx1, iy), tx);
    let top = mix_f32(corner(hx0, iy1), corner(hx1, iy1), tx);
    mix_f32(bottom, top, ty)
}

/// 3D value noise in [0, 1]. At integer points it equals `hash01(ix, iy, iz, seed)`.
pub fn value3(x: f32, y: f32, z: f32, seed: u32) -> f32 {
    let (ix, fx) = cell(x);
    let (iy, fy) = cell(y);
    let (iz, fz) = cell(z);
    let (tx, ty, tz) = (fade(fx), fade(fy), fade(fz));
    let s = seed_key(seed);
    let (ix1, iy1, iz1) = (ix.wrapping_add(1), iy.wrapping_add(1), iz.wrapping_add(1));
    let (hx0, hx1) = (mix_in(s, ix), mix_in(s, ix1));
    // x-then-y prefixes of the chain, each shared by the two z corners.
    let (h00, h10, h01, h11) = (mix_in(hx0, iy), mix_in(hx1, iy), mix_in(hx0, iy1), mix_in(hx1, iy1));
    let corner = |hxy: u32, k: i32| unit(mix_in(hxy, k));
    let near = mix_f32(
        mix_f32(corner(h00, iz), corner(h10, iz), tx),
        mix_f32(corner(h01, iz), corner(h11, iz), tx),
        ty,
    );
    let far = mix_f32(
        mix_f32(corner(h00, iz1), corner(h10, iz1), tx),
        mix_f32(corner(h01, iz1), corner(h11, iz1), tx),
        ty,
    );
    mix_f32(near, far, tz)
}

/// Dot product of the offset (x, y, z) with one of Perlin's 12 cube-edge
/// gradients (improved noise, 2002), picked by the hash's top four bits. The
/// last four codes repeat edges so 16 codes cover the 12 edges evenly enough.
fn grad3(h: u32, x: f32, y: f32, z: f32) -> f32 {
    match h >> 28 {
        0 => x + y,
        1 => -x + y,
        2 => x - y,
        3 => -x - y,
        4 => x + z,
        5 => -x + z,
        6 => x - z,
        7 => -x - z,
        8 => y + z,
        9 => -y + z,
        10 => y - z,
        11 => -y - z,
        12 => x + y,
        13 => -x + y,
        14 => -y + z,
        _ => -y - z,
    }
}

/// 3D gradient (Perlin) noise in [-1, 1], zero at every lattice point.
pub fn perlin3(x: f32, y: f32, z: f32, seed: u32) -> f32 {
    let (ix, fx) = cell(x);
    let (iy, fy) = cell(y);
    let (iz, fz) = cell(z);
    let (u, v, w) = (fade(fx), fade(fy), fade(fz));
    let s = seed_key(seed);
    let g = |dx: i32, dy: i32, dz: i32| {
        let h = mix_in(mix_in(mix_in(s, ix.wrapping_add(dx)), iy.wrapping_add(dy)), iz.wrapping_add(dz));
        grad3(h, fx - dx as f32, fy - dy as f32, fz - dz as f32)
    };
    let near = mix_f32(mix_f32(g(0, 0, 0), g(1, 0, 0), u), mix_f32(g(0, 1, 0), g(1, 1, 0), u), v);
    let far = mix_f32(mix_f32(g(0, 0, 1), g(1, 0, 1), u), mix_f32(g(0, 1, 1), g(1, 1, 1), u), v);
    // This gradient set can peak a little above 1 in rare spots; the clamp
    // makes the documented range hold for fbm remaps downstream.
    mix_f32(near, far, w).clamp(-1.0, 1.0)
}

/// 3D Worley (cellular) noise: the distance to the nearest of one hashed
/// feature point per cell (F1), capped at 1.
pub fn worley3(x: f32, y: f32, z: f32, seed: u32) -> f32 {
    let (ix, fx) = cell(x);
    let (iy, fy) = cell(y);
    let (iz, fz) = cell(z);
    let s = seed_key(seed);
    let mut nearest = f32::MAX;
    // Feature points two cells away are more than 1 away and the result is
    // capped at 1, so searching the 3x3x3 block around our cell is exact.
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let h = mix_in(mix_in(mix_in(s, ix.wrapping_add(dx)), iy.wrapping_add(dy)), iz.wrapping_add(dz));
                // Three independent coordinates from the one cell hash.
                let px = dx as f32 + unit(h) - fx;
                let py = dy as f32 + unit(hash_u32(h ^ 0x68e3_1da4)) - fy;
                let pz = dz as f32 + unit(hash_u32(h ^ 0xb529_7a4d)) - fz;
                nearest = nearest.min(px * px + py * py + pz * pz);
            }
        }
    }
    nearest.sqrt().min(1.0)
}

/// Each octave gets its own seed, so octaves are decorrelated and not
/// just scaled copies of one another.
fn octave_seed(seed: u32, octave: u32) -> u32 {
    seed.wrapping_add(octave.wrapping_mul(0x9e37_79b9))
}

/// 2D value-noise fbm in [0, 1]: lacunarity 2, gain 0.5, each octave turned
/// about 37 degrees and shifted. `octaves` is clamped to 1..=12.
pub fn fbm2(x: f32, y: f32, octaves: u32, seed: u32) -> f32 {
    let octaves = octaves.clamp(1, FBM_MAX_OCTAVES);
    let (mut px, mut py) = (x, y);
    let (mut amplitude, mut sum, mut total) = (1.0f32, 0.0f32, 0.0f32);
    for octave in 0..octaves {
        sum += amplitude * value2(px, py, octave_seed(seed, octave));
        total += amplitude;
        amplitude *= 0.5;
        // Turn by a 3-4-5 triangle's angle, double the frequency and shift,
        // so no two octaves share lattice lines (they would show as a grid).
        let (rx, ry) = (0.8 * px - 0.6 * py, 0.6 * px + 0.8 * py);
        px = rx * 2.0 + 17.13;
        py = ry * 2.0 + 5.71;
    }
    (sum / total).clamp(0.0, 1.0)
}

/// 3D value-noise fbm in [0, 1], as `fbm2` with an orthonormal turn per octave.
pub fn fbm3(x: f32, y: f32, z: f32, octaves: u32, seed: u32) -> f32 {
    let octaves = octaves.clamp(1, FBM_MAX_OCTAVES);
    let mut p = [x, y, z];
    let (mut amplitude, mut sum, mut total) = (1.0f32, 0.0f32, 0.0f32);
    for octave in 0..octaves {
        sum += amplitude * value3(p[0], p[1], p[2], octave_seed(seed, octave));
        total += amplitude;
        amplitude *= 0.5;
        let m = &OCTAVE_TURN;
        let q = [
            m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2],
            m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2],
            m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2],
        ];
        p = [q[0] * 2.0 + 17.13, q[1] * 2.0 + 5.71, q[2] * 2.0 + 11.37];
    }
    (sum / total).clamp(0.0, 1.0)
}

/// 1D value noise in [0, 1] on the integer lattice with the quintic fade, and its derivative: at an integer it is
/// the lattice's hash and its slope is 0. Plan hdri-7 (the aurora's folds, rays and border).
pub fn value1_d(x: f32, seed: u32) -> (f32, f32) {
    let (ix, fx) = cell(x);
    let s = seed_key(seed);
    let (a, b) = (unit(mix_in(s, ix)), unit(mix_in(s, ix.wrapping_add(1))));
    // d fade / d t = 30 t^2 (t - 1)^2
    let dt = 30.0 * fx * fx * (fx - 1.0) * (fx - 1.0);
    (mix_f32(a, b, fade(fx)), (b - a) * dt)
}

/// fbm of `value1_d` (lacunarity 2, gain 0.5, normalised to [0, 1]) and its derivative, with `octaves` held to
/// 1..=12 and fractional: a fraction f of a last octave moves the sum of the whole octaves f of the way to the sum
/// with it, so the finest octave fades in from its mean and the noise is continuous in `octaves` (a texel's band
/// limit, `octaves_for`). At a whole number it is the plain sum.
pub fn fbm1_d(x: f32, octaves: f32, seed: u32) -> (f32, f32) {
    let octaves = if octaves.is_nan() { 1.0 } else { octaves.clamp(1.0, FBM_MAX_OCTAVES as f32) };
    let whole = octaves.floor() as u32;
    let part = octaves - whole as f32;
    let count = if part > 0.0 { whole + 1 } else { whole };
    let (mut px, mut freq) = (x, 1.0f32);
    let (mut amplitude, mut sum, mut slope, mut total) = (1.0f32, 0.0f32, 0.0f32, 0.0f32);
    let (mut sum_n, mut slope_n, mut total_n) = (0.0f32, 0.0f32, 1.0f32);
    for octave in 0..count {
        if octave == whole {
            (sum_n, slope_n, total_n) = (sum, slope, total);
        }
        let (v, d) = value1_d(px, octave_seed(seed, octave));
        sum += amplitude * v;
        slope += amplitude * d * freq;
        total += amplitude;
        amplitude *= 0.5;
        px = px * 2.0 + 17.13;
        freq *= 2.0;
    }
    if part > 0.0 {
        let (a, da) = (sum_n / total_n, slope_n / total_n);
        let (b, db) = (sum / total, slope / total);
        (a + (b - a) * part, da + (db - da) * part)
    } else {
        (sum / total, slope / total)
    }
}

/// `fbm1_d`'s value on the scale of `full` octaves (a whole number), as `fbm3_frac_full` is fbm3_frac's: every octave
/// keeps the weight it has in the fbm of `full` octaves, and the finer ones a coarse lookup leaves out count as their
/// mean (one half), so what a texel's band limit leaves of a texture keeps its local means. Between two whole counts it
/// is their straight line; at `full` or more (texel 0), or a NaN count, it is `fbm1_d`'s value at `full`, bit for bit.
/// Plan hdri-7 Task 4c (the aurora's rays: with `fbm1_d` a 1024 and a 2048 export weighed their first octave
/// differently).
pub fn fbm1_frac_full(x: f32, octaves: f32, full: f32, seed: u32) -> f32 {
    let full = if full.is_nan() { 1.0 } else { full.clamp(1.0, FBM_MAX_OCTAVES as f32).floor() };
    if !(octaves < full) {
        return fbm1_d(x, full, seed).0;
    }
    let octaves = octaves.max(1.0);
    let whole = octaves.floor() as u32;
    let part = octaves - whole as f32;
    let count = if part > 0.0 { whole + 1 } else { whole };
    let (mut px, mut amplitude, mut sum, mut total) = (x, 1.0f32, 0.0f32, 0.0f32);
    let (mut sum_n, mut total_n) = (0.0f32, 0.0f32);
    for octave in 0..count {
        if octave == whole {
            (sum_n, total_n) = (sum, total);
        }
        sum += amplitude * value1_d(px, octave_seed(seed, octave)).0;
        total += amplitude;
        amplitude *= 0.5;
        px = px * 2.0 + 17.13;
    }
    let total_full = 2.0 * (1.0 - 0.5f32.powi(full as i32));
    // each sum less its mean, so the octaves left out count as one half
    let (a, b) = (sum_n - 0.5 * total_n, sum - 0.5 * total);
    let centred = if part > 0.0 { a + (b - a) * part } else { b };
    0.5 + centred / total_full
}

/// `fbm3` with fractional octaves (as `fbm1_d`): at a whole number of octaves it is `fbm3`, bit for bit (the same
/// sums in the same order); between, the last octave fades in by its fraction. Plan hdri-7 (the Milky Way's dust and
/// clouds, the nebulas' sprites).
pub fn fbm3_frac(x: f32, y: f32, z: f32, octaves: f32, seed: u32) -> f32 {
    let (sum_n, total_n, sum, total, part) = fbm3_sums(x, y, z, octaves, seed);
    if part > 0.0 {
        let (a, b) = (sum_n / total_n, sum / total);
        (a + (b - a) * part).clamp(0.0, 1.0)
    } else {
        (sum / total).clamp(0.0, 1.0)
    }
}

/// `fbm3_frac` of `octaves` on the scale of `full` octaves (a whole number; plan hdri-7 D10): every octave keeps the
/// weight it has in the fbm of `full` octaves, and the finer ones a coarse lookup leaves out count as their mean (one
/// half). `fbm3_frac` normalises by the octaves it sums, so a coarse lookup weighs its first octave more (the Milky
/// Way's dust: 0.72 at a 512 bake, 1.8 octaves, against 0.59 at 1024, 2.8) and a texture's contrast would change with
/// the width. Between two whole counts it is their straight line, as `fbm3_frac` is; at `full` or more (texel 0), or a
/// NaN count, it is `fbm3_frac` at `full`, bit for bit.
pub fn fbm3_frac_full(x: f32, y: f32, z: f32, octaves: f32, full: f32, seed: u32) -> f32 {
    let full = if full.is_nan() { 1.0 } else { full.clamp(1.0, FBM_MAX_OCTAVES as f32).floor() };
    if !(octaves < full) {
        return fbm3_frac(x, y, z, full, seed);
    }
    let (sum_n, total_n, sum, total, part) = fbm3_sums(x, y, z, octaves, seed);
    let total_full = 2.0 * (1.0 - 0.5f32.powi(full as i32));
    // each sum less its mean, so the octaves left out count as one half
    let (a, b) = (sum_n - 0.5 * total_n, sum - 0.5 * total);
    let centred = if part > 0.0 { a + (b - a) * part } else { b };
    (0.5 + centred / total_full).clamp(0.0, 1.0)
}

/// The sums behind `fbm3_frac`: the whole octaves' sum and amplitude total, the same with the next octave (the whole
/// octaves' alone when there is no fraction), and that octave's fraction.
fn fbm3_sums(x: f32, y: f32, z: f32, octaves: f32, seed: u32) -> (f32, f32, f32, f32, f32) {
    let octaves = if octaves.is_nan() { 1.0 } else { octaves.clamp(1.0, FBM_MAX_OCTAVES as f32) };
    let whole = octaves.floor() as u32;
    let part = octaves - whole as f32;
    let count = if part > 0.0 { whole + 1 } else { whole };
    let mut p = [x, y, z];
    let (mut amplitude, mut sum, mut total) = (1.0f32, 0.0f32, 0.0f32);
    let (mut sum_n, mut total_n) = (0.0f32, 1.0f32);
    for octave in 0..count {
        if octave == whole {
            (sum_n, total_n) = (sum, total);
        }
        sum += amplitude * value3(p[0], p[1], p[2], octave_seed(seed, octave));
        total += amplitude;
        amplitude *= 0.5;
        let m = &OCTAVE_TURN;
        let q = [
            m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2],
            m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2],
            m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2],
        ];
        p = [q[0] * 2.0 + 17.13, q[1] * 2.0 + 5.71, q[2] * 2.0 + 11.37];
    }
    (sum_n, total_n, sum, total, part)
}

/// How many octaves of an fbm whose first wavelength is `base_wavelength` keep its finest wavelength at least two
/// texels of `texel` (both in one unit): log2(base / (2 texel)), held to 1..=max (the plan's D10); `max` at texel 0,
/// an exact lookup. log2 is read off the float's exponent and its mantissa as a straight line between powers of two
/// (exact at them, within 0.09 between), so no transcendental function enters the noise's bits.
pub fn octaves_for(base_wavelength: f32, texel: f32, max: f32) -> f32 {
    let max = if max.is_nan() { 1.0 } else { max.clamp(1.0, FBM_MAX_OCTAVES as f32) };
    if !(texel > 0.0) || !(base_wavelength > 0.0) {
        return max;
    }
    let ratio = base_wavelength / (2.0 * texel);
    if ratio.is_infinite() {
        return max;
    }
    if !(ratio >= f32::MIN_POSITIVE) {
        return 1.0;
    }
    let bits = ratio.to_bits();
    let exponent = ((bits >> 23) & 0xff) as i32 - 127;
    let mantissa = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    (exponent as f32 + (mantissa - 1.0)).clamp(1.0, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A spread of points over several cells, negative coordinates included.
    fn points() -> Vec<(f32, f32, f32)> {
        let mut out = Vec::with_capacity(1600);
        for i in 0..40 {
            for j in 0..40 {
                let (i, j) = (i as f32, j as f32);
                out.push((-3.7 + i * 0.173, 2.1 - j * 0.219, 0.37 * i - 0.11 * j));
            }
        }
        out
    }

    #[test]
    fn the_hash_is_pinned_and_collision_free() {
        // Pinned values: changing the hash would silently change every preset's clouds.
        assert_eq!(hash_u32(0), 0);
        assert_eq!(hash_u32(1), 0x6889_90c0);
        assert_eq!(hash_u32(2), 0xd113_2181);
        assert_eq!(hash_u32(0xdead_beef), 0xe628_c683);
        assert_eq!(hash01(0, 0, 0, 1), 0.044_623_494);
        assert_eq!(hash01(-3, 7, 2, 42), 0.947_568_95);
        let mut seen = std::collections::HashSet::new();
        assert!((0..100_000u32).all(|i| seen.insert(hash_u32(i))), "the hash is a bijection");
    }

    #[test]
    fn hash01_is_uniform_enough() {
        let mut bins = [0usize; 10];
        let mut sum = 0.0f64;
        for i in 0..64 {
            for j in 0..64 {
                let h = hash01(i, j, 0, 3);
                assert!((0.0..1.0).contains(&h), "{h}");
                bins[(h * 10.0) as usize] += 1;
                sum += h as f64;
            }
        }
        let mean = sum / 4096.0;
        assert!((mean - 0.5).abs() < 0.02, "mean {mean}");
        assert!(bins.iter().all(|&n| (330..=490).contains(&n)), "{bins:?}");
    }

    #[test]
    fn noise_is_deterministic_and_exact_on_the_lattice() {
        for &(x, y, z) in &points() {
            assert_eq!(value2(x, y, 9).to_bits(), value2(x, y, 9).to_bits());
            assert_eq!(value3(x, y, z, 9).to_bits(), value3(x, y, z, 9).to_bits());
            assert_eq!(perlin3(x, y, z, 9).to_bits(), perlin3(x, y, z, 9).to_bits());
            assert_eq!(worley3(x, y, z, 9).to_bits(), worley3(x, y, z, 9).to_bits());
            assert_eq!(fbm2(x, y, 5, 9).to_bits(), fbm2(x, y, 5, 9).to_bits());
            assert_eq!(fbm3(x, y, z, 5, 9).to_bits(), fbm3(x, y, z, 5, 9).to_bits());
        }
        // Value noise passes through the hashed lattice values exactly...
        assert_eq!(value2(3.0, -2.0, 9), hash01(3, -2, 0, 9));
        assert_eq!(value3(3.0, -2.0, 5.0, 9), hash01(3, -2, 5, 9));
        // ...and gradient noise is zero on the lattice.
        for (x, y, z) in [(0.0, 0.0, 0.0), (1.0, -2.0, 3.0), (-5.0, 4.0, 2.0)] {
            assert_eq!(perlin3(x, y, z, 3), 0.0);
        }
    }

    /// Fixed sample points for the golden values: negative and large
    /// coordinates, seeds from 0 to 0xdeadbeef and octave counts from 1 to the
    /// maximum of 12. (x, y, z, seed, octaves)
    const GOLDEN_POINTS: [(f32, f32, f32, u32, u32); 6] = [
        (-3.7, 2.1, 0.37, 9, 1),
        (0.25, -0.75, 5.5, 1, 2),
        (-0.001, 1000.5, -33.3, 0, 3),
        (12.34, -56.78, 7.7, 0xdead_beef, 5),
        (-17.9, -4.2, -2.5, 42, 8),
        (-123.456, -0.5, -0.5, 7, 12),
    ];

    /// `got` must be `want` bit for bit; a failure shows both as floats.
    #[track_caller]
    fn assert_golden(name: &str, i: usize, got: f32, want: u32) {
        assert_eq!(got.to_bits(), want, "{name} at point {i}: got {got:?}, want {:?}", f32::from_bits(want));
    }

    #[test]
    fn noise_matches_its_golden_values() {
        // Taken from the implementation as written. The purity test above
        // cannot see what these pin: the lattice hashing, the fade, the
        // gradient set, worley's salts and every fbm constant (the 3-4-5 turn,
        // OCTAVE_TURN, the shifts, octave_seed). Changing any of them moves
        // every preset's clouds, so a change here is a deliberate re-bake of
        // them all. The floats in the comments are for the reader.
        let want_value2: [u32; 6] = [
            0x3f11584b, // 0.5677535
            0x3e0402da, // 0.12891713
            0x3efb22ea, // 0.49050075
            0x3f293ad1, // 0.6610537
            0x3f6be785, // 0.92150146
            0x3f542dba, // 0.82882273
        ];
        let want_value3: [u32; 6] = [
            0x3f057fe2, // 0.5214826
            0x3ee7591f, // 0.45185181
            0x3eaf2099, // 0.34204558
            0x3f57e109, // 0.8432775
            0x3f2c7cca, // 0.67377913
            0x3f33bde6, // 0.7021164
        ];
        let want_perlin3: [u32; 6] = [
            0x3da21568, // 0.07914239
            0x3df88a80, // 0.12135792
            0xbeb2383c, // -0.34808528
            0xbeb86342, // -0.36013228
            0x3f271444, // 0.652653
            0x3f060ce2, // 0.5236341
        ];
        let want_worley3: [u32; 6] = [
            0x3f2297eb, // 0.6351306
            0x3f068e63, // 0.52561015
            0x3e8d123b, // 0.2755297
            0x3f426e51, // 0.7594958
            0x3f3c018a, // 0.7343985
            0x3efb1cb7, // 0.49045345
        ];
        let want_fbm2: [u32; 6] = [
            0x3f11584b, // 0.5677535 (one octave is value2 itself)
            0x3e973592, // 0.29533058
            0x3f1021e1, // 0.56301695
            0x3f075707, // 0.5286717
            0x3f3d6b9b, // 0.7399232
            0x3f43d1b8, // 0.7649188
        ];
        let want_fbm3: [u32; 6] = [
            0x3f057fe2, // 0.5214826 (one octave is value3 itself)
            0x3ef091d8, // 0.4698627
            0x3eafa60d, // 0.34306374
            0x3f402cee, // 0.7506856
            0x3f12632e, // 0.57182586
            0x3f18890f, // 0.59584135
        ];
        for (i, &(x, y, z, seed, octaves)) in GOLDEN_POINTS.iter().enumerate() {
            assert_golden("value2", i, value2(x, y, seed), want_value2[i]);
            assert_golden("value3", i, value3(x, y, z, seed), want_value3[i]);
            assert_golden("perlin3", i, perlin3(x, y, z, seed), want_perlin3[i]);
            assert_golden("worley3", i, worley3(x, y, z, seed), want_worley3[i]);
            assert_golden("fbm2", i, fbm2(x, y, octaves, seed), want_fbm2[i]);
            assert_golden("fbm3", i, fbm3(x, y, z, octaves, seed), want_fbm3[i]);
        }
    }

    #[test]
    fn the_gradient_table_and_the_fade_are_pinned() {
        // Offsets 1, 2, 4 make every code's combination of x, y and z a
        // different sum, so this is the whole 16-entry table, repeats included.
        let table: Vec<f32> = (0..16u32).map(|code| grad3(code << 28, 1.0, 2.0, 4.0)).collect();
        assert_eq!(
            table,
            [3.0, 1.0, -1.0, -3.0, 5.0, 3.0, -3.0, -5.0, 6.0, 2.0, -2.0, -6.0, 3.0, 1.0, 2.0, -6.0]
        );
        // The low 28 bits of the hash do not pick a gradient.
        assert_eq!(grad3(0x5fff_ffff, 1.0, 2.0, 4.0), grad3(5 << 28, 1.0, 2.0, 4.0));
        // Exact in f32: the quintic at the quarter points, and its symmetry.
        assert_eq!(
            [fade(0.0), fade(0.25), fade(0.5), fade(0.75), fade(1.0)],
            [0.0, 0.103_515_625, 0.5, 0.896_484_4, 1.0]
        );
    }

    #[test]
    fn the_fbm_statistics_promised_to_the_clouds_hold() {
        // The cloud layer sizes its coverage thresholds from these: the mean
        // and spread of 5-octave fbm over the shared grid of points (seed 7).
        // Measured from the implementation, with a margin for the f64 sums.
        let pts = points();
        let stats = |f: &dyn Fn(f32, f32, f32) -> f32| {
            let v: Vec<f64> = pts.iter().map(|&(x, y, z)| f(x, y, z) as f64).collect();
            let mean = v.iter().sum::<f64>() / v.len() as f64;
            let std = (v.iter().map(|a| (a - mean).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
            (mean, std)
        };
        let (mean2, std2) = stats(&|x, y, _| fbm2(x, y, 5, 7));
        assert!((mean2 - 0.4915).abs() < 0.002 && (std2 - 0.1231).abs() < 0.002, "fbm2 mean {mean2} std {std2}");
        let (mean3, std3) = stats(&|x, y, z| fbm3(x, y, z, 5, 7));
        assert!((mean3 - 0.4925).abs() < 0.002 && (std3 - 0.1194).abs() < 0.002, "fbm3 mean {mean3} std {std3}");
    }

    #[test]
    fn noise_stays_in_its_documented_range() {
        let pts = points();
        let mut perlin_peak = 0.0f32;
        let mut value_sum = 0.0f64;
        let mut fbm = Vec::with_capacity(pts.len());
        for &(x, y, z) in &pts {
            let (v2, v3) = (value2(x, y, 7), value3(x, y, z, 7));
            assert!((0.0..=1.0).contains(&v2) && (0.0..=1.0).contains(&v3), "{v2} {v3}");
            let p = perlin3(x, y, z, 7);
            assert!((-1.0..=1.0).contains(&p), "{p}");
            perlin_peak = perlin_peak.max(p.abs());
            let (f2, f3) = (fbm2(x, y, 5, 7), fbm3(x, y, z, 5, 7));
            assert!((0.0..=1.0).contains(&f2) && (0.0..=1.0).contains(&f3), "{f2} {f3}");
            value_sum += v2 as f64;
            fbm.push(f2 as f64);
        }
        let (mut near, mut far) = (1.0f32, 0.0f32);
        for &(x, y, z) in &pts[..400] {
            let w = worley3(x, y, z, 7);
            assert!((0.0..=1.0).contains(&w), "{w}");
            near = near.min(w);
            far = far.max(w);
        }
        let mean = value_sum / pts.len() as f64;
        assert!((0.4..0.6).contains(&mean), "value noise mean {mean}");
        assert!(perlin_peak > 0.5, "gradient noise is not flat: {perlin_peak}");
        assert!(near < 0.3 && far > 0.6, "worley spans near and far: {near}..{far}");
        let fbm_mean = fbm.iter().sum::<f64>() / fbm.len() as f64;
        let fbm_std = (fbm.iter().map(|v| (v - fbm_mean).powi(2)).sum::<f64>() / fbm.len() as f64).sqrt();
        assert!((0.4..0.6).contains(&fbm_mean) && fbm_std > 0.05, "fbm mean {fbm_mean} std {fbm_std}");
        // Octave counts outside 1..=12 are clamped, not a panic or a NaN.
        assert!(fbm2(0.3, 0.7, 0, 1).is_finite() && fbm2(0.3, 0.7, 1000, 1).is_finite());
        assert!(fbm3(0.3, 0.7, 0.1, 0, 1).is_finite());
    }

    #[test]
    fn noise_is_continuous_across_cell_seams() {
        let e = 1.0e-4;
        let fields: [(&str, &dyn Fn(f32) -> f32); 6] = [
            ("value2", &|x| value2(x, 0.37, 5)),
            ("value3", &|x| value3(0.21, x, 0.63, 5)),
            ("perlin3", &|x| perlin3(0.41, 0.77, x, 5)),
            ("worley3", &|x| worley3(x, 0.3, 0.6, 5)),
            ("fbm2", &|x| fbm2(x, 0.37, 5, 5)),
            ("fbm3", &|x| fbm3(0.2, x, 0.5, 5, 5)),
        ];
        for (name, f) in fields.iter() {
            for k in [-2.0f32, -1.0, 0.0, 1.0, 2.0, 7.0] {
                let jump = (f(k - e) - f(k + e)).abs();
                assert!(jump < 2.0e-3, "{name} jumps by {jump} at {k}");
            }
            // A fine walk across five cells never steps further than the slope allows.
            let mut prev = f(-2.5);
            for i in 1..5000 {
                let v = f(-2.5 + i as f32 * 1.0e-3);
                assert!((v - prev).abs() < 5.0e-3, "{name} steps by {} at step {i}", (v - prev).abs());
                prev = v;
            }
        }
    }

    #[test]
    fn a_new_seed_gives_a_new_pattern() {
        let pts = &points()[..400];
        let differs = |f: &dyn Fn(f32, f32, f32, u32) -> f32| {
            pts.iter().filter(|&&(x, y, z)| f(x, y, z, 1) != f(x, y, z, 2)).count()
        };
        assert!(differs(&|x, y, _, s| value2(x, y, s)) > 390);
        assert!(differs(&|x, y, z, s| value3(x, y, z, s)) > 390);
        assert!(differs(&|x, y, z, s| perlin3(x, y, z, s)) > 390);
        assert!(differs(&|x, y, z, s| worley3(x, y, z, s)) > 390);
        assert!(differs(&|x, y, _, s| fbm2(x, y, 4, s)) > 390);
        assert!(differs(&|x, y, z, s| fbm3(x, y, z, 4, s)) > 390);
        assert_ne!(hash01(1, 2, 3, 1), hash01(1, 2, 3, 2));
    }

    /// Plan hdri-7: the 1D value noise is pinned like the others (the aurora's folds and rays read it); on the lattice it
    /// is the lattice's hash and flat. The bits were computed in float32 from the module's own hash.
    #[test]
    fn value1_matches_its_golden_values() {
        let golden: [(f32, u32, u32, Option<u32>); 4] = [
            (3.0, 7, 1_051_720_604, None),
            (3.5, 7, 1_051_619_681, Some(3_157_838_760)),
            (-2.5, 42, 1_052_681_427, Some(3_172_548_106)),
            (0.25, 0, 1_059_913_176, Some(3_177_932_037)),
        ];
        for (x, seed, v, d) in golden {
            let (got_v, got_d) = value1_d(x, seed);
            assert_eq!(got_v.to_bits(), v, "value1_d({x}, {seed})");
            match d {
                Some(d) => assert_eq!(got_d.to_bits(), d, "value1_d({x}, {seed})'s slope"),
                None => assert_eq!(got_d, 0.0, "flat on the lattice"),
            }
        }
        assert_eq!(value1_d(3.0, 7).0, unit(mix_in(seed_key(7), 3)));
    }

    /// The slope is the derivative (central differences); one octave of fbm1_d is value1_d; a fraction of an octave
    /// fades the last one in, continuously.
    #[test]
    fn fbm1_slopes_are_derivatives_and_octaves_fade_in() {
        for i in 0..200 {
            let x = -7.3 + i as f32 * 0.0731;
            for octaves in [1.0f32, 2.5, 4.0] {
                let (_, d) = fbm1_d(x, octaves, 11);
                let h = 1.0e-3;
                let fd = (fbm1_d(x + h, octaves, 11).0 - fbm1_d(x - h, octaves, 11).0) / (2.0 * h);
                assert!((d - fd).abs() < 0.02 * (1.0 + d.abs()) * 2f32.powf(octaves), "x {x}, {octaves} octaves: {d} vs {fd}");
            }
            let one = fbm1_d(x, 1.0, 11);
            assert_eq!((one.0.to_bits(), one.1.to_bits()), (value1_d(x, 11).0.to_bits(), value1_d(x, 11).1.to_bits()));
            let (a, b, mid) = (fbm1_d(x, 3.0, 11).0, fbm1_d(x, 4.0, 11).0, fbm1_d(x, 3.5, 11).0);
            assert!((mid - (a + (b - a) * 0.5)).abs() < 1.0e-6, "half an octave is half way: {a} {mid} {b}");
            assert!((fbm1_d(x, 3.0001, 11).0 - a).abs() < 1.0e-4, "continuous at a whole octave");
            assert!((0.0..=1.0).contains(&b));
        }
    }

    /// fbm3_frac at a whole number of octaves is fbm3 bit for bit; between, the last octave fades in by its fraction.
    #[test]
    fn fbm3_frac_is_fbm3_at_whole_octaves() {
        for &(x, y, z, seed, octaves) in &GOLDEN_POINTS {
            assert_eq!(fbm3_frac(x, y, z, octaves as f32, seed).to_bits(), fbm3(x, y, z, octaves, seed).to_bits(), "{x} {y} {z} {seed} {octaves}");
        }
        for &(x, y, z) in &points() {
            assert_eq!(fbm3_frac(x, y, z, 5.0, 9).to_bits(), fbm3(x, y, z, 5, 9).to_bits());
            let (a, b, mid) = (fbm3(x, y, z, 3, 9), fbm3(x, y, z, 4, 9), fbm3_frac(x, y, z, 3.25, 9));
            assert!((mid - (a + (b - a) * 0.25)).abs() < 1.0e-6);
            assert!((0.0..=1.0).contains(&mid));
        }
        assert_eq!(fbm3_frac(0.3, 0.4, 0.5, 20.0, 1).to_bits(), fbm3(0.3, 0.4, 0.5, 12, 1).to_bits(), "held to 12 octaves");
        assert_eq!(fbm3_frac(0.3, 0.4, 0.5, f32::NAN, 1).to_bits(), fbm3(0.3, 0.4, 0.5, 1, 1).to_bits(), "NaN is one octave");
    }

    /// fbm3_frac_full keeps every octave at its weight in the fbm of `full` octaves: at a whole number w of them it is
    /// fbm3 of w on full's scale (0.5 + (fbm3 - 0.5) x total(w) / total(full)), between two it is their straight line, and
    /// at `full` or more (texel 0), or a NaN count, it is fbm3_frac at `full` bit for bit.
    #[test]
    fn fbm3_frac_full_keeps_each_octaves_weight() {
        let total = |w: u32| 2.0 * (1.0 - 0.5f32.powi(w as i32));
        for &(x, y, z) in &points() {
            let at = |w: u32| 0.5 + (fbm3(x, y, z, w, 9) - 0.5) * total(w) / total(6);
            for w in 1..6u32 {
                let v = fbm3_frac_full(x, y, z, w as f32, 6.0, 9);
                assert!((v - at(w)).abs() < 1.0e-6, "{w} octaves: {v} against {}", at(w));
                let mid = fbm3_frac_full(x, y, z, w as f32 + 0.25, 6.0, 9);
                let line = at(w) + (if w < 5 { at(w + 1) } else { fbm3(x, y, z, 6, 9) } - at(w)) * 0.25;
                assert!((mid - line).abs() < 1.0e-6, "{w}.25 octaves: {mid} against {line}");
            }
            assert_eq!(fbm3_frac_full(x, y, z, 6.0, 6.0, 9).to_bits(), fbm3(x, y, z, 6, 9).to_bits());
            assert_eq!(fbm3_frac_full(x, y, z, 9.0, 6.0, 9).to_bits(), fbm3(x, y, z, 6, 9).to_bits());
            assert_eq!(fbm3_frac_full(x, y, z, f32::NAN, 2.0, 9).to_bits(), fbm3(x, y, z, 2, 9).to_bits());
            assert!((fbm3_frac_full(x, y, z, 5.9999, 6.0, 9) - fbm3(x, y, z, 6, 9)).abs() < 1.0e-4, "continuous at full");
        }
    }

    /// fbm1_frac_full is fbm1_d's value as fbm3_frac_full is fbm3's: at a whole number w of octaves it is fbm1_d of w on
    /// full's scale, between two it is their straight line, and at `full` or more (texel 0), or a NaN count, it is
    /// fbm1_d's value at `full` bit for bit.
    #[test]
    fn fbm1_frac_full_keeps_each_octaves_weight() {
        let total = |w: u32| 2.0 * (1.0 - 0.5f32.powi(w as i32));
        let plain = |x: f32, w: u32| fbm1_d(x, w as f32, 9).0;
        for &(x, _, _) in &points() {
            let at = |w: u32| 0.5 + (plain(x, w) - 0.5) * total(w) / total(3);
            for w in 1..3u32 {
                let v = fbm1_frac_full(x, w as f32, 3.0, 9);
                assert!((v - at(w)).abs() < 1.0e-6, "{w} octaves: {v} against {}", at(w));
                let mid = fbm1_frac_full(x, w as f32 + 0.25, 3.0, 9);
                let line = at(w) + (if w < 2 { at(w + 1) } else { plain(x, 3) } - at(w)) * 0.25;
                assert!((mid - line).abs() < 1.0e-6, "{w}.25 octaves: {mid} against {line}");
            }
            assert_eq!(fbm1_frac_full(x, 3.0, 3.0, 9).to_bits(), plain(x, 3).to_bits());
            assert_eq!(fbm1_frac_full(x, 5.0, 3.0, 9).to_bits(), plain(x, 3).to_bits());
            assert_eq!(fbm1_frac_full(x, f32::NAN, 2.0, 9).to_bits(), plain(x, 2).to_bits());
        }
    }

    /// The octaves a texel allows (D10): log2(base / (2 texel)), held to 1..=max, the most at texel 0; exact at powers of
    /// two, within 0.09 of log2 between, and never rising as the texel grows.
    #[test]
    fn octaves_follow_the_texel() {
        assert_eq!(octaves_for(1.0, 0.0, 6.0), 6.0);
        assert_eq!(octaves_for(1.0, 1.0 / 16.0, 6.0), 3.0);
        assert_eq!(octaves_for(1.0, 1.0 / 64.0, 6.0), 5.0);
        assert_eq!(octaves_for(1.0, 1.0, 6.0), 1.0, "never fewer than one");
        assert_eq!(octaves_for(1.0, 1.0e-9, 6.0), 6.0, "never more than asked");
        let mut last = f32::MAX;
        for i in 1..400 {
            let texel = i as f32 * 1.0e-3;
            let o = octaves_for(1.0, texel, 12.0);
            assert!(o <= last, "{i}: {o} after {last}");
            assert!((o - (1.0 / (2.0 * texel)).log2().clamp(1.0, 12.0)).abs() < 0.09, "{i}: {o}");
            last = o;
        }
    }
}

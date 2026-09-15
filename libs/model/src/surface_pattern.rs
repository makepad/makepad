//! Deterministic continuous material fields, baked by the document worker.
//! Integral scale repeats exactly over the UV tile. Legacy cell noise keeps
//! its old implementation and encoded tag, preserving existing source hashes.
use super::PatternKind;
use std::f64::consts::{FRAC_1_SQRT_2, TAU};

fn hash(x: i64, y: i64, seed: u64) -> u64 {
    let mut n = seed ^ (x as u64).wrapping_mul(0x9e3779b97f4a7c15)
        ^ (y as u64).wrapping_mul(0xbf58476d1ce4e5b9);
    n = (n ^ (n >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    n = (n ^ (n >> 27)).wrapping_mul(0x94d049bb133111eb);
    n ^ (n >> 31)
}
fn fade(t: f64) -> f64 { t * t * t * (t * (t * 6. - 15.) + 10.) }
fn mix(a: f64, b: f64, t: f64) -> f64 { a + (b - a) * t }
fn period(scale: f64) -> i64 {
    if scale.fract() == 0. { scale as i64 } else { 0 }
}
fn gradient(x: i64, y: i64, dx: f64, dy: f64, periods: [i64; 2], seed: u64) -> f64 {
    let wrap = |n: i64, p: i64| if p > 0 { n.rem_euclid(p) } else { n };
    let (gx, gy) = match hash(wrap(x, periods[0]), wrap(y, periods[1]), seed) & 7 {
        0 => (1., 0.), 1 => (-1., 0.), 2 => (0., 1.), 3 => (0., -1.),
        4 => (FRAC_1_SQRT_2, FRAC_1_SQRT_2), 5 => (-FRAC_1_SQRT_2, FRAC_1_SQRT_2),
        6 => (FRAC_1_SQRT_2, -FRAC_1_SQRT_2), _ => (-FRAC_1_SQRT_2, -FRAC_1_SQRT_2),
    };
    gx * dx + gy * dy
}
fn perlin(x: f64, y: f64, periods: [i64; 2], seed: u64) -> f64 {
    let ix = x.floor() as i64; let iy = y.floor() as i64;
    let x = x - ix as f64; let y = y - iy as f64;
    let a = mix(gradient(ix, iy, x, y, periods, seed), gradient(ix + 1, iy, x - 1., y, periods, seed), fade(x));
    let b = mix(gradient(ix, iy + 1, x, y - 1., periods, seed), gradient(ix + 1, iy + 1, x - 1., y - 1., periods, seed), fade(x));
    mix(a, b, fade(y))
}
fn fbm(x: f64, y: f64, periods: [i64; 2], seed: u64) -> f64 {
    let mut value = 0.; let mut amplitude = 1.; let mut total = 0.;
    for octave in 0..4 {
        let frequency = 1i64 << octave;
        value += amplitude * perlin(x * frequency as f64, y * frequency as f64,
            periods.map(|p| p * frequency), seed.wrapping_add(octave * 0x9e3779b9));
        total += amplitude; amplitude *= 0.5;
    }
    value / total
}
pub(super) fn sample(kind: PatternKind, x: f64, y: f64, scale: [f64; 2], seed: u64) -> f64 {
    let periods = scale.map(period);
    let value = match kind {
        PatternKind::Perlin => 0.5 + perlin(x, y, periods, seed),
        PatternKind::Fbm => 0.5 + fbm(x, y, periods, seed),
        PatternKind::Yarn => {
            let warp = 0.18 * (TAU * y).sin() + 0.08 * fbm(x, y, periods, seed);
            let strand = 0.5 + 0.5 * (TAU * (x + warp)).cos();
            let fibers = 0.5 + 0.5 * (TAU * (x * 12. + y + warp)).cos();
            (0.16 + 0.84 * strand.sqrt()) * (0.84 + 0.16 * fibers)
        }
        _ => unreachable!("legacy patterns retain their original sampler"),
    };
    value.clamp(0., 1.)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn continuous_patterns_repeat_over_integral_uv_tiles() {
        for kind in [PatternKind::Perlin, PatternKind::Fbm, PatternKind::Yarn] {
            for (x, y) in [(0.13, 0.27), (2.83, 3.52), (-0.19, 7.15)] {
                let a = sample(kind, x, y, [8., 6.], 918);
                assert!((0. ..=1.).contains(&a));
                assert!((a - sample(kind, x + 8., y, [8., 6.], 918)).abs() < 1e-12);
                assert!((a - sample(kind, x, y + 6., [8., 6.], 918)).abs() < 1e-12);
            }
        }
    }
    #[test]
    fn perlin_has_continuous_values_and_slopes_at_cell_boundaries() {
        for x in [-1., 0., 1., 7., 8.] {
            let e = 1e-5;
            let mid = perlin(x, 0.31, [8, 6], 42);
            let left = perlin(x - e, 0.31, [8, 6], 42);
            let right = perlin(x + e, 0.31, [8, 6], 42);
            assert!((left - right).abs() < e * 3.);
            assert!(((mid - left) / e - (right - mid) / e).abs() < 1e-4);
        }
    }
    #[test]
    fn smooth_noise_is_seeded_and_varies_within_a_cell() {
        let a = sample(PatternKind::Perlin, 0.2, 0.3, [8., 8.], 42);
        assert_eq!(a, sample(PatternKind::Perlin, 0.2, 0.3, [8., 8.], 42));
        assert_ne!(a, sample(PatternKind::Perlin, 0.3, 0.3, [8., 8.], 42));
        assert_ne!(a, sample(PatternKind::Perlin, 0.2, 0.3, [8., 8.], 1024));
    }
}

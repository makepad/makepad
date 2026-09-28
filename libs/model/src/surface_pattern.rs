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
        PatternKind::Bricks | PatternKind::Tiles | PatternKind::Planks => return masonry(kind, x, y, periods, seed),
        PatternKind::Vents => {
            // A stadium-shaped slot per cell with a soft bevelled lip.
            let (fx, fy) = (x - x.floor(), y - y.floor());
            let (hx, hy) = (0.42, 0.22);
            let dx = ((fx - 0.5).abs() - (hx - hy)).max(0.);
            let d = (dx * dx + (fy - 0.5) * (fy - 0.5)).sqrt() - hy;
            (d / 0.06).clamp(0., 1.)
        }
        PatternKind::Grille => {
            // Hexagonal cells: distance to the nearest cell centre on a
            // staggered lattice, holes inside a hexagonal radius.
            let row = y.floor();
            let fx = x + if (row as i64).rem_euclid(2) == 1 { 0.5 } else { 0. };
            let (cx, cy) = (fx.floor() + 0.5, row + 0.5);
            let (dx, dy) = ((fx - cx).abs(), (y - cy).abs());
            let hex = (dx * 0.866 + dy * 0.5).max(dy);
            ((hex - 0.36) / 0.05).clamp(0., 1.)
        }
        PatternKind::Knurl => {
            let a = (TAU * (x + y)).sin().abs();
            let b = (TAU * (x - y)).sin().abs();
            0.35 + 0.65 * (a.min(b) * 1.6).min(1.)
        }
        _ => unreachable!("legacy patterns retain their original sampler"),
    };
    value.clamp(0., 1.)
}

fn unit_hash(x: i64, y: i64, seed: u64) -> f64 { (hash(x, y, seed) >> 11) as f64 / ((1u64 << 53) - 1) as f64 }
/// Joint-and-unit surfaces. 0 is the joint (mortar, grout, gap); units get a
/// seeded tone in 0.55..1 plus fine surface noise. Unit identities wrap with
/// the integral scale, so tiles repeat exactly (use an even row count for
/// running bond and staggered planks).
fn masonry(kind: PatternKind, x: f64, y: f64, periods: [i64; 2], seed: u64) -> f64 {
    let wrap = |n: i64, p: i64| if p > 0 { n.rem_euclid(p) } else { n };
    let row = y.floor() as i64;
    let (shift, joint_u, joint_v) = match kind {
        PatternKind::Bricks => (if row.rem_euclid(2) == 1 { 0.5 } else { 0. }, 0.06, 0.12),
        PatternKind::Tiles => (0., 0.05, 0.05),
        _ => ((unit_hash(wrap(row, periods[1]), 7, seed) * 4.).floor() * 0.25, 0.01, 0.07),
    };
    let xs = x + shift;
    let (col, fu, fv) = (xs.floor() as i64, xs - xs.floor(), y - y.floor());
    let edge = |f: f64, width: f64| { let d = f.min(1. - f); ((d - width * 0.5) / (width * 0.5)).clamp(0., 1.) };
    let joint = edge(fu, joint_u).min(edge(fv, joint_v));
    if joint <= 0. { return 0.; }
    let id = (wrap(col, periods[0]), wrap(row, periods[1]));
    let tone = 0.55 + 0.45 * unit_hash(id.0, id.1, seed);
    let detail = match kind {
        PatternKind::Planks => {
            let grain = fbm(x * 0.5, y * 6., periods.map(|p| p.max(1)), seed ^ 0x51);
            0.82 + 0.18 * (0.5 + 0.5 * (TAU * (y * 3. + grain * 2.5)).sin())
        }
        _ => 0.9 + 0.1 * (0.5 + fbm(x * 4., y * 4., periods.map(|p| p * 4), seed ^ 0x77)),
    };
    (joint * tone * detail).clamp(0., 1.)
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

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
        PatternKind::Camo => {
            // Two warped fields thresholded into blotches: the first lays the
            // dark shapes, the second the mid-tone patches between them; the
            // rest is the light ground. A narrow ramp keeps the edges crisp
            // without aliasing in the mips.
            let wx = x + 0.45 * fbm(x + 3.1, y, periods, seed ^ 0xca30);
            let wy = y + 0.45 * fbm(x, y - 1.7, periods, seed ^ 0xca31);
            let dark = 0.5 + fbm(wx, wy, periods, seed);
            let mid = 0.5 + fbm(wx + 7.3, wy - 2.9, periods, seed ^ 0xca32);
            let ramp = |v: f64, at: f64| ((v - at) / 0.025).clamp(0., 1.);
            let d = ramp(dark, 0.56);
            let m = ramp(mid, 0.52) * (1. - d);
            // 0 = color_a (dark), 0.5 = the mean, 1 = color_b (light).
            (1. - d) * (1. - m) + 0.5 * m
        }
        PatternKind::Leaves => return foliage(x, y, scale, periods, seed, false),
        PatternKind::Needles => return foliage(x, y, scale, periods, seed, true),
        _ => unreachable!("legacy patterns retain their original sampler"),
    };
    value.clamp(0., 1.)
}

fn unit_hash(x: i64, y: i64, seed: u64) -> f64 { (hash(x, y, seed) >> 11) as f64 / ((1u64 << 53) - 1) as f64 }

/// Foliage card texture: 0 between leaves (the mask cuts it), 0.55..1
/// inside one — per-leaf tone, a darker midrib and a darker base, so the
/// alpha test keeps every leaf whole at every tone. Each cell of the lattice
/// carries three leaves (needles: a twig with a spray) whose centre, angle
/// and size are hashed; neighbours overlap across cell borders and the
/// lattice wraps with the tile. The cluster thins toward the tile's rim so a
/// card reads as a round clump, never as a square.
fn foliage(x: f64, y: f64, scale: [f64; 2], periods: [i64; 2], seed: u64, needles: bool) -> f64 {
    let wrap = |n: i64, p: i64| if p > 0 { n.rem_euclid(p) } else { n };
    let (ix, iy) = (x.floor() as i64, y.floor() as i64);
    let mut best = (-1.0f64, 0.0f64);
    let per_cell = if needles { 4 } else { 3 };
    for oy in -1..=1 {
        for ox in -1..=1 {
            let (cx, cy) = (ix + ox, iy + oy);
            for k in 0..per_cell {
                let s = seed ^ (k as u64).wrapping_mul(0x51_7cc1_b727_220a);
                let (hx, hy) = (wrap(cx, periods[0]), wrap(cy, periods[1]));
                let r = |salt: i64| unit_hash(hx * 7 + salt, hy * 13 - salt, s);
                let (px, py) = (cx as f64 + r(1), cy as f64 + r(2));
                // Cluster density: full in the middle of the tile, none at its rim.
                let (tu, tv) = ((px / scale[0]).rem_euclid(1.0), (py / scale[1]).rem_euclid(1.0));
                let rim = ((tu - 0.5).powi(2) + (tv - 0.5).powi(2)).sqrt();
                let keep = 1.0 - ((rim - 0.28) / 0.2).clamp(0.0, 1.0);
                if r(3) > keep { continue; }
                let angle = r(4) * TAU;
                let (ca, sa) = (angle.cos(), angle.sin());
                let (dx, dy) = (x - px, y - py);
                // Leaf frame: u along the blade, v across it.
                let u = dx * ca + dy * sa;
                let v = -dx * sa + dy * ca;
                let tone = r(5);
                let hit = if needles {
                    // A twig 1.1 cells long with needles every 0.06 along it.
                    let len = 0.55 + 0.25 * r(6);
                    if u.abs() > len { continue; }
                    let along = (u / len).abs();
                    let half = 0.3 * (1.0 - along * 0.6);
                    let twig = v.abs() < 0.018;
                    let phase = (u / 0.06).rem_euclid(1.0);
                    let needle = v.abs() < half && (phase - 0.5).abs() < 0.14 + 0.22 * (v.abs() / half);
                    if !(twig || needle) { continue; }
                    0.55 + 0.45 * (0.35 + 0.65 * tone) * (0.7 + 0.3 * (v.abs() / half.max(1e-3)))
                } else {
                    let len = 0.42 + 0.22 * r(6);
                    let t = u / len;
                    if t.abs() >= 1.0 { continue; }
                    // A pointed lens: widest a little behind the middle.
                    let half = 0.36 * len * (1.0 - t * t) * (1.0 - 0.25 * t);
                    if v.abs() >= half { continue; }
                    let across = v.abs() / half;
                    let rib = if across < 0.08 { 0.8 } else { 1.0 };
                    let base = 0.75 + 0.25 * ((t + 1.0) * 0.5);
                    0.55 + 0.45 * (0.3 + 0.7 * tone) * rib * base * (0.85 + 0.15 * across)
                };
                // The topmost leaf is the one with the highest draw order.
                let order = r(7);
                if order > best.0 { best = (order, hit); }
            }
        }
    }
    if best.0 < 0.0 { 0.0 } else { best.1.clamp(0.55, 1.0) }
}
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
    fn foliage_cards_cut_between_leaves_and_keep_every_leaf_above_the_mask() {
        for kind in [PatternKind::Leaves, PatternKind::Needles] {
            let (mut empty, mut leaf) = (0, 0);
            for i in 0..64 {
                for j in 0..64 {
                    let (x, y) = (i as f64 / 64. * 4., j as f64 / 64. * 4.);
                    let a = sample(kind, x, y, [4., 4.], 3);
                    assert!(a == 0. || (0.55..=1.).contains(&a), "{kind:?} {a}");
                    if a == 0. { empty += 1 } else { leaf += 1 }
                    assert!((a - sample(kind, x + 4., y, [4., 4.], 3)).abs() < 1e-9);
                }
            }
            // A clump: dense leaves with gaps, and an empty rim.
            assert!(leaf > 64 * 64 / 4 && empty > 64 * 64 / 5, "{kind:?} {leaf} {empty}");
            assert_eq!(sample(kind, 0.02, 0.02, [4., 4.], 3), 0.);
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

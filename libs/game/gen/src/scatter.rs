//! Seed-deterministic placement: where the trees actually go.
//!
//! A forest is a scatter problem, not a mesh problem. Uniform random placement
//! clumps and leaves bald patches; a grid reads as a plantation. Poisson-disk
//! sampling gives the natural-looking spacing, and because it runs off
//! [`GenRng`] every device produces the identical layout from one seed — so a
//! forest replicates as (preset, seed, area) rather than as a list of
//! positions.

use crate::rng::GenRng;
use makepad_game_math as gm;
use makepad_math::*;

/// One placed instance.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub pos: Vec3f,
    /// Yaw in radians, so a forest of one mesh does not look stamped.
    pub yaw: f32,
    /// Uniform scale jitter around 1.0.
    pub scale: f32,
    /// Which variant/seed this instance should use, for callers holding a
    /// small pool of generated meshes.
    pub variant: u32,
}

/// Rules governing where instances may land.
pub struct ScatterParams<'a> {
    pub seed: u64,
    /// Minimum distance between instances.
    pub spacing: f32,
    /// Rectangular area, centred on the origin.
    pub extent: Vec2f,
    /// Hard cap, so a large area with tight spacing cannot run away.
    pub max_count: usize,
    /// Scale jitter range.
    pub scale_range: (f32, f32),
    /// How many distinct generated variants callers will provide.
    pub variants: u32,
    /// Terrain height at (x, z), or None for a flat ground plane at y = 0.
    pub height_at: Option<&'a dyn Fn(f32, f32) -> f32>,
    /// Reject placements steeper than this (dot of the surface normal with
    /// up). 1.0 accepts only flat ground, 0.0 accepts anything.
    pub min_flatness: f32,
    /// Reject placements below this height — keeps trees out of water.
    pub min_height: f32,
    pub max_height: f32,
    /// Density in [0, 1] at (x, z); returning 0 excludes the point entirely.
    /// This is how a caller carves clearings, roads or a lake out of a forest.
    pub density_at: Option<&'a dyn Fn(f32, f32) -> f32>,
}

impl<'a> Default for ScatterParams<'a> {
    fn default() -> Self {
        Self {
            seed: 0,
            spacing: 4.0,
            extent: vec2f(60.0, 60.0),
            max_count: 2000,
            scale_range: (0.8, 1.25),
            variants: 6,
            height_at: None,
            min_flatness: 0.75,
            min_height: f32::NEG_INFINITY,
            max_height: f32::INFINITY,
            density_at: None,
        }
    }
}

/// Bridson-style Poisson-disk sampling over a rectangle, with the placement
/// rules applied as rejections.
///
/// The background grid makes the neighbour test O(1): with cell size
/// `spacing/sqrt(2)` a cell holds at most one sample, so only the 5x5
/// neighbourhood can possibly be too close.
pub fn scatter(p: &ScatterParams) -> Vec<Placement> {
    let mut rng = GenRng::new(p.seed);
    let spacing = p.spacing.max(0.05);
    let (w, h) = (p.extent.x.max(spacing), p.extent.y.max(spacing));
    let cell = spacing / 1.414_213_6;
    let cols = (w / cell).ceil() as usize + 1;
    let rows = (h / cell).ceil() as usize + 1;
    // usize::MAX marks an empty cell; a Vec of indices into `out`.
    let mut grid = vec![usize::MAX; cols * rows];
    let mut out: Vec<Placement> = Vec::new();
    let mut active: Vec<usize> = Vec::new();

    let to_cell = |x: f32, z: f32| -> (usize, usize) {
        let cx = (((x + w * 0.5) / cell) as usize).min(cols - 1);
        let cz = (((z + h * 0.5) / cell) as usize).min(rows - 1);
        (cx, cz)
    };

    let far_enough = |out: &[Placement], grid: &[usize], x: f32, z: f32| -> bool {
        let (cx, cz) = to_cell(x, z);
        let x0 = cx.saturating_sub(2);
        let z0 = cz.saturating_sub(2);
        for gz in z0..(cz + 3).min(rows) {
            for gx in x0..(cx + 3).min(cols) {
                let idx = grid[gz * cols + gx];
                if idx == usize::MAX {
                    continue;
                }
                let o = out[idx].pos;
                let (dx, dz) = (o.x - x, o.z - z);
                if dx * dx + dz * dz < spacing * spacing {
                    return false;
                }
            }
        }
        true
    };

    // Accept/reject against the terrain and density rules.
    let accept = |rng: &mut GenRng, x: f32, z: f32| -> Option<f32> {
        if x < -w * 0.5 || x > w * 0.5 || z < -h * 0.5 || z > h * 0.5 {
            return None;
        }
        if let Some(d) = p.density_at {
            let density = d(x, z).clamp(0.0, 1.0);
            if density <= 0.0 || !rng.chance(density) {
                return None;
            }
        }
        let y = match p.height_at {
            Some(f) => f(x, z),
            None => 0.0,
        };
        if y < p.min_height || y > p.max_height {
            return None;
        }
        if let Some(f) = p.height_at {
            // Central-difference slope. A tree on a cliff face reads as a bug,
            // and this is cheaper than asking the caller for a normal.
            let e = spacing * 0.25;
            let dx = (f(x + e, z) - f(x - e, z)) / (2.0 * e);
            let dz = (f(x, z + e) - f(x, z - e)) / (2.0 * e);
            let flat = 1.0 / (1.0 + dx * dx + dz * dz).sqrt();
            if flat < p.min_flatness {
                return None;
            }
        }
        Some(y)
    };

    let push = |out: &mut Vec<Placement>,
                    grid: &mut [usize],
                    active: &mut Vec<usize>,
                    rng: &mut GenRng,
                    x: f32,
                    y: f32,
                    z: f32| {
        let pl = Placement {
            pos: vec3f(x, y, z),
            yaw: rng.range(0.0, 6.283_185_3),
            scale: rng.range(p.scale_range.0, p.scale_range.1),
            variant: if p.variants == 0 {
                0
            } else {
                rng.index(p.variants as usize) as u32
            },
        };
        let (cx, cz) = to_cell(x, z);
        out.push(pl);
        grid[cz * cols + cx] = out.len() - 1;
        active.push(out.len() - 1);
    };

    // Seed point: try a few times, since the very first sample can land on a
    // cliff or in a hole and an empty forest from one unlucky draw is a bug.
    for _ in 0..64 {
        let x = rng.range(-w * 0.5, w * 0.5);
        let z = rng.range(-h * 0.5, h * 0.5);
        if let Some(y) = accept(&mut rng, x, z) {
            push(&mut out, &mut grid, &mut active, &mut rng, x, y, z);
            break;
        }
    }

    const TRIES: usize = 24;
    while let Some(&ai) = active.last() {
        if out.len() >= p.max_count {
            break;
        }
        let origin = out[ai].pos;
        let mut placed = false;
        for _ in 0..TRIES {
            let a = rng.range(0.0, 6.283_185_3);
            let r = rng.range(spacing, spacing * 2.0);
            let (s, c) = gm::sincos(a);
            let (x, z) = (origin.x + c * r, origin.z + s * r);
            if !far_enough(&out, &grid, x, z) {
                continue;
            }
            if let Some(y) = accept(&mut rng, x, z) {
                push(&mut out, &mut grid, &mut active, &mut rng, x, y, z);
                placed = true;
                break;
            }
        }
        if !placed {
            active.pop();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn min_gap(v: &[Placement]) -> f32 {
        let mut m = f32::INFINITY;
        for i in 0..v.len() {
            for j in (i + 1)..v.len() {
                let (a, b) = (v[i].pos, v[j].pos);
                let (dx, dz) = (a.x - b.x, a.z - b.z);
                m = m.min((dx * dx + dz * dz).sqrt());
            }
        }
        m
    }

    #[test]
    fn spacing_is_respected() {
        let p = ScatterParams {
            seed: 1,
            spacing: 5.0,
            extent: vec2f(60.0, 60.0),
            ..Default::default()
        };
        let v = scatter(&p);
        assert!(v.len() > 20, "too few placements: {}", v.len());
        let gap = min_gap(&v);
        assert!(gap >= 5.0 - 1.0e-3, "min gap {gap} under spacing 5.0");
    }

    #[test]
    fn fills_the_area_rather_than_clumping() {
        let p = ScatterParams {
            seed: 2,
            spacing: 4.0,
            extent: vec2f(60.0, 60.0),
            ..Default::default()
        };
        let v = scatter(&p);
        // A Poisson-disk fill of a 60x60 area at spacing 4 should be in the
        // low hundreds; a clumping sampler gives far fewer.
        assert!(v.len() > 100, "sparse fill: {}", v.len());
        // Every quadrant should be occupied.
        for (sx, sz) in [(1.0f32, 1.0f32), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
            assert!(
                v.iter()
                    .any(|p| p.pos.x * sx > 5.0 && p.pos.z * sz > 5.0),
                "quadrant ({sx},{sz}) empty"
            );
        }
    }

    #[test]
    fn same_seed_same_layout() {
        let mk = |seed| {
            scatter(&ScatterParams {
                seed,
                spacing: 5.0,
                ..Default::default()
            })
            .iter()
            .map(|p| (p.pos.x.to_bits(), p.pos.z.to_bits(), p.yaw.to_bits()))
            .collect::<Vec<_>>()
        };
        assert_eq!(mk(7), mk(7));
        assert_ne!(mk(7), mk(8), "seed had no effect");
    }

    #[test]
    fn stays_inside_the_extent() {
        let p = ScatterParams {
            seed: 3,
            spacing: 3.0,
            extent: vec2f(40.0, 20.0),
            ..Default::default()
        };
        for pl in scatter(&p) {
            assert!(pl.pos.x.abs() <= 20.0 + 1.0e-3, "x {} escaped", pl.pos.x);
            assert!(pl.pos.z.abs() <= 10.0 + 1.0e-3, "z {} escaped", pl.pos.z);
        }
    }

    #[test]
    fn slope_rule_keeps_trees_off_a_cliff() {
        // A ramp that is flat for x < 0 and very steep for x > 0.
        let height = |x: f32, _z: f32| if x > 0.0 { x * 6.0 } else { 0.0 };
        let p = ScatterParams {
            seed: 4,
            spacing: 3.0,
            extent: vec2f(40.0, 40.0),
            min_flatness: 0.9,
            height_at: Some(&height),
            ..Default::default()
        };
        let v = scatter(&p);
        assert!(!v.is_empty());
        let on_slope = v.iter().filter(|p| p.pos.x > 2.0).count();
        assert_eq!(on_slope, 0, "{on_slope} placements on the cliff");
    }

    #[test]
    fn height_band_keeps_trees_out_of_water() {
        // A bowl: negative in the middle, positive at the rim.
        let height = |x: f32, z: f32| (x * x + z * z) * 0.01 - 4.0;
        let p = ScatterParams {
            seed: 5,
            spacing: 3.0,
            extent: vec2f(60.0, 60.0),
            min_height: 0.0,
            min_flatness: 0.0,
            height_at: Some(&height),
            ..Default::default()
        };
        for pl in scatter(&p) {
            assert!(pl.pos.y >= 0.0, "placed underwater at y={}", pl.pos.y);
        }
    }

    #[test]
    fn density_map_carves_a_clearing() {
        // No trees within 10 units of the origin.
        let density = |x: f32, z: f32| {
            if (x * x + z * z).sqrt() < 10.0 {
                0.0
            } else {
                1.0
            }
        };
        let p = ScatterParams {
            seed: 6,
            spacing: 3.0,
            extent: vec2f(60.0, 60.0),
            density_at: Some(&density),
            ..Default::default()
        };
        let v = scatter(&p);
        assert!(!v.is_empty());
        for pl in &v {
            let d = (pl.pos.x * pl.pos.x + pl.pos.z * pl.pos.z).sqrt();
            assert!(d >= 10.0, "tree in the clearing at {d}");
        }
    }

    #[test]
    fn max_count_is_a_hard_cap() {
        let p = ScatterParams {
            seed: 8,
            spacing: 1.0,
            extent: vec2f(200.0, 200.0),
            max_count: 50,
            ..Default::default()
        };
        assert!(scatter(&p).len() <= 50);
    }

    #[test]
    fn variants_and_yaw_vary() {
        let p = ScatterParams {
            seed: 9,
            spacing: 4.0,
            variants: 4,
            ..Default::default()
        };
        let v = scatter(&p);
        assert!(v.len() > 20);
        let distinct: std::collections::HashSet<u32> = v.iter().map(|p| p.variant).collect();
        assert!(distinct.len() > 1, "all instances got one variant");
        assert!(v.iter().any(|p| p.yaw > 0.1), "no yaw variation");
        for pl in &v {
            assert!((0.8..=1.25).contains(&pl.scale));
        }
    }
}

//! Generated textures: a small noise kit, the material presets built from it,
//! and CPU mip-chain generation.
//!
//! Mips are not optional here. The backends upload a chain level by level and
//! never generate one, so a texture supplied as a single level both aliases
//! badly when minified and costs more bandwidth than a mipped one — the two
//! things a tiler punishes hardest.
//!
//! Textures are deliberately small (64–256px). These are stylised tiling
//! materials, not photographs, and every texture byte competes with the vertex
//! bandwidth that the packed-vertex work just bought back.

use crate::rng::GenRng;
use makepad_game_math as gm;

/// One RGBA8 image level.
#[derive(Clone, Debug)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    /// RGBA8, row-major.
    pub pixels: Vec<u8>,
}

impl Image {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width * height * 4],
        }
    }

    pub fn set(&mut self, x: usize, y: usize, rgb: [f32; 3], a: f32) {
        let i = (y * self.width + x) * 4;
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        self.pixels[i] = q(rgb[0]);
        self.pixels[i + 1] = q(rgb[1]);
        self.pixels[i + 2] = q(rgb[2]);
        self.pixels[i + 3] = q(a);
    }

    pub fn get(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.width + x) * 4;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }

    /// Box-filter this image to half size. Assumes even dimensions.
    fn halve(&self) -> Image {
        let (w, h) = ((self.width / 2).max(1), (self.height / 2).max(1));
        let mut out = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (sx, sy) = (x * 2, y * 2);
                let mut acc = [0u32; 4];
                for (ox, oy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = self.get((sx + ox).min(self.width - 1), (sy + oy).min(self.height - 1));
                    for c in 0..4 {
                        acc[c] += p[c] as u32;
                    }
                }
                let i = (y * w + x) * 4;
                for c in 0..4 {
                    out.pixels[i + c] = (acc[c] / 4) as u8;
                }
            }
        }
        out
    }
}

/// A full mip chain, level 0 first, down to 1x1.
#[derive(Clone, Debug)]
pub struct MipChain {
    pub levels: Vec<Image>,
}

impl MipChain {
    /// Build the chain by repeated box filtering.
    pub fn build(base: Image) -> Self {
        let mut levels = vec![base];
        while {
            let l = levels.last().unwrap();
            l.width > 1 || l.height > 1
        } {
            let next = levels.last().unwrap().halve();
            levels.push(next);
        }
        Self { levels }
    }

    pub fn total_bytes(&self) -> usize {
        self.levels.iter().map(|l| l.pixels.len()).sum()
    }
}

// ---------------------------------------------------------------- noise kit

fn hash2(x: i32, y: i32, seed: u32) -> f32 {
    // Integer hash then to [0,1). Deterministic across platforms: pure
    // wrapping integer maths, no floats until the final divide.
    let mut h = (x as u32).wrapping_mul(0x8DA6_B343)
        ^ (y as u32).wrapping_mul(0xD8163_841u32)
        ^ seed.wrapping_mul(0x1B87_3593);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xff_ffff) as f32 / 16_777_216.0
}

fn smootherstep(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Value noise, tiling with period `period`.
pub fn value_noise(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor(), y.floor());
    let (xf, yf) = (x - xi, y - yi);
    let wrap = |v: i32| v.rem_euclid(period.max(1));
    let (x0, y0) = (wrap(xi as i32), wrap(yi as i32));
    let (x1, y1) = (wrap(xi as i32 + 1), wrap(yi as i32 + 1));
    let (u, v) = (smootherstep(xf), smootherstep(yf));
    let a = hash2(x0, y0, seed);
    let b = hash2(x1, y0, seed);
    let c = hash2(x0, y1, seed);
    let d = hash2(x1, y1, seed);
    let top = a + (b - a) * u;
    let bot = c + (d - c) * u;
    top + (bot - top) * v
}

/// Fractal Brownian motion over [`value_noise`].
pub fn fbm(x: f32, y: f32, period: i32, octaves: u32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut norm = 0.0;
    let mut p = period;
    let (mut cx, mut cy) = (x, y);
    for o in 0..octaves.max(1) {
        sum += value_noise(cx, cy, p, seed.wrapping_add(o * 977)) * amp;
        norm += amp;
        amp *= 0.5;
        cx *= 2.0;
        cy *= 2.0;
        p = p.saturating_mul(2);
    }
    sum / norm.max(1.0e-6)
}

/// Worley / cellular noise: distance to the nearest feature point. Tiles on
/// `period` cells.
pub fn worley(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (xf, yf) = (x - x.floor(), y - y.floor());
    let mut best = f32::INFINITY;
    for oy in -1..=1 {
        for ox in -1..=1 {
            let (cx, cy) = (xi + ox, yi + oy);
            let (wx, wy) = (cx.rem_euclid(period.max(1)), cy.rem_euclid(period.max(1)));
            let px = ox as f32 + hash2(wx, wy, seed);
            let py = oy as f32 + hash2(wx, wy, seed ^ 0x9E37_79B9);
            let (dx, dy) = (px - xf, py - yf);
            best = best.min(dx * dx + dy * dy);
        }
    }
    best.sqrt().min(1.0)
}

/// Domain warp: displace the sample point by noise before sampling again.
/// Turns bland fbm into something with flow and structure.
pub fn warped_fbm(x: f32, y: f32, period: i32, strength: f32, seed: u32) -> f32 {
    let wx = fbm(x, y, period, 3, seed ^ 0x1234) - 0.5;
    let wy = fbm(x + 5.2, y + 1.3, period, 3, seed ^ 0x5678) - 0.5;
    fbm(x + wx * strength, y + wy * strength, period, 4, seed)
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

// ---------------------------------------------------------------- materials

/// Names the texture generator accepts.
pub const MATERIALS: &[&str] = &[
    "bark", "foliage", "rock", "dirt", "grass", "sand", "water", "brick", "wood", "tiles", "metal",
];

/// Generate a tiling material texture. Deterministic in `seed`.
pub fn material(name: &str, size: usize, seed: u64) -> Image {
    let size = size.clamp(8, 512).next_power_of_two();
    let mut img = Image::new(size, size);
    let s32 = (seed ^ (seed >> 32)) as u32;
    let mut rng = GenRng::new(seed);
    let period = 8;
    // Sample in tile space so the result wraps: [0, period) maps to the image.
    let f = |i: usize| i as f32 / size as f32 * period as f32;

    match name {
        "foliage" => {
            let dark = [0.13, 0.30, 0.14];
            let light = [0.36, 0.62, 0.26];
            for y in 0..size {
                for x in 0..size {
                    // Cellular gives leaf-cluster clumps rather than mush.
                    let c = worley(f(x), f(y), period, s32);
                    let n = fbm(f(x) * 2.0, f(y) * 2.0, period * 2, 3, s32);
                    let t = (1.0 - c) * 0.7 + n * 0.3;
                    img.set(x, y, mix3(dark, light, t), 1.0);
                }
            }
        }
        "rock" => {
            let dark = [0.32, 0.31, 0.30];
            let light = [0.62, 0.61, 0.58];
            for y in 0..size {
                for x in 0..size {
                    // Warping is what stops rock reading as generic noise.
                    let n = warped_fbm(f(x), f(y), period, 1.4, s32);
                    let crack = (1.0 - worley(f(x) * 1.5, f(y) * 1.5, period, s32 ^ 0xAA)).powi(6);
                    let c = mix3(dark, light, n);
                    img.set(x, y, mix3(c, [0.16, 0.15, 0.15], crack), 1.0);
                }
            }
        }
        "dirt" => {
            let dark = [0.24, 0.17, 0.11];
            let light = [0.46, 0.34, 0.22];
            for y in 0..size {
                for x in 0..size {
                    let n = fbm(f(x) * 1.5, f(y) * 1.5, period, 4, s32);
                    let grit = hash2(x as i32, y as i32, s32) * 0.12;
                    img.set(x, y, mix3(dark, light, n + grit), 1.0);
                }
            }
        }
        "grass" => {
            let dark = [0.18, 0.36, 0.14];
            let light = [0.40, 0.62, 0.24];
            for y in 0..size {
                for x in 0..size {
                    // Stretched vertically so it reads as blades, not moss.
                    let n = fbm(f(x) * 3.0, f(y) * 0.6, period * 2, 3, s32);
                    let blade = hash2(x as i32, (y / 3) as i32, s32) * 0.25;
                    img.set(x, y, mix3(dark, light, n + blade), 1.0);
                }
            }
        }
        "sand" => {
            let dark = [0.68, 0.58, 0.38];
            let light = [0.90, 0.82, 0.62];
            for y in 0..size {
                for x in 0..size {
                    let ripple = (gm::sin(f(x) * 3.0 + fbm(f(x), f(y), period, 2, s32) * 4.0) * 0.5
                        + 0.5)
                        * 0.4;
                    let n = fbm(f(x) * 2.0, f(y) * 2.0, period, 3, s32) * 0.6;
                    img.set(x, y, mix3(dark, light, ripple + n), 1.0);
                }
            }
        }
        "water" => {
            let deep = [0.06, 0.24, 0.42];
            let shallow = [0.22, 0.55, 0.72];
            for y in 0..size {
                for x in 0..size {
                    let n = warped_fbm(f(x), f(y), period, 0.8, s32);
                    img.set(x, y, mix3(deep, shallow, n), 0.82);
                }
            }
        }
        "brick" => {
            let mortar = [0.72, 0.70, 0.66];
            let brick_a = [0.62, 0.26, 0.20];
            let brick_b = [0.48, 0.20, 0.16];
            let rows = 8usize;
            let cols = 4usize;
            let rh = size / rows;
            let cw = size / cols;
            let m = (size / 64).max(1);
            for y in 0..size {
                let row = y / rh;
                // Every other row offsets by half a brick — the running bond
                // that stops it looking like a grid.
                let offset = if row % 2 == 0 { 0 } else { cw / 2 };
                for x in 0..size {
                    let bx = (x + offset) % size;
                    let in_mortar = (y % rh) < m || (bx % cw) < m;
                    let jitter = hash2((bx / cw) as i32, row as i32, s32);
                    let base = mix3(brick_a, brick_b, jitter);
                    let n = fbm(f(x) * 4.0, f(y) * 4.0, period * 2, 2, s32) * 0.18 - 0.09;
                    let c = if in_mortar {
                        mortar
                    } else {
                        [base[0] + n, base[1] + n, base[2] + n]
                    };
                    img.set(x, y, c, 1.0);
                }
            }
        }
        "wood" => {
            let dark = [0.32, 0.20, 0.10];
            let light = [0.63, 0.44, 0.24];
            for y in 0..size {
                for x in 0..size {
                    // Rings: a warped coordinate run through fract gives grain.
                    let warp = fbm(f(x) * 0.6, f(y) * 3.0, period, 3, s32) * 1.6;
                    let ring = ((f(x) * 1.5 + warp) * 3.0).fract().abs();
                    let plank = if (y / (size / 4)) % 2 == 0 { 0.0 } else { 0.08 };
                    img.set(x, y, mix3(dark, light, ring * 0.8 + plank), 1.0);
                }
            }
        }
        "tiles" => {
            let grout = [0.78, 0.77, 0.74];
            let tile_a = [0.86, 0.86, 0.84];
            let tile_b = [0.68, 0.72, 0.74];
            let n_tiles = 4usize;
            let tw = size / n_tiles;
            let g = (size / 64).max(1);
            for y in 0..size {
                for x in 0..size {
                    let in_grout = (x % tw) < g || (y % tw) < g;
                    let checker = ((x / tw) + (y / tw)) % 2 == 0;
                    let base = if checker { tile_a } else { tile_b };
                    let n = fbm(f(x) * 6.0, f(y) * 6.0, period * 2, 2, s32) * 0.06 - 0.03;
                    let c = if in_grout {
                        grout
                    } else {
                        [base[0] + n, base[1] + n, base[2] + n]
                    };
                    img.set(x, y, c, 1.0);
                }
            }
        }
        "metal" => {
            let dark = [0.42, 0.44, 0.48];
            let light = [0.76, 0.78, 0.82];
            for y in 0..size {
                for x in 0..size {
                    // Anisotropic: brushed along x.
                    let brush = hash2((x / 2) as i32, y as i32, s32) * 0.35;
                    let n = fbm(f(x) * 0.5, f(y) * 8.0, period, 2, s32) * 0.65;
                    img.set(x, y, mix3(dark, light, brush + n), 1.0);
                }
            }
        }
        // "bark" and anything unrecognised.
        _ => {
            let dark = [0.20, 0.14, 0.09];
            let light = [0.46, 0.34, 0.22];
            for y in 0..size {
                for x in 0..size {
                    // Vertical fissures: noise stretched hard along y.
                    let n = fbm(f(x) * 4.0, f(y) * 0.5, period * 2, 4, s32);
                    let fissure = (1.0 - worley(f(x) * 2.0, f(y) * 0.4, period, s32)).powi(3);
                    let c = mix3(dark, light, n);
                    img.set(x, y, mix3(c, [0.12, 0.08, 0.05], fissure), 1.0);
                }
            }
        }
    }
    let _ = rng.next_u64();
    img
}

/// Generate a material complete with its mip chain.
pub fn material_mipped(name: &str, size: usize, seed: u64) -> MipChain {
    MipChain::build(material(name, size, seed))
}

/// A square atlas of generated materials, plus the uv rect of each cell.
///
/// One binding for a whole material set: on a tiler, state changes cost more
/// than the texture memory saved by keeping them separate.
#[derive(Clone, Debug)]
pub struct Atlas {
    pub chain: MipChain,
    /// (u0, v0, u1, v1) per entry, in the order given.
    pub cells: Vec<[f32; 4]>,
    pub names: Vec<String>,
}

/// Pack the named materials into a square atlas grid.
///
/// NOTE the tiling caveat: an atlased material can no longer wrap by repeating
/// the uv, because it would bleed into its neighbour. Callers that need
/// wrapping should use [`material_mipped`] for that one material instead.
pub fn atlas(names: &[&str], cell: usize, seed: u64) -> Atlas {
    if names.is_empty() {
        return Atlas {
            chain: MipChain::build(Image::new(1, 1)),
            cells: Vec::new(),
            names: Vec::new(),
        };
    }
    let cell = cell.clamp(8, 256).next_power_of_two();
    let grid = (names.len() as f32).sqrt().ceil() as usize;
    let size = (grid * cell).next_power_of_two();
    let mut img = Image::new(size, size);
    let mut cells = Vec::with_capacity(names.len());

    for (i, name) in names.iter().enumerate() {
        let tile = material(name, cell, seed.wrapping_add(i as u64 * 7919));
        let (gx, gy) = (i % grid, i / grid);
        let (ox, oy) = (gx * cell, gy * cell);
        for y in 0..cell {
            for x in 0..cell {
                let p = tile.get(x, y);
                let d = ((oy + y) * size + ox + x) * 4;
                img.pixels[d..d + 4].copy_from_slice(&p);
            }
        }
        let s = size as f32;
        cells.push([
            ox as f32 / s,
            oy as f32 / s,
            (ox + cell) as f32 / s,
            (oy + cell) as f32 / s,
        ]);
    }

    Atlas {
        chain: MipChain::build(img),
        cells,
        names: names.iter().map(|s| s.to_string()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_deterministic_and_in_range() {
        for i in 0..200 {
            let (x, y) = (i as f32 * 0.37, i as f32 * 0.11);
            let a = fbm(x, y, 8, 4, 42);
            let b = fbm(x, y, 8, 4, 42);
            assert_eq!(a.to_bits(), b.to_bits(), "fbm not deterministic");
            assert!((0.0..=1.0).contains(&a), "fbm out of range: {a}");
            let w = worley(x, y, 8, 42);
            assert!((0.0..=1.5).contains(&w), "worley out of range: {w}");
            let v = value_noise(x, y, 8, 42);
            assert!((0.0..=1.0).contains(&v));
        }
    }

    #[test]
    fn noise_actually_varies() {
        // A constant "noise" function would pass a range check and be useless.
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for i in 0..100 {
            let v = fbm(i as f32 * 0.31, i as f32 * 0.77, 8, 4, 1);
            min = min.min(v);
            max = max.max(v);
        }
        assert!(max - min > 0.2, "fbm too flat: {min}..{max}");
    }

    #[test]
    fn value_noise_tiles_on_its_period() {
        // Sampling one period apart must give the same value, or textures seam.
        for i in 0..40 {
            let (x, y) = (i as f32 * 0.19, i as f32 * 0.23);
            let a = value_noise(x, y, 8, 5);
            let b = value_noise(x + 8.0, y, 8, 5);
            let c = value_noise(x, y + 8.0, 8, 5);
            assert!((a - b).abs() < 1.0e-5, "x seam: {a} vs {b}");
            assert!((a - c).abs() < 1.0e-5, "y seam: {a} vs {c}");
        }
    }

    #[test]
    fn every_material_generates_and_is_deterministic() {
        for name in MATERIALS {
            let a = material(name, 32, 9);
            let b = material(name, 32, 9);
            assert_eq!(a.pixels, b.pixels, "{name} not deterministic");
            assert_eq!(a.width, 32);
            assert_eq!(a.pixels.len(), 32 * 32 * 4);
            // Not a flat fill — a solid colour would mean the generator ran
            // but produced nothing worth uploading.
            let first = &a.pixels[0..3];
            assert!(
                a.pixels.chunks_exact(4).any(|p| &p[0..3] != first),
                "{name} is a flat colour"
            );
        }
    }

    #[test]
    fn different_seeds_give_different_textures() {
        let a = material("rock", 32, 1);
        let b = material("rock", 32, 2);
        assert_ne!(a.pixels, b.pixels);
    }

    #[test]
    fn mip_chain_halves_to_one_pixel() {
        let chain = material_mipped("bark", 64, 3);
        assert_eq!(chain.levels.len(), 7, "64 -> 1 is 7 levels");
        let dims: Vec<_> = chain.levels.iter().map(|l| (l.width, l.height)).collect();
        assert_eq!(
            dims,
            vec![(64, 64), (32, 32), (16, 16), (8, 8), (4, 4), (2, 2), (1, 1)]
        );
        // The classic 4/3 total for a full chain.
        let base = 64 * 64 * 4;
        let total = chain.total_bytes();
        assert!(total > base && total < base * 3 / 2, "total {total}");
    }

    #[test]
    fn mip_reduction_averages_rather_than_drops() {
        // A checkerboard must average to mid-grey, not pick one colour.
        let mut img = Image::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                let v = if (x + y) % 2 == 0 { 1.0 } else { 0.0 };
                img.set(x, y, [v, v, v], 1.0);
            }
        }
        let chain = MipChain::build(img);
        let l1 = &chain.levels[1];
        for y in 0..l1.height {
            for x in 0..l1.width {
                let p = l1.get(x, y);
                assert!((100..=155).contains(&p[0]), "expected mid-grey, got {p:?}");
            }
        }
    }

    #[test]
    fn atlas_packs_cells_with_correct_uvs() {
        let a = atlas(&["bark", "rock", "grass", "sand"], 32, 4);
        assert_eq!(a.cells.len(), 4);
        assert_eq!(a.names.len(), 4);
        let base = &a.chain.levels[0];
        assert!(base.width >= 64 && base.width.is_power_of_two());
        for c in &a.cells {
            assert!(c[0] >= 0.0 && c[2] <= 1.0 && c[0] < c[2]);
            assert!(c[1] >= 0.0 && c[3] <= 1.0 && c[1] < c[3]);
        }
        // Cells must not overlap.
        for i in 0..a.cells.len() {
            for j in (i + 1)..a.cells.len() {
                let (p, q) = (a.cells[i], a.cells[j]);
                let disjoint = p[2] <= q[0] || q[2] <= p[0] || p[3] <= q[1] || q[3] <= p[1];
                assert!(disjoint, "cells {i} and {j} overlap");
            }
        }
    }

    #[test]
    fn empty_atlas_does_not_panic() {
        let a = atlas(&[], 32, 1);
        assert!(a.cells.is_empty());
        assert_eq!(a.chain.levels.len(), 1);
    }
}

//! The particle flipbook atlas, generated procedurally once per process on a
//! worker thread (no files, no fleet round-trip). 1024² as an 8×8 grid of
//! 128 px cells:
//!
//! * rows 0–1: 16 frames of a billowing smoke puff that swells and breaks
//!   up over its life. rg = normal (for sun lighting), a = density.
//! * rows 2–3: 16 frames of flame tongues licking upward. r = heat,
//!   a = coverage.
//! * row 4: col 0 and 4–7 lumpy dust clumps, cols 1–3 faceted debris chips
//!   (rg = normal, a = coverage).
//! * rows 6–7: 16 frames of thin curling smoke (gun smoke, steam).
//!
//! Until the atlas lands the shader draws its procedural fallback shapes, so
//! nothing waits on it.

pub const ATLAS_SIZE: usize = 1024;
pub const CELL: usize = 128;

fn hash3(x: i32, y: i32, z: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ (z as u32).wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0x00ff_ffff) as f32 / 16_777_215.0
}

fn noise3(x: f32, y: f32, z: f32) -> f32 {
    let (ix, iy, iz) = (x.floor(), y.floor(), z.floor());
    let (fx, fy, fz) = (x - ix, y - iy, z - iz);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (ux, uy, uz) = (s(fx), s(fy), s(fz));
    let (ix, iy, iz) = (ix as i32, iy as i32, iz as i32);
    let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let c = |dx: i32, dy: i32, dz: i32| hash3(ix + dx, iy + dy, iz + dz);
    let a = l(l(c(0, 0, 0), c(1, 0, 0), ux), l(c(0, 1, 0), c(1, 1, 0), ux), uy);
    let b = l(l(c(0, 0, 1), c(1, 0, 1), ux), l(c(0, 1, 1), c(1, 1, 1), ux), uy);
    l(a, b, uz)
}

fn fbm(x: f32, y: f32, z: f32, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut norm) = (0.0, 0.5, 1.0, 0.0);
    for o in 0..octaves {
        let k = o as f32 * 17.3;
        sum += noise3(x * freq + k, y * freq - k, z * freq + k * 0.5) * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.03;
    }
    sum / norm
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn pack(r: f32, g: f32, b: f32, a: f32) -> u32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    (q(a) << 24) | (q(r) << 16) | (q(g) << 8) | q(b)
}

/// Write one cell from a density field: normals from its gradient blended
/// with a sphere's (so a puff reads round, with lumpy detail).
fn write_density_cell(out: &mut [u32], col: usize, row: usize, dens: &[f32], relief: f32) {
    for y in 0..CELL {
        for x in 0..CELL {
            let at = |x: usize, y: usize| dens[y.min(CELL - 1) * CELL + x.min(CELL - 1)];
            let gx = at(x + 1, y) - at(x.saturating_sub(1), y);
            let gy = at(x, y + 1) - at(x, y.saturating_sub(1));
            let px = (x as f32 + 0.5) / CELL as f32 * 2.0 - 1.0;
            let py = (y as f32 + 0.5) / CELL as f32 * 2.0 - 1.0;
            let r2 = (px * px + py * py).min(1.0);
            // Texture v grows DOWN; the normal's y is up.
            let (sx, sy, sz) = (px * 0.7, -py * 0.7, (1.0 - r2 * 0.49).sqrt());
            let (mut nx, mut ny, mut nz) = (sx - gx * relief, sy + gy * relief, sz);
            let l = (nx * nx + ny * ny + nz * nz).sqrt().max(1.0e-4);
            nx /= l;
            ny /= l;
            nz /= l;
            let d = at(x, y);
            out[(row * CELL + y) * ATLAS_SIZE + col * CELL + x] = pack(nx * 0.5 + 0.5, ny * 0.5 + 0.5, nz, d);
        }
    }
}

fn cell_coords() -> impl Iterator<Item = (usize, usize, f32, f32)> {
    (0..CELL).flat_map(|y| {
        (0..CELL).map(move |x| {
            let px = (x as f32 + 0.5) / CELL as f32 * 2.0 - 1.0;
            let py = (y as f32 + 0.5) / CELL as f32 * 2.0 - 1.0;
            (x, y, px, py)
        })
    })
}

fn smoke_frame(f: f32, seed: f32) -> Vec<f32> {
    let mut d = vec![0.0; CELL * CELL];
    // The puff swells and thins over its frames.
    let reach = 0.62 + 0.3 * f;
    for (x, y, px, py) in cell_coords() {
        let r = (px * px + py * py).sqrt() / reach;
        let fall = (1.0 - r * r).clamp(0.0, 1.0);
        let n = fbm(px * 2.3 + seed, py * 2.3 - f * 0.6, f * 1.3 + seed * 3.1, 5);
        let v = (fall * 1.7 - (1.0 - n) * (0.85 + 0.35 * f)).max(0.0) * (1.0 - 0.35 * f);
        d[y * CELL + x] = v.min(1.0) * smoothstep(1.0, 0.86, (px * px + py * py).sqrt());
    }
    d
}

fn wisp_frame(f: f32) -> Vec<f32> {
    let mut d = vec![0.0; CELL * CELL];
    for (x, y, px, py) in cell_coords() {
        let curl = (py * 3.1 + f * 4.0).sin() * 0.22 * (0.4 + f);
        let band = (-((px - curl) / (0.22 + 0.25 * f)).powi(2)).exp();
        let n = fbm(px * 2.0 + 3.0, py * 3.5 - f * 2.0, f * 1.7, 4);
        let v = (band * (0.55 + n) - 0.45).max(0.0) * 1.6 * (1.0 - py * py) * (1.0 - 0.4 * f);
        d[y * CELL + x] = v.min(1.0) * smoothstep(1.0, 0.85, (px * px + py * py).sqrt());
    }
    d
}

fn dust_cell(seed: f32) -> Vec<f32> {
    let mut d = vec![0.0; CELL * CELL];
    for (x, y, px, py) in cell_coords() {
        let r = (px * px + py * py).sqrt();
        let fall = (1.0 - r * r / 0.7).clamp(0.0, 1.0);
        let n = fbm(px * 3.4 + seed, py * 3.4 + seed * 1.7, seed, 5);
        d[y * CELL + x] = ((fall * 1.8 - (1.0 - n) * 1.1).max(0.0)).min(1.0) * smoothstep(1.0, 0.8, r);
    }
    d
}

fn debris_cell(out: &mut [u32], col: usize, row: usize, seed: f32) {
    // A faceted chip: a radius that jumps between a few facets, domed.
    let facets = 5.0 + (seed * 3.0).floor();
    for (x, y, px, py) in cell_coords() {
        let a = py.atan2(px);
        let k = ((a / std::f32::consts::TAU + 0.5) * facets).floor();
        let rim = 0.5 + 0.35 * hash3(k as i32, seed as i32 * 7 + 3, 11);
        let r = (px * px + py * py).sqrt();
        let cover = smoothstep(rim, rim - 0.03, r);
        // Each facet tilts its own way.
        let tilt = hash3(k as i32, 5, seed as i32) * std::f32::consts::TAU;
        let (nx, ny) = (tilt.cos() * 0.45, tilt.sin() * 0.45);
        let nz = (1.0 - nx * nx - ny * ny).sqrt();
        out[(row * CELL + y) * ATLAS_SIZE + col * CELL + x] = pack(nx * 0.5 + 0.5, ny * 0.5 + 0.5, nz, cover);
    }
}

fn fire_cell(out: &mut [u32], col: usize, row: usize, f: f32) {
    for (x, y, px, py) in cell_coords() {
        // h = 0 at the base (bottom of the cell), 1 at the tip.
        let h = (1.0 - (py * 0.5 + 0.5)).clamp(0.0, 1.0);
        let n = fbm(px * 3.0, h * 3.0 - f * 5.0, f * 2.0, 4);
        let width = 0.62 * (1.0 - h).powf(0.65) + 0.04;
        let edge = px.abs() + (n - 0.5) * 0.45 * (0.3 + h);
        let body = smoothstep(width, width * 0.35, edge);
        let tip = smoothstep(1.0, 0.55, h + (n - 0.5) * 0.5);
        let base = smoothstep(0.0, 0.12, h);
        let cover = (body * tip * base).clamp(0.0, 1.0);
        let heat = cover * (1.0 - h).powf(0.7) * (0.65 + 0.35 * n);
        out[(row * CELL + y) * ATLAS_SIZE + col * CELL + x] = pack(heat, heat * heat, 0.0, cover);
    }
}

/// Mip levels below the 1024² base (128 px cells down to 8 px; deeper
/// levels would blend neighbouring cells).
pub const ATLAS_MIPS: usize = 4;

/// The atlas plus its box-filtered mip chain, levels concatenated largest
/// first — the `VecMipBGRAu8_32` layout.
pub fn build_atlas_mips() -> Vec<u32> {
    let mut out = build_atlas();
    let mut size = ATLAS_SIZE;
    let mut start = 0;
    for _ in 0..ATLAS_MIPS {
        let half = size / 2;
        let mut next = Vec::with_capacity(half * half);
        for y in 0..half {
            for x in 0..half {
                let mut acc = [0u32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let t = out[start + (y * 2 + dy) * size + x * 2 + dx];
                    for (c, a) in acc.iter_mut().enumerate() {
                        *a += (t >> (c * 8)) & 255;
                    }
                }
                next.push(acc.iter().enumerate().fold(0u32, |v, (c, a)| v | (((a + 2) / 4) << (c * 8))));
            }
        }
        start = out.len();
        out.extend(next);
        size = half;
    }
    out
}

/// Build the whole atlas: BGRA texels, row-major, top row first.
pub fn build_atlas() -> Vec<u32> {
    let mut out = vec![0u32; ATLAS_SIZE * ATLAS_SIZE];
    for i in 0..16 {
        let f = i as f32 / 15.0;
        let (col, row) = (i % 8, i / 8);
        write_density_cell(&mut out, col, row, &smoke_frame(f, 0.0), 6.0);
        fire_cell(&mut out, col, 2 + row, f);
        write_density_cell(&mut out, col, 6 + row, &wisp_frame(f), 5.0);
    }
    for (c, seed) in [(0usize, 1.0f32), (4, 2.0), (5, 3.0), (6, 4.0), (7, 5.0)] {
        write_density_cell(&mut out, c, 4, &dust_cell(seed * 9.1), 8.0);
    }
    for c in 1..4 {
        debris_cell(&mut out, c, 4, c as f32);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(t: u32) -> f32 {
        (t >> 24) as f32 / 255.0
    }

    #[test]
    fn cells_are_filled_and_fade_at_their_edges() {
        let atlas = build_atlas();
        assert_eq!(atlas.len(), ATLAS_SIZE * ATLAS_SIZE);
        let texel = |col: usize, row: usize, x: usize, y: usize| atlas[(row * CELL + y) * ATLAS_SIZE + col * CELL + x];
        for (col, row) in [(0, 0), (7, 1), (3, 2), (0, 4), (2, 4), (5, 6)] {
            let mut covered = 0;
            for y in 0..CELL {
                for x in 0..CELL {
                    if alpha(texel(col, row, x, y)) > 0.2 {
                        covered += 1;
                    }
                }
            }
            assert!(covered > CELL * CELL / 40, "cell {col},{row} is nearly empty ({covered})");
            // Corners are transparent so no quad edge ever shows.
            assert!(alpha(texel(col, row, 0, 0)) < 0.02 && alpha(texel(col, row, CELL - 1, CELL - 1)) < 0.02);
        }
    }
}

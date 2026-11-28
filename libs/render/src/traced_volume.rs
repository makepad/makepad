//! Dense volume tiles in a shared R8 atlas page, with one coarse occupancy byte per 4³ block.
//! The proxy packs each tile into model vertices. The shader mirrors the reference march here.

use makepad_draw::pack_pair_f16;

/// Cells on a side of a volume atlas page. 256 is the guaranteed minimum 3D texture size on
/// GLES 3.0 and Vulkan, so a page runs on every backend.
pub const VOLUME_PAGE_CELLS: u32 = 256;
/// Cells on a side of the block one coarse byte describes.
pub const COARSE_BLOCK: u32 = 4;

fn coarse_dims(dims: [u32; 3]) -> [u32; 3] {
    assert!(
        dims.iter().all(|d| d % COARSE_BLOCK == 0),
        "volume dimensions must be multiples of 4"
    );
    dims.map(|d| d / COARSE_BLOCK)
}

fn cell_index([x, y, z]: [u32; 3], [w, h, _]: [u32; 3]) -> usize {
    x as usize + w as usize * (y as usize + h as usize * z as usize)
}

/// Compute the 2x2x2 = 8 coarse bytes (one per 4^3 block) directly from an 8^3 brick's
/// `[u64; 8]` occupancy bitmask. Matches `libs/render/src/traced_volume.rs::build_coarse` bit-for-bit.
#[inline]
pub fn brick_coarse_bytes(occ: [u64; 8]) -> [u8; 8] {
    let mut out = [0u8; 8];
    for bz in 0..2 {
        let lo = occ[4 * bz] | occ[4 * bz + 1];
        let hi = occ[4 * bz + 2] | occ[4 * bz + 3];
        for by in 0..2 {
            for bx in 0..2 {
                let shift = 4 * bx + 32 * by;
                let a = lo >> shift;
                let b = hi >> shift;
                let mut byte = 0u8;
                if a & 0x0000_0303 != 0 { byte |= 1 << 0; }
                if a & 0x0000_0C0C != 0 { byte |= 1 << 1; }
                if a & 0x0303_0000 != 0 { byte |= 1 << 2; }
                if a & 0x0C0C_0000 != 0 { byte |= 1 << 3; }
                if b & 0x0000_0303 != 0 { byte |= 1 << 4; }
                if b & 0x0000_0C0C != 0 { byte |= 1 << 5; }
                if b & 0x0303_0000 != 0 { byte |= 1 << 6; }
                if b & 0x0C0C_0000 != 0 { byte |= 1 << 7; }
                out[bx + 2 * by + 4 * bz] = byte;
            }
        }
    }
    out
}

/// Build the eight child occupancy bits for every 4³ block of the grid.
pub fn build_coarse(cells: &[u8], dims: [u32; 3]) -> Vec<u8> {
    let mut coarse = vec![0; coarse_dims(dims).iter().map(|&d| d as usize).product()];
    patch_coarse(cells, dims, &mut coarse, [0; 3], dims);
    coarse
}

/// Recompute the coarse bytes touched by the half-open cell box `[lo, hi)`.
pub fn patch_coarse(
    cells: &[u8],
    dims: [u32; 3],
    coarse: &mut [u8],
    lo: [u32; 3],
    hi: [u32; 3],
) {
    let blocks = coarse_dims(dims);
    assert!((0..3).all(|a| lo[a] <= hi[a] && hi[a] <= dims[a]), "edit outside volume");
    if (0..3).any(|a| lo[a] == hi[a]) {
        return;
    }
    let first = lo.map(|q| q / COARSE_BLOCK);
    let end = hi.map(|q| q.div_ceil(COARSE_BLOCK));
    for bz in first[2]..end[2] {
        for by in first[1]..end[1] {
            for bx in first[0]..end[0] {
                let mut byte = 0;
                for z in 0..COARSE_BLOCK {
                    for y in 0..COARSE_BLOCK {
                        let row = cell_index([4 * bx, 4 * by + y, 4 * bz + z], dims);
                        let pair = u8::from(cells[row] | cells[row + 1] != 0)
                            | (u8::from(cells[row + 2] | cells[row + 3] != 0) << 1);
                        byte |= pair << (2 * (y >> 1) + 4 * (z >> 1));
                    }
                }
                coarse[cell_index([bx, by, bz], blocks)] = byte;
            }
        }
    }
}

/// One proxy box of a dense volume, stored in an atlas page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TracedVolumeTile {
    /// Model-grid coordinates of the tile's cell (0,0,0).
    pub origin_cells: [i32; 3],
    /// Extent in cells, each in 1..=128.
    pub extent: [u8; 3],
    /// Page coordinates of the tile's cell (0,0,0). atlas_origin + extent <= VOLUME_PAGE_CELLS.
    pub atlas_origin: [u8; 3],
    pub palette_page: u8,
}

pub struct VolumePage<'a> {
    pub dims: [u32; 3],
    pub cells: &'a [u8],
    pub coarse: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeHit {
    pub t: f32,
    /// Page coordinates of the hit cell.
    pub cell: [u32; 3],
    /// Axis of the face the ray entered the cell through (0 = x, 1 = y, 2 = z).
    /// Simultaneous entries prefer X, then Y, then Z.
    pub axis: u8,
    pub material: u8,
    /// Loop iterations used, including the one that found the hit.
    pub steps: u32,
}

fn ray_cell(at: f32, d: f32) -> i32 {
    if d >= 0.0 { at.floor() as i32 } else { at.ceil() as i32 - 1 }
}

pub(crate) fn volume_child_occupied(byte: u8, cell: [u32; 3]) -> bool {
    let [cx, cy, cz] = cell.map(|v| (v >> 1) & 1);
    byte & (1 << (cx + 2 * cy + 4 * cz)) != 0
}

pub(crate) fn volume_exit_plane(block_lo: i32, size: i32, d: f32) -> i32 {
    if d > 0.0 { block_lo + size } else { block_lo }
}

impl TracedVolumeTile {
    /// The march the shader mirrors. `p` is the ray origin in tile-local cells (the tile spans
    /// [0, extent)), `d` its direction in cells (any non-zero length). With `skip` false the coarse
    /// bytes are ignored and every step is one cell: the fine-only reference.
    /// The recipe's `tile_cells` covers the largest extent.
    pub fn march(&self, page: &VolumePage<'_>, p: [f32; 3], d: [f32; 3], tile_cells: u32, skip: bool) -> Option<VolumeHit> {
        self.march_counted(page, p, d, tile_cells, skip).ok()
    }

    /// Like [`Self::march`], returning the iteration count on a miss as `Err(steps)`.
    pub fn march_counted(&self, page: &VolumePage<'_>, p: [f32; 3], d: [f32; 3], tile_cells: u32, skip: bool) -> Result<VolumeHit, u32> {
        debug_assert!(self.extent.iter().all(|e| (1..=128).contains(e)), "invalid tile extent");
        debug_assert!((0..3).all(|a| u32::from(self.atlas_origin[a]) + u32::from(self.extent[a]) <= page.dims[a]),
            "tile outside atlas page");
        debug_assert!(tile_cells >= u32::from(*self.extent.iter().max().unwrap()), "tile exceeds march bound");
        debug_assert_eq!(page.cells.len(), page.dims.iter().map(|&d| d as usize).product::<usize>(), "material page length");
        let mut entry = f32::NEG_INFINITY;
        let mut exit_t = f32::INFINITY;
        for a in 0..3 {
            let hi = f32::from(self.extent[a]);
            if d[a] == 0.0 {
                if p[a] < 0.0 || p[a] >= hi {
                    return Err(0);
                }
            } else {
                let t0 = (0.0 - p[a]) / d[a];
                let t1 = (hi - p[a]) / d[a];
                entry = entry.max(t0.min(t1));
                exit_t = exit_t.min(t0.max(t1));
            }
        }
        let mut t = entry.max(0.0);
        if t >= exit_t {
            return Err(0);
        }

        let lo = self.atlas_origin.map(i32::from);
        let hi = [0, 1, 2].map(|a| lo[a] + i32::from(self.extent[a]));
        let pa = [0, 1, 2].map(|a| p[a] + lo[a] as f32);
        let mut cell = [0, 1, 2].map(|a| ray_cell(pa[a] + t * d[a], d[a]).clamp(lo[a], hi[a] - 1));
        let blocks = coarse_dims(page.dims);
        for i in 0..3 * tile_cells {
            let q = cell.map(|v| v as u32);
            let mut size = 1;
            if skip {
                let byte = page.coarse[cell_index(q.map(|v| v >> 2), blocks)];
                if byte == 0 {
                    size = 4;
                } else if !volume_child_occupied(byte, q) {
                    size = 2;
                }
            }
            if size == 1 {
                let material = page.cells[cell_index(q, page.dims)];
                if material != 0 {
                    let [x, y, z] = [0, 1, 2].map(|a| {
                        if d[a] > 0.0 {
                            (cell[a] as f32 - pa[a]) / d[a]
                        } else if d[a] < 0.0 {
                            ((cell[a] + 1) as f32 - pa[a]) / d[a]
                        } else {
                            f32::NEG_INFINITY
                        }
                    });
                    let axis = if x >= y && x >= z { 0 } else if y >= z { 1 } else { 2 };
                    return Ok(VolumeHit { t, cell: q, axis, material, steps: i + 1 });
                }
            }

            let block_lo = cell.map(|v| (v / size) * size);
            let mut t_axis = [f32::INFINITY; 3];
            for a in 0..3 {
                if d[a] != 0.0 {
                    let plane = volume_exit_plane(block_lo[a], size, d[a]);
                    t_axis[a] = (plane as f32 - pa[a]) / d[a];
                }
            }
            let t_next = t_axis[0].min(t_axis[1]).min(t_axis[2]);
            if t_next >= exit_t {
                return Err(i + 1);
            }
            for a in 0..3 {
                if t_axis[a] <= t_next {
                    cell[a] = if d[a] > 0.0 { block_lo[a] + size } else { block_lo[a] - 1 };
                } else if d[a] != 0.0 {
                    let at = pa[a] + t_next * d[a];
                    let next = ray_cell(at, d[a]).clamp(block_lo[a], block_lo[a] + size - 1);
                    cell[a] = if d[a] > 0.0 { cell[a].max(next) } else { cell[a].min(next) };
                }
            }
            if (0..3).any(|a| cell[a] < lo[a] || cell[a] >= hi[a]) {
                return Err(i + 1);
            }
            t = t_next;
        }
        debug_assert!(false, "volume march exhausted its monotone 3T bound");
        Err(3 * tile_cells)
    }

    /// Append the tile's eight corners and twelve triangles to a static model.
    pub fn append_box(&self, vertices: &mut Vec<f32>, indices: &mut Vec<u32>, cell_pitch: f32) {
        assert!(self.extent.iter().all(|e| (1..=128).contains(e)), "invalid tile extent");
        assert!((0..3).all(|a| {
            u32::from(self.atlas_origin[a]) + u32::from(self.extent[a]) <= VOLUME_PAGE_CELLS
        }), "tile outside atlas page");
        assert!(cell_pitch.is_finite() && cell_pitch > 0.0);
        let stride = crate::model::MODEL_VERTEX_FLOATS;
        assert_eq!(vertices.len() % stride, 0);
        let start = u32::try_from(vertices.len() / stride).expect("tile vertex index");
        assert!(start <= u32::MAX - 7, "tile vertex indices exceed the model index format");

        let [ax, ay, az] = self.atlas_origin;
        let [ex, ey, ez] = self.extent;
        // The palette page shares lane 6 with the extent: the traced vertex stage hands lane 6 to
        // `v_trace_aux`, whose `w` the tile palette lookup already reads.
        let atlas = f32::from_bits(u32::from_le_bytes([ax, ay, az, 0]));
        let extent = f32::from_bits(u32::from_le_bytes([ex, ey, ez, self.palette_page]));
        for corner in 0..8 {
            let local = [0, 1, 2].map(|a| {
                if corner & (1 << a) == 0 { 0.0 } else { f32::from(self.extent[a]) }
            });
            let [px, py, pz] = [0, 1, 2].map(|a| {
                let grid = f64::from(self.origin_cells[a]) + f64::from(local[a]);
                (grid * f64::from(cell_pitch)) as f32
            });
            vertices.extend([px, py, pz, pack_pair_f16(local[2], 0.0),
                pack_pair_f16(local[0], local[1]), atlas, extent]);
        }
        for [a, b, c, d] in [
            [0, 2, 3, 1], [4, 5, 7, 6], [0, 4, 6, 2],
            [1, 3, 7, 5], [0, 1, 5, 4], [2, 6, 7, 3],
        ] {
            indices.extend([a, b, c, a, c, d].map(|i| start + i));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIMS: [u32; 3] = [64; 3];

    struct Rng(u32);

    impl Rng {
        fn next(&mut self) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            self.0
        }

        fn unit(&mut self) -> f32 {
            (self.next() >> 8) as f32 / 16_777_216.0
        }
    }

    // Independent oracle: intersect unit boxes in page space. Share the 1-D slabs
    // between boxes on the same row, column or layer to keep the exhaustive test small.
    fn slab(p: f32, d: f32, lo: f32, hi: f32) -> (f32, f32) {
        if d > 0.0 {
            ((lo - p) / d, (hi - p) / d)
        } else if d < 0.0 {
            ((hi - p) / d, (lo - p) / d)
        } else if lo <= p && p < hi {
            (f32::NEG_INFINITY, f32::INFINITY)
        } else {
            (f32::INFINITY, f32::NEG_INFINITY)
        }
    }

    fn brute_force(tile: &TracedVolumeTile, occupied: &[[u32; 3]], p: [f32; 3], d: [f32; 3])
        -> (Option<([u32; 3], f32, u8)>, bool)
    {
        let mut slabs = [[(0.0, 0.0); 64]; 3];
        let mut t_start = 0.0_f32;
        let mut t_end = f32::INFINITY;
        for a in 0..3 {
            let lo = usize::from(tile.atlas_origin[a]);
            let hi = lo + usize::from(tile.extent[a]);
            let origin = p[a] + lo as f32;
            let (entry, exit) = slab(origin, d[a], lo as f32, hi as f32);
            t_start = t_start.max(entry);
            t_end = t_end.min(exit);
            for q in lo..hi {
                slabs[a][q] = slab(origin, d[a], q as f32, (q + 1) as f32);
            }
        }
        if t_start >= t_end {
            return (None, false);
        }
        let mut hit = None;
        let mut best = f32::INFINITY;
        let mut second = f32::INFINITY;
        for &[x, y, z] in occupied {
            let (x0, x1) = slabs[0][x as usize];
            let (y0, y1) = slabs[1][y as usize];
            let (z0, z1) = slabs[2][z as usize];
            let cell_entry = x0.max(y0).max(z0);
            let t = t_start.max(cell_entry);
            if t >= x1.min(y1).min(z1) {
                continue;
            }
            if t < best {
                second = best;
                best = t;
                let axis = if x0 == cell_entry { 0 } else if y0 == cell_entry { 1 } else { 2 };
                hit = Some(([x, y, z], t, axis));
            } else {
                second = second.min(t);
            }
        }
        (hit, hit.is_some() && second - best <= 1e-5)
    }

    fn random_ray(rng: &mut Rng, i: usize, extent: [u8; 3]) -> ([f32; 3], [f32; 3]) {
        let e = extent.map(f32::from);
        let mut p = e.map(|size| rng.unit() * size);
        let a = i % 3;
        if i % 2 == 0 {
            p[a] = if i % 4 == 0 { -1.0 - p[a] } else { e[a] + p[a] };
        }
        let mut d = [0, 1, 2].map(|b| {
            if i % 4 == 0 { rng.unit() * e[b] - p[b] } else { rng.unit() * 2.0 - 1.0 }
        });
        match i % 7 {
            0 => {
                d = [0.0; 3];
                d[a] = if i % 2 == 0 { 1.0 } else { -1.0 };
            }
            1 => d[a] = 0.0,
            2 => d = d.map(|v| if v >= 0.0 { 1.0 } else { -1.0 }),
            3 => {
                // Graze a unit plane with both signs on the moving axes.
                p[a] = p[a].floor();
                d[a] = 0.0;
            }
            _ => {}
        }
        (p, d)
    }

    #[test]
    fn march_matches_brute_force_and_fine_only() {
        let mut rng = Rng(0xec19_5f3a);
        let mut cells = vec![0; 64 * 64 * 64];
        let mut comparisons = Vec::new();
        for (name, extent, atlas_origin) in [
            ("empty", [9, 8, 7], [5, 6, 7]),
            ("isolated", [9, 8, 7], [5, 6, 7]),
            ("long run", [32, 8, 8], [8, 12, 16]),
            ("wall", [8, 9, 10], [12, 8, 4]),
            ("door", [8, 8, 8], [4, 8, 12]),
            ("random 30%", [8, 8, 8], [13, 9, 5]),
            ("64³", [64, 64, 64], [0, 0, 0]),
        ] {
            let tile = TracedVolumeTile { origin_cells: [0; 3], extent, atlas_origin, palette_page: 0 };
            let tile_cells = u32::from(*extent.iter().max().unwrap());
            let mut occupied = Vec::new();
            cells.fill(0);
            for z in 0..extent[2] {
                for y in 0..extent[1] {
                    for x in 0..extent[0] {
                        let solid = match name {
                            "empty" => false,
                            "isolated" => [x, y, z] == [6, 6, 3],
                            "long run" => [x, y, z] == [30, 1, 1],
                            "wall" => (4..7).contains(&x),
                            "door" => {
                                let shell = [x, y, z].iter().any(|&q| q == 0 || q == 7);
                                let door = x == 0 && (1..6).contains(&y) && (3..5).contains(&z);
                                shell && !door
                            }
                            "random 30%" => rng.next() % 10 < 3,
                            "64³" => [x, y, z] == [63; 3] || [x, y, z] == [0, 1, 0],
                            _ => unreachable!(),
                        };
                        if solid {
                            let cell = [0, 1, 2].map(|a| u32::from(atlas_origin[a] + [x, y, z][a]));
                            cells[cell_index(cell, DIMS)] = (1 + rng.next() % 255) as u8;
                            occupied.push(cell);
                        }
                    }
                }
            }
            if extent[0] < 64 {
                // A neighbouring tile may set bits in the same coarse block.
                let [x, y, z] = atlas_origin.map(u32::from);
                cells[cell_index([x + u32::from(extent[0]), y, z], DIMS)] = 255;
            }
            let coarse = build_coarse(&cells, DIMS);
            let page = VolumePage { dims: DIMS, cells: &cells, coarse: &coarse };
            let mut rays = vec![
                ([4.5, 4.5, 3.5], [1.0, 1.0, 0.0]), // simultaneous fine X and coarse Y crossing
                ([-1.0, 1.5, 1.5], [1.0, 0.0, 0.0]), // whole 4³ blocks then a 2³ child
                ([-1.0, 3.5, 3.5], [1.0, 0.0, 0.0]), // outside looking through the door
                ([3.5; 3], [-1.0, 0.0, 0.0]),        // inside looking out through the door
                ([3.5; 3], [0.0, 1.0, 0.0]),         // inside looking at the ceiling
                ([0.125, 0.375, 0.625], [1.0; 3]),    // nearly 3T fine steps to the far corner
                (extent.map(f32::from), [-1.0; 3]),   // enter from the exclusive upper corner
                ([7.0, 7.0, 4.0], [-1.0, -1.0, 0.0]),
                ([-1.0, extent[1] as f32, 1.0], [1.0, 0.0, 0.0]),
            ];
            rays.extend((0..2048).map(|i| random_ray(&mut rng, i, extent)));
            let mut skip_steps = Vec::new();
            let mut fine_steps = Vec::new();
            let mut ties = 0;
            let bound: u32 = extent.iter().map(|&v| u32::from(v)).sum();
            for (i, &(p, d)) in rays.iter().enumerate() {
                let skipped = tile.march_counted(&page, p, d, tile_cells, true);
                let fine = tile.march_counted(&page, p, d, tile_cells, false);
                let steps = |r: Result<VolumeHit, u32>| match r {
                    Ok(hit) => hit.steps,
                    Err(count) => count,
                };
                assert!(steps(skipped) <= bound && steps(fine) <= bound,
                    "{name} ray {i} {p:?} {d:?}: bound {bound}, skip {skipped:?}, fine {fine:?}");
                skip_steps.push(steps(skipped));
                fine_steps.push(steps(fine));
                let (brute, near_tie) = brute_force(&tile, &occupied, p, d);
                ties += usize::from(near_tie);
                comparisons.push((name, i, p, d, skipped.ok(), fine.ok(), brute, near_tie));
            }
            fn histogram(steps: &mut [u32]) -> [u32; 3] {
                steps.sort_unstable();
                [steps[steps.len().div_ceil(2) - 1], steps[(steps.len() * 95).div_ceil(100) - 1], steps[steps.len() - 1]]
            }
            println!("M1 {name}: p50/p95/max skip {:?}, fine {:?}; near ties {ties}/{}",
                histogram(&mut skip_steps), histogram(&mut fine_steps), rays.len());
            assert!(ties * 100 < rays.len(), "{name}: {ties}/{} ambiguous rays", rays.len());
        }

        // Collect all fixture measurements before reporting the first counterexample.
        for (name, i, p, d, skipped, fine, brute, near_tie) in comparisons {
            let agrees = match (skipped, fine, brute) {
                (None, None, None) => true,
                (Some(s), Some(f), Some((cell, t, axis))) => {
                    (s.t - f.t).abs() <= 1e-4 && (f.t - t).abs() <= 1e-4
                        && s.material == f.material
                        && (near_tie || (s.cell == f.cell && s.axis == f.axis && f.cell == cell && f.axis == axis))
                }
                _ => false,
            };
            assert!(agrees, "{name} ray {i}: p={p:?} d={d:?}, skip={skipped:?}, fine={fine:?}, brute={brute:?}, near_tie={near_tie}");
        }
    }

    #[test]
    fn append_box_triangles_face_outward_ccw() {
        use makepad_draw::{vec3f, Vec3f};
        let tile = TracedVolumeTile { origin_cells: [-3, 9, 1], extent: [5, 3, 7], atlas_origin: [0; 3], palette_page: 0 };
        let (mut vertices, mut indices) = (Vec::new(), Vec::new());
        tile.append_box(&mut vertices, &mut indices, 1.0);
        let corner = |i: u32| {
            let n = i as usize * crate::model::MODEL_VERTEX_FLOATS;
            vec3f(vertices[n], vertices[n + 1], vertices[n + 2])
        };
        let center = (corner(0) + corner(7)) * 0.5;
        assert_eq!(indices.len(), 36);
        for tri in indices.chunks_exact(3) {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(corner);
            assert!(Vec3f::cross(b - a, c - a).dot(a - center) > 0.0, "inward or degenerate triangle {tri:?}");
        }
    }

    #[test]
    fn proxy_lanes_round_trip() {
        use makepad_draw::vector::unpack_pair_f16;

        let tile = TracedVolumeTile {
            origin_cells: [16_777_215, -121, 903],
            extent: [3, 64, 17],
            atlas_origin: [4, 192, 9],
            palette_page: 5,
        };
        let pitch = 0.1_f32;
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        tile.append_box(&mut vertices, &mut indices, pitch);
        assert_eq!(vertices.len(), 8 * 7, "proxy vertex stride");
        assert_eq!(indices.len(), 12 * 3, "proxy triangles");
        for (v, [x, y, z]) in vertices.chunks_exact(7).zip([
            [0, 0, 0], [3, 0, 0], [0, 64, 0], [3, 64, 0],
            [0, 0, 17], [3, 0, 17], [0, 64, 17], [3, 64, 17],
        ]) {
            assert_eq!(unpack_pair_f16(v[3]), (z as f32, 0.0), "local z");
            assert_eq!(unpack_pair_f16(v[4]), (x as f32, y as f32), "local xy");
            assert_eq!(v[5].to_bits().to_le_bytes(), [4, 192, 9, 0], "atlas");
            assert_eq!(v[6].to_bits().to_le_bytes(), [3, 64, 17, 5], "extent and palette");
            let grid = [16_777_215 + x, -121 + y, 903 + z];
            assert_eq!(v[..3], grid.map(|n| (f64::from(n) * f64::from(pitch)) as f32), "position");
        }
        let next = TracedVolumeTile { origin_cells: [16_777_218, -121, 903], ..tile };
        next.append_box(&mut vertices, &mut indices, pitch);
        for pair in 0..4 {
            let left = 7 * (2 * pair + 1);
            let right = 7 * (8 + 2 * pair);
            for a in 0..3 {
                assert_eq!(vertices[left + a].to_bits(), vertices[right + a].to_bits(),
                    "shared corner {pair}, axis {a}");
            }
        }
        for i in 0..36 {
            assert_eq!(indices[36 + i], indices[i] + 8, "second box index {i}");
        }
    }

    #[test]
    fn coarse_patch_matches_rebuild() {
        let check_brick = |occ: [u64; 8]| {
            let cells: [u8; 512] = std::array::from_fn(|i| ((occ[i / 64] >> (i % 64)) & 1) as u8);
            assert_eq!(brick_coarse_bytes(occ).as_slice(), build_coarse(&cells, [8; 3]), "brick occupancy {occ:x?}");
        };
        check_brick([0; 8]);
        check_brick([u64::MAX; 8]);
        for i in 0..512 {
            // Walk a single occupied cell through every corner, edge and interior child.
            let mut occ = [0; 8];
            occ[i / 64] = 1 << (i % 64);
            check_brick(occ);
        }
        let mut bricks = Rng(0x5f31_b096);
        for _ in 0..16 {
            check_brick(std::array::from_fn(|_| u64::from(bricks.next()) << 32 | u64::from(bricks.next())));
        }
        let mut rng = Rng(0x8ac4_2197);
        let mut cells = vec![0; 64 * 64 * 64];
        let mut coarse = build_coarse(&cells, DIMS);
        let block = cell_index([1, 2, 3], [16; 3]);
        for (bit, offset) in [
            [0, 0, 0], [3, 1, 1], [1, 2, 0], [2, 3, 1],
            [1, 0, 2], [2, 1, 3], [0, 3, 2], [3, 2, 3],
        ].into_iter().enumerate() {
            let q = [4 + offset[0], 8 + offset[1], 12 + offset[2]];
            let end = q.map(|v| v + 1);
            cells[cell_index(q, DIMS)] = 17;
            patch_coarse(&cells, DIMS, &mut coarse, q, end);
            assert_eq!(coarse[block], 1 << bit, "child bit {bit}");
            cells[cell_index(q, DIMS)] = 0;
            patch_coarse(&cells, DIMS, &mut coarse, q, end);
            assert_eq!(coarse[block], 0, "clear child {bit}");
        }

        // An unrelated byte makes an accidental whole-page rewrite observable.
        coarse[block] = 255;
        patch_coarse(&cells, DIMS, &mut coarse, [4, 9, 12], [7, 9, 16]);
        patch_coarse(&cells, DIMS, &mut coarse, [8, 8, 12], [12, 12, 16]);
        assert_eq!(coarse[block], 255, "empty or adjacent edit touched the block");
        coarse[block] = 0;

        for edit in 0..10 {
            let lo = [0; 3].map(|_| rng.next() % 64);
            let hi = lo.map(|q| (q + 1 + rng.next() % 12).min(64));
            let material = (1 + rng.next() % 255) as u8;
            for value in [material, 0] {
                for z in lo[2]..hi[2] {
                    for y in lo[1]..hi[1] {
                        let row = cell_index([lo[0], y, z], DIMS);
                        cells[row..row + (hi[0] - lo[0]) as usize].fill(value);
                    }
                }
                patch_coarse(&cells, DIMS, &mut coarse, lo, hi);
                assert_eq!(coarse, build_coarse(&cells, DIMS), "edit {edit}, value {value}");
            }
        }
    }
}

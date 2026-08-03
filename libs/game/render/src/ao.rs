//! Baked ambient occlusion — the light-independent half of shading.
//!
//! Two kinds, both computed once and both costing one multiply at runtime:
//!
//! * **Model self-AO** ([`bake_vertex_ao`]) — occlusion *within* one mesh: the
//!   crevice under a roof eave, the inside of an archway, the gap between a
//!   bench's slats. Baked at load into a spare vertex lane.
//! * **Contact AO** (see [`crate::shadow_mesh::build_contact_ao`]) — the dark
//!   skirt where a prop meets the ground.
//!
//! Neither depends on the sun, which is the point. A cast shadow swings and
//! shortens as the day cycles; the darkness inside an archway does not. That
//! constancy is what stops a prop reading as a decal laid over the ground.
//!
//! This is deliberately NOT [`crate::bake`], which occludes objects against
//! *each other* per instance. Same final multiply, different scale — the two
//! compose.

use makepad_draw::makepad_math::Vec3f;

/// Rays per vertex. Kenney models average 294 triangles, so the cost here is
/// dominated by ray count rather than mesh size; 12 is where the banding on a
/// flat wall stops being visible.
pub const AO_RAYS: usize = 12;

/// Occlusion search radius, as a fraction of the model's largest dimension.
///
/// AO is a *local* effect: a wall does not darken because there is another
/// wall twenty metres away. Keeping the radius short is both more truthful and
/// what makes the grid acceleration worthwhile, since a ray then crosses only
/// a few cells.
pub const AO_RADIUS_FRAC: f32 = 0.30;

/// How dark full occlusion is allowed to get.
///
/// Physically this would be 0. On flat low-poly art 0 reads as dirt — Kenney's
/// charm is clean colour, so AO should imply depth rather than smear it. The
/// darkest crevice lands here instead.
pub const AO_FLOOR: f32 = 0.52;

/// Grid cells along the longest axis. Small models get a coarse grid, which is
/// fine — the win is skipping most triangles, not perfect bucketing.
const GRID_DIM: usize = 8;

/// Above this vertex count the ray budget is halved, and halved again past
/// twice it.
///
/// Cost is vertices x rays x triangles-near-the-ray, so the handful of large
/// interior kits (`room-large.glb` and friends, ~18k vertices) cost two orders
/// of magnitude more than the 294-triangle median prop. Those meshes are also
/// the ones where AO matters least per vertex — a wall subdivided into
/// hundreds of quads gets its shape from neighbouring vertices agreeing, not
/// from each one being individually precise. Trading rays for load time there
/// is the cheap direction.
const AO_BIG_MESH_VERTS: usize = 4_000;

/// Golden angle, for the Fibonacci hemisphere. A fixed low-discrepancy set
/// rather than random directions: the bake must be deterministic, or two
/// devices loading the same model would shade it differently.
const GOLDEN_ANGLE: f32 = 2.399_963_2;

/// A uniform grid over a mesh's triangles. Rebuilt per model and dropped
/// straight after — it exists only for the duration of one bake.
struct TriGrid {
    min: Vec3f,
    inv_cell: Vec3f,
    dim: [usize; 3],
    /// Triangle indices, bucketed by cell; `starts` indexes into it.
    items: Vec<u32>,
    starts: Vec<u32>,
}

impl TriGrid {
    fn build(positions: &[Vec3f], indices: &[u32], min: Vec3f, max: Vec3f) -> TriGrid {
        let span = vec_max3(max.x - min.x, max.y - min.y, max.z - min.z).max(1.0e-4);
        let cell = span / GRID_DIM as f32;
        let dim = [
            (((max.x - min.x) / cell).ceil() as usize).clamp(1, GRID_DIM),
            (((max.y - min.y) / cell).ceil() as usize).clamp(1, GRID_DIM),
            (((max.z - min.z) / cell).ceil() as usize).clamp(1, GRID_DIM),
        ];
        let inv_cell = Vec3f {
            x: dim[0] as f32 / (max.x - min.x).max(1.0e-4),
            y: dim[1] as f32 / (max.y - min.y).max(1.0e-4),
            z: dim[2] as f32 / (max.z - min.z).max(1.0e-4),
        };
        let cells = dim[0] * dim[1] * dim[2];

        // Count then fill: two passes over the triangles, but no per-cell Vec
        // allocation. Same reason the mover-separation grid is built this way.
        let tri_count = indices.len() / 3;
        let mut counts = vec![0u32; cells + 1];
        let mut spans: Vec<(usize, usize, usize, usize, usize, usize)> =
            Vec::with_capacity(tri_count);
        for t in 0..tri_count {
            let (lo, hi) = tri_bounds(positions, indices, t);
            let a = cell_of(lo, min, inv_cell, dim);
            let b = cell_of(hi, min, inv_cell, dim);
            spans.push((a[0], a[1], a[2], b[0], b[1], b[2]));
            for z in a[2]..=b[2] {
                for y in a[1]..=b[1] {
                    for x in a[0]..=b[0] {
                        counts[cell_index(x, y, z, dim) + 1] += 1;
                    }
                }
            }
        }
        for i in 1..counts.len() {
            counts[i] += counts[i - 1];
        }
        let starts = counts.clone();
        let mut cursor = counts;
        let mut items = vec![0u32; starts[cells] as usize];
        for (t, s) in spans.iter().enumerate() {
            for z in s.2..=s.5 {
                for y in s.1..=s.4 {
                    for x in s.0..=s.3 {
                        let ci = cell_index(x, y, z, dim);
                        items[cursor[ci] as usize] = t as u32;
                        cursor[ci] += 1;
                    }
                }
            }
        }
        TriGrid {
            min,
            inv_cell,
            dim,
            items,
            starts,
        }
    }

    /// Any triangle blocking `origin -> dir` within `max_t`?
    ///
    /// Walks the cells the ray's own bounding box touches rather than a proper
    /// DDA: the radius is short, so that box is a handful of cells, and the
    /// simpler code cannot get the traversal order subtly wrong.
    fn occluded(
        &self,
        positions: &[Vec3f],
        indices: &[u32],
        origin: Vec3f,
        dir: Vec3f,
        max_t: f32,
    ) -> bool {
        let end = Vec3f {
            x: origin.x + dir.x * max_t,
            y: origin.y + dir.y * max_t,
            z: origin.z + dir.z * max_t,
        };
        let lo = Vec3f {
            x: origin.x.min(end.x),
            y: origin.y.min(end.y),
            z: origin.z.min(end.z),
        };
        let hi = Vec3f {
            x: origin.x.max(end.x),
            y: origin.y.max(end.y),
            z: origin.z.max(end.z),
        };
        let a = cell_of(lo, self.min, self.inv_cell, self.dim);
        let b = cell_of(hi, self.min, self.inv_cell, self.dim);
        for z in a[2]..=b[2] {
            for y in a[1]..=b[1] {
                for x in a[0]..=b[0] {
                    let ci = cell_index(x, y, z, self.dim);
                    let s = self.starts[ci] as usize;
                    let e = self.starts[ci + 1] as usize;
                    for &t in &self.items[s..e] {
                        if ray_hits_tri(positions, indices, t as usize, origin, dir, max_t) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
}

fn vec_max3(a: f32, b: f32, c: f32) -> f32 {
    a.max(b).max(c)
}

fn cell_index(x: usize, y: usize, z: usize, dim: [usize; 3]) -> usize {
    (z * dim[1] + y) * dim[0] + x
}

fn cell_of(p: Vec3f, min: Vec3f, inv_cell: Vec3f, dim: [usize; 3]) -> [usize; 3] {
    [
        (((p.x - min.x) * inv_cell.x) as isize).clamp(0, dim[0] as isize - 1) as usize,
        (((p.y - min.y) * inv_cell.y) as isize).clamp(0, dim[1] as isize - 1) as usize,
        (((p.z - min.z) * inv_cell.z) as isize).clamp(0, dim[2] as isize - 1) as usize,
    ]
}

fn tri_bounds(positions: &[Vec3f], indices: &[u32], t: usize) -> (Vec3f, Vec3f) {
    let p = |k: usize| positions[indices[t * 3 + k] as usize];
    let (a, b, c) = (p(0), p(1), p(2));
    (
        Vec3f {
            x: a.x.min(b.x).min(c.x),
            y: a.y.min(b.y).min(c.y),
            z: a.z.min(b.z).min(c.z),
        },
        Vec3f {
            x: a.x.max(b.x).max(c.x),
            y: a.y.max(b.y).max(c.y),
            z: a.z.max(b.z).max(c.z),
        },
    )
}

/// Möller–Trumbore, double-sided. Double-sided matters: Kenney models are not
/// reliably closed, and a one-sided test would let rays escape through the
/// back of a wall and report a crevice as open sky.
fn ray_hits_tri(
    positions: &[Vec3f],
    indices: &[u32],
    t: usize,
    origin: Vec3f,
    dir: Vec3f,
    max_t: f32,
) -> bool {
    let v0 = positions[indices[t * 3] as usize];
    let v1 = positions[indices[t * 3 + 1] as usize];
    let v2 = positions[indices[t * 3 + 2] as usize];
    let e1 = sub(v1, v0);
    let e2 = sub(v2, v0);
    let p = cross(dir, e2);
    let det = dot(e1, p);
    if det.abs() < 1.0e-9 {
        return false;
    }
    let inv_det = 1.0 / det;
    let tv = sub(origin, v0);
    let u = dot(tv, p) * inv_det;
    if !(-1.0e-5..=1.000_01).contains(&u) {
        return false;
    }
    let q = cross(tv, e1);
    let v = dot(dir, q) * inv_det;
    if v < -1.0e-5 || u + v > 1.000_01 {
        return false;
    }
    let hit = dot(e2, q) * inv_det;
    hit > 1.0e-4 && hit < max_t
}

fn sub(a: Vec3f, b: Vec3f) -> Vec3f {
    Vec3f {
        x: a.x - b.x,
        y: a.y - b.y,
        z: a.z - b.z,
    }
}

fn cross(a: Vec3f, b: Vec3f) -> Vec3f {
    Vec3f {
        x: a.y * b.z - a.z * b.y,
        y: a.z * b.x - a.x * b.z,
        z: a.x * b.y - a.y * b.x,
    }
}

fn dot(a: Vec3f, b: Vec3f) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

/// Cosine-weighted hemisphere directions about `n`, as a fixed Fibonacci set.
/// Fills the first `rays` slots; the caller reads only those.
fn hemisphere(n: Vec3f, rays: usize, out: &mut [Vec3f; AO_RAYS]) {
    // Any vector not parallel to n gives a usable tangent frame; picking the
    // axis n is *least* aligned with avoids the degenerate case.
    let up = if n.y.abs() < 0.9 {
        Vec3f { x: 0.0, y: 1.0, z: 0.0 }
    } else {
        Vec3f { x: 1.0, y: 0.0, z: 0.0 }
    };
    let t = normalize(cross(up, n));
    let b = cross(n, t);
    for (i, slot) in out.iter_mut().enumerate().take(rays) {
        // Distribute over `rays`, not AO_RAYS: a reduced budget must still
        // cover the whole hemisphere, or a dense mesh would sample only the
        // directions nearest its normal and read as uniformly unoccluded.
        let u = (i as f32 + 0.5) / rays as f32;
        // sqrt weighting is what makes the set cosine-distributed, which is
        // the correct importance for a diffuse occlusion term.
        let cos_theta = (1.0 - u).sqrt();
        let sin_theta = u.sqrt();
        let phi = i as f32 * GOLDEN_ANGLE;
        let (sp, cp) = (phi.sin(), phi.cos());
        *slot = Vec3f {
            x: t.x * (cp * sin_theta) + b.x * (sp * sin_theta) + n.x * cos_theta,
            y: t.y * (cp * sin_theta) + b.y * (sp * sin_theta) + n.y * cos_theta,
            z: t.z * (cp * sin_theta) + b.z * (sp * sin_theta) + n.z * cos_theta,
        };
    }
}

fn normalize(v: Vec3f) -> Vec3f {
    let l = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
    if l < 1.0e-8 {
        return Vec3f { x: 0.0, y: 1.0, z: 0.0 };
    }
    Vec3f {
        x: v.x / l,
        y: v.y / l,
        z: v.z / l,
    }
}

/// Per-vertex ambient occlusion in `[AO_FLOOR, 1]`, one entry per position.
///
/// Rays start slightly off the surface along the normal — starting exactly on
/// it makes a vertex's own triangles report a hit at t≈0 and every vertex come
/// back fully black.
pub fn bake_vertex_ao(
    positions: &[Vec3f],
    normals: &[Vec3f],
    indices: &[u32],
    min: Vec3f,
    max: Vec3f,
) -> Vec<f32> {
    let count = positions.len();
    if count == 0 || indices.len() < 3 {
        return vec![1.0; count];
    }
    let span = vec_max3(max.x - min.x, max.y - min.y, max.z - min.z);
    if span <= 1.0e-5 {
        return vec![1.0; count];
    }
    let radius = span * AO_RADIUS_FRAC;
    let epsilon = span * 1.0e-3;
    let grid = TriGrid::build(positions, indices, min, max);

    // Ray budget, reduced on the few very dense meshes (see AO_BIG_MESH_VERTS).
    let rays = if count > AO_BIG_MESH_VERTS * 2 {
        AO_RAYS / 4
    } else if count > AO_BIG_MESH_VERTS {
        AO_RAYS / 2
    } else {
        AO_RAYS
    }
    .max(3);

    let mut out = Vec::with_capacity(count);
    let mut dirs = [Vec3f { x: 0.0, y: 1.0, z: 0.0 }; AO_RAYS];
    for i in 0..count {
        let n = normals.get(i).copied().unwrap_or(Vec3f { x: 0.0, y: 1.0, z: 0.0 });
        let n = normalize(n);
        let origin = Vec3f {
            x: positions[i].x + n.x * epsilon,
            y: positions[i].y + n.y * epsilon,
            z: positions[i].z + n.z * epsilon,
        };
        hemisphere(n, rays, &mut dirs);
        let mut blocked = 0usize;
        for d in dirs.iter().take(rays) {
            if grid.occluded(positions, indices, origin, *d, radius) {
                blocked += 1;
            }
        }
        let open = 1.0 - blocked as f32 / rays as f32;
        // Remap into [AO_FLOOR, 1] rather than using `open` directly.
        out.push(AO_FLOOR + open * (1.0 - AO_FLOOR));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32, z: f32) -> Vec3f {
        Vec3f { x, y, z }
    }

    /// A lone flat quad has nothing to occlude it, so every vertex must come
    /// back fully open. This is the test that catches self-intersection at the
    /// ray origin — get the epsilon wrong and the whole model goes black.
    #[test]
    fn an_unoccluded_surface_stays_fully_lit() {
        let positions = vec![v(-1.0, 0.0, -1.0), v(1.0, 0.0, -1.0), v(1.0, 0.0, 1.0), v(-1.0, 0.0, 1.0)];
        let normals = vec![v(0.0, 1.0, 0.0); 4];
        let indices = vec![0, 1, 2, 0, 2, 3];
        let ao = bake_vertex_ao(&positions, &normals, &indices, v(-1.0, 0.0, -1.0), v(1.0, 0.0, 1.0));
        for a in ao {
            assert!(a > 0.99, "open surface darkened to {a}");
        }
    }

    /// A floor with a lid close above it: the floor is in a crevice and must
    /// darken, which is the entire point of the feature.
    #[test]
    fn a_surface_under_an_overhang_darkens() {
        // Floor quad at y=0, lid quad at y=0.3 — well inside the search radius.
        let positions = vec![
            v(-1.0, 0.0, -1.0), v(1.0, 0.0, -1.0), v(1.0, 0.0, 1.0), v(-1.0, 0.0, 1.0),
            v(-1.0, 0.3, -1.0), v(1.0, 0.3, -1.0), v(1.0, 0.3, 1.0), v(-1.0, 0.3, 1.0),
        ];
        let mut normals = vec![v(0.0, 1.0, 0.0); 4];
        normals.extend(vec![v(0.0, -1.0, 0.0); 4]);
        let indices = vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7];
        let ao = bake_vertex_ao(&positions, &normals, &indices, v(-1.0, 0.0, -1.0), v(1.0, 0.3, 1.0));
        // Vertex 0 is a floor corner under the lid.
        assert!(ao[0] < 0.9, "floor under an overhang not darkened: {}", ao[0]);
    }

    /// Never fully black: full occlusion reads as dirt on flat low-poly art,
    /// so the darkest value is clamped by design rather than by accident.
    ///
    /// Note the geometry: two nearly-touching plates, not a vertex centred in
    /// a box. AO here is deliberately LOCAL (radius is 30% of the model span),
    /// so a vertex at the middle of a cube is further from its own walls than
    /// the search reaches and is correctly unoccluded. A crevice is what the
    /// feature is for, and a crevice is what this has to test.
    #[test]
    fn occlusion_never_reaches_black() {
        // Vertex 0 is a probe at the CENTRE of the crevice, so the lid covers
        // its whole hemisphere. A corner vertex would sit half off the plate
        // and be legitimately only ~50% occluded — real behaviour, but not
        // what this test is asking about.
        let positions = vec![
            v(0.0, 0.0, 0.0),
            v(-1.0, 0.04, -1.0), v(1.0, 0.04, -1.0), v(1.0, 0.04, 1.0), v(-1.0, 0.04, 1.0),
        ];
        let mut normals = vec![v(0.0, 1.0, 0.0)];
        normals.extend(vec![v(0.0, -1.0, 0.0); 4]);
        let indices = vec![1, 2, 3, 1, 3, 4];
        let ao = bake_vertex_ao(&positions, &normals, &indices, v(-1.0, 0.0, -1.0), v(1.0, 0.04, 1.0));
        assert!(ao[0] >= AO_FLOOR - 1.0e-6, "clamped below the floor: {}", ao[0]);
        assert!(
            ao[0] < AO_FLOOR + 0.05,
            "vertex in a tight crevice barely darkened: {}",
            ao[0]
        );
    }

    /// Determinism: no RNG anywhere, so two bakes of one mesh must agree
    /// bit-for-bit or two devices would shade the same model differently.
    #[test]
    fn the_bake_is_deterministic() {
        let positions = vec![v(-1.0, 0.0, -1.0), v(1.0, 0.0, -1.0), v(1.0, 0.0, 1.0), v(-1.0, 0.5, 1.0)];
        let normals = vec![v(0.0, 1.0, 0.0); 4];
        let indices = vec![0, 1, 2, 0, 2, 3];
        let a = bake_vertex_ao(&positions, &normals, &indices, v(-1.0, 0.0, -1.0), v(1.0, 0.5, 1.0));
        let b = bake_vertex_ao(&positions, &normals, &indices, v(-1.0, 0.0, -1.0), v(1.0, 0.5, 1.0));
        assert_eq!(a, b);
    }

    #[test]
    fn degenerate_input_is_safe() {
        assert!(bake_vertex_ao(&[], &[], &[], v(0.0, 0.0, 0.0), v(0.0, 0.0, 0.0)).is_empty());
        // A single point with no triangles must not divide by a zero span.
        let ao = bake_vertex_ao(&[v(0.0, 0.0, 0.0)], &[v(0.0, 1.0, 0.0)], &[], v(0.0, 0.0, 0.0), v(0.0, 0.0, 0.0));
        assert_eq!(ao, vec![1.0]);
    }
}

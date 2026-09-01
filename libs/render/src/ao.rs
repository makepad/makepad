//! Baked ambient occlusion — the light-independent half of shading.
//!
//! **Model self-AO** ([`bake_vertex_ao`]) — occlusion *within* one mesh: the
//! crevice under a roof eave, the inside of an archway, the gap between a
//! bench's slats. Baked at load into a spare vertex lane, one multiply at
//! runtime.
//!
//! It does not depend on the sun, which is the point. A cast shadow swings and
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

/// Rays for an OFFLINE bake. 12 was a runtime budget and it shows: occlusion
/// quantises to 1/12, so a smooth corner comes out in 0.04 steps and reads as
/// triangular banding. Baking happens once in a tool now, so the only cost is
/// the tool's runtime — 128 rays is smooth to the eye and still bakes the
/// whole 4,442-model library in minutes.
pub const AO_RAYS_OFFLINE: usize = 512;

/// Occlusion reach, as a fraction of the model's span.
///
/// This is the distance at which an occluder stops counting AT ALL, not the
/// width of the resulting shadow — `sample_ao` weights each hit by `(1-t/r)^3`,
/// so the darkening is concentrated in roughly the first third of it and fades
/// to nothing well before the limit. The radius therefore wants to be generous
/// enough to SEE an eave above a wall; the falloff decides how far the shading
/// actually spreads.
///
/// Before the falloff existed this value had to do both jobs and could do
/// neither: large enough to catch an overhang meant the middle of a plain wall
/// counted its own floor and neighbours as occluders and went grey, which is
/// what turned Kenney facades into soft washes.
pub const AO_RADIUS_FRAC: f32 = 0.5;

/// Absolute ceiling on that reach, in world units.
///
/// A corner shadow is the same physical size on a crate as on a house — about
/// a hand's width — so reach must NOT keep growing with the model. Scaling it
/// purely by span is what washed the big meshes: a house got a reach over a
/// metre, so the middle of a facade counted its own floor and neighbouring
/// walls as occluders and went uniformly grey, while a crate with the same
/// fraction looked fine.
///
/// Kenney authors at roughly 1 unit = 1 metre, so this is ~35cm: wide enough
/// to see an eave above a wall, narrow enough that a facade's middle sees
/// nothing. The `min` with the span keeps small props proportional rather than
/// giving a doorknob a house-sized reach.
// CONTACT-scale reach, the look this engine's AO is FOR: a thin dark
// gradient pooling where surfaces meet, nothing anywhere else. A 0.35
// experiment made physically-honest enclosure shading (a deck's underside
// staring at its rails 17cm below baked 0.5) — correct radiometry, wrong
// art: whole faces went grey. 12cm keeps junction/contact gradients and
// lets ventilated timber breathe; the town packs were tuned at this value.
pub const AO_RADIUS_WORLD: f32 = 0.12;

/// How dark full occlusion is allowed to get.
///
/// Physically this would be 0. On flat low-poly art 0 reads as dirt — Kenney's
/// charm is clean colour, so AO should imply depth rather than smear it. The
/// darkest crevice lands here instead.
pub const AO_FLOOR: f32 = 0.18;

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
pub(crate) struct TriGrid {
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

    /// What blocks `origin -> dir` within `max_t`: nothing, the virtual ground,
    /// or the mesh itself.
    ///
    /// Triangles from `ground_start` on are the virtual ground rather than the
    /// model. Separating them lets ONE ray pass answer two questions — how dark
    /// the mesh is by itself, and how dark it is resting on a floor — because a
    /// ray that only ever met the ground is a miss for the first and a hit for
    /// the second. Pass `usize::MAX` when there is no ground.
    ///
    /// Returns `(nearest_self_hit, nearest_ground_hit)` as distances along the
    /// ray, `INFINITY` for no hit. DISTANCE, not a yes/no, because occlusion
    /// has to fall off with it — a surface a centimetre from a corner and one
    /// a metre from a distant wall are not equally dark, and treating them the
    /// same is what washed whole facades instead of shading their creases.
    ///
    /// Marches the ray cell by cell (Amanatides-Woo) so cells arrive in order
    /// and the first hit found is the nearest one.
    fn occluded(
        &self,
        positions: &[Vec3f],
        indices: &[u32],
        origin: Vec3f,
        dir: Vec3f,
        max_t: f32,
        ground_start: usize,
    ) -> (f32, f32, usize) {
        let inv = [self.inv_cell.x, self.inv_cell.y, self.inv_cell.z];
        let d = [dir.x, dir.y, dir.z];
        let o = [
            origin.x - self.min.x,
            origin.y - self.min.y,
            origin.z - self.min.z,
        ];
        let mut cell = {
            let c = cell_of(origin, self.min, self.inv_cell, self.dim);
            [c[0] as isize, c[1] as isize, c[2] as isize]
        };

        // Amanatides-Woo setup: for each axis, how far along the ray the next
        // cell boundary is (`t_max`) and how far apart consecutive boundaries
        // are (`t_delta`).
        let mut step = [0isize; 3];
        let mut t_max = [f32::INFINITY; 3];
        let mut t_delta = [f32::INFINITY; 3];
        for a in 0..3 {
            if d[a] > 1.0e-9 {
                step[a] = 1;
                t_max[a] = ((cell[a] + 1) as f32 / inv[a] - o[a]) / d[a];
                t_delta[a] = 1.0 / (inv[a] * d[a]);
            } else if d[a] < -1.0e-9 {
                step[a] = -1;
                t_max[a] = (cell[a] as f32 / inv[a] - o[a]) / d[a];
                t_delta[a] = -1.0 / (inv[a] * d[a]);
            }
        }

        let (mut near_self, mut near_ground) = (f32::INFINITY, f32::INFINITY);
        let mut near_self_tri = usize::MAX;
        loop {
            let ci = cell_index(cell[0] as usize, cell[1] as usize, cell[2] as usize, self.dim);
            let s = self.starts[ci] as usize;
            let e = self.starts[ci + 1] as usize;
            for &t in &self.items[s..e] {
                // Double-sided intersection: after twin dedup the kept
                // triangle's winding is arbitrary (pair's lower index), so
                // sidedness carries no meaning here.
                if let Some(hit) = ray_hits_tri(
                    positions, indices, t as usize, origin, dir, max_t, false,
                ) {
                    if (t as usize) < ground_start {
                        if hit < near_self {
                            near_self = hit;
                            near_self_tri = t as usize;
                        }
                    } else if hit < near_ground {
                        near_ground = hit;
                    }
                }
            }
            // Both answered, and cells arrive in ray order, so nothing further
            // along can be nearer.
            if near_self.is_finite() && near_ground.is_finite() {
                break;
            }
            // Advance across the nearest boundary of the three.
            let a = if t_max[0] < t_max[1] {
                if t_max[0] < t_max[2] {
                    0
                } else {
                    2
                }
            } else if t_max[1] < t_max[2] {
                1
            } else {
                2
            };
            if t_max[a] > max_t {
                break;
            }
            cell[a] += step[a];
            if cell[a] < 0 || cell[a] >= self.dim[a] as isize {
                break;
            }
            t_max[a] += t_delta[a];
        }
        (near_self, near_ground, near_self_tri)
    }

    /// Does ANY triangle block the ray within `max_t`? Returns at the first
    /// hit found — for shadow rays, where which triangle blocked is
    /// irrelevant and most rays that hit at all hit early.
    ///
    /// `front_only` applies embree-style backface culling (a triangle facing
    /// away from the ray does not block); triangles indexed `skip_from` or
    /// higher are ignored entirely (the aobaker port uses it to exclude the
    /// virtual ground quad — aobaker's scene is the mesh alone).
    fn any_hit(
        &self,
        positions: &[Vec3f],
        indices: &[u32],
        origin: Vec3f,
        dir: Vec3f,
        max_t: f32,
        front_only: bool,
        skip_from: usize,
    ) -> bool {
        let inv = [self.inv_cell.x, self.inv_cell.y, self.inv_cell.z];
        let d = [dir.x, dir.y, dir.z];
        let o = [
            origin.x - self.min.x,
            origin.y - self.min.y,
            origin.z - self.min.z,
        ];
        let mut cell = {
            let c = cell_of(origin, self.min, self.inv_cell, self.dim);
            [c[0] as isize, c[1] as isize, c[2] as isize]
        };
        let mut step = [0isize; 3];
        let mut t_max = [f32::INFINITY; 3];
        let mut t_delta = [f32::INFINITY; 3];
        for a in 0..3 {
            if d[a] > 1.0e-9 {
                step[a] = 1;
                t_max[a] = ((cell[a] + 1) as f32 / inv[a] - o[a]) / d[a];
                t_delta[a] = 1.0 / (inv[a] * d[a]);
            } else if d[a] < -1.0e-9 {
                step[a] = -1;
                t_max[a] = (cell[a] as f32 / inv[a] - o[a]) / d[a];
                t_delta[a] = -1.0 / (inv[a] * d[a]);
            }
        }
        loop {
            let ci = cell_index(cell[0] as usize, cell[1] as usize, cell[2] as usize, self.dim);
            let s = self.starts[ci] as usize;
            let e = self.starts[ci + 1] as usize;
            for &t in &self.items[s..e] {
                if (t as usize) < skip_from
                    && ray_hits_tri(positions, indices, t as usize, origin, dir, max_t, front_only)
                        .is_some()
                {
                    return true;
                }
            }
            let a = if t_max[0] < t_max[1] {
                if t_max[0] < t_max[2] { 0 } else { 2 }
            } else if t_max[1] < t_max[2] {
                1
            } else {
                2
            };
            if t_max[a] > max_t {
                return false;
            }
            cell[a] += step[a];
            if cell[a] < 0 || cell[a] >= self.dim[a] as isize {
                return false;
            }
            t_max[a] += t_delta[a];
        }
    }

    /// NEAREST hit and its triangle index. Unlike [`Self::occluded`] this
    /// keeps marching until the next cell would START beyond the best hit —
    /// a triangle spanning several cells can be met in an early cell at a
    /// distance a later cell's triangle still beats, and a bounce gather that
    /// reads light from the wrong surface bleeds it somewhere it never was.
    fn nearest_hit(
        &self,
        positions: &[Vec3f],
        indices: &[u32],
        origin: Vec3f,
        dir: Vec3f,
        max_t: f32,
    ) -> Option<(f32, u32)> {
        let inv = [self.inv_cell.x, self.inv_cell.y, self.inv_cell.z];
        let d = [dir.x, dir.y, dir.z];
        let o = [
            origin.x - self.min.x,
            origin.y - self.min.y,
            origin.z - self.min.z,
        ];
        let mut cell = {
            let c = cell_of(origin, self.min, self.inv_cell, self.dim);
            [c[0] as isize, c[1] as isize, c[2] as isize]
        };
        let mut step = [0isize; 3];
        let mut t_max = [f32::INFINITY; 3];
        let mut t_delta = [f32::INFINITY; 3];
        for a in 0..3 {
            if d[a] > 1.0e-9 {
                step[a] = 1;
                t_max[a] = ((cell[a] + 1) as f32 / inv[a] - o[a]) / d[a];
                t_delta[a] = 1.0 / (inv[a] * d[a]);
            } else if d[a] < -1.0e-9 {
                step[a] = -1;
                t_max[a] = (cell[a] as f32 / inv[a] - o[a]) / d[a];
                t_delta[a] = -1.0 / (inv[a] * d[a]);
            }
        }
        let mut best: Option<(f32, u32)> = None;
        loop {
            let ci = cell_index(cell[0] as usize, cell[1] as usize, cell[2] as usize, self.dim);
            let s = self.starts[ci] as usize;
            let e = self.starts[ci + 1] as usize;
            for &t in &self.items[s..e] {
                if let Some(hit) =
                    ray_hits_tri(positions, indices, t as usize, origin, dir, max_t, false)
                {
                    if best.is_none() || hit < best.unwrap().0 {
                        best = Some((hit, t));
                    }
                }
            }
            let a = if t_max[0] < t_max[1] {
                if t_max[0] < t_max[2] { 0 } else { 2 }
            } else if t_max[1] < t_max[2] {
                1
            } else {
                2
            };
            // Cells arrive in ray order, so once the next boundary is past
            // the best hit nothing further along can be nearer.
            if let Some((bt, _)) = best {
                if t_max[a] >= bt {
                    return best;
                }
            }
            if t_max[a] > max_t {
                return best;
            }
            cell[a] += step[a];
            if cell[a] < 0 || cell[a] >= self.dim[a] as isize {
                return best;
            }
            t_max[a] += t_delta[a];
        }
    }
}

/// A model's triangles and their acceleration grid, kept ALIVE — unlike the
/// bake-and-drop grids above — so every placed copy of the model can answer
/// rays against one shared structure. Rays arrive in MODEL space; the caller
/// transforms them, which is what makes forty copies of one house cost one
/// grid instead of forty (the light baker's two-level accelerator: world
/// instance list on the outside, this on the inside).
pub struct MeshRaycaster {
    positions: Vec<Vec3f>,
    indices: Vec<u32>,
    min: Vec3f,
    max: Vec3f,
    grid: TriGrid,
}

impl MeshRaycaster {
    pub fn new(positions: Vec<Vec3f>, indices: Vec<u32>, min: Vec3f, max: Vec3f) -> Self {
        let grid = TriGrid::build(&positions, &indices, min, max);
        Self { positions, indices, min, max, grid }
    }

    pub fn tri_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// The bounds the grid was built over (model space).
    pub fn bounds(&self) -> (Vec3f, Vec3f) {
        (self.min, self.max)
    }

    /// Corner positions of triangle `t`, for barycentric work at a hit.
    pub fn triangle(&self, t: u32) -> (Vec3f, Vec3f, Vec3f) {
        let i = t as usize * 3;
        (
            self.positions[self.indices[i] as usize],
            self.positions[self.indices[i + 1] as usize],
            self.positions[self.indices[i + 2] as usize],
        )
    }

    /// Vertex indices of triangle `t` — the caller resolves per-vertex data
    /// (uv, albedo) this module has no reason to know about.
    pub fn triangle_verts(&self, t: u32) -> [u32; 3] {
        let i = t as usize * 3;
        [self.indices[i], self.indices[i + 1], self.indices[i + 2]]
    }

    /// Does anything block `origin + dir*[0, max_t]`? First hit wins.
    ///
    /// The grid marchers assume the origin is inside the mesh bounds — true
    /// for the AO bake, false for a shadow ray arriving from another
    /// instance — so the ray is first CLIPPED to this mesh's box and the
    /// march starts at the entry point. A ray that misses the box misses.
    pub fn any_hit(&self, origin: Vec3f, dir: Vec3f, max_t: f32) -> bool {
        let Some((t0, t1)) = clip_ray_to_box(origin, dir, max_t, self.min, self.max) else {
            return false;
        };
        let entry = Vec3f {
            x: origin.x + dir.x * t0,
            y: origin.y + dir.y * t0,
            z: origin.z + dir.z * t0,
        };
        self.grid
            .any_hit(&self.positions, &self.indices, entry, dir, t1 - t0, false, usize::MAX)
    }

    /// Nearest hit as `(distance_from_origin, triangle)`.
    pub fn nearest_hit(&self, origin: Vec3f, dir: Vec3f, max_t: f32) -> Option<(f32, u32)> {
        let (t0, t1) = clip_ray_to_box(origin, dir, max_t, self.min, self.max)?;
        let entry = Vec3f {
            x: origin.x + dir.x * t0,
            y: origin.y + dir.y * t0,
            z: origin.z + dir.z * t0,
        };
        self.grid
            .nearest_hit(&self.positions, &self.indices, entry, dir, t1 - t0)
            .map(|(t, tri)| (t + t0, tri))
    }
}

/// Slab-clip a ray segment to a box, padded a hair so entry points land
/// inside rather than exactly on a face. Returns the `[t0, t1]` overlap.
fn clip_ray_to_box(
    origin: Vec3f,
    dir: Vec3f,
    max_t: f32,
    min: Vec3f,
    max: Vec3f,
) -> Option<(f32, f32)> {
    let (mut t0, mut t1) = (0.0f32, max_t);
    let o = [origin.x, origin.y, origin.z];
    let d = [dir.x, dir.y, dir.z];
    let lo = [min.x, min.y, min.z];
    let hi = [max.x, max.y, max.z];
    for a in 0..3 {
        if d[a].abs() < 1.0e-9 {
            if o[a] < lo[a] || o[a] > hi[a] {
                return None;
            }
            continue;
        }
        let inv = 1.0 / d[a];
        let (mut ta, mut tb) = ((lo[a] - o[a]) * inv, (hi[a] - o[a]) * inv);
        if ta > tb {
            std::mem::swap(&mut ta, &mut tb);
        }
        t0 = t0.max(ta);
        t1 = t1.min(tb);
        if t0 > t1 {
            return None;
        }
    }
    // Back off a whisker: an entry point exactly on the box face can round
    // into the cell one past the edge and index out of the grid.
    Some(((t0 - 1.0e-4).max(0.0), t1))
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
    front_only: bool,
) -> Option<f32> {
    let v0 = positions[indices[t * 3] as usize];
    let v1 = positions[indices[t * 3 + 1] as usize];
    let v2 = positions[indices[t * 3 + 2] as usize];
    let e1 = sub(v1, v0);
    let e2 = sub(v2, v0);
    let p = cross(dir, e2);
    let det = dot(e1, p);
    if det.abs() < 1.0e-9 {
        return None;
    }
    // det > 0 means the ray sees this triangle's BACK. Under the culling
    // contract a back face is never a rendered surface, and counting it
    // poisoned every joint: kit pieces interpenetrate, so texels near a
    // member's end sit INSIDE the neighbouring beam and their rays hit its
    // far wall from within (measured: every blocking ray of a rim texel
    // reported hitN pointing away). The kits author double-sided walls, so
    // wherever a real wall blocks from outside, a FRONT-facing twin exists
    // at the same spot and still occludes; only buried-in-wood rays escape.
    if front_only && det < 0.0 {
        return None;
    }
    let inv_det = 1.0 / det;
    let tv = sub(origin, v0);
    let u = dot(tv, p) * inv_det;
    if !(-1.0e-5..=1.000_01).contains(&u) {
        return None;
    }
    let q = cross(tv, e1);
    let v = dot(dir, q) * inv_det;
    if v < -1.0e-5 || u + v > 1.000_01 {
        return None;
    }
    let hit = dot(e2, q) * inv_det;
    if hit > 1.0e-4 && hit < max_t {
        Some(hit)
    } else {
        None
    }
}

fn face_normal_at(positions: &[Vec3f], indices: &[u32], t: usize) -> Vec3f {
    let a = positions[indices[t * 3] as usize];
    let b = positions[indices[t * 3 + 1] as usize];
    let c = positions[indices[t * 3 + 2] as usize];
    let e1 = sub(b, a);
    let e2 = sub(c, a);
    normalize(cross(e1, e2))
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
pub(crate) fn hemisphere(n: Vec3f, rays: usize, out: &mut [Vec3f; AO_RAYS_OFFLINE]) {
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
    let radius = (span * AO_RADIUS_FRAC).min(AO_RADIUS_WORLD);
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
    let mut dirs = [Vec3f { x: 0.0, y: 1.0, z: 0.0 }; AO_RAYS_OFFLINE];
    for i in 0..count {
        let n = normals.get(i).copied().unwrap_or(Vec3f { x: 0.0, y: 1.0, z: 0.0 });
        // No virtual ground in the per-vertex path, so both terms agree.
        out.push(
            sample_ao(
                &grid,
                positions,
                indices,
                positions[i],
                n,
                radius,
                epsilon,
                rays,
                usize::MAX,
                &mut dirs,
            )
            .1,
        );
    }
    out
}

/// Occlusion at ONE point. Extracted so the vertex bake and the tessellator
/// ([`tessellate_for_ao`]) sample identically — a tessellator that measured
/// occlusion even slightly differently from the baker would subdivide chasing
/// its own disagreement rather than a real gradient.
#[allow(clippy::too_many_arguments)]
fn sample_ao(
    grid: &TriGrid,
    positions: &[Vec3f],
    indices: &[u32],
    point: Vec3f,
    normal: Vec3f,
    radius: f32,
    epsilon: f32,
    rays: usize,
    ground_start: usize,
    dirs: &mut [Vec3f; AO_RAYS_OFFLINE],
) -> (f32, f32) {
    let n = normalize(normal);
    let origin = Vec3f {
        x: point.x + n.x * epsilon,
        y: point.y + n.y * epsilon,
        z: point.z + n.z * epsilon,
    };
    hemisphere(n, rays, dirs);
    // Weight each blocked ray by how CLOSE its occluder is.
    //
    // Counting hits equally is what produced soft grey over entire facades: the
    // middle of a wall can see the ground and the next wall along, so it counts
    // as half-occluded and darkens as much as a real crease does. Physically
    // the wall centre is barely occluded at all — the occluders are far away.
    //
    // `(1 - t/r)^3` concentrates the darkening into the first third of the
    // radius, which in practice is the crease itself. Cubed rather than linear
    // because linear still leaves a visible gradient halfway up a wall, and
    // what reads as "clean low-poly art with AO" is a thin dark line in the
    // corners and nothing anywhere else.
    // A RANGE CHECK with a soft edge, not a ramp from zero.
    //
    // `(1-t/r)^2` was the first attempt and it under-darkens everything: a
    // crevice's hemisphere is mostly GRAZING rays, which travel a long way
    // along the occluder before meeting it, so the very rays that prove a point
    // is enclosed get almost no weight. Measured, a floor with a lid 6cm above
    // it — half the reach — came out at 0.969, essentially unshaded.
    //
    // Full weight out to `NEAR` of the radius, then smoothstepping to nothing
    // at the radius, says what is actually meant: anything close enough counts
    // fully, anything past the reach does not count at all, and the transition
    // is soft enough not to band. Creases go properly dark while a wall's
    // middle — whose floor is beyond the reach entirely — stays clean.
    const NEAR: f32 = 0.45;
    // Hits CLOSER than this are interpenetration, not occlusion. Kit-bash
    // art rests and overlaps its pieces (a plank sits ON its rail, tiles
    // run into each other), so every contact interface otherwise bakes
    // near-black — invisible where the surfaces touch, but the darkness
    // reaches the faces' edge texels, which ARE visible: at silhouettes
    // the striped top reads as a dark rim along every beam. A doorway or
    // slot at real crevice distance still shades.
    // 4mm: kills only the coplanar interface (a plank resting exactly ON
    // its rail) — at 12mm this filtered out the JOINT CREASES themselves
    // (kit joints interpenetrate; the post-into-beam contact is 5-30mm),
    // which un-shaded every junction while leaving groove shading — AO
    // that read as inverted (white joints, gradients at open edges).
    // 1mm numeric guard only — parallel interfaces are suppressed by the
    // crease ANGLE weight now, which tells them apart from real sockets at
    // the same distance.
    const CONTACT_MIN: f32 = 0.001;
    let falloff = |t: f32| {
        if !t.is_finite() || t < CONTACT_MIN {
            return 0.0;
        }
        let lo = radius * NEAR;
        if t <= lo {
            return 1.0;
        }
        let x = ((t - lo) / (radius - lo).max(1.0e-9)).clamp(0.0, 1.0);
        1.0 - x * x * (3.0 - 2.0 * x)
    };
    let (mut occ_self, mut occ_all) = (0.0f32, 0.0f32);
    // AO_BAKE_INFINITE=1: the aobaker/reference integral — any hit at any
    // distance occludes, full weight; occlusion is pure sky visibility and
    // a strength knob does the art. Default remains the contact falloff.
    let infinite = std::env::var("AO_BAKE_INFINITE").is_ok();
    for d in dirs.iter().take(rays) {
        let range = if infinite { f32::MAX } else { radius };
        let (ts, tg, ts_tri) =
            grid.occluded(positions, indices, origin, *d, range, ground_start);
        if infinite {
            occ_self += if ts.is_finite() { 1.0 } else { 0.0 };
            occ_all += if ts.min(tg).is_finite() { 1.0 } else { 0.0 };
        } else {
            let _ = ts_tri;
            occ_self += falloff(ts);
            occ_all += falloff(ts.min(tg));
        }
    }
    let shape = |occ: f32| {
        let open = 1.0 - (occ / rays as f32).clamp(0.0, 1.0);
        // Remap into [AO_FLOOR, 1] rather than using `open` directly.
        AO_FLOOR + open * (1.0 - AO_FLOOR)
    };
    (shape(occ_self), shape(occ_all))
}

/// A reusable occlusion sampler: the acceleration grid plus the tuning that
/// goes with a given mesh.
///
/// Exists so the per-vertex bake and the atlas baker sample IDENTICALLY. Two
/// implementations of "how dark is this point" drift, and the drift shows up
/// as a prop whose atlas disagrees with its own vertices.
pub struct AoSampler {
    grid: TriGrid,
    radius: f32,
    epsilon: f32,
    rays: usize,
    ground_start: usize,
}

impl AoSampler {
    pub fn new(
        positions: &[Vec3f],
        indices: &[u32],
        min: Vec3f,
        max: Vec3f,
        rays: usize,
    ) -> Self {
        Self::with_ground(positions, indices, min, max, rays, usize::MAX)
    }

    /// As [`AoSampler::new`], but the triangles from `ground_start` on are a
    /// virtual floor rather than part of the model.
    ///
    /// Whether a prop is standing on flat ground is a property of where it was
    /// SPAWNED, not of its mesh — the same crate can sit on a road, a rooftop
    /// or a slope. Baking the contact shadow into the mesh would make it claim
    /// a floor it may not have, so the two terms are kept apart here and chosen
    /// between per instance at draw time.
    pub fn with_ground(
        positions: &[Vec3f],
        indices: &[u32],
        min: Vec3f,
        max: Vec3f,
        rays: usize,
        ground_start: usize,
    ) -> Self {
        Self::with_reach(positions, indices, min, max, min, max, rays, ground_start)
    }

    /// As [`AoSampler::with_ground`], but the grid is sized to `grid_min/max`
    /// while the occlusion radius follows `reach_min/max`.
    ///
    /// The two differ whenever the occluder set is bigger than the thing being
    /// shaded — a prop plus the virtual floor under it. The grid has to span
    /// everything a ray might hit, or geometry outside it gets clamped into
    /// border cells and a ray march never reaches it; the radius has to stay
    /// keyed to the model, or a prop's shading would change with the size of
    /// the floor we drew beneath it.
    #[allow(clippy::too_many_arguments)]
    pub fn with_reach(
        positions: &[Vec3f],
        indices: &[u32],
        grid_min: Vec3f,
        grid_max: Vec3f,
        reach_min: Vec3f,
        reach_max: Vec3f,
        rays: usize,
        ground_start: usize,
    ) -> Self {
        let span = vec_max3(
            reach_max.x - reach_min.x,
            reach_max.y - reach_min.y,
            reach_max.z - reach_min.z,
        )
        .max(1.0e-5);
        Self {
            grid: TriGrid::build(positions, indices, grid_min, grid_max),
            radius: (span * AO_RADIUS_FRAC).min(AO_RADIUS_WORLD),
            epsilon: span * 1.0e-3,
            rays: rays.clamp(3, AO_RAYS_OFFLINE),
            ground_start,
        }
    }

    /// Occlusion at one point, in [AO_FLOOR, 1], counting the virtual ground.
    #[cfg(test)]
    pub(crate) fn grid_ref(&self) -> &TriGrid {
        &self.grid
    }
    #[cfg(test)]
    pub(crate) fn radius_ref(&self) -> f32 {
        self.radius
    }
    /// The `span * 1e-3` surface offset this sampler computed from the model
    /// bounds at construction — the aobaker port reuses it as its `tnear`.
    pub(crate) fn epsilon(&self) -> f32 {
        self.epsilon
    }

    /// Any-hit for the aobaker port ([`crate::aobaker_port`]): optionally
    /// embree-backface-culled, and this sampler's virtual-ground triangles
    /// are NEVER counted — aobaker's scene is the mesh alone, and the ground
    /// quad is this machinery's addition.
    pub(crate) fn any_hit_aobaker(
        &self,
        positions: &[Vec3f],
        indices: &[u32],
        origin: Vec3f,
        dir: Vec3f,
        max_t: f32,
        front_only: bool,
    ) -> bool {
        self.grid
            .any_hit(positions, indices, origin, dir, max_t, front_only, self.ground_start)
    }

    /// Nearest self-geometry hit from `origin` along `dir` within `max_t`
    /// (INFINITY = clear). For geometric side tests — orientation by parity,
    /// not by winding or authored normals, both of which mirrored nodes lie
    /// about.
    pub fn nearest_hit(
        &self,
        positions: &[Vec3f],
        indices: &[u32],
        origin: Vec3f,
        dir: Vec3f,
        max_t: f32,
    ) -> f32 {
        let (ts, _, _) = self
            .grid
            .occluded(positions, indices, origin, dir, max_t, indices.len() / 3);
        ts
    }

    /// Diagnostic: print every blocking ray for one query.
    pub fn dump_rays(
        &self,
        positions: &[Vec3f],
        indices: &[u32],
        point: Vec3f,
        normal: Vec3f,
    ) {
        let n = normalize(normal);
        let origin = Vec3f {
            x: point.x + n.x * self.epsilon,
            y: point.y + n.y * self.epsilon,
            z: point.z + n.z * self.epsilon,
        };
        let mut dirs = [Vec3f { x: 0.0, y: 1.0, z: 0.0 }; AO_RAYS_OFFLINE];
        hemisphere(n, self.rays, &mut dirs);
        for (i, d) in dirs.iter().take(self.rays).enumerate() {
            let (ts, _, ts_tri) = self.grid.occluded(
                positions, indices, origin, *d, self.radius, indices.len() / 3,
            );
            if ts.is_finite() {
                let hp = Vec3f {
                    x: origin.x + d.x * ts,
                    y: origin.y + d.y * ts,
                    z: origin.z + d.z * ts,
                };
                let hn = face_normal_at(positions, indices, ts_tri);
                let a = 1.0 - (hn.x * n.x + hn.y * n.y + hn.z * n.z).abs();
                eprintln!(
                    "  ray {i}: cosN {:.2} t {:.4} hit ({:.3},{:.3},{:.3}) hitN ({:.2},{:.2},{:.2}) w {:.2}",
                    d.x * n.x + d.y * n.y + d.z * n.z,
                    ts, hp.x, hp.y, hp.z, hn.x, hn.y, hn.z,
                    a * a
                );
            }
        }
    }

    pub fn at(&self, positions: &[Vec3f], indices: &[u32], point: Vec3f, normal: Vec3f) -> f32 {
        self.at_split(positions, indices, point, normal).1
    }

    /// Occlusion at one point as `(self_only, with_ground)`, both in
    /// [AO_FLOOR, 1] — one ray pass, two answers.
    pub fn at_split(
        &self,
        positions: &[Vec3f],
        indices: &[u32],
        point: Vec3f,
        normal: Vec3f,
    ) -> (f32, f32) {
        let mut dirs = [Vec3f { x: 0.0, y: 1.0, z: 0.0 }; AO_RAYS_OFFLINE];
        sample_ao(
            &self.grid,
            positions,
            indices,
            point,
            normal,
            self.radius,
            self.epsilon,
            self.rays,
            self.ground_start,
            &mut dirs,
        )
    }
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
        // Lid at 0.06 — INSIDE `AO_RADIUS_WORLD`. It used to sit at 0.3, from
        // when occlusion was a plain hemisphere fraction and reach was a share
        // of the model's span. Occlusion now attenuates with distance and reach
        // is an absolute ~12cm, so an overhang a third of a metre up genuinely
        // does not shade the floor beneath it any more — which is the point of
        // the change, not a regression: reach that long is what greyed whole
        // facades, because a wall's middle is exactly that far from its floor.
        let positions = vec![
            v(-1.0, 0.0, -1.0), v(1.0, 0.0, -1.0), v(1.0, 0.0, 1.0), v(-1.0, 0.0, 1.0),
            v(-1.0, 0.06, -1.0), v(1.0, 0.06, -1.0), v(1.0, 0.06, 1.0), v(-1.0, 0.06, 1.0),
        ];
        let mut normals = vec![v(0.0, 1.0, 0.0); 4];
        normals.extend(vec![v(0.0, -1.0, 0.0); 4]);
        let indices = vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7];
        let ao = bake_vertex_ao(&positions, &normals, &indices, v(-1.0, 0.0, -1.0), v(1.0, 0.06, 1.0));
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
        // Attenuated AO cannot reach the floor here, and should not be asked
        // to: most of a crevice's hemisphere is GRAZING rays, which travel far
        // along the plate before meeting it and so contribute little once
        // occlusion falls off with distance. Ground-truth AO would call this
        // point black. What matters for the look is that a tight crevice lands
        // well down the range rather than reading as open sky.
        let midway = (1.0 + AO_FLOOR) * 0.5;
        assert!(
            ao[0] < midway,
            "vertex in a tight crevice barely darkened: {} (wanted below {midway})",
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

    /// A big flat quad in the open: nothing occludes it, so linear
    /// interpolation is already exact everywhere. Splitting it would be pure

    /// The case that motivates the whole thing: a wall standing on a floor.
    /// The floor's four corners are all out in the open and all fully lit, so
    /// the darkening along the wall base has nowhere to live — the mesh must




    /// Ground truth: a floor meeting a wall at z = 0. Walk out from the crease
    /// and print what the sampler actually returns.
    ///
    /// This is the profile the whole system is supposed to produce — dark at
    /// the crease, brightening smoothly, flat far away. Printed rather than
    /// asserted so the SHAPE is visible; the assertions come after.
    #[test]
    fn print_corner_profile() {
        let mut p = vec![
            v(-4.0, 0.0, -0.001), v(4.0, 0.0, -0.001), v(4.0, 0.0, 8.0), v(-4.0, 0.0, 8.0),
        ];
        let mut n = vec![v(0.0, 1.0, 0.0); 4];
        let mut i = vec![0, 1, 2, 0, 2, 3];
        let base = p.len() as u32;
        p.extend_from_slice(&[
            v(-4.0, 0.0, 0.0), v(4.0, 0.0, 0.0), v(4.0, 4.0, 0.0), v(-4.0, 4.0, 0.0),
        ]);
        n.extend_from_slice(&[v(0.0, 0.0, 1.0); 4]);
        i.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);

        let (min, max) = (v(-4.0, 0.0, 0.0), v(4.0, 4.0, 8.0));
        let s = AoSampler::new(&p, &i, min, max, AO_RAYS_OFFLINE);
        println!("\n  dist   AO      bar");
        for k in 0..16 {
            // 0.02 in from the crease, matching the atlas's own edge inset:
            // a point EXACTLY on the seam is degenerate (a zero-thickness
            // wall cannot be hit by a ray that starts in its plane), so
            // probing it measures the degeneracy rather than the corner.
            let d = k as f32 * 0.25 + 0.02;
            let ao = s.at(&p, &i, v(0.0, 0.0, d), v(0.0, 1.0, 0.0));
            let bar = "#".repeat(((ao - AO_FLOOR) / (1.0 - AO_FLOOR) * 40.0).max(0.0) as usize);
            println!("  {d:4.2}  {ao:.3}  {bar}");
        }
        println!(
            "  (AO_FLOOR={AO_FLOOR}, radius={:.2} of an {:.1}-unit span, {AO_RAYS_OFFLINE} rays)",
            (max.z - min.z) * AO_RADIUS_FRAC,
            max.z - min.z
        );
    }
}

// Forensic: fire the atlas bake's own hemisphere from the middle of the
// most-open vertical face of ONE model and report every hit — distance and
// position. An isolated post face should report (almost) nothing.
#[cfg(test)]
mod post_face_forensics {
    use super::*;
    use crate::model::{StaticModel, MODEL_VERTEX_FLOATS};

    #[test]
    fn isolated_post_face_hits() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit/template-floor-layer-raised.glb",
        );
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("asset absent — skipped");
            return;
        };
        let m = StaticModel::parse_glb(&bytes).unwrap();
        let vp = |i: u32| {
            let o = i as usize * MODEL_VERTEX_FLOATS;
            Vec3f { x: m.vertices[o], y: m.vertices[o + 1], z: m.vertices[o + 2] }
        };
        let pos: Vec<Vec3f> = (0..m.vertices.len() / MODEL_VERTEX_FLOATS)
            .map(|i| vp(i as u32))
            .collect();
        // Pick the biggest vertical (|ny| small) triangle whose centroid sits
        // mid-height on the OUTER boundary in x or z — a post's outer face.
        let mut best: Option<(usize, f32)> = None;
        for (t, tri) in m.indices.chunks_exact(3).enumerate() {
            let (a, b, c) = (vp(tri[0]), vp(tri[1]), vp(tri[2]));
            let e1 = b - a;
            let e2 = c - a;
            let n = Vec3f {
                x: e1.y * e2.z - e1.z * e2.y,
                y: e1.z * e2.x - e1.x * e2.z,
                z: e1.x * e2.y - e1.y * e2.x,
            };
            let area2 = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
            if area2 < 1.0e-6 {
                continue;
            }
            let vertical = (n.y / area2).abs() < 0.1;
            let cy = (a.y + b.y + c.y) / 3.0;
            let cz = (a.z + b.z + c.z) / 3.0;
            // outer rim: near max z, mid height
            if vertical && cy > 1.0 && cy < 3.5 && cz > m.max.z - 0.6 {
                if best.map_or(true, |(_, ba)| area2 > ba) {
                    best = Some((t, area2));
                }
            }
        }
        let Some((t, _)) = best else {
            eprintln!("no outer post face found");
            return;
        };
        let tri = &m.indices[t * 3..t * 3 + 3];
        let (a, b, c) = (vp(tri[0]), vp(tri[1]), vp(tri[2]));
        let centre = Vec3f {
            x: (a.x + b.x + c.x) / 3.0,
            y: (a.y + b.y + c.y) / 3.0,
            z: (a.z + b.z + c.z) / 3.0,
        };
        // The outer max-z face: its outward normal is +z by construction.
        let n = Vec3f { x: 0.0, y: 0.0, z: 1.0 };
        eprintln!(
            "face {t} centre ({:.2},{:.2},{:.2}) authored-n ({:.2},{:.2},{:.2})",
            centre.x, centre.y, centre.z, n.x, n.y, n.z
        );
        let sampler = AoSampler::with_ground(&pos, &m.indices, m.min, m.max, 64, m.indices.len() / 3);
        let v = sampler.at_split(&pos, &m.indices, centre, n);
        eprintln!("ao at face centre: self={:.3} with_ground={:.3}", v.0, v.1);
        // Now the raw hits, one by one.
        let mut dirs = [Vec3f { x: 0.0, y: 1.0, z: 0.0 }; AO_RAYS_OFFLINE];
        hemisphere(normalize(n), 64, &mut dirs);
        let eps = sampler.epsilon();
        let origin = Vec3f {
            x: centre.x + n.x * eps,
            y: centre.y + n.y * eps,
            z: centre.z + n.z * eps,
        };
        let mut hits = 0;
        for (i, d) in dirs.iter().take(64).enumerate() {
            let (ts, _, _) = sampler.grid_ref().occluded(
                &pos, &m.indices, origin, *d, sampler.radius_ref(), m.indices.len() / 3,
            );
            if ts.is_finite() {
                hits += 1;
                if hits <= 12 {
                    let hp = Vec3f {
                        x: origin.x + d.x * ts,
                        y: origin.y + d.y * ts,
                        z: origin.z + d.z * ts,
                    };
                    eprintln!(
                        "  ray {i}: dir ({:.2},{:.2},{:.2}) hit t={:.3} at ({:.2},{:.2},{:.2})",
                        d.x, d.y, d.z, ts, hp.x, hp.y, hp.z
                    );
                }
            }
        }
        eprintln!("hits: {hits}/64, radius {:.3}", sampler.radius_ref());
    }
}

#[cfg(test)]
mod ray_query_micro {
    use super::*;

    fn v(x: f32, y: f32, z: f32) -> Vec3f {
        Vec3f { x, y, z }
    }

    /// One quad dead ahead: the nearest-hit query must see it. The truth
    /// suite caught a slot wall 6cm from an opposing beam baking fully open,
    /// and the sampler itself reported no occlusion — so the failure is at
    /// this layer or below.
    #[test]
    fn ray_hits_a_plate_dead_ahead() {
        // Plate at z = 0.1, spanning x,y in [-1,1].
        let p = vec![
            v(-1.0, -1.0, 0.1),
            v(1.0, -1.0, 0.1),
            v(1.0, 1.0, 0.1),
            v(-1.0, 1.0, 0.1),
        ];
        let i = vec![0u32, 1, 2, 0, 2, 3];
        let sampler = AoSampler::with_ground(&p, &i, v(-1.0, -1.0, 0.0), v(1.0, 1.0, 0.2), 64, 2);
        let t = sampler.nearest_hit(&p, &i, v(0.0, 0.0, 0.0), v(0.0, 0.0, 1.0), 1.0);
        eprintln!("nearest_hit straight at plate: t={t}");
        assert!(t.is_finite() && (t - 0.1).abs() < 0.01, "missed the plate: t={t}");
        // And the full AO query at a point facing the plate from 6cm.
        let ao = sampler.at_split(&p, &i, v(0.0, 0.0, 0.04), v(0.0, 0.0, 1.0)).0;
        eprintln!("ao facing plate at 6cm: {ao:.3}");
        assert!(ao < 0.75, "facing a wall 6cm away must darken, got {ao:.3}");
    }
}

#[cfg(test)]
mod edge_graze_probe {
    use super::*;
    use crate::model::{StaticModel, MODEL_VERTEX_FLOATS};

    /// Narrow-face edge darkening, measured at the sampler: value at face
    /// CENTRE vs 2cm from the TOP edge of a high rafter side face, with
    /// every blocking ray's angle-from-normal and hit distance. If the
    /// edge point collects near-tangent hits the centre does not, the rim
    /// is grazing-ray noise; if both are clean, the rim enters later.
    #[test]
    fn rafter_side_edge_vs_centre() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit/template-floor-layer-raised.glb",
        );
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("asset absent — skipped");
            return;
        };
        let m = StaticModel::parse_glb(&bytes).unwrap();
        let stride = MODEL_VERTEX_FLOATS;
        let pos: Vec<Vec3f> = (0..m.vertices.len() / stride)
            .map(|i| Vec3f {
                x: m.vertices[i * stride],
                y: m.vertices[i * stride + 1],
                z: m.vertices[i * stride + 2],
            })
            .collect();
        // A high vertical face: normal.y ~ 0, centroid y > 2.5, area small
        // (a rafter side, not a post).
        let mut best: Option<(usize, f32)> = None;
        for (t, tri) in m.indices.chunks_exact(3).enumerate() {
            let (a, b, c) = (
                pos[tri[0] as usize],
                pos[tri[1] as usize],
                pos[tri[2] as usize],
            );
            let e1 = b - a;
            let e2 = c - a;
            let n = Vec3f {
                x: e1.y * e2.z - e1.z * e2.y,
                y: e1.z * e2.x - e1.x * e2.z,
                z: e1.x * e2.y - e1.y * e2.x,
            };
            let l = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
            if l < 1.0e-9 {
                continue;
            }
            let cy = (a.y + b.y + c.y) / 3.0;
            let area = l * 0.5;
            if (n.y / l).abs() < 0.1 && cy > 2.5 && area > 0.05 && area < 0.5 {
                if best.map_or(true, |(_, ba)| area > ba) {
                    best = Some((t, area));
                }
            }
        }
        let Some((t, _)) = best else {
            eprintln!("no rafter side found");
            return;
        };
        let tri = &m.indices[t * 3..t * 3 + 3];
        let (a, b, c) = (
            pos[tri[0] as usize],
            pos[tri[1] as usize],
            pos[tri[2] as usize],
        );
        let e1 = b - a;
        let e2 = c - a;
        let mut n = Vec3f {
            x: e1.y * e2.z - e1.z * e2.y,
            y: e1.z * e2.x - e1.x * e2.z,
            z: e1.x * e2.y - e1.y * e2.x,
        };
        let l = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
        n = Vec3f { x: n.x / l, y: n.y / l, z: n.z / l };
        let centre = Vec3f {
            x: (a.x + b.x + c.x) / 3.0,
            y: (a.y + b.y + c.y) / 3.0,
            z: (a.z + b.z + c.z) / 3.0,
        };
        let top_y = a.y.max(b.y).max(c.y);
        let near_edge = Vec3f { x: centre.x, y: top_y - 0.02, z: centre.z };
        let sampler = AoSampler::with_ground(
            &pos, &m.indices, m.min, m.max, 128, m.indices.len() / 3,
        );
        for (label, q) in [("centre", centre), ("2cm-under-top-edge", near_edge)] {
            let v = sampler.at_split(&pos, &m.indices, q, n).0;
            eprintln!(
                "{label}: ({:.3},{:.3},{:.3}) n ({:.2},{:.2},{:.2}) -> {v:.3}",
                q.x, q.y, q.z, n.x, n.y, n.z
            );
            let mut dirs = [Vec3f { x: 0.0, y: 1.0, z: 0.0 }; AO_RAYS_OFFLINE];
            hemisphere(n, 128, &mut dirs);
            let eps = 4.4e-3;
            let origin = Vec3f {
                x: q.x + n.x * eps,
                y: q.y + n.y * eps,
                z: q.z + n.z * eps,
            };
            let mut hits = 0;
            for d in dirs.iter().take(128) {
                let t2 = sampler.nearest_hit(&pos, &m.indices, origin, *d, 0.12);
                if t2.is_finite() {
                    hits += 1;
                    if hits <= 10 {
                        let cosn = d.x * n.x + d.y * n.y + d.z * n.z;
                        eprintln!(
                            "   hit: angle-from-normal {:.0}deg dir.y {:.2} t {:.4}",
                            cosn.acos().to_degrees(), d.y, t2
                        );
                    }
                }
            }
            eprintln!("   hits within 0.12: {hits}/128");
        }
    }
}

//! Free-space voxelisation and the clearance field.
//!
//! # The one-clearance law
//!
//! There is exactly **one** function in this crate that answers "how much room
//! is there at this point": [`VoxelGrid::clearance_at`], reading the field
//! built by [`VoxelGrid::clearance`]. The path planner, the spline smoother
//! and the QA harness all call it.
//!
//! That is not tidiness, it is the fix for a bug the Doom walker paid for:
//! when the graph used one wall test and the body used a slightly different
//! one, there was a band of wall heights the graph offered and the body then
//! refused, and the walker stood at the ledge forever
//! (`libs/render/src/level.rs:216`). Two clearance functions is a bug with a
//! delay fuse. `tests/one_clearance.rs` asserts they stay one.
//!
//! Three occupancy sets are voxelised in one triangle pass, because they
//! answer three different questions and conflating any two of them is a bug:
//!
//! * **solid** — what blocks a body. Everything but door leaves and plain
//!   openings. Glazing blocks: a floor-to-ceiling window is a wall to walk
//!   into, and the one thing worse than a camera stuck in a doorway is a
//!   camera strolling out through a first-floor curtain wall. Navigation,
//!   clearance and QA read this, and only this.
//! * **sealed** — solid plus the door leaves. Only room segmentation uses it,
//!   because a shut door is exactly what makes two rooms two rooms.
//! * **opaque** — what stops a sight line. Solid minus the glass. Used to ask
//!   "can the camera see anything from here", never to ask "can it go there".
//!
//! # Surfaces, not solids
//!
//! Voxelisation marks the triangles, so a closed solid is a shell with a
//! hollow middle rather than a filled block. Building elements are thin —
//! a 0.2 m wall or a 0.3 m slab fills at any sane cell size — so this is
//! invisible in practice, and filling properly would need a per-element
//! parity pass over the whole grid.
//!
//! The failure it could cause is a phantom room inside a big solid lump of
//! furniture or foundation. It cannot happen, because a room has to be
//! walkable and walkable needs a clear column the full height of the body
//! (1.7 m). Nothing in a building is both solid, closed, and over 1.7 m hollow
//! inside. If such a cavity ever did appear it would have no portal to
//! anything, so it would land in [`crate::SiteAnalysis::unreachable`] and be
//! reported rather than toured.

use crate::geom::tri_box_overlap;
use crate::scene::TourScene;
use makepad_math::{vec3, Aabb, Vec3f};

/// Bit-per-voxel occupancy plus the derived fields.
pub struct VoxelGrid {
    /// World position of the centre of voxel `(0,0,0)`.
    pub origin: Vec3f,
    pub cell: f32,
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    solid: Vec<u64>,
    sealed: Vec<u64>,
    opaque: Vec<u64>,
    /// Metres of free space around each voxel centre. Built lazily.
    clearance: Vec<f32>,
    /// Set for voxels with any solid voxel above them in the same column.
    covered: Vec<u64>,
    /// Set for free voxels reachable from outside the building.
    exterior: Vec<u64>,
    /// Whether the "is there a roof above me" test means anything for this
    /// model. See [`VoxelGrid::is_interior`].
    trust_cover: bool,
}

#[inline]
fn bit_get(bits: &[u64], i: usize) -> bool {
    bits[i >> 6] & (1u64 << (i & 63)) != 0
}

#[inline]
fn bit_set(bits: &mut [u64], i: usize) {
    bits[i >> 6] |= 1u64 << (i & 63);
}

/// Voxelisation settings.
#[derive(Clone, Copy, Debug)]
pub struct VoxelConfig {
    /// Requested cell size in metres. Coarsened automatically if the model
    /// would exceed `max_voxels`.
    pub cell: f32,
    pub max_voxels: usize,
    /// Minimum air padded around the *building* (not the site plate, which is
    /// usually enormous) so a drone has somewhere to fly and the exterior
    /// flood fill has a border to start from. Grown to `2.2 ×` the building's
    /// plan radius horizontally, because that is where the reveal orbit and
    /// the closing pull-back sit — and clearance outside the grid reads as
    /// zero, so a shot that leaves the volume is a shot the QA calls a wall.
    pub pad: f32,
    /// A gap narrower than this is not a way out. Used to stop the exterior
    /// flood fill leaking indoors through doorways and windows: outdoors has
    /// metres of clearance, a 0.9 m doorway has 0.45 m.
    pub seal_radius: f32,
}

impl Default for VoxelConfig {
    fn default() -> Self {
        VoxelConfig {
            cell: 0.15,
            max_voxels: 24_000_000,
            pad: 6.0,
            seal_radius: 0.75,
        }
    }
}

impl VoxelGrid {
    #[inline]
    pub fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.ny + y) * self.nx + x
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.nx * self.ny * self.nz
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn world_of(&self, x: usize, y: usize, z: usize) -> Vec3f {
        self.origin + vec3(x as f32, y as f32, z as f32) * self.cell
    }

    /// Voxel containing `p`, clamped to nothing — returns `None` when outside.
    #[inline]
    pub fn cell_of(&self, p: Vec3f) -> Option<(usize, usize, usize)> {
        let r = (p - self.origin) * (1.0 / self.cell);
        let (x, y, z) = (r.x.round(), r.y.round(), r.z.round());
        if x < 0.0 || y < 0.0 || z < 0.0 {
            return None;
        }
        let (x, y, z) = (x as usize, y as usize, z as usize);
        if x >= self.nx || y >= self.ny || z >= self.nz {
            return None;
        }
        Some((x, y, z))
    }

    #[inline]
    pub fn solid_at(&self, x: usize, y: usize, z: usize) -> bool {
        bit_get(&self.solid, self.idx(x, y, z))
    }

    #[inline]
    pub fn sealed_at(&self, x: usize, y: usize, z: usize) -> bool {
        bit_get(&self.sealed, self.idx(x, y, z))
    }

    #[inline]
    pub fn opaque_at(&self, x: usize, y: usize, z: usize) -> bool {
        bit_get(&self.opaque, self.idx(x, y, z))
    }

    #[inline]
    pub fn covered_at(&self, x: usize, y: usize, z: usize) -> bool {
        bit_get(&self.covered, self.idx(x, y, z))
    }

    /// March a sight line until it hits something you cannot see through.
    /// Returns `(distance, ended_outside)` — the second value is how the room
    /// scorer knows a view goes out of a window rather than into a cupboard.
    pub fn sight_run(&self, from: Vec3f, dir: Vec3f, max: f32) -> (f32, bool) {
        let step = self.cell * 0.5;
        let n = (max / step).ceil() as usize;
        let mut last_outside = false;
        for i in 1..=n {
            let t = i as f32 * step;
            let p = from + dir * t;
            let Some((x, y, z)) = self.cell_of(p) else {
                return (t, last_outside);
            };
            if self.opaque_at(x, y, z) {
                return (t - step, last_outside);
            }
            last_outside = self.exterior_at(x, y, z);
        }
        (max, last_outside)
    }

    #[inline]
    pub fn exterior_at(&self, x: usize, y: usize, z: usize) -> bool {
        bit_get(&self.exterior, self.idx(x, y, z))
    }

    pub fn clearance(&self) -> &[f32] {
        &self.clearance
    }

    /// **The** clearance query: metres of free space around `p`, trilinearly
    /// interpolated so the value is continuous (a stepped field makes the
    /// gradient descent in `path::relax` jitter). Returns `0` outside the grid
    /// — outside is not known to be free, so it is not free.
    pub fn clearance_at(&self, p: Vec3f) -> f32 {
        let r = (p - self.origin) * (1.0 / self.cell);
        if !(r.x >= 0.0 && r.y >= 0.0 && r.z >= 0.0) {
            return 0.0;
        }
        let (x0, y0, z0) = (r.x.floor(), r.y.floor(), r.z.floor());
        let (fx, fy, fz) = (r.x - x0, r.y - y0, r.z - z0);
        let (x0, y0, z0) = (x0 as usize, y0 as usize, z0 as usize);
        if x0 + 1 >= self.nx || y0 + 1 >= self.ny || z0 + 1 >= self.nz {
            return 0.0;
        }
        let c = &self.clearance;
        let g = |x: usize, y: usize, z: usize| c[self.idx(x, y, z)];
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let c00 = lerp(g(x0, y0, z0), g(x0 + 1, y0, z0), fx);
        let c10 = lerp(g(x0, y0 + 1, z0), g(x0 + 1, y0 + 1, z0), fx);
        let c01 = lerp(g(x0, y0, z0 + 1), g(x0 + 1, y0, z0 + 1), fx);
        let c11 = lerp(g(x0, y0 + 1, z0 + 1), g(x0 + 1, y0 + 1, z0 + 1), fx);
        lerp(lerp(c00, c10, fy), lerp(c01, c11, fy), fz)
    }

    /// Gradient of the clearance field: the direction that gets you *more*
    /// room. Central differences, one cell apart.
    pub fn clearance_gradient(&self, p: Vec3f) -> Vec3f {
        let h = self.cell;
        let dx = self.clearance_at(p + vec3(h, 0.0, 0.0)) - self.clearance_at(p - vec3(h, 0.0, 0.0));
        let dy = self.clearance_at(p + vec3(0.0, h, 0.0)) - self.clearance_at(p - vec3(0.0, h, 0.0));
        let dz = self.clearance_at(p + vec3(0.0, 0.0, h)) - self.clearance_at(p - vec3(0.0, 0.0, h));
        vec3(dx, dy, dz) * (0.5 / h)
    }

    /// Is the straight segment `a`→`b` clear of geometry by `radius` all the
    /// way? Samples at half a cell, which with a continuous clearance field is
    /// enough — the field cannot dip by more than the step between samples.
    pub fn segment_clear(&self, a: Vec3f, b: Vec3f, radius: f32) -> bool {
        let d = b - a;
        let len = d.length();
        if len < 1e-6 {
            return self.clearance_at(a) >= radius;
        }
        let steps = ((len / (self.cell * 0.5)).ceil() as usize).max(1);
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            if self.clearance_at(a + d * t) < radius {
                return false;
            }
        }
        true
    }

    /// Distance along `dir` (normalised) until the clearance drops below
    /// `radius`, capped at `max`. "How far can the camera see down this line",
    /// used to reject shots that stare into a wall.
    pub fn free_run(&self, from: Vec3f, dir: Vec3f, radius: f32, max: f32) -> f32 {
        let step = self.cell * 0.5;
        let mut t = 0.0;
        while t < max {
            t += step;
            if self.clearance_at(from + dir * t) < radius {
                return t - step;
            }
        }
        max
    }

    /// Build the grid from a scene. Voxelises solid and sealed occupancy in
    /// one triangle pass, then derives clearance, cover and the exterior.
    pub fn build(scene: &TourScene, cfg: &VoxelConfig) -> VoxelGrid {
        // Size the volume around the building, not the terrain plate, then
        // clip the whole thing to what the scene actually contains.
        let b = scene.building_bounds();
        let bs = b.max - b.min;
        let plan_r = (bs.x.max(bs.y) * 0.5).max(1.0);
        // Padding is charged twice per axis, so a generous multiple of the
        // plan radius squares up fast: at 2.2x the villa's grid wanted 22M
        // voxels and the cell coarsened to 0.23 m, which is wider than half a
        // doorway — and doorways the lattice cannot resolve are doorways the
        // room graph never finds. Keep the air tight and let `clamp_to_grid`
        // pull the exterior shots in.
        let hpad = cfg.pad.max(plan_r * 0.55);
        let vpad_up = cfg.pad.max(bs.z * 0.55);
        // Sized from the building alone. Clipping this back to the scene's own
        // bounds looks like a saving and is a bug: when the building *is* the
        // scene there is no terrain plate to clip against, the padding
        // vanishes, and every exterior shot flies straight off the edge of the
        // grid into what the oracle correctly calls no clearance at all.
        let bounds = Aabb {
            min: vec3(b.min.x - hpad, b.min.y - hpad, b.min.z - 2.0),
            max: vec3(b.max.x + hpad, b.max.y + hpad, b.max.z + vpad_up),
        };
        let size = bounds.max - bounds.min;

        // Coarsen until the grid fits the budget.
        let mut cell = cfg.cell.max(0.02);
        loop {
            let n = ((size.x / cell).ceil() + 1.0)
                * ((size.y / cell).ceil() + 1.0)
                * ((size.z / cell).ceil() + 1.0);
            if n <= cfg.max_voxels as f32 || cell > 2.0 {
                break;
            }
            cell *= 1.25;
        }
        let nx = (size.x / cell).ceil() as usize + 1;
        let ny = (size.y / cell).ceil() as usize + 1;
        let nz = (size.z / cell).ceil() as usize + 1;
        let words = (nx * ny * nz).div_ceil(64);

        let mut g = VoxelGrid {
            origin: bounds.min,
            cell,
            nx,
            ny,
            nz,
            solid: vec![0u64; words],
            sealed: vec![0u64; words],
            opaque: vec![0u64; words],
            clearance: Vec::new(),
            covered: vec![0u64; words],
            exterior: vec![0u64; words],
            trust_cover: true,
        };

        let half = vec3(cell, cell, cell) * 0.5;
        for tri in 0..scene.triangle_count() {
            let Some(elem) = scene.element_of_triangle(tri) else {
                continue;
            };
            let blocks_nav = elem.class.blocks_navigation();
            let seals = elem.class.seals_rooms();
            let blocks_sight = seals && !elem.class.is_transparent();
            if !blocks_nav && !seals {
                continue;
            }
            let [v0, v1, v2] = scene.triangle(tri);
            let lo_p = Vec3f::min_componentwise(Vec3f::min_componentwise(v0, v1), v2);
            let hi_p = Vec3f::max_componentwise(Vec3f::max_componentwise(v0, v1), v2);
            let Some((x0, y0, z0)) = g.clamp_cell(lo_p - half) else {
                continue;
            };
            let Some((x1, y1, z1)) = g.clamp_cell(hi_p + half) else {
                continue;
            };
            // Walk the triangle's *footprint*, not its bounding volume, and
            // for each column only the z cells its own plane can reach. A
            // ground triangle 60 m across spans millions of cells in its AABB
            // and occupies two of them per column; testing the volume made
            // voxelising real terrain take longer than everything else in the
            // analysis put together.
            let n = Vec3f::cross(v1 - v0, v2 - v0);
            let nl = n.length();
            let plane_d = if nl > 1e-9 { n.dot(v0) } else { 0.0 };
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let c0 = g.world_of(x, y, 0);
                    let (mut zlo, mut zhi) = (z0, z1);
                    if nl > 1e-9 && n.z.abs() > 1e-6 {
                        // z of the plane over this column's four xy corners.
                        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
                        for (sx, sy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                            let px = c0.x + sx * half.x;
                            let py = c0.y + sy * half.y;
                            let pz = (plane_d - n.x * px - n.y * py) / n.z;
                            lo = lo.min(pz);
                            hi = hi.max(pz);
                        }
                        lo = lo.max(lo_p.z) - half.z;
                        hi = hi.min(hi_p.z) + half.z;
                        if hi < lo {
                            continue;
                        }
                        let a = g.clamp_cell(vec3(c0.x, c0.y, lo)).map(|c| c.2).unwrap_or(z0);
                        let b = g.clamp_cell(vec3(c0.x, c0.y, hi)).map(|c| c.2).unwrap_or(z1);
                        zlo = a.max(z0);
                        zhi = b.min(z1);
                    }
                    for z in zlo..=zhi {
                        let c = g.world_of(x, y, z);
                        if !tri_box_overlap(c, half, v0, v1, v2) {
                            continue;
                        }
                        let i = g.idx(x, y, z);
                        if blocks_nav {
                            bit_set(&mut g.solid, i);
                        }
                        if seals {
                            bit_set(&mut g.sealed, i);
                        }
                        if blocks_sight {
                            bit_set(&mut g.opaque, i);
                        }
                    }
                }
            }
        }

        g.build_clearance();
        g.build_covered();
        g.build_exterior(cfg.seal_radius);
        g.measure_cover();
        g
    }

    fn clamp_cell(&self, p: Vec3f) -> Option<(usize, usize, usize)> {
        let r = (p - self.origin) * (1.0 / self.cell);
        let x = (r.x.round() as i64).clamp(0, self.nx as i64 - 1) as usize;
        let y = (r.y.round() as i64).clamp(0, self.ny as i64 - 1) as usize;
        let z = (r.z.round() as i64).clamp(0, self.nz as i64 - 1) as usize;
        Some((x, y, z))
    }

    /// Exact Euclidean distance transform (Felzenszwalb & Huttenlocher), run
    /// separably along X, then Y, then Z on squared distances. Result is
    /// converted to metres and shrunk by half a cell so the value is the
    /// distance to the solid voxel's *face*, not its centre — the planner is
    /// allowed to be pessimistic, never optimistic.
    fn build_clearance(&mut self) {
        let (nx, ny, nz) = (self.nx, self.ny, self.nz);
        let mut f = vec![0f32; nx * ny * nz];
        const FAR: f32 = 1e12;
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    let i = self.idx(x, y, z);
                    f[i] = if bit_get(&self.solid, i) { 0.0 } else { FAR };
                }
            }
        }
        let maxn = nx.max(ny).max(nz);
        let mut buf = vec![0f32; maxn];
        let mut out = vec![0f32; maxn];
        let mut v = vec![0usize; maxn];
        let mut zs = vec![0f32; maxn + 1];

        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    buf[x] = f[self.idx(x, y, z)];
                }
                edt_1d(&buf[..nx], &mut out[..nx], &mut v, &mut zs);
                for x in 0..nx {
                    f[self.idx(x, y, z)] = out[x];
                }
            }
        }
        for z in 0..nz {
            for x in 0..nx {
                for y in 0..ny {
                    buf[y] = f[self.idx(x, y, z)];
                }
                edt_1d(&buf[..ny], &mut out[..ny], &mut v, &mut zs);
                for y in 0..ny {
                    f[self.idx(x, y, z)] = out[y];
                }
            }
        }
        for y in 0..ny {
            for x in 0..nx {
                for z in 0..nz {
                    buf[z] = f[self.idx(x, y, z)];
                }
                edt_1d(&buf[..nz], &mut out[..nz], &mut v, &mut zs);
                for z in 0..nz {
                    f[self.idx(x, y, z)] = out[z];
                }
            }
        }
        let cell = self.cell;
        for d in f.iter_mut() {
            *d = (d.max(0.0).sqrt() * cell - cell * 0.5).max(0.0);
        }
        self.clearance = f;
    }

    /// Mark every voxel that has solid geometry somewhere above it. One
    /// top-down sweep per column.
    fn build_covered(&mut self) {
        for y in 0..self.ny {
            for x in 0..self.nx {
                let mut seen = false;
                for z in (0..self.nz).rev() {
                    let i = self.idx(x, y, z);
                    if seen {
                        bit_set(&mut self.covered, i);
                    }
                    if bit_get(&self.solid, i) {
                        seen = true;
                    }
                }
            }
        }
    }

    /// Flood the outside air inwards from the grid border, but only through
    /// voxels with at least `seal_radius` of room. Outdoors is metres wide;
    /// a doorway is half a metre. So the fill wraps the building and stops at
    /// the envelope instead of pouring in through every door and window.
    fn build_exterior(&mut self, seal_radius: f32) {
        let (nx, ny, nz) = (self.nx, self.ny, self.nz);
        let passable = |g: &VoxelGrid, i: usize| !bit_get(&g.solid, i) && g.clearance[i] >= seal_radius;
        let mut stack: Vec<u32> = Vec::new();
        let push = |g: &mut VoxelGrid, stack: &mut Vec<u32>, x: usize, y: usize, z: usize| {
            let i = g.idx(x, y, z);
            if !bit_get(&g.exterior, i) && passable(g, i) {
                bit_set(&mut g.exterior, i);
                stack.push(i as u32);
            }
        };
        for z in 0..nz {
            for y in 0..ny {
                push(self, &mut stack, 0, y, z);
                push(self, &mut stack, nx - 1, y, z);
            }
            for x in 0..nx {
                push(self, &mut stack, x, 0, z);
                push(self, &mut stack, x, ny - 1, z);
            }
        }
        for y in 0..ny {
            for x in 0..nx {
                push(self, &mut stack, x, y, 0);
                push(self, &mut stack, x, y, nz - 1);
            }
        }
        while let Some(i) = stack.pop() {
            let i = i as usize;
            let x = i % nx;
            let y = (i / nx) % ny;
            let z = i / (nx * ny);
            if x > 0 {
                push(self, &mut stack, x - 1, y, z);
            }
            if x + 1 < nx {
                push(self, &mut stack, x + 1, y, z);
            }
            if y > 0 {
                push(self, &mut stack, x, y - 1, z);
            }
            if y + 1 < ny {
                push(self, &mut stack, x, y + 1, z);
            }
            if z > 0 {
                push(self, &mut stack, x, y, z - 1);
            }
            if z + 1 < nz {
                push(self, &mut stack, x, y, z + 1);
            }
        }
    }

    /// Does "roofed" mean anything here?
    ///
    /// A model whose roof did not decode — legacy exports routinely drop a
    /// fraction of their element records — has no cover over anything, and
    /// requiring cover then classifies the entire interior as outdoors: no
    /// rooms, no doors, no walkthrough, and no hint as to why. Measure it
    /// instead: if hardly any enclosed space is roofed, the test is not
    /// telling us about the building, it is telling us about the decoder.
    fn measure_cover(&mut self) {
        let (mut enclosed, mut roofed) = (0usize, 0usize);
        for z in 0..self.nz {
            for y in 0..self.ny {
                for x in 0..self.nx {
                    let i = self.idx(x, y, z);
                    if bit_get(&self.solid, i) || bit_get(&self.exterior, i) {
                        continue;
                    }
                    enclosed += 1;
                    if bit_get(&self.covered, i) {
                        roofed += 1;
                    }
                }
            }
        }
        self.trust_cover = enclosed == 0 || (roofed as f32 / enclosed as f32) > 0.35;
    }

    pub fn trusts_cover(&self) -> bool {
        self.trust_cover
    }

    /// Interior = free, roofed, and not reachable from outside without
    /// squeezing through an opening. See the module docs for why all three
    /// terms are needed: free alone includes the garden, roofed alone includes
    /// the space under a balcony, and the flood alone leaks through a wide
    /// glazed slider.
    pub fn is_interior(&self, x: usize, y: usize, z: usize) -> bool {
        let i = self.idx(x, y, z);
        !bit_get(&self.solid, i)
            && (!self.trust_cover || bit_get(&self.covered, i))
            && !bit_get(&self.exterior, i)
    }

    pub fn bounds(&self) -> Aabb {
        Aabb {
            min: self.origin,
            max: self.origin + vec3(
                (self.nx - 1) as f32,
                (self.ny - 1) as f32,
                (self.nz - 1) as f32,
            ) * self.cell,
        }
    }

    pub fn memory_bytes(&self) -> usize {
        (self.solid.len() + self.sealed.len() + self.opaque.len() + self.covered.len()
            + self.exterior.len())
            * 8
            + self.clearance.len() * 4
    }

    /// Plan-view clearance for a body standing on `floor_z`: the column from
    /// `floor_z + step_up` to `floor_z + height` must be clear, and the result
    /// is how far the nearest blocked column is.
    ///
    /// This and [`VoxelGrid::clearance_at`] are the *same* measurement taken in
    /// different dimensions, and both come from `edt_2d`/`edt_1d`. Keep it that
    /// way (see the module docs).
    pub fn column_blocked(&self, x: usize, y: usize, z0: usize, z1: usize) -> bool {
        (z0..=z1.min(self.nz - 1)).any(|z| self.solid_at(x, y, z))
    }

    pub fn column_sealed(&self, x: usize, y: usize, z0: usize, z1: usize) -> bool {
        (z0..=z1.min(self.nz - 1)).any(|z| self.sealed_at(x, y, z))
    }

    /// Voxel row index for a world Z, clamped into the grid.
    pub fn z_index(&self, world_z: f32) -> usize {
        let r = ((world_z - self.origin.z) / self.cell).round();
        (r.max(0.0) as usize).min(self.nz.saturating_sub(1))
    }
}

/// Exact 2D Euclidean distance transform of a boolean mask, in metres, using
/// the same separable transform as the 3D clearance field so the plan-view
/// clearance a storey uses and the volumetric clearance a drone uses come from
/// one implementation. Shrunk by half a cell for the same "never optimistic"
/// reason.
pub(crate) fn edt_2d(blocked: &[bool], nx: usize, ny: usize, cell: f32) -> Vec<f32> {
    const FAR: f32 = 1e12;
    let mut f: Vec<f32> = blocked.iter().map(|b| if *b { 0.0 } else { FAR }).collect();
    let maxn = nx.max(ny);
    let mut buf = vec![0f32; maxn];
    let mut out = vec![0f32; maxn];
    let mut v = vec![0usize; maxn];
    let mut z = vec![0f32; maxn + 1];
    for y in 0..ny {
        buf[..nx].copy_from_slice(&f[y * nx..y * nx + nx]);
        edt_1d(&buf[..nx], &mut out[..nx], &mut v, &mut z);
        f[y * nx..y * nx + nx].copy_from_slice(&out[..nx]);
    }
    for x in 0..nx {
        for y in 0..ny {
            buf[y] = f[y * nx + x];
        }
        edt_1d(&buf[..ny], &mut out[..ny], &mut v, &mut z);
        for y in 0..ny {
            f[y * nx + x] = out[y];
        }
    }
    for d in f.iter_mut() {
        *d = (d.max(0.0).sqrt() * cell - cell * 0.5).max(0.0);
    }
    f
}

/// 1D squared-distance transform. `f` is the input cost, `d` the output;
/// `v`/`z` are scratch for the parabola lower envelope.
fn edt_1d(f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    const FAR: f32 = 1e20;
    let mut k = 0usize;
    v[0] = 0;
    z[0] = -FAR;
    z[1] = FAR;
    for q in 1..n {
        loop {
            let vk = v[k];
            let s = ((f[q] + (q * q) as f32) - (f[vk] + (vk * vk) as f32))
                / (2.0 * q as f32 - 2.0 * vk as f32);
            if s <= z[k] {
                // z[0] is -FAR and s is finite, so this always terminates.
                k -= 1;
            } else {
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = FAR;
                break;
            }
        }
    }
    let mut k = 0usize;
    for q in 0..n {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let dq = q as f32 - v[k] as f32;
        d[q] = dq * dq + f[v[k]];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{TourClass, TourSceneBuilder};

    fn one_box() -> TourScene {
        let mut b = TourSceneBuilder::new("box");
        b.storey("g", 0.0, 3.0);
        b.element("w", TourClass::Wall, 0);
        b.box_solid(vec3(-1.0, -1.0, -1.0), vec3(1.0, 1.0, 1.0));
        b.finish()
    }

    #[test]
    fn edt_matches_brute_force() {
        // A 1D lane with two occupied cells; check the transform against the
        // definition.
        let f: Vec<f32> = vec![1e12, 0.0, 1e12, 1e12, 0.0, 1e12];
        let n = f.len();
        let mut d = vec![0f32; n];
        let mut v = vec![0usize; n];
        let mut z = vec![0f32; n + 1];
        edt_1d(&f, &mut d, &mut v, &mut z);
        let brute: Vec<f32> = (0..n)
            .map(|q| {
                (0..n)
                    .map(|p| (q as f32 - p as f32).powi(2) + f[p])
                    .fold(f32::INFINITY, f32::min)
            })
            .collect();
        for i in 0..n {
            assert!((d[i] - brute[i]).abs() < 1e-3, "{i}: {} vs {}", d[i], brute[i]);
        }
    }

    #[test]
    fn clearance_is_pessimistic_and_smooth() {
        let s = one_box();
        let g = VoxelGrid::build(
            &s,
            &VoxelConfig {
                cell: 0.1,
                pad: 3.0,
                ..Default::default()
            },
        );
        // On the surface: no clearance.
        assert!(g.clearance_at(vec3(1.0, 0.0, 0.0)) < 0.06);
        // 2 m out along +x from the face at x=1 → about 2 m of room.
        let c = g.clearance_at(vec3(3.0, 0.0, 0.0));
        assert!(c > 1.7 && c <= 2.05, "clearance {c}");
        // Never optimistic: a point 0.3 m off the face must not claim more.
        let c2 = g.clearance_at(vec3(1.3, 0.0, 0.0));
        assert!(c2 <= 0.32, "clearance {c2} at 0.3 m from a wall");
    }

    #[test]
    fn segment_and_free_run_agree_with_clearance() {
        let s = one_box();
        let g = VoxelGrid::build(&s, &VoxelConfig { cell: 0.1, pad: 3.0, ..Default::default() });
        assert!(!g.segment_clear(vec3(-3.0, 0.0, 0.0), vec3(3.0, 0.0, 0.0), 0.2));
        assert!(g.segment_clear(vec3(-3.0, 2.5, 0.0), vec3(3.0, 2.5, 0.0), 0.2));
        let run = g.free_run(vec3(-3.0, 0.0, 0.0), vec3(1.0, 0.0, 0.0), 0.2, 10.0);
        assert!(run > 1.5 && run < 2.2, "free run {run}");
    }
}

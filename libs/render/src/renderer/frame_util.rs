//! Frame helpers: light selection, frustum/chunk culling, terrain/water tiles, sun and sky uniforms.

use super::*;

pub(super) fn perf_us(t0: f64) -> u64 {
    ((Cx::monotonic_now() - t0) * 1_000_000.0) as u64
}

/// Fold a baked shade multiplier into an instance colour.
///
/// Emissive surfaces opt out in proportion to their glow: a beacon in a
/// tunnel is the light source, so darkening it reads as a bug rather than
/// as occlusion. Alpha is never touched — for the alpha batch that would
/// change coverage, not brightness.
pub(super) fn shade_color(color: Vec4f, shade: f32, glow: f32) -> Vec4f {
    let k = shade + (1.0 - shade) * glow.clamp(0.0, 1.0);
    vec4(color.x * k, color.y * k, color.z * k, color.w)
}

/// Hard cap on per-draw dynamic lights — 8 pairs of vec4 uniforms, the
/// Quest budget line. Selection below never returns more.
pub const MAX_DYNAMIC_LIGHTS: usize = 8;

/// Brightest component — the "how strong is this light" scalar the
/// selection ranks by.
pub(super) fn light_intensity(l: &crate::lightmap::LmLight) -> f32 {
    l.color.x.max(l.color.y).max(l.color.z)
}

/// The transient flash a bursting firework shell throws on the world.
///
/// `None` while the shell is climbing (negative age) or after its sparks
/// die. Colour comes from the shell; intensity pops hard at the burst (the
/// same quarter-second "heat" window the spark shader whitens with) and
/// decays quadratically over the shell's life. Radius is generous — a shell
/// bursts 30-46 units up and the pool it throws on the street below is the
/// whole point.
pub fn firework_flash_light(
    f: &crate::firework::FireworkInstance,
) -> Option<crate::lightmap::LmLight> {
    if f.age < 0.0 || f.age >= f.life || f.life <= 0.0 {
        return None;
    }
    let t = (f.age / f.life).clamp(0.0, 1.0);
    let fade = (1.0 - t) * (1.0 - t);
    let heat = (1.0 - f.age * 4.0).clamp(0.0, 1.0);
    let s = fade * (1.2 + 1.8 * heat);
    Some(crate::lightmap::LmLight::omni(
        f.origin,
        vec3f(f.color.x * s, f.color.y * s, f.color.z * s),
        55.0,
    ))
}

/// Rank `lights` by their strongest contribution to ANY anchor point and
/// append up to `max` indices to `out`, strongest first, skipping indices
/// already in `out`. Score is intensity × (1 - d/radius)² at the nearest
/// anchor; lights whose radius reaches no anchor are rejected outright.
/// `range` restricts which light indices compete (so lamps and transients
/// can be ranked separately). No allocation in the steady state — `rank`
/// and `out` are caller-owned scratch.
pub fn select_strongest_lights(
    lights: &[crate::lightmap::LmLight],
    range: std::ops::Range<usize>,
    anchors: &[Vec3f],
    max: usize,
    rank: &mut Vec<(f32, usize)>,
    out: &mut Vec<usize>,
) {
    rank.clear();
    for i in range {
        let l = &lights[i];
        if l.radius <= 0.0 || out.contains(&i) {
            continue;
        }
        let mut best_d2 = f32::MAX;
        for a in anchors {
            let (dx, dy, dz) = (a.x - l.pos.x, a.y - l.pos.y, a.z - l.pos.z);
            best_d2 = best_d2.min(dx * dx + dy * dy + dz * dz);
        }
        if best_d2 >= l.radius * l.radius {
            continue; // early radius reject: reaches no anchor
        }
        let att = 1.0 - best_d2.sqrt() / l.radius;
        rank.push((light_intensity(l) * att * att, i));
    }
    rank.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, i) in rank.iter().take(max.min(MAX_DYNAMIC_LIGHTS)) {
        out.push(*i);
    }
}

/// Selection for the WORLD-SPANNING batches (cube slabs, terrain): there is
/// no meaningful batch anchor, so lights are kept when their sphere touches
/// the frustum and ranked by intensity with a soft distance-to-camera bias
/// (no hard radius reject — a shell bursting far from the camera still
/// lights the visible ground beneath it). v1 per-frame selection; a chunked
/// world would want per-chunk lists.
pub(super) fn select_lights_for_world(
    lights: &[crate::lightmap::LmLight],
    range: std::ops::Range<usize>,
    camera: Vec3f,
    frustum: Option<&Frustum>,
    rank: &mut Vec<(f32, usize)>,
    out: &mut Vec<usize>,
) {
    rank.clear();
    out.clear();
    for i in range {
        let l = &lights[i];
        if l.radius <= 0.0 {
            continue;
        }
        if let Some(f) = frustum {
            if !f.intersects_sphere(l.pos, l.radius) {
                continue;
            }
        }
        let (dx, dy, dz) = (camera.x - l.pos.x, camera.y - l.pos.y, camera.z - l.pos.z);
        let d = (dx * dx + dy * dy + dz * dz).sqrt();
        rank.push((light_intensity(l) * l.radius / (l.radius + d), i));
    }
    rank.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, i) in rank.iter().take(MAX_DYNAMIC_LIGHTS) {
        out.push(*i);
    }
}

/// Write a selected light list into a shader's `dl_*` uniforms: 8 pairs of
/// vec4 (pos.xyz + radius, rgb + spot), unfilled slots zeroed so the shader
/// rejects them on radius. `split` is how many leading slots hold TRANSIENT
/// lights — DrawSceneSkinned's per-instance static gate reads it as
/// `dl_split`; shaders without that uniform ignore the write.
///
/// The shader-side spot term assumes emission straight DOWN (the harvested
/// street-lamp convention); a spot light with any other `dir` will still
/// light, but its cone will point down. Fine for v1 — lamps are the only
/// spot sources.
/// Write a pre-packed light block (light_grid.rs layout — 8 × pos/col vec4
/// pairs) straight into a shader's `dl_*` uniforms. The per-instance path:
/// the append test compares dyn uniforms, so instances sharing a block still
/// merge into one draw item and instances in different cells split — which
/// is the point (a car must never flicker because a batch-shared ranking
/// churned under it).
pub(super) fn write_light_block(
    cx: &Cx,
    dv: &mut DrawVars,
    packed: &[f32; LIGHT_BLOCK_FLOATS],
    split: usize,
) {
    let pos_ids = [
        live_id!(dl_pos0),
        live_id!(dl_pos1),
        live_id!(dl_pos2),
        live_id!(dl_pos3),
        live_id!(dl_pos4),
        live_id!(dl_pos5),
        live_id!(dl_pos6),
        live_id!(dl_pos7),
    ];
    let col_ids = [
        live_id!(dl_col0),
        live_id!(dl_col1),
        live_id!(dl_col2),
        live_id!(dl_col3),
        live_id!(dl_col4),
        live_id!(dl_col5),
        live_id!(dl_col6),
        live_id!(dl_col7),
    ];
    for slot in 0..MAX_DYNAMIC_LIGHTS {
        dv.set_uniform(cx, pos_ids[slot], &packed[slot * 8..slot * 8 + 4]);
        dv.set_uniform(cx, col_ids[slot], &packed[slot * 8 + 4..slot * 8 + 8]);
    }
    dv.set_uniform(cx, live_id!(dl_split), &[split as f32]);
}

pub(super) fn write_light_uniforms(
    cx: &Cx,
    dv: &mut DrawVars,
    lights: &[crate::lightmap::LmLight],
    sel: &[usize],
    split: usize,
) {
    let pos_ids = [
        live_id!(dl_pos0),
        live_id!(dl_pos1),
        live_id!(dl_pos2),
        live_id!(dl_pos3),
        live_id!(dl_pos4),
        live_id!(dl_pos5),
        live_id!(dl_pos6),
        live_id!(dl_pos7),
    ];
    let col_ids = [
        live_id!(dl_col0),
        live_id!(dl_col1),
        live_id!(dl_col2),
        live_id!(dl_col3),
        live_id!(dl_col4),
        live_id!(dl_col5),
        live_id!(dl_col6),
        live_id!(dl_col7),
    ];
    for slot in 0..MAX_DYNAMIC_LIGHTS {
        let (p, c) = match sel.get(slot).map(|i| &lights[*i]) {
            Some(l) => (
                [l.pos.x, l.pos.y, l.pos.z, l.radius],
                [l.color.x, l.color.y, l.color.z, l.spot],
            ),
            None => ([0.0; 4], [0.0; 4]),
        };
        dv.set_uniform(cx, pos_ids[slot], &p);
        dv.set_uniform(cx, col_ids[slot], &c);
    }
    dv.set_uniform(cx, live_id!(dl_split), &[split as f32]);
}

/// World-unit slack on every cull test. The bounds themselves are measured
/// (mesh min/max, entity half extents), so this only has to absorb float
/// rounding across the frustum math — but a skipped visible object is a real
/// bug and an extra drawn one is not, so it is sized generously.
pub(super) const CULL_MARGIN: f32 = 0.5;

/// Side of a world-space culling cell. The tradeoff: smaller cells cull
/// tighter but cost more per-frame AABB tests and — for the chunked shadow
/// geometry and terrain tiles, which draw one item per visible cell — more
/// draw items on a tiler. 48 keeps the village in a handful of cells and a
/// Zelda-scale map under ~100, while a ground-level camera still rejects
/// most of them. (The static instance SLABS are chunked too but share the
/// per-shape draw item, so their count is unaffected by this value.)
pub const CHUNK_SIZE: f32 = 48.0;

/// Which grid cell a world-space point lives in. Floor, not truncation:
/// negative coordinates must not fold cell -1 onto cell 0.
pub(super) fn chunk_cell(x: f32, z: f32) -> (i32, i32) {
    (
        (x / CHUNK_SIZE).floor() as i32,
        (z / CHUNK_SIZE).floor() as i32,
    )
}

/// The view frustum in world space — the space instance transforms live in,
/// which is why the clip matrix handed to [`Frustum::from_clip_matrix`] must
/// include the stage transform (shaders compute
/// `projection * view * view_transform * transform`).
///
/// Planes come from the rows of the clip matrix (Gribb-Hartmann) and point
/// INWARD: a point is inside a plane when its signed distance is >= 0. Every
/// test below is conservative — it only rejects what is provably beyond ONE
/// plane, so anything straddling or spanning the frustum always draws.
#[derive(Clone, Copy, Debug)]
pub struct Frustum {
    pub(super) planes: [Vec4f; 6],
}

impl Frustum {
    /// `clip` maps world space to clip space, column-vector convention as
    /// `Mat4f` stores it — row `i` is `v[i], v[i+4], v[i+8], v[i+12]`.
    pub fn from_clip_matrix(clip: &Mat4f) -> Self {
        let row = |i: usize| vec4(clip.v[i], clip.v[i + 4], clip.v[i + 8], clip.v[i + 12]);
        let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
        // Near uses the GL convention (-w <= z). A backend that clips at
        // 0 <= z has a tighter near plane, so testing against the looser one
        // only ever keeps more — conservative in the direction that matters.
        let mut planes = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 + r2, r3 - r2];
        // Normalised so caller margins are world units.
        for p in planes.iter_mut() {
            let len = vec3f(p.x, p.y, p.z).length();
            if len > 1.0e-8 {
                *p = *p * (1.0 / len);
            }
        }
        Self { planes }
    }

    pub(super) fn distance(p: Vec4f, v: Vec3f) -> f32 {
        p.x * v.x + p.y * v.y + p.z * v.z + p.w
    }

    /// Sphere-vs-frustum; false only when the sphere is fully beyond a plane.
    pub fn intersects_sphere(&self, center: Vec3f, radius: f32) -> bool {
        self.planes
            .iter()
            .all(|p| Self::distance(*p, center) >= -(radius + CULL_MARGIN))
    }

    /// World-axis-aligned box vs the frustum, via the positive-vertex trick:
    /// per plane, only the corner most inside can decide "fully outside".
    /// Same conservatism as the corner tests — straddlers always pass.
    pub fn intersects_aabb(&self, min: Vec3f, max: Vec3f) -> bool {
        self.planes.iter().all(|p| {
            let v = vec3f(
                if p.x >= 0.0 { max.x } else { min.x },
                if p.y >= 0.0 { max.y } else { min.y },
                if p.z >= 0.0 { max.z } else { min.z },
            );
            Self::distance(*p, v) >= -CULL_MARGIN
        })
    }

    /// Model-space AABB under `transform` vs the frustum: the 8 transformed
    /// corners, rejected only when all of them sit beyond one plane. That is
    /// exact for "entirely outside plane P" and never false-culls, because a
    /// box with any part inside keeps at least one corner inside every plane's
    /// reject test.
    pub fn intersects_obb(&self, min: Vec3f, max: Vec3f, transform: &Mat4f) -> bool {
        let t = &transform.v;
        let mut corners = [Vec3f::default(); 8];
        for (i, c) in corners.iter_mut().enumerate() {
            let l = vec3f(
                if i & 1 == 0 { min.x } else { max.x },
                if i & 2 == 0 { min.y } else { max.y },
                if i & 4 == 0 { min.z } else { max.z },
            );
            *c = vec3f(
                t[0] * l.x + t[4] * l.y + t[8] * l.z + t[12],
                t[1] * l.x + t[5] * l.y + t[9] * l.z + t[13],
                t[2] * l.x + t[6] * l.y + t[10] * l.z + t[14],
            );
        }
        for p in &self.planes {
            if corners
                .iter()
                .all(|c| Self::distance(*p, *c) < -CULL_MARGIN)
            {
                return false;
            }
        }
        true
    }
}

/// One world-grid cell's packed static instances, per shape and pass. The
/// bounds are CONTENT bounds — the union of what is actually packed here
/// (height included), never the cell footprint — so a tall tower culls by
/// what it is, and a cell whose content leans over the border still tests
/// correctly. Chunks exist only when something packed into them, so an
/// empty cell x shape combination costs nothing anywhere.
pub(super) struct SlabChunk {
    pub(super) cell: (i32, i32),
    pub(super) min: Vec3f,
    pub(super) max: Vec3f,
    pub(super) slab: [Vec<f32>; 5],
    pub(super) slab_alpha: [Vec<f32>; 5],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PrimitiveBucket {
    Opaque,
    Alpha,
}

/// `None` = not drawn at all. `Entity::shell` boxes are pure containment:
/// solid to movement, skipped by the camera boom, and INVISIBLE — an
/// interior is an open stage (user, 2026-08-27: "make these indoor spaces
/// without walls — all you see is a little box you're inside of"). The
/// earlier inward-wound "dollhouse" rendering was wrong twice from a thin
/// wall slab: inside the room you saw its far skin, outside you saw the
/// near wall's inner skin — a box either way.
pub(super) fn primitive_bucket(entity: &Entity) -> Option<PrimitiveBucket> {
    if entity.alpha_primitive {
        Some(PrimitiveBucket::Alpha)
    } else if entity.suppress_primitive {
        None
    } else {
        Some(PrimitiveBucket::Opaque)
    }
}

/// A dynamic cube instance whose colour carries fractional alpha. The opaque
/// cube batch replaces the destination (its shader does not blend), so such
/// instances are held back here and drawn in the alpha pass of the same
/// shape through `DrawSceneAlpha`, after all opaque geometry, where blending
/// sees depth. All-opaque batches are unaffected: nothing is deferred.
pub(super) struct DeferredAlphaCube {
    pub(super) transform: Mat4f,
    pub(super) size: Vec3f,
    pub(super) color: Vec4f,
    pub(super) glow: f32,
    pub(super) color_adjust_ctl: Vec4f,
}

/// Settle delay before the world-settle work (receiver refresh + lightmap
/// kick) runs after an edit. A burst of mutations (a game spawning
/// platforms, a dragged kinematic) pays ONE refresh once the world has been
/// still this long — "only re-render the shadows when an object stops
/// moving" — instead of one per mutation.
pub(super) const SHADOW_SETTLE: f64 = 0.2;

/// Coalescing debounce for the settle work. Pure state machine — the caller
/// supplies the clock — so the burst behaviour is testable without threads
/// or sleeps. However many times the key moves while pending, at most one
/// rebuild fires, for the LATEST key, once it has been stable for the
/// settle window; the very first build runs immediately, because there is
/// no stale state to keep showing in its place.
#[derive(Default)]
pub(super) struct ShadowRebuildGate {
    /// The key the current chunks were built for. None = never built.
    pub(super) built: Option<(u64, u64, u64, u32)>,
    /// Latest key seen since `built`, and when it last CHANGED.
    pub(super) pending: Option<((u64, u64, u64, u32), f64)>,
}

impl ShadowRebuildGate {
    /// True when the caller should rebuild for `key` NOW. Call `mark_built`
    /// after doing so, or the gate will keep saying yes.
    pub(super) fn should_rebuild(
        &mut self,
        key: (u64, u64, u64, u32),
        now: f64,
        settle: f64,
    ) -> bool {
        if self.built == Some(key) {
            self.pending = None;
            return false;
        }
        if self.built.is_none() {
            return true;
        }
        match self.pending {
            // A hair of slack: `t + 0.2 - t` is a few ulps under 0.2 in f64,
            // and a settle that never fires is a rebuild that never comes.
            Some((k, since)) if k == key => now - since + 1.0e-9 >= settle,
            // New key (first change, or changed again mid-wait): the settle
            // clock restarts — the world is still being edited.
            _ => {
                self.pending = Some((key, now));
                false
            }
        }
    }

    pub(super) fn mark_built(&mut self, key: (u64, u64, u64, u32)) {
        self.built = Some(key);
        self.pending = None;
    }
}

/// One tile of the terrain mesh: the same triangles the whole-mesh builder
/// emitted for its cell range, regrouped so offscreen tiles skip the draw.
pub(super) struct TerrainTile {
    pub(super) min: Vec3f,
    pub(super) max: Vec3f,
    pub(super) geometry: Geometry,
    pub(super) shadow_geometry: Option<Geometry>,
}

/// One uploaded voxel chunk mesh. The vertex layout is the terrain tiles'
/// PbrVertex (the sim mesher emits it directly), so DrawSceneTerrain draws
/// both without knowing which is which.
pub(super) struct VoxelTile {
    pub(super) key: ChunkKey,
    pub(super) rev: u64,
    pub(super) min: Vec3f,
    pub(super) max: Vec3f,
    pub(super) geometry: Geometry,
    pub(super) shadow_geometry: Option<Geometry>,
}

/// One `game.water` volume's flat sheet grid plus its packed wave uniforms.
/// The grid is built at the STILL level; every displacement happens in the
/// vertex shader, so the bounds carry the amplitude headroom for culling.
pub(super) struct WaterTile {
    pub(super) min: Vec3f,
    pub(super) max: Vec3f,
    pub(super) geometry: Geometry,
    pub(super) waves_a: [[f32; 4]; MAX_WAVES],
    pub(super) waves_b: [[f32; 4]; MAX_WAVES],
    /// Grid spacing of the sheet (metres): waves shorter than a few cells
    /// cannot be displaced per vertex without aliasing into stripes.
    pub(super) cell: f32,
}

/// Pack a volume's wave list into the shader's uniform slots — the RAW
/// WaterWave fields, bit-for-bit, no unit conversion: this function being
/// trivial IS the CPU/GPU agreement story (the pin test below holds the
/// shader to the same expression over these same numbers). Unused slots are
/// zero; a zero amplitude contributes nothing in the shader.
pub fn pack_wave_uniforms(
    volume: &WaterSurface,
) -> ([[f32; 4]; MAX_WAVES], [[f32; 4]; MAX_WAVES]) {
    let mut a = [[0.0f32; 4]; MAX_WAVES];
    let mut b = [[0.0f32; 4]; MAX_WAVES];
    for (i, w) in volume.waves.iter().take(MAX_WAVES).enumerate() {
        a[i] = [w.dir_x, w.dir_z, w.k, w.omega];
        b[i] = [w.amp, w.phase, w.group, 0.0];
    }
    (a, b)
}

/// Build one volume's sheet grid (PbrVertex layout, indexed). Resolution
/// follows the shortest wavelength — eight cells per wave is enough for the
/// crest to read as a curve — clamped so a huge bay stays a few thousand
/// triangles.
/// The sheet's grid: cells along x and z. A cell is an eighth of the
/// shortest wavelength (0.5..16 m), and at most 128 per side.
fn water_sheet_grid(volume: &WaterSurface) -> (usize, usize) {
    let span_x = (volume.max.x - volume.min.x).max(0.01);
    let span_z = (volume.max.z - volume.min.z).max(0.01);
    let shortest = volume
        .waves
        .iter()
        .map(|w| std::f32::consts::TAU / w.k.max(1.0e-3))
        .fold(f32::MAX, f32::min);
    let target = if shortest == f32::MAX {
        // Still water: a coarse sheet is enough.
        span_x.max(span_z) / 8.0
    } else {
        shortest / 8.0
    }
    .clamp(0.5, 16.0);
    let nx = ((span_x / target).ceil() as usize).clamp(1, 128);
    let nz = ((span_z / target).ceil() as usize).clamp(1, 128);
    (nx, nz)
}

/// The sheet's grid spacing in metres (the longer cell side). On a sea many
/// kilometres wide it is far longer than the waves.
pub(super) fn water_sheet_cell(volume: &WaterSurface) -> f32 {
    let (nx, nz) = water_sheet_grid(volume);
    let span_x = (volume.max.x - volume.min.x).max(0.01);
    let span_z = (volume.max.z - volume.min.z).max(0.01);
    (span_x / nx as f32).max(span_z / nz as f32)
}

pub(super) fn water_sheet_data(volume: &WaterSurface) -> (Vec<f32>, Vec<u32>, Vec3f, Vec3f) {
    let span_x = (volume.max.x - volume.min.x).max(0.01);
    let span_z = (volume.max.z - volume.min.z).max(0.01);
    let (nx, nz) = water_sheet_grid(volume);
    let level = volume.level();
    let color = volume.color;
    let mut vertices: Vec<f32> = Vec::with_capacity((nx + 1) * (nz + 1) * 16);
    for gz in 0..=nz {
        for gx in 0..=nx {
            let x = volume.min.x + span_x * (gx as f32 / nx as f32);
            let z = volume.min.z + span_z * (gz as f32 / nz as f32);
            // PbrVertex: pos_nx, ny_nz_uv, color, tangent — 16 floats. The
            // normal here is a placeholder; the vertex shader replaces it
            // with the analytic wave normal.
            vertices.extend_from_slice(&[
                x, level, z, 0.0, 1.0, 0.0, 0.0, 0.0, color.x, color.y, color.z, color.w,
                1.0, 0.0, 0.0, 1.0,
            ]);
        }
    }
    let stride = (nx + 1) as u32;
    let mut indices: Vec<u32> = Vec::with_capacity(nx * nz * 6);
    for gz in 0..nz as u32 {
        for gx in 0..nx as u32 {
            let a = gz * stride + gx;
            let b = a + 1;
            let c = a + stride;
            let d = c + 1;
            // Same diagonal split as the terrain, CCW seen from +y.
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    let headroom = volume.amp_sum() + 0.5;
    let min = vec3f(volume.min.x, level - headroom, volume.min.z);
    let max = vec3f(volume.max.x, level + headroom, volume.max.z);
    (vertices, indices, min, max)
}

/// Emit terrain triangles for grid cells [gx0..gx1) x [gz0..gz1) — exactly
/// the primitives the single-mesh builder produced for those cells, in the
/// same per-cell order, so tiling regroups the mesh without changing one
/// triangle. Returns (vertices, indices, min, max); bounds come from the
/// emitted vertices, heights included.
pub(super) fn terrain_tile_data(
    terrain: &Terrain,
    materials: Option<&TerrainMaterials>,
    gx0: usize,
    gx1: usize,
    gz0: usize,
    gz1: usize,
) -> (Vec<f32>, Vec<u32>, Vec3f, Vec3f) {
    let n = terrain.cells;
    let mut vertices: Vec<f32> = Vec::with_capacity((gx1 - gx0) * (gz1 - gz0) * 2 * 3 * 16);
    let mut indices: Vec<u32> = Vec::with_capacity((gx1 - gx0) * (gz1 - gz0) * 6);
    let mut min = vec3f(f32::MAX, f32::MAX, f32::MAX);
    let mut max = vec3f(f32::MIN, f32::MIN, f32::MIN);
    let world_pos = |gx: usize, gz: usize| -> Vec3f {
        vec3f(
            terrain.origin + gx as f32 * terrain.cell_size,
            terrain.heights[gz * n + gx],
            terrain.origin + gz as f32 * terrain.cell_size,
        )
    };
    let mut push_tri = |vertices: &mut Vec<f32>, indices: &mut Vec<u32>, a: Vec3f, b: Vec3f, c: Vec3f, color: Vec4f| {
        let normal = Vec3f::cross(b - a, c - a).normalize();
        for p in [a, b, c] {
            // PbrVertex: pos_nx, ny_nz_uv, color, tangent — 16 floats.
            vertices.extend_from_slice(&[
                p.x, p.y, p.z, normal.x, normal.y, normal.z, 0.0, 0.0, color.x, color.y,
                color.z, color.w, 1.0, 0.0, 0.0, 1.0,
            ]);
            indices.push(vertices.len() as u32 / 16 - 1);
            min = vec3f(min.x.min(p.x), min.y.min(p.y), min.z.min(p.z));
            max = vec3f(max.x.max(p.x), max.y.max(p.y), max.z.max(p.z));
        }
    };
    for gz in gz0..gz1 {
        for gx in gx0..gx1 {
            // 0xFF is the hole value (mix.md T4): the voxel field took over
            // this cell's surface — box3d skips it, and so do we.
            if let Some(m) = materials {
                if m.indices.get(gz * (n - 1) + gx) == Some(&0xFF) {
                    continue;
                }
            }
            let a = world_pos(gx, gz);
            let b = world_pos(gx + 1, gz);
            let c = world_pos(gx, gz + 1);
            let d = world_pos(gx + 1, gz + 1);
            let color_at = |gx: usize, gz: usize| terrain.colors[gz * n + gx];
            let c0 = color_at(gx, gz);
            let c1 = color_at(gx + 1, gz);
            let c2 = color_at(gx, gz + 1);
            let c3 = color_at(gx + 1, gz + 1);
            let avg3 = |x: Vec4f, y: Vec4f, z: Vec4f| {
                vec4(
                    (x.x + y.x + z.x) / 3.0,
                    (x.y + y.y + z.y) / 3.0,
                    (x.z + y.z + z.z) / 3.0,
                    1.0,
                )
            };
            // Same diagonal split as Terrain::height_at, CCW seen from +y.
            push_tri(&mut vertices, &mut indices, a, c, b, avg3(c0, c2, c1));
            push_tri(&mut vertices, &mut indices, b, c, d, avg3(c1, c2, c3));
        }
    }
    (vertices, indices, min, max)
}

/// Write one [`SunLight`] into every game shader. This is the whole of the
/// T7 unification on the game side: before it, cube/terrain/skinned each
/// hardcoded their own ambient/direct split and five script blocks set the
/// light direction by hand. Skinned is applied in `draw_skinned_inner`,
/// which owns that struct.
/// Exact sRGB -> linear, per channel (the HDR lane's Rust-side decode of
/// authored colours).
pub fn srgb_to_linear(c: Vec3f) -> Vec3f {
    let f = |v: f32| {
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    vec3f(f(c.x), f(c.y), f(c.z))
}

pub(crate) fn apply_sun(cx: &Cx, draws: &mut SceneDraws, sun: &SunLight, fog_color: Vec3f) {
    // The cube family batches thousands of instances, so its sun and fog
    // colour ride in uniforms; only the direction stays per-instance (it
    // belongs to DrawCube, shared with consumers outside this crate).
    draws.cube.cube.light_dir = sun.dir;
    sun.write_uniforms(cx, &mut draws.cube.cube.draw_vars);
    draws
        .cube
        .cube
        .draw_vars
        .set_uniform(cx, live_id!(fog_color), &[fog_color.x, fog_color.y, fog_color.z]);
    draws.alpha.cube.cube.light_dir = sun.dir;
    sun.write_uniforms(cx, &mut draws.alpha.cube.cube.draw_vars);
    draws.alpha.cube.cube.draw_vars.set_uniform(
        cx,
        live_id!(fog_color),
        &[fog_color.x, fog_color.y, fog_color.z],
    );
    // Terrain and skinned draw one instance each, so there is nothing to
    // save by moving their sun off the instance stream.
    sun.write_into(
        &mut draws.terrain.light_dir,
        &mut draws.terrain.sun_color,
        &mut draws.terrain.sun_sky,
        &mut draws.terrain.sun_ground,
    );
}

/// Does this script-configured sky take the ANALYTIC (Preetham) dome? Yes
/// while the DOME colours are the stock defaults — a script that authored
/// its own top/ground palette keeps the gradient it asked for. The horizon
/// colour is deliberately NOT part of the test: it only ever tinted the fog
/// (the village demo customises exactly that), and analytic mode derives
/// its fog from the model instead.
/// This world's analytic sky frame for `sun_dir`, when it draws one.
pub(super) fn analytic_sky_frame(world: &World, sun_dir: Vec3f, shows_environment: bool) -> Option<crate::sky::SkyFrame> {
    world
        .sky
        .as_ref()
        .filter(|s| shows_environment && sky_wants_analytic(s))
        .map(|sky| {
            let turbidity = if sky.turbidity.is_finite() { sky.turbidity } else { sky_turbidity() };
            let compensation = if sky.exposure_ev.is_finite() {
                2.0f32.powf(sky.exposure_ev.clamp(-12.0, 12.0))
            } else {
                1.0
            };
            crate::sky::preetham_frame(sun_dir, turbidity, sky_exposure() * compensation)
        })
}

pub(super) fn sky_wants_analytic(sky: &makepad_scene::SkyConfig) -> bool {
    let d = makepad_scene::SkyConfig::default();
    let close = |a: Vec4f, b: Vec4f| {
        (a.x - b.x).abs() < 1.0e-3 && (a.y - b.y).abs() < 1.0e-3 && (a.z - b.z).abs() < 1.0e-3
    };
    close(sky.top, d.top) && close(sky.ground, d.ground) && close(sky.ground_bottom, d.ground_bottom)
}

/// Sky turbidity (haze), MAKEPAD_SKY_TURBIDITY overridable; 2.5 is a clear
/// day with enough aerosol for a warm horizon.
pub(super) fn sky_turbidity() -> f32 {
    static V: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("MAKEPAD_SKY_TURBIDITY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2.5)
    })
}

/// Sky exposure for the Preetham tone-map (MAKEPAD_SKY_EXPOSURE); tuned so
/// the default noon matches the hand-painted dome's brightness.
pub(super) fn sky_exposure() -> f32 {
    static V: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("MAKEPAD_SKY_EXPOSURE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.1)
    })
}

/// Desktop-class default; see [`Renderer::set_shadow_budget`].
/// Default bloom share of the HDR composite.
pub(super) const DEFAULT_BLOOM: f32 = 0.05;

pub const DEFAULT_SHADOW_BUDGET: usize = 24;

/// Palette-texture width in texels. Power of two: `(i + 0.5) / width` is then
/// exact in f32, so nearest sampling at texel centres cannot land on a
/// neighbouring matrix row. 128 texels = 42 joints per row; palettes wrap
/// rows freely because the shader addresses texels by flat index.
pub(super) const JOINT_TEX_WIDTH: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WorldModelLane {
    Placed,
    Attachment,
}

impl WorldModelLane {
    pub(super) fn is_dynamic(self, inst: &ModelInstance) -> bool {
        self == Self::Attachment || inst.dynamic
    }

    pub(super) fn light_key(self, slot: usize) -> u64 {
        // Top-bit namespaces are deliberately disjoint from skinned actors
        // (0x8000...) and from each other. Stable slot keys let the light
        // grid's dead-band remember an attached prop without allowing it to
        // disturb a placed car's light selection.
        match self {
            Self::Placed => 0x4000_0000_0000_0000 | slot as u64,
            Self::Attachment => 0xC000_0000_0000_0000 | slot as u64,
        }
    }
}

/// One settled bake's atlas signature, and — for every bake after the
/// first — its DIFF against that first one. Re-baking a world nothing has
/// edited must land on the same bytes; a bake that only ever grows the lit
/// set is an accumulator that was not cleared (gpu_lightmap.rs, section 1,
/// carries the measurement this instrument produced).
///
/// Driven by MAKEPAD_GPU_LM_REBAKE. macOS only: it needs a texture readback.
#[cfg(target_os = "macos")]
pub(super) fn lm_probe_report(
    k: usize,
    w: usize,
    h: usize,
    regions: usize,
    lamps: usize,
    bytes: &[u8],
    rects: &[(crate::lightmap::LmRect, bool)],
) {
    thread_local! {
        static REFERENCE: std::cell::RefCell<Option<Vec<u8>>> =
            const { std::cell::RefCell::new(None) };
    }
    let mut hash = 0xcbf29ce484222325u64;
    let mut sum = [0u64; 4];
    let mut mx = [0u8; 4];
    for px in bytes.chunks_exact(4) {
        for c in 0..4 {
            hash = (hash ^ px[c] as u64).wrapping_mul(0x100000001b3);
            sum[c] += px[c] as u64;
            mx[c] = mx[c].max(px[c]);
        }
    }
    let px_count = (bytes.len() / 4).max(1) as f64;
    log!(
        "lm probe: bake {k} — {w}x{h}, {regions} regions, {lamps} lamp(s), hash {hash:016x}, \
         mean BGRA {:.3} {:.3} {:.3} {:.1}, max {:?}",
        sum[0] as f64 / px_count,
        sum[1] as f64 / px_count,
        sum[2] as f64 / px_count,
        sum[3] as f64 / px_count,
        mx
    );
    REFERENCE.with(|r| {
        let mut r = r.borrow_mut();
        let Some(prev) = r.as_ref() else {
            *r = Some(bytes.to_vec());
            return;
        };
        if prev.len() != bytes.len() {
            log!("lm probe: bake {k} — atlas resized, no diff against bake 0");
            return;
        }
        let (mut diff, mut up, mut down) = (0u64, 0u64, 0u64);
        let mut worst = 0i32;
        let mut worst_at = (0usize, 0usize, 0usize);
        let mut sum_delta = 0i64;
        for (i, (a, b)) in prev.chunks_exact(4).zip(bytes.chunks_exact(4)).enumerate() {
            let mut any = false;
            for c in 0..4 {
                let d = b[c] as i32 - a[c] as i32;
                if d != 0 {
                    any = true;
                    sum_delta += d as i64;
                    if d.abs() > worst.abs() {
                        worst = d;
                        worst_at = (i % w, i / w, c);
                    }
                }
            }
            if any {
                diff += 1;
                let sa: i32 = a[..3].iter().map(|v| *v as i32).sum();
                let sb: i32 = b[..3].iter().map(|v| *v as i32).sum();
                if sb > sa {
                    up += 1;
                } else if sb < sa {
                    down += 1;
                }
            }
        }
        let owner = rects
            .iter()
            .position(|(rc, _)| {
                worst_at.0 >= rc.x
                    && worst_at.0 < rc.x + rc.w
                    && worst_at.1 >= rc.y
                    && worst_at.1 < rc.y + rc.h
            })
            .map(|i| i as i64)
            .unwrap_or(-1);
        log!(
            "lm probe: bake {k} vs 0 — {diff} texels differ (rgb up {up}, down {down}), \
             worst delta {worst} at ({},{}) chan {} region {owner}, sum delta {sum_delta}",
            worst_at.0,
            worst_at.1,
            worst_at.2
        );
    });
}

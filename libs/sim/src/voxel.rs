//! Editable voxel terrain (mix.md D5, §5.5, tasks T1-T7).
//!
//! One chunked voxel field layered over the authored heightfield, two meshers
//! (smooth surface nets + blocky greedy cubes) chosen per declared volume.
//!
//! Representation rules, all load-bearing:
//!
//! - The field is a lattice of SITES on a global grid: site `s: i32³` sits at
//!   world `s * cell`. Each site carries a quantized signed distance
//!   (`i8`, negative = solid, ±127 ≙ ±one cell) and a material byte
//!   (0 = air). Chunks are 32³ sites keyed by `floor(s / 32)`.
//! - **Unedited chunks cost nothing.** A chunk exists only once an edit op
//!   touches it; at that moment it MATERIALIZES: every site is filled from
//!   the base layer (the authored heightfield, quantized identically to what
//!   implicit sampling would produce) and the edit applies on top. Meshing a
//!   chunk samples neighbours implicitly when they are not materialized, so
//!   the combine with the base happens at mesh/query time, never as storage.
//! - **Edits are ops** ([`VoxelOp`]), applied in host tick order. All ops are
//!   idempotent (min/max/assign — never additive), which is what lets the
//!   session layer re-deliver an op after a late-join snapshot without
//!   drift. Same op stream → same chunk bytes → same [`VoxelField::field_hash`].
//! - Determinism: integer lattice, `BTreeMap` chunk maps (sorted iteration),
//!   f32 expressions with a fixed order, no transcendentals, no wall clock.
//!   The mesher computes vertex positions from GLOBAL site coordinates so a
//!   chunk and its neighbour derive bit-identical seam vertices from the
//!   same samples — that is the whole seam-continuity story (T2).
//!
//! Where the heightfield hands over: when a materialized chunk's y-range
//! contains the terrain surface, the heightfield cells fully inside that
//! chunk's footprint are punched to box3d's hole value (0xFF) and the voxel
//! mesh takes over surface duty there (render + collider). Deep chunks (a
//! tunnel under a ridge) punch nothing — the ridge above stays heightfield.

use std::collections::BTreeMap;

use makepad_math::*;

use crate::terrain::{Terrain, TerrainMaterials, TerrainSurface};

/// Sites per chunk axis.
pub const CHUNK: i32 = 32;
const CHUNK_U: usize = CHUNK as usize;
/// Sites per chunk.
pub const CHUNK_SITES: usize = CHUNK_U * CHUNK_U * CHUNK_U;
/// Density value for open air (+1 cell outside the surface).
pub const AIR: i8 = 127;
/// Density value for deep solid.
pub const SOLID: i8 = -127;
/// Hard cap on materialized chunks (64 KB each) — a runaway script cannot
/// swallow the machine. Logged once by the op path when hit.
pub const MAX_CHUNKS: usize = 1024;
/// Chunks remeshed per [`update_world_voxel`] call (T7 budget). Sorted-key
/// order, so the schedule is deterministic given the same dirty set.
pub const REMESH_BUDGET_PER_TICK: usize = 4;

/// Chunk coordinate: `floor(site / CHUNK)` per axis. Ord = (x, y, z)
/// lexicographic — the deterministic iteration order everywhere.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct ChunkKey {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl ChunkKey {
    pub fn of_site(s: [i32; 3]) -> ChunkKey {
        ChunkKey {
            x: s[0].div_euclid(CHUNK),
            y: s[1].div_euclid(CHUNK),
            z: s[2].div_euclid(CHUNK),
        }
    }
    /// Lowest site this chunk owns.
    pub fn base(&self) -> [i32; 3] {
        [self.x * CHUNK, self.y * CHUNK, self.z * CHUNK]
    }
}

/// In-chunk site index, `(z*32 + y)*32 + x` over local coords.
#[inline]
fn site_index(lx: i32, ly: i32, lz: i32) -> usize {
    debug_assert!((0..CHUNK).contains(&lx) && (0..CHUNK).contains(&ly) && (0..CHUNK).contains(&lz));
    ((lz as usize * CHUNK_U) + ly as usize) * CHUNK_U + lx as usize
}

/// One materialized 32³ chunk. `rev` moves on every data change, which is
/// what schedules remeshing, collider swaps and wire resends.
#[derive(Clone, Debug)]
pub struct VoxelChunk {
    /// The COMPOSED density every mesher and probe reads: the history layer
    /// with the plan patch clipped over it.
    pub density: Vec<i8>,
    /// The HISTORY layer alone — the player's digs, fills and tunnels. This
    /// is what persists, replicates as chunk bytes and survives a plan
    /// change; a plan press never writes here, so retracting a railway
    /// cannot leave its berm behind and a snapshot cannot bake it.
    pub history: Vec<i8>,
    pub material: Vec<u8>,
    pub rev: u64,
}

/// One PLAN-layer press: open air above the plane `y` inside the x/z box.
/// `owner` is the plan feature that asked for it (0 = arrived over the
/// wire without provenance), the key a retraction uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanPress {
    pub owner: u64,
    pub min: Vec3f,
    pub max: Vec3f,
    pub y: f32,
}

/// The plan's terrain patch over the voxel layer (worldgen DESIGN.md,
/// amendment B): derived from plan features, rebuilt by every solve,
/// retracted by owner, composed over the history at READ time and never
/// written into chunk bytes. Presses today; lot pads and channel carves
/// take the same shape.
#[derive(Clone, Debug, Default)]
pub struct PlanTerrainPatch {
    pub presses: Vec<PlanPress>,
}

impl PlanTerrainPatch {
    pub fn is_empty(&self) -> bool {
        self.presses.is_empty()
    }

    /// The plan's clip at a world point: the strongest "air above the pad
    /// plane" any press asks for there, `None` when no press covers the
    /// column.
    pub fn clip(&self, w: Vec3f, cell: f32) -> Option<i8> {
        let mut best: Option<i8> = None;
        for p in &self.presses {
            if w.x >= p.min.x && w.x <= p.max.x && w.z >= p.min.z && w.z <= p.max.z {
                let q = VoxelField::quantize((w.y - p.y) / cell);
                best = Some(best.map_or(q, |b| b.max(q)));
            }
        }
        best
    }

    /// History composed with the plan: air wins wherever a press clips.
    pub fn compose(history: i8, clip: Option<i8>) -> i8 {
        match clip {
            Some(q) => history.max(q),
            None => history,
        }
    }
}

/// Which mesher a volume uses (D5: two meshers, one field).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VoxelMode {
    Smooth,
    Blocky,
}

impl VoxelMode {
    pub fn parse(name: &str) -> VoxelMode {
        match name {
            "blocky" | "blocks" | "cubes" => VoxelMode::Blocky,
            _ => VoxelMode::Smooth,
        }
    }
    pub fn to_u8(self) -> u8 {
        match self {
            VoxelMode::Smooth => 0,
            VoxelMode::Blocky => 1,
        }
    }
    pub fn from_u8(v: u8) -> VoxelMode {
        if v == 1 { VoxelMode::Blocky } else { VoxelMode::Smooth }
    }
}

/// A declared style region: chunks whose centre falls inside pick this
/// volume's mesher ([`VoxelField::chunk_mode`]). Historically ops applied
/// ONLY inside declared volumes; the world is now implicitly editable
/// everywhere (one unified destructible ground), so volumes are about LOOK
/// (smooth vs blocky), not permission.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxelVolume {
    pub min: Vec3f,
    pub max: Vec3f,
    pub mode: VoxelMode,
}

impl VoxelVolume {
    fn contains(&self, p: Vec3f) -> bool {
        p.x >= self.min.x
            && p.x <= self.max.x
            && p.y >= self.min.y
            && p.y <= self.max.y
            && p.z >= self.min.z
            && p.z <= self.max.z
    }
}

/// Brush behaviour for [`VoxelOp::Dig`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DigMode {
    /// Remove material (SDF subtraction — `max`).
    Carve,
    /// Add material (SDF union — `min`).
    Fill,
    /// Force the surface to the brush centre's y plane inside the ball.
    Flatten,
}

impl DigMode {
    pub fn parse(name: &str) -> DigMode {
        match name {
            "fill" | "raise" | "add" => DigMode::Fill,
            "flatten" | "flat" | "level" => DigMode::Flatten,
            _ => DigMode::Carve,
        }
    }
    pub fn to_u8(self) -> u8 {
        match self {
            DigMode::Carve => 0,
            DigMode::Fill => 1,
            DigMode::Flatten => 2,
        }
    }
    pub fn from_u8(v: u8) -> DigMode {
        match v {
            1 => DigMode::Fill,
            2 => DigMode::Flatten,
            _ => DigMode::Carve,
        }
    }
}

/// One tick-ordered, host-serialized edit (T6). Every op is idempotent:
/// re-applying it to a field that already took it changes nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VoxelOp {
    Dig {
        pos: Vec3f,
        r: f32,
        mode: DigMode,
        material: u8,
    },
    /// `material` 0 = remove the block.
    SetBlock { x: i32, y: i32, z: i32, material: u8 },
    /// Carve a straight capsule from `from` to `to` (a tunnel through a
    /// hill). Materializes only chunks the capsule actually passes through,
    /// never the whole segment AABB.
    Tunnel { from: Vec3f, to: Vec3f, r: f32 },
    /// Foundation-press composition: open air above the plane `y` inside the
    /// x/z rectangle `min..max`, up to `max.y`. Emitted when a structure pad
    /// lies under materialized chunks — the voxel twin of the heightfield
    /// press. Never materializes new chunks (untouched ground already obeys
    /// the pressed heightfield implicitly).
    Press { min: Vec3f, max: Vec3f, y: f32 },
    /// Macro landform (mountain / valley / crater / ...): the heightfield
    /// part is applied by [`crate::landform`]; the voxel part composes into
    /// already-materialized chunks. `pos.y` is the captured base height,
    /// `kind` is [`crate::landform::LandKind::to_u8`]. Never dispatched
    /// through [`VoxelField::apply_op`] — the world-level appliers own it.
    Landform { pos: Vec3f, kind: u8, r: f32, height: f32, seed: u32 },
}

/// One remeshed chunk, in the renderer's 16-float PbrVertex layout
/// (`pos3 | normal3 | uv2 | color4 | tangent4` — the exact layout the
/// heightfield tiles use, so the voxel mesh rides the same terrain shader).
/// The collider reads positions back out of the same vertices — collision
/// IS the visual, by construction.
#[derive(Clone, Debug, Default)]
pub struct ChunkMesh {
    /// Bumped on every rebuild: renderer re-uploads, dynamics hot-swaps.
    pub rev: u64,
    pub verts: Vec<f32>,
    pub indices: Vec<u32>,
    pub min: Vec3f,
    pub max: Vec3f,
}

pub const MESH_VERTEX_FLOATS: usize = 16;

/// What implicit (non-materialized) sites read as during meshing.
#[derive(Clone, Copy)]
pub enum BaseSample<'a> {
    /// The authority's view: the authored heightfield, or the y=0 ground
    /// plane when the world has none — the same base ops materialize from.
    World(Option<&'a Terrain>),
    /// A replica's view: it has no base layer, so missing samples clamp to
    /// the chunk's own edge — dead neighbours never invent crossings.
    /// Visual-only (meshes never cross the wire).
    Clamp,
}

/// Default material palette: index 0 is air (never drawn), 1.. are solids.
pub fn default_palette() -> Vec<Vec4f> {
    vec![
        vec4f(0.0, 0.0, 0.0, 0.0),   // 0 air
        vec4f(0.45, 0.36, 0.26, 1.0), // 1 dirt (the base-layer fill)
        vec4f(0.35, 0.62, 0.33, 1.0), // 2 grass
        vec4f(0.52, 0.52, 0.55, 1.0), // 3 rock
        vec4f(0.78, 0.71, 0.48, 1.0), // 4 sand
    ]
}

/// Material the base layer materializes as.
pub const BASE_MATERIAL: u8 = 1;

/// The chunked voxel field. Lives on `GameWorld` as `Option<Box<...>>` so
/// worlds that never call `game.terrain_volume` carry a null pointer and the
/// old code paths byte-identically.
#[derive(Clone)]
pub struct VoxelField {
    /// Site spacing in world units. Fixed at creation; volumes declared with
    /// a different cell keep the field's (logged by the verb).
    pub cell: f32,
    pub palette: Vec<Vec4f>,
    /// Material the top ~2.5 cells of base ground materialize as (the rest
    /// is [`BASE_MATERIAL`] dirt). Defaults to dirt — byte-identical to the
    /// original behavior; the implicit whole-world field sets it to the
    /// grass slot so an untouched materialized surface reads as the map,
    /// and digging exposes dirt beneath.
    pub surface_material: u8,
    pub volumes: Vec<VoxelVolume>,
    /// Bumped when volumes / palette / cell change — the session layer
    /// rebroadcasts the structure snapshot when it moves.
    pub structure_rev: u64,
    /// Materialized chunks. BTreeMap: sorted, deterministic iteration.
    chunks: BTreeMap<ChunkKey, VoxelChunk>,
    /// Meshes for materialized chunks, rebuilt under the tick budget.
    pub meshes: BTreeMap<ChunkKey, ChunkMesh>,
    /// Chunks whose data changed since their last remesh. Sorted + deduped.
    dirty: Vec<ChunkKey>,
    /// Ops applied since the session last drained (host replication). The
    /// session drains every tick in every role; the cap is a leak guard for
    /// hosts that never pump a session.
    pub pending_ops: Vec<VoxelOp>,
    /// Chunks materialized since the session last drained — late-join /
    /// first-sight chunk snapshots ride these.
    pub fresh_chunks: Vec<ChunkKey>,
    /// Macro landform ops applied to this world, in order (heightfield-scale
    /// terrain surgery — see [`crate::landform`]). Kept across script
    /// re-evals so the mountains an AI raised REPLAY onto the freshly
    /// rebuilt heightfield; ride the structure snapshot on the wire so late
    /// joiners and reloading clients replay the same list.
    pub land_ops: Vec<VoxelOp>,
    /// Set when `land_ops` must re-apply (after a re-eval rebuilt the
    /// terrain, or a structure snapshot delivered the list). Drained by
    /// [`crate::landform::replay_land_ops`] once a terrain exists.
    pub land_replay_pending: bool,
    /// Ops since the last persistence flush — drained by the host app's
    /// DEBOUNCED terrain saver (parallel to `pending_ops`, which the session
    /// drains every tick for the wire). Only the authority records here.
    pub persist_ops: Vec<VoxelOp>,
    /// The persist queue hit its cap and was cleared — the saver must cut a
    /// full snapshot instead of appending a tail.
    pub persist_overflow: bool,
    /// Terrain revision the heightfield hole-punch was last applied against.
    pub punch_rev: Option<u64>,
    /// The plan layer over this field — see [`PlanTerrainPatch`]. Cleared on
    /// every reset_content (the eval re-registers every press it still
    /// wants), never persisted, never part of a chunk blob.
    pub plan: PlanTerrainPatch,
    /// Monotonic mesh revision source.
    mesh_rev: u64,
    /// One-shot "chunk cap hit" log latch.
    cap_logged: bool,
}

impl VoxelField {
    pub fn new(cell: f32) -> Self {
        Self {
            cell: if cell.is_finite() { cell.clamp(0.1, 4.0) } else { 0.5 },
            palette: default_palette(),
            surface_material: BASE_MATERIAL,
            volumes: Vec::new(),
            structure_rev: 1,
            chunks: BTreeMap::new(),
            meshes: BTreeMap::new(),
            dirty: Vec::new(),
            pending_ops: Vec::new(),
            fresh_chunks: Vec::new(),
            land_ops: Vec::new(),
            land_replay_pending: false,
            persist_ops: Vec::new(),
            persist_overflow: false,
            punch_rev: None,
            plan: PlanTerrainPatch::default(),
            mesh_rev: 0,
            cap_logged: false,
        }
    }

    pub fn declare_volume(&mut self, min: Vec3f, max: Vec3f, mode: VoxelMode) -> usize {
        let volume = VoxelVolume {
            min: vec3f(min.x.min(max.x), min.y.min(max.y), min.z.min(max.z)),
            max: vec3f(min.x.max(max.x), min.y.max(max.y), min.z.max(max.z)),
            mode,
        };
        // Re-declaring the same volume (hot-reload re-runs the script) must
        // not churn the structure revision or the wire.
        if let Some(at) = self.volumes.iter().position(|v| *v == volume) {
            return at;
        }
        self.volumes.push(volume);
        self.structure_rev += 1;
        // Materialized chunks the new volume covers may mesh differently
        // (mode) — remesh them.
        let keys: Vec<ChunkKey> = self.chunks.keys().copied().collect();
        for key in keys {
            self.mark_dirty(key);
        }
        self.volumes.len() - 1
    }

    pub fn set_palette(&mut self, palette: Vec<Vec4f>) {
        if palette.len() >= 2 && palette != self.palette {
            self.palette = palette;
            self.structure_rev += 1;
            let keys: Vec<ChunkKey> = self.chunks.keys().copied().collect();
            for key in keys {
                self.mark_dirty(key);
            }
        }
    }

    // ── sampling ────────────────────────────────────────────────────────

    /// World position of a site.
    #[inline]
    pub fn site_world(&self, s: [i32; 3]) -> Vec3f {
        vec3f(s[0] as f32 * self.cell, s[1] as f32 * self.cell, s[2] as f32 * self.cell)
    }

    /// Site containing a world position (floor).
    #[inline]
    pub fn world_site(&self, p: Vec3f) -> [i32; 3] {
        [
            (p.x / self.cell).floor() as i32,
            (p.y / self.cell).floor() as i32,
            (p.z / self.cell).floor() as i32,
        ]
    }

    /// Quantize a signed distance measured in CELLS into the i8 band.
    #[inline]
    fn quantize(d_cells: f32) -> i8 {
        (d_cells.clamp(-1.0, 1.0) * 127.0) as i8
    }

    /// The base (heightfield) layer's density at a site: vertical distance to
    /// the authored surface, in cells, quantized. No terrain = ground plane
    /// at y=0, so a bare world still has something to dig into.
    fn base_density(&self, s: [i32; 3], base: Option<&Terrain>) -> i8 {
        let w = self.site_world(s);
        let h = base.and_then(|t| t.height_at(w.x, w.z)).unwrap_or(0.0);
        Self::quantize((w.y - h) / self.cell)
    }

    /// Density at a site: chunk data if materialized, else the base layer.
    pub fn density_at(&self, s: [i32; 3], base: Option<&Terrain>) -> i8 {
        let key = ChunkKey::of_site(s);
        match self.chunks.get(&key) {
            Some(c) => {
                let b = key.base();
                c.density[site_index(s[0] - b[0], s[1] - b[1], s[2] - b[2])]
            }
            None => self.base_density(s, base),
        }
    }

    pub fn material_at(&self, s: [i32; 3]) -> u8 {
        let key = ChunkKey::of_site(s);
        match self.chunks.get(&key) {
            Some(c) => {
                let b = key.base();
                c.material[site_index(s[0] - b[0], s[1] - b[1], s[2] - b[2])]
            }
            None => BASE_MATERIAL,
        }
    }

    /// Is this world point inside carved-open air of a materialized chunk?
    /// The mover integration uses it to decide "legitimately underground"
    /// (suppress the terrain floor snap) vs "clipped under the heightfield".
    /// STRICTLY positive: a quantized-zero site is the base surface itself
    /// (a mover standing on flat materialized ground), not open air — the
    /// terrain rules must keep applying there.
    pub fn is_carved_air(&self, p: Vec3f) -> bool {
        let s = self.world_site(p);
        let key = ChunkKey::of_site(s);
        match self.chunks.get(&key) {
            Some(c) => {
                let b = key.base();
                c.density[site_index(s[0] - b[0], s[1] - b[1], s[2] - b[2])] > 0
            }
            None => false,
        }
    }

    /// Is this world point inside materialized solid?
    pub fn is_solid_at(&self, p: Vec3f) -> bool {
        let s = self.world_site(p);
        let key = ChunkKey::of_site(s);
        match self.chunks.get(&key) {
            Some(c) => {
                let b = key.base();
                c.density[site_index(s[0] - b[0], s[1] - b[1], s[2] - b[2])] < 0
            }
            None => false,
        }
    }

    /// Any solid site inside this world AABB (materialized chunks only)?
    /// The mover x/z sweeps use it as their voxel wall test.
    pub fn solid_in_box(&self, min: Vec3f, max: Vec3f) -> bool {
        if self.chunks.is_empty() {
            return false;
        }
        let lo = self.world_site(min);
        let hi = self.world_site(max);
        for z in lo[2]..=hi[2] {
            for y in lo[1]..=hi[1] {
                for x in lo[0]..=hi[0] {
                    let key = ChunkKey::of_site([x, y, z]);
                    if let Some(c) = self.chunks.get(&key) {
                        let b = key.base();
                        if c.density[site_index(x - b[0], y - b[1], z - b[2])] < 0 {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// Highest air→solid crossing at (x, z) scanning DOWN from `from_y`,
    /// through materialized data only. Returns the surface height. The scan
    /// stops when it leaves materialized chunks (the base layer takes over).
    pub fn floor_probe(&self, x: f32, z: f32, from_y: f32) -> Option<f32> {
        if self.chunks.is_empty() {
            return None;
        }
        let sx = (x / self.cell).floor() as i32;
        let sz = (z / self.cell).floor() as i32;
        // Column bounds: lowest/highest materialized chunk at this (x, z).
        let cx = sx.div_euclid(CHUNK);
        let cz = sz.div_euclid(CHUNK);
        let mut top = i32::MIN;
        let mut bottom = i32::MAX;
        let mut any = false;
        for key in self.chunks.keys() {
            if key.x == cx && key.z == cz {
                any = true;
                top = top.max((key.y + 1) * CHUNK - 1);
                bottom = bottom.min(key.y * CHUNK);
            }
        }
        if !any {
            return None;
        }
        let start = ((from_y / self.cell).floor() as i32).min(top);
        let mut prev: Option<i8> = None;
        let mut sy = start;
        while sy >= bottom {
            let key = ChunkKey::of_site([sx, sy, sz]);
            let Some(c) = self.chunks.get(&key) else {
                // Left the materialized column — base layer's business below.
                return None;
            };
            let b = key.base();
            let d = c.density[site_index(sx - b[0], sy - b[1], sz - b[2])];
            if d < 0 {
                // Blocky regions stand on the CUBE face (site+1), matching
                // the greedy mesh and its collider; smooth regions stand on
                // the interpolated SDF crossing, matching surface nets.
                if self.chunk_mode(key) == VoxelMode::Blocky {
                    return Some((sy + 1) as f32 * self.cell);
                }
                let y_solid = sy as f32 * self.cell;
                return Some(match prev {
                    Some(d_air) if d_air >= 0 => {
                        let t = d as f32 / (d as f32 - d_air as f32);
                        y_solid + self.cell * t
                    }
                    _ => y_solid,
                });
            }
            prev = Some(d);
            sy -= 1;
        }
        None
    }

    /// Voxel floor under a mover footprint (same 5-probe pattern as
    /// [`Terrain::floor_under`]), scanning down from `from_y`. The corner
    /// probes sit slightly INSIDE the footprint: the wall test
    /// ([`Self::solid_in_box`]) uses the full extents, so a wall column
    /// always blocks sideways motion before a floor probe can land in it
    /// and stair-lift the mover up the wall.
    pub fn floor_under(&self, pos: Vec3f, half: Vec3f, from_y: f32) -> Option<f32> {
        if self.chunks.is_empty() {
            return None;
        }
        let hx = (half.x - 0.05).max(0.0);
        let hz = (half.z - 0.05).max(0.0);
        let probes = [
            (pos.x, pos.z),
            (pos.x - hx, pos.z - hz),
            (pos.x + hx, pos.z - hz),
            (pos.x - hx, pos.z + hz),
            (pos.x + hx, pos.z + hz),
        ];
        let mut best: Option<f32> = None;
        for (x, z) in probes {
            if let Some(h) = self.floor_probe(x, z, from_y) {
                best = Some(best.map_or(h, |b: f32| b.max(h)));
            }
        }
        best
    }

    /// THE surface-seam query (world-surface composition): the voxel layer's
    /// surface height at (x, z) IF it owns the surface there, else `None`
    /// (the heightfield's business). Ownership is the punch rule: the base
    /// surface crossing at `base_h` — its air site AND the solid site below —
    /// must both be materialized. A deep tunnel chunk under an untouched
    /// ridge owns nothing; a pit carved at the surface, or a mound filled on
    /// top of it, owns the column and answers with the topmost air→solid
    /// crossing in materialized data. `None` also when the column was carved
    /// clean through everything materialized (the caller falls back to the
    /// heightfield, which is what the base layer below would say).
    /// Does a materialized column own the surface at (x, z)? The sites just
    /// above and below the base surface are both in chunks — the punch rule.
    /// Ownership without a floor below means a HOLE, not "ask the
    /// heightfield" (see `GameWorld::surface_sample_at`).
    pub fn owns_surface(&self, x: f32, z: f32, base_h: f32) -> bool {
        if self.chunks.is_empty() {
            return false;
        }
        let air = self.world_site(vec3f(x, base_h, z));
        let solid = [air[0], air[1] - 1, air[2]];
        self.chunks.contains_key(&ChunkKey::of_site(air))
            && self.chunks.contains_key(&ChunkKey::of_site(solid))
    }

    pub fn surface_at(&self, x: f32, z: f32, base_h: f32) -> Option<f32> {
        if self.chunks.is_empty() {
            return None;
        }
        let air = self.world_site(vec3f(x, base_h, z));
        let solid = [air[0], air[1] - 1, air[2]];
        if !self.chunks.contains_key(&ChunkKey::of_site(air))
            || !self.chunks.contains_key(&ChunkKey::of_site(solid))
        {
            return None;
        }
        // Scan from above every materialized chunk: floor_probe clamps its
        // start into the column's own top.
        self.floor_probe(x, z, 1.0e9)
    }

    // ── ops ─────────────────────────────────────────────────────────────

    /// Site AABB an op can touch (inclusive), for chunk materialization and
    /// dirty marking.
    fn op_site_bounds(&self, op: &VoxelOp) -> ([i32; 3], [i32; 3]) {
        match *op {
            VoxelOp::Dig { pos, r, .. } => {
                let r = r.abs();
                (
                    self.world_site(vec3f(pos.x - r, pos.y - r, pos.z - r)),
                    {
                        let hi = vec3f(pos.x + r, pos.y + r, pos.z + r);
                        [
                            (hi.x / self.cell).ceil() as i32,
                            (hi.y / self.cell).ceil() as i32,
                            (hi.z / self.cell).ceil() as i32,
                        ]
                    },
                )
            }
            VoxelOp::SetBlock { x, y, z, .. } => ([x, y, z], [x + 1, y + 1, z + 1]),
            VoxelOp::Tunnel { from, to, r } => {
                let r = r.abs();
                let lo = vec3f(
                    from.x.min(to.x) - r,
                    from.y.min(to.y) - r,
                    from.z.min(to.z) - r,
                );
                let hi = vec3f(
                    from.x.max(to.x) + r,
                    from.y.max(to.y) + r,
                    from.z.max(to.z) + r,
                );
                (
                    self.world_site(lo),
                    [
                        (hi.x / self.cell).ceil() as i32,
                        (hi.y / self.cell).ceil() as i32,
                        (hi.z / self.cell).ceil() as i32,
                    ],
                )
            }
            VoxelOp::Press { min, max, y } => (
                self.world_site(vec3f(min.x, y - self.cell, min.z)),
                [
                    (max.x / self.cell).ceil() as i32,
                    (max.y / self.cell).ceil() as i32,
                    (max.z / self.cell).ceil() as i32,
                ],
            ),
            // Landform never reaches the generic path (the world-level
            // appliers in `crate::landform` own it); bounds are its region.
            VoxelOp::Landform { pos, r, height, .. } => {
                let reach = r.abs() * 2.1;
                let h = height.abs() * 1.2 + 2.0 * self.cell;
                (
                    self.world_site(vec3f(pos.x - reach, pos.y - h, pos.z - reach)),
                    {
                        let hi = vec3f(pos.x + reach, pos.y + h, pos.z + reach);
                        [
                            (hi.x / self.cell).ceil() as i32,
                            (hi.y / self.cell).ceil() as i32,
                            (hi.z / self.cell).ceil() as i32,
                        ]
                    },
                )
            }
        }
    }

    /// Squared distance from a point to the segment `a..b` — the Tunnel
    /// capsule test, and the chunk filter that keeps a long tunnel from
    /// materializing its whole AABB.
    fn seg_dist_sq(p: Vec3f, a: Vec3f, b: Vec3f) -> f32 {
        let ab = b - a;
        let len_sq = ab.x * ab.x + ab.y * ab.y + ab.z * ab.z;
        let t = if len_sq > 1.0e-8 {
            let ap = p - a;
            ((ap.x * ab.x + ap.y * ab.y + ap.z * ab.z) / len_sq).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let c = vec3f(a.x + ab.x * t, a.y + ab.y * t, a.z + ab.z * t);
        let d = p - c;
        d.x * d.x + d.y * d.y + d.z * d.z
    }

    /// Materialize the chunk containing `s` from the base layer, if the cap
    /// allows. Returns true when the chunk exists after the call.
    fn materialize(&mut self, key: ChunkKey, base: Option<&Terrain>, log: &mut Vec<String>) -> bool {
        if self.chunks.contains_key(&key) {
            return true;
        }
        if self.chunks.len() >= MAX_CHUNKS {
            if !self.cap_logged {
                self.cap_logged = true;
                log.push(format!(
                    "voxel: chunk cap reached ({MAX_CHUNKS}) — further edits outside \
                     materialized chunks are ignored"
                ));
            }
            return false;
        }
        let b = key.base();
        let mut density = vec![0i8; CHUNK_SITES];
        let mut material = vec![0u8; CHUNK_SITES];
        for lz in 0..CHUNK {
            for lx in 0..CHUNK {
                // One height lookup per column, not per site.
                let w = self.site_world([b[0] + lx, 0, b[2] + lz]);
                let h = base.and_then(|t| t.height_at(w.x, w.z)).unwrap_or(0.0);
                for ly in 0..CHUNK {
                    let y = (b[1] + ly) as f32 * self.cell;
                    let d = Self::quantize((y - h) / self.cell);
                    let at = site_index(lx, ly, lz);
                    density[at] = d;
                    material[at] = if d < 0 {
                        // Topsoil band: the untouched surface keeps the
                        // map's look; digging exposes dirt beneath.
                        if h - y < self.cell * 2.5 { self.surface_material } else { BASE_MATERIAL }
                    } else {
                        0
                    };
                }
            }
        }
        // The base layer IS history at birth (nothing has been dug yet);
        // the plan clips over it at read time.
        let history = density;
        let density = self.compose_history(key, &history);
        self.chunks.insert(
            key,
            VoxelChunk {
                density,
                history,
                material,
                rev: 1,
            },
        );
        self.fresh_chunks.push(key);
        self.mark_dirty(key);
        // A new chunk may take over surface duty from the heightfield —
        // force the hole-punch pass to re-run (idempotent per cell).
        self.punch_rev = None;
        true
    }

    fn mark_dirty(&mut self, key: ChunkKey) {
        if let Err(at) = self.dirty.binary_search(&key) {
            self.dirty.insert(at, key);
        }
    }

    /// Apply one op. `materialize` is true on the authority (host/local):
    /// chunks spring into existence under the brush. The wire-apply path
    /// passes false — a client edits only chunks it was handed, and receives
    /// fresh chunks as snapshots instead (it has no base layer to fill from).
    /// `record` queues the op for replication (authority only).
    pub fn apply_op(
        &mut self,
        op: VoxelOp,
        base: Option<&Terrain>,
        materialize: bool,
        record: bool,
        log: &mut Vec<String>,
    ) {
        // A press is PLAN, not history: it registers in the patch (owner 0 —
        // the wire carries no provenance) and composes at read time. The
        // op still records so replicas compose the same patch.
        if let VoxelOp::Press { min, max, y } = op {
            self.plan_press(0, min, max, y);
            if record {
                self.record_op(op);
            }
            return;
        }
        let (lo, hi) = self.op_site_bounds(&op);
        // Materialize every chunk the op's bounds touch — the WHOLE world is
        // implicitly editable; lazy materialization is what keeps that free
        // until an edit lands. Sorted chunk-key order (x, then y, then z).
        // Press never materializes (it only removes solid that exists), and
        // Tunnel materializes only chunks its capsule actually passes near —
        // a long diagonal tunnel must not swallow its whole AABB.
        let wants_chunks = !matches!(op, VoxelOp::Press { .. });
        if materialize && wants_chunks {
            let k0 = ChunkKey::of_site(lo);
            let k1 = ChunkKey::of_site(hi);
            for kx in k0.x..=k1.x {
                for ky in k0.y..=k1.y {
                    for kz in k0.z..=k1.z {
                        let key = ChunkKey { x: kx, y: ky, z: kz };
                        if let VoxelOp::Tunnel { from, to, r } = op {
                            let cmin = self.site_world(key.base());
                            let half = CHUNK as f32 * self.cell * 0.5;
                            let center = vec3f(cmin.x + half, cmin.y + half, cmin.z + half);
                            // Conservative: chunk half-diagonal as slack.
                            let slack = r.abs() + half * 1.7321 + self.cell;
                            if Self::seg_dist_sq(center, from, to) > slack * slack {
                                continue;
                            }
                        }
                        self.materialize(key, base, log);
                    }
                }
            }
        }

        // The edit itself — chunk-major over materialized chunks whose site
        // range intersects the op bounds (a per-site map lookup over a long
        // tunnel's AABB would be millions of misses; the chunk set is ≤
        // MAX_CHUNKS by construction).
        let mut changed_any = false;
        let cell = self.cell;
        let keys: Vec<ChunkKey> = self
            .chunks
            .keys()
            .filter(|k| {
                let b = k.base();
                b[0] <= hi[0]
                    && b[0] + CHUNK > lo[0]
                    && b[1] <= hi[1]
                    && b[1] + CHUNK > lo[1]
                    && b[2] <= hi[2]
                    && b[2] + CHUNK > lo[2]
            })
            .copied()
            .collect();
        let plan = self.plan.clone();
        for key in keys {
            let b = key.base();
            let (x0, x1) = (lo[0].max(b[0]), hi[0].min(b[0] + CHUNK - 1));
            let (y0, y1) = (lo[1].max(b[1]), hi[1].min(b[1] + CHUNK - 1));
            let (z0, z1) = (lo[2].max(b[2]), hi[2].min(b[2] + CHUNK - 1));
            let Some(chunk) = self.chunks.get_mut(&key) else {
                continue;
            };
            for sz in z0..=z1 {
                for sy in y0..=y1 {
                    for sx in x0..=x1 {
                    let s = [sx, sy, sz];
                    let w = vec3f(s[0] as f32 * cell, s[1] as f32 * cell, s[2] as f32 * cell);
                    let at = site_index(sx - b[0], sy - b[1], sz - b[2]);
                    // History ops read and write the HISTORY layer; the
                    // composed byte is re-derived from it below.
                    let old_d = chunk.history[at];
                    let old_m = chunk.material[at];
                    let (new_d, new_m) = match op {
                        VoxelOp::Dig { pos, r, mode, material } => {
                            let d = w - pos;
                            let dist = crate::vec3_len(d);
                            match mode {
                                DigMode::Carve => {
                                    // SDF subtract: d = max(d, -(sphere sdf)).
                                    let q = Self::quantize((r - dist) / cell);
                                    let nd = old_d.max(q);
                                    (nd, if nd >= 0 { 0 } else { old_m })
                                }
                                DigMode::Fill => {
                                    // SDF union: d = min(d, sphere sdf).
                                    let q = Self::quantize((dist - r) / cell);
                                    let nd = old_d.min(q);
                                    let nm = if dist < r && nd < 0 {
                                        material.max(1)
                                    } else {
                                        old_m
                                    };
                                    (nd, nm)
                                }
                                DigMode::Flatten => {
                                    if dist < r {
                                        let nd = Self::quantize((w.y - pos.y) / cell);
                                        let nm = if nd < 0 {
                                            if old_d < 0 { old_m } else { material.max(1) }
                                        } else {
                                            0
                                        };
                                        (nd, nm)
                                    } else {
                                        (old_d, old_m)
                                    }
                                }
                            }
                        }
                        VoxelOp::SetBlock { x, y, z, material } => {
                            if sx == x && sy == y && sz == z {
                                if material == 0 {
                                    (AIR, 0)
                                } else {
                                    (SOLID, material)
                                }
                            } else {
                                (old_d, old_m)
                            }
                        }
                        VoxelOp::Tunnel { from, to, r } => {
                            // Capsule subtract: air within r of the segment.
                            let dist = Self::seg_dist_sq(w, from, to).sqrt();
                            let q = Self::quantize((r - dist) / cell);
                            let nd = old_d.max(q);
                            (nd, if nd >= 0 { 0 } else { old_m })
                        }
                        // Routed into the plan patch above; never a site op.
                        VoxelOp::Press { .. } => (old_d, old_m),
                        // World-level appliers own it (crate::landform);
                        // reaching here is a routing bug, kept harmless.
                        VoxelOp::Landform { .. } => (old_d, old_m),
                    };
                    if new_d != old_d || new_m != old_m {
                        chunk.history[at] = new_d;
                        chunk.density[at] = PlanTerrainPatch::compose(new_d, plan.clip(w, cell));
                        chunk.material[at] = new_m;
                        changed_any = true;
                        chunk.rev += 1;
                    }
                    }
                }
            }
        }

        if changed_any {
            // Dirty every materialized chunk whose mesh sampled the changed
            // sites — the op bounds expanded by one site cover every apron.
            let k0 = ChunkKey::of_site([lo[0] - 1, lo[1] - 1, lo[2] - 1]);
            let k1 = ChunkKey::of_site([hi[0] + 1, hi[1] + 1, hi[2] + 1]);
            for kx in k0.x..=k1.x {
                for ky in k0.y..=k1.y {
                    for kz in k0.z..=k1.z {
                        let key = ChunkKey { x: kx, y: ky, z: kz };
                        if self.chunks.contains_key(&key) {
                            self.mark_dirty(key);
                        }
                    }
                }
            }
        }
        if record {
            self.record_op(op);
        }
    }

    fn record_op(&mut self, op: VoxelOp) {
        // Leak guard for hosts that never pump a session (raw sim tests):
        // the session drains this every tick in every role.
        if self.pending_ops.len() < 65536 {
            self.pending_ops.push(op);
        }
        // The persistence tail (drained by the app's debounced saver).
        if self.persist_ops.len() < 8192 {
            self.persist_ops.push(op);
        } else {
            self.persist_overflow = true;
            self.persist_ops.clear();
        }
    }

    // ── the plan layer ──────────────────────────────────────────────────

    /// Register a plan press (a foundation pad, a corridor bed) keyed by its
    /// owning feature. Composed over the history at read time; the chunk
    /// bytes never change, so a retraction is exact and a snapshot never
    /// bakes it.
    pub fn plan_press(&mut self, owner: u64, min: Vec3f, max: Vec3f, y: f32) {
        let press = PlanPress {
            owner,
            min: vec3f(min.x.min(max.x), min.y.min(max.y), min.z.min(max.z)),
            max: vec3f(min.x.max(max.x), min.y.max(max.y), min.z.max(max.z)),
            y,
        };
        if self.plan.presses.contains(&press) {
            return;
        }
        self.plan.presses.push(press);
        self.recompose_region(press.min, press.max);
    }

    /// Drop every plan press `owner` registered and give the ground back to
    /// its history — the berm leaves with the railway.
    pub fn retract_plan(&mut self, owner: u64) {
        let gone: Vec<PlanPress> =
            self.plan.presses.iter().filter(|p| p.owner == owner).copied().collect();
        if gone.is_empty() {
            return;
        }
        self.plan.presses.retain(|p| p.owner != owner);
        for p in gone {
            self.recompose_region(p.min, p.max);
        }
    }

    /// Drop the whole plan layer (a solve starts from nothing and re-registers
    /// every press it still wants).
    pub fn clear_plan(&mut self) {
        let all = std::mem::take(&mut self.plan.presses);
        for p in all {
            self.recompose_region(p.min, p.max);
        }
    }

    /// The history bytes of a chunk — what persistence and the wire carry.
    pub fn chunk_history(&self, key: ChunkKey) -> Option<&[i8]> {
        self.chunks.get(&key).map(|c| c.history.as_slice())
    }

    /// `history ⊕ plan` for one chunk's worth of sites.
    fn compose_history(&self, key: ChunkKey, history: &[i8]) -> Vec<i8> {
        if self.plan.is_empty() {
            return history.to_vec();
        }
        let b = key.base();
        let cell = self.cell;
        let mut out = history.to_vec();
        for lz in 0..CHUNK {
            for lx in 0..CHUNK {
                for ly in 0..CHUNK {
                    let w = vec3f(
                        (b[0] + lx) as f32 * cell,
                        (b[1] + ly) as f32 * cell,
                        (b[2] + lz) as f32 * cell,
                    );
                    let at = site_index(lx, ly, lz);
                    out[at] = PlanTerrainPatch::compose(history[at], self.plan.clip(w, cell));
                }
            }
        }
        out
    }

    /// Recompose every materialized chunk under the x/z box after the plan
    /// changed there; a chunk whose composed bytes moved is dirtied with its
    /// neighbours (their meshes sampled its apron).
    fn recompose_region(&mut self, min: Vec3f, max: Vec3f) {
        let cell = self.cell;
        let span = CHUNK as f32 * cell;
        let keys: Vec<ChunkKey> = self
            .chunks
            .keys()
            .filter(|k| {
                let x0 = k.x as f32 * span;
                let z0 = k.z as f32 * span;
                x0 <= max.x && x0 + span >= min.x && z0 <= max.z && z0 + span >= min.z
            })
            .copied()
            .collect();
        let plan = self.plan.clone();
        let mut changed_keys = Vec::new();
        for key in keys {
            let b = key.base();
            let Some(chunk) = self.chunks.get_mut(&key) else {
                continue;
            };
            let mut changed = false;
            for lz in 0..CHUNK {
                for lx in 0..CHUNK {
                    let x = (b[0] + lx) as f32 * cell;
                    let z = (b[2] + lz) as f32 * cell;
                    for ly in 0..CHUNK {
                        let y = (b[1] + ly) as f32 * cell;
                        let at = site_index(lx, ly, lz);
                        let d = PlanTerrainPatch::compose(
                            chunk.history[at],
                            plan.clip(vec3f(x, y, z), cell),
                        );
                        if d != chunk.density[at] {
                            chunk.density[at] = d;
                            changed = true;
                        }
                    }
                }
            }
            if changed {
                chunk.rev += 1;
                changed_keys.push(key);
            }
        }
        for key in changed_keys {
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let k = ChunkKey { x: key.x + dx, y: key.y + dy, z: key.z + dz };
                        if self.chunks.contains_key(&k) {
                            self.mark_dirty(k);
                        }
                    }
                }
            }
        }
    }

    /// Landform composition over the voxel layer ([`crate::landform`]):
    /// apply a per-column surface-target shape to every materialized chunk
    /// inside the x/z region, first materializing — in columns that ALREADY
    /// hold chunks, never spreading to untouched ones — the chunks covering
    /// the `y_lo..y_hi` target band, so a mountain raised over a dug-open
    /// pit owns its new surface. `target(x, z)` returns `(raise_to,
    /// lower_to)`: raise unions solid below its height (min), lower carves
    /// air above its height (max) — both idempotent; both `Some` assigns.
    pub fn compose_surface_targets(
        &mut self,
        min_x: f32,
        max_x: f32,
        min_z: f32,
        max_z: f32,
        y_lo: f32,
        y_hi: f32,
        base: Option<&Terrain>,
        log: &mut Vec<String>,
        target: &mut dyn FnMut(f32, f32) -> (Option<f32>, Option<f32>),
    ) {
        if self.chunks.is_empty() {
            return;
        }
        let cell = self.cell;
        let plan = self.plan.clone();
        let span = CHUNK as f32 * cell;
        let cols: std::collections::BTreeSet<(i32, i32)> = self
            .chunks
            .keys()
            .filter(|k| {
                let x0 = k.x as f32 * span;
                let z0 = k.z as f32 * span;
                x0 <= max_x && x0 + span >= min_x && z0 <= max_z && z0 + span >= min_z
            })
            .map(|k| (k.x, k.z))
            .collect();
        if cols.is_empty() {
            return;
        }
        let ky0 = (y_lo / span).floor() as i32;
        let ky1 = (y_hi / span).floor() as i32;
        for &(kx, kz) in &cols {
            for ky in ky0..=ky1 {
                self.materialize(ChunkKey { x: kx, y: ky, z: kz }, base, log);
            }
        }
        let keys: Vec<ChunkKey> = self
            .chunks
            .keys()
            .filter(|k| cols.contains(&(k.x, k.z)))
            .copied()
            .collect();
        let mut changed_keys: Vec<ChunkKey> = Vec::new();
        for key in keys {
            let b = key.base();
            let Some(chunk) = self.chunks.get_mut(&key) else {
                continue;
            };
            let mut changed = false;
            for lz in 0..CHUNK {
                for lx in 0..CHUNK {
                    let x = (b[0] + lx) as f32 * cell;
                    let z = (b[2] + lz) as f32 * cell;
                    if x < min_x || x > max_x || z < min_z || z > max_z {
                        continue;
                    }
                    let (raise, lower) = target(x, z);
                    if raise.is_none() && lower.is_none() {
                        continue;
                    }
                    for ly in 0..CHUNK {
                        let y = (b[1] + ly) as f32 * cell;
                        let at = site_index(lx, ly, lz);
                        let old_d = chunk.history[at];
                        let old_m = chunk.material[at];
                        let mut nd = old_d;
                        let mut nm = old_m;
                        if let Some(t) = raise {
                            let q = Self::quantize((y - t) / cell);
                            if q < nd {
                                nd = q;
                            }
                            if nd < 0 && old_d >= 0 {
                                nm = BASE_MATERIAL;
                            }
                        }
                        if let Some(t) = lower {
                            let q = Self::quantize((y - t) / cell);
                            if q > nd {
                                nd = q;
                            }
                            if nd >= 0 {
                                nm = 0;
                            }
                        }
                        if nd != old_d || nm != old_m {
                            chunk.history[at] = nd;
                            chunk.density[at] =
                                PlanTerrainPatch::compose(nd, plan.clip(vec3f(x, y, z), cell));
                            chunk.material[at] = nm;
                            changed = true;
                        }
                    }
                }
            }
            if changed {
                chunk.rev += 1;
                changed_keys.push(key);
            }
        }
        for key in changed_keys {
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let k = ChunkKey { x: key.x + dx, y: key.y + dy, z: key.z + dz };
                        if self.chunks.contains_key(&k) {
                            self.mark_dirty(k);
                        }
                    }
                }
            }
            self.punch_rev = None;
        }
    }

    // ── hashing (T1 gate) ───────────────────────────────────────────────

    /// FNV-1a over cell size + every materialized chunk's key and bytes, in
    /// sorted key order. Pure integer math over quantized data — identical
    /// across runs, machines and archs for the same op stream.
    pub fn field_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let eat = |h: &mut u64, byte: u8| {
            *h ^= byte as u64;
            *h = h.wrapping_mul(0x0000_0100_0000_01b3);
        };
        for b in self.cell.to_le_bytes() {
            eat(&mut h, b);
        }
        for (key, chunk) in &self.chunks {
            for v in [key.x, key.y, key.z] {
                for b in v.to_le_bytes() {
                    eat(&mut h, b);
                }
            }
            for d in &chunk.density {
                eat(&mut h, *d as u8);
            }
            for m in &chunk.material {
                eat(&mut h, *m);
            }
        }
        h
    }

    // ── wire access (session layer) ─────────────────────────────────────

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn chunk_keys(&self) -> impl Iterator<Item = ChunkKey> + '_ {
        self.chunks.keys().copied()
    }

    pub fn chunk(&self, key: ChunkKey) -> Option<&VoxelChunk> {
        self.chunks.get(&key)
    }

    /// Install replicated chunk data verbatim (client side). Marks the chunk
    /// and its materialized neighbours dirty so meshes follow.
    pub fn install_chunk(&mut self, key: ChunkKey, density: Vec<i8>, material: Vec<u8>) {
        if density.len() != CHUNK_SITES || material.len() != CHUNK_SITES {
            return;
        }
        let rev = self.chunks.get(&key).map_or(1, |c| c.rev + 1);
        // A blob carries HISTORY bytes; the plan composes over them here.
        let history = density;
        let density = self.compose_history(key, &history);
        self.chunks.insert(key, VoxelChunk { density, history, material, rev });
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let k = ChunkKey { x: key.x + dx, y: key.y + dy, z: key.z + dz };
                    if self.chunks.contains_key(&k) {
                        self.mark_dirty(k);
                    }
                }
            }
        }
    }

    /// Adopt a replicated structure wholesale (replica side): palette,
    /// volumes and the authoritative chunk-key set — chunks the host does
    /// not list are dropped. `cell` only lands while no chunks exist (the
    /// lattice is fixed once data lives on it).
    pub fn replace_structure(
        &mut self,
        cell: f32,
        palette: Vec<Vec4f>,
        volumes: Vec<(Vec3f, Vec3f, VoxelMode)>,
        keys: &[ChunkKey],
    ) {
        if self.chunks.is_empty() && cell.is_finite() {
            self.cell = cell.clamp(0.1, 4.0);
        }
        if palette.len() >= 2 {
            self.palette = palette;
        }
        self.volumes = volumes
            .into_iter()
            .map(|(min, max, mode)| VoxelVolume { min, max, mode })
            .collect();
        self.structure_rev += 1;
        self.retain_chunks(keys);
        // Modes/palette may have moved: remesh everything, re-punch.
        let keys: Vec<ChunkKey> = self.chunks.keys().copied().collect();
        for key in keys {
            self.mark_dirty(key);
        }
        self.punch_rev = None;
    }

    /// Drop every chunk not in `keep` (full-snapshot apply).
    pub fn retain_chunks(&mut self, keep: &[ChunkKey]) {
        let gone: Vec<ChunkKey> = self
            .chunks
            .keys()
            .filter(|k| keep.binary_search(k).is_err())
            .copied()
            .collect();
        for key in gone {
            self.chunks.remove(&key);
            self.meshes.remove(&key);
            if let Ok(at) = self.dirty.binary_search(&key) {
                self.dirty.remove(at);
            }
        }
    }

    /// A script hot-reload clears script content (volumes/palette) but keeps
    /// the edits — they are player state (mix.md D5). Meshes stay valid: the
    /// data they were built from is still here, and a re-declared volume
    /// re-marks what its mode change affects.
    pub fn on_reset_content(&mut self) {
        // The plan layer is script content: the eval re-registers every
        // press it still wants, and a feature that vanished leaves nothing.
        self.clear_plan();
        self.volumes.clear();
        self.structure_rev += 1;
        self.punch_rev = None;
        self.pending_ops.clear();
        self.fresh_chunks.clear();
        // Landforms are player state like the chunks: the re-eval rebuilds
        // the authored heightfield, then the list replays onto it.
        self.land_replay_pending = !self.land_ops.is_empty();
    }

    // ── meshing (T2/T3/T7) ──────────────────────────────────────────────

    pub fn dirty_len(&self) -> usize {
        self.dirty.len()
    }

    /// Remesh up to `budget` dirty chunks, lowest key first. Returns how many
    /// were processed.
    pub fn update_meshes(&mut self, base: BaseSample, budget: usize) -> usize {
        let take = self.dirty.len().min(budget);
        if take == 0 {
            return 0;
        }
        let keys: Vec<ChunkKey> = self.dirty.drain(..take).collect();
        for key in keys {
            if !self.chunks.contains_key(&key) {
                self.meshes.remove(&key);
                continue;
            }
            self.mesh_rev += 1;
            let rev = self.mesh_rev;
            let mesh = self.mesh_chunk(key, base, rev);
            if mesh.indices.is_empty() {
                self.meshes.remove(&key);
            } else {
                self.meshes.insert(key, mesh);
            }
        }
        take
    }

    /// Mesher mode for a chunk: the first volume containing its centre, else
    /// the first volume it intersects, else smooth.
    fn chunk_mode(&self, key: ChunkKey) -> VoxelMode {
        let b = key.base();
        let center = self.site_world([b[0] + CHUNK / 2, b[1] + CHUNK / 2, b[2] + CHUNK / 2]);
        for v in &self.volumes {
            if v.contains(center) {
                return v.mode;
            }
        }
        let cmin = self.site_world(b);
        let cmax = self.site_world([b[0] + CHUNK, b[1] + CHUNK, b[2] + CHUNK]);
        for v in &self.volumes {
            if cmin.x <= v.max.x
                && cmax.x >= v.min.x
                && cmin.y <= v.max.y
                && cmax.y >= v.min.y
                && cmin.z <= v.max.z
                && cmax.z >= v.min.z
            {
                return v.mode;
            }
        }
        VoxelMode::Smooth
    }

    /// Fill the local sample buffer for one chunk: sites `base-1 ..= base+32`
    /// per axis (34³), from chunk data, neighbours, or the implicit base.
    fn fill_samples(&self, key: ChunkKey, base: BaseSample, out: &mut SampleGrid) {
        let b = key.base();
        for gz in -1..=CHUNK {
            for gy in -1..=CHUNK {
                for gx in -1..=CHUNK {
                    let s = [b[0] + gx, b[1] + gy, b[2] + gz];
                    let sk = ChunkKey::of_site(s);
                    let (d, m) = match self.chunks.get(&sk) {
                        Some(c) => {
                            let cb = sk.base();
                            let at = site_index(s[0] - cb[0], s[1] - cb[1], s[2] - cb[2]);
                            (c.density[at], c.material[at])
                        }
                        None => match base {
                            BaseSample::World(terrain) => {
                                let w = self.site_world(s);
                                let h = terrain
                                    .and_then(|t| t.height_at(w.x, w.z))
                                    .unwrap_or(0.0);
                                let d = Self::quantize((w.y - h) / self.cell);
                                let m = if d < 0 {
                                    // Same topsoil rule as materialize().
                                    if h - w.y < self.cell * 2.5 {
                                        self.surface_material
                                    } else {
                                        BASE_MATERIAL
                                    }
                                } else {
                                    0
                                };
                                (d, m)
                            }
                            BaseSample::Clamp => {
                                // Clamp into this chunk's own site range.
                                let cs = [
                                    s[0].clamp(b[0], b[0] + CHUNK - 1),
                                    s[1].clamp(b[1], b[1] + CHUNK - 1),
                                    s[2].clamp(b[2], b[2] + CHUNK - 1),
                                ];
                                match self.chunks.get(&key) {
                                    Some(c) => {
                                        let at = site_index(
                                            cs[0] - b[0],
                                            cs[1] - b[1],
                                            cs[2] - b[2],
                                        );
                                        (c.density[at], c.material[at])
                                    }
                                    None => (AIR, 0),
                                }
                            }
                        },
                    };
                    out.set(gx, gy, gz, d, m);
                }
            }
        }
    }

    fn mesh_chunk(&self, key: ChunkKey, base: BaseSample, rev: u64) -> ChunkMesh {
        let mut grid = SampleGrid::new();
        self.fill_samples(key, base, &mut grid);
        let mut mesh = match self.chunk_mode(key) {
            VoxelMode::Smooth => mesh_smooth(self, key, &grid),
            VoxelMode::Blocky => mesh_blocky(self, key, &grid),
        };
        mesh.rev = rev;
        mesh
    }
}

impl std::fmt::Debug for VoxelField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoxelField")
            .field("cell", &self.cell)
            .field("volumes", &self.volumes.len())
            .field("chunks", &self.chunks.len())
            .field("meshes", &self.meshes.len())
            .field("dirty", &self.dirty.len())
            .finish()
    }
}

// ── sample grid ─────────────────────────────────────────────────────────

/// Local 34³ sample window for one chunk: sites -1..=32 per axis.
struct SampleGrid {
    density: Vec<i8>,
    material: Vec<u8>,
}

const GRID_N: usize = (CHUNK as usize) + 2; // 34

impl SampleGrid {
    fn new() -> Self {
        Self {
            density: vec![0; GRID_N * GRID_N * GRID_N],
            material: vec![0; GRID_N * GRID_N * GRID_N],
        }
    }
    #[inline]
    fn idx(gx: i32, gy: i32, gz: i32) -> usize {
        debug_assert!((-1..=CHUNK).contains(&gx));
        debug_assert!((-1..=CHUNK).contains(&gy));
        debug_assert!((-1..=CHUNK).contains(&gz));
        (((gz + 1) as usize * GRID_N) + (gy + 1) as usize) * GRID_N + (gx + 1) as usize
    }
    #[inline]
    fn set(&mut self, gx: i32, gy: i32, gz: i32, d: i8, m: u8) {
        let at = Self::idx(gx, gy, gz);
        self.density[at] = d;
        self.material[at] = m;
    }
    #[inline]
    fn d(&self, gx: i32, gy: i32, gz: i32) -> i8 {
        self.density[Self::idx(gx, gy, gz)]
    }
    #[inline]
    fn m(&self, gx: i32, gy: i32, gz: i32) -> u8 {
        self.material[Self::idx(gx, gy, gz)]
    }
}

// ── smooth mesher: chunked surface nets (T2) ────────────────────────────
//
// Same algorithm as gen/src/implicit.rs's surface_net(), restructured for
// chunked incremental use:
//
// - one vertex per straddling CELL (cell = dual cube between 8 sites), at
//   the mean of its edge crossings;
// - one quad per sign-changing lattice EDGE, joining the 4 cells around it;
// - a chunk owns the edges whose base lattice point lies in its own site
//   range; cells outside the chunk (the apron) get vertices too, computed
//   from the same global samples with the same expressions — so the copy a
//   neighbour computes is bit-identical and the seam is watertight.

/// Cell corner offsets, in implicit.rs's OFFS order.
const OFFS: [(i32, i32, i32); 8] = [
    (0, 0, 0),
    (1, 0, 0),
    (0, 1, 0),
    (1, 1, 0),
    (0, 0, 1),
    (1, 0, 1),
    (0, 1, 1),
    (1, 1, 1),
];
/// The 12 cell edges as corner index pairs (implicit.rs's EDGES).
const EDGES: [(usize, usize); 12] = [
    (0, 1),
    (2, 3),
    (4, 5),
    (6, 7),
    (0, 2),
    (1, 3),
    (4, 6),
    (5, 7),
    (0, 4),
    (1, 5),
    (2, 6),
    (3, 7),
];

fn mesh_smooth(field: &VoxelField, key: ChunkKey, grid: &SampleGrid) -> ChunkMesh {
    let b = key.base();
    let cell = field.cell;
    // Vertex per straddling cell, indexed by local cell coord (-1..CHUNK).
    let side = (CHUNK + 1) as usize; // cells -1..=CHUNK-1 → 33 per axis
    let cidx = |cx: i32, cy: i32, cz: i32| -> usize {
        (((cz + 1) as usize * side) + (cy + 1) as usize) * side + (cx + 1) as usize
    };
    let mut cell_vertex = vec![u32::MAX; side * side * side];
    let mut verts: Vec<f32> = Vec::new();
    let mut min = vec3f(f32::MAX, f32::MAX, f32::MAX);
    let mut max = vec3f(f32::MIN, f32::MIN, f32::MIN);

    let make_vertex = |cx: i32, cy: i32, cz: i32,
                           cell_vertex: &mut Vec<u32>,
                           verts: &mut Vec<f32>,
                           min: &mut Vec3f,
                           max: &mut Vec3f|
     -> u32 {
        let at = cidx(cx, cy, cz);
        if cell_vertex[at] != u32::MAX {
            return cell_vertex[at];
        }
        let mut d = [0i8; 8];
        let mut neg = 0;
        for (i, (ox, oy, oz)) in OFFS.iter().enumerate() {
            d[i] = grid.d(cx + ox, cy + oy, cz + oz);
            if d[i] < 0 {
                neg += 1;
            }
        }
        if neg == 0 || neg == 8 {
            return u32::MAX;
        }
        // Mean of the edge crossings, in GLOBAL lattice coordinates so a
        // neighbour chunk lands on the identical floats.
        let mut acc = Vec3f::default();
        let mut count = 0.0f32;
        for (a, c) in EDGES {
            let (da, dc) = (d[a] as f32, d[c] as f32);
            if (da < 0.0) == (dc < 0.0) {
                continue;
            }
            let t = if (dc - da).abs() > 1.0e-8 { da / (da - dc) } else { 0.5 };
            let (ax, ay, az) = OFFS[a];
            let (cx2, cy2, cz2) = OFFS[c];
            let pa = vec3f(
                (b[0] + cx + ax) as f32,
                (b[1] + cy + ay) as f32,
                (b[2] + cz + az) as f32,
            );
            let pc = vec3f(
                (b[0] + cx + cx2) as f32,
                (b[1] + cy + cy2) as f32,
                (b[2] + cz + cz2) as f32,
            );
            acc.x += pa.x + (pc.x - pa.x) * t;
            acc.y += pa.y + (pc.y - pa.y) * t;
            acc.z += pa.z + (pc.z - pa.z) * t;
            count += 1.0;
        }
        if count == 0.0 {
            return u32::MAX;
        }
        let pos = vec3f(
            acc.x / count * cell,
            acc.y / count * cell,
            acc.z / count * cell,
        );
        // Normal: density gradient across the cell (central-ish difference
        // over the 8 corners) — local to the same samples, so seam vertices
        // get seam-identical normals.
        // Promote each signed density before summing. Four perfectly solid
        // i8 corners legitimately exceed i8::MAX; debug builds must produce
        // the same gradient as release rather than overflow here.
        let gx = (d[1] as i32 + d[3] as i32 + d[5] as i32 + d[7] as i32) as f32
            - (d[0] as i32 + d[2] as i32 + d[4] as i32 + d[6] as i32) as f32;
        let gy = (d[2] as i32 + d[3] as i32 + d[6] as i32 + d[7] as i32) as f32
            - (d[0] as i32 + d[1] as i32 + d[4] as i32 + d[5] as i32) as f32;
        let gz = (d[4] as i32 + d[5] as i32 + d[6] as i32 + d[7] as i32) as f32
            - (d[0] as i32 + d[1] as i32 + d[2] as i32 + d[3] as i32) as f32;
        let glen = (gx * gx + gy * gy + gz * gz).sqrt();
        let n = if glen > 1.0e-6 {
            vec3f(gx / glen, gy / glen, gz / glen)
        } else {
            vec3f(0.0, 1.0, 0.0)
        };
        // Material: the most-solid corner's material; ties break on corner
        // order. Deterministic, and dug tunnels show the material they cut.
        let mut best_d = i8::MAX;
        let mut mat = BASE_MATERIAL;
        for (i, (ox, oy, oz)) in OFFS.iter().enumerate() {
            if d[i] < best_d {
                best_d = d[i];
                mat = grid.m(cx + ox, cy + oy, cz + oz);
            }
        }
        let color = field
            .palette
            .get(mat as usize)
            .copied()
            .unwrap_or(vec4f(0.5, 0.5, 0.5, 1.0));
        let index = (verts.len() / MESH_VERTEX_FLOATS) as u32;
        verts.extend_from_slice(&[
            pos.x, pos.y, pos.z, n.x, n.y, n.z, 0.0, 0.0, color.x, color.y, color.z, color.w,
            1.0, 0.0, 0.0, 1.0,
        ]);
        min.x = min.x.min(pos.x);
        min.y = min.y.min(pos.y);
        min.z = min.z.min(pos.z);
        max.x = max.x.max(pos.x);
        max.y = max.y.max(pos.y);
        max.z = max.z.max(pos.z);
        cell_vertex[at] = index;
        index
    };

    let mut indices: Vec<u32> = Vec::new();
    let quad = |cells: [(i32, i32, i32); 4],
                    flip: bool,
                    cell_vertex: &mut Vec<u32>,
                    verts: &mut Vec<f32>,
                    indices: &mut Vec<u32>,
                    min: &mut Vec3f,
                    max: &mut Vec3f| {
        let mut v = [0u32; 4];
        for (i, (cx, cy, cz)) in cells.iter().enumerate() {
            v[i] = make_vertex(*cx, *cy, *cz, cell_vertex, verts, min, max);
            if v[i] == u32::MAX {
                return;
            }
        }
        // Same emission as implicit.rs: quad(a,b,c,d) → (a,b,c) + (a,c,d),
        // reversed when the sign says the face points the other way.
        let (a, bb, c, dd) = (v[0], v[1], v[2], v[3]);
        if flip {
            indices.extend_from_slice(&[a, bb, c, a, c, dd]);
        } else {
            indices.extend_from_slice(&[dd, c, bb, dd, bb, a]);
        }
    };

    // Edges owned by this chunk: base lattice point in [0, CHUNK) per axis.
    for z in 0..CHUNK {
        for y in 0..CHUNK {
            for x in 0..CHUNK {
                let d0 = grid.d(x, y, z);
                // +X edge
                let d1 = grid.d(x + 1, y, z);
                if (d0 < 0) != (d1 < 0) {
                    quad(
                        [(x, y - 1, z - 1), (x, y, z - 1), (x, y, z), (x, y - 1, z)],
                        d0 < 0,
                        &mut cell_vertex,
                        &mut verts,
                        &mut indices,
                        &mut min,
                        &mut max,
                    );
                }
                // +Y edge
                let d1 = grid.d(x, y + 1, z);
                if (d0 < 0) != (d1 < 0) {
                    quad(
                        [(x - 1, y, z - 1), (x - 1, y, z), (x, y, z), (x, y, z - 1)],
                        d0 < 0,
                        &mut cell_vertex,
                        &mut verts,
                        &mut indices,
                        &mut min,
                        &mut max,
                    );
                }
                // +Z edge
                let d1 = grid.d(x, y, z + 1);
                if (d0 < 0) != (d1 < 0) {
                    quad(
                        [(x - 1, y - 1, z), (x, y - 1, z), (x, y, z), (x - 1, y, z)],
                        d0 < 0,
                        &mut cell_vertex,
                        &mut verts,
                        &mut indices,
                        &mut min,
                        &mut max,
                    );
                }
            }
        }
    }

    if indices.is_empty() {
        return ChunkMesh::default();
    }
    ChunkMesh {
        rev: 0,
        verts,
        indices,
        min,
        max,
    }
}

// ── blocky mesher: greedy cubes on the same field (T3) ──────────────────
//
// A SITE is a block: solid iff its density is negative. Block s occupies the
// world cube [s*cell, (s+1)*cell). A face is emitted where a solid block
// meets air, owned by the SOLID block's chunk (no duplicates, no gaps —
// apron samples answer for the neighbour chunk). Faces merge greedily per
// slice while material matches.

fn mesh_blocky(field: &VoxelField, key: ChunkKey, grid: &SampleGrid) -> ChunkMesh {
    let b = key.base();
    let cell = field.cell;
    let mut verts: Vec<f32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut min = vec3f(f32::MAX, f32::MAX, f32::MAX);
    let mut max = vec3f(f32::MIN, f32::MIN, f32::MIN);

    // (axis, sign): face normal direction. u/v are the in-plane axes.
    const DIRS: [([i32; 3], usize, usize, usize); 6] = [
        ([1, 0, 0], 0, 1, 2),
        ([-1, 0, 0], 0, 1, 2),
        ([0, 1, 0], 1, 2, 0),
        ([0, -1, 0], 1, 2, 0),
        ([0, 0, 1], 2, 0, 1),
        ([0, 0, -1], 2, 0, 1),
    ];

    let solid = |s: [i32; 3]| -> Option<u8> {
        let g = [s[0] - b[0], s[1] - b[1], s[2] - b[2]];
        if grid.d(g[0], g[1], g[2]) < 0 {
            Some(grid.m(g[0], g[1], g[2]))
        } else {
            None
        }
    };

    for (normal, axis, ua, va) in DIRS {
        for w in 0..CHUNK {
            // Face mask for this slice: material where the face is exposed.
            let mut mask = [[0u8; CHUNK_U]; CHUNK_U];
            let mut any = false;
            for v in 0..CHUNK {
                for u in 0..CHUNK {
                    let mut s = b;
                    s[axis] += w;
                    s[ua] += u;
                    s[va] += v;
                    if let Some(m) = solid(s) {
                        let n = [s[0] + normal[0], s[1] + normal[1], s[2] + normal[2]];
                        let ng = [n[0] - b[0], n[1] - b[1], n[2] - b[2]];
                        if grid.d(ng[0], ng[1], ng[2]) >= 0 {
                            mask[v as usize][u as usize] = m.max(1);
                            any = true;
                        }
                    }
                }
            }
            if !any {
                continue;
            }
            // Greedy rectangles over the mask.
            for v0 in 0..CHUNK_U {
                let mut u0 = 0;
                while u0 < CHUNK_U {
                    let m = mask[v0][u0];
                    if m == 0 {
                        u0 += 1;
                        continue;
                    }
                    let mut u1 = u0 + 1;
                    while u1 < CHUNK_U && mask[v0][u1] == m {
                        u1 += 1;
                    }
                    let mut v1 = v0 + 1;
                    'grow: while v1 < CHUNK_U {
                        for u in u0..u1 {
                            if mask[v1][u] != m {
                                break 'grow;
                            }
                        }
                        v1 += 1;
                    }
                    for row in mask.iter_mut().take(v1).skip(v0) {
                        for x in row.iter_mut().take(u1).skip(u0) {
                            *x = 0;
                        }
                    }
                    // Emit the rectangle as one quad. Face plane sits at the
                    // solid block's boundary toward `normal`.
                    let mut base_site = b;
                    base_site[axis] += w;
                    let positive = normal[axis] > 0;
                    let plane = if positive {
                        (base_site[axis] + 1) as f32 * cell
                    } else {
                        base_site[axis] as f32 * cell
                    };
                    let color = field
                        .palette
                        .get(m as usize)
                        .copied()
                        .unwrap_or(vec4f(0.5, 0.5, 0.5, 1.0));
                    let u_lo = (b[ua] + u0 as i32) as f32 * cell;
                    let u_hi = (b[ua] + u1 as i32) as f32 * cell;
                    let v_lo = (b[va] + v0 as i32) as f32 * cell;
                    let v_hi = (b[va] + v1 as i32) as f32 * cell;
                    let corner = |uu: f32, vv: f32| -> Vec3f {
                        let mut p = [0.0f32; 3];
                        p[axis] = plane;
                        p[ua] = uu;
                        p[va] = vv;
                        vec3f(p[0], p[1], p[2])
                    };
                    let n = vec3f(normal[0] as f32, normal[1] as f32, normal[2] as f32);
                    let quad = [
                        corner(u_lo, v_lo),
                        corner(u_hi, v_lo),
                        corner(u_hi, v_hi),
                        corner(u_lo, v_hi),
                    ];
                    let base_index = (verts.len() / MESH_VERTEX_FLOATS) as u32;
                    for p in quad {
                        verts.extend_from_slice(&[
                            p.x, p.y, p.z, n.x, n.y, n.z, 0.0, 0.0, color.x, color.y, color.z,
                            color.w, 1.0, 0.0, 0.0, 1.0,
                        ]);
                        min.x = min.x.min(p.x);
                        min.y = min.y.min(p.y);
                        min.z = min.z.min(p.z);
                        max.x = max.x.max(p.x);
                        max.y = max.y.max(p.y);
                        max.z = max.z.max(p.z);
                    }
                    // Winding: CCW seen from the normal side. The u/v axes
                    // form a right-handed frame with the +axis normal, so the
                    // negative direction flips.
                    if positive {
                        indices.extend_from_slice(&[
                            base_index,
                            base_index + 1,
                            base_index + 2,
                            base_index,
                            base_index + 2,
                            base_index + 3,
                        ]);
                    } else {
                        indices.extend_from_slice(&[
                            base_index,
                            base_index + 2,
                            base_index + 1,
                            base_index,
                            base_index + 3,
                            base_index + 2,
                        ]);
                    }
                    u0 = u1;
                }
            }
        }
    }

    if indices.is_empty() {
        return ChunkMesh::default();
    }
    ChunkMesh {
        rev: 0,
        verts,
        indices,
        min,
        max,
    }
}

// ── world integration ───────────────────────────────────────────────────

/// Punch heightfield holes around one materialized chunk: a terrain cell
/// stops existing as heightfield (0xFF, box3d's hole value; the renderer
/// skips the same cells) when the voxel field has taken over its surface —
/// every corner of the cell, AT its surface height, lies inside some
/// materialized chunk. Cells straddling chunk borders punch once all their
/// covering chunks exist. Deep chunks (a tunnel under a ridge) cover no
/// surface corner and punch nothing — the ridge above stays authored
/// heightfield.
fn punch_chunk(
    field: &VoxelField,
    key: ChunkKey,
    terrain: &Terrain,
    materials: &mut TerrainMaterials,
) -> bool {
    let b = key.base();
    let cmin = field.site_world(b);
    let cmax = field.site_world([b[0] + CHUNK, b[1] + CHUNK, b[2] + CHUNK]);
    let cells = terrain.cells;
    if cells < 2 {
        return false;
    }
    let span = cells - 1;
    // Terrain cells INTERSECTING the chunk footprint, as an index range —
    // never a scan of the whole (cells-1)² field per chunk.
    let cs = terrain.cell_size.max(1.0e-6);
    let cx0 = (((cmin.x - terrain.origin) / cs).floor().max(0.0)) as usize;
    let cz0 = (((cmin.z - terrain.origin) / cs).floor().max(0.0)) as usize;
    let cx1 = ((((cmax.x - terrain.origin) / cs).ceil()).max(0.0) as usize).min(span);
    let cz1 = ((((cmax.z - terrain.origin) / cs).ceil()).max(0.0) as usize).min(span);
    let mut changed = false;
    for cz in cz0..cz1 {
        for cx in cx0..cx1 {
            let at = cz * span + cx;
            if at >= materials.indices.len() || materials.indices[at] == 0xFF {
                continue;
            }
            let h = |gx: usize, gz: usize| terrain.heights[gz * cells + gx];
            let covered = [(0usize, 0usize), (1, 0), (0, 1), (1, 1)].iter().all(|(ox, oz)| {
                let wx = terrain.origin + (cx + ox) as f32 * cs;
                let wz = terrain.origin + (cz + oz) as f32 * cs;
                let wh = h(cx + ox, cz + oz);
                // BOTH sides of the surface crossing must be materialized:
                // the air site at the surface and the solid site below it.
                // A surface lying exactly on a chunk's bottom boundary has
                // its solid side in the chunk below — if that one was never
                // touched, the crossing is not meshable and punching the
                // heightfield here would open a hole with nothing behind it.
                let air = field.world_site(vec3f(wx, wh, wz));
                let solid = [air[0], air[1] - 1, air[2]];
                field.chunks.contains_key(&ChunkKey::of_site(air))
                    && field.chunks.contains_key(&ChunkKey::of_site(solid))
            });
            if covered {
                materials.indices[at] = 0xFF;
                changed = true;
            }
        }
    }
    changed
}

/// Per-tick voxel maintenance on a full world: (re)apply heightfield hole
/// punches when the terrain revision moved, then remesh under the tick
/// budget. Called by `step_world` on simulating devices (`authority` = true)
/// and by the session's client tick (false: the replica has no base layer,
/// so implicit samples clamp) — both sides derive meshes locally; only field
/// data crosses the wire. A world without a field returns immediately.
pub fn update_world_voxel(world: &mut crate::world::GameWorld, authority: bool) {
    // Landform replay: after a re-eval rebuilt the heightfield (or a
    // structure snapshot delivered the list), re-apply the recorded macro
    // landforms — heightfield only; the chunks already carry their history.
    crate::landform::replay_land_ops(world);
    if world.terrain.is_some() {
        let needs_punch = match (&world.voxel, &world.terrain) {
            (Some(v), Some(t)) => v.punch_rev != Some(t.revision),
            _ => false,
        };
        if needs_punch {
            // Ensure a materials table exists to punch into. Creating the
            // single-default-surface table is byte-identical physics to None.
            if world.terrain_materials.is_none() {
                let cells = world.terrain.as_ref().map_or(0, |t| t.cells);
                let count = cells.saturating_sub(1) * cells.saturating_sub(1);
                world.terrain_materials = Some(TerrainMaterials {
                    indices: vec![0u8; count],
                    surfaces: vec![TerrainSurface {
                        friction: 0.6,
                        restitution: 0.0,
                    }],
                });
            }
            let Some(voxel) = world.voxel.as_mut() else {
                return;
            };
            let terrain = world.terrain.as_mut().unwrap();
            let materials = world.terrain_materials.as_mut().unwrap();
            let mut changed = false;
            let keys: Vec<ChunkKey> = voxel.chunk_keys().collect();
            for key in keys {
                changed |= punch_chunk(voxel, key, terrain, materials);
            }
            if changed {
                terrain.revision += 1;
            }
            voxel.punch_rev = Some(terrain.revision);
        }
    }
    let crate::world::GameWorld { voxel, terrain, .. } = world;
    if let Some(voxel) = voxel.as_mut() {
        let base = if authority {
            BaseSample::World(terrain.as_ref())
        } else {
            BaseSample::Clamp
        };
        voxel.update_meshes(base, REMESH_BUDGET_PER_TICK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field_with_volume(mode: VoxelMode) -> VoxelField {
        let mut f = VoxelField::new(0.5);
        f.declare_volume(vec3f(-40.0, -20.0, -40.0), vec3f(40.0, 40.0, 40.0), mode);
        f
    }

    fn dig(f: &mut VoxelField, pos: Vec3f, r: f32, mode: DigMode) {
        let mut log = Vec::new();
        f.apply_op(
            VoxelOp::Dig {
                pos,
                r,
                mode,
                material: 1,
            },
            None,
            true,
            true,
            &mut log,
        );
        assert!(log.is_empty(), "{log:?}");
    }

    #[test]
    fn unedited_field_stores_nothing() {
        let f = field_with_volume(VoxelMode::Smooth);
        assert_eq!(f.chunk_count(), 0);
        assert_eq!(f.meshes.len(), 0);
    }

    #[test]
    fn field_hash_is_deterministic_for_the_same_op_stream() {
        let build = || {
            let mut f = field_with_volume(VoxelMode::Smooth);
            dig(&mut f, vec3f(1.0, 0.0, 2.0), 3.0, DigMode::Carve);
            dig(&mut f, vec3f(-4.0, 1.0, 2.0), 2.0, DigMode::Fill);
            dig(&mut f, vec3f(0.0, 0.5, -6.0), 2.5, DigMode::Flatten);
            let mut log = Vec::new();
            f.apply_op(
                VoxelOp::SetBlock { x: 3, y: 4, z: 5, material: 3 },
                None,
                true,
                true,
                &mut log,
            );
            f
        };
        let a = build();
        let b = build();
        assert_eq!(a.field_hash(), b.field_hash());
        assert!(a.chunk_count() > 0);
        // The exact value is part of the determinism contract: it may only
        // move with a deliberate format change (re-baseline this constant).
        assert_eq!(a.field_hash(), 6019352774262045796);
        // Order sensitivity: a different stream hashes differently.
        let mut c = field_with_volume(VoxelMode::Smooth);
        dig(&mut c, vec3f(1.0, 0.0, 2.0), 3.0, DigMode::Carve);
        assert_ne!(a.field_hash(), c.field_hash());
    }

    #[test]
    fn ops_are_idempotent_on_reapplication() {
        let mut f = field_with_volume(VoxelMode::Smooth);
        dig(&mut f, vec3f(0.0, 0.0, 0.0), 3.0, DigMode::Carve);
        dig(&mut f, vec3f(4.0, 0.0, 0.0), 2.0, DigMode::Fill);
        let h1 = f.field_hash();
        // Re-deliveries (late-join snapshot + op replay) must not drift.
        dig(&mut f, vec3f(0.0, 0.0, 0.0), 3.0, DigMode::Carve);
        dig(&mut f, vec3f(4.0, 0.0, 0.0), 2.0, DigMode::Fill);
        assert_eq!(f.field_hash(), h1);
    }

    #[test]
    fn the_whole_world_is_implicitly_editable() {
        // No declared volume at all: the main ground is one world-spanning
        // editable volume. An edit far from anywhere materializes exactly
        // the chunks under the brush and nothing else.
        let mut f = VoxelField::new(0.5);
        assert_eq!(f.chunk_count(), 0, "unedited field must store nothing");
        dig(&mut f, vec3f(500.0, 0.0, 0.0), 3.0, DigMode::Carve);
        assert!(f.chunk_count() > 0, "dig without a volume did not materialize");
        assert!(
            f.chunk_count() <= 27,
            "dig materialized far beyond the brush: {}",
            f.chunk_count()
        );
        assert!(f.is_carved_air(vec3f(500.0, -1.0, 0.0)), "carve did not open air");
    }

    #[test]
    fn tunnel_materializes_the_capsule_not_the_aabb() {
        let mut f = VoxelField::new(0.5);
        let mut log = Vec::new();
        // A long diagonal tunnel: the AABB covers ~13×13 chunk columns, the
        // capsule itself passes through only a corridor of them.
        f.apply_op(
            VoxelOp::Tunnel {
                from: vec3f(-90.0, -3.0, -90.0),
                to: vec3f(90.0, -3.0, 90.0),
                r: 2.5,
            },
            None,
            true,
            true,
            &mut log,
        );
        assert!(log.is_empty(), "{log:?}");
        let n = f.chunk_count();
        assert!(n > 0, "tunnel materialized nothing");
        assert!(n < 200, "tunnel swallowed its AABB: {n} chunks");
        // The bore is open along the segment, closed off to the side.
        assert!(f.is_carved_air(vec3f(0.0, -3.0, 0.0)), "bore not open");
        assert!(!f.is_carved_air(vec3f(0.0, -3.0, 40.0)), "carved far off axis");
    }

    #[test]
    fn surface_seam_reports_pits_and_mounds_but_not_deep_tunnels() {
        let mut f = VoxelField::new(0.5);
        // Base = ground plane y 0. A pit at the surface owns its column.
        dig(&mut f, vec3f(0.0, 0.0, 0.0), 3.0, DigMode::Carve);
        let pit = f.surface_at(0.0, 0.0, 0.0).expect("pit does not own surface");
        assert!(pit < -1.0, "pit surface {pit} not below ground");
        // A mound filled on top of the surface answers with its top.
        dig(&mut f, vec3f(20.0, 1.0, 0.0), 3.0, DigMode::Fill);
        let mound = f.surface_at(20.0, 0.0, 0.0).expect("mound does not own surface");
        assert!(mound > 1.0, "mound surface {mound} not above ground");
        // A deep tunnel far below leaves the surface to the heightfield.
        dig(&mut f, vec3f(-40.0, -30.0, 0.0), 3.0, DigMode::Carve);
        assert!(
            f.surface_at(-40.0, 0.0, 0.0).is_none(),
            "deep tunnel stole surface ownership"
        );
    }

    #[test]
    fn carve_opens_air_and_fill_closes_it() {
        let mut f = field_with_volume(VoxelMode::Smooth);
        let p = vec3f(0.0, -1.0, 0.0); // below the y=0 ground plane: solid
        assert!(!f.is_carved_air(p));
        dig(&mut f, p, 2.0, DigMode::Carve);
        assert!(f.is_carved_air(p), "carve did not open air");
        dig(&mut f, p, 2.5, DigMode::Fill);
        assert!(!f.is_carved_air(p), "fill did not re-close");
    }

    #[test]
    fn a_sculpt_spanning_a_seam_is_watertight() {
        // A filled ball floating in air, centred ON a chunk corner so its
        // surface spans up to 8 chunks. The union of all chunk meshes must be
        // a closed 2-manifold: after welding bit-identical positions, every
        // edge is used exactly twice, in opposite directions.
        let mut f = field_with_volume(VoxelMode::Smooth);
        let center = vec3f(16.0, 16.0, 16.0); // site 32 = chunk boundary
        dig(&mut f, center, 3.0, DigMode::Fill);
        while f.dirty_len() > 0 {
            f.update_meshes(BaseSample::World(None), 64);
        }
        assert!(f.meshes.len() >= 4, "ball did not span chunks: {}", f.meshes.len());

        // Weld across chunks by EXACT float bits — seam vertices must agree
        // to the bit or the seam is not watertight.
        let mut weld: std::collections::BTreeMap<[u32; 3], u32> = std::collections::BTreeMap::new();
        let mut tris: Vec<[u32; 3]> = Vec::new();
        for mesh in f.meshes.values() {
            let mut remap = Vec::new();
            for v in mesh.verts.chunks_exact(MESH_VERTEX_FLOATS) {
                let bits = [v[0].to_bits(), v[1].to_bits(), v[2].to_bits()];
                let next = weld.len() as u32;
                let id = *weld.entry(bits).or_insert(next);
                remap.push(id);
            }
            for t in mesh.indices.chunks_exact(3) {
                tris.push([
                    remap[t[0] as usize],
                    remap[t[1] as usize],
                    remap[t[2] as usize],
                ]);
            }
        }
        // Count directed edges; a closed orientable surface uses every
        // undirected edge exactly once in each direction.
        let mut edges: std::collections::BTreeMap<(u32, u32), i32> = std::collections::BTreeMap::new();
        for t in &tris {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                if a == b {
                    continue; // degenerate sliver from a merged crossing
                }
                let key = (a.min(b), a.max(b));
                *edges.entry(key).or_insert(0) += if a < b { 1 } else { -1 };
            }
        }
        let bad: Vec<_> = edges.iter().filter(|(_, n)| **n != 0).collect();
        assert!(
            bad.is_empty(),
            "{} unmatched seam edges (of {}) — mesh is not watertight",
            bad.len(),
            edges.len()
        );
    }

    #[test]
    fn blocky_mesher_emits_merged_cube_faces() {
        let mut f = field_with_volume(VoxelMode::Blocky);
        let mut log = Vec::new();
        // A 4×1×2 slab of blocks in the air.
        for x in 0..4 {
            for z in 0..2 {
                f.apply_op(
                    VoxelOp::SetBlock { x, y: 10, z, material: 3 },
                    None,
                    true,
                    true,
                    &mut log,
                );
            }
        }
        while f.dirty_len() > 0 {
            f.update_meshes(BaseSample::World(None), 64);
        }
        let mesh = f.meshes.values().next().expect("no mesh");
        // Greedy meshing: the slab's top face merges into ONE quad (2 tris),
        // 6 faces × 2 tris = 12 tris total for the whole slab.
        assert_eq!(mesh.indices.len() / 3, 12, "greedy merge failed");
        // All triangles at the slab's top face sit at y = 11 * cell.
        let top = 11.0 * f.cell;
        let has_top = mesh
            .verts
            .chunks_exact(MESH_VERTEX_FLOATS)
            .any(|v| (v[1] - top).abs() < 1.0e-6);
        assert!(has_top, "no top-face vertex at {top}");
    }

    #[test]
    fn set_block_zero_removes() {
        let mut f = field_with_volume(VoxelMode::Blocky);
        let mut log = Vec::new();
        f.apply_op(VoxelOp::SetBlock { x: 1, y: 8, z: 1, material: 2 }, None, true, true, &mut log);
        assert!(!f.is_carved_air(vec3f(0.75, 4.25, 0.75)));
        f.apply_op(VoxelOp::SetBlock { x: 1, y: 8, z: 1, material: 0 }, None, true, true, &mut log);
        assert!(f.is_carved_air(vec3f(0.75, 4.25, 0.75)));
    }

    #[test]
    fn remesh_budget_bounds_work_per_tick() {
        let mut f = field_with_volume(VoxelMode::Smooth);
        // Dirty a wide stripe of chunks.
        for i in 0..6 {
            dig(&mut f, vec3f(i as f32 * 16.0 - 40.0, 0.0, 0.0), 3.0, DigMode::Carve);
        }
        let dirty = f.dirty_len();
        assert!(dirty > REMESH_BUDGET_PER_TICK, "not enough dirty chunks: {dirty}");
        let done = f.update_meshes(BaseSample::World(None), REMESH_BUDGET_PER_TICK);
        assert_eq!(done, REMESH_BUDGET_PER_TICK);
        assert_eq!(f.dirty_len(), dirty - REMESH_BUDGET_PER_TICK);
    }

    #[test]
    fn base_layer_materializes_the_ground_plane() {
        let mut f = field_with_volume(VoxelMode::Smooth);
        dig(&mut f, vec3f(0.0, 0.0, 0.0), 2.0, DigMode::Carve);
        // Deep below the crater the materialized chunk holds base solid.
        assert!(!f.is_carved_air(vec3f(0.0, -6.0, 0.0)));
        assert_eq!(f.material_at([0, -12, 0]), BASE_MATERIAL);
        // High above it, air.
        assert!(f.density_at([0, 20, 0], None) > 0);
    }

    #[test]
    fn floor_probe_finds_carved_tunnel_floor() {
        let mut f = field_with_volume(VoxelMode::Smooth);
        // Tunnel at y = -5, ground plane at 0: carve a bubble underground.
        dig(&mut f, vec3f(0.0, -5.0, 0.0), 3.0, DigMode::Carve);
        let floor = f.floor_probe(0.0, 0.0, -4.0).expect("no voxel floor");
        assert!(
            (-8.5..=-7.0).contains(&floor),
            "tunnel floor at {floor}, expected ≈ -8 (centre -5 minus r 3)"
        );
    }
}

//! Navigation — derived walkability grid, A*, and flow fields (mix.md D6).
//!
//! Pathfinding is not an RTS feature: it is the missing half of every NPC in
//! the engine. The grid here is **derived**, never authored — recomputed from
//! terrain slope, static entities and water, so it can never disagree with the
//! world the way a baked navmesh does. It is chunk-aligned with the D5 voxel
//! plan (32 cells of 0.5 units = the 16-unit chunk footprint), so a terrain
//! edit can dirty exactly the nav chunks it touched ([`GameWorld::nav_mark_dirty`]
//! is the API-level hook; the terrain-edit producer lands in another track).
//!
//! Determinism rules: integer cells, fixed iteration order, `total_cmp` for
//! every float sort, no HashMap anywhere. Two derivations of the same world
//! hash identically ([`GameWorld::nav_grid_hash`] is the gate).
//!
//! Consumers: [`NavAgent`] (single-agent path following — brains and NPCs),
//! [`FlowField`] (group orders — the RTS unit block). Script never sees any of
//! this: it sees goals and events, the engine does the per-tick steering.

use std::sync::Arc;

use makepad_math::*;

use crate::entity::{BodyKind, Entity, Shape};
use crate::queries::{wedge_surface_at, Solid};
use crate::terrain::Terrain;
use crate::world::GameWorld;

/// Cell edge in world units. Matches D5's 0.5 m voxel cell so nav chunks and
/// future terrain-edit chunks address the same 16-unit tiles.
pub const NAV_CELL: f32 = 0.5;
/// Cells per chunk edge (32 × 0.5 = 16 units — the D5 chunk footprint).
pub const NAV_CHUNK: i32 = 32;
/// The mover sweep's step-up contract: anything this much above the floor is
/// a kerb, not a wall.
pub const NAV_STEP: f32 = 0.55;
/// Max floor-height change between adjacent cells that still counts as
/// traversable ground. Terrain has no hard slope limit in the sweep (CLIMB
/// walks anything), so this is a *legibility* limit: past ~61° a slope reads
/// as a cliff and paths should go around it.
pub const NAV_EDGE_RISE: f32 = 0.9;
/// Headroom a walker needs. A solid whose underside is closer to the floor
/// than this is a wall; higher is a bridge overhead.
pub const NAV_CLEARANCE: f32 = 1.8;
/// Grid span cap per axis, in cells (512 units). Worlds larger than this get
/// nav coverage over the min-corner-anchored window; consumers fall back to
/// straight-line steering outside coverage, which is exactly the pre-nav
/// behavior.
pub const NAV_MAX_SPAN: i32 = 1024;

/// A body of this footprint or less can walk anywhere WALKABLE; wider agents
/// (the standard 0.8–1.0-unit walker) plan on CLEAR, which erodes one cell
/// off every wall.
pub const FLAG_WALKABLE: u8 = 1;
/// Walkable AND all 8 neighbours walkable — safe for ~1-unit-wide agents.
pub const FLAG_CLEAR: u8 = 2;

const CHUNK_AREA: usize = (NAV_CHUNK * NAV_CHUNK) as usize;
/// A* / flow-field working-region span cap per axis, in cells (160 units).
const SEARCH_SPAN: i32 = 320;
/// A* expansion budget. On overrun the plan fails and the consumer keeps its
/// straight-line fallback — bounded cost beats a perfect path.
const ASTAR_BUDGET: usize = 20000;
/// How far (in cells, Chebyshev) an endpoint may be snapped onto the grid.
const SNAP_RADIUS: i32 = 6;
/// Minimum time between invalidating already-derived cells. The simulation
/// runs at 60 Hz, so 30 ticks is roughly half a second. Until this expires,
/// queries deliberately use the previous grid: steering self-corrects on the
/// next rebuild, while a moving obstacle can no longer force a derivation on
/// every authored tick.
const REBUILD_COOLDOWN_TICKS: u64 = 30;

/// Fixed neighbour order — load-bearing for determinism (tie-breaks in A*
/// and flow-field descent resolve by first-in-this-order).
const NEIGHBORS: [(i32, i32, u32); 8] = [
    (1, 0, 10),
    (-1, 0, 10),
    (0, 1, 10),
    (0, -1, 10),
    (1, 1, 14),
    (1, -1, 14),
    (-1, 1, 14),
    (-1, -1, 14),
];

/// One 32×32 tile of derived cells. `Arc`'d so world snapshots stay cheap.
pub struct NavChunk {
    /// FLAG_* bits, row-major `z * 32 + x`.
    pub flags: [u8; CHUNK_AREA],
    /// Walk-surface height per cell (undefined where not WALKABLE).
    pub floor: [f32; CHUNK_AREA],
}

/// A STREAMED level's own authored walkability, folded into the derived
/// grid as one static layer.
///
/// The derived grid reads terrain, static solids and water — everything the
/// engine itself built. A streamed level is neither: its ground is one flat
/// textured quad and its walls live in a sidecar the importer wrote. Without
/// this layer such a level has no walkable cells at all (no terrain, no
/// static bodies), so every flow field fails and every mover falls back to
/// straight-line steering through the walls.
///
/// The layer therefore does two things: it declares the level's ground
/// height inside its extent (making the map walkable in the first place),
/// and it marks the cells the level says are impassable.
#[derive(Clone, Debug, PartialEq)]
pub struct StaticBlocked {
    /// World-space (x, z) of the layer's `(0, 0)` cell corner.
    pub origin: [f32; 2],
    /// Metres per authored cell (NOT [`NAV_CELL`] — one authored cell
    /// usually covers many nav cells).
    pub cell: f32,
    pub width: u32,
    pub height: u32,
    /// Row-major `width * height`; `true` = impassable.
    pub blocked: Vec<bool>,
    /// Ground height of the level's flat floor, metres. Also the fallback
    /// height wherever `floors` is empty.
    pub floor: f32,
    /// The most a body may CLIMB and the most it may FALL between adjacent
    /// cells on this ground, in metres — the level's own step height and
    /// fall limit, as its walker declared them.
    ///
    /// The derived grid's own [`NAV_EDGE_RISE`] is a symmetric legibility
    /// limit for TERRAIN: past ~61 degrees a slope reads as a cliff and paths
    /// should go around it. A streamed level's floors are not a slope. Its
    /// rooms sit whole metres apart and its bodies drop into them on purpose,
    /// so one symmetric 0.9 m rule cut the level's own graph at every real
    /// step down — an army ordered across the map stopped at the first ledge
    /// its own walker would have walked off. Zero (the default) keeps
    /// `NAV_EDGE_RISE` for both directions, which is every world that is not
    /// a streamed level.
    pub climb: f32,
    pub fall: f32,
    /// Row-major `width * height` walk-surface height per cell, when the
    /// level publishes one. EMPTY means "one flat floor everywhere" — a
    /// tiled strategy map, whose ground really is a single plane.
    ///
    /// A streamed 3D level is not flat: its rooms sit at a dozen different
    /// heights, and a single `floor` would either bury half the map or
    /// float the other half. The derived grid's own cell-to-cell rise check
    /// ([`NAV_EDGE_RISE`]) then does the rest: a stair is walkable, a cliff
    /// edge is not, and a flow field routes around both without knowing
    /// what a level is.
    pub floors: Vec<f32>,
}

impl StaticBlocked {
    /// May a body step from a cell at `here` to one at `next`? Climbing and
    /// falling are separate limits, because on a real level they are.
    #[inline]
    pub fn step_ok(&self, here: f32, next: f32) -> bool {
        let climb = if self.climb > 0.0 { self.climb } else { NAV_EDGE_RISE };
        let fall = if self.fall > 0.0 { self.fall } else { NAV_EDGE_RISE };
        next - here <= climb && here - next <= fall
    }

    /// Walk-surface height of authored cell `i`.
    #[inline]
    pub fn floor_at(&self, i: usize) -> f32 {
        self.floors.get(i).copied().unwrap_or(self.floor)
    }

    /// Row-major index of the authored cell containing world `(x, z)`.
    #[inline]
    pub fn index_at(&self, x: f32, z: f32) -> Option<usize> {
        if self.cell <= 0.0 || self.width == 0 || self.height == 0 {
            return None;
        }
        let fx = (x - self.origin[0]) / self.cell;
        let fz = (z - self.origin[1]) / self.cell;
        if fx < 0.0 || fz < 0.0 {
            return None;
        }
        let (cx, cz) = (fx as u32, fz as u32);
        if cx >= self.width || cz >= self.height {
            return None;
        }
        Some((cz as usize) * (self.width as usize) + cx as usize)
    }

    #[inline]
    pub fn is_blocked_at(&self, x: f32, z: f32) -> bool {
        self.index_at(x, z)
            .map(|i| self.blocked[i])
            .unwrap_or(false)
    }

    /// Deterministic fingerprint — the invalidation key.
    pub fn fingerprint(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x100_0000_01b3);
        };
        mix(self.origin[0].to_bits() as u64);
        mix(self.origin[1].to_bits() as u64);
        mix(self.cell.to_bits() as u64);
        mix(self.floor.to_bits() as u64);
        mix(self.width as u64);
        mix(self.height as u64);
        mix(self.climb.to_bits() as u64);
        mix(self.fall.to_bits() as u64);
        for (i, b) in self.blocked.iter().enumerate() {
            if *b {
                mix(i as u64);
            }
        }
        for f in &self.floors {
            mix(f.to_bits() as u64);
        }
        h
    }
}

/// The world's walkability grid: chunked, lazily derived, dirty-flagged.
/// Lives on [`GameWorld`] beside `dynamics` — derived state, reconciled
/// against the entities, never replicated (only the simulating host runs
/// brains and units). Cloning shares the built chunks.
#[derive(Clone, Default)]
pub struct NavMap {
    /// Grid origin in CELL coordinates: cell (0,0) covers world
    /// `[min_cx·0.5, min_cx·0.5 + 0.5)`. Chunk-aligned (multiple of 32) so
    /// chunk addressing is world-absolute.
    min_cx: i32,
    min_cz: i32,
    /// Grid size in cells (multiples of 32). 0 = no coverage.
    w: i32,
    h: i32,
    /// Row-major chunk slots; `None` = dirty / not yet derived.
    chunks: Vec<Option<Arc<NavChunk>>>,
    /// Regional edits wait here until the stale-ok cooldown expires. This is
    /// kept parallel to `chunks`; entity changes rebuild the whole layout
    /// because they may also change its coverage bounds.
    pending_chunks: Vec<bool>,
    /// What the current layout was derived against. `render_rev` is only a
    /// cheap hint to recompute the obstacle fingerprint: presentation-only
    /// changes must never invalidate navigation.
    built_entity_hash: u64,
    observed_entity_hash: u64,
    observed_render_rev: u64,
    built_terrain_rev: u64,
    synced_once: bool,
    last_invalidation_tick: u64,
    /// Number of chunk-sized cell derivations performed. Kept as a cheap
    /// runtime diagnostic and as the regression guard for nav storms.
    derive_cells_runs: u64,
    /// Bumped whenever coverage or content is invalidated — path caches and
    /// flow fields key on it to know when to re-plan.
    pub generation: u64,
    /// The streamed level's authored layer, if one was installed. `Arc`'d so
    /// a world snapshot stays cheap.
    static_blocked: Option<Arc<StaticBlocked>>,
}

/// The world geometry a derivation reads. Borrowed out of [`GameWorld`] by
/// the wrapper methods (same split-borrow pattern as `sync_queries`).
pub struct NavSrc<'a> {
    pub entities: &'a [Entity],
    pub terrain: Option<&'a Terrain>,
    pub render_rev: u64,
    pub tick: u64,
    /// The streamed level's authored walkability layer. Cloned out of the
    /// map before the split borrow, so it is an owned handle here.
    pub static_blocked: Option<Arc<StaticBlocked>>,
}

impl NavMap {
    /// Cells per axis of current coverage (diagnostics).
    pub fn size_cells(&self) -> (i32, i32) {
        (self.w, self.h)
    }

    /// Bytes held by built chunks (diagnostics / budget reporting).
    pub fn built_bytes(&self) -> usize {
        self.chunks.iter().flatten().count() * std::mem::size_of::<NavChunk>()
    }

    /// Number of chunk derivations performed since this map was created.
    pub fn derive_cells_runs(&self) -> u64 {
        self.derive_cells_runs
    }

    /// Dirty every chunk overlapping the world-space box — the terrain-edit
    /// hook (API-level; the producer lands in another track). Also records
    /// the terrain revision so the next sync does NOT full-invalidate: the
    /// caller told us exactly what moved.
    pub fn mark_dirty(
        &mut self,
        min: Vec3f,
        max: Vec3f,
        terrain_rev: u64,
        tick: u64,
    ) {
        if self.w == 0 {
            return;
        }
        let c0x = ((min.x / NAV_CELL).floor() as i32 - self.min_cx).div_euclid(NAV_CHUNK);
        let c0z = ((min.z / NAV_CELL).floor() as i32 - self.min_cz).div_euclid(NAV_CHUNK);
        let c1x = ((max.x / NAV_CELL).ceil() as i32 - self.min_cx).div_euclid(NAV_CHUNK);
        let c1z = ((max.z / NAV_CELL).ceil() as i32 - self.min_cz).div_euclid(NAV_CHUNK);
        let (cw, ch) = (self.w / NAV_CHUNK, self.h / NAV_CHUNK);
        if self.pending_chunks.len() != self.chunks.len() {
            self.pending_chunks.resize(self.chunks.len(), false);
        }
        // The 1-cell derivation apron means an edit inside a chunk can change
        // its neighbours' border cells too — dirty one chunk outward.
        for cz in (c0z - 1).max(0)..=(c1z + 1).min(ch - 1) {
            for cx in (c0x - 1).max(0)..=(c1x + 1).min(cw - 1) {
                self.pending_chunks[(cz * cw + cx) as usize] = true;
            }
        }
        // This revision is acknowledged by the pending bitmap. An unreported
        // terrain revision still takes the full-layout path in `sync`.
        self.built_terrain_rev = terrain_rev;
        if tick.saturating_sub(self.last_invalidation_tick) >= REBUILD_COOLDOWN_TICKS {
            self.flush_pending(tick);
        }
    }

    /// Install (or replace) the streamed level's authored walkability layer.
    ///
    /// `blocked` lists the impassable cells of a `size.0 * size.1` grid whose
    /// `(0, 0)` corner sits at world `origin`, each cell `cell` metres wide;
    /// `floor` is the level's ground height. Passing an empty grid
    /// (`size.0 == 0`) removes the layer. The whole grid is re-derived
    /// immediately: a level change is not a per-tick obstacle edit.
    pub fn set_static_blocked(
        &mut self,
        cells: &[(u32, u32)],
        size: (u32, u32),
        origin: [f32; 2],
        cell: f32,
        floor: f32,
    ) {
        let layer = if size.0 == 0 || size.1 == 0 || cell <= 0.0 {
            None
        } else {
            let mut blocked = vec![false; (size.0 as usize) * (size.1 as usize)];
            for (cx, cz) in cells {
                if *cx < size.0 && *cz < size.1 {
                    blocked[(*cz as usize) * (size.0 as usize) + *cx as usize] = true;
                }
            }
            Some(StaticBlocked {
                origin,
                cell,
                width: size.0,
                height: size.1,
                blocked,
                floor,
                climb: 0.0,
                fall: 0.0,
                floors: Vec::new(),
            })
        };
        self.set_static_layer(layer);
    }

    /// Install (or replace) a fully-formed authored layer — the form a
    /// streamed 3D level uses, because it carries a floor height PER CELL
    /// rather than one plane ([`StaticBlocked::floors`]). `None` removes it.
    pub fn set_static_layer(&mut self, layer: Option<StaticBlocked>) {
        let layer = layer
            .filter(|l| l.width != 0 && l.height != 0 && l.cell > 0.0)
            .map(Arc::new);
        if self.static_blocked.as_deref() == layer.as_deref() {
            return;
        }
        self.static_blocked = layer;
        // Coverage bounds change with the layer, so the whole layout is
        // rebuilt rather than a set of chunks dirtied.
        self.synced_once = false;
        self.chunks.clear();
        self.pending_chunks.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    /// Drop the streamed level's authored layer (level unload).
    pub fn clear_static_blocked(&mut self) {
        self.set_static_blocked(&[], (0, 0), [0.0; 2], 0.0, 0.0);
    }

    /// The installed layer, if any.
    pub fn static_blocked(&self) -> Option<&StaticBlocked> {
        self.static_blocked.as_deref()
    }

    fn flush_pending(&mut self, tick: u64) {
        let mut any = false;
        for (chunk, pending) in self.chunks.iter_mut().zip(&mut self.pending_chunks) {
            if *pending {
                *chunk = None;
                *pending = false;
                any = true;
            }
        }
        if any {
            self.last_invalidation_tick = tick;
            self.generation = self.generation.wrapping_add(1);
        }
    }
}

// ── derivation ──────────────────────────────────────────────────────────

/// Exactly the entities sampled by `derive_cell`. Parts are not entities at
/// all; ordinary sensors, non-colliding decoration, movers, kinematics and
/// rigids are also absent from the derived grid and therefore cannot dirty it.
#[inline]
fn entity_contributes_cells(e: &Entity) -> bool {
    (e.kind == BodyKind::Static && !e.sensor && e.collide)
        || (e.sensor && e.tag == "water")
}

/// Fingerprint only the fields `derive_cell` reads. The renderer's revision
/// remains the cheap "something changed" hint, but a presentation-only edit
/// computes the same fingerprint and leaves every nav chunk intact.
fn entity_cells_hash(entities: &[Entity]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |value: u64| {
        hash ^= value;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    };
    for e in entities.iter().filter(|e| entity_contributes_cells(e)) {
        mix(e.id);
        mix(if e.sensor { 1 } else { 0 });
        mix(e.pos.x.to_bits() as u64);
        mix(e.pos.y.to_bits() as u64);
        mix(e.pos.z.to_bits() as u64);
        mix(e.half.x.to_bits() as u64);
        mix(e.half.y.to_bits() as u64);
        mix(e.half.z.to_bits() as u64);
        mix(e.shape as u64);
    }
    hash
}

/// Bring layout up to date. A renderer revision only prompts a comparison of
/// the actual rasterized inputs. Legitimate entity or unreported terrain
/// changes full-invalidate no more than once per cooldown; stale chunks keep
/// serving queries in between.
fn sync(map: &mut NavMap, src: &NavSrc) {
    let trev = src.terrain.map_or(0, |t| t.revision);
    if !map.synced_once {
        let entity_hash = entity_cells_hash(src.entities);
        map.synced_once = true;
        map.observed_render_rev = src.render_rev;
        map.observed_entity_hash = entity_hash;
        rebuild_layout(map, src, entity_hash, trev);
        map.last_invalidation_tick = src.tick;
        return;
    }

    if map.observed_render_rev != src.render_rev {
        map.observed_render_rev = src.render_rev;
        map.observed_entity_hash = entity_cells_hash(src.entities);
    }

    if src.tick.saturating_sub(map.last_invalidation_tick) < REBUILD_COOLDOWN_TICKS {
        return;
    }

    if map.built_entity_hash != map.observed_entity_hash || map.built_terrain_rev != trev {
        rebuild_layout(map, src, map.observed_entity_hash, trev);
        map.last_invalidation_tick = src.tick;
        return;
    }
    map.flush_pending(src.tick);
}

fn rebuild_layout(map: &mut NavMap, src: &NavSrc, entity_hash: u64, trev: u64) {
    map.built_entity_hash = entity_hash;
    map.built_terrain_rev = trev;
    map.generation = map.generation.wrapping_add(1);

    // Coverage = terrain extent ∪ static solids, padded. No geometry at all
    // = no coverage: every query answers None and consumers keep their
    // straight-line steering, which is what makes empty worlds zero-cost.
    let mut min = vec2f(f32::MAX, f32::MAX);
    let mut max = vec2f(f32::MIN, f32::MIN);
    let mut any = false;
    if let Some(t) = src.terrain {
        let span = (t.cells.saturating_sub(1)) as f32 * t.cell_size;
        min = vec2f(t.origin, t.origin);
        max = vec2f(t.origin + span, t.origin + span);
        any = true;
    }
    for e in src.entities {
        if !entity_contributes_cells(e) || e.sensor {
            continue;
        }
        min.x = min.x.min(e.pos.x - e.half.x);
        min.y = min.y.min(e.pos.z - e.half.z);
        max.x = max.x.max(e.pos.x + e.half.x);
        max.y = max.y.max(e.pos.z + e.half.z);
        any = true;
    }
    if let Some(layer) = src.static_blocked.as_deref() {
        min.x = min.x.min(layer.origin[0]);
        min.y = min.y.min(layer.origin[1]);
        max.x = max.x.max(layer.origin[0] + layer.width as f32 * layer.cell);
        max.y = max.y.max(layer.origin[1] + layer.height as f32 * layer.cell);
        any = true;
    }
    if !any {
        map.w = 0;
        map.h = 0;
        map.chunks.clear();
        map.pending_chunks.clear();
        return;
    }
    const PAD: f32 = 2.0;
    let align_down = |v: f32| {
        let c = ((v - PAD) / NAV_CELL).floor() as i32;
        c.div_euclid(NAV_CHUNK) * NAV_CHUNK
    };
    let align_up_span = |lo: i32, v: f32| {
        let c = ((v + PAD) / NAV_CELL).ceil() as i32;
        let span = ((c - lo).max(NAV_CHUNK) + NAV_CHUNK - 1).div_euclid(NAV_CHUNK) * NAV_CHUNK;
        span.min(NAV_MAX_SPAN)
    };
    map.min_cx = align_down(min.x);
    map.min_cz = align_down(min.y);
    map.w = align_up_span(map.min_cx, max.x);
    map.h = align_up_span(map.min_cz, max.y);
    map.chunks.clear();
    map.chunks
        .resize(((map.w / NAV_CHUNK) * (map.h / NAV_CHUNK)) as usize, None);
    map.pending_chunks.clear();
    map.pending_chunks.resize(map.chunks.len(), false);
}

/// Walk-surface height and blocked-ness at one cell centre. This is THE
/// definition of walkable; everything else in the module is bookkeeping.
///
/// `entities` is the chunk's pre-filtered slice of `src.entities` (same
/// order — id order — so the height sort below stays the only ordering):
/// a chunk touches a handful of solids, the world holds thousands.
fn derive_cell(src: &NavSrc, entities: &[&Entity], cx: i32, cz: i32) -> (Option<f32>, bool) {
    let x = (cx as f32 + 0.5) * NAV_CELL;
    let z = (cz as f32 + 0.5) * NAV_CELL;
    let mut floor: Option<f32> = src.terrain.and_then(|t| t.height_at(x, z));
    // The streamed level's authored layer is ground in its own right: a flat
    // strategy map has no terrain and no static solids under it, so without
    // this seed not one of its cells would be walkable.
    let mut grid_blocked = false;
    if let Some(layer) = src.static_blocked.as_deref() {
        if let Some(i) = layer.index_at(x, z) {
            // A streamed 3D level publishes a height per cell; a tiled map
            // publishes one plane. Either way this is the level's own word
            // on where its floor is here.
            let h = layer.floor_at(i);
            floor = Some(floor.map_or(h, |f: f32| f.max(h)));
            grid_blocked = layer.blocked[i];
        }
    }
    let inflate = NAV_CELL * 0.5;
    // (top, bottom) of every static solid over this cell. Entities are sorted
    // by id; the sort below is by height with total_cmp — fully deterministic.
    let mut solids: Vec<(f32, f32)> = Vec::new();
    let mut water_top: Option<f32> = None;
    for &e in entities {
        if (x - e.pos.x).abs() >= e.half.x + inflate || (z - e.pos.z).abs() >= e.half.z + inflate {
            continue;
        }
        if e.sensor {
            if e.tag == "water" {
                let top = e.pos.y + e.half.y;
                water_top = Some(water_top.map_or(top, |w: f32| w.max(top)));
            }
            continue;
        }
        if !entity_contributes_cells(e) {
            continue;
        }
        // A wedge is a ramp: its walk surface here is GROUND, like terrain —
        // it seeds the floor rather than stepping from whatever lies beneath
        // it (a mid-ramp cell compared against the slab under the ramp would
        // read as a wall). Cell-to-cell rise still catches slopes that are
        // genuinely too steep.
        if e.shape == Shape::Wedge {
            if let Some(surface) = wedge_surface_at(&Solid::from(e), x, z) {
                floor = Some(floor.map_or(surface, |f: f32| f.max(surface)));
            }
            continue;
        }
        solids.push((e.pos.y + e.half.y, e.pos.y - e.half.y));
    }
    solids.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let mut blocked = false;
    for (top, bottom) in solids {
        match floor {
            // First solid from below is the ground here (worlds without
            // terrain stand everything on a big static slab).
            None => floor = Some(top),
            Some(f) => {
                if top <= f + NAV_STEP {
                    // A kerb: walk surface steps up.
                    if top > f {
                        floor = Some(top);
                    }
                } else if bottom < f + NAV_CLEARANCE {
                    // Rises past step-up within headroom: a wall.
                    blocked = true;
                }
                // else: overhead — a bridge you walk under.
            }
        }
    }
    if let (Some(f), Some(wt)) = (floor, water_top) {
        // Water above the walk surface makes the cell unwalkable (D6: the
        // grid derives from terrain + statics + water). Swimming is a wave-3
        // block; nav routes armies around the bay, not through it.
        if wt > f + 0.05 {
            blocked = true;
        }
    }
    (floor, blocked || grid_blocked)
}

/// Derive one chunk. A 1-cell apron is derived alongside so the CLEAR
/// erosion never depends on neighbouring chunks' build state — every chunk
/// is a pure function of the world, which is what makes the grid hash
/// independent of build ORDER.
fn derive_cells(src: &NavSrc, base_cx: i32, base_cz: i32) -> NavChunk {
    const N: usize = (NAV_CHUNK + 2) as usize;
    let mut floor = [0.0f32; N * N];
    let mut walk = [false; N * N];
    // Spatial prefilter: the entities whose inflated box reaches any cell
    // centre of this chunk (apron included), in world order. One pass over
    // the world per chunk instead of one per cell.
    let inflate = NAV_CELL * 0.5;
    let x0 = (base_cx as f32 - 0.5) * NAV_CELL;
    let z0 = (base_cz as f32 - 0.5) * NAV_CELL;
    let x1 = (base_cx as f32 + NAV_CHUNK as f32 + 0.5) * NAV_CELL;
    let z1 = (base_cz as f32 + NAV_CHUNK as f32 + 0.5) * NAV_CELL;
    let entities: Vec<&Entity> = src
        .entities
        .iter()
        .filter(|e| {
            e.pos.x + e.half.x + inflate > x0
                && e.pos.x - e.half.x - inflate < x1
                && e.pos.z + e.half.z + inflate > z0
                && e.pos.z - e.half.z - inflate < z1
        })
        .collect();
    for az in 0..N {
        for ax in 0..N {
            let (f, blocked) = derive_cell(
                src,
                &entities,
                base_cx - 1 + ax as i32,
                base_cz - 1 + az as i32,
            );
            let i = az * N + ax;
            walk[i] = f.is_some() && !blocked;
            floor[i] = f.unwrap_or(0.0);
        }
    }
    let mut chunk = NavChunk {
        flags: [0; CHUNK_AREA],
        floor: [0.0; CHUNK_AREA],
    };
    for z in 0..NAV_CHUNK as usize {
        for x in 0..NAV_CHUNK as usize {
            let a = (z + 1) * N + (x + 1);
            let out = z * NAV_CHUNK as usize + x;
            chunk.floor[out] = floor[a];
            if !walk[a] {
                continue;
            }
            let mut flags = FLAG_WALKABLE;
            let clear = walk[a - 1]
                && walk[a + 1]
                && walk[a - N]
                && walk[a + N]
                && walk[a - N - 1]
                && walk[a - N + 1]
                && walk[a + N - 1]
                && walk[a + N + 1];
            if clear {
                flags |= FLAG_CLEAR;
            }
            chunk.flags[out] = flags;
        }
    }
    chunk
}

/// May a route step between two adjacent cells at these floor heights?
///
/// The world's own symmetric [`NAV_EDGE_RISE`] unless a streamed level
/// installed its walker's real climb/fall limits ([`StaticBlocked::step_ok`]).
#[inline]
fn step_ok(src: &NavSrc, here: f32, next: f32) -> bool {
    match src.static_blocked.as_deref() {
        Some(layer) => layer.step_ok(here, next),
        None => (next - here).abs() <= NAV_EDGE_RISE,
    }
}

#[inline]
fn in_bounds(map: &NavMap, cx: i32, cz: i32) -> bool {
    map.w > 0
        && cx >= map.min_cx
        && cz >= map.min_cz
        && cx < map.min_cx + map.w
        && cz < map.min_cz + map.h
}

/// Flags + floor at a cell, deriving its chunk on first touch.
fn cell(map: &mut NavMap, src: &NavSrc, cx: i32, cz: i32) -> (u8, f32) {
    debug_assert!(in_bounds(map, cx, cz));
    let lx = cx - map.min_cx;
    let lz = cz - map.min_cz;
    let cw = map.w / NAV_CHUNK;
    let ci = ((lz / NAV_CHUNK) * cw + lx / NAV_CHUNK) as usize;
    if map.chunks[ci].is_none() {
        let base_cx = map.min_cx + (lx / NAV_CHUNK) * NAV_CHUNK;
        let base_cz = map.min_cz + (lz / NAV_CHUNK) * NAV_CHUNK;
        map.derive_cells_runs = map.derive_cells_runs.wrapping_add(1);
        map.chunks[ci] = Some(Arc::new(derive_cells(src, base_cx, base_cz)));
    }
    let chunk = map.chunks[ci].as_ref().expect("just built");
    let i = ((lz % NAV_CHUNK) * NAV_CHUNK + lx % NAV_CHUNK) as usize;
    (chunk.flags[i], chunk.floor[i])
}

#[inline]
pub fn cell_of(p: Vec3f) -> (i32, i32) {
    (
        (p.x / NAV_CELL).floor() as i32,
        (p.z / NAV_CELL).floor() as i32,
    )
}

#[inline]
fn cell_center(cx: i32, cz: i32, y: f32) -> Vec3f {
    vec3f(
        (cx as f32 + 0.5) * NAV_CELL,
        y,
        (cz as f32 + 0.5) * NAV_CELL,
    )
}

// ── queries ─────────────────────────────────────────────────────────────

/// Is the straight ground line from `a` to `b` passable for a standard
/// agent? `None` = no coverage there (caller falls back to legacy straight
/// steering — the bit-identical path). Walks the supercover cells; every
/// cell must carry `mask` and consecutive floors must be within
/// [`NAV_EDGE_RISE`].
pub fn line_passable(map: &mut NavMap, src: &NavSrc, a: Vec3f, b: Vec3f, mask: u8) -> Option<bool> {
    sync(map, src);
    let (ax, az) = cell_of(a);
    let (bx, bz) = cell_of(b);
    if !in_bounds(map, ax, az) || !in_bounds(map, bx, bz) {
        return None;
    }
    let (mut cx, mut cz) = (ax, az);
    let (flags, mut last_floor) = cell(map, src, cx, cz);
    if flags & mask == 0 {
        return Some(false);
    }
    let dx = b.x - a.x;
    let dz = b.z - a.z;
    let step_x: i32 = if dx > 0.0 { 1 } else { -1 };
    let step_z: i32 = if dz > 0.0 { 1 } else { -1 };
    // Parametric distance to the next cell border per axis (Amanatides-Woo).
    let next_border = |c: i32, step: i32| -> f32 {
        (if step > 0 { (c + 1) as f32 } else { c as f32 }) * NAV_CELL
    };
    let mut t_max_x = if dx != 0.0 {
        (next_border(cx, step_x) - a.x) / dx
    } else {
        f32::MAX
    };
    let mut t_max_z = if dz != 0.0 {
        (next_border(cz, step_z) - a.z) / dz
    } else {
        f32::MAX
    };
    let t_delta_x = if dx != 0.0 { NAV_CELL / dx.abs() } else { f32::MAX };
    let t_delta_z = if dz != 0.0 { NAV_CELL / dz.abs() } else { f32::MAX };
    let mut guard = (map.w + map.h) * 2;
    while (cx, cz) != (bx, bz) && guard > 0 {
        guard -= 1;
        if t_max_x < t_max_z {
            t_max_x += t_delta_x;
            cx += step_x;
        } else {
            t_max_z += t_delta_z;
            cz += step_z;
        }
        if !in_bounds(map, cx, cz) {
            return None;
        }
        let (flags, floor) = cell(map, src, cx, cz);
        if flags & mask == 0 || !step_ok(src, last_floor, floor) {
            return Some(false);
        }
        last_floor = floor;
    }
    Some(true)
}

/// Nearest cell within [`SNAP_RADIUS`] carrying `mask`, scanning rings in a
/// fixed order (radius, then z, then x) so the snap is deterministic.
fn snap(map: &mut NavMap, src: &NavSrc, cx: i32, cz: i32, mask: u8) -> Option<(i32, i32)> {
    for r in 0..=SNAP_RADIUS {
        for dz in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dz.abs()) != r {
                    continue;
                }
                let (nx, nz) = (cx + dx, cz + dz);
                if !in_bounds(map, nx, nz) {
                    continue;
                }
                if cell(map, src, nx, nz).0 & mask != 0 {
                    return Some((nx, nz));
                }
            }
        }
    }
    None
}

/// A* from `from` to `to` over the grid, string-pulled. Returns false (and
/// clears `out`) when there is no coverage or no route — the caller keeps
/// its straight-line fallback. On success `out` holds smoothed waypoints
/// ending exactly at `to`.
pub fn find_path(map: &mut NavMap, src: &NavSrc, from: Vec3f, to: Vec3f, out: &mut Vec<Vec3f>) -> bool {
    match find_route(map, src, from, to, out) {
        RouteKind::Full => true,
        _ => {
            out.clear();
            false
        }
    }
}

/// What a route search produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteKind {
    /// `out` reaches the goal.
    Full,
    /// No route within the search budget: `out` walks over passable cells
    /// to the reachable point nearest the goal and stops there.
    Partial,
    /// No coverage, or nothing meaningfully nearer than where the agent
    /// already stands.
    None,
}

/// [`find_path`] that does not give up empty-handed: a walled-in or
/// out-of-budget goal still yields the passable path to the reachable point
/// nearest it, so an agent can close in and hold instead of beelining.
pub fn find_route(map: &mut NavMap, src: &NavSrc, from: Vec3f, to: Vec3f, out: &mut Vec<Vec3f>) -> RouteKind {
    out.clear();
    sync(map, src);
    let (fx, fz) = cell_of(from);
    let (tx, tz) = cell_of(to);
    if !in_bounds(map, fx, fz) || !in_bounds(map, tx, tz) {
        return RouteKind::None;
    }
    // Plan on CLEAR (eroded) cells; if either end has no CLEAR nearby or no
    // route exists, retry on the raw WALKABLE mask so narrow gaps stay usable.
    // Of two partial routes, keep the one that ends nearer the goal.
    let mut partial: Option<Vec<Vec3f>> = None;
    for mask in [FLAG_CLEAR, FLAG_WALKABLE] {
        let Some(start) = snap(map, src, fx, fz, mask) else {
            continue;
        };
        let Some(goal) = snap(map, src, tx, tz, mask) else {
            continue;
        };
        match astar(map, src, start, goal, mask, out) {
            RouteKind::Full => {
                string_pull(map, src, from, to, mask, out);
                return RouteKind::Full;
            }
            RouteKind::Partial => {
                let end = *out.last().unwrap();
                string_pull(map, src, from, end, mask, out);
                let nearer = partial
                    .as_ref()
                    .and_then(|p| p.last())
                    .map_or(true, |prev| planar(end, to) < planar(*prev, to));
                if nearer {
                    partial = Some(std::mem::take(out));
                } else {
                    out.clear();
                }
            }
            RouteKind::None => out.clear(),
        }
    }
    match partial {
        Some(p) => {
            *out = p;
            RouteKind::Partial
        }
        None => RouteKind::None,
    }
}

/// A partial route must end at least this much nearer the goal (octile
/// units: 10 per straight cell) than the start to be worth walking.
const PARTIAL_GAIN: u32 = 30;

/// Grid A*: 10/14 costs, octile heuristic, deterministic tie-break (lower
/// f, then lower cell index). Bounded to a working region around the
/// endpoints so cost never scales with world size.
fn astar(
    map: &mut NavMap,
    src: &NavSrc,
    start: (i32, i32),
    goal: (i32, i32),
    mask: u8,
    out: &mut Vec<Vec3f>,
) -> RouteKind {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    // Working region: endpoints' bounding box inflated, clamped to coverage
    // and to SEARCH_SPAN (anchored on the box centre; endpoints outside a
    // clamped region mean the route is longer than we search — fail fast).
    let margin = 32;
    let bx0 = start.0.min(goal.0) - margin;
    let bz0 = start.1.min(goal.1) - margin;
    let bx1 = start.0.max(goal.0) + margin;
    let bz1 = start.1.max(goal.1) + margin;
    let (cx, cz) = ((bx0 + bx1) / 2, (bz0 + bz1) / 2);
    let half_w = ((bx1 - bx0).min(SEARCH_SPAN)) / 2;
    let half_h = ((bz1 - bz0).min(SEARCH_SPAN)) / 2;
    let rx0 = (cx - half_w).max(map.min_cx);
    let rz0 = (cz - half_h).max(map.min_cz);
    let rx1 = (cx + half_w).min(map.min_cx + map.w - 1);
    let rz1 = (cz + half_h).min(map.min_cz + map.h - 1);
    let rw = rx1 - rx0 + 1;
    let rh = rz1 - rz0 + 1;
    let inside = |x: i32, z: i32| x >= rx0 && z >= rz0 && x <= rx1 && z <= rz1;
    if !inside(start.0, start.1) || !inside(goal.0, goal.1) {
        return RouteKind::None;
    }
    let idx = |x: i32, z: i32| ((z - rz0) * rw + (x - rx0)) as usize;
    let cells = (rw * rh) as usize;
    let mut g = vec![u32::MAX; cells];
    let mut came: Vec<u32> = vec![u32::MAX; cells];
    let octile = |x: i32, z: i32| -> u32 {
        let dx = (x - goal.0).abs() as u32;
        let dz = (z - goal.1).abs() as u32;
        let (lo, hi) = (dx.min(dz), dx.max(dz));
        14 * lo + 10 * (hi - lo)
    };
    let mut open: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::new();
    let si = idx(start.0, start.1);
    g[si] = 0;
    let start_h = octile(start.0, start.1);
    open.push(Reverse((start_h, si as u32)));
    let mut pops = 0usize;
    let mut found = false;
    // The expanded cell nearest the goal (heuristic, then lower index):
    // where a partial route ends when the goal is out of reach.
    let mut best = (start_h, si);
    while let Some(Reverse((f, ci))) = open.pop() {
        let ci = ci as usize;
        let x = rx0 + (ci as i32 % rw);
        let z = rz0 + (ci as i32 / rw);
        let h = octile(x, z);
        if f > g[ci].saturating_add(h) {
            continue; // stale entry
        }
        if (x, z) == goal {
            found = true;
            break;
        }
        pops += 1;
        if pops > ASTAR_BUDGET {
            break;
        }
        if h < best.0 || (h == best.0 && ci < best.1) {
            best = (h, ci);
        }
        let (_, here_floor) = cell(map, src, x, z);
        for &(dx, dz, cost) in NEIGHBORS.iter() {
            let (nx, nz) = (x + dx, z + dz);
            if !inside(nx, nz) {
                continue;
            }
            let (nf, nfloor) = cell(map, src, nx, nz);
            if nf & mask == 0 || !step_ok(src, here_floor, nfloor) {
                continue;
            }
            // No corner cutting: diagonals need both orthogonal cells open.
            if dx != 0 && dz != 0 {
                let (ax, _) = cell(map, src, x + dx, z);
                let (az, _) = cell(map, src, x, z + dz);
                if ax & mask == 0 || az & mask == 0 {
                    continue;
                }
            }
            let ni = idx(nx, nz);
            let ng = g[ci] + cost;
            if ng < g[ni] {
                g[ni] = ng;
                came[ni] = ci as u32;
                open.push(Reverse((ng + octile(nx, nz), ni as u32)));
            }
        }
    }
    let end = if found {
        idx(goal.0, goal.1)
    } else if best.0 + PARTIAL_GAIN <= start_h {
        best.1
    } else {
        return RouteKind::None;
    };
    // Reconstruct end→start, then reverse.
    let mut ci = end;
    loop {
        let x = rx0 + (ci as i32 % rw);
        let z = rz0 + (ci as i32 / rw);
        let (_, floor) = cell(map, src, x, z);
        out.push(cell_center(x, z, floor));
        if ci == si {
            break;
        }
        ci = came[ci] as usize;
    }
    out.reverse();
    if found {
        RouteKind::Full
    } else {
        RouteKind::Partial
    }
}

/// String-pulling: greedily keep only waypoints the previous kept point
/// cannot see past, so movement doesn't look grid-locked. Ends exactly at
/// the caller's `to`.
fn string_pull(map: &mut NavMap, src: &NavSrc, from: Vec3f, to: Vec3f, mask: u8, path: &mut Vec<Vec3f>) {
    let raw = std::mem::take(path);
    let mut cur = from;
    let mut i = 0usize;
    while i < raw.len() {
        let mut j = i;
        while j + 1 < raw.len() && line_passable(map, src, cur, raw[j + 1], mask) == Some(true) {
            j += 1;
        }
        path.push(raw[j]);
        cur = raw[j];
        i = j + 1;
    }
    // The final leg heads for the exact goal point, not its cell centre.
    if let Some(last) = path.last_mut() {
        if line_passable(map, src, cur, to, mask) == Some(true) {
            *last = to;
        } else {
            path.push(to);
        }
    } else {
        path.push(to);
    }
}

// ── flow fields (group orders) ──────────────────────────────────────────

/// A Dijkstra integration field over a working region, descended by every
/// unit in a group order — one solve serves a hundred units, which is the
/// whole point (per-unit A* at RTS scale would burn the tick budget).
#[derive(Clone, Debug, Default)]
pub struct FlowField {
    min_cx: i32,
    min_cz: i32,
    w: i32,
    h: i32,
    /// Integration cost per cell; `u16::MAX` = unreachable.
    cost: Vec<u16>,
    /// Descent direction per cell: index into [`NEIGHBORS`], 8 = at target.
    dir: Vec<u8>,
    /// The world-space goal this field descends toward.
    pub target: Vec3f,
    /// Nav generation this field was derived from — stale when it moves.
    pub generation: u64,
}

impl FlowField {
    /// Direction to walk from `p` (unit-length on the ground plane) and the
    /// remaining integration cost. None = outside the field or unreachable.
    pub fn sample(&self, p: Vec3f) -> Option<(Vec3f, u16)> {
        let (cx, cz) = cell_of(p);
        if self.w == 0
            || cx < self.min_cx
            || cz < self.min_cz
            || cx >= self.min_cx + self.w
            || cz >= self.min_cz + self.h
        {
            return None;
        }
        let i = ((cz - self.min_cz) * self.w + (cx - self.min_cx)) as usize;
        let cost = self.cost[i];
        if cost == u16::MAX {
            return None;
        }
        let dir = match self.dir[i] {
            8 => vec3f(0.0, 0.0, 0.0),
            d => {
                let (dx, dz, cost) = NEIGHBORS[d as usize];
                let s = if cost == 14 {
                    std::f32::consts::FRAC_1_SQRT_2
                } else {
                    1.0
                };
                vec3f(dx as f32 * s, 0.0, dz as f32 * s)
            }
        };
        Some((dir, cost))
    }

    pub fn cells(&self) -> usize {
        self.cost.len()
    }
}

/// Build a flow field descending to `target`, covering `around` (the group)
/// plus margin. Deterministic: Dijkstra with (cost, cell-index) tie-break,
/// descent direction by first-lowest in the fixed neighbour order.
pub fn build_flow(map: &mut NavMap, src: &NavSrc, target: Vec3f, around: &[Vec3f]) -> Option<FlowField> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    sync(map, src);
    let (tx, tz) = cell_of(target);
    if !in_bounds(map, tx, tz) {
        return None;
    }
    let goal = snap(map, src, tx, tz, FLAG_CLEAR)
        .or_else(|| snap(map, src, tx, tz, FLAG_WALKABLE))?;
    // Region: target ∪ group, inflated, clamped to coverage and to
    // SEARCH_SPAN anchored on the target (the far side of a huge group goes
    // uncovered; those units fall back to single-agent pathing).
    let margin = 16;
    let (mut bx0, mut bz0, mut bx1, mut bz1) = (goal.0, goal.1, goal.0, goal.1);
    for p in around {
        let (cx, cz) = cell_of(*p);
        bx0 = bx0.min(cx);
        bz0 = bz0.min(cz);
        bx1 = bx1.max(cx);
        bz1 = bz1.max(cz);
    }
    let rx0 = (bx0 - margin).max(goal.0 - SEARCH_SPAN / 2).max(map.min_cx);
    let rz0 = (bz0 - margin).max(goal.1 - SEARCH_SPAN / 2).max(map.min_cz);
    let rx1 = (bx1 + margin)
        .min(goal.0 + SEARCH_SPAN / 2)
        .min(map.min_cx + map.w - 1);
    let rz1 = (bz1 + margin)
        .min(goal.1 + SEARCH_SPAN / 2)
        .min(map.min_cz + map.h - 1);
    let rw = rx1 - rx0 + 1;
    let rh = rz1 - rz0 + 1;
    if rw <= 0 || rh <= 0 {
        return None;
    }
    let cells = (rw * rh) as usize;
    let idx = |x: i32, z: i32| ((z - rz0) * rw + (x - rx0)) as usize;
    let inside = |x: i32, z: i32| x >= rx0 && z >= rz0 && x <= rx1 && z <= rz1;
    let mut cost = vec![u32::MAX; cells];
    let mut open: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::new();
    let gi = idx(goal.0, goal.1);
    cost[gi] = 0;
    open.push(Reverse((0, gi as u32)));
    while let Some(Reverse((c, ci))) = open.pop() {
        let ci = ci as usize;
        if c > cost[ci] {
            continue;
        }
        let x = rx0 + (ci as i32 % rw);
        let z = rz0 + (ci as i32 / rw);
        let (_, here_floor) = cell(map, src, x, z);
        for &(dx, dz, step) in NEIGHBORS.iter() {
            let (nx, nz) = (x + dx, z + dz);
            if !inside(nx, nz) {
                continue;
            }
            let (nf, nfloor) = cell(map, src, nx, nz);
            if nf & FLAG_WALKABLE == 0 || !step_ok(src, here_floor, nfloor) {
                continue;
            }
            if dx != 0 && dz != 0 {
                let (ax, _) = cell(map, src, x + dx, z);
                let (az, _) = cell(map, src, x, z + dz);
                if ax & FLAG_WALKABLE == 0 || az & FLAG_WALKABLE == 0 {
                    continue;
                }
            }
            let ni = idx(nx, nz);
            let nc = c + step;
            if nc < cost[ni] {
                cost[ni] = nc;
                open.push(Reverse((nc, ni as u32)));
            }
        }
    }
    // Descent directions: for every reachable cell, the first neighbour (in
    // fixed order) with the lowest integration cost.
    let mut dir = vec![8u8; cells];
    let mut cost16 = vec![u16::MAX; cells];
    for ci in 0..cells {
        if cost[ci] == u32::MAX {
            continue;
        }
        cost16[ci] = cost[ci].min(u16::MAX as u32 - 1) as u16;
        if ci == gi {
            continue;
        }
        let x = rx0 + (ci as i32 % rw);
        let z = rz0 + (ci as i32 / rw);
        let mut best = cost[ci];
        let mut best_dir = 8u8;
        for (d, &(dx, dz, _)) in NEIGHBORS.iter().enumerate() {
            let (nx, nz) = (x + dx, z + dz);
            if !inside(nx, nz) {
                continue;
            }
            if dx != 0 && dz != 0 {
                // Same no-corner-cutting rule on the way DOWN the field.
                let ai = idx(x + dx, z);
                let bi = idx(x, z + dz);
                if cost[ai] == u32::MAX || cost[bi] == u32::MAX {
                    continue;
                }
            }
            let nc = cost[idx(nx, nz)];
            if nc < best {
                best = nc;
                best_dir = d as u8;
            }
        }
        dir[ci] = best_dir;
    }
    Some(FlowField {
        min_cx: rx0,
        min_cz: rz0,
        w: rw,
        h: rh,
        cost: cost16,
        dir,
        target,
        generation: map.generation,
    })
}

// ── single-agent path following ─────────────────────────────────────────

/// Path cache + steering for one agent — the shared machinery that upgrades
/// `chase`/`patrol`/`wander`, the NPC utility AI, and off-field units. The
/// contract that keeps existing games bit-identical: **when the straight
/// line to the goal is passable (or nav has no coverage), `steer` returns
/// the goal itself** and the caller's math sees numbers indistinguishable
/// from the pre-nav engine.
#[derive(Clone, Debug, Default)]
pub struct NavAgent {
    path: Vec<Vec3f>,
    at: usize,
    goal: Vec3f,
    generation: u64,
    /// Re-plan cooldown (ticks) for moving targets, so a chase does not
    /// solve A* sixty times a second.
    cool: u16,
    /// Ticks to stand before trying again after a failed or partial plan —
    /// doubling per consecutive failure up to `HOLD_MAX`. The no-route case
    /// used to re-plan every tick and then beeline at the goal.
    hold: u16,
    fails: u8,
    /// The current path stops short of the goal (`RouteKind::Partial`).
    partial: bool,
}

/// Longest hold between plan attempts on a goal that keeps failing (4 s).
const HOLD_MAX: u16 = 240;

fn hold_for(fails: u8) -> u16 {
    (REPLAN_COOLDOWN << fails.min(4) as u16).min(HOLD_MAX)
}

/// How far a goal may drift from the planned one before a re-plan (moving
/// chase targets).
const REPLAN_DRIFT: f32 = 2.0;
const REPLAN_COOLDOWN: u16 = 15;
/// A waypoint within this planar distance counts as consumed.
const WAYPOINT_REACHED: f32 = 0.7;

impl NavAgent {
    /// Currently routing around something?
    pub fn active(&self) -> bool {
        !self.path.is_empty()
    }

    pub fn reset(&mut self) {
        self.path.clear();
        self.at = 0;
        self.cool = 0;
        self.hold = 0;
        self.fails = 0;
        self.partial = false;
    }

    /// Standing still because the goal has no route (or the partial route
    /// to it is walked out), waiting for the next attempt.
    pub fn holding(&self) -> bool {
        self.hold > 0 && (self.path.is_empty() || (self.partial && self.at >= self.path.len()))
    }

    /// Consecutive plan attempts that found no full route.
    pub fn failures(&self) -> u8 {
        self.fails
    }

    /// The point to steer straight at this tick, given where the agent
    /// ultimately wants to go.
    pub fn steer(&mut self, world: &mut GameWorld, pos: Vec3f, goal: Vec3f) -> Vec3f {
        match world.nav_line_clear(pos, goal) {
            None | Some(true) => {
                // Straight line is fine (or no coverage): legacy steering,
                // bit-identical to the pre-nav brains.
                if !self.path.is_empty() || self.hold > 0 {
                    self.reset();
                }
                goal
            }
            Some(false) => {
                if self.cool > 0 {
                    self.cool -= 1;
                }
                if self.hold > 0 {
                    self.hold -= 1;
                }
                let drifted = planar(goal, self.goal) > REPLAN_DRIFT;
                let stale = self.generation != world.nav.generation;
                let exhausted =
                    self.path.is_empty() || (self.partial && self.at >= self.path.len());
                if stale || (self.hold == 0 && (exhausted || (drifted && self.cool == 0))) {
                    self.cool = REPLAN_COOLDOWN;
                    self.goal = goal;
                    self.generation = world.nav.generation;
                    self.at = 0;
                    let mut path = std::mem::take(&mut self.path);
                    match world.nav_find_route(pos, goal, &mut path) {
                        RouteKind::Full => {
                            self.partial = false;
                            self.fails = 0;
                            self.hold = 0;
                        }
                        RouteKind::Partial => {
                            self.partial = true;
                            self.fails = self.fails.saturating_add(1);
                            self.hold = hold_for(self.fails);
                        }
                        RouteKind::None => {
                            path.clear();
                            self.partial = false;
                            self.fails = self.fails.saturating_add(1);
                            self.hold = hold_for(self.fails);
                        }
                    }
                    self.path = path;
                }
                if self.path.is_empty() {
                    // No route: HOLD. Standing where the agent is known to be
                    // safe beats walking a straight line into whatever
                    // blocked the plan (water, a wall, a drop); the
                    // stuck/give-up machinery above this stays in charge and
                    // the next attempt comes after the hold.
                    return pos;
                }
                while self.at < self.path.len() && planar(pos, self.path[self.at]) < WAYPOINT_REACHED
                {
                    self.at += 1;
                }
                // Waypoint skip: if the one after next is already visible,
                // cut the corner — this is what keeps paths from looking
                // grid-locked while the agent walks them.
                if self.at + 1 < self.path.len()
                    && world.nav_line_clear(pos, self.path[self.at + 1]) == Some(true)
                {
                    self.at += 1;
                }
                if self.at >= self.path.len() {
                    // A full route ends at the goal itself; a partial one
                    // ends as near as the world allows — hold there.
                    if self.partial {
                        pos
                    } else {
                        goal
                    }
                } else {
                    self.path[self.at]
                }
            }
        }
    }
}

fn planar(a: Vec3f, b: Vec3f) -> f32 {
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    crate::math::sqrt(dx * dx + dz * dz)
}

// ── world-facing wrappers ───────────────────────────────────────────────

impl GameWorld {
    fn nav_src(&mut self) -> (&mut NavMap, NavSrc<'_>) {
        // Cloned (one `Arc` bump) before the split borrow so the derivation
        // can read the layer while the map itself is borrowed mutably.
        let static_blocked = self.nav.static_blocked.clone();
        let GameWorld {
            nav,
            entities,
            terrain,
            render_rev,
            tick,
            ..
        } = self;
        (
            nav,
            NavSrc {
                entities,
                terrain: terrain.as_ref(),
                render_rev: *render_rev,
                tick: *tick,
                static_blocked,
            },
        )
    }

    /// Fold a streamed level's authored walkability into the derived grid —
    /// see [`NavMap::set_static_blocked`]. Call once per level load.
    pub fn nav_set_static_blocked(
        &mut self,
        cells: &[(u32, u32)],
        size: (u32, u32),
        origin: [f32; 2],
        cell: f32,
        floor: f32,
    ) {
        self.nav.set_static_blocked(cells, size, origin, cell, floor);
    }

    /// Fold a streamed 3D level's own walkable surface in — the same layer,
    /// but with a floor HEIGHT per cell ([`StaticBlocked::floors`]) so a
    /// map with rooms at a dozen heights is one navigable ground.
    pub fn nav_set_static_layer(&mut self, layer: StaticBlocked) {
        self.nav.set_static_layer(Some(layer));
    }

    /// Drop the streamed level's authored layer (level unload).
    pub fn nav_clear_static_blocked(&mut self) {
        self.nav.clear_static_blocked();
    }

    /// Straight-line passability for a standard agent; None = no coverage.
    /// Streamed-level routing is deliberately not folded into this derived
    /// entity grid: ActorKit asks the level's one-step provider directly.
    pub fn nav_line_clear(&mut self, a: Vec3f, b: Vec3f) -> Option<bool> {
        let (map, src) = self.nav_src();
        line_passable(map, &src, a, b, FLAG_CLEAR)
    }

    /// A route through the entity-derived grid. False = no coverage or no
    /// route. Streamed-level ActorKit routing uses `NavProvider::next_step`.
    pub fn nav_find_path(&mut self, from: Vec3f, to: Vec3f, out: &mut Vec<Vec3f>) -> bool {
        let (map, src) = self.nav_src();
        find_path(map, &src, from, to, out)
    }

    /// `nav_find_path` that hands back the passable path to the reachable
    /// point nearest an unroutable goal (`RouteKind::Partial`) instead of
    /// nothing — what `NavAgent` walks before it holds.
    pub fn nav_find_route(&mut self, from: Vec3f, to: Vec3f, out: &mut Vec<Vec3f>) -> RouteKind {
        let (map, src) = self.nav_src();
        find_route(map, &src, from, to, out)
    }

    /// Flow field toward `target` covering the group at `around`.
    pub fn nav_flow_field(&mut self, target: Vec3f, around: &[Vec3f]) -> Option<FlowField> {
        let (map, src) = self.nav_src();
        build_flow(map, &src, target, around)
    }

    /// Terrain-edit dirty hook: re-derive only the chunks under this box.
    /// Call AFTER bumping `Terrain::revision` — the ack stored here is what
    /// stops the next query from full-invalidating.
    pub fn nav_mark_dirty(&mut self, min: Vec3f, max: Vec3f) {
        let trev = self.terrain.as_ref().map_or(0, |t| t.revision);
        let tick = self.tick;
        // The first edit may arrive before any query established coverage.
        // After that, do not call `sync` before acknowledging the regional
        // terrain revision: it would mistake the just-reported edit for an
        // unreported one and discard the full layout.
        if !self.nav.synced_once {
            let (map, src) = self.nav_src();
            sync(map, &src);
        }
        self.nav.mark_dirty(min, max, trev, tick);
    }

    /// Fully derive the grid and hash it — the determinism gate. Chunk build
    /// order cannot matter (each chunk derives from the world alone), and
    /// this hashes in fixed row-major order regardless.
    pub fn nav_grid_hash(&mut self) -> u64 {
        let (map, src) = self.nav_src();
        sync(map, &src);
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x100_0000_01b3);
        };
        mix(map.min_cx as u64);
        mix(map.min_cz as u64);
        mix(map.w as u64);
        mix(map.h as u64);
        // The streamed level's authored layer is part of what the grid IS,
        // so the determinism gate hashes its descriptor as well as the cell
        // flags it produced.
        mix(src.static_blocked.as_deref().map_or(0, |l| l.fingerprint()));
        for cz in 0..(map.h / NAV_CHUNK) {
            for cx in 0..(map.w / NAV_CHUNK) {
                // Touch one cell to force the chunk build.
                let _ = cell(map, &src, map.min_cx + cx * NAV_CHUNK, map.min_cz + cz * NAV_CHUNK);
                let chunk = map.chunks[(cz * (map.w / NAV_CHUNK) + cx) as usize]
                    .as_ref()
                    .expect("built above");
                for i in 0..CHUNK_AREA {
                    mix(chunk.flags[i] as u64);
                    if chunk.flags[i] & FLAG_WALKABLE != 0 {
                        mix(chunk.floor[i].to_bits() as u64);
                    }
                }
            }
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::Entity;

    fn solid(id: u64, pos: Vec3f, half: Vec3f) -> Entity {
        Entity {
            id,
            kind: BodyKind::Static,
            pos,
            half,
            collide: true,
            ..Default::default()
        }
    }

    /// A 40×40 plaza slab with a wall across the middle, one 3-unit gap.
    fn plaza() -> GameWorld {
        let mut world = GameWorld::new();
        world.push_entity(solid(1, vec3f(0.0, -0.5, 0.0), vec3f(20.0, 0.5, 20.0)));
        // Wall along z=0 from x=-20..-1.5 and x=1.5..20, 2 high.
        world.push_entity(solid(2, vec3f(-10.75, 1.0, 0.0), vec3f(9.25, 1.0, 0.5)));
        world.push_entity(solid(3, vec3f(10.75, 1.0, 0.0), vec3f(9.25, 1.0, 0.5)));
        world
    }

    #[test]
    fn walls_block_and_gaps_pass() {
        let mut world = plaza();
        // Straight across the wall: blocked.
        assert_eq!(
            world.nav_line_clear(vec3f(-5.0, 0.5, -8.0), vec3f(-5.0, 0.5, 8.0)),
            Some(false)
        );
        // Through the gap: clear.
        assert_eq!(
            world.nav_line_clear(vec3f(0.0, 0.5, -8.0), vec3f(0.0, 0.5, 8.0)),
            Some(true)
        );
        // Open floor: clear.
        assert_eq!(
            world.nav_line_clear(vec3f(-8.0, 0.5, -8.0), vec3f(8.0, 0.5, -8.0)),
            Some(true)
        );
    }

    #[test]
    fn path_routes_through_the_gap() {
        let mut world = plaza();
        let from = vec3f(-6.0, 0.5, -8.0);
        let to = vec3f(-6.0, 0.5, 8.0);
        let mut path = Vec::new();
        assert!(world.nav_find_path(from, to, &mut path), "route exists");
        assert!(path.len() >= 2, "must detour, not beeline: {path:?}");
        // Every waypoint funnels through the gap region on the wall line.
        let crossing = path
            .windows(2)
            .find(|w| (w[0].z <= 0.0) != (w[1].z <= 0.0))
            .expect("path crosses the wall line");
        let t = (0.0 - crossing[0].z) / (crossing[1].z - crossing[0].z);
        let x_at_wall = crossing[0].x + (crossing[1].x - crossing[0].x) * t;
        assert!(
            x_at_wall.abs() < 1.5,
            "crossing must be inside the gap, was x={x_at_wall}"
        );
        assert_eq!(*path.last().unwrap(), to, "path ends at the exact goal");
    }

    /// A goal walled in on all four sides has no route. The search must not
    /// come back empty: it yields the passable path to the nearest reachable
    /// point, and the agent walks it and HOLDS there — it never steers a
    /// straight line into the pen, and it backs off between attempts
    /// instead of solving A* every tick.
    #[test]
    fn unreachable_goal_yields_a_partial_route_and_the_agent_holds() {
        let mut world = plaza();
        // A closed pen around (12, 12) on the north half of the plaza.
        world.push_entity(solid(4, vec3f(12.0, 1.0, 8.5), vec3f(3.5, 1.0, 0.5)));
        world.push_entity(solid(5, vec3f(12.0, 1.0, 15.5), vec3f(3.5, 1.0, 0.5)));
        world.push_entity(solid(6, vec3f(8.5, 1.0, 12.0), vec3f(0.5, 1.0, 3.5)));
        world.push_entity(solid(7, vec3f(15.5, 1.0, 12.0), vec3f(0.5, 1.0, 3.5)));
        let from = vec3f(-6.0, 0.5, 8.0);
        let to = vec3f(12.0, 0.5, 12.0);
        let mut path = Vec::new();
        assert!(!world.nav_find_path(from, to, &mut path), "the pen is sealed");
        assert_eq!(world.nav_find_route(from, to, &mut path), RouteKind::Partial);
        let end = *path.last().unwrap();
        assert!(planar(end, to) < planar(from, to), "the partial route gets nearer: {end:?}");
        assert!(planar(end, to) > 3.5, "but stops outside the pen: {end:?}");

        let mut agent = NavAgent::default();
        let mut pos = from;
        let mut plans_at_start = None;
        for _ in 0..600 {
            let target = agent.steer(&mut world, pos, to);
            assert!(planar(target, to) > 3.5, "no-route must never steer at the goal: {target:?}");
            let len = planar(target, pos);
            if len > 1.0e-6 {
                let step = len.min(0.3);
                pos = pos + vec3f(target.x - pos.x, 0.0, target.z - pos.z) * (step / len);
            }
            if plans_at_start.is_none() {
                plans_at_start = Some(agent.failures());
            }
        }
        assert!(agent.failures() >= 2, "attempts back off and repeat: {}", agent.failures());
        assert!(agent.failures() <= 8, "but not sixty times a second: {}", agent.failures());
        let d = planar(pos, to);
        assert!(d > 3.5 && d < 10.0, "closed in on the pen and stayed out of it: {pos:?}");
    }

    /// A streamed strategy map: no terrain, no static bodies, only the
    /// authored grid. The layer must make the map walkable at all, mark its
    /// wall impassable, and make a flow field route around that wall.
    #[test]
    fn an_authored_grid_makes_a_flat_level_walkable_and_its_wall_solid() {
        const CELL: f32 = 6.0;
        let mut world = GameWorld::new();
        // Nothing to walk on until the layer says otherwise.
        assert_eq!(
            world.nav_line_clear(vec3f(3.0, 0.0, 3.0), vec3f(9.0, 0.0, 3.0)),
            None,
            "an empty world has no coverage"
        );
        // 6x6 cells, a wall across row 3 with one gap at column 2.
        let blocked: Vec<(u32, u32)> = [0u32, 1, 3, 4, 5].iter().map(|cx| (*cx, 3)).collect();
        world.nav_set_static_blocked(&blocked, (6, 6), [0.0, 0.0], CELL, 0.0);

        let open = vec3f(9.0, 0.0, 3.0);
        let across = vec3f(9.0, 0.0, 33.0);
        assert_eq!(
            world.nav_line_clear(open, across),
            Some(false),
            "the authored wall blocks the straight line"
        );
        assert_eq!(
            world.nav_line_clear(vec3f(3.0, 0.0, 3.0), vec3f(27.0, 0.0, 3.0)),
            Some(true),
            "the rest of the map is open ground"
        );

        let mut path = Vec::new();
        assert!(world.nav_find_path(open, across, &mut path), "route exists");
        let crossing = path
            .windows(2)
            .find(|w| (w[0].z <= 3.5 * CELL) != (w[1].z <= 3.5 * CELL))
            .expect("path crosses the wall row");
        let t = (3.5 * CELL - crossing[0].z) / (crossing[1].z - crossing[0].z);
        let x_at_wall = crossing[0].x + (crossing[1].x - crossing[0].x) * t;
        assert!(
            (2.0 * CELL..3.0 * CELL).contains(&x_at_wall),
            "the detour must go through the gap column, was x={x_at_wall}"
        );

        // The group flow field sees the same wall.
        let flow = world
            .nav_flow_field(across, &[open])
            .expect("flow field over the covered map");
        let (dir, _) = flow.sample(open).expect("the start is inside the field");
        assert!(
            dir.x > 0.0 && dir.z > 0.0,
            "the field must send the group toward the gap, dir=({},{})",
            dir.x,
            dir.z
        );

        // The gate covers the layer, and dropping it drops the coverage.
        let hashed = world.nav_grid_hash();
        assert_eq!(hashed, world.nav_grid_hash(), "stable");
        world.nav_clear_static_blocked();
        assert_ne!(hashed, world.nav_grid_hash(), "the layer is part of the grid");
        assert_eq!(world.nav_line_clear(open, across), None, "coverage went with it");
    }

    #[test]
    fn kerbs_walk_walls_do_not() {
        let mut world = GameWorld::new();
        world.push_entity(solid(1, vec3f(0.0, -0.5, 0.0), vec3f(20.0, 0.5, 20.0)));
        // A kerb 0.4 high: within step-up, stays walkable.
        world.push_entity(solid(2, vec3f(0.0, 0.2, -5.0), vec3f(4.0, 0.2, 1.0)));
        // A wall 2 high at +5.
        world.push_entity(solid(3, vec3f(0.0, 1.0, 5.0), vec3f(4.0, 1.0, 1.0)));
        assert_eq!(
            world.nav_line_clear(vec3f(0.0, 0.5, -8.0), vec3f(0.0, 0.5, -2.0)),
            Some(true),
            "kerb is a step, not a wall"
        );
        assert_eq!(
            world.nav_line_clear(vec3f(0.0, 0.5, 2.0), vec3f(0.0, 0.5, 8.0)),
            Some(false),
            "wall blocks"
        );
    }

    #[test]
    fn water_is_unwalkable() {
        let mut world = plaza();
        world.push_entity(Entity {
            id: 9,
            kind: BodyKind::Static,
            pos: vec3f(8.0, 0.25, -8.0),
            half: vec3f(3.0, 0.5, 3.0),
            sensor: true,
            collide: false,
            tag: "water".into(),
            ..Default::default()
        });
        assert_eq!(
            world.nav_line_clear(vec3f(2.0, 0.5, -8.0), vec3f(14.0, 0.5, -8.0)),
            Some(false),
            "pond blocks the straight line"
        );
        let mut path = Vec::new();
        assert!(
            world.nav_find_path(vec3f(2.0, 0.5, -8.0), vec3f(14.0, 0.5, -8.0), &mut path),
            "routes around the pond"
        );
    }

    #[test]
    fn empty_world_has_no_coverage() {
        let mut world = GameWorld::new();
        assert_eq!(world.nav_line_clear(vec3f(0.0, 0.0, 0.0), vec3f(5.0, 0.0, 5.0)), None);
        let mut path = Vec::new();
        assert!(!world.nav_find_path(vec3f(0.0, 0.0, 0.0), vec3f(5.0, 0.0, 5.0), &mut path));
    }

    #[test]
    fn grid_hash_is_deterministic_and_order_independent() {
        let mut a = plaza();
        // Warm some chunks in a query-driven (partial, different) order first.
        let _ = a.nav_line_clear(vec3f(-5.0, 0.5, -8.0), vec3f(-5.0, 0.5, 8.0));
        let ha = a.nav_grid_hash();
        let mut b = plaza();
        let hb = b.nav_grid_hash();
        assert_eq!(ha, hb, "two derivations of the same world must hash equal");
        // A second full pass over already-built chunks is stable too.
        assert_eq!(ha, a.nav_grid_hash());
    }

    #[test]
    fn dirty_region_rederives_after_world_change() {
        let mut world = plaza();
        assert_eq!(
            world.nav_line_clear(vec3f(-5.0, 0.5, -8.0), vec3f(-5.0, 0.5, 8.0)),
            Some(false)
        );
        let gen_before = world.nav.generation;
        // Knock the west wall down (static change bumps render_rev via the
        // caller contract — mark_render_dirty is what the DSL does).
        world.entities.retain(|e| e.id != 2);
        world.mark_render_dirty();
        assert_eq!(
            world.nav_line_clear(vec3f(-5.0, 0.5, -8.0), vec3f(-5.0, 0.5, 8.0)),
            Some(false),
            "the stale grid is served during the rebuild cooldown"
        );
        assert_eq!(world.nav.generation, gen_before);
        world.tick += REBUILD_COOLDOWN_TICKS;
        assert_eq!(
            world.nav_line_clear(vec3f(-5.0, 0.5, -8.0), vec3f(-5.0, 0.5, 8.0)),
            Some(true),
            "grid re-derived after the cooldown"
        );
        assert!(world.nav.generation > gen_before, "generation moved");
    }

    #[test]
    fn mark_dirty_rederives_only_touched_chunks() {
        let mut world = plaza();
        let _ = world.nav_grid_hash(); // build everything
        let built = world.nav.built_bytes();
        world.nav_mark_dirty(vec3f(-2.0, 0.0, -2.0), vec3f(2.0, 0.0, 2.0));
        assert_eq!(
            world.nav.built_bytes(),
            built,
            "regional edits keep serving stale chunks during the cooldown"
        );
        world.tick += REBUILD_COOLDOWN_TICKS;
        // The first query after the cooldown drops the pending chunks and
        // lazily re-derives only what that query touches.
        assert_eq!(
            world.nav_line_clear(vec3f(0.0, 0.5, -8.0), vec3f(0.0, 0.5, 8.0)),
            Some(true)
        );
        assert!(
            world.nav.built_bytes() < built,
            "some chunks dropped for lazy re-derivation"
        );
    }

    #[test]
    fn moving_real_obstacle_is_rate_limited() {
        let mut world = plaza();
        let from = vec3f(-5.0, 0.5, -8.0);
        let to = vec3f(-5.0, 0.5, 8.0);
        let _ = world.nav_line_clear(from, to);
        for tick in 1..=300u64 {
            world.entity_mut(2).unwrap().pos.x = -10.75 + (tick % 7) as f32 * 0.01;
            world.mark_render_dirty();
            world.tick += 1;
            let _ = world.nav_line_clear(from, to);
        }
        assert!(
            world.nav.derive_cells_runs() <= 22,
            "a real moving obstacle derived {} chunks in 300 ticks",
            world.nav.derive_cells_runs()
        );
    }

    #[test]
    fn flow_field_descends_to_target() {
        let mut world = plaza();
        let target = vec3f(0.0, 0.5, 8.0);
        let group = [vec3f(-6.0, 0.5, -8.0), vec3f(6.0, 0.5, -8.0)];
        let flow = world.nav_flow_field(target, &group).expect("field builds");
        // Walk a probe down the field from each group position; it must
        // reach the target cell.
        for start in group {
            let mut p = start;
            let mut cost_prev = u16::MAX;
            for _ in 0..2000 {
                let Some((dir, cost)) = flow.sample(p) else {
                    panic!("probe left the field at {p:?}");
                };
                if cost == 0 {
                    break;
                }
                assert!(cost <= cost_prev, "integration cost must not rise");
                cost_prev = cost;
                p = p + dir * (NAV_CELL * 0.9);
            }
            assert!(
                planar(p, target) < 2.0,
                "probe from {start:?} ended at {p:?}"
            );
        }
    }

    #[test]
    fn wedge_ramps_are_walkable_slopes() {
        let mut world = GameWorld::new();
        world.push_entity(solid(1, vec3f(0.0, -0.5, 0.0), vec3f(20.0, 0.5, 20.0)));
        // A ramp from ground (front, -z) up to 1.6 (back, +z), long enough
        // that the per-cell rise stays under NAV_EDGE_RISE.
        world.push_entity(Entity {
            id: 2,
            kind: BodyKind::Static,
            pos: vec3f(0.0, 0.8, 5.0),
            half: vec3f(2.0, 0.8, 3.0),
            collide: true,
            shape: Shape::Wedge,
            ..Default::default()
        });
        assert_eq!(
            world.nav_line_clear(vec3f(0.0, 0.5, 0.0), vec3f(0.0, 2.0, 7.0)),
            Some(true),
            "ramp reads as slope, not wall"
        );
    }
}

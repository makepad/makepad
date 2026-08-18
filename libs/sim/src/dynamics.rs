//! box3d dynamics layer (M1a, hybrid design — see game.md §Engine design).
//!
//! Movers keep the verbatim kinematic sweep in step.rs (tape parity); this
//! module owns everything box3d: statics/kinematics are MIRRORED into a box3d
//! world so the new `body:"rigid"` entities have something to collide with,
//! rigid bodies get full dynamics (stacking, impulses, rotation), and rigid
//! poses/velocities are read back into the Entity each tick.
//!
//! The mirror is RECONCILED against the entity list once per tick instead of
//! hooked into every spawn/remove path: `retain`-based removal, eval rollback
//! (WorldSnapshot restore) and reset all become automatically correct — a
//! body exists iff its entity exists. Per-rigid caches of the last read-back
//! (pos/orient/vel, compared bit-exactly) detect script writes like
//! game.set_pos/set_vel and push them INTO box3d, so the existing verb
//! surface works on rigids with no new dispatch arms (game.push is the one
//! exception: it applies a real impulse via [`RigidDynamics::rigid_impulse`]).
//!
//! Determinism: box3d is bit-exact cross-arch; the only transcendentals here
//! (yaw→quat) go through makepad-game-math. No wall clock, no libm.

use makepad_math::*;

use crate::entity::{push_mass_of, BodyKind, Entity, Shape};
use crate::terrain::{Terrain, TERRAIN_ID};
use crate::TICK_DT;

use makepad_box3d::body::*;
use makepad_box3d::height_field::create_height_field;
use makepad_box3d::hull::make_box_hull;
use makepad_box3d::id::{BodyId, ShapeId};
use makepad_box3d::mesh::create_mesh;
use makepad_box3d::math_functions::{
    pos as b3pos, vec3 as b3vec3, Pos as B3Pos, Quat as B3Quat, Transform, Vec3 as B3Vec3,
};
use makepad_box3d::mover::{clip_vector, solve_planes};
use makepad_box3d::physics_world::{
    create_world, world_cast_mover, world_cast_ray_closest, world_cast_shape, world_collide_mover,
    world_set_gravity, world_step, World,
};
use makepad_box3d::recording::{write_registry, RecBuffer, Recording};
use makepad_box3d::recording_replay::load_registry;
use makepad_box3d::shape::{
    create_capsule_shape, create_height_field_shape, create_hull_shape, create_mesh_shape,
    create_sphere_shape, shape_get_user_data,
};
use makepad_box3d::types::{
    default_body_def, default_shape_def, default_surface_material, default_world_def, BodyType,
    Capsule, CollisionPlane, Filter, HeightFieldDef, MeshDef, QueryFilter, ShapeProxy, Sphere,
    SurfaceMaterial,
};
use makepad_box3d::world_snapshot::{deserialize_into_shell, serialize_world};

/// Fixed solver sub-steps per tick (matches the xr engine's choice).
pub const SUBSTEPS: i32 = 4;

// ---------------------------------------------------------------- filtering
//
// Collision categories (D2/D4, mix.md). Filter bits never enter float math,
// so giving every shape an explicit category instead of box3d's all-bits
// default changes NOTHING about which existing pairs collide (world/rigid
// remain all-with-all) — the goldens stay bit-identical — while making room
// for the two new populations:
//
// - mover CAPSULES see rigids only (capsule-capsule is masked off), and the
//   capsule is a SENSOR shape: queries and casts hit it, but it is never a
//   physical wall. A car does not bounce off a pedestrian; the overlap
//   events become an impulse on the pedestrian instead (D4).
// - DECOR (`collide:false`, plus seat-attached movers) collides with nothing
//   and answers only to queries: `game.raycast` keeps its documented "hits
//   visually solid decor" contract without the decor ever touching the
//   solver. The continuous-collision pass also consults these masks, so a
//   fast rigid can never TOI-clip against decor or a capsule.

/// Solid statics, kinematics and the terrain heightfield.
pub const CAT_WORLD: u64 = 1 << 0;
/// Rigid bodies.
pub const CAT_RIGID: u64 = 1 << 1;
/// Mover capsules (D2 mirror).
pub const CAT_MOVER: u64 = 1 << 2;
/// Query-only ghosts: `collide:false` decor and seat-attached movers.
pub const CAT_DECOR: u64 = 1 << 3;
/// Category carried by QUERIES only — never by a shape. Its presence in a
/// shape's mask is what makes that shape ray-visible.
pub const CAT_QUERY: u64 = 1 << 62;

const SOLID_MASK: u64 = CAT_WORLD | CAT_RIGID | CAT_MOVER | CAT_QUERY;

fn filter_of(kind: MirrorKind) -> Filter {
    let (category_bits, mask_bits) = match kind {
        MirrorKind::Static | MirrorKind::Kinematic => (CAT_WORLD, SOLID_MASK),
        MirrorKind::Rigid => (CAT_RIGID, SOLID_MASK),
        MirrorKind::MoverCapsule => (CAT_MOVER, CAT_RIGID | CAT_QUERY),
        MirrorKind::Decor => (CAT_DECOR, CAT_QUERY),
    };
    Filter { category_bits, mask_bits, group_index: 0 }
}

/// Query filter for `game.raycast`: everything the old AABB march could hit —
/// world, rigids, movers (via capsules) and visually-solid decor. Sensors are
/// never mirrored, so they stay invisible to rays, as documented.
pub fn raycast_filter() -> QueryFilter {
    QueryFilter {
        category_bits: CAT_QUERY,
        mask_bits: CAT_WORLD | CAT_RIGID | CAT_MOVER | CAT_DECOR,
        id: 0,
        name: "game.raycast",
    }
}

/// Query filter for projectile CCD sweeps: solid things only. Decor never
/// blocked a projectile under the old sweep and still doesn't.
pub fn projectile_filter() -> QueryFilter {
    QueryFilter {
        category_bits: CAT_QUERY,
        mask_bits: CAT_WORLD | CAT_RIGID | CAT_MOVER,
        id: 0,
        name: "projectile_ccd",
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum MirrorKind {
    Static,
    Kinematic,
    Rigid,
    /// D2: a Mover's kinematic capsule. The mover sweep stays authoritative
    /// for mover MOTION; the capsule exists so exact queries and contact
    /// events can SEE the mover. Target-transformed each tick like Kinematic.
    MoverCapsule,
    /// Query-only static box: `collide:false` decor of any body kind, and
    /// seat-attached movers (their AABB rides inside a vehicle chassis — a
    /// live capsule there would spam disabled contacts against the owner's
    /// rigid every tick for nothing). Rays hit it; physics never does.
    Decor,
}

#[derive(Clone, Copy, Debug)]
struct MirrorEntry {
    entity_id: u64,
    body: BodyId,
    kind: MirrorKind,
    /// Last pose pushed to / read from box3d, compared BIT-exactly to detect
    /// outside writes (script verbs, rollback). For rigids these are the
    /// read-back values; for statics/kinematics the last mirrored pose.
    pos: Vec3f,
    yaw: f32,
    orient: Quat,
    vel: Vec3f,
    /// Rigid only: the read-back velocity of the PREVIOUS tick, so per-rigid
    /// acceleration is readable as (vel - prev_vel) / TICK_DT — the g-force
    /// feed for telemetry (F10). Statics/kinematics/capsules leave it zero.
    prev_vel: Vec3f,
}

/// One live capsule↔rigid overlap (F8), maintained from box3d's sensor
/// begin/end events. Plain data so world snapshots (Clone) carry the active
/// set verbatim — a rollback must replay the same impulses.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct MoverOverlap {
    pub mover_id: u64,
    pub rigid_id: u64,
}

/// One live voxel-chunk collider (T4): a static box3d triangle mesh built
/// from the chunk's render mesh — collision IS the visual, so a car drives
/// exactly what the player sees.
#[derive(Clone, Copy, Debug)]
struct VoxelBody {
    key: crate::voxel::ChunkKey,
    /// The [`crate::voxel::ChunkMesh::rev`] this collider was built from.
    rev: u64,
    body: BodyId,
}

pub struct RigidDynamics {
    pub world: World,
    /// Sorted by entity_id (entities are sorted by id; the merge-walk in
    /// reconcile preserves order) — deterministic iteration, O(log n) find.
    mirror: Vec<MirrorEntry>,
    terrain_rev: Option<u64>,
    terrain_body: Option<BodyId>,
    /// Voxel chunk colliders, sorted by chunk key (T4). Hot-swapped when a
    /// chunk's mesh revision moves: the replacement body is created BEFORE
    /// the stale one is destroyed, both inside one reconcile — no solver
    /// step ever runs without a collider under a resting body.
    voxel_bodies: Vec<VoxelBody>,
    last_gravity: f32,
    /// Live capsule↔rigid overlaps (D4's rigid-hits-mover signal), sorted.
    /// Updated from box3d sensor events after every step; consumed per tick
    /// by [`apply_mover_contacts`]. The sensor pass re-queries every sensor
    /// every step, so this cannot be starved by contact recycling or sleep —
    /// the reason capsules are SENSOR shapes rather than solid ones whose
    /// contacts a pre-solve callback disables.
    pub overlaps: Vec<MoverOverlap>,
}

impl Default for RigidDynamics {
    fn default() -> Self {
        Self::new()
    }
}

impl RigidDynamics {
    pub fn new() -> Self {
        let def = default_world_def();
        Self {
            world: create_world(&def),
            mirror: Vec::new(),
            terrain_rev: None,
            terrain_body: None,
            voxel_bodies: Vec::new(),
            // NAN forces the first reconcile to sync gravity.
            last_gravity: f32::NAN,
            overlaps: Vec::new(),
        }
    }

    /// Number of live mirrored bodies (tests/diagnostics).
    pub fn body_count(&self) -> usize {
        self.mirror.len() + self.terrain_body.iter().len() + self.voxel_bodies.len()
    }

    /// Live voxel-chunk colliders (tests/diagnostics).
    pub fn voxel_body_count(&self) -> usize {
        self.voxel_bodies.len()
    }

    /// box3d body backing a Rigid entity, once the mirror has reconciled it
    /// (None on the entity's spawn tick). The vehicle/plane blocks drive their
    /// chassis through this handle — forces go INTO box3d, and the pose comes
    /// back out through the normal read-back, so no block ever writes a pose
    /// the reconcile would have to fight over.
    pub fn rigid_body_of(&self, entity_id: u64) -> Option<BodyId> {
        let at = self.find(entity_id)?;
        (self.mirror[at].kind == MirrorKind::Rigid).then(|| self.mirror[at].body)
    }

    /// Adopt this tick's read-back as the baseline for `entity_id`, so a pose
    /// the caller wrote straight into box3d isn't seen as an outside write by
    /// the next reconcile (which would teleport the body back).
    pub fn sync_baseline(&mut self, entity_id: u64, pos: Vec3f, orient: Quat, vel: Vec3f) {
        if let Some(at) = self.find(entity_id) {
            self.mirror[at].pos = pos;
            self.mirror[at].orient = orient;
            self.mirror[at].vel = vel;
        }
    }

    /// Per-rigid acceleration over the last stepped tick, from the cached
    /// prev_vel: a = Δv / TICK_DT (F10). None until the body has been stepped
    /// twice — telemetry consumers treat that as zero. Includes gravity while
    /// falling (a resting body reads ~0; free fall reads ~(0,-g,0)): this is
    /// coordinate acceleration, and the OutSim-style g-force feed subtracts
    /// gravity itself where it wants proper acceleration.
    pub fn rigid_accel(&self, entity_id: u64) -> Option<Vec3f> {
        let at = self.find(entity_id)?;
        let entry = &self.mirror[at];
        (entry.kind == MirrorKind::Rigid)
            .then(|| (entry.vel - entry.prev_vel) * (1.0 / TICK_DT))
    }

    fn find(&self, entity_id: u64) -> Option<usize> {
        self.mirror
            .binary_search_by_key(&entity_id, |m| m.entity_id)
            .ok()
    }

    /// Velocity-add "push" as a real impulse (dv scaled by mass). Returns
    /// false if the entity has no rigid body yet (spawned this eval; the
    /// caller falls back to entity.vel, which seeds the body at creation).
    pub fn rigid_impulse(&mut self, entity_id: u64, dv: Vec3f) -> bool {
        let Some(at) = self.find(entity_id) else {
            return false;
        };
        let entry = self.mirror[at];
        if entry.kind != MirrorKind::Rigid {
            return false;
        }
        let mass = body_get_mass(&self.world, entry.body);
        body_apply_linear_impulse_to_center(
            &mut self.world,
            entry.body,
            b3vec3(dv.x * mass, dv.y * mass, dv.z * mass),
            true,
        );
        true
    }

    /// Angular kick for demos/verbs (rad/s impulse-ish, mass-scaled).
    pub fn rigid_spin(&mut self, entity_id: u64, axis_vel: Vec3f) -> bool {
        let Some(at) = self.find(entity_id) else {
            return false;
        };
        let entry = self.mirror[at];
        if entry.kind != MirrorKind::Rigid {
            return false;
        }
        let mass = body_get_mass(&self.world, entry.body);
        body_apply_angular_impulse(
            &mut self.world,
            entry.body,
            b3vec3(axis_vel.x * mass, axis_vel.y * mass, axis_vel.z * mass),
            true,
        );
        true
    }
}

/// What this entity mirrors as. Sensors have no mirror at all (rays skip
/// them — documented raycast contract). Anything `collide:false` mirrors as
/// query-only Decor so `game.raycast` still hits visually solid scenery.
/// Movers mirror as kinematic capsules (D2) — except seat-attached riders,
/// whose box rides inside a vehicle chassis: they demote to Decor so rays
/// still see them but the owner's rigid never grinds contacts against them.
fn mirror_kind(e: &Entity) -> Option<MirrorKind> {
    if e.sensor || e.non_interactive {
        return None;
    }
    if !e.collide {
        return Some(MirrorKind::Decor);
    }
    match e.kind {
        BodyKind::Static => Some(MirrorKind::Static),
        BodyKind::Kinematic => Some(MirrorKind::Kinematic),
        BodyKind::Rigid => Some(MirrorKind::Rigid),
        BodyKind::Mover => {
            if e.attached_to != 0 {
                Some(MirrorKind::Decor)
            } else {
                Some(MirrorKind::MoverCapsule)
            }
        }
    }
}

fn yaw_quat(yaw: f32) -> B3Quat {
    // game_math, not libm: this is sim state (game.md determinism rule).
    let (s, c) = crate::math::sincos(yaw * 0.5);
    B3Quat {
        v: b3vec3(0.0, s, 0.0),
        s: c,
    }
}

fn to_b3_quat(q: Quat) -> B3Quat {
    B3Quat {
        v: b3vec3(q.x, q.y, q.z),
        s: q.w,
    }
}

fn from_b3_quat(q: B3Quat) -> Quat {
    Quat {
        x: q.v.x,
        y: q.v.y,
        z: q.v.z,
        w: q.s,
    }
}

/// Clamp a dimension to something a solver can actually work with.
///
/// A kid's game (or a generated one) can hand us 0, a negative, a NaN or an
/// INFINITY. NaN is already absorbed by `max`, but infinity is not: it poisons
/// every plane normal to NaN, so box3d's face query never beats its
/// `-f32::MAX` starting separation, leaves `max_face_index` at the -1
/// sentinel, and that sentinel is cast through `u8` into 255 and used as an
/// array index. The port is faithful to upstream C here — C just reads garbage
/// where Rust panics — so the fix belongs at the boundary: never hand the
/// solver a value it cannot reason about.
fn sane_extent(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.01, 1.0e6)
    } else {
        0.01
    }
}

fn create_mirror_body(world: &mut World, e: &Entity, kind: MirrorKind) -> BodyId {
    let mut body_def = default_body_def();
    body_def.body_type = match kind {
        MirrorKind::Static | MirrorKind::Decor => BodyType::Static,
        MirrorKind::Kinematic | MirrorKind::MoverCapsule => BodyType::Kinematic,
        MirrorKind::Rigid => BodyType::Dynamic,
    };
    body_def.position = b3pos(e.pos.x, e.pos.y, e.pos.z);
    body_def.rotation = if kind == MirrorKind::Rigid && e.orient != Quat::default() {
        to_b3_quat(e.orient)
    } else {
        yaw_quat(e.yaw)
    };
    body_def.linear_velocity = b3vec3(e.vel.x, e.vel.y, e.vel.z);
    body_def.gravity_scale = e.gravity_scale;
    body_def.user_data = e.id;
    let body = create_body(world, &body_def);

    let mut shape_def = default_shape_def();
    shape_def.density = sane_extent(e.density);
    shape_def.base_material.friction = e.friction;
    shape_def.base_material.restitution = e.restitution;
    // Queryable surface type (F10): rays and contacts report this back, so a
    // tire model or footstep mixer can tell tarmac from grass per entity.
    shape_def.base_material.user_material_id = e.material as u64;
    shape_def.user_data = e.id;
    shape_def.filter = filter_of(kind);
    // Rigids announce themselves to the capsule sensors (a sensor only sees
    // visitor shapes that opted in). Event flag only — no solver effect.
    if kind == MirrorKind::Rigid {
        shape_def.enable_sensor_events = true;
    }
    match kind {
        // D2: the mover capsule. Radius covers the AABB footprint, clamped by
        // the half-height so squat movers degrade to a sphere-ish capsule
        // instead of a degenerate one. It is a SENSOR: queries and casts hit
        // it (box3d rays/casts do not skip sensors), rigids drive straight
        // through it — the overlap begin/end events are the D4
        // rigid-hits-mover signal, and the mover sweep stays the only
        // authority over mover motion.
        MirrorKind::MoverCapsule => {
            shape_def.is_sensor = true;
            shape_def.enable_sensor_events = true;
            let radius = sane_extent(e.half.x.max(e.half.z)).min(sane_extent(e.half.y));
            let stem = (sane_extent(e.half.y) - radius).max(0.0);
            let capsule = Capsule {
                center1: b3vec3(0.0, -stem, 0.0),
                center2: b3vec3(0.0, stem, 0.0),
                radius,
            };
            create_capsule_shape(world, body, &shape_def, &capsule);
        }
        // A `shape:"sphere"` rigid rolls like one (radius = half.x; the
        // visual may be an ellipsoid, collision is the sphere — documented).
        MirrorKind::Rigid if e.shape == Shape::Sphere => {
            let sphere = Sphere {
                center: b3vec3(0.0, 0.0, 0.0),
                radius: e.half.x.max(0.01),
            };
            create_sphere_shape(world, body, &shape_def, &sphere);
        }
        // A wedge mirrors as the triangular prism it draws as — the same
        // low-front-edge-to-high-back-edge slope `wedge_surface_at` walks —
        // so exact queries (raycast, the P3 capsule mover) meet the ramp
        // they can see instead of the invisible box that contains it. The
        // AABB mover sweep never consults box3d for statics, so the wedge
        // special-case there is untouched and the mover goldens cannot move.
        _ if e.shape == Shape::Wedge => {
            let (hx, hy, hz) = (
                sane_extent(e.half.x),
                sane_extent(e.half.y),
                sane_extent(e.half.z),
            );
            let points = [
                b3vec3(-hx, -hy, -hz),
                b3vec3(hx, -hy, -hz),
                b3vec3(-hx, -hy, hz),
                b3vec3(hx, -hy, hz),
                b3vec3(-hx, hy, hz),
                b3vec3(hx, hy, hz),
            ];
            match makepad_box3d::hull::create_hull(&points, points.len() as i32) {
                Some(hull) => {
                    create_hull_shape(world, body, &shape_def, &hull);
                }
                None => {
                    // Degenerate extents: fall back to the box.
                    let hull = make_box_hull(hx, hy, hz);
                    create_hull_shape(world, body, &shape_def, &hull);
                }
            }
        }
        _ => {
            // `max` already swallows NaN (it returns the non-NaN operand), but
            // INFINITY survives it and makes every plane computation NaN, which
            // leaves box3d's face query at its -1 sentinel — and that sentinel
            // is cast through u8 to 255 and used as an index. A generated game
            // that produced an infinite extent took the whole process down.
            let hull = make_box_hull(
                sane_extent(e.half.x),
                sane_extent(e.half.y),
                sane_extent(e.half.z),
            );
            create_hull_shape(world, body, &shape_def, &hull);
        }
    }
    body
}

/// Build one voxel chunk's static mesh collider from its render mesh.
/// Returns None for a mesh box3d rejects (degenerate) — the caller keeps
/// whatever collider it had.
fn create_voxel_body(
    world: &mut World,
    mesh: &crate::voxel::ChunkMesh,
) -> Option<BodyId> {
    let stride = crate::voxel::MESH_VERTEX_FLOATS;
    let mut vertices: Vec<B3Vec3> = Vec::with_capacity(mesh.verts.len() / stride);
    for v in mesh.verts.chunks_exact(stride) {
        vertices.push(b3vec3(v[0], v[1], v[2]));
    }
    let indices: Vec<i32> = mesh.indices.iter().map(|i| *i as i32).collect();
    let def = MeshDef {
        vertices: &vertices,
        indices: &indices,
        material_indices: &[],
        // The mesher already emits welded, seam-exact vertices; box3d's weld
        // pass folds the duplicated apron vertices so edge adjacency exists
        // and contacts roll smoothly across triangle borders.
        weld_tolerance: 1.0e-5,
        weld_vertices: true,
        use_median_split: true,
        identify_edges: true,
    };
    let data = create_mesh(&def, None)?;
    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Static;
    let body = create_body(world, &body_def);
    let mut shape_def = default_shape_def();
    shape_def.filter = filter_of(MirrorKind::Static);
    // Edited ground is still ground: rays and sweeps report the terrain
    // sentinel, so gameplay treats a dug floor exactly like the heightfield.
    shape_def.user_data = TERRAIN_ID;
    create_mesh_shape(world, body, &shape_def, &data, b3vec3(1.0, 1.0, 1.0));
    Some(body)
}

/// T4: diff the voxel field's mesh set against the live chunk colliders.
/// Merge-walk over two sorted sequences; create-then-destroy on revision
/// change so collision never lapses. Wheels, movers, projectiles and rays
/// see edited ground with zero per-consumer work from here on.
fn sync_voxel_colliders(dynamics: &mut RigidDynamics, voxel: Option<&crate::voxel::VoxelField>) {
    let empty = std::collections::BTreeMap::new();
    let meshes = voxel.map_or(&empty, |v| &v.meshes);
    if dynamics.voxel_bodies.is_empty() && meshes.is_empty() {
        return;
    }
    let old = std::mem::take(&mut dynamics.voxel_bodies);
    let mut out: Vec<VoxelBody> = Vec::with_capacity(meshes.len());
    let mut oi = 0;
    for (key, mesh) in meshes {
        while oi < old.len() && old[oi].key < *key {
            destroy_body(&mut dynamics.world, old[oi].body);
            oi += 1;
        }
        let have = (oi < old.len() && old[oi].key == *key).then(|| old[oi]);
        if have.is_some() {
            oi += 1;
        }
        match have {
            Some(entry) if entry.rev == mesh.rev => out.push(entry),
            Some(entry) => match create_voxel_body(&mut dynamics.world, mesh) {
                Some(body) => {
                    destroy_body(&mut dynamics.world, entry.body);
                    out.push(VoxelBody { key: *key, rev: mesh.rev, body });
                }
                // A rejected replacement keeps the stale collider: stale
                // ground beats no ground under a resting body.
                None => out.push(entry),
            },
            None => {
                if let Some(body) = create_voxel_body(&mut dynamics.world, mesh) {
                    out.push(VoxelBody { key: *key, rev: mesh.rev, body });
                }
            }
        }
    }
    while oi < old.len() {
        destroy_body(&mut dynamics.world, old[oi].body);
        oi += 1;
    }
    dynamics.voxel_bodies = out;
}

/// Diff the (sorted) entity list against the (sorted) mirror: create missing
/// bodies, destroy orphaned ones, propagate pose/velocity writes, sync
/// terrain + gravity + voxel chunk colliders. Runs once per tick before the
/// box3d step; cost is a merge walk, zero allocation in the steady state.
pub fn reconcile(
    dynamics: &mut RigidDynamics,
    entities: &[Entity],
    terrain: Option<&Terrain>,
    terrain_materials: Option<&crate::terrain::TerrainMaterials>,
    voxel: Option<&crate::voxel::VoxelField>,
    gravity: f32,
) {
    sync_voxel_colliders(dynamics, voxel);
    if gravity != dynamics.last_gravity {
        world_set_gravity(&mut dynamics.world, b3vec3(0.0, -gravity, 0.0));
        dynamics.last_gravity = gravity;
    }

    // Terrain heightfield mirror, rebuilt when the terrain revision moves.
    let want_rev = terrain.map(|t| t.revision);
    if want_rev != dynamics.terrain_rev {
        if let Some(body) = dynamics.terrain_body.take() {
            destroy_body(&mut dynamics.world, body);
        }
        if let Some(t) = terrain {
            let cells = t.cells as i32;
            let cell_count = ((cells - 1) * (cells - 1)).max(0) as usize;
            // REAL per-cell material indices (F10, fixes B6). Without an
            // authored table this is all-zeros + no material list — byte-for-
            // byte the pre-F10 heightfield, so existing goldens cannot move.
            let mut surfaces: Vec<SurfaceMaterial> = Vec::new();
            let materials: Vec<u8> = match terrain_materials {
                Some(tm) if !tm.surfaces.is_empty() => {
                    for (i, s) in tm.surfaces.iter().enumerate() {
                        let mut mat = default_surface_material();
                        mat.friction = s.friction;
                        mat.restitution = s.restitution;
                        mat.user_material_id = i as u64;
                        surfaces.push(mat);
                    }
                    let last = (tm.surfaces.len() - 1) as u8;
                    (0..cell_count)
                        .map(|i| {
                            let idx = tm.indices.get(i).copied().unwrap_or(0);
                            // 0xFF is box3d's hole value — pass it through.
                            if idx == 0xFF { idx } else { idx.min(last) }
                        })
                        .collect()
                }
                _ => vec![0u8; cell_count],
            };
            let mut min_h = f32::MAX;
            let mut max_h = f32::MIN;
            for h in &t.heights {
                min_h = min_h.min(*h);
                max_h = max_h.max(*h);
            }
            let def = HeightFieldDef {
                heights: &t.heights,
                material_indices: &materials,
                scale: b3vec3(t.cell_size, 1.0, t.cell_size),
                count_x: cells,
                count_z: cells,
                global_minimum_height: min_h - 1.0,
                global_maximum_height: max_h + 1.0,
                clockwise_winding: false,
            };
            let data = create_height_field(&def);
            let mut body_def = default_body_def();
            body_def.body_type = BodyType::Static;
            body_def.position = b3pos(t.origin, 0.0, t.origin);
            let body = create_body(&mut dynamics.world, &body_def);
            let mut shape_def = default_shape_def();
            shape_def.filter = filter_of(MirrorKind::Static);
            // Casts resolve hit shapes to entity ids through user_data; the
            // terrain reports the same sentinel the sweep contract uses.
            shape_def.user_data = TERRAIN_ID;
            // Per-triangle materials the cell indices point into (empty =
            // base material only, the pre-F10 behavior).
            shape_def.materials = surfaces;
            create_height_field_shape(&mut dynamics.world, body, &shape_def, &data);
            dynamics.terrain_body = Some(body);
        }
        dynamics.terrain_rev = want_rev;
    }

    // Merge-walk entities (sorted by id) against mirror (sorted by id).
    let mut out: Vec<MirrorEntry> = Vec::with_capacity(dynamics.mirror.len());
    let mut mi = 0;
    for e in entities {
        // Drop mirror entries whose entities are gone (ids below e.id).
        while mi < dynamics.mirror.len() && dynamics.mirror[mi].entity_id < e.id {
            destroy_body(&mut dynamics.world, dynamics.mirror[mi].body);
            mi += 1;
        }
        let want = mirror_kind(e);
        let have = (mi < dynamics.mirror.len() && dynamics.mirror[mi].entity_id == e.id)
            .then(|| dynamics.mirror[mi]);
        if have.is_some() {
            mi += 1;
        }
        match (have, want) {
            (None, None) => {}
            (Some(entry), None) => {
                destroy_body(&mut dynamics.world, entry.body);
            }
            (None, Some(kind)) => {
                let body = create_mirror_body(&mut dynamics.world, e, kind);
                out.push(MirrorEntry {
                    entity_id: e.id,
                    body,
                    kind,
                    pos: e.pos,
                    yaw: e.yaw,
                    orient: e.orient,
                    vel: e.vel,
                    prev_vel: e.vel,
                });
            }
            (Some(mut entry), Some(kind)) => {
                if entry.kind != kind {
                    // Kind flips DO happen now: a mover demotes to Decor when
                    // it takes a seat (attached_to) and back on dismount; and
                    // rollback could restore an older entity under a reused
                    // id. Destroy + recreate keeps every flip correct.
                    destroy_body(&mut dynamics.world, entry.body);
                    let body = create_mirror_body(&mut dynamics.world, e, kind);
                    entry = MirrorEntry {
                        entity_id: e.id,
                        body,
                        kind,
                        pos: e.pos,
                        yaw: e.yaw,
                        orient: e.orient,
                        vel: e.vel,
                        prev_vel: e.vel,
                    };
                    out.push(entry);
                    continue;
                }
                match kind {
                    MirrorKind::Static | MirrorKind::Decor => {
                        // set_pos/face on placed geometry: teleport the body.
                        // Decor rides the same arm — a carried rider's ghost
                        // box teleports along each tick.
                        if e.pos != entry.pos || e.yaw != entry.yaw {
                            body_set_transform(
                                &mut dynamics.world,
                                entry.body,
                                b3pos(e.pos.x, e.pos.y, e.pos.z),
                                yaw_quat(e.yaw),
                            );
                            entry.pos = e.pos;
                            entry.yaw = e.yaw;
                        }
                    }
                    MirrorKind::Kinematic => {
                        // Platforms: target-transform so box3d derives the
                        // velocity rigids resting on top should inherit.
                        if e.pos != entry.pos || e.yaw != entry.yaw {
                            body_set_target_transform(
                                &mut dynamics.world,
                                entry.body,
                                Transform {
                                    p: b3vec3(e.pos.x, e.pos.y, e.pos.z),
                                    q: yaw_quat(e.yaw),
                                },
                                TICK_DT,
                                true,
                            );
                            entry.pos = e.pos;
                            entry.yaw = e.yaw;
                        }
                    }
                    MirrorKind::MoverCapsule => {
                        // Same target-transform pattern (D2), but issued
                        // EVERY tick: a target sets a kinematic velocity that
                        // persists after arrival, so a mover that stops
                        // walking would otherwise leave its capsule drifting
                        // off at the last walk speed. Re-targeting the same
                        // pose derives a zero velocity and pins the capsule
                        // where the mover stands — which is exactly what the
                        // queries and contact events must see.
                        body_set_target_transform(
                            &mut dynamics.world,
                            entry.body,
                            Transform {
                                p: b3vec3(e.pos.x, e.pos.y, e.pos.z),
                                q: yaw_quat(e.yaw),
                            },
                            TICK_DT,
                            true,
                        );
                        entry.pos = e.pos;
                        entry.yaw = e.yaw;
                    }
                    MirrorKind::Rigid => {
                        // Bit-compare against the last read-back: any
                        // difference means script/rollback wrote the entity —
                        // entity is authoritative, push into box3d.
                        let moved = e.pos != entry.pos || e.orient != entry.orient;
                        let kicked = e.vel != entry.vel;
                        if moved {
                            body_set_transform(
                                &mut dynamics.world,
                                entry.body,
                                b3pos(e.pos.x, e.pos.y, e.pos.z),
                                if e.orient != Quat::default() {
                                    to_b3_quat(e.orient)
                                } else {
                                    yaw_quat(e.yaw)
                                },
                            );
                            entry.pos = e.pos;
                            entry.orient = e.orient;
                        }
                        if kicked {
                            body_set_linear_velocity(
                                &mut dynamics.world,
                                entry.body,
                                b3vec3(e.vel.x, e.vel.y, e.vel.z),
                            );
                            if moved {
                                // A teleport (set_pos zeroes vel host-side)
                                // also stops any tumble.
                                body_set_angular_velocity(
                                    &mut dynamics.world,
                                    entry.body,
                                    b3vec3(0.0, 0.0, 0.0),
                                );
                            }
                            entry.vel = e.vel;
                        }
                    }
                }
                out.push(entry);
            }
        }
    }
    // Anything left in the mirror lost its entity.
    while mi < dynamics.mirror.len() {
        destroy_body(&mut dynamics.world, dynamics.mirror[mi].body);
        mi += 1;
    }
    dynamics.mirror = out;
}

/// Step box3d and read rigid poses/velocities back into the entities.
pub fn step_dynamics(dynamics: &mut RigidDynamics, entities: &mut [Entity]) {
    // Statics never need a step; anything solver-movable does. Rigids for the
    // dynamics themselves; kinematics and mover capsules so their pending
    // target transforms are consumed and queries see CURRENT poses (a ray
    // must find the walker where it stands, not where it spawned — F6/F7).
    // A purely static world (menus, dioramas) still skips the step, and
    // box3d never writes back to non-rigids, so mover-only worlds keep their
    // entity state bit-identical to the pre-capsule engine.
    if !dynamics.mirror.iter().any(|m| {
        matches!(
            m.kind,
            MirrorKind::Rigid | MirrorKind::Kinematic | MirrorKind::MoverCapsule
        )
    }) {
        return;
    }
    world_step(&mut dynamics.world, TICK_DT, SUBSTEPS);
    for entry in dynamics.mirror.iter_mut() {
        if entry.kind != MirrorKind::Rigid {
            continue;
        }
        let Some(at) = crate::world::entity_index_sorted(entities, entry.entity_id) else {
            continue;
        };
        let p = body_get_position(&dynamics.world, entry.body);
        let q = body_get_rotation(&dynamics.world, entry.body);
        let v = body_get_linear_velocity(&dynamics.world, entry.body);
        let e = &mut entities[at];
        e.pos = vec3f(p.x, p.y, p.z);
        e.orient = from_b3_quat(q);
        e.vel = vec3f(v.x, v.y, v.z);
        entry.pos = e.pos;
        entry.orient = e.orient;
        entry.prev_vel = entry.vel;
        entry.vel = e.vel;
    }
    update_overlaps(dynamics);
}

/// Rebuild the live-overlap set from the sensors' CURRENT overlap arrays
/// (each capsule sensor re-queried the world this step). Rebuilt rather than
/// folded from begin/end events on purpose: the event arrays are double-
/// buffered across steps and that buffering is not part of the world
/// snapshot, so an event-driven set could drop an `end` after a Clone and
/// diverge — the overlap arrays themselves ARE snapshotted, making this
/// derivation rollback-exact. Only capsule↔rigid pairs exist (the capsule's
/// filter mask admits rigids alone); stale visitors are dropped by the
/// generation check. Sorted for deterministic iteration.
fn update_overlaps(dynamics: &mut RigidDynamics) {
    dynamics.overlaps.clear();
    let world = &dynamics.world;
    for sensor in &world.sensors {
        let sensor_shape = &world.shapes[sensor.shape_id as usize];
        let mover_id = sensor_shape.user_data;
        for v in &sensor.overlaps2 {
            let Some(shape) = world.shapes.get(v.shape_id as usize) else {
                continue;
            };
            if shape.generation != v.generation {
                continue; // the visitor shape died and its slot was reused
            }
            dynamics.overlaps.push(MoverOverlap {
                mover_id,
                rigid_id: shape.user_data,
            });
        }
    }
    dynamics.overlaps.sort_unstable();
    dynamics.overlaps.dedup();
}

// ---------------------------------------------------------- exact queries

/// One exact query hit: entity id ([`TERRAIN_ID`] for terrain), world point,
/// true surface normal, distance along the ray, and the surface's
/// `user_material_id` (per-entity `material`, per-cell terrain material).
#[derive(Clone, Copy, Debug)]
pub struct QueryHit {
    pub id: u64,
    pub pos: Vec3f,
    pub normal: Vec3f,
    pub dist: f32,
    pub material: u64,
}

/// F7: the exact world raycast, replacing the 0.15-step AABB march. Runs
/// against the box3d mirror, so it sees statics, kinematics, rigids, the
/// terrain heightfield, MOVERS (as their D2 capsules — a strafing player is
/// hittable now) and `collide:false` decor (mirrored as query-only shapes),
/// with true surface normals and exact distances. Sensors are not mirrored
/// and stay invisible, as documented.
///
/// Contract changes from the march, both deliberate: normals/distances are
/// exact instead of quantized to the step, and a ray STARTING inside a shape
/// no longer reports that shape (the march reported the caster itself; eye
/// rays kept having to skip their own id — now they don't hit it at all).
///
/// The caller is responsible for the mirror being current — reconcile before
/// querying ([`crate::world::GameWorld::sync_queries`] does exactly that).
pub fn cast_ray(dynamics: &RigidDynamics, from: Vec3f, dir: Vec3f, max: f32) -> Option<QueryHit> {
    if !(from.x.is_finite() && from.y.is_finite() && from.z.is_finite()) {
        return None;
    }
    if !(dir.x.is_finite() && dir.y.is_finite() && dir.z.is_finite()) {
        return None;
    }
    if !max.is_finite() || max <= 0.0 {
        return None;
    }
    let len = crate::vec3_len(dir);
    if len <= 1.0e-6 {
        return None;
    }
    let dir = dir * (1.0 / len);
    let result = world_cast_ray_closest(
        &dynamics.world,
        b3pos(from.x, from.y, from.z),
        b3vec3(dir.x * max, dir.y * max, dir.z * max),
        raycast_filter(),
    );
    if !result.hit {
        return None;
    }
    Some(QueryHit {
        id: shape_get_user_data(&dynamics.world, result.shape_id),
        pos: vec3f(
            result.point.x as f32,
            result.point.y as f32,
            result.point.z as f32,
        ),
        normal: vec3f(result.normal.x, result.normal.y, result.normal.z),
        dist: result.fraction * max,
        material: result.user_material_id,
    })
}

/// F9: exact swept-box cast for a fast projectile, start→end of this tick's
/// motion. Returns the clamped stop position (a skin short of the surface)
/// and the id of what it hit ([`TERRAIN_ID`] for ground). Skips the caster,
/// every other projectile (`hits` entities pass through their own kind, the
/// documented overlap-IS-the-hit contract) and anything already overlapped
/// at the start (fraction 0 — a bullet spawned inside its shooter must fly
/// OUT, not detonate in the barrel). Decor is filtered out by mask: it never
/// blocked a projectile under the axis sweeps and still doesn't.
pub fn projectile_ccd(
    dynamics: &RigidDynamics,
    projectile_ids: &[u64],
    self_id: u64,
    start: Vec3f,
    end: Vec3f,
    half: Vec3f,
) -> Option<(Vec3f, u64)> {
    let delta = end - start;
    let len = crate::vec3_len(delta);
    if len <= 1.0e-6 || !len.is_finite() {
        return None;
    }
    let (hx, hy, hz) = (
        sane_extent(half.x),
        sane_extent(half.y),
        sane_extent(half.z),
    );
    let corners = [
        b3vec3(-hx, -hy, -hz),
        b3vec3(hx, -hy, -hz),
        b3vec3(-hx, hy, -hz),
        b3vec3(hx, hy, -hz),
        b3vec3(-hx, -hy, hz),
        b3vec3(hx, -hy, hz),
        b3vec3(-hx, hy, hz),
        b3vec3(hx, hy, hz),
    ];
    let proxy = ShapeProxy {
        points: &corners,
        radius: 0.0,
    };
    let mut best: Option<(f32, u64)> = None;
    {
        let world = &dynamics.world;
        let mut callback = |shape_id: ShapeId,
                            _point: B3Pos,
                            _normal: B3Vec3,
                            fraction: f32,
                            _material: u64,
                            _tri: i32,
                            _child: i32|
         -> f32 {
            if fraction <= 0.0 {
                return -1.0;
            }
            let id = shape_get_user_data(world, shape_id);
            if id == self_id || projectile_ids.binary_search(&id).is_ok() {
                return -1.0;
            }
            best = Some((fraction, id));
            fraction
        };
        world_cast_shape(
            world,
            b3pos(start.x, start.y, start.z),
            &proxy,
            b3vec3(delta.x, delta.y, delta.z),
            projectile_filter(),
            &mut callback,
        );
    }
    best.map(|(fraction, id)| {
        // Stop a skin short of flush, the same margin the sweeps leave.
        let back = (fraction - crate::queries::CONTACT_SKIN / len).max(0.0);
        (start + delta * back, id)
    })
}

// ------------------------------------------------------ capsule mover (P3)

/// Query filter for the opt-in capsule mover: the world and rigids block it;
/// other movers' sensor capsules never do (movers pass through movers — the
/// documented contract) and decor stays walk-through, exactly as it is for
/// the AABB sweep.
fn capsule_mover_filter() -> QueryFilter {
    QueryFilter {
        category_bits: CAT_QUERY,
        mask_bits: CAT_WORLD | CAT_RIGID,
        id: 0,
        name: "capsule_mover",
    }
}

/// The mover capsule the flag routes through — the same dimensions as the D2
/// mirror capsule, so what a raycast sees of this mover and what it collides
/// as are one shape.
fn mover_capsule(half: Vec3f) -> Capsule {
    let radius = sane_extent(half.x.max(half.z)).min(sane_extent(half.y));
    let stem = (sane_extent(half.y) - radius).max(0.0);
    Capsule {
        center1: b3vec3(0.0, -stem, 0.0),
        center2: b3vec3(0.0, stem, 0.0),
        radius,
    }
}

/// Fixed plane-solver iterations per tick — fixed, not convergence-gated, so
/// the tick cost and the float path are the same on every machine. (The
/// early-out on a sub-micron delta below is a deterministic float compare.)
const CAPSULE_ITERATIONS: usize = 4;
/// Plane budget per tick. 16 covers a capsule wedged in a corner on stairs;
/// beyond that the nearest planes win (broad-phase order, deterministic).
const CAPSULE_MAX_PLANES: usize = 16;
/// A contact plane steeper than this (normal.y) is a floor; between +/- it
/// is a wall. cos(53°) — the same walkable band character slopes use.
const CAPSULE_FLOOR_NY: f32 = 0.6;

/// P3: one tick of motion for a `capsule_collider` mover, replacing the
/// axis-separated AABB sweep wholesale. The box3d character-mover primitives
/// do the work: `world_collide_mover` gathers contact planes at the current
/// pose, `solve_planes` finds the translation that satisfies them (this is
/// what makes an angled wall a SLIDE instead of a dead stop), and
/// `world_cast_mover` clamps each sub-move so a fast capsule cannot tunnel.
/// Velocity is then clipped against the touching planes, and the contact
/// set is classified into the same fields the sweep reports — `on_floor`,
/// `floor_id`, `floor_normal`, `hit_wall`, `wall_normal` — so every consumer
/// (character block, brains, scripts) works unchanged on either path.
///
/// Runs against the box3d mirror at its end-of-last-tick poses — the same
/// ordering contract as the sweep's pre-integration snapshot. Movers pressed
/// into rigids record a [`SweepPush`] like the sweep does; the caller applies
/// them after the loop.
pub fn capsule_mover_step(
    dynamics: &RigidDynamics,
    e: &mut Entity,
    pushes: &mut Vec<SweepPush>,
) {
    let capsule = mover_capsule(e.half);
    // The velocity the mover CARRIED into its contacts this tick, read before
    // clipping: the shove a rigid takes is the press, not the remainder.
    let start_vel = e.vel;
    let mut pos = e.pos;
    let target = pos + e.vel * TICK_DT;
    let mut plane_ids: Vec<u64> = Vec::with_capacity(CAPSULE_MAX_PLANES);
    let mut planes: Vec<CollisionPlane> = Vec::with_capacity(CAPSULE_MAX_PLANES);

    for _ in 0..CAPSULE_ITERATIONS {
        plane_ids.clear();
        planes.clear();
        {
            let world = &dynamics.world;
            let plane_ids = &mut plane_ids;
            let planes = &mut planes;
            world_collide_mover(
                world,
                b3pos(pos.x, pos.y, pos.z),
                &capsule,
                capsule_mover_filter(),
                &mut |shape_id, results| {
                    for r in results {
                        if planes.len() < CAPSULE_MAX_PLANES {
                            plane_ids.push(shape_get_user_data(world, shape_id));
                            planes.push(CollisionPlane {
                                plane: r.plane,
                                // Statics and rigids are equally rigid to the
                                // capsule; the rigid's give comes from the
                                // impulse below, not from the solver.
                                push_limit: f32::MAX,
                                push: 0.0,
                                clip_velocity: true,
                            });
                        }
                    }
                    true
                },
            );
        }
        let want = target - pos;
        let solved = solve_planes(b3vec3(want.x, want.y, want.z), &mut planes);
        let fraction = world_cast_mover(
            &dynamics.world,
            b3pos(pos.x, pos.y, pos.z),
            &capsule,
            solved.delta,
            capsule_mover_filter(),
            None,
        );
        let delta = vec3f(solved.delta.x, solved.delta.y, solved.delta.z) * fraction;
        pos = pos + delta;
        if delta.x * delta.x + delta.y * delta.y + delta.z * delta.z < 1.0e-10 {
            break;
        }
    }
    e.pos = pos;

    // Kill the velocity components the touching planes disallow. THIS is the
    // wall slide: a diagonal walk into an angled wall keeps its tangential
    // speed instead of the axis sweep's hard zero.
    let clipped = clip_vector(b3vec3(e.vel.x, e.vel.y, e.vel.z), &planes);
    e.vel = vec3f(clipped.x, clipped.y, clipped.z);

    // Classify the final contact set into the sweep's own contract fields.
    // Only planes the solver actually pushed against count — a wall being
    // slid ALONG (push 0) is not a wall being pressed INTO, matching the
    // sweep's "hit only when clamped" behavior.
    e.on_floor = false;
    e.floor_id = 0;
    e.floor_normal = vec3f(0.0, 0.0, 0.0);
    e.hit_wall = 0;
    e.wall_normal = vec3f(0.0, 0.0, 0.0);
    let mut best_floor = 0.0f32;
    for (i, p) in planes.iter().enumerate() {
        if p.push <= 0.0 {
            continue;
        }
        let n = vec3f(p.plane.normal.x, p.plane.normal.y, p.plane.normal.z);
        let id = plane_ids[i];
        if n.y >= CAPSULE_FLOOR_NY {
            if e.vel.y <= 0.0 && n.y > best_floor {
                best_floor = n.y;
                e.on_floor = true;
                // Terrain floors report floor_id 0, like the sweep.
                e.floor_id = if id == TERRAIN_ID { 0 } else { id };
                e.floor_normal = n;
            }
        } else if n.y > -CAPSULE_FLOOR_NY {
            if e.hit_wall == 0 {
                e.hit_wall = id;
                e.wall_normal = n;
            }
            // Mover pressed into a rigid: shove it (D4's mover-pushes-rigid
            // half; `apply_sweep_pushes` ignores non-rigid ids).
            let speed = -(start_vel.x * n.x + start_vel.y * n.y + start_vel.z * n.z);
            if speed > CONTACT_MIN_APPROACH {
                pushes.push(SweepPush {
                    rigid_id: id,
                    dir: vec3f(-n.x, -n.y, -n.z),
                    speed,
                    mover_mass: push_mass_of(e),
                });
            }
        }
    }
}

// ------------------------------------------------------- contact impulses

/// A rigid closing on a mover slower than this (after mass scaling) is
/// resting contact, not a hit — no impulse, no knockdown.
pub const CONTACT_MIN_APPROACH: f32 = 0.05;
/// Mass-scaled hit speed at which a mover is knocked off its feet.
pub const KNOCKDOWN_MIN_SPEED: f32 = 3.0;
/// Knockdown duration scaling: ticks of `knocked_down` per m/s of kick.
pub const KNOCKDOWN_TICKS_PER_SPEED: f32 = 8.0;
pub const KNOCKDOWN_MIN_TICKS: u16 = 30;
/// Also the wire cap: `knocked_down` replicates in 8 bits (see the session
/// crate's state_flags), so this must stay below 256.
pub const KNOCKDOWN_MAX_TICKS: u16 = 240;
/// Vertical pop added to a knockdown kick — "sent flying", not "slid away".
pub const KNOCKDOWN_POP: f32 = 0.3;
/// Damping on the mover→rigid counter-push. Deliberately below the friction
/// a grounded rigid sees per tick at ~car mass, so a walker shoulders a
/// loose crate around but cannot bulldoze a car (D4: "pushes LIGHT ones").
pub const MOVER_PUSH_SCALE: f32 = 0.4;

/// D4's rigid-hits-mover half (F8): turn the LIVE capsule↔rigid overlaps
/// into gameplay. Called at the TOP of step_world, before the sweeps, so an
/// impulse lands in the same tick's integration and a knocked walker leaves
/// the ground immediately.
///
/// Per overlapping pair, per tick: if the rigid is closing on the mover it
/// kicks the mover along the center-to-center direction, mass-ratio scaled
/// (`m_r/(m_r+m_m)`) — a car at speed sends a walker flying (plus vertical
/// pop and a `knocked_down` timer); a drifting crate barely nudges. The
/// kick is proportional to the REMAINING closing speed, so a walker already
/// flying away at car speed takes nothing further — self-limiting, no
/// re-boost loop.
///
/// The direction is the horizontal-dominant center-to-center vector rather
/// than a solver manifold normal — deliberate: the capsule is a sensor (no
/// manifolds exist), and for "what way does the victim fly", center-derived
/// is exactly the arcade answer. The mover→rigid half (walker shoves a
/// crate) lives in the SWEEP, which is the authority on mover contact — see
/// [`apply_sweep_pushes`].
///
/// Mover mass is [`push_mass_of`] — the same unit rigid masses land in
/// (density × volume): a default walker weighs like a unit crate.
pub fn apply_mover_contacts(dynamics: &mut RigidDynamics, entities: &mut [Entity]) {
    if dynamics.overlaps.is_empty() {
        return;
    }
    // Prune pairs whose entities are gone (removed between steps) — the
    // sensor's end event may reference destroyed shapes and is skipped, so
    // this is where such pairs actually die.
    let mut overlaps = std::mem::take(&mut dynamics.overlaps);
    overlaps.retain(|c| {
        crate::world::entity_index_sorted(entities, c.mover_id).is_some()
            && crate::world::entity_index_sorted(entities, c.rigid_id).is_some()
    });
    for c in &overlaps {
        let Some(mover_at) = crate::world::entity_index_sorted(entities, c.mover_id) else {
            continue;
        };
        let Some(rigid_at) = crate::world::entity_index_sorted(entities, c.rigid_id) else {
            continue;
        };
        let Some(rigid_body) = dynamics.rigid_body_of(c.rigid_id) else {
            continue;
        };
        // Direction from the rigid's center toward the mover's.
        let d = entities[mover_at].pos - entities[rigid_at].pos;
        let len = crate::vec3_len(d);
        let n = if len > 1.0e-4 {
            d * (1.0 / len)
        } else {
            vec3f(0.0, 1.0, 0.0)
        };
        let v_m = entities[mover_at].vel;
        let v_r = entities[rigid_at].vel;
        let m_m = push_mass_of(&entities[mover_at]);
        let m_r = body_get_mass(&dynamics.world, rigid_body).max(1.0e-3);

        // Only when the rigid itself is driving in.
        let rigid_inbound = v_r.x * n.x + v_r.y * n.y + v_r.z * n.z;
        if rigid_inbound <= CONTACT_MIN_APPROACH {
            continue;
        }
        let closing = ((v_r.x - v_m.x) * n.x + (v_r.y - v_m.y) * n.y + (v_r.z - v_m.z) * n.z)
            .max(0.0);
        let kick = closing * (m_r / (m_m + m_r));
        if kick > CONTACT_MIN_APPROACH {
            let e = &mut entities[mover_at];
            e.vel = e.vel + n * kick;
            if kick > KNOCKDOWN_MIN_SPEED {
                e.vel.y += kick * KNOCKDOWN_POP;
                let ticks =
                    (kick * KNOCKDOWN_TICKS_PER_SPEED).min(KNOCKDOWN_MAX_TICKS as f32) as u16;
                e.knocked_down = e.knocked_down.max(ticks.max(KNOCKDOWN_MIN_TICKS));
            }
        }
    }
    dynamics.overlaps = overlaps;
}

/// One mover-pressed-into-a-rigid contact reported by the axis sweep this
/// tick (the sweep is the authority on mover collision — it knows the exact
/// blocked axis and the speed it zeroed).
#[derive(Clone, Copy, Debug)]
pub struct SweepPush {
    pub rigid_id: u64,
    /// Push direction: the blocked sweep axis, signed toward the rigid.
    pub dir: Vec3f,
    /// The mover's speed into the rigid at the moment the sweep clamped it.
    pub speed: f32,
    /// [`push_mass_of`] the pressing mover.
    pub mover_mass: f32,
}

/// D4's mover-pushes-rigid half (F8): walkers shove LIGHT rigids around.
/// Fed by step_world from the axis sweeps (a mover whose x/z sweep clamped
/// against a Rigid), applied as real impulses, mass-ratio scaled and damped
/// by [`MOVER_PUSH_SCALE`] so a walker trundles a loose crate along but
/// cannot bulldoze a car against its ground friction. The pressing mover
/// itself is NOT bounced back — its blocking is the sweep's own job, and an
/// equal-and-opposite would make leaning on a parked car vibrate you away.
pub fn apply_sweep_pushes(dynamics: &mut RigidDynamics, pushes: &[SweepPush]) {
    for p in pushes {
        let Some(body) = dynamics.rigid_body_of(p.rigid_id) else {
            continue;
        };
        let m_r = body_get_mass(&dynamics.world, body).max(1.0e-3);
        let shove = p.speed * (p.mover_mass / (p.mover_mass + m_r)) * MOVER_PUSH_SCALE;
        if shove > 1.0e-4 {
            dynamics.rigid_impulse(p.rigid_id, p.dir * shove);
        }
    }
}

impl Clone for RigidDynamics {
    /// Bit-exact clone via the box3d snapshot round trip (its own test suite
    /// proves restored worlds continue bit-identically). BodyIds are stable
    /// across the round trip, so the mirror table clones verbatim.
    fn clone(&self) -> Self {
        let mut world = create_world(&default_world_def());
        if !self.mirror.is_empty() || self.terrain_body.is_some() || !self.voxel_bodies.is_empty() {
            let mut buf = RecBuffer::new();
            let mut rec = Recording::new();
            let bytes = serialize_world(&self.world, &mut buf, &mut rec);
            debug_assert!(bytes > 0, "box3d snapshot serialize failed");
            write_registry(&mut rec);
            if let Some(reader) = load_registry(&rec.buffer.data) {
                let ok = deserialize_into_shell(&buf.data, &mut world, &reader);
                debug_assert!(ok, "box3d snapshot restore failed");
            }
        }
        if !self.last_gravity.is_nan() {
            world_set_gravity(&mut world, b3vec3(0.0, -self.last_gravity, 0.0));
        }
        Self {
            world,
            mirror: self.mirror.clone(),
            terrain_rev: self.terrain_rev,
            terrain_body: self.terrain_body,
            // BodyIds are stable across the snapshot round trip, and mesh
            // shapes are interned by the recording — chunk colliders clone
            // with the world.
            voxel_bodies: self.voxel_bodies.clone(),
            last_gravity: self.last_gravity,
            // The live overlap set is world state: a rolled-back world must
            // replay the same impulses.
            overlaps: self.overlaps.clone(),
        }
    }
}

impl std::fmt::Debug for RigidDynamics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RigidDynamics")
            .field("bodies", &self.body_count())
            .field("terrain", &self.terrain_rev)
            .finish()
    }
}

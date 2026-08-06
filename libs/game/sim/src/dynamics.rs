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

use crate::entity::{BodyKind, Entity, Shape};
use crate::terrain::Terrain;
use crate::TICK_DT;

use makepad_box3d::body::*;
use makepad_box3d::height_field::create_height_field;
use makepad_box3d::hull::make_box_hull;
use makepad_box3d::id::BodyId;
use makepad_box3d::math_functions::{pos as b3pos, vec3 as b3vec3, Quat as B3Quat, Transform};
use makepad_box3d::physics_world::{create_world, world_set_gravity, world_step, World};
use makepad_box3d::recording::{write_registry, RecBuffer, Recording};
use makepad_box3d::recording_replay::load_registry;
use makepad_box3d::shape::{
    create_height_field_shape, create_hull_shape, create_sphere_shape,
};
use makepad_box3d::types::{
    default_body_def, default_shape_def, default_world_def, BodyType, HeightFieldDef, Sphere,
};
use makepad_box3d::world_snapshot::{deserialize_into_shell, serialize_world};

/// Fixed solver sub-steps per tick (matches the xr engine's choice).
pub const SUBSTEPS: i32 = 4;

#[derive(Clone, Copy, PartialEq, Debug)]
enum MirrorKind {
    Static,
    Kinematic,
    Rigid,
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
}

pub struct RigidDynamics {
    pub world: World,
    /// Sorted by entity_id (entities are sorted by id; the merge-walk in
    /// reconcile preserves order) — deterministic iteration, O(log n) find.
    mirror: Vec<MirrorEntry>,
    terrain_rev: Option<u64>,
    terrain_body: Option<BodyId>,
    last_gravity: f32,
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
            // NAN forces the first reconcile to sync gravity.
            last_gravity: f32::NAN,
        }
    }

    /// Number of live mirrored bodies (tests/diagnostics).
    pub fn body_count(&self) -> usize {
        self.mirror.len() + self.terrain_body.iter().len()
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

/// What (if anything) this entity mirrors as. Sensors and `collide:false`
/// decor have no physical presence — same rule the mover sweep uses.
fn mirror_kind(e: &Entity) -> Option<MirrorKind> {
    if e.sensor || !e.collide {
        return None;
    }
    match e.kind {
        BodyKind::Static => Some(MirrorKind::Static),
        BodyKind::Kinematic => Some(MirrorKind::Kinematic),
        BodyKind::Rigid => Some(MirrorKind::Rigid),
        BodyKind::Mover => None,
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
        MirrorKind::Static => BodyType::Static,
        MirrorKind::Kinematic => BodyType::Kinematic,
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
    shape_def.user_data = e.id;
    match e.shape {
        // A `shape:"sphere"` rigid rolls like one (radius = half.x; the
        // visual may be an ellipsoid, collision is the sphere — documented).
        Shape::Sphere if kind == MirrorKind::Rigid => {
            let sphere = Sphere {
                center: b3vec3(0.0, 0.0, 0.0),
                radius: e.half.x.max(0.01),
            };
            create_sphere_shape(world, body, &shape_def, &sphere);
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

/// Diff the (sorted) entity list against the (sorted) mirror: create missing
/// bodies, destroy orphaned ones, propagate pose/velocity writes, sync
/// terrain + gravity. Runs once per tick before the box3d step; cost is a
/// merge walk, zero allocation in the steady state.
pub fn reconcile(
    dynamics: &mut RigidDynamics,
    entities: &[Entity],
    terrain: Option<&Terrain>,
    gravity: f32,
) {
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
            let materials = vec![0u8; ((cells - 1) * (cells - 1)).max(0) as usize];
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
            let shape_def = default_shape_def();
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
                });
            }
            (Some(mut entry), Some(kind)) => {
                if entry.kind != kind {
                    // Kind flips don't exist in the verb surface, but rollback
                    // could restore an older entity under a reused id.
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
                    };
                    out.push(entry);
                    continue;
                }
                match kind {
                    MirrorKind::Static => {
                        // set_pos/face on placed geometry: teleport the body.
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
    // No rigid bodies → nothing the solver could move; skip the step
    // entirely. Statics/kinematics are mirrored but never solver-driven, so
    // a rigid-free world (every existing gamemaker game) pays ~nothing.
    if !dynamics.mirror.iter().any(|m| m.kind == MirrorKind::Rigid) {
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
        entry.vel = e.vel;
    }
}

impl Clone for RigidDynamics {
    /// Bit-exact clone via the box3d snapshot round trip (its own test suite
    /// proves restored worlds continue bit-identically). BodyIds are stable
    /// across the round trip, so the mirror table clones verbatim.
    fn clone(&self) -> Self {
        let mut world = create_world(&default_world_def());
        if !self.mirror.is_empty() || self.terrain_body.is_some() {
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
            last_gravity: self.last_gravity,
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

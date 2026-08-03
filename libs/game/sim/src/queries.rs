//! Spatial queries + sweep primitives — moved verbatim from gamemaker's
//! game_view.rs (M0 stage A extraction). Float expression order preserved.

use makepad_math::*;

use crate::entity::*;
use crate::terrain::*;
use crate::world::GameWorld;

/// Decaying random camera offset (game.cam_shake). Hash of the tick, NOT the
/// world rng — pixels may wobble, simulation must not. Pinned to zero in tapes.
pub fn camera_shake_offset(world: &GameWorld, in_test: bool) -> Vec3f {
    if in_test || world.cam_shake <= 0.001 {
        return vec3f(0.0, 0.0, 0.0);
    }
    let mut h = world.tick.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 33;
    let fx = ((h & 0xFFFF) as f32 / 65535.0) * 2.0 - 1.0;
    let fy = (((h >> 16) & 0xFFFF) as f32 / 65535.0) * 2.0 - 1.0;
    let fz = (((h >> 32) & 0xFFFF) as f32 / 65535.0) * 2.0 - 1.0;
    vec3f(fx, fy, fz) * world.cam_shake * 0.35
}

/// game.raycast: march a ray against the terrain heightfield and every solid
/// AABB (sensors skipped; `collide:false` decor IS hit — it's visually solid,
/// and the grapple/AI wall-senses should see it). Returns (id, point, normal,
/// distance); TERRAIN_ID for ground hits.
pub fn world_raycast(
    world: &GameWorld,
    from: Vec3f,
    dir: Vec3f,
    max: f32,
) -> Option<(u64, Vec3f, Vec3f, f32)> {
    let len = dir.length();
    if len <= 1.0e-6 || max <= 0.0 {
        return None;
    }
    let dir = dir * (1.0 / len);
    const STEP: f32 = 0.15;
    let steps = ((max / STEP).ceil() as usize).max(1);
    let mut prev = from;
    for i in 1..=steps {
        let t = (i as f32 * STEP).min(max);
        let p = from + dir * t;
        if let Some(terrain) = &world.terrain {
            if let Some(h) = terrain.height_at(p.x, p.z) {
                if p.y <= h {
                    // Refine between prev and p for a tighter hit point.
                    let hit = (prev + p) * 0.5;
                    let normal = terrain
                        .normal_at(hit.x, hit.z)
                        .unwrap_or(vec3f(0.0, 1.0, 0.0));
                    return Some((TERRAIN_ID, vec3f(hit.x, h, hit.z), normal, t));
                }
            }
        }
        for e in &world.entities {
            if e.sensor {
                continue;
            }
            if (p.x - e.pos.x).abs() < e.half.x
                && (p.y - e.pos.y).abs() < e.half.y
                && (p.z - e.pos.z).abs() < e.half.z
            {
                // Face normal: the axis the ray is deepest along, pushed
                // back out — right for boxes, close enough for shapes.
                let rel = p - e.pos;
                let dx = (rel.x / e.half.x).abs();
                let dy = (rel.y / e.half.y).abs();
                let dz = (rel.z / e.half.z).abs();
                let normal = if dx >= dy && dx >= dz {
                    vec3f(rel.x.signum(), 0.0, 0.0)
                } else if dy >= dz {
                    vec3f(0.0, rel.y.signum(), 0.0)
                } else {
                    vec3f(0.0, 0.0, rel.z.signum())
                };
                let hit = (prev + p) * 0.5;
                return Some((e.id, hit, normal, t));
            }
        }
        prev = p;
    }
    None
}

/// How far the third-person boom may extend before hitting geometry: march
/// from the pivot toward the camera and stop at terrain or any solid box.
/// Entities tagged "scenery" are ignored (Godot keeps trees on a layer the
/// camera ray never sees, so foliage doesn't yank the view in).
pub fn camera_boom_limit(world: &GameWorld, pivot: Vec3f, dir: Vec3f, boom: f32) -> f32 {
    const STEPS: i32 = 32;
    for i in 1..=STEPS {
        let t = boom * i as f32 / STEPS as f32;
        let p = pivot + dir * t;
        if let Some(terrain) = &world.terrain {
            if let Some(h) = terrain.height_at(p.x, p.z) {
                if p.y < h + 0.2 {
                    return (t - 0.5).max(1.0);
                }
            }
        }
        for e in &world.entities {
            if e.sensor || e.tag == "scenery" {
                continue;
            }
            if !matches!(
                e.kind,
                BodyKind::Static | BodyKind::Kinematic | BodyKind::Rigid
            ) {
                continue;
            }
            if (p.x - e.pos.x).abs() < e.half.x
                && (p.y - e.pos.y).abs() < e.half.y
                && (p.z - e.pos.z).abs() < e.half.z
            {
                return (t - 0.5).max(1.0);
            }
        }
    }
    boom
}

pub fn axis_get(v: Vec3f, axis: usize) -> f32 {
    match axis {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    }
}

pub fn axis_set(v: &mut Vec3f, axis: usize, value: f32) {
    match axis {
        0 => v.x = value,
        1 => v.y = value,
        _ => v.z = value,
    }
}

/// Gap left between a swept body and whatever it lands against, so resting
/// contact is never exactly flush. Small enough to be invisible, large enough
/// that a perpendicular sweep in the same tick cannot mistake it for overlap.
pub const CONTACT_SKIN: f32 = 1.0e-3;

pub fn overlaps(a_pos: Vec3f, a_half: Vec3f, b_pos: Vec3f, b_half: Vec3f) -> bool {
    (a_pos.x - b_pos.x).abs() < a_half.x + b_half.x
        && (a_pos.y - b_pos.y).abs() < a_half.y + b_half.y
        && (a_pos.z - b_pos.z).abs() < a_half.z + b_half.z
}

/// Move one axis and clamp against every solid; returns (clamped, hit_dir, hit_id).
/// The subset of an [`Entity`] a mover sweep actually reads. `step_world`
/// snapshots the collidable statics/kinematics once per tick (movers hold
/// `&mut entities` while sweeping, and the snapshot must predate this tick's
/// kinematic integration — that ordering is load-bearing for tape parity).
/// Copying 48 bytes instead of a 208-byte `Entity` is the whole point: at
/// 2000 statics that is 416 KB/tick of memcpy turned into 96 KB.
#[derive(Clone, Copy, Debug)]
pub struct Solid {
    pub id: u64,
    pub kind: BodyKind,
    pub pos: Vec3f,
    pub half: Vec3f,
    pub vel: Vec3f,
}

impl From<&Entity> for Solid {
    fn from(e: &Entity) -> Self {
        Self {
            id: e.id,
            kind: e.kind,
            pos: e.pos,
            half: e.half,
            vel: e.vel,
        }
    }
}

/// Relaxation passes per tick in [`separate_movers`]. Fixed, not
/// convergence-based: an early-exit on "nothing moved" makes the result depend
/// on iteration order, and this has to be bit-reproducible.
pub const SEPARATION_ITERATIONS: usize = 3;

/// Push overlapping movers apart so characters shoulder past each other
/// instead of clipping through.
///
/// Deliberately a post-pass, not a change to the sweep: the sweep carries the
/// 0.55 step-up, `CONTACT_SKIN` and the terrain-cliff logic, and it is the
/// path every existing contract was written against. Movers are resolved
/// AFTER they have all moved, which is also what makes the result independent
/// of who was simulated first.
///
/// Horizontal only. Resolving the vertical axis is how characters end up
/// standing on each other's heads — an overlapping pair is pushed apart on the
/// ground plane and the stack unpicks itself.
///
/// `hits` entities (projectiles) are excluded, and that is a correctness
/// requirement rather than a preference: [`collect_touches`] reports a
/// projectile strike *from the overlap itself* ("movers pass through each
/// other spatially, so overlap IS the hit"). Separating them would mean a
/// bullet could never touch anyone.
///
/// [`collect_touches`]: crate::step::collect_touches
pub fn separate_movers(entities: &mut [Entity], statics: &[Solid]) {
    let mut idx: Vec<usize> = Vec::new();
    let mut widest = 0.0f32;
    for (i, e) in entities.iter().enumerate() {
        if e.kind != BodyKind::Mover
            || e.sensor
            || !e.collide
            // Riders are pinned to their carrier after this; shoving one would
            // be overwritten anyway, and shoving its carrier is not our call.
            || e.attached_to != 0
            || e.hits
        {
            continue;
        }
        idx.push(i);
        widest = widest.max(e.half.x).max(e.half.z);
    }
    if idx.len() < 2 {
        return;
    }
    // A cell this wide means an overlapping pair can only ever be in the same
    // or an adjacent cell, so the 3x3 neighbourhood is exhaustive.
    let cell = (widest * 2.0).max(0.5);
    // Sorted (cell, index) pairs rather than a hash map of buckets: a map
    // allocates a Vec per occupied cell per iteration, and this runs three
    // times a tick. Sorting is also what makes the neighbour walk ordered
    // without a second sort of the candidates.
    let mut keys: Vec<(i64, usize)> = Vec::with_capacity(idx.len());

    for _ in 0..SEPARATION_ITERATIONS {
        keys.clear();
        for (n, &i) in idx.iter().enumerate() {
            keys.push((cell_key(entities[i].pos, cell), n));
        }
        keys.sort_unstable();
        // `idx` follows entity order and entities are sorted by id, so this
        // walk is deterministic; within a cell, `keys` is ordered by index.
        for an in 0..idx.len() {
            let (cx, cz) = cell_xz(entities[idx[an]].pos, cell);
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let want = pack_cell(cx + dx, cz + dz);
                    let lo = keys.partition_point(|&(k, _)| k < want);
                    for &(k, bn) in &keys[lo..] {
                        if k != want {
                            break;
                        }
                        if bn > an {
                            resolve_mover_pair(entities, idx[an], idx[bn], statics);
                        }
                    }
                }
            }
        }
    }
}

fn cell_xz(p: Vec3f, cell: f32) -> (i32, i32) {
    ((p.x / cell).floor() as i32, (p.z / cell).floor() as i32)
}

fn pack_cell(x: i32, z: i32) -> i64 {
    ((x as i64) << 32) | (z as u32 as i64)
}

fn cell_key(p: Vec3f, cell: f32) -> i64 {
    let (x, z) = cell_xz(p, cell);
    pack_cell(x, z)
}

fn resolve_mover_pair(entities: &mut [Entity], a: usize, b: usize, statics: &[Solid]) {
    let (pa, ha) = (entities[a].pos, entities[a].half);
    let (pb, hb) = (entities[b].pos, entities[b].half);
    let ox = (ha.x + hb.x) - (pa.x - pb.x).abs();
    let oy = (ha.y + hb.y) - (pa.y - pb.y).abs();
    let oz = (ha.z + hb.z) - (pa.z - pb.z).abs();
    if ox <= 0.0 || oy <= 0.0 || oz <= 0.0 {
        return;
    }
    // Least-penetration axis: the shortest way out is the one that looks like
    // stepping aside rather than teleporting around.
    let (axis, depth) = if ox <= oz { (0usize, ox) } else { (2usize, oz) };
    let delta = axis_get(pa, axis) - axis_get(pb, axis);
    let sign = if delta > 0.0 {
        1.0
    } else if delta < 0.0 {
        -1.0
    } else {
        // Exactly coincident (two NPCs spawned on one point). Break by id so
        // the pair always unpicks the same way.
        if entities[a].id < entities[b].id {
            -1.0
        } else {
            1.0
        }
    };
    let push = (depth + CONTACT_SKIN) * sign;
    let ma = push_mass_of(&entities[a]);
    let mb = push_mass_of(&entities[b]);
    let total = ma + mb;
    // Heavier moves less: a's share is weighted by the OTHER body's mass.
    shove(entities, a, axis, push * (mb / total), statics);
    shove(entities, b, axis, -push * (ma / total), statics);
}

/// Apply one axis of separation, clamped against the solid world. Without the
/// clamp a crowd pressed against a wall would squeeze its outermost members
/// straight through it.
fn shove(entities: &mut [Entity], i: usize, axis: usize, delta: f32, statics: &[Solid]) {
    if delta == 0.0 {
        return;
    }
    let (id, pos, half) = {
        let e = &entities[i];
        (e.id, e.pos, e.half)
    };
    let (clamped, _hit, _hit_id) = sweep_axis(statics, id, pos, half, axis, delta);
    axis_set(&mut entities[i].pos, axis, clamped);
}

pub fn sweep_axis(
    entities: &[Solid],
    self_id: u64,
    pos: Vec3f,
    half: Vec3f,
    axis: usize,
    delta: f32,
) -> (f32, f32, u64) {
    let mut new_axis = axis_get(pos, axis) + delta;
    let mut hit = 0.0f32;
    let mut hit_id = 0u64;
    for other in entities {
        // The caller's snapshot filter already drops sensors and non-colliders,
        // so the old `other.sensor` test here was provably dead; the kind test
        // is kept because it is part of the documented sweep contract.
        if other.id == self_id {
            continue;
        }
        if !matches!(
            other.kind,
            BodyKind::Static | BodyKind::Kinematic | BodyKind::Rigid
        ) {
            continue;
        }
        let mut probe = pos;
        axis_set(&mut probe, axis, new_axis);
        if overlaps(probe, half, other.pos, other.half) {
            // Stop a hair short of flush. Clamping to exactly touching leaves
            // the two boxes at |d| == sum-of-halves, where float error decides
            // the next axis' overlap test either way — and a "yes" there sent a
            // falling mover UP onto a crate it had merely walked into, clean
            // through the 0.55 step-up contract. The skin makes contact
            // unambiguous instead of borderline.
            let gap = axis_get(half, axis) + axis_get(other.half, axis) + CONTACT_SKIN;
            if delta > 0.0 {
                new_axis = new_axis.min(axis_get(other.pos, axis) - gap);
                hit = 1.0;
                hit_id = other.id;
            } else if delta < 0.0 {
                new_axis = new_axis.max(axis_get(other.pos, axis) + gap);
                hit = -1.0;
                hit_id = other.id;
            }
        }
    }
    (new_axis, hit, hit_id)
}

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
            if !matches!(e.kind, BodyKind::Static | BodyKind::Kinematic) {
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

pub fn overlaps(a_pos: Vec3f, a_half: Vec3f, b_pos: Vec3f, b_half: Vec3f) -> bool {
    (a_pos.x - b_pos.x).abs() < a_half.x + b_half.x
        && (a_pos.y - b_pos.y).abs() < a_half.y + b_half.y
        && (a_pos.z - b_pos.z).abs() < a_half.z + b_half.z
}

/// Move one axis and clamp against every solid; returns (clamped, hit_dir, hit_id).
pub fn sweep_axis(
    entities: &[Entity],
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
        if other.id == self_id || other.sensor {
            continue;
        }
        if !matches!(other.kind, BodyKind::Static | BodyKind::Kinematic) {
            continue;
        }
        let mut probe = pos;
        axis_set(&mut probe, axis, new_axis);
        if overlaps(probe, half, other.pos, other.half) {
            let gap = axis_get(half, axis) + axis_get(other.half, axis);
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

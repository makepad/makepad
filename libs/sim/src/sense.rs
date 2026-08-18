//! Perception probes — what an NPC can find out about the world around it.
//!
//! Every probe reads the SAME solid set the mover sweep collides against
//! (`!sensor && collide && Static|Kinematic|Rigid`), so what an NPC believes it
//! can walk through and what actually stops it cannot disagree. A second,
//! subtly different notion of "solid" is exactly how AI ends up walking into
//! walls it was told were clear.
//!
//! These are deliberately cheap and stateless: a linear scan with an early-out,
//! called a few times per NPC per decision rather than every tick for every
//! NPC. A nav grid would beat them once a level needs real pathfinding through
//! rooms and doorways — see the note in `blocks::npc`.

use makepad_math::*;

use crate::entity::{BodyKind, Entity};
use crate::queries::overlaps;
use crate::world::GameWorld;

/// A solid a mover would run into.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Obstacle {
    pub id: u64,
    /// World-space top of the blocking box. Compare against the mover's feet:
    /// within step-up it is a kerb, within jump reach it is a crate, above
    /// that it is a wall and the only options are around or give up.
    pub top: f32,
    /// How far ahead the block starts.
    pub dist: f32,
}

/// The sweep's own filter, so perception and collision agree by construction.
#[inline]
fn blocks_movement(e: &Entity) -> bool {
    !e.sensor
        && e.collide
        && matches!(
            e.kind,
            BodyKind::Static | BodyKind::Kinematic | BodyKind::Rigid
        )
}

/// Highest solid within `reach` along `dir`, or None if the way is clear.
///
/// Samples the mover's own box forward rather than casting a ray: a ray
/// through a doorway says "clear" for a body twice its width, and an NPC that
/// trusts it wedges itself in the frame.
pub fn obstacle_ahead(
    world: &GameWorld,
    self_id: u64,
    pos: Vec3f,
    half: Vec3f,
    dir: Vec3f,
    reach: f32,
) -> Option<Obstacle> {
    let len = crate::math::sqrt(dir.x * dir.x + dir.z * dir.z);
    if len <= 1.0e-6 || reach <= 0.0 {
        return None;
    }
    let (dx, dz) = (dir.x / len, dir.z / len);
    const SAMPLES: usize = 3;
    for i in 1..=SAMPLES {
        let t = reach * i as f32 / SAMPLES as f32;
        let probe = vec3f(pos.x + dx * t, pos.y, pos.z + dz * t);
        let mut best: Option<Obstacle> = None;
        for e in &world.entities {
            if e.id == self_id || !blocks_movement(e) {
                continue;
            }
            if overlaps(probe, half, e.pos, e.half) {
                let top = e.pos.y + e.half.y;
                // Tallest wins: a doorstep in front of a wall must report the
                // wall, or the NPC steps up and heads straight into it.
                if best.map_or(true, |b: Obstacle| top > b.top) {
                    best = Some(Obstacle { id: e.id, top, dist: t });
                }
            }
        }
        // Nearest sample that hits anything is the one that matters.
        if best.is_some() {
            return best;
        }
    }
    None
}

/// Height of the surface an NPC would land on `ahead` units along `dir`, or
/// None over a hole. Used to refuse to walk off a ledge.
///
/// Anything whose top is well above the mover's feet is a wall, not a floor,
/// and is ignored here — that case belongs to [`obstacle_ahead`].
pub fn ground_ahead(
    world: &GameWorld,
    pos: Vec3f,
    half: Vec3f,
    dir: Vec3f,
    ahead: f32,
) -> Option<f32> {
    let len = crate::math::sqrt(dir.x * dir.x + dir.z * dir.z);
    if len <= 1.0e-6 {
        return None;
    }
    let probe = vec3f(pos.x + dir.x / len * ahead, pos.y, pos.z + dir.z / len * ahead);
    let feet = pos.y - half.y;
    let mut ground = world
        .terrain
        .as_ref()
        .and_then(|t| t.height_at(probe.x, probe.z));
    for e in &world.entities {
        if !blocks_movement(e) {
            continue;
        }
        let top = e.pos.y + e.half.y;
        if top > feet + 0.6 {
            continue;
        }
        if (probe.x - e.pos.x).abs() < e.half.x && (probe.z - e.pos.z).abs() < e.half.z {
            ground = Some(ground.map_or(top, |g: f32| g.max(top)));
        }
    }
    ground
}

/// Could a body of this size stand here without overlapping anything solid?
pub fn spot_clear(world: &GameWorld, self_id: u64, pos: Vec3f, half: Vec3f) -> bool {
    !world
        .entities
        .iter()
        .any(|e| e.id != self_id && blocks_movement(e) && overlaps(pos, half, e.pos, e.half))
}

/// Is there room to sidestep `offset` units perpendicular to `dir` and keep
/// going? Checks the sidestep AND the step after it, because a gap you can
/// stand in but not walk out of is not a way around.
pub fn side_clearance(
    world: &GameWorld,
    self_id: u64,
    pos: Vec3f,
    half: Vec3f,
    dir: Vec3f,
    side: f32,
    offset: f32,
) -> bool {
    let len = crate::math::sqrt(dir.x * dir.x + dir.z * dir.z);
    if len <= 1.0e-6 {
        return false;
    }
    let (dx, dz) = (dir.x / len, dir.z / len);
    // Perpendicular on the ground plane.
    let (px, pz) = (-dz * side, dx * side);
    let step = vec3f(pos.x + px * offset, pos.y, pos.z + pz * offset);
    if !spot_clear(world, self_id, step, half) {
        return false;
    }
    let onward = vec3f(step.x + dx * offset, step.y, step.z + dz * offset);
    spot_clear(world, self_id, onward, half)
}

//! Spatial queries + sweep primitives — moved verbatim from gamemaker's
//! game_view.rs (M0 stage A extraction). Float expression order preserved.

use makepad_math::*;

use crate::entity::*;
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
    // The amplitude is world units of EYE offset, calibrated for a boomed
    // third-person camera. At first-person distances (boom ≈ 0.5) the same
    // offset swings the whole view — recoil at 6 rounds/s read as the camera
    // "spazzing out". Scale with the boom so a shake means the same thing at
    // every camera distance.
    let boom_scale = (world.cam_distance / 8.0).clamp(0.12, 1.0);
    vec3f(fx, fy, fz) * world.cam_shake * 0.35 * boom_scale
}

/// game.raycast: the EXACT world ray (F7 — the 0.15-step AABB march is
/// retired). Runs against the box3d mirror via
/// [`crate::dynamics::cast_ray`], so it hits the terrain heightfield,
/// statics, kinematics, rigids, movers (their D2 capsules — hitscan sees a
/// strafing player) and `collide:false` decor (visually solid, so grapples
/// and AI wall-senses should see it; mirrored as query-only shapes). Sensors
/// are skipped, as always. Returns (id, point, TRUE surface normal, exact
/// distance, surface material id); TERRAIN_ID for ground hits.
///
/// Takes `&mut` because it reconciles the mirror first
/// ([`GameWorld::sync_queries`]): a box the script spawned this very eval is
/// hittable immediately, exactly like it was under the march.
pub fn world_raycast(
    world: &mut GameWorld,
    from: Vec3f,
    dir: Vec3f,
    max: f32,
) -> Option<(u64, Vec3f, Vec3f, f32, u64)> {
    world.sync_queries();
    let hit = crate::dynamics::cast_ray(&world.dynamics, from, dir, max)?;
    Some((hit.id, hit.pos, hit.normal, hit.dist, hit.material))
}

/// Pick ray for screen coordinates under the ORBIT camera — the RTS /
/// top-down view (N4). `u`/`v` are screen fractions (0,0 = top-left,
/// 1,1 = bottom-right), `aspect` = viewport width/height.
///
/// This mirrors the renderer's orbit branch (`game_render::scene_state`):
/// target = `cam_target` (or the followed entity), eye = target − forward ·
/// `cam_distance`, forward from `orbit_yaw`/`orbit_pitch`, vertical FOV =
/// `cam_fov`. Camera shake is deliberately excluded (picking through a
/// wobble would mis-select) and the third-person rig branch is NOT
/// mirrored — selection is an orbit/top-down-camera feature; under a rig
/// camera the caller should pass world points instead.
///
/// Returns (origin, unit direction).
pub fn camera_pick_ray(world: &GameWorld, u: f32, v: f32, aspect: f32) -> (Vec3f, Vec3f) {
    let mut target = world.cam_target;
    if world.cam_follow != 0 {
        if let Some(e) = world.entity(world.cam_follow) {
            target = e.pos;
        }
    }
    let distance = world.cam_distance.max(0.5);
    let (yaw, pitch) = (world.orbit_yaw, world.orbit_pitch.clamp(-1.45, 1.45));
    // Screen selection becomes Shared state through `game.select`, so the
    // camera-derived ray is gameplay math even though drawing the camera is
    // device-local.
    let (sin_yaw, cos_yaw) = crate::math::sincos(yaw);
    let (sin_pitch, cos_pitch) = crate::math::sincos(pitch);
    let forward = crate::vec3_normalize(vec3f(
        sin_yaw * cos_pitch,
        sin_pitch,
        -cos_yaw * cos_pitch,
    ));
    let eye = target - forward * distance;
    let up = vec3f(0.0, 1.0, 0.0);
    let right = crate::vec3_normalize(Vec3f::cross(forward, up));
    let cam_up = Vec3f::cross(right, forward);
    let half_fov = world.cam_fov.clamp(20.0, 120.0).to_radians() * 0.5;
    let tan_half = crate::math::tan(half_fov);
    let dir = crate::vec3_normalize(
        forward
        + right * ((u * 2.0 - 1.0) * tan_half * aspect)
        + cam_up * ((1.0 - v * 2.0) * tan_half),
    );
    (eye, dir)
}

/// Where a pick ray meets the world: the exact raycast first, else the
/// horizontal plane through the camera target (so a drag that overshoots
/// the geometry still spans a sensible ground rect).
pub fn pick_ground_point(world: &mut GameWorld, u: f32, v: f32, aspect: f32) -> Vec3f {
    let (from, dir) = camera_pick_ray(world, u, v, aspect);
    if let Some((_, pos, _, _, _)) = world_raycast(world, from, dir, 600.0) {
        return pos;
    }
    let plane_y = world.cam_target.y;
    let denom = dir.y;
    if denom.abs() > 1.0e-5 {
        let t = (plane_y - from.y) / denom;
        if t > 0.0 {
            return from + dir * t;
        }
    }
    // Looking at the sky: fall back to a far point along the ray's ground
    // direction so the rect degenerates gracefully.
    from + dir * 600.0
}

/// How far a camera offset may extend before hitting geometry. This is the
/// shared point sweep used by both the backwards boom and a lateral shoulder
/// offset, so the two paths cannot disagree about terrain or collision layers.
/// Entities tagged "scenery" are ignored (Godot keeps trees on a layer the
/// camera ray never sees, so foliage doesn't yank the view in).
pub fn camera_path_limit(
    world: &GameWorld,
    origin: Vec3f,
    dir: Vec3f,
    distance: f32,
    clearance: f32,
    minimum: f32,
) -> f32 {
    const STEPS: i32 = 32;
    for i in 1..=STEPS {
        let t = distance * i as f32 / STEPS as f32;
        let p = origin + dir * t;
        if let Some(terrain) = &world.terrain {
            if let Some(h) = terrain.height_at(p.x, p.z) {
                if p.y < h + 0.2 {
                    return (t - clearance).max(minimum);
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
                return (t - clearance).max(minimum);
            }
        }
    }
    distance
}

/// How far the third-person boom may extend before hitting geometry: march
/// from the pivot toward the camera and stop half a metre before a solid,
/// retaining the historical one-metre minimum for close walls.
pub fn camera_boom_limit(world: &GameWorld, pivot: Vec3f, dir: Vec3f, boom: f32) -> f32 {
    camera_path_limit(world, pivot, dir, boom, 0.5, 1.0)
}

// ── hurtbox bands (mix.md §5.4.2 / K3) ──────────────────────────────────
//
// A mover's melee hurtbox is its D2 mirror capsule split into three stacked
// segments — high/mid/low — enough for overheads, mids and sweeps. Attack
// volumes are sim QUERIES against these bands, not bodies: nothing here
// touches box3d or the interaction matrix, and the radius expression is the
// mirror capsule's own (`dynamics::mover_capsule`) so what a raycast hits
// and what a jab hits are one shape.

/// Which stacked third of the hurtbox capsule a hit landed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HitBand {
    Low = 0,
    Mid = 1,
    High = 2,
}

impl HitBand {
    pub fn parse(name: &str) -> Option<HitBand> {
        match name {
            "low" => Some(HitBand::Low),
            "mid" => Some(HitBand::Mid),
            "high" => Some(HitBand::High),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            HitBand::Low => "low",
            HitBand::Mid => "mid",
            HitBand::High => "high",
        }
    }
}

/// The hurtbox capsule's radius — the SAME expression as the D2 mirror
/// capsule (`half.x.max(half.z)` clamped by the half-height), so the melee
/// query and the physics mirror cannot disagree about how wide a body is.
pub fn hurtbox_radius(e: &Entity) -> f32 {
    e.half.x.max(e.half.z).min(e.half.y).max(0.01)
}

/// The three band ranges `[(y_lo, y_hi); 3]` in world y, low/mid/high, an
/// equal-thirds split of the body from feet to crown.
pub fn hurtbox_bands(e: &Entity) -> [(f32, f32); 3] {
    let feet = e.pos.y - e.half.y;
    let third = (e.half.y * 2.0) / 3.0;
    [
        (feet, feet + third),
        (feet + third, feet + third * 2.0),
        (feet + third * 2.0, feet + third * 3.0),
    ]
}

/// Test a sphere attack volume against an entity's banded hurtbox. Returns
/// the band STRUCK (the band containing the sphere's centre height, clamped
/// into the body) or `None` on a whiff. Horizontal test is centre-axis
/// distance against `r + hurtbox_radius` — the capsule test, exact for the
/// vertical capsule the mirror uses; vertical test requires the sphere to
/// overlap the body's feet-to-crown extent.
pub fn hurtbox_hit(e: &Entity, center: Vec3f, r: f32) -> Option<HitBand> {
    if e.non_interactive {
        return None;
    }
    let dx = center.x - e.pos.x;
    let dz = center.z - e.pos.z;
    let reach = r + hurtbox_radius(e);
    if dx * dx + dz * dz > reach * reach {
        return None;
    }
    let feet = e.pos.y - e.half.y;
    let crown = e.pos.y + e.half.y;
    if center.y + r < feet || center.y - r > crown {
        return None;
    }
    // Band of the clamped centre height: a sweep at the ankle reads Low even
    // when its sphere laps into the shin, which is the classification a
    // guard check wants. Deterministic: pure float compares, no rounding.
    let y = center.y.clamp(feet, crown);
    let third = (e.half.y * 2.0) / 3.0;
    Some(if y < feet + third {
        HitBand::Low
    } else if y < feet + third * 2.0 {
        HitBand::Mid
    } else {
        HitBand::High
    })
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
    /// Carried so a sloped solid can be walked up instead of collided with as
    /// the box that contains it — see [`ramp_floor_under`].
    pub shape: Shape,
}

impl From<&Entity> for Solid {
    fn from(e: &Entity) -> Self {
        Self {
            id: e.id,
            kind: e.kind,
            pos: e.pos,
            half: e.half,
            vel: e.vel,
            shape: e.shape,
        }
    }
}

/// Height of the walkable surface of a WEDGE at a world x/z, or `None` when
/// the point is outside its footprint.
///
/// A wedge is authored sloping from its top back edge (+z) down to its bottom
/// front edge (-z) — the same geometry the renderer draws, so what you see is
/// what you climb.
///
/// This exists because a wedge was previously collided as its bounding box:
/// the ramp built to be driven and walked up was, to the simulation, a solid
/// cube with a vertical face. Walking into it stopped you dead against an
/// invisible wall halfway up nothing.
pub fn wedge_surface_at(s: &Solid, x: f32, z: f32) -> Option<f32> {
    if s.shape != Shape::Wedge {
        return None;
    }
    if x < s.pos.x - s.half.x || x > s.pos.x + s.half.x {
        return None;
    }
    let z0 = s.pos.z - s.half.z;
    let z1 = s.pos.z + s.half.z;
    if z < z0 || z > z1 {
        return None;
    }
    // 0 at the low front edge, 1 at the high back edge.
    let t = ((z - z0) / (z1 - z0).max(1e-6)).clamp(0.0, 1.0);
    Some(s.pos.y - s.half.y + t * (s.half.y * 2.0))
}

/// Upward unit normal of a wedge's sloped surface (P2). The slope runs from
/// the low front edge (-z) to the high back edge (+z) — the same geometry
/// [`wedge_surface_at`] walks — so the surface `y = m·z + c` with
/// `m = half.y / half.z` has upward normal `(0, 1, -m) / len`. Pure algebra
/// (one sqrt), no transcendentals.
pub fn wedge_normal(s: &Solid) -> Vec3f {
    let m = (s.half.y * 2.0) / (s.half.z * 2.0).max(1e-6);
    let len = crate::math::sqrt(1.0 + m * m);
    vec3f(0.0, 1.0 / len, -m / len)
}

/// Highest wedge surface under a mover's FOOTPRINT, with the id of the wedge.
///
/// Sampled at the footprint's corners and centre rather than at its centre
/// alone: a body standing with half its feet on a ramp should stand on the
/// ramp, exactly as `Terrain::floor_under` treats the ground.
pub fn ramp_floor_under(statics: &[Solid], pos: Vec3f, half: Vec3f) -> Option<(f32, u64)> {
    let mut best: Option<(f32, u64)> = None;
    for s in statics {
        if s.shape != Shape::Wedge {
            continue;
        }
        for (dx, dz) in [
            (0.0, 0.0),
            (-half.x, -half.z),
            (half.x, -half.z),
            (-half.x, half.z),
            (half.x, half.z),
        ] {
            if let Some(y) = wedge_surface_at(s, pos.x + dx, pos.z + dz) {
                if best.map_or(true, |(by, _)| y > by) {
                    best = Some((y, s.id));
                }
            }
        }
    }
    best
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

#[cfg(test)]
mod pick_tests {
    use super::*;

    #[test]
    fn center_pick_ray_aims_at_the_camera_target() {
        let mut world = GameWorld::new();
        world.cam_target = vec3f(3.0, 2.0, -4.0);
        world.cam_distance = 20.0;
        world.cam_fov = 40.0;
        world.orbit_yaw = 0.6;
        world.orbit_pitch = -0.35;
        let (from, dir) = camera_pick_ray(&world, 0.5, 0.5, 16.0 / 9.0);
        let to_target = (world.cam_target - from).normalize();
        let dot = dir.x * to_target.x + dir.y * to_target.y + dir.z * to_target.z;
        assert!(dot > 0.9999, "center ray must pass through the target: {dot}");
        // Corner rays diverge from the center one.
        let (_, corner) = camera_pick_ray(&world, 0.0, 0.0, 16.0 / 9.0);
        let dot_c = dir.x * corner.x + dir.y * corner.y + dir.z * corner.z;
        assert!(dot_c < 0.999, "corner ray must diverge: {dot_c}");
    }

    #[test]
    fn pick_ground_point_lands_on_the_slab() {
        let mut world = GameWorld::new();
        world.push_entity(Entity {
            id: 1,
            kind: BodyKind::Static,
            pos: vec3f(0.0, -0.5, 0.0),
            half: vec3f(50.0, 0.5, 50.0),
            collide: true,
            ..Default::default()
        });
        world.next_id = 1;
        world.cam_target = vec3f(0.0, 0.0, 0.0);
        world.cam_distance = 25.0;
        world.cam_fov = 45.0;
        world.orbit_yaw = 0.0;
        world.orbit_pitch = -1.2; // near top-down
        let p = pick_ground_point(&mut world, 0.5, 0.5, 16.0 / 9.0);
        assert!(p.y.abs() < 0.1, "hit the slab surface: {p:?}");
        assert!(p.x.abs() < 2.0 && p.z.abs() < 6.0, "near the target: {p:?}");
    }
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
            // A solid this body ALREADY overlaps cannot clamp the sweep. The
            // sweep prevents ENTERING solids and, with CONTACT_SKIN, never
            // itself creates overlap — so pre-existing overlap means the
            // solid overran US (a car running over a knocked-down walker,
            // D4/F8). The old clamp would snap the victim to whichever face
            // the axis math picked — visibly, a walker teleporting through
            // the car that just hit them. Being inside, they keep their
            // motion and exit when the geometry lets them.
            if overlaps(pos, half, other.pos, other.half) {
                continue;
            }
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

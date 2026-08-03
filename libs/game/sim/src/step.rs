//! The fixed-step world update + touch collection — moved verbatim from
//! gamemaker's game_view.rs (M0 stage A extraction). Float expression order
//! preserved exactly; tapes must replay bit-identically across the move.

use std::collections::HashSet;

use makepad_math::*;

use crate::entity::*;
use crate::queries::*;
use crate::terrain::TERRAIN_ID;
use crate::world::GameWorld;
use crate::TICK_DT;

pub fn step_world(world: &mut GameWorld) {
    // The sorted-by-id invariant backs every binary-search lookup (world +
    // renderer). push_entity asserts incrementally; this catches everything
    // else (retain/rollback) once per tick in debug builds.
    debug_assert!(world.entities_sorted_by_id());
    let gravity = world.gravity;
    // Snapshotted BEFORE the kinematic integration below: movers sweep against
    // last tick's kinematic poses. That ordering is load-bearing for tape
    // parity, so this stays a copy — but a copy of the 48-byte `Solid` view,
    // not the whole 208-byte entity.
    let statics: Vec<Solid> = world
        .entities
        .iter()
        // Sensors report touches but never collide; `collide: false` decor
        // neither collides nor reports — both documented contracts.
        .filter(|e| {
            !e.sensor && e.collide && matches!(e.kind, BodyKind::Static | BodyKind::Kinematic)
        })
        .map(Solid::from)
        .collect();
    /// A step this tall walks up for free (Godot floor snapping over the
    /// terraced 0.5 steps); anything taller is a cliff wall.
    const CLIMB: f32 = 0.55;

    // Terrain is read-only here and was previously cloned once per tick purely
    // to dodge the `&mut entities` borrow — 1.3 MB of memcpy per tick on a
    // 257² field. Splitting the struct borrow costs nothing and copies nothing.
    let GameWorld {
        entities: world_entities,
        terrain: world_terrain,
        ..
    } = &mut *world;
    let terrain = world_terrain.as_ref();

    // Kinematics move first (script set their velocity).
    for e in world_entities.iter_mut() {
        if e.kind == BodyKind::Kinematic {
            e.pos = e.pos + e.vel * TICK_DT;
        }
    }

    for e in world_entities.iter_mut() {
        if e.kind != BodyKind::Mover {
            continue;
        }
        // Riders are pinned to their vehicle after this loop, not simulated.
        if e.attached_to != 0 {
            continue;
        }
        // Carried by the platform we stand on.
        if e.on_floor && e.floor_id != 0 {
            if let Some(base) = statics.iter().find(|s| s.id == e.floor_id) {
                if base.kind == BodyKind::Kinematic {
                    e.pos = e.pos + base.vel * TICK_DT;
                }
            }
        }

        e.vel.y -= gravity * e.gravity_scale * TICK_DT;

        // Axis-separated sweeps: x, z, then y (so walking into a wall while
        // falling doesn't stick, and floors resolve last for on_floor).
        e.hit_wall = 0;
        let feet = e.pos.y - e.half.y;
        let (nx, hx, hx_id) = sweep_axis(&statics, e.id, e.pos, e.half, 0, e.vel.x * TICK_DT);
        e.pos.x = nx;
        if hx != 0.0 {
            e.vel.x = 0.0;
            e.hit_wall = hx_id;
        }
        // Terrain cliffs block sideways movement; steps ≤ CLIMB pass (the y
        // pass snaps the mover up onto them).
        if let Some(t) = terrain {
            if let Some(ground) = t.floor_under(e.pos, e.half) {
                if ground > feet + CLIMB {
                    e.pos.x = nx - e.vel.x * TICK_DT;
                    e.vel.x = 0.0;
                    if e.hit_wall == 0 {
                        e.hit_wall = TERRAIN_ID;
                    }
                }
            }
        }
        let (nz, hz, hz_id) = sweep_axis(&statics, e.id, e.pos, e.half, 2, e.vel.z * TICK_DT);
        e.pos.z = nz;
        if hz != 0.0 {
            e.vel.z = 0.0;
            if e.hit_wall == 0 {
                e.hit_wall = hz_id;
            }
        }
        if let Some(t) = terrain {
            if let Some(ground) = t.floor_under(e.pos, e.half) {
                if ground > feet + CLIMB {
                    e.pos.z = nz - e.vel.z * TICK_DT;
                    e.vel.z = 0.0;
                    if e.hit_wall == 0 {
                        e.hit_wall = TERRAIN_ID;
                    }
                }
            }
        }
        let (ny, hy, hy_id) = sweep_axis(&statics, e.id, e.pos, e.half, 1, e.vel.y * TICK_DT);
        e.pos.y = ny;
        e.on_floor = false;
        e.floor_id = 0;
        if hy != 0.0 {
            if e.vel.y < 0.0 {
                e.on_floor = true;
                e.floor_id = hy_id;
            }
            e.vel.y = 0.0;
            if e.hit_wall == 0 && e.hits {
                // A lobbed projectile landing counts as a hit too.
                e.hit_wall = hy_id;
            }
        }
        // The terrain is a floor: feet never sink below the ground surface.
        if let Some(t) = terrain {
            if let Some(ground) = t.floor_under(e.pos, e.half) {
                let floor_y = ground + e.half.y;
                if e.pos.y <= floor_y {
                    e.pos.y = floor_y;
                    if e.vel.y <= 0.0 {
                        e.on_floor = true;
                        e.floor_id = 0;
                        if e.hit_wall == 0 && e.hits {
                            e.hit_wall = TERRAIN_ID;
                        }
                        e.vel.y = 0.0;
                    }
                }
            }
        }
    }

    // Pin riders to their owners (vehicle seats, latched headcrabs). One pass
    // after integration, same frame the owner moved — the Godot mount pattern.
    // Most worlds attach nothing, and this pose table used to be built over
    // every entity regardless; the guard skips both the allocation and the
    // scan when no rider exists. The loop below is a no-op in that case, so
    // behaviour is unchanged.
    let owner_pose: Vec<(u64, Vec3f, f32)> = if world.entities.iter().any(|e| e.attached_to != 0) {
        world.entities.iter().map(|e| (e.id, e.pos, e.yaw)).collect()
    } else {
        Vec::new()
    };
    for e in world.entities.iter_mut() {
        if e.attached_to == 0 {
            continue;
        }
        if let Some((_, base, owner_yaw)) =
            owner_pose.iter().find(|(id, _, _)| *id == e.attached_to)
        {
            e.pos = *base + e.attach_offset;
            e.vel = vec3f(0.0, 0.0, 0.0);
            if e.attach_ride {
                // A latched rider scrabbles: its model spins in place.
                e.yaw += e.attach_spin * TICK_DT;
            } else {
                // A seated passenger faces where the vehicle faces.
                e.yaw = *owner_yaw;
            }
        } else {
            // Owner despawned: let go rather than freezing in the air.
            e.attached_to = 0;
        }
    }

    // Visual animation: facing, model scale, part poses. Rendering-only
    // state, but stepped with physics so input tapes replay identically.
    for e in world.entities.iter_mut() {
        if e.auto_face && e.kind == BodyKind::Mover {
            let speed = (e.vel.x * e.vel.x + e.vel.z * e.vel.z).sqrt();
            if speed > 0.2 {
                // Godot's shared _drive(): face where you walk, turn-rate
                // clamped, fronts at -z.
                let want = (-e.vel.x).atan2(-e.vel.z);
                let mut diff = want - e.yaw;
                while diff > std::f32::consts::PI {
                    diff -= std::f32::consts::TAU;
                }
                while diff < -std::f32::consts::PI {
                    diff += std::f32::consts::TAU;
                }
                let max_turn = e.turn_rate * TICK_DT;
                e.yaw += diff.clamp(-max_turn, max_turn);
            }
        }
        let ease = (6.0 * TICK_DT).min(1.0);
        e.scale = e.scale + (e.scale_target - e.scale) * ease;
    }
    // Part easing runs only while a move_part animation is live; on arrival the
    // part snaps to its target and settles, making it slab-eligible again.
    let mut settled_owners: Vec<u64> = Vec::new();
    for part in world.parts.iter_mut() {
        if !part.anim_active {
            continue;
        }
        let ease = (part.rate * TICK_DT).min(1.0);
        part.offset = part.offset + (part.target_offset - part.offset) * ease;
        part.rot = part.rot + (part.target_rot - part.rot) * ease;
        part.half = part.half + (part.target_half - part.half) * ease;
        let remaining = (part.target_offset - part.offset).length()
            + (part.target_rot - part.rot).length()
            + (part.target_half - part.half).length();
        if remaining < 1.0e-3 {
            part.offset = part.target_offset;
            part.rot = part.target_rot;
            part.half = part.target_half;
            part.anim_active = false;
            settled_owners.push(part.owner);
        }
    }
    if !settled_owners.is_empty()
        && settled_owners.iter().any(|o| world.is_static_visual(*o))
    {
        // A static owner's decoration finished moving: it re-enters the slab.
        world.mark_render_dirty();
    }

    // Projectile lifetimes: `life` seconds, then gone.
    let mut expired = false;
    for e in world.entities.iter_mut() {
        if e.life > 0.0 {
            e.life -= TICK_DT;
            if e.life <= 0.0 {
                e.life = f32::NEG_INFINITY;
                expired = true;
            }
        }
    }
    if expired {
        world.entities.retain(|e| e.life != f32::NEG_INFINITY);
    }

    // Decoration follows its owner out (lifetime, game.remove, whatever).
    if !world.parts.is_empty() || !world.labels.is_empty() {
        let ids: HashSet<u64> = world.entities.iter().map(|e| e.id).collect();
        world.parts.retain(|p| ids.contains(&p.owner));
        world.labels.retain(|l| ids.contains(&l.owner));
    }

    // box3d dynamics layer (M1a): reconcile the body mirror against the
    // post-retain entity set, step the solver, read rigid poses back. Runs
    // LAST so a rigid spawned/removed/rolled-back this tick is already
    // settled in the entity list. Movers never touch this path.
    {
        let GameWorld {
            dynamics,
            entities,
            terrain,
            gravity,
            ..
        } = world;
        crate::dynamics::reconcile(dynamics, entities, terrain.as_ref(), *gravity);
        crate::dynamics::step_dynamics(dynamics, entities);
    }
}

pub fn collect_touches(world: &GameWorld) -> Vec<(u64, u64)> {
    let mut touches = Vec::new();
    for sensor in world.entities.iter().filter(|e| e.sensor) {
        // Rigid inclusion is additive: with no Rigid entities the pair set is
        // identical to the pre-M1a scan (tape parity).
        for other in world
            .entities
            .iter()
            .filter(|e| matches!(e.kind, BodyKind::Mover | BodyKind::Rigid))
        {
            if overlaps(sensor.pos, sensor.half, other.pos, other.half) {
                touches.push((sensor.id, other.id));
            }
        }
    }
    // `hits` entities (projectiles) report movers/kinematics they overlap —
    // movers pass through each other spatially, so overlap IS the hit — plus
    // whatever solid the sweep stopped them against this tick.
    for hitter in world.entities.iter().filter(|e| e.hits) {
        if hitter.hit_wall != 0 {
            touches.push((hitter.id, hitter.hit_wall));
        }
        for other in world.entities.iter() {
            if other.id == hitter.id
                || other.sensor
                || other.hits
                || !matches!(
                    other.kind,
                    BodyKind::Mover | BodyKind::Kinematic | BodyKind::Rigid
                )
            {
                continue;
            }
            if overlaps(hitter.pos, hitter.half, other.pos, other.half) {
                touches.push((hitter.id, other.id));
            }
        }
    }
    touches
}

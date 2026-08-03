//! Golden world-state hashes for the kinematic/mover path — the half of
//! `step_world` that the rigid-dynamics goldens don't reach: terrain cliffs
//! and floors, static/kinematic sweeps, platform carry, rider attachment,
//! projectile lifetimes and auto-facing.
//!
//! This is the regression gate for optimisation work in `step_world`. Any
//! change that alters an observable result moves these hashes; a pure perf
//! change must leave them alone. (The snapshot ordering matters: movers sweep
//! against the kinematic poses from BEFORE this tick's integration, so an
//! "obvious" simplification that reads live entity positions instead of the
//! snapshot will be caught here.)

use makepad_game_sim::*;
use makepad_math::*;

fn fnv(h: u64, v: u32) -> u64 {
    (h ^ v as u64).wrapping_mul(0x100000001b3)
}

/// Hashes everything the mover path can influence, including the flags —
/// `on_floor`/`floor_id`/`hit_wall` are contract surface that scripts read.
fn hash_world(w: &GameWorld) -> u64 {
    let mut h = 0xcbf29ce484222325;
    for e in &w.entities {
        for f in [
            e.pos.x, e.pos.y, e.pos.z, e.vel.x, e.vel.y, e.vel.z, e.yaw, e.scale.x,
        ] {
            h = fnv(h, f.to_bits());
        }
        h = fnv(h, e.id as u32);
        h = fnv(h, e.on_floor as u32);
        h = fnv(h, e.floor_id as u32);
        h = fnv(h, e.hit_wall as u32);
        h = fnv(h, e.attached_to as u32);
    }
    h
}

fn ent(id: u64, kind: BodyKind, pos: Vec3f, half: Vec3f) -> Entity {
    Entity {
        id,
        kind,
        pos,
        half,
        collide: true,
        gravity_scale: 1.0,
        density: 1.0,
        friction: 0.6,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        speed_mult: 1.0,
        ..Default::default()
    }
}

/// A ridged heightfield: hills tall enough to be cliffs (blocking sideways
/// movement) and slopes shallow enough to walk up, so both terrain branches
/// are exercised rather than just the flat floor.
fn ridged_terrain(cells: usize) -> Terrain {
    let cell_size = 1.0;
    let origin = -(cells as f32 * cell_size) * 0.5;
    let mut heights = Vec::with_capacity(cells * cells);
    let mut colors = Vec::with_capacity(cells * cells);
    for z in 0..cells {
        for x in 0..cells {
            let fx = x as f32 * 0.35;
            let fz = z as f32 * 0.21;
            // Deterministic ridges: no rng, no libm beyond the sim's own.
            let hgt = (fx.sin() * 1.4 + fz.cos() * 0.9 + 1.0).max(0.0);
            heights.push(hgt);
            colors.push(vec4f(0.35, 0.55, 0.3, 1.0));
        }
    }
    Terrain { cells, cell_size, origin, heights, colors, revision: 1 }
}

/// Terrain + walls + a moving platform + walkers + a rider + projectiles.
fn mover_scene() -> GameWorld {
    let mut w = GameWorld::new();
    w.reset_content();
    w.terrain = Some(ridged_terrain(33));
    let mut id = 0u64;

    // Static walls in the walkers' path.
    for i in 0..6 {
        id += 1;
        w.push_entity(ent(
            id,
            BodyKind::Static,
            vec3f(-4.0 + i as f32 * 2.5, 1.0, 3.0),
            vec3f(0.6, 1.0, 0.4),
        ));
    }

    // Kinematic platform: movers standing on it must be carried.
    id += 1;
    let mut plat = ent(id, BodyKind::Kinematic, vec3f(0.0, 2.0, -6.0), vec3f(3.0, 0.3, 3.0));
    plat.vel = vec3f(1.2, 0.0, 0.0);
    let platform_id = id;
    w.push_entity(plat);

    // A rider standing on the platform.
    id += 1;
    let mut rider = ent(id, BodyKind::Mover, vec3f(0.0, 3.0, -6.0), vec3f(0.35, 0.7, 0.35));
    rider.auto_face = true;
    rider.turn_rate = 5.0;
    w.push_entity(rider);

    // Walkers driving into the walls and up the ridges.
    for i in 0..5 {
        id += 1;
        let mut m = ent(
            id,
            BodyKind::Mover,
            vec3f(-6.0 + i as f32 * 1.7, 4.0, 0.0),
            vec3f(0.4, 0.8, 0.4),
        );
        m.vel = vec3f(1.5 + i as f32 * 0.2, 0.0, 1.1);
        m.auto_face = true;
        m.turn_rate = 6.0;
        m.scale_target = vec3f(1.2, 1.2, 1.2);
        w.push_entity(m);
    }

    // A passenger attached to the platform (attach pin path).
    id += 1;
    let mut seat = ent(id, BodyKind::Mover, vec3f(0.0, 3.0, -6.0), vec3f(0.3, 0.6, 0.3));
    seat.attached_to = platform_id;
    seat.attach_offset = vec3f(0.5, 0.9, 0.0);
    w.push_entity(seat);

    // Projectiles: lifetimes expire mid-run, exercising the retain path.
    for i in 0..4 {
        id += 1;
        let mut p = ent(
            id,
            BodyKind::Mover,
            vec3f(-2.0 + i as f32, 5.0, -1.0),
            vec3f(0.15, 0.15, 0.15),
        );
        p.vel = vec3f(2.0, 1.0, -0.5);
        p.hits = true;
        p.life = 1.0 + i as f32 * 0.4;
        w.push_entity(p);
    }
    w
}

#[test]
fn mover_path_matches_golden() {
    let mut w = mover_scene();
    for _ in 0..600 {
        step_world(&mut w);
        let _ = collect_touches(&w);
    }
    let got = hash_world(&w);
    // Recorded on aarch64. Re-baselined once, deliberately: CONTACT_SKIN made
    // resting contact stop a hair short of flush, which shifts every clamped
    // position by 1e-3. That was a bug fix — flush contact let a falling mover
    // be resolved UP onto a crate it had walked into, ignoring the 0.55
    // step-up contract — so the movement it describes is now correct, not
    // merely different. Any FURTHER change to this hash needs the same
    // justification.
    assert_eq!(
        got, 0x4461e8c70ceaf60e,
        "mover/terrain/attach path changed observable results (got {got:#x})"
    );
}

#[test]
fn mover_path_is_run_to_run_deterministic() {
    let mut a = mover_scene();
    let mut b = mover_scene();
    for _ in 0..300 {
        step_world(&mut a);
        step_world(&mut b);
    }
    assert_eq!(hash_world(&a), hash_world(&b));
}

/// The snapshot must predate this tick's kinematic integration: a mover riding
/// a platform is carried by the platform's velocity, not teleported by its
/// already-advanced position. Guards the ordering an optimisation could break.
#[test]
fn platform_carry_uses_pre_integration_snapshot() {
    let mut w = GameWorld::new();
    w.reset_content();
    w.push_entity(ent(1, BodyKind::Static, vec3f(0.0, -0.5, 0.0), vec3f(20.0, 0.5, 20.0)));
    let mut plat = ent(2, BodyKind::Kinematic, vec3f(0.0, 1.0, 0.0), vec3f(2.0, 0.5, 2.0));
    plat.vel = vec3f(2.0, 0.0, 0.0);
    w.push_entity(plat);
    let rider = ent(3, BodyKind::Mover, vec3f(0.0, 2.0, 0.0), vec3f(0.3, 0.5, 0.3));
    w.push_entity(rider);

    // Settle the rider onto the platform.
    for _ in 0..30 {
        step_world(&mut w);
    }
    let before = w.entity(3).unwrap().pos.x;
    let carried_ticks = 10;
    for _ in 0..carried_ticks {
        step_world(&mut w);
    }
    let after = w.entity(3).unwrap().pos.x;
    let moved = after - before;
    // Carried at the platform's 2.0 u/s for `carried_ticks` at 60Hz.
    let expected = 2.0 * TICK_DT * carried_ticks as f32;
    assert!(
        (moved - expected).abs() < 0.02,
        "rider should be carried {expected} but moved {moved}"
    );
}

/// A character must not walk through a rigid body. Rigids were absent from the
/// mover sweep set when box3d dynamics landed, so the Knight strolled straight
/// through a crate stack — the most obvious "this world isn't real" tell.
/// Their pose is read back from box3d at the end of the previous tick, so at
/// snapshot time a rigid is as settled as a kinematic.
#[test]
fn mover_is_blocked_by_a_rigid_body() {
    let mut w = GameWorld::new();
    w.reset_content();
    w.push_entity(ent(1, BodyKind::Static, vec3f(0.0, -0.5, 0.0), vec3f(20.0, 0.5, 20.0)));
    // A crate sitting on the ground, directly in the walker's path.
    w.push_entity(ent(2, BodyKind::Rigid, vec3f(2.0, 0.5, 0.0), vec3f(0.5, 0.5, 0.5)));
    let mut walker = ent(3, BodyKind::Mover, vec3f(0.0, 0.5, 0.0), vec3f(0.3, 0.5, 0.3));
    walker.vel = vec3f(3.0, 0.0, 0.0);
    w.push_entity(walker);

    for _ in 0..60 {
        // Walk is a velocity the script re-asserts each tick.
        w.entity_mut(3).unwrap().vel.x = 3.0;
        step_world(&mut w);
    }

    let x = w.entity(3).unwrap().pos.x;
    // Unblocked it would reach x = 3.0; it must stop at the crate's near face
    // (2.0 - 0.5 crate half - 0.3 walker half = 1.2).
    assert!(
        x < 1.35,
        "walker should stop at the crate's face (~1.2) but reached {x}"
    );
}

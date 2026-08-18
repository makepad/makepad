//! Gates for the mix sim-core work (mix.md F6-F10):
//!
//! - F6  movers mirror into box3d as kinematic capsules → a ray SEES a
//!   moving player, at its current position, not its spawn.
//! - F7  `world_raycast` runs on box3d (exact normals/distances, terrain,
//!   decor, sensors skipped) — the 0.15 march is gone.
//! - F8  capsule↔rigid contact events → mover impulse + `knocked_down`,
//!   mass-ratio scaled, deterministic to a golden hash; movers push light
//!   rigids.
//! - F9  fast projectiles cannot tunnel (shapecast CCD).
//! - F10 per-rigid acceleration is readable; REAL terrain material indices
//!   and per-entity materials come back from queries.

use makepad_game_sim::*;
use makepad_math::*;

fn fnv(h: u64, v: u32) -> u64 {
    (h ^ v as u64).wrapping_mul(0x100000001b3)
}

/// Everything the contact pipeline can influence, knockdown included.
fn hash_world(w: &GameWorld) -> u64 {
    let mut h = 0xcbf29ce484222325;
    for e in &w.entities {
        for f in [
            e.pos.x, e.pos.y, e.pos.z, e.vel.x, e.vel.y, e.vel.z, e.orient.x, e.orient.y,
            e.orient.z, e.orient.w,
        ] {
            h = fnv(h, f.to_bits());
        }
        h = fnv(h, e.id as u32);
        h = fnv(h, e.knocked_down as u32);
        h = fnv(h, e.hit_wall as u32);
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
        restitution: 0.0,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        speed_mult: 1.0,
        push_mass: 1.0,
        ..Default::default()
    }
}

fn ground(w: &mut GameWorld) {
    w.push_entity(ent(
        1,
        BodyKind::Static,
        vec3f(0.0, -0.5, 0.0),
        vec3f(60.0, 0.5, 60.0),
    ));
}

// ------------------------------------------------------------------- F6/F7

/// The F6 gate: an exact box3d ray finds a WALKING mover where it stands
/// now. Casts at head height from the side; also proves the capsule moved
/// (a cast at the spawn position finds nothing).
#[test]
fn box3d_ray_sees_a_moving_mover() {
    let mut w = GameWorld::new();
    w.reset_content();
    ground(&mut w);
    let mut m = ent(2, BodyKind::Mover, vec3f(0.0, 0.8, 0.0), vec3f(0.35, 0.8, 0.35));
    m.vel = vec3f(3.0, 0.0, 0.0);
    w.push_entity(m);

    for _ in 0..60 {
        w.entity_mut(2).unwrap().vel.x = 3.0;
        step_world(&mut w);
    }
    let pos = w.entity(2).unwrap().pos;
    assert!(pos.x > 2.5, "walker should have walked, at {pos:?}");

    // Side-on ray at the walker's current x: must hit the capsule.
    let hit = world_raycast(&mut w, vec3f(pos.x, pos.y, -5.0), vec3f(0.0, 0.0, 1.0), 20.0)
        .expect("ray should hit the walking mover");
    let (id, point, normal, dist, _mat) = hit;
    assert_eq!(id, 2, "ray hit {id}, wanted the mover");
    // Exact contact: the capsule surface is radius (0.35) in front of its
    // center, and the normal faces the ray.
    assert!(
        (dist - (5.0 + pos.z - 0.35)).abs() < 1.0e-3,
        "dist {dist} should be exact to the capsule surface"
    );
    assert!(normal.z < -0.99, "true surface normal, got {normal:?}");
    assert!((point.z - (pos.z - 0.35)).abs() < 1.0e-3);

    // The spawn position is empty now — the mirror FOLLOWED the mover.
    assert!(
        world_raycast(&mut w, vec3f(0.0, pos.y, -5.0), vec3f(0.0, 0.0, 1.0), 20.0)
            .map_or(true, |(id, ..)| id != 2),
        "stale capsule left at the spawn position"
    );
}

/// F7 contract preservation: `collide:false` decor is visually solid and
/// still hittable (now as a query-only mirror shape); sensors stay
/// invisible; terrain reports TERRAIN_ID with a true surface normal.
#[test]
fn raycast_hits_decor_skips_sensors_reports_terrain() {
    let mut w = GameWorld::new();
    w.reset_content();
    // Flat terrain instead of a ground slab.
    let cells = 17usize;
    let cell_size = 2.0;
    let origin = -(cells as f32 - 1.0) * cell_size * 0.5;
    w.terrain = Some(Terrain {
        cells,
        cell_size,
        origin,
        heights: vec![0.0; cells * cells],
        colors: Vec::new(),
        revision: 1,
    });
    // A sensor directly in the ray's path — must be ignored.
    let mut sensor = ent(1, BodyKind::Static, vec3f(0.0, 1.0, 4.0), vec3f(1.0, 1.0, 0.5));
    sensor.sensor = true;
    w.push_entity(sensor);
    // Decor (collide:false) behind it — must be hit.
    let mut decor = ent(2, BodyKind::Static, vec3f(0.0, 1.0, 8.0), vec3f(1.0, 1.0, 0.5));
    decor.collide = false;
    w.push_entity(decor);

    let (id, _p, normal, dist, _m) =
        world_raycast(&mut w, vec3f(0.0, 1.0, 0.0), vec3f(0.0, 0.0, 1.0), 30.0)
            .expect("decor should be hit");
    assert_eq!(id, 2, "ray must skip the sensor and hit the decor");
    assert!((dist - 7.5).abs() < 1.0e-3, "exact decor face at 7.5, got {dist}");
    assert!(normal.z < -0.99);

    // Straight down: terrain, sentinel id, up normal.
    let (id, point, normal, _d, _m) =
        world_raycast(&mut w, vec3f(3.0, 5.0, 3.0), vec3f(0.0, -1.0, 0.0), 30.0)
            .expect("terrain should be hit");
    assert_eq!(id, TERRAIN_ID);
    assert!(point.y.abs() < 1.0e-3);
    assert!(normal.y > 0.99);
}

/// A ray that starts inside the caster's own body no longer reports the
/// caster (the march did; FPS eye rays had to skip their own id). It reports
/// what is BEYOND instead.
#[test]
fn raycast_from_inside_own_body_sees_past_it() {
    let mut w = GameWorld::new();
    w.reset_content();
    ground(&mut w);
    w.push_entity(ent(2, BodyKind::Mover, vec3f(0.0, 0.8, 0.0), vec3f(0.35, 0.8, 0.35)));
    w.push_entity(ent(3, BodyKind::Static, vec3f(0.0, 1.0, 6.0), vec3f(1.0, 1.0, 0.5)));
    for _ in 0..5 {
        step_world(&mut w);
    }
    let eye = w.entity(2).unwrap().pos + vec3f(0.0, 0.5, 0.0);
    let (id, ..) = world_raycast(&mut w, eye, vec3f(0.0, 0.0, 1.0), 30.0)
        .expect("should hit the wall beyond");
    assert_eq!(id, 3, "eye ray must not hit the caster's own capsule");
}

// --------------------------------------------------------------------- F8

/// The F8 gate: a rigid "car" at speed hits a standing walker. The walker
/// takes a mass-ratio-scaled impulse with vertical pop and a knocked_down
/// timer, deterministically — double-run identical AND pinned to a golden.
#[test]
fn car_hits_walker_sends_them_flying_golden() {
    fn scenario() -> (GameWorld, u64) {
        let mut w = GameWorld::new();
        w.reset_content();
        ground(&mut w);
        // A heavy sliding box plays the car (density 2, ~7.4 mass units).
        let mut car = ent(2, BodyKind::Rigid, vec3f(-12.0, 0.45, 0.0), vec3f(0.9, 0.4, 1.6));
        car.density = 2.0;
        car.friction = 0.1;
        car.vel = vec3f(18.0, 0.0, 0.0);
        w.push_entity(car);
        let walker = ent(3, BodyKind::Mover, vec3f(0.0, 0.8, 0.0), vec3f(0.35, 0.8, 0.35));
        w.push_entity(walker);

        let mut max_knock = 0u16;
        for _ in 0..180 {
            step_world(&mut w);
            max_knock = max_knock.max(w.entity(3).map_or(0, |e| e.knocked_down));
        }
        (w, max_knock as u64)
    }
    let (a, knock_a) = scenario();
    let (b, knock_b) = scenario();
    assert_eq!(hash_world(&a), hash_world(&b), "car-hits-walker diverged between runs");
    assert!(knock_a > 0, "walker was never knocked down");
    assert_eq!(knock_a, knock_b);

    let walker = a.entity(3).unwrap();
    assert!(
        walker.pos.x > 3.0,
        "walker should be sent flying downrange, only reached {:?}",
        walker.pos
    );
    // Recorded on aarch64 (2026-08-11). Equality elsewhere is the cross-arch
    // determinism claim for the whole contact pipeline. Re-record via
    // --nocapture println if the impulse constants change deliberately.
    let got = hash_world(&a);
    println!("car-hits-walker golden hash: {got:#018x}");
    const GOLDEN: u64 = 0x6819478b276cd7fc;
    assert_eq!(got, GOLDEN, "car-hits-walker golden changed (got {got:#x})");
}

/// D4's other direction: a walker walking into a LIGHT crate shoves it
/// along; the walker itself is not bounced backward (the sweep owns the
/// walker's blocking).
#[test]
fn walker_pushes_a_light_crate() {
    let mut w = GameWorld::new();
    w.reset_content();
    ground(&mut w);
    let mut crate_e = ent(2, BodyKind::Rigid, vec3f(2.0, 0.5, 0.0), vec3f(0.5, 0.5, 0.5));
    crate_e.density = 0.2; // light: 1 m³ * 0.2
    crate_e.friction = 0.2;
    w.push_entity(crate_e);
    let walker = ent(3, BodyKind::Mover, vec3f(0.0, 0.8, 0.0), vec3f(0.35, 0.8, 0.35));
    w.push_entity(walker);

    for _ in 0..240 {
        // The walk is a velocity the controller re-asserts each tick.
        w.entity_mut(3).unwrap().vel.x = 3.0;
        step_world(&mut w);
    }
    let crate_x = w.entity(2).unwrap().pos.x;
    let walker_x = w.entity(3).unwrap().pos.x;
    assert!(
        crate_x > 3.0,
        "light crate should have been shoved along, still at x {crate_x}"
    );
    assert!(
        walker_x > 1.0,
        "walker should advance behind the crate, at x {walker_x}"
    );
    // And nobody got floored by a walking-speed shove.
    assert_eq!(w.entity(3).unwrap().knocked_down, 0);
}

/// Pending contacts are world state: a snapshot taken between ticks replays
/// the same impulses — clone and original stay bit-identical through the
/// collision.
#[test]
fn clone_replays_the_same_collision() {
    let mut w = GameWorld::new();
    w.reset_content();
    ground(&mut w);
    let mut car = ent(2, BodyKind::Rigid, vec3f(-6.0, 0.45, 0.0), vec3f(0.9, 0.4, 1.6));
    car.density = 2.0;
    car.friction = 0.1;
    car.vel = vec3f(18.0, 0.0, 0.0);
    w.push_entity(car);
    w.push_entity(ent(3, BodyKind::Mover, vec3f(0.0, 0.8, 0.0), vec3f(0.35, 0.8, 0.35)));

    for _ in 0..20 {
        step_world(&mut w);
    }
    let mut c = w.clone();
    for _ in 0..120 {
        step_world(&mut w);
        step_world(&mut c);
    }
    assert_eq!(
        hash_world(&w),
        hash_world(&c),
        "clone diverged through the collision"
    );
}

// --------------------------------------------------------------------- F9

/// The F9 gate: a projectile at extreme speed must stop at a thin wall, not
/// pass through it. 300 u/s = 5 units per tick against a 0.2-thick wall the
/// axis sweeps would step straight over.
#[test]
fn high_speed_projectile_never_tunnels() {
    let mut w = GameWorld::new();
    w.reset_content();
    ground(&mut w);
    // Thin wall at x = 10.
    w.push_entity(ent(2, BodyKind::Static, vec3f(10.0, 2.0, 0.0), vec3f(0.1, 2.0, 4.0)));
    let mut p = ent(3, BodyKind::Mover, vec3f(0.0, 2.0, 0.0), vec3f(0.15, 0.15, 0.15));
    p.vel = vec3f(300.0, 0.0, 0.0);
    p.gravity_scale = 0.0;
    p.hits = true;
    p.life = 5.0;
    w.push_entity(p);

    // hit_wall is transient (reset each tick, read by collect_touches every
    // tick) — capture it on the tick it fires.
    let mut reported = 0u64;
    for _ in 0..30 {
        step_world(&mut w);
        let hw = w.entity(3).map_or(0, |e| e.hit_wall);
        if hw != 0 {
            reported = hw;
        }
    }
    let p = w.entity(3).expect("projectile should still exist");
    assert!(
        p.pos.x < 10.0,
        "projectile tunneled through the wall to x {}",
        p.pos.x
    );
    assert!(
        p.pos.x > 9.0,
        "projectile should be AT the wall, stopped at x {}",
        p.pos.x
    );
    assert_eq!(reported, 2, "the wall hit must be reported for on_touch");
    assert_eq!(p.vel.x, 0.0);
}

/// A fast projectile stops at a MOVER too (D4: projectile → mover = hit,
/// via the capsule mirror) and reports the victim's id.
#[test]
fn high_speed_projectile_hits_a_mover() {
    let mut w = GameWorld::new();
    w.reset_content();
    ground(&mut w);
    w.push_entity(ent(2, BodyKind::Mover, vec3f(12.0, 0.8, 0.0), vec3f(0.35, 0.8, 0.35)));
    let mut p = ent(3, BodyKind::Mover, vec3f(0.0, 1.0, 0.0), vec3f(0.1, 0.1, 0.1));
    p.vel = vec3f(240.0, 0.0, 0.0);
    p.gravity_scale = 0.0;
    p.hits = true;
    p.life = 5.0;
    w.push_entity(p);

    // Let the victim's capsule exist (mirror reconciles during the tick),
    // then fire through. hit_wall is transient — capture it when it fires.
    let mut reported = 0u64;
    for _ in 0..30 {
        step_world(&mut w);
        let hw = w.entity(3).map_or(0, |e| e.hit_wall);
        if hw != 0 {
            reported = hw;
        }
    }
    let p = w.entity(3).expect("projectile should still exist");
    assert!(
        p.pos.x < 12.0 && p.pos.x > 10.5,
        "projectile should stop at the mover capsule, at x {}",
        p.pos.x
    );
    assert_eq!(reported, 2, "the mover hit must be reported");
}

// -------------------------------------------------------------------- F10

/// Per-rigid acceleration from the cached prev_vel: free fall reads the
/// gravity vector; resting reads ~zero.
#[test]
fn rigid_acceleration_is_readable() {
    let mut w = GameWorld::new();
    w.reset_content();
    ground(&mut w);
    w.push_entity(ent(2, BodyKind::Rigid, vec3f(0.0, 20.0, 0.0), vec3f(0.4, 0.4, 0.4)));

    // Two steps so prev_vel and vel are both post-solver values.
    step_world(&mut w);
    step_world(&mut w);
    let a = w.dynamics.rigid_accel(2).expect("accel should be readable");
    assert!(
        (a.y + 30.0).abs() < 1.5,
        "free fall should read ~-30 (world gravity), got {a:?}"
    );

    // Let it land and settle: acceleration returns to ~0.
    for _ in 0..240 {
        step_world(&mut w);
    }
    let a = w.dynamics.rigid_accel(2).expect("accel still readable");
    assert!(a.length() < 0.5, "resting accel should be ~0, got {a:?}");

    // Movers have no rigid accel.
    w.push_entity(ent(9, BodyKind::Mover, vec3f(3.0, 1.0, 0.0), vec3f(0.3, 0.5, 0.3)));
    step_world(&mut w);
    assert!(w.dynamics.rigid_accel(9).is_none());
}

/// REAL terrain material indices (B6 fixed): a two-surface terrain reports
/// which half the ray hit; a per-entity material comes back from props.
#[test]
fn material_indices_reach_queries() {
    let mut w = GameWorld::new();
    w.reset_content();
    let cells = 17usize;
    let cell_size = 2.0;
    let origin = -(cells as f32 - 1.0) * cell_size * 0.5;
    w.terrain = Some(Terrain {
        cells,
        cell_size,
        origin,
        heights: vec![0.0; cells * cells],
        colors: Vec::new(),
        revision: 1,
    });
    // Left half tarmac (0), right half gravel (1).
    let side = cells - 1;
    let mut indices = vec![0u8; side * side];
    for z in 0..side {
        for x in 0..side {
            if x >= side / 2 {
                indices[z * side + x] = 1;
            }
        }
    }
    w.terrain_materials = Some(TerrainMaterials {
        indices,
        surfaces: vec![
            TerrainSurface { friction: 0.9, restitution: 0.0 },
            TerrainSurface { friction: 0.4, restitution: 0.0 },
        ],
    });
    // A prop with a per-entity material id.
    let mut prop = ent(1, BodyKind::Static, vec3f(0.0, 1.0, 10.0), vec3f(1.0, 1.0, 1.0));
    prop.material = 7;
    w.push_entity(prop);

    let (id, _p, _n, _d, mat) =
        world_raycast(&mut w, vec3f(-10.0, 5.0, 0.0), vec3f(0.0, -1.0, 0.0), 20.0)
            .expect("left terrain hit");
    assert_eq!(id, TERRAIN_ID);
    assert_eq!(mat, 0, "left half should be material 0");

    let (id, _p, _n, _d, mat) =
        world_raycast(&mut w, vec3f(10.0, 5.0, 0.0), vec3f(0.0, -1.0, 0.0), 20.0)
            .expect("right terrain hit");
    assert_eq!(id, TERRAIN_ID);
    assert_eq!(mat, 1, "right half should be material 1");

    let (id, _p, _n, _d, mat) =
        world_raycast(&mut w, vec3f(0.0, 1.0, 0.0), vec3f(0.0, 0.0, 1.0), 20.0)
            .expect("prop hit");
    assert_eq!(id, 1);
    assert_eq!(mat, 7, "per-entity material should come back from the ray");
}

/// Different terrain materials produce different PHYSICS: the same crate
/// slides further on the slick half than on the grippy half.
#[test]
fn terrain_materials_change_friction() {
    fn slide_distance(indices_value: u8) -> f32 {
        let mut w = GameWorld::new();
        w.reset_content();
        let cells = 33usize;
        let cell_size = 2.0;
        let origin = -(cells as f32 - 1.0) * cell_size * 0.5;
        w.terrain = Some(Terrain {
            cells,
            cell_size,
            origin,
            heights: vec![0.0; cells * cells],
            colors: Vec::new(),
            revision: 1,
        });
        let side = cells - 1;
        w.terrain_materials = Some(TerrainMaterials {
            indices: vec![indices_value; side * side],
            surfaces: vec![
                TerrainSurface { friction: 1.2, restitution: 0.0 },
                TerrainSurface { friction: 0.05, restitution: 0.0 },
            ],
        });
        let mut crate_e = ent(1, BodyKind::Rigid, vec3f(-20.0, 0.5, 0.0), vec3f(0.5, 0.5, 0.5));
        crate_e.friction = 0.6;
        crate_e.vel = vec3f(12.0, 0.0, 0.0);
        w.push_entity(crate_e);
        for _ in 0..300 {
            step_world(&mut w);
        }
        w.entity(1).unwrap().pos.x - (-20.0)
    }
    let grippy = slide_distance(0);
    let slick = slide_distance(1);
    assert!(
        slick > grippy + 5.0,
        "low-friction surface should slide much further (grippy {grippy:.2}, slick {slick:.2})"
    );
}

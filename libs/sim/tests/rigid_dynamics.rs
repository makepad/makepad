//! M1a rigid-dynamics gates: determinism (double-run + recorded golden),
//! bit-exact Clone via the box3d snapshot, heightfield rest position (also
//! validates the terrain→box3d transform orientation), reconcile lifecycle,
//! and a perf smoke (run with --release --nocapture for the real number).

use makepad_game_sim::*;
use makepad_math::*;

fn fnv(h: u64, v: u32) -> u64 {
    (h ^ v as u64).wrapping_mul(0x100000001b3)
}

fn hash_world(w: &GameWorld) -> u64 {
    let mut h = 0xcbf29ce484222325;
    for e in &w.entities {
        for f in [
            e.pos.x, e.pos.y, e.pos.z, e.vel.x, e.vel.y, e.vel.z, e.orient.x, e.orient.y,
            e.orient.z, e.orient.w,
        ] {
            h = fnv(h, f.to_bits());
        }
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
        ..Default::default()
    }
}

/// Ground slab + offset crate stack + two spheres, one kinematic platform.
fn rigid_scene() -> GameWorld {
    let mut w = GameWorld::new();
    w.reset_content();
    let mut id = 0;
    id += 1;
    w.push_entity(ent(id, BodyKind::Static, vec3f(0.0, -0.5, 0.0), vec3f(30.0, 0.5, 30.0)));
    for i in 0..6 {
        id += 1;
        let jitter = (i as f32) * 0.03;
        let mut e = ent(
            id,
            BodyKind::Rigid,
            vec3f(jitter, 0.5 + i as f32 * 1.02, jitter * 0.7),
            vec3f(0.5, 0.5, 0.5),
        );
        e.restitution = 0.05;
        w.push_entity(e);
    }
    for i in 0..2 {
        id += 1;
        let mut e = ent(
            id,
            BodyKind::Rigid,
            vec3f(3.0 + i as f32 * 1.5, 4.0, -2.0),
            vec3f(0.6, 0.6, 0.6),
        );
        e.shape = Shape::Sphere;
        e.vel = vec3f(-1.0, 0.0, 0.5);
        e.restitution = 0.4;
        w.push_entity(e);
    }
    id += 1;
    let mut plat = ent(id, BodyKind::Kinematic, vec3f(-4.0, 1.0, 4.0), vec3f(2.0, 0.25, 2.0));
    plat.vel = vec3f(0.5, 0.0, 0.0);
    w.push_entity(plat);
    w.next_id = id;
    w
}

fn run(w: &mut GameWorld, ticks: usize) {
    for _ in 0..ticks {
        step_world(w);
    }
}

#[test]
fn rigid_determinism_double_run_matches_golden() {
    let mut a = rigid_scene();
    let mut b = rigid_scene();
    run(&mut a, 600);
    run(&mut b, 600);
    let (ha, hb) = (hash_world(&a), hash_world(&b));
    assert_eq!(ha, hb, "two identical runs diverged");
    // Recorded on aarch64 (2026-08-02). Equality on every other target IS the
    // cross-arch determinism claim for the whole sim+box3d stack. If a solver
    // or scene change is intentional, re-record via --nocapture println.
    const GOLDEN: u64 = 0xa8a2baf71e4a564f;
    println!("rigid golden hash: {ha:#018x}");
    assert_eq!(ha, GOLDEN, "rigid determinism golden changed");
    // Stack must have settled, not exploded or sunk: every crate above the
    // slab top, below spawn ceiling.
    for e in a.entities.iter().filter(|e| e.kind == BodyKind::Rigid) {
        assert!(e.pos.y > -0.1 && e.pos.y < 8.0, "rigid escaped: {:?}", e.pos);
    }
}

#[test]
fn clone_is_bit_identical_going_forward() {
    let mut w = rigid_scene();
    run(&mut w, 300);
    let mut c = w.clone();
    assert_eq!(hash_world(&w), hash_world(&c), "clone changed state");
    run(&mut w, 120);
    run(&mut c, 120);
    assert_eq!(
        hash_world(&w),
        hash_world(&c),
        "clone diverged from original within 120 ticks"
    );
}

#[test]
fn rigid_rests_on_heightfield_terrain() {
    let mut w = GameWorld::new();
    w.reset_content();
    // Asymmetric ramp along +x: catches x/z transposition in the mirror.
    let cells = 17usize;
    let cell_size = 2.0f32;
    let origin = -(cells as f32 - 1.0) * cell_size * 0.5;
    let mut heights = vec![0.0f32; cells * cells];
    for z in 0..cells {
        for x in 0..cells {
            heights[z * cells + x] = x as f32 * 0.5;
        }
    }
    w.terrain = Some(Terrain {
        cells,
        cell_size,
        origin,
        heights,
        colors: Vec::new(),
        revision: 1,
    });
    let expect_low = w.terrain.as_ref().unwrap().height_at(-10.0, 0.0).unwrap();
    let expect_high = w.terrain.as_ref().unwrap().height_at(10.0, 0.0).unwrap();
    assert!(expect_high - expect_low > 3.0, "ramp fixture is flat?");
    w.next_id = 2;
    let mut low = ent(1, BodyKind::Rigid, vec3f(-10.0, expect_low + 6.0, 0.0), vec3f(0.4, 0.4, 0.4));
    low.friction = 2.0; // park on the slope instead of sliding off
    let mut high = ent(2, BodyKind::Rigid, vec3f(10.0, expect_high + 6.0, 0.0), vec3f(0.4, 0.4, 0.4));
    high.friction = 2.0;
    w.push_entity(low);
    w.push_entity(high);
    run(&mut w, 480);
    let e_low = w.entity(1).unwrap();
    let e_high = w.entity(2).unwrap();
    // Resting near the local surface (generous: they may settle slightly
    // downhill of the drop point on the slope).
    let terrain = w.terrain.as_ref().unwrap();
    for e in [e_low, e_high] {
        let ground = terrain
            .height_at(e.pos.x, e.pos.z)
            .expect("rigid slid off the terrain");
        assert!(
            (e.pos.y - ground).abs() < 1.5,
            "rigid not resting on terrain: pos {:?} ground {}",
            e.pos,
            ground
        );
    }
    // And the ramp ordering held: the high-side crate rests above the low one.
    assert!(e_high.pos.y > e_low.pos.y + 2.0, "x/z transposed in heightfield mirror?");
}

#[test]
fn reconcile_lifecycle_creates_and_destroys_bodies() {
    let mut w = rigid_scene();
    run(&mut w, 5);
    let full = w.dynamics.body_count();
    assert!(full >= 9, "expected mirrored bodies, got {full}");
    // remove all rigids (game.remove path = retain)
    w.entities.retain(|e| e.kind != BodyKind::Rigid);
    run(&mut w, 1);
    let after = w.dynamics.body_count();
    assert_eq!(after, full - 8, "rigid bodies not destroyed on retain");
    // reset drops everything
    w.reset_content();
    run(&mut w, 1);
    assert_eq!(w.dynamics.body_count(), 0);
}

#[test]
fn set_pos_and_set_vel_semantics_reach_box3d() {
    let mut w = rigid_scene();
    run(&mut w, 200);
    // Host-side set_pos semantics: teleport + zero velocity.
    {
        let e = w.entity_mut(2).unwrap();
        e.pos = vec3f(8.0, 6.0, 8.0);
        e.orient = Quat::default();
        e.vel = vec3f(0.0, 0.0, 0.0);
    }
    run(&mut w, 1);
    let e = w.entity(2).unwrap();
    assert!(
        (e.pos - vec3f(8.0, 6.0, 8.0)).length() < 1.0,
        "teleport ignored by box3d: {:?}",
        e.pos
    );
    // It should now be falling from the new spot.
    run(&mut w, 30);
    let e = w.entity(2).unwrap();
    assert!(e.pos.y < 6.0, "teleported rigid not simulated: {:?}", e.pos);
    // Let it land and settle before kicking (an impulse against a -15 m/s
    // fall correctly nets downward — learned from the first version).
    run(&mut w, 150);
    // Impulse helper: a solid upward kick must actually lift it.
    let before = w.entity(2).unwrap().pos.y;
    assert!(w.dynamics.rigid_impulse(2, vec3f(0.0, 12.0, 0.0)));
    run(&mut w, 12);
    assert!(
        w.entity(2).unwrap().pos.y > before + 0.5,
        "impulse had no effect"
    );
}

#[test]
fn perf_smoke_100_movers_50_rigids_terrain() {
    let mut w = GameWorld::new();
    w.reset_content();
    let cells = 65usize;
    let cell_size = 2.0f32;
    let origin = -(cells as f32 - 1.0) * cell_size * 0.5;
    let mut heights = vec![0.0f32; cells * cells];
    for z in 0..cells {
        for x in 0..cells {
            heights[z * cells + x] =
                ((x as f32 * 0.37).sin() + (z as f32 * 0.29).cos()) * 1.5;
        }
    }
    w.terrain = Some(Terrain {
        cells,
        cell_size,
        origin,
        heights,
        colors: Vec::new(),
        revision: 1,
    });
    let mut id = 0;
    for i in 0..20 {
        id += 1;
        w.push_entity(ent(
            id,
            BodyKind::Static,
            vec3f((i % 5) as f32 * 8.0 - 16.0, 1.0, (i / 5) as f32 * 8.0 - 16.0),
            vec3f(1.0, 1.0, 1.0),
        ));
    }
    for i in 0..100 {
        id += 1;
        let mut e = ent(
            id,
            BodyKind::Mover,
            vec3f((i % 10) as f32 * 3.0 - 15.0, 5.0, (i / 10) as f32 * 3.0 - 15.0),
            vec3f(0.4, 0.8, 0.4),
        );
        e.vel = vec3f(if i % 2 == 0 { 1.0 } else { -1.0 }, 0.0, 0.7);
        w.push_entity(e);
    }
    for i in 0..50 {
        id += 1;
        w.push_entity(ent(
            id,
            BodyKind::Rigid,
            vec3f(
                (i % 7) as f32 * 2.0 - 6.0,
                3.0 + (i / 7) as f32 * 1.5,
                (i % 5) as f32 * 2.0 + 10.0,
            ),
            vec3f(0.5, 0.5, 0.5),
        ));
    }
    w.next_id = id;
    let t0 = std::time::Instant::now();
    run(&mut w, 600);
    let total = t0.elapsed().as_secs_f64() * 1000.0;
    println!(
        "perf: 600 ticks (100 movers + 50 rigids + 65x65 terrain) = {:.2} ms total, {:.3} ms/tick",
        total,
        total / 600.0
    );
    // Debug builds are ~10x slower; the real target (<2ms/tick) is asserted
    // loosely here and read off a --release run for the report.
    let budget = if cfg!(debug_assertions) { 20.0 } else { 2.0 };
    assert!(
        total / 600.0 < budget,
        "tick budget blown: {:.3} ms/tick (budget {budget})",
        total / 600.0
    );
}

/// A generated game handed the solver an infinite extent and took the whole
/// process down. NaN is absorbed by `max`, but INFINITY survives it and
/// poisons every plane normal to NaN — box3d's face query then never beats its
/// -f32::MAX starting separation, leaves max_face_index at the -1 sentinel,
/// and that sentinel is cast through u8 into 255 and used as an array index.
/// The port is faithful to upstream C there (C reads garbage where Rust
/// panics), so the boundary is the right place to refuse the value.
#[test]
fn absurd_entity_dimensions_cannot_crash_the_solver() {
    for bad in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN, 0.0, -5.0] {
        let mut w = GameWorld::new();
        w.reset_content();
        w.push_entity(ent(1, BodyKind::Static, vec3f(0.0, -0.5, 0.0), vec3f(20.0, 0.5, 20.0)));
        let mut e = ent(2, BodyKind::Rigid, vec3f(0.0, 3.0, 0.0), vec3f(bad, bad, bad));
        e.density = bad;
        w.push_entity(e);
        // Must not panic, and must still be stepping a coherent world.
        for _ in 0..30 {
            step_world(&mut w);
        }
        let p = w.entity(2).unwrap().pos;
        assert!(
            p.x.is_finite() && p.y.is_finite() && p.z.is_finite(),
            "half {bad} left a non-finite pose {p:?}"
        );
    }
}

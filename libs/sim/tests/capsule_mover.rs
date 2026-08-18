//! P3: the opt-in capsule mover. `capsule_collider: true` routes a mover's
//! motion through box3d `world_collide_mover` + `solve_planes` — real wall
//! sliding on angled geometry, smooth corners, true contact normals — while
//! flag-off movers keep the AABB sweep byte-for-byte (mover_golden.rs is the
//! proof: its hash did not move when this landed).

use makepad_game_sim::*;
use makepad_math::*;

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

fn capsule_walker(id: u64, pos: Vec3f) -> Entity {
    let mut m = ent(id, BodyKind::Mover, pos, vec3f(0.35, 0.9, 0.35));
    m.capsule_collider = true;
    m
}

fn ground(w: &mut GameWorld) {
    w.push_entity(ent(
        1,
        BodyKind::Static,
        vec3f(0.0, -0.5, 0.0),
        vec3f(60.0, 0.5, 60.0),
    ));
}

fn fnv(h: u64, v: u32) -> u64 {
    (h ^ v as u64).wrapping_mul(0x100000001b3)
}

fn hash_world(w: &GameWorld) -> u64 {
    let mut h = 0xcbf29ce484222325;
    for e in &w.entities {
        for f in [
            e.pos.x,
            e.pos.y,
            e.pos.z,
            e.vel.x,
            e.vel.y,
            e.vel.z,
            e.floor_normal.x,
            e.floor_normal.y,
            e.floor_normal.z,
            e.wall_normal.x,
            e.wall_normal.z,
        ] {
            h = fnv(h, f.to_bits());
        }
        h = fnv(h, e.on_floor as u32);
        h = fnv(h, e.hit_wall as u32);
    }
    h
}

#[test]
fn a_capsule_mover_settles_on_the_ground_and_reports_a_floor() {
    let mut w = GameWorld::new();
    w.gravity = 30.0;
    ground(&mut w);
    w.push_entity(capsule_walker(2, vec3f(0.0, 3.0, 0.0)));
    for _ in 0..90 {
        step_world(&mut w);
    }
    let e = w.entity(2).unwrap();
    assert!(e.on_floor, "capsule never grounded (y {})", e.pos.y);
    assert_eq!(e.floor_id, 1);
    assert!(e.floor_normal.y > 0.99, "flat floor normal {:?}", e.floor_normal);
    // Resting roughly a half-height above the slab's top face.
    assert!(
        (e.pos.y - 0.9).abs() < 0.1,
        "resting at y {} instead of ~0.9",
        e.pos.y
    );
}

/// The gate: wall-slide along an ANGLED wall. The AABB sweep collides a
/// rotated wall as its unrotated bounding box and kills the whole axis; the
/// capsule meets the true rotated face and keeps its tangential speed —
/// walking square into a 0.4-rad wall must carry the mover sideways.
#[test]
fn walking_into_an_angled_wall_slides_along_it() {
    let mut w = GameWorld::new();
    w.gravity = 30.0;
    ground(&mut w);
    let mut wall = ent(2, BodyKind::Static, vec3f(4.0, 1.5, 0.0), vec3f(0.3, 1.5, 10.0));
    wall.yaw = 0.4; // the box3d mirror rotates the body; the AABB sweep never would
    w.push_entity(wall);
    w.push_entity(capsule_walker(3, vec3f(0.0, 0.9, 0.0)));

    for _ in 0..150 {
        let e = w.entity_mut(3).unwrap();
        // Walk straight at the wall, the full intent re-asserted each tick
        // exactly as a character block would (both components, so the slide
        // rate is the clip's answer, not an accumulator).
        e.vel.x = 4.0;
        e.vel.z = 0.0;
        step_world(&mut w);
    }
    let e = w.entity(3).unwrap();
    // Slid along the face: the wall's world normal is (-cos .4, 0, sin .4),
    // so a +x walk deflects toward +z.
    assert!(
        e.pos.z > 1.0,
        "no slide along the angled wall (z {})",
        e.pos.z
    );
    // And the wall was reported with its TRUE normal, not an axis guess.
    assert!(e.hit_wall != 0, "wall contact never reported");
    let n = e.wall_normal;
    assert!(n.x < -0.8 && n.z > 0.2, "wall normal {:?} is not the rotated face", n);
    // Never through it: every point on the mover's side of the face satisfies
    // dot(p - face_point, n) > 0. Check the capsule centre against the plane
    // through the wall's near face.
    let face = vec3f(4.0 - 0.3 * (0.4f32).cos(), 0.0, 0.3 * (0.4f32).sin());
    let d = (e.pos.x - face.x) * n.x + (e.pos.z - face.z) * n.z;
    assert!(d > -0.05, "capsule penetrated the wall ({d})");
}

/// A corner: sliding along one wall into a second must stop dead without
/// jitter or tunneling — the plane solver holds both constraints at once.
#[test]
fn a_corner_holds_the_capsule_without_penetration() {
    let mut w = GameWorld::new();
    w.gravity = 30.0;
    ground(&mut w);
    w.push_entity(ent(2, BodyKind::Static, vec3f(5.0, 1.5, 0.0), vec3f(0.4, 1.5, 8.0)));
    w.push_entity(ent(3, BodyKind::Static, vec3f(0.0, 1.5, 5.0), vec3f(8.0, 1.5, 0.4)));
    w.push_entity(capsule_walker(4, vec3f(0.0, 0.9, 0.0)));
    for _ in 0..240 {
        let e = w.entity_mut(4).unwrap();
        e.vel.x = 3.0;
        e.vel.z = 3.0;
        step_world(&mut w);
    }
    let e = w.entity(4).unwrap();
    // Wedged into the corner, a capsule-radius short of both faces.
    assert!(e.pos.x < 5.0 - 0.4 + 0.01, "through wall A: x {}", e.pos.x);
    assert!(e.pos.z < 5.0 - 0.4 + 0.01, "through wall B: z {}", e.pos.z);
    assert!(e.pos.x > 3.8 && e.pos.z > 3.8, "never reached the corner ({}, {})", e.pos.x, e.pos.z);
    assert!(e.on_floor, "lost the floor while cornering");
}

/// The wedge mirrors as a true prism now, so the capsule path can walk the
/// ramp it sees — the same contract the AABB sweep gets from its wedge
/// special-case.
#[test]
fn a_capsule_mover_walks_up_a_wedge_ramp() {
    let mut w = GameWorld::new();
    w.gravity = 30.0;
    ground(&mut w);
    let mut ramp = ent(2, BodyKind::Static, vec3f(0.0, 2.0, 0.0), vec3f(6.0, 2.0, 7.0));
    ramp.shape = Shape::Wedge;
    w.push_entity(ramp);
    w.push_entity(capsule_walker(3, vec3f(0.0, 0.9, -9.0)));
    for _ in 0..240 {
        let e = w.entity_mut(3).unwrap();
        e.vel.x = 0.0;
        e.vel.z = 4.0;
        step_world(&mut w);
    }
    let e = w.entity(3).unwrap();
    assert!(e.pos.z > -2.0, "stuck at the ramp's low edge (z {})", e.pos.z);
    assert!(e.pos.y > 1.5, "at z {} but only y {} — through the ramp, not up it", e.pos.z, e.pos.y);
    // On the slope the floor normal tilts toward the low edge.
    if e.on_floor {
        assert!(e.floor_normal.z < -0.1, "slope normal {:?}", e.floor_normal);
    }
}

#[test]
fn the_capsule_path_is_run_to_run_deterministic() {
    fn scene() -> GameWorld {
        let mut w = GameWorld::new();
        w.gravity = 30.0;
        ground(&mut w);
        let mut wall = ent(2, BodyKind::Static, vec3f(4.0, 1.5, 0.0), vec3f(0.3, 1.5, 10.0));
        wall.yaw = 0.35;
        w.push_entity(wall);
        let mut ramp = ent(3, BodyKind::Static, vec3f(-6.0, 1.0, 4.0), vec3f(4.0, 1.0, 5.0));
        ramp.shape = Shape::Wedge;
        w.push_entity(ramp);
        // A light crate the capsule can shove (the sweep-push half).
        let mut crate_e = ent(4, BodyKind::Rigid, vec3f(2.0, 0.4, -3.0), vec3f(0.4, 0.4, 0.4));
        crate_e.density = 0.4;
        w.push_entity(crate_e);
        w.push_entity(capsule_walker(5, vec3f(0.0, 0.9, -3.0)));
        w.push_entity(capsule_walker(6, vec3f(-6.0, 0.9, -2.0)));
        w
    }
    let mut a = scene();
    let mut b = scene();
    for t in 0..300 {
        for (id, vx, vz) in [(5u64, 3.0f32, 0.6f32), (6, 0.0, 3.0)] {
            let steer = if (t / 60) % 2 == 0 { 1.0 } else { -0.4 };
            a.entity_mut(id).unwrap().vel.x = vx * steer;
            a.entity_mut(id).unwrap().vel.z = vz;
            b.entity_mut(id).unwrap().vel.x = vx * steer;
            b.entity_mut(id).unwrap().vel.z = vz;
        }
        step_world(&mut a);
        step_world(&mut b);
    }
    assert_eq!(hash_world(&a), hash_world(&b), "capsule path diverged between runs");
    // And it actually did something: both walkers moved.
    assert!((a.entity(5).unwrap().pos - vec3f(0.0, 0.9, -3.0)).length() > 1.0);
}

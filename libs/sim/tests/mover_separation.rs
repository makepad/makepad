//! Movers shove each other apart (task #16).
//!
//! Before this, `step_world`'s solid snapshot filtered to
//! `Static | Kinematic | Rigid` and movers passed straight through one
//! another — an inherited Godot-parity rule that was defensible with one
//! player character and reads as broken in a village of NPCs.

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

/// Flat ground as a static slab, so gravity has something to rest on.
fn ground(w: &mut GameWorld, id: u64) {
    w.push_entity(ent(
        id,
        BodyKind::Static,
        vec3f(0.0, -0.5, 0.0),
        vec3f(60.0, 0.5, 60.0),
    ));
}

fn horizontal_gap(a: &Entity, b: &Entity) -> f32 {
    let dx = (a.pos.x - b.pos.x).abs() - (a.half.x + b.half.x);
    let dz = (a.pos.z - b.pos.z).abs() - (a.half.z + b.half.z);
    dx.max(dz)
}

#[test]
fn two_movers_walking_together_do_not_end_up_inside_each_other() {
    let mut w = GameWorld::new();
    w.reset_content();
    ground(&mut w, 1);
    w.push_entity(ent(2, BodyKind::Mover, vec3f(-1.0, 0.8, 0.0), vec3f(0.4, 0.8, 0.4)));
    w.push_entity(ent(3, BodyKind::Mover, vec3f(1.0, 0.8, 0.0), vec3f(0.4, 0.8, 0.4)));

    // Drive them into each other for a while, re-asserting velocity the way a
    // walk verb does.
    for _ in 0..120 {
        w.entity_mut(2).unwrap().vel.x = 2.0;
        w.entity_mut(3).unwrap().vel.x = -2.0;
        step_world(&mut w);
    }

    let a = w.entity(2).unwrap();
    let b = w.entity(3).unwrap();
    assert!(
        horizontal_gap(a, b) > -0.05,
        "movers interpenetrated: a={:?} b={:?}",
        a.pos,
        b.pos
    );
    // And neither tunnelled to the far side of the other.
    assert!(a.pos.x < b.pos.x, "movers swapped sides: {} vs {}", a.pos.x, b.pos.x);
}

#[test]
fn a_heavy_mover_shoulders_through_a_light_one() {
    // Same collision, opposite mass ratio, run twice: the heavy body should
    // hold its ground and the light one should give way.
    let run = |heavy_first: bool| -> (f32, f32) {
        let mut w = GameWorld::new();
        w.reset_content();
        ground(&mut w, 1);
        // Overlapping by 0.2 on x: combined halves are 0.8, centres 0.6 apart.
        let mut a = ent(2, BodyKind::Mover, vec3f(-0.3, 0.8, 0.0), vec3f(0.4, 0.8, 0.4));
        let mut b = ent(3, BodyKind::Mover, vec3f(0.3, 0.8, 0.0), vec3f(0.4, 0.8, 0.4));
        a.push_mass = if heavy_first { 8.0 } else { 1.0 };
        b.push_mass = if heavy_first { 1.0 } else { 8.0 };
        w.push_entity(a);
        w.push_entity(b);
        for _ in 0..30 {
            step_world(&mut w);
        }
        (
            (w.entity(2).unwrap().pos.x + 0.3).abs(),
            (w.entity(3).unwrap().pos.x - 0.3).abs(),
        )
    };

    let (heavy_moved, light_moved) = run(true);
    assert!(
        light_moved > heavy_moved * 2.0,
        "heavy should barely budge: heavy {heavy_moved}, light {light_moved}"
    );
    // Mirrored, so the result is about mass and not about who is listed first.
    let (light_moved2, heavy_moved2) = run(false);
    assert!(
        light_moved2 > heavy_moved2 * 2.0,
        "mirrored case failed: light {light_moved2}, heavy {heavy_moved2}"
    );
}

#[test]
fn a_crowd_converging_on_one_point_settles_without_stacking_or_exploding() {
    let mut w = GameWorld::new();
    w.reset_content();
    ground(&mut w, 1);
    // Twelve villagers on a ring, all walking at the middle — the bench-crowd
    // case that made this visible in the first place.
    for i in 0..12u64 {
        let a = i as f32 * std::f32::consts::TAU / 12.0;
        w.push_entity(ent(
            i + 2,
            BodyKind::Mover,
            vec3f(a.cos() * 5.0, 0.8, a.sin() * 5.0),
            vec3f(0.4, 0.8, 0.4),
        ));
    }

    for _ in 0..600 {
        for i in 0..12u64 {
            let e = w.entity(i + 2).unwrap();
            let (dx, dz) = (-e.pos.x, -e.pos.z);
            let len = (dx * dx + dz * dz).sqrt().max(1.0e-3);
            let m = w.entity_mut(i + 2).unwrap();
            m.vel.x = dx / len * 1.5;
            m.vel.z = dz / len * 1.5;
        }
        step_world(&mut w);
    }

    let people: Vec<Entity> = (0..12u64).map(|i| w.entity(i + 2).unwrap().clone()).collect();
    for p in &people {
        assert!(
            p.pos.x.is_finite() && p.pos.y.is_finite() && p.pos.z.is_finite(),
            "crowd exploded to non-finite: {:?}",
            p.pos
        );
        // Nobody climbed: everyone is still standing on the ground slab.
        assert!(
            (p.pos.y - 0.8).abs() < 0.1,
            "someone was pushed off the ground plane: y={}",
            p.pos.y
        );
        // Nobody was flung across the map by a runaway relaxation.
        assert!(
            p.pos.x.abs() < 12.0 && p.pos.z.abs() < 12.0,
            "someone was flung out of the crowd: {:?}",
            p.pos
        );
    }
    for i in 0..people.len() {
        for j in i + 1..people.len() {
            assert!(
                horizontal_gap(&people[i], &people[j]) > -0.12,
                "crowd stacked: {} and {} at {:?} / {:?}",
                people[i].id,
                people[j].id,
                people[i].pos,
                people[j].pos
            );
        }
    }
}

#[test]
fn separation_is_horizontal_so_nobody_is_pushed_onto_a_head() {
    let mut w = GameWorld::new();
    w.reset_content();
    ground(&mut w, 1);
    // One dropped almost exactly onto another.
    w.push_entity(ent(2, BodyKind::Mover, vec3f(0.0, 0.8, 0.0), vec3f(0.4, 0.8, 0.4)));
    w.push_entity(ent(3, BodyKind::Mover, vec3f(0.05, 2.4, 0.05), vec3f(0.4, 0.8, 0.4)));

    for _ in 0..180 {
        step_world(&mut w);
    }

    let a = w.entity(2).unwrap();
    let b = w.entity(3).unwrap();
    // Both ended up on the floor, side by side — not one perched on the other.
    assert!((a.pos.y - 0.8).abs() < 0.1, "a left the ground: {}", a.pos.y);
    assert!((b.pos.y - 0.8).abs() < 0.1, "b perched on a head: {}", b.pos.y);
    assert!(
        horizontal_gap(a, b) > -0.05,
        "coincident pair never separated: {:?} / {:?}",
        a.pos,
        b.pos
    );
}

#[test]
fn a_rider_is_pinned_not_shoved() {
    let mut w = GameWorld::new();
    w.reset_content();
    ground(&mut w, 1);
    let mut car = ent(2, BodyKind::Mover, vec3f(0.0, 0.8, 0.0), vec3f(0.9, 0.8, 1.6));
    car.vel = vec3f(1.0, 0.0, 0.0);
    w.push_entity(car);
    // Rider sits inside the carrier's box — a permanent overlap that
    // separation must not try to resolve.
    let mut rider = ent(3, BodyKind::Mover, vec3f(0.0, 1.2, 0.0), vec3f(0.35, 0.7, 0.35));
    rider.attached_to = 2;
    rider.attach_offset = vec3f(0.0, 0.4, 0.0);
    w.push_entity(rider);

    for _ in 0..90 {
        w.entity_mut(2).unwrap().vel.x = 1.0;
        step_world(&mut w);
    }

    let car = w.entity(2).unwrap().clone();
    let rider = w.entity(3).unwrap();
    assert_eq!(rider.attached_to, 2, "rider was detached");
    let want = car.pos + vec3f(0.0, 0.4, 0.0);
    assert!(
        (rider.pos - want).length() < 1.0e-4,
        "rider drifted off its seat: {:?} want {:?}",
        rider.pos,
        want
    );
}

#[test]
fn projectiles_still_pass_through_movers() {
    // collect_touches reports a hit FROM the overlap, so a projectile that
    // gets separated could never touch anyone.
    let mut w = GameWorld::new();
    w.reset_content();
    ground(&mut w, 1);
    w.push_entity(ent(2, BodyKind::Mover, vec3f(0.0, 0.8, 0.0), vec3f(0.4, 0.8, 0.4)));
    let mut bullet = ent(3, BodyKind::Mover, vec3f(0.0, 0.8, 0.0), vec3f(0.1, 0.1, 0.1));
    bullet.hits = true;
    bullet.gravity_scale = 0.0;
    w.push_entity(bullet);

    step_world(&mut w);
    let touches = collect_touches(&w);
    assert!(
        touches.iter().any(|(a, b)| (*a == 3 && *b == 2) || (*a == 2 && *b == 3)),
        "projectile stopped reporting its hit: {touches:?}"
    );
}

#[test]
fn non_colliding_and_sensor_movers_are_left_alone() {
    let mut w = GameWorld::new();
    w.reset_content();
    ground(&mut w, 1);
    w.push_entity(ent(2, BodyKind::Mover, vec3f(0.0, 0.8, 0.0), vec3f(0.4, 0.8, 0.4)));
    let mut decor = ent(3, BodyKind::Mover, vec3f(0.0, 0.8, 0.0), vec3f(0.4, 0.8, 0.4));
    decor.collide = false;
    w.push_entity(decor);
    let mut sensor = ent(4, BodyKind::Mover, vec3f(0.0, 0.8, 0.0), vec3f(0.4, 0.8, 0.4));
    sensor.sensor = true;
    w.push_entity(sensor);

    for _ in 0..60 {
        step_world(&mut w);
    }

    // Both opted out, so both are still sitting exactly on top of the solid one.
    for id in [3u64, 4] {
        let e = w.entity(id).unwrap();
        assert!(
            e.pos.x.abs() < 1.0e-3 && e.pos.z.abs() < 1.0e-3,
            "opted-out mover {id} was shoved to {:?}",
            e.pos
        );
    }
}

#[test]
fn separation_cannot_squeeze_a_crowd_through_a_wall() {
    let mut w = GameWorld::new();
    w.reset_content();
    ground(&mut w, 1);
    // A wall at x = 2, and six movers pressed into it.
    w.push_entity(ent(2, BodyKind::Static, vec3f(2.5, 1.0, 0.0), vec3f(0.5, 1.0, 8.0)));
    for i in 0..6u64 {
        let mut m = ent(
            i + 3,
            BodyKind::Mover,
            vec3f(1.0 - i as f32 * 0.05, 0.8, i as f32 * 0.05),
            vec3f(0.4, 0.8, 0.4),
        );
        m.vel = vec3f(3.0, 0.0, 0.0);
        w.push_entity(m);
    }

    for _ in 0..240 {
        for i in 0..6u64 {
            w.entity_mut(i + 3).unwrap().vel.x = 3.0;
        }
        step_world(&mut w);
    }

    for i in 0..6u64 {
        let e = w.entity(i + 3).unwrap();
        assert!(
            e.pos.x + e.half.x < 2.1,
            "mover {i} was squeezed into the wall: x={}",
            e.pos.x
        );
    }
}

#[test]
fn separation_is_deterministic() {
    let build = || {
        let mut w = GameWorld::new();
        w.reset_content();
        ground(&mut w, 1);
        for i in 0..10u64 {
            let a = i as f32 * 0.7;
            let mut m = ent(
                i + 2,
                BodyKind::Mover,
                vec3f(a.cos() * 2.0, 0.8, a.sin() * 2.0),
                vec3f(0.4, 0.8, 0.4),
            );
            m.vel = vec3f(-a.cos(), 0.0, -a.sin());
            w.push_entity(m);
        }
        w
    };
    let (mut a, mut b) = (build(), build());
    for _ in 0..300 {
        step_world(&mut a);
        step_world(&mut b);
    }
    for i in 0..10u64 {
        let (pa, pb) = (a.entity(i + 2).unwrap().pos, b.entity(i + 2).unwrap().pos);
        assert_eq!(pa.x.to_bits(), pb.x.to_bits(), "mover {i} x diverged");
        assert_eq!(pa.y.to_bits(), pb.y.to_bits(), "mover {i} y diverged");
        assert_eq!(pa.z.to_bits(), pb.z.to_bits(), "mover {i} z diverged");
    }
}

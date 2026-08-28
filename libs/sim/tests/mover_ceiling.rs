//! Ceiling contract for the canonical mover sweep.
//!
//! FPS, third-person and scripted walkers all end here. A head hit must be a
//! clamp, not a bounce or a sticky grounded state.

use makepad_game_sim::{step_world, BodyKind, Entity, GameWorld};
use makepad_math::{vec3f, Vec3f};

fn body(id: u64, kind: BodyKind, pos: Vec3f, half: Vec3f) -> Entity {
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

#[test]
fn a_low_ceiling_zeroes_upward_velocity_without_wedging_the_mover() {
    let mut world = GameWorld::new();
    world.gravity = 9.81;
    // Floor top y=0, ceiling underside y=1.0. The 0.8 m body has 0.2 m
    // standing headroom: enough to jump into the roof, not enough to pass it.
    world.push_entity(body(
        1,
        BodyKind::Static,
        vec3f(0.0, -0.5, 0.0),
        vec3f(4.0, 0.5, 4.0),
    ));
    world.push_entity(body(
        2,
        BodyKind::Static,
        vec3f(0.0, 1.1, 0.0),
        vec3f(4.0, 0.1, 4.0),
    ));
    world.push_entity(body(
        3,
        BodyKind::Mover,
        vec3f(0.0, 0.4, 0.0),
        vec3f(0.25, 0.4, 0.25),
    ));

    for _ in 0..4 {
        step_world(&mut world);
    }
    assert!(world.entity(3).unwrap().on_floor);
    world.entity_mut(3).unwrap().vel.y = 6.0;

    let mut hit_head = false;
    let mut landed = false;
    for _ in 0..180 {
        let before = world.entity(3).unwrap().vel.y;
        step_world(&mut world);
        let mover = world.entity(3).unwrap();
        assert!(
            mover.pos.y + mover.half.y <= 1.0 + 1.0e-4,
            "mover head crossed the ceiling: {:?}",
            mover.pos
        );
        if before > 0.0 && mover.vel.y == 0.0 && !mover.on_floor {
            hit_head = true;
        }
        if hit_head && mover.on_floor {
            landed = true;
            break;
        }
    }

    let mover = world.entity(3).unwrap();
    assert!(hit_head, "the upward sweep never reported a head stop");
    assert!(landed, "the head stop left the mover wedged under the roof");
    assert!(
        (mover.pos.y - 0.4).abs() < 0.01,
        "landed at y {} instead of the floor centre 0.4",
        mover.pos.y
    );
    assert_eq!((mover.pos.x, mover.pos.z), (0.0, 0.0), "ceiling contact must not kick sideways");
}

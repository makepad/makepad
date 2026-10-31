// Load-dependent joint friction (port extension, not in the C library).
//
// A pendulum on a revolute or spherical joint with friction radius r and coefficient mu
// sticks wherever the gravity torque m g L sin(theta) is below mu r m g (the pin's reaction
// is the weight when it hangs still), so it settles inside the dead band
// sin(theta) <= mu r / L, independent of its mass. A block on a tilted prismatic rail
// holds iff mu >= tan(tilt).

use makepad_box3d::body::*;
use makepad_box3d::hull::make_box_hull;
use makepad_box3d::id::{BodyId, JointId};
use makepad_box3d::joint::*;
use makepad_box3d::math_functions::{make_quat_from_axis_angle, pos, vec3, Quat, Transform, Vec3};
use makepad_box3d::physics_world::*;
use makepad_box3d::prismatic_joint::*;
use makepad_box3d::shape::create_hull_shape;
use makepad_box3d::types::*;

const L: f32 = 1.0;

/// A static pivot at the origin and a 0.1 x 2L x 0.1 bar whose top end is on the pivot,
/// turned by `theta` about z from hanging straight down (-y).
fn pendulum(density: f32, theta: f32) -> (World, BodyId, BodyId) {
    let mut world = create_world(&default_world_def());
    let ground = create_body(&mut world, &default_body_def());
    let mut bd = default_body_def();
    bd.body_type = BodyType::Dynamic;
    let (s, c) = theta.sin_cos();
    bd.position = pos(L * s, -L * c, 0.0);
    bd.rotation = make_quat_from_axis_angle(vec3(0.0, 0.0, 1.0), theta);
    bd.enable_sleep = false;
    let body = create_body(&mut world, &bd);
    let mut sd = default_shape_def();
    sd.density = density;
    create_hull_shape(&mut world, body, &sd, &make_box_hull(0.05, L, 0.05));
    (world, ground, body)
}

fn frames(base: &mut JointDef, ground: BodyId, body: BodyId) {
    base.body_id_a = ground;
    base.body_id_b = body;
    base.local_frame_a = Transform::IDENTITY;
    base.local_frame_b = Transform { p: vec3(0.0, L, 0.0), q: Quat::IDENTITY };
}

fn revolute(world: &mut World, ground: BodyId, body: BodyId, friction: JointFriction) -> JointId {
    let mut jd = default_revolute_joint_def();
    frames(&mut jd.base, ground, body);
    jd.base.friction = friction;
    create_revolute_joint(world, &jd)
}

fn settle(world: &mut World, seconds: f32) {
    let steps = (seconds * 60.0) as i32;
    for _ in 0..steps {
        world_step(world, 1.0 / 60.0, 4);
    }
}

fn angle_from_down(world: &World, body: BodyId) -> f32 {
    let p = body_get_position(world, body);
    p.x.atan2(-p.y)
}

#[test]
fn revolute_friction_holds_inside_dead_band() {
    // dead band: asin(mu r / L) = asin(0.5 * 0.4) = 11.5 degrees
    let friction = JointFriction { coefficient: 0.5, radius: 0.4, torque: 0.0 };
    for density in [1.0, 50.0] {
        let (mut world, ground, body) = pendulum(density, 8f32.to_radians());
        revolute(&mut world, ground, body, friction);
        settle(&mut world, 3.0);
        let a = angle_from_down(&world, body).to_degrees();
        assert!((a - 8.0).abs() < 0.5, "held pendulum moved to {a} deg (density {density})");
    }
}

#[test]
fn revolute_friction_slides_outside_dead_band_and_stops_inside() {
    let friction = JointFriction { coefficient: 0.5, radius: 0.4, torque: 0.0 };
    let band = (0.5f32 * 0.4 / L).asin().to_degrees();
    let (mut world, ground, body) = pendulum(1.0, 40f32.to_radians());
    let joint = revolute(&mut world, ground, body, friction);
    settle(&mut world, 10.0);
    let a = angle_from_down(&world, body).to_degrees();
    assert!(a.abs() <= band + 1.0, "settled at {a} deg, dead band {band}");
    assert!(a.abs() < 39.0, "never moved");
    let w = body_get_angular_velocity(&world, body);
    assert!(w.z.abs() < 1e-3);
    // at rest the friction torque balances gravity: m g L sin(a)
    let t = joint_get_friction_torque(&world, joint);
    let m = body_get_mass(&world, body);
    let expect = m * 10.0 * L * a.to_radians().sin();
    assert!((t.x.abs() - expect.abs()).abs() < 0.05 * m * 10.0 * L, "friction torque {} vs gravity torque {}", t.x, expect);
}

#[test]
fn revolute_without_friction_keeps_swinging() {
    let (mut world, ground, body) = pendulum(1.0, 40f32.to_radians());
    revolute(&mut world, ground, body, JointFriction::default());
    let mut max_w: f32 = 0.0;
    for _ in 0..600 {
        world_step(&mut world, 1.0 / 60.0, 4);
        max_w = max_w.max(body_get_angular_velocity(&world, body).z.abs());
    }
    let w = body_get_angular_velocity(&world, body).z.abs();
    let a = angle_from_down(&world, body).to_degrees().abs();
    assert!(w > 0.1 || a > 10.0, "frictionless pendulum stopped (w {w}, a {a})");
    assert!(max_w > 1.0);
}

#[test]
fn constant_friction_torque_holds_without_load_term() {
    let (mut world, ground, body) = pendulum(1.0, 30f32.to_radians());
    let m = body_get_mass(&world, body);
    let gravity_torque = m * 10.0 * L * 30f32.to_radians().sin();
    let joint = revolute(&mut world, ground, body, JointFriction { coefficient: 0.0, radius: 0.0, torque: 1.2 * gravity_torque });
    settle(&mut world, 2.0);
    assert!((angle_from_down(&world, body).to_degrees() - 30.0).abs() < 0.5);
    // switching it off lets it fall
    joint_set_friction(&mut world, joint, JointFriction::default());
    settle(&mut world, 0.5);
    assert!(angle_from_down(&world, body).to_degrees() < 25.0);
}

#[test]
fn spherical_friction_dead_band() {
    let friction = JointFriction { coefficient: 0.5, radius: 0.4, torque: 0.0 };
    let band = (0.5f32 * 0.4 / L).asin().to_degrees();
    // inside: holds
    let (mut world, ground, body) = pendulum(1.0, 8f32.to_radians());
    let mut sd = default_spherical_joint_def();
    frames(&mut sd.base, ground, body);
    sd.base.friction = friction;
    create_spherical_joint(&mut world, &sd);
    settle(&mut world, 3.0);
    assert!((angle_from_down(&world, body).to_degrees() - 8.0).abs() < 0.5);
    // outside: falls into the band
    let (mut world, ground, body) = pendulum(1.0, 40f32.to_radians());
    let mut sd = default_spherical_joint_def();
    frames(&mut sd.base, ground, body);
    sd.base.friction = friction;
    create_spherical_joint(&mut world, &sd);
    settle(&mut world, 10.0);
    let p = body_get_position(&world, body);
    let a = (p.x * p.x + p.z * p.z).sqrt().atan2(-p.y).to_degrees();
    assert!(a <= band + 1.0 && a < 39.0, "spherical settled at {a} deg, band {band}");
}

#[test]
fn prismatic_friction_holds_iff_mu_above_tan_tilt() {
    for (mu, holds) in [(0.7f32, true), (0.3f32, false)] {
        let mut world = create_world(&default_world_def());
        let ground = create_body(&mut world, &default_body_def());
        let mut bd = default_body_def();
        bd.body_type = BodyType::Dynamic;
        bd.enable_sleep = false;
        let body = create_body(&mut world, &bd);
        let mut shape = default_shape_def();
        shape.density = 1.0;
        create_hull_shape(&mut world, body, &shape, &make_box_hull(0.2, 0.2, 0.2));
        // rail along x tilted 30 degrees down: tan(30) = 0.577
        let tilt = make_quat_from_axis_angle(vec3(0.0, 0.0, 1.0), -30f32.to_radians());
        let mut pd = default_prismatic_joint_def();
        pd.base.body_id_a = ground;
        pd.base.body_id_b = body;
        pd.base.local_frame_a = Transform { p: Vec3::ZERO, q: tilt };
        pd.base.local_frame_b = Transform { p: Vec3::ZERO, q: tilt };
        pd.base.friction = JointFriction { coefficient: mu, radius: 0.0, torque: 0.0 };
        let j = create_prismatic_joint(&mut world, &pd);
        settle(&mut world, 1.0);
        let t = prismatic_joint_get_translation(&mut world, j);
        if holds {
            assert!(t.abs() < 0.01, "mu {mu}: slid {t}");
        } else {
            assert!(t > 0.5, "mu {mu}: only slid {t}");
        }
    }
}

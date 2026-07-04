// Port of box3d/test/test_joint.c
//
// One test per joint type. Each creates the joint, exercises the shared
// joint API plus every type-specific accessor, then steps to make sure the
// joint solves without tripping a validation assert.
//
// Adaptations from C (owned World instead of world ids):
// - b3Joint_GetWorld returns a WorldId; the port compares against the id
//   reconstructed from the owned world.
// - user data is u64 instead of void*.

use makepad_box3d::body::*;
use makepad_box3d::distance_joint::*;
use makepad_box3d::ensure;
use makepad_box3d::ensure_small;
use makepad_box3d::hull::make_cube_hull;
use makepad_box3d::id::{BodyId, JointId, WorldId};
use makepad_box3d::joint::*;
use makepad_box3d::math_functions::{vec3, Quat, Transform, Vec3};
use makepad_box3d::motor_joint::*;
use makepad_box3d::parallel_joint::*;
use makepad_box3d::physics_world::*;
use makepad_box3d::prismatic_joint::*;
use makepad_box3d::revolute_joint::*;
use makepad_box3d::shape::create_hull_shape;
use makepad_box3d::spherical_joint::*;
use makepad_box3d::types::*;
use makepad_box3d::weld_joint::*;
use makepad_box3d::wheel_joint::*;

struct JointFixture {
    world: World,
    ground_id: BodyId,
    body_id: BodyId,
}

// Static ground plus a dynamic box, anchored so a point-coincident joint starts
// satisfied. Gravity is off so the body stays put across the handful of steps
// each sub-test takes.
fn create_joint_fixture() -> JointFixture {
    let mut world_def = default_world_def();
    world_def.gravity = Vec3::ZERO;

    let mut world = create_world(&world_def);

    let ground_def = default_body_def();
    let ground_id = create_body(&mut world, &ground_def);

    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    body_def.position = vec3(0.0, 4.0, 0.0);
    let body_id = create_body(&mut world, &body_def);

    let mut shape_def = default_shape_def();
    shape_def.density = 1.0;
    let box_hull = make_cube_hull(0.5);
    create_hull_shape(&mut world, body_id, &shape_def, &box_hull);

    JointFixture { world, ground_id, body_id }
}

// Place the joint anchor at the dynamic body so both local frames map to the
// same world point.
fn set_common_frames(base: &mut JointDef, f: &JointFixture) {
    base.body_id_a = f.ground_id;
    base.body_id_b = f.body_id;
    base.local_frame_a.p = vec3(0.0, 4.0, 0.0);
    base.local_frame_b.p = vec3(0.0, 0.0, 0.0);
}

// Step a few times, destroy the joint, then the world. Destroying the joint
// explicitly also covers destroy_joint and stale-handle detection.
fn finish_joint(joint_id: JointId, mut world: World) {
    for _ in 0..8 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    destroy_joint(&mut world, joint_id, true);
    ensure!(joint_is_valid(&world, joint_id) == false);

    destroy_world(world);
}

// Exercise the API shared by every joint type. Frames are saved and restored so
// the caller's setup survives.
fn exercise_joint_base(
    joint_id: JointId,
    world: &mut World,
    body_id_a: BodyId,
    body_id_b: BodyId,
    expected_type: JointType,
) {
    ensure!(joint_is_valid(world, joint_id));
    ensure!(joint_get_type(world, joint_id) == expected_type);
    ensure!(joint_get_body_a(world, joint_id) == body_id_a);
    ensure!(joint_get_body_b(world, joint_id) == body_id_b);

    // The world id reconstructed from the owned world.
    let world_id = WorldId { index1: world.world_id + 1, generation: world.generation };
    let got_world = joint_get_world(world, joint_id);
    ensure!(got_world.index1 == world_id.index1);

    let original_a = joint_get_local_frame_a(world, joint_id);
    let original_b = joint_get_local_frame_b(world, joint_id);

    let frame_a = Transform { p: vec3(0.1, 0.2, 0.3), q: Quat::IDENTITY };
    joint_set_local_frame_a(world, joint_id, frame_a);
    let got_a = joint_get_local_frame_a(world, joint_id);
    ensure!(got_a.p.x == frame_a.p.x && got_a.p.y == frame_a.p.y && got_a.p.z == frame_a.p.z);

    let frame_b = Transform { p: vec3(-0.4, 0.5, -0.6), q: Quat::IDENTITY };
    joint_set_local_frame_b(world, joint_id, frame_b);
    let got_b = joint_get_local_frame_b(world, joint_id);
    ensure!(got_b.p.x == frame_b.p.x && got_b.p.y == frame_b.p.y && got_b.p.z == frame_b.p.z);

    joint_set_collide_connected(world, joint_id, true);
    ensure!(joint_get_collide_connected(world, joint_id) == true);
    joint_set_collide_connected(world, joint_id, false);
    ensure!(joint_get_collide_connected(world, joint_id) == false);

    let user_data: u64 = 0xC0FFEE;
    joint_set_user_data(world, joint_id, user_data);
    ensure!(joint_get_user_data(world, joint_id) == user_data);

    joint_set_constraint_tuning(world, joint_id, 90.0, 3.0);
    let mut hertz = 0.0;
    let mut damping_ratio = 0.0;
    joint_get_constraint_tuning(world, joint_id, &mut hertz, &mut damping_ratio);
    ensure!(hertz == 90.0);
    ensure!(damping_ratio == 3.0);

    joint_set_force_threshold(world, joint_id, 100.0);
    ensure!(joint_get_force_threshold(world, joint_id) == 100.0);

    joint_set_torque_threshold(world, joint_id, 200.0);
    ensure!(joint_get_torque_threshold(world, joint_id) == 200.0);

    joint_wake_bodies(world, joint_id);

    // No stable value to assert before the first step, call for coverage
    let _force = joint_get_constraint_force(world, joint_id);
    let _torque = joint_get_constraint_torque(world, joint_id);
    let _linear_separation = joint_get_linear_separation(world, joint_id);

    // Wheel joint angular separation is an unimplemented todo in joint.c that
    // asserts. Every other type computes it.
    if expected_type != JointType::Wheel {
        let _angular_separation = joint_get_angular_separation(world, joint_id);
    }

    joint_set_local_frame_a(world, joint_id, original_a);
    joint_set_local_frame_b(world, joint_id, original_b);
}

#[test]
fn test_parallel_joint() {
    let mut f = create_joint_fixture();

    let mut def = default_parallel_joint_def();
    set_common_frames(&mut def.base, &f);
    def.hertz = 2.0;
    def.damping_ratio = 0.5;
    def.max_torque = 100.0;
    let joint_id = create_parallel_joint(&mut f.world, &def);

    exercise_joint_base(joint_id, &mut f.world, f.ground_id, f.body_id, JointType::Parallel);

    parallel_joint_set_spring_hertz(&mut f.world, joint_id, 5.0);
    ensure!(parallel_joint_get_spring_hertz(&mut f.world, joint_id) == 5.0);

    parallel_joint_set_spring_damping_ratio(&mut f.world, joint_id, 0.7);
    ensure!(parallel_joint_get_spring_damping_ratio(&mut f.world, joint_id) == 0.7);

    parallel_joint_set_max_torque(&mut f.world, joint_id, 250.0);
    ensure!(parallel_joint_get_max_torque(&mut f.world, joint_id) == 250.0);

    finish_joint(joint_id, f.world);
}

#[test]
fn test_distance_joint() {
    let mut f = create_joint_fixture();

    let mut def = default_distance_joint_def();
    set_common_frames(&mut def.base, &f);
    def.length = 2.0;
    let joint_id = create_distance_joint(&mut f.world, &def);

    exercise_joint_base(joint_id, &mut f.world, f.ground_id, f.body_id, JointType::Distance);

    distance_joint_set_length(&mut f.world, joint_id, 3.0);
    ensure!(distance_joint_get_length(&mut f.world, joint_id) == 3.0);

    distance_joint_enable_spring(&mut f.world, joint_id, true);
    ensure!(distance_joint_is_spring_enabled(&mut f.world, joint_id) == true);

    distance_joint_set_spring_force_range(&mut f.world, joint_id, -50.0, 75.0);
    let mut lower_force = 0.0;
    let mut upper_force = 0.0;
    distance_joint_get_spring_force_range(&mut f.world, joint_id, &mut lower_force, &mut upper_force);
    ensure!(lower_force == -50.0 && upper_force == 75.0);

    distance_joint_set_spring_hertz(&mut f.world, joint_id, 4.0);
    ensure!(distance_joint_get_spring_hertz(&mut f.world, joint_id) == 4.0);

    distance_joint_set_spring_damping_ratio(&mut f.world, joint_id, 0.6);
    ensure!(distance_joint_get_spring_damping_ratio(&mut f.world, joint_id) == 0.6);

    distance_joint_enable_limit(&mut f.world, joint_id, true);
    ensure!(distance_joint_is_limit_enabled(&mut f.world, joint_id) == true);

    distance_joint_set_length_range(&mut f.world, joint_id, 1.0, 5.0);
    ensure!(distance_joint_get_min_length(&mut f.world, joint_id) == 1.0);
    ensure!(distance_joint_get_max_length(&mut f.world, joint_id) == 5.0);

    let _current_length = distance_joint_get_current_length(&mut f.world, joint_id);

    distance_joint_enable_motor(&mut f.world, joint_id, true);
    ensure!(distance_joint_is_motor_enabled(&mut f.world, joint_id) == true);

    distance_joint_set_motor_speed(&mut f.world, joint_id, 1.5);
    ensure!(distance_joint_get_motor_speed(&mut f.world, joint_id) == 1.5);

    distance_joint_set_max_motor_force(&mut f.world, joint_id, 25.0);
    ensure!(distance_joint_get_max_motor_force(&mut f.world, joint_id) == 25.0);

    let _motor_force = distance_joint_get_motor_force(&mut f.world, joint_id);

    finish_joint(joint_id, f.world);
}

#[test]
fn test_filter_joint() {
    let mut f = create_joint_fixture();

    // The filter joint has no type-specific API. It only disables collision and
    // keeps both bodies in the same island.
    let mut def = default_filter_joint_def();
    def.base.body_id_a = f.ground_id;
    def.base.body_id_b = f.body_id;
    let joint_id = create_filter_joint(&mut f.world, &def);

    exercise_joint_base(joint_id, &mut f.world, f.ground_id, f.body_id, JointType::Filter);

    finish_joint(joint_id, f.world);
}

#[test]
fn test_motor_joint() {
    let mut f = create_joint_fixture();

    let mut def = default_motor_joint_def();
    set_common_frames(&mut def.base, &f);
    let joint_id = create_motor_joint(&mut f.world, &def);

    exercise_joint_base(joint_id, &mut f.world, f.ground_id, f.body_id, JointType::Motor);

    let linear_velocity = vec3(1.0, 2.0, 3.0);
    motor_joint_set_linear_velocity(&mut f.world, joint_id, linear_velocity);
    let got_linear = motor_joint_get_linear_velocity(&mut f.world, joint_id);
    ensure!(got_linear.x == 1.0 && got_linear.y == 2.0 && got_linear.z == 3.0);

    let angular_velocity = vec3(0.1, 0.2, 0.3);
    motor_joint_set_angular_velocity(&mut f.world, joint_id, angular_velocity);
    let got_angular = motor_joint_get_angular_velocity(&mut f.world, joint_id);
    ensure!(got_angular.x == 0.1 && got_angular.y == 0.2 && got_angular.z == 0.3);

    motor_joint_set_max_velocity_force(&mut f.world, joint_id, 500.0);
    ensure!(motor_joint_get_max_velocity_force(&mut f.world, joint_id) == 500.0);

    motor_joint_set_max_velocity_torque(&mut f.world, joint_id, 600.0);
    ensure!(motor_joint_get_max_velocity_torque(&mut f.world, joint_id) == 600.0);

    motor_joint_set_linear_hertz(&mut f.world, joint_id, 3.0);
    ensure!(motor_joint_get_linear_hertz(&mut f.world, joint_id) == 3.0);

    motor_joint_set_linear_damping_ratio(&mut f.world, joint_id, 0.8);
    ensure!(motor_joint_get_linear_damping_ratio(&mut f.world, joint_id) == 0.8);

    motor_joint_set_angular_hertz(&mut f.world, joint_id, 4.0);
    ensure!(motor_joint_get_angular_hertz(&mut f.world, joint_id) == 4.0);

    motor_joint_set_angular_damping_ratio(&mut f.world, joint_id, 0.9);
    ensure!(motor_joint_get_angular_damping_ratio(&mut f.world, joint_id) == 0.9);

    motor_joint_set_max_spring_force(&mut f.world, joint_id, 700.0);
    ensure!(motor_joint_get_max_spring_force(&mut f.world, joint_id) == 700.0);

    motor_joint_set_max_spring_torque(&mut f.world, joint_id, 800.0);
    ensure!(motor_joint_get_max_spring_torque(&mut f.world, joint_id) == 800.0);

    finish_joint(joint_id, f.world);
}

#[test]
fn test_prismatic_joint() {
    let mut f = create_joint_fixture();

    let mut def = default_prismatic_joint_def();
    set_common_frames(&mut def.base, &f);
    let joint_id = create_prismatic_joint(&mut f.world, &def);

    exercise_joint_base(joint_id, &mut f.world, f.ground_id, f.body_id, JointType::Prismatic);

    prismatic_joint_enable_spring(&mut f.world, joint_id, true);
    ensure!(prismatic_joint_is_spring_enabled(&mut f.world, joint_id) == true);

    prismatic_joint_set_spring_hertz(&mut f.world, joint_id, 5.0);
    ensure!(prismatic_joint_get_spring_hertz(&mut f.world, joint_id) == 5.0);

    prismatic_joint_set_spring_damping_ratio(&mut f.world, joint_id, 0.5);
    ensure!(prismatic_joint_get_spring_damping_ratio(&mut f.world, joint_id) == 0.5);

    prismatic_joint_set_target_translation(&mut f.world, joint_id, 1.0);
    ensure!(prismatic_joint_get_target_translation(&mut f.world, joint_id) == 1.0);

    prismatic_joint_enable_limit(&mut f.world, joint_id, true);
    ensure!(prismatic_joint_is_limit_enabled(&mut f.world, joint_id) == true);

    prismatic_joint_set_limits(&mut f.world, joint_id, -2.0, 2.0);
    ensure!(prismatic_joint_get_lower_limit(&mut f.world, joint_id) == -2.0);
    ensure!(prismatic_joint_get_upper_limit(&mut f.world, joint_id) == 2.0);

    prismatic_joint_enable_motor(&mut f.world, joint_id, true);
    ensure!(prismatic_joint_is_motor_enabled(&mut f.world, joint_id) == true);

    prismatic_joint_set_motor_speed(&mut f.world, joint_id, 1.5);
    ensure!(prismatic_joint_get_motor_speed(&mut f.world, joint_id) == 1.5);

    prismatic_joint_set_max_motor_force(&mut f.world, joint_id, 30.0);
    ensure!(prismatic_joint_get_max_motor_force(&mut f.world, joint_id) == 30.0);

    let _motor_force = prismatic_joint_get_motor_force(&mut f.world, joint_id);
    let _translation = prismatic_joint_get_translation(&mut f.world, joint_id);
    let _speed = prismatic_joint_get_speed(&mut f.world, joint_id);

    finish_joint(joint_id, f.world);
}

#[test]
fn test_revolute_joint() {
    let mut f = create_joint_fixture();

    let mut def = default_revolute_joint_def();
    set_common_frames(&mut def.base, &f);
    let joint_id = create_revolute_joint(&mut f.world, &def);

    exercise_joint_base(joint_id, &mut f.world, f.ground_id, f.body_id, JointType::Revolute);

    revolute_joint_enable_spring(&mut f.world, joint_id, true);
    ensure!(revolute_joint_is_spring_enabled(&mut f.world, joint_id) == true);

    revolute_joint_set_spring_hertz(&mut f.world, joint_id, 5.0);
    ensure!(revolute_joint_get_spring_hertz(&mut f.world, joint_id) == 5.0);

    revolute_joint_set_spring_damping_ratio(&mut f.world, joint_id, 0.5);
    ensure!(revolute_joint_get_spring_damping_ratio(&mut f.world, joint_id) == 0.5);

    revolute_joint_set_target_angle(&mut f.world, joint_id, 0.5);
    ensure!(revolute_joint_get_target_angle(&mut f.world, joint_id) == 0.5);

    let _angle = revolute_joint_get_angle(&mut f.world, joint_id);

    revolute_joint_enable_limit(&mut f.world, joint_id, true);
    ensure!(revolute_joint_is_limit_enabled(&mut f.world, joint_id) == true);

    revolute_joint_set_limits(&mut f.world, joint_id, -1.0, 1.0);
    ensure!(revolute_joint_get_lower_limit(&mut f.world, joint_id) == -1.0);
    ensure!(revolute_joint_get_upper_limit(&mut f.world, joint_id) == 1.0);

    revolute_joint_enable_motor(&mut f.world, joint_id, true);
    ensure!(revolute_joint_is_motor_enabled(&mut f.world, joint_id) == true);

    revolute_joint_set_motor_speed(&mut f.world, joint_id, 2.0);
    ensure!(revolute_joint_get_motor_speed(&mut f.world, joint_id) == 2.0);

    revolute_joint_set_max_motor_torque(&mut f.world, joint_id, 40.0);
    ensure!(revolute_joint_get_max_motor_torque(&mut f.world, joint_id) == 40.0);

    let _motor_torque = revolute_joint_get_motor_torque(&mut f.world, joint_id);

    finish_joint(joint_id, f.world);
}

#[test]
fn test_spherical_joint() {
    let mut f = create_joint_fixture();

    let mut def = default_spherical_joint_def();
    set_common_frames(&mut def.base, &f);
    let joint_id = create_spherical_joint(&mut f.world, &def);

    exercise_joint_base(joint_id, &mut f.world, f.ground_id, f.body_id, JointType::Spherical);

    spherical_joint_enable_cone_limit(&mut f.world, joint_id, true);
    ensure!(spherical_joint_is_cone_limit_enabled(&mut f.world, joint_id) == true);

    spherical_joint_set_cone_limit(&mut f.world, joint_id, 0.5);
    ensure!(spherical_joint_get_cone_limit(&mut f.world, joint_id) == 0.5);

    let _cone_angle = spherical_joint_get_cone_angle(&mut f.world, joint_id);

    spherical_joint_enable_twist_limit(&mut f.world, joint_id, true);
    ensure!(spherical_joint_is_twist_limit_enabled(&mut f.world, joint_id) == true);

    spherical_joint_set_twist_limits(&mut f.world, joint_id, -0.5, 0.5);
    ensure!(spherical_joint_get_lower_twist_limit(&mut f.world, joint_id) == -0.5);
    ensure!(spherical_joint_get_upper_twist_limit(&mut f.world, joint_id) == 0.5);

    let _twist_angle = spherical_joint_get_twist_angle(&mut f.world, joint_id);

    spherical_joint_enable_spring(&mut f.world, joint_id, true);
    ensure!(spherical_joint_is_spring_enabled(&mut f.world, joint_id) == true);

    spherical_joint_set_spring_hertz(&mut f.world, joint_id, 5.0);
    ensure!(spherical_joint_get_spring_hertz(&mut f.world, joint_id) == 5.0);

    spherical_joint_set_spring_damping_ratio(&mut f.world, joint_id, 0.5);
    ensure!(spherical_joint_get_spring_damping_ratio(&mut f.world, joint_id) == 0.5);

    // 90 degrees about z, a unit quaternion that round-trips through storage
    let target_rotation = Quat { v: vec3(0.0, 0.0, 0.7071068), s: 0.7071068 };
    spherical_joint_set_target_rotation(&mut f.world, joint_id, target_rotation);
    let got_rotation = spherical_joint_get_target_rotation(&mut f.world, joint_id);
    ensure_small!(got_rotation.v.x - target_rotation.v.x, 1.0e-5);
    ensure_small!(got_rotation.v.y - target_rotation.v.y, 1.0e-5);
    ensure_small!(got_rotation.v.z - target_rotation.v.z, 1.0e-5);
    ensure_small!(got_rotation.s - target_rotation.s, 1.0e-5);

    spherical_joint_enable_motor(&mut f.world, joint_id, true);
    ensure!(spherical_joint_is_motor_enabled(&mut f.world, joint_id) == true);

    let motor_velocity = vec3(0.1, 0.2, 0.3);
    spherical_joint_set_motor_velocity(&mut f.world, joint_id, motor_velocity);
    let got_motor_velocity = spherical_joint_get_motor_velocity(&mut f.world, joint_id);
    ensure!(got_motor_velocity.x == 0.1 && got_motor_velocity.y == 0.2 && got_motor_velocity.z == 0.3);

    spherical_joint_set_max_motor_torque(&mut f.world, joint_id, 50.0);
    ensure!(spherical_joint_get_max_motor_torque(&mut f.world, joint_id) == 50.0);

    let _motor_torque = spherical_joint_get_motor_torque(&mut f.world, joint_id);

    finish_joint(joint_id, f.world);
}

#[test]
fn test_weld_joint() {
    let mut f = create_joint_fixture();

    let mut def = default_weld_joint_def();
    set_common_frames(&mut def.base, &f);
    let joint_id = create_weld_joint(&mut f.world, &def);

    exercise_joint_base(joint_id, &mut f.world, f.ground_id, f.body_id, JointType::Weld);

    weld_joint_set_linear_hertz(&mut f.world, joint_id, 3.0);
    ensure!(weld_joint_get_linear_hertz(&mut f.world, joint_id) == 3.0);

    weld_joint_set_linear_damping_ratio(&mut f.world, joint_id, 0.5);
    ensure!(weld_joint_get_linear_damping_ratio(&mut f.world, joint_id) == 0.5);

    weld_joint_set_angular_hertz(&mut f.world, joint_id, 4.0);
    ensure!(weld_joint_get_angular_hertz(&mut f.world, joint_id) == 4.0);

    weld_joint_set_angular_damping_ratio(&mut f.world, joint_id, 0.7);
    ensure!(weld_joint_get_angular_damping_ratio(&mut f.world, joint_id) == 0.7);

    finish_joint(joint_id, f.world);
}

#[test]
fn test_wheel_joint() {
    let mut f = create_joint_fixture();

    let mut def = default_wheel_joint_def();
    set_common_frames(&mut def.base, &f);
    let joint_id = create_wheel_joint(&mut f.world, &def);

    exercise_joint_base(joint_id, &mut f.world, f.ground_id, f.body_id, JointType::Wheel);

    wheel_joint_enable_suspension(&mut f.world, joint_id, true);
    ensure!(wheel_joint_is_suspension_enabled(&mut f.world, joint_id) == true);

    wheel_joint_set_suspension_hertz(&mut f.world, joint_id, 5.0);
    ensure!(wheel_joint_get_suspension_hertz(&mut f.world, joint_id) == 5.0);

    wheel_joint_set_suspension_damping_ratio(&mut f.world, joint_id, 0.5);
    ensure!(wheel_joint_get_suspension_damping_ratio(&mut f.world, joint_id) == 0.5);

    wheel_joint_enable_suspension_limit(&mut f.world, joint_id, true);
    ensure!(wheel_joint_is_suspension_limit_enabled(&mut f.world, joint_id) == true);

    wheel_joint_set_suspension_limits(&mut f.world, joint_id, -1.0, 1.0);
    ensure!(wheel_joint_get_lower_suspension_limit(&mut f.world, joint_id) == -1.0);
    ensure!(wheel_joint_get_upper_suspension_limit(&mut f.world, joint_id) == 1.0);

    wheel_joint_enable_spin_motor(&mut f.world, joint_id, true);
    ensure!(wheel_joint_is_spin_motor_enabled(&mut f.world, joint_id) == true);

    wheel_joint_set_spin_motor_speed(&mut f.world, joint_id, 6.0);
    ensure!(wheel_joint_get_spin_motor_speed(&mut f.world, joint_id) == 6.0);

    wheel_joint_set_max_spin_torque(&mut f.world, joint_id, 35.0);
    ensure!(wheel_joint_get_max_spin_torque(&mut f.world, joint_id) == 35.0);

    let _spin_speed = wheel_joint_get_spin_speed(&mut f.world, joint_id);
    let _spin_torque = wheel_joint_get_spin_torque(&mut f.world, joint_id);

    wheel_joint_enable_steering(&mut f.world, joint_id, true);
    ensure!(wheel_joint_is_steering_enabled(&mut f.world, joint_id) == true);

    wheel_joint_set_steering_hertz(&mut f.world, joint_id, 7.0);
    ensure!(wheel_joint_get_steering_hertz(&mut f.world, joint_id) == 7.0);

    wheel_joint_set_steering_damping_ratio(&mut f.world, joint_id, 0.8);
    ensure!(wheel_joint_get_steering_damping_ratio(&mut f.world, joint_id) == 0.8);

    wheel_joint_set_max_steering_torque(&mut f.world, joint_id, 45.0);
    ensure!(wheel_joint_get_max_steering_torque(&mut f.world, joint_id) == 45.0);

    wheel_joint_enable_steering_limit(&mut f.world, joint_id, true);
    ensure!(wheel_joint_is_steering_limit_enabled(&mut f.world, joint_id) == true);

    wheel_joint_set_steering_limits(&mut f.world, joint_id, -0.6, 0.6);
    ensure!(wheel_joint_get_lower_steering_limit(&mut f.world, joint_id) == -0.6);
    ensure!(wheel_joint_get_upper_steering_limit(&mut f.world, joint_id) == 0.6);

    wheel_joint_set_target_steering_angle(&mut f.world, joint_id, 0.25);
    ensure!(wheel_joint_get_target_steering_angle(&mut f.world, joint_id) == 0.25);

    let _steering_angle = wheel_joint_get_steering_angle(&mut f.world, joint_id);
    let _steering_torque = wheel_joint_get_steering_torque(&mut f.world, joint_id);

    finish_joint(joint_id, f.world);
}

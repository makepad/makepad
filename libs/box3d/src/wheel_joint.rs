// Port of box3d/src/wheel_joint.c
// See constraints.pdf
//
// Deviations: recording hooks (B3_REC) and b3DrawWheelJoint are not ported.
// get_wheel_joint_force mirrors the C code verbatim, including its use of
// lowerSuspensionLimit (not lowerSuspensionImpulse) in the impulse sum.

use crate::b3_assert;
use crate::body::{DYNAMIC_FLAG, IDENTITY_BODY_STATE};
use crate::core::NULL_INDEX;
use crate::id::JointId;
use crate::joint::{JointSim, JointUnion};
use crate::math_functions::*;
use crate::math_internal::{blend3, solve2, Matrix2};
use crate::physics_world::{World, AWAKE_SET};
use crate::solver::{make_soft, StateAccess, StepContext};
use crate::types::JointType;

fn get_wheel(base: &mut JointSim) -> &mut crate::joint::WheelJoint {
    match &mut base.joint {
        JointUnion::Wheel(j) => j,
        _ => panic!("wrong joint type"),
    }
}

fn get_wheel_ref(base: &JointSim) -> &crate::joint::WheelJoint {
    match &base.joint {
        JointUnion::Wheel(j) => j,
        _ => panic!("wrong joint type"),
    }
}

pub fn wheel_joint_enable_suspension(world: &mut World, joint_id: JointId, enable_spring: bool) {

    crate::recording::rec_op(world, crate::recording::RecOp::WheelJointEnableSuspension, |b| {
        b.w_jointid(joint_id);
        b.w_bool(enable_spring);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    let joint = get_wheel(base);
    if enable_spring != joint.enable_suspension_spring {
        joint.enable_suspension_spring = enable_spring;
        joint.suspension_spring_impulse = 0.0;
    }
}

pub fn wheel_joint_is_suspension_enabled(world: &mut World, joint_id: JointId) -> bool {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).enable_suspension_spring
}

pub fn wheel_joint_set_suspension_hertz(world: &mut World, joint_id: JointId, hertz: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::WheelJointSetSuspensionHertz, |b| {
        b.w_jointid(joint_id);
        b.w_f32(hertz);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).suspension_hertz = hertz;
}

pub fn wheel_joint_get_suspension_hertz(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).suspension_hertz
}

pub fn wheel_joint_set_suspension_damping_ratio(world: &mut World, joint_id: JointId, damping_ratio: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::WheelJointSetSuspensionDampingRatio, |b| {
        b.w_jointid(joint_id);
        b.w_f32(damping_ratio);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).suspension_damping_ratio = damping_ratio;
}

pub fn wheel_joint_get_suspension_damping_ratio(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).suspension_damping_ratio
}

pub fn wheel_joint_enable_suspension_limit(world: &mut World, joint_id: JointId, enable_limit: bool) {

    crate::recording::rec_op(world, crate::recording::RecOp::WheelJointEnableSuspensionLimit, |b| {
        b.w_jointid(joint_id);
        b.w_bool(enable_limit);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    let joint = get_wheel(base);
    if joint.enable_suspension_limit != enable_limit {
        joint.lower_suspension_impulse = 0.0;
        joint.upper_suspension_impulse = 0.0;
        joint.enable_suspension_limit = enable_limit;
    }
}

pub fn wheel_joint_is_suspension_limit_enabled(world: &mut World, joint_id: JointId) -> bool {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).enable_suspension_limit
}

pub fn wheel_joint_get_lower_suspension_limit(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).lower_suspension_limit
}

pub fn wheel_joint_get_upper_suspension_limit(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).upper_suspension_limit
}

pub fn wheel_joint_set_suspension_limits(world: &mut World, joint_id: JointId, lower: f32, upper: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::WheelJointSetSuspensionLimits, |b| {
        b.w_jointid(joint_id);
        b.w_f32(lower);
        b.w_f32(upper);
    });
    b3_assert!(lower <= upper);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    let joint = get_wheel(base);
    if lower != joint.lower_suspension_limit || upper != joint.upper_suspension_limit {
        joint.lower_suspension_limit = lower;
        joint.upper_suspension_limit = upper;
        joint.lower_suspension_impulse = 0.0;
        joint.upper_suspension_impulse = 0.0;
    }
}

pub fn wheel_joint_enable_spin_motor(world: &mut World, joint_id: JointId, enable_motor: bool) {

    crate::recording::rec_op(world, crate::recording::RecOp::WheelJointEnableSpinMotor, |b| {
        b.w_jointid(joint_id);
        b.w_bool(enable_motor);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    let joint = get_wheel(base);
    if joint.enable_spin_motor != enable_motor {
        joint.spin_impulse = 0.0;
        joint.enable_spin_motor = enable_motor;
    }
}

pub fn wheel_joint_is_spin_motor_enabled(world: &mut World, joint_id: JointId) -> bool {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).enable_spin_motor
}

pub fn wheel_joint_set_spin_motor_speed(world: &mut World, joint_id: JointId, motor_speed: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::WheelJointSetSpinMotorSpeed, |b| {
        b.w_jointid(joint_id);
        b.w_f32(motor_speed);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).spin_speed = motor_speed;
}

pub fn wheel_joint_get_spin_motor_speed(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).spin_speed
}

pub fn wheel_joint_set_max_spin_torque(world: &mut World, joint_id: JointId, torque: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::WheelJointSetMaxSpinTorque, |b| {
        b.w_jointid(joint_id);
        b.w_f32(torque);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).max_spin_torque = torque;
}

pub fn wheel_joint_get_max_spin_torque(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).max_spin_torque
}

pub fn wheel_joint_enable_steering(world: &mut World, joint_id: JointId, flag: bool) {

    crate::recording::rec_op(world, crate::recording::RecOp::WheelJointEnableSteering, |b| {
        b.w_jointid(joint_id);
        b.w_bool(flag);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    let joint = get_wheel(base);
    if joint.enable_steering != flag {
        joint.angular_impulse = vec2(0.0, 0.0);
        joint.enable_steering = flag;
    }
}

pub fn wheel_joint_is_steering_enabled(world: &mut World, joint_id: JointId) -> bool {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).enable_steering
}

pub fn wheel_joint_set_steering_hertz(world: &mut World, joint_id: JointId, hertz: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::WheelJointSetSteeringHertz, |b| {
        b.w_jointid(joint_id);
        b.w_f32(hertz);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).steering_hertz = hertz;
}

pub fn wheel_joint_get_steering_hertz(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).steering_hertz
}

pub fn wheel_joint_set_steering_damping_ratio(world: &mut World, joint_id: JointId, damping_ratio: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::WheelJointSetSteeringDampingRatio, |b| {
        b.w_jointid(joint_id);
        b.w_f32(damping_ratio);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).steering_damping_ratio = damping_ratio;
}

pub fn wheel_joint_get_steering_damping_ratio(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).steering_damping_ratio
}

pub fn wheel_joint_set_max_steering_torque(world: &mut World, joint_id: JointId, max_torque: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::WheelJointSetMaxSteeringTorque, |b| {
        b.w_jointid(joint_id);
        b.w_f32(max_torque);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).max_steering_torque = max_torque;
}

pub fn wheel_joint_get_max_steering_torque(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).max_steering_torque
}

pub fn wheel_joint_enable_steering_limit(world: &mut World, joint_id: JointId, flag: bool) {

    crate::recording::rec_op(world, crate::recording::RecOp::WheelJointEnableSteeringLimit, |b| {
        b.w_jointid(joint_id);
        b.w_bool(flag);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    let joint = get_wheel(base);
    if joint.enable_steering_limit != flag {
        joint.lower_steering_impulse = 0.0;
        joint.upper_steering_impulse = 0.0;
        joint.enable_steering_limit = flag;
    }
}

pub fn wheel_joint_is_steering_limit_enabled(world: &mut World, joint_id: JointId) -> bool {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).enable_steering_limit
}

pub fn wheel_joint_get_lower_steering_limit(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).lower_steering_limit
}

pub fn wheel_joint_get_upper_steering_limit(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).upper_steering_limit
}

pub fn wheel_joint_set_steering_limits(world: &mut World, joint_id: JointId, lower_radians: f32, upper_radians: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::WheelJointSetSteeringLimits, |b| {
        b.w_jointid(joint_id);
        b.w_f32(lower_radians);
        b.w_f32(upper_radians);
    });
    b3_assert!(lower_radians <= upper_radians);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    let joint = get_wheel(base);
    joint.lower_steering_limit = lower_radians;
    joint.upper_steering_limit = upper_radians;
}

pub fn wheel_joint_set_target_steering_angle(world: &mut World, joint_id: JointId, radians: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::WheelJointSetTargetSteeringAngle, |b| {
        b.w_jointid(joint_id);
        b.w_f32(radians);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).target_steering_angle = radians;
}

pub fn wheel_joint_get_target_steering_angle(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    get_wheel(base).target_steering_angle
}

pub fn wheel_joint_get_spin_speed(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    let id_a = base.body_id_a;
    let id_b = base.body_id_b;
    let local_frame_b_q = base.local_frame_b.q;

    let body_sim_b = *crate::body::get_body_sim_from_id(world, id_b);

    let quat_b = mul_quat(body_sim_b.transform.q, local_frame_b_q);
    let spin_axis = rotate_vector(quat_b, Vec3::AXIS_Z);

    let mut w_a = Vec3::ZERO;
    if let Some(state_a) = crate::body::get_body_state_from_id_mut(world, id_a) {
        w_a = state_a.angular_velocity;
    }

    let mut w_b = Vec3::ZERO;
    if let Some(state_b) = crate::body::get_body_state_from_id_mut(world, id_b) {
        w_b = state_b.angular_velocity;
    }

    dot(sub(w_b, w_a), spin_axis)
}

pub fn wheel_joint_get_spin_torque(world: &mut World, joint_id: JointId) -> f32 {
    let inv_h = world.inv_h;
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    inv_h * get_wheel(base).spin_impulse
}

pub fn wheel_joint_get_steering_angle(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    let id_a = base.body_id_a;
    let id_b = base.body_id_b;
    let local_frame_a_q = base.local_frame_a.q;
    let local_frame_b_q = base.local_frame_b.q;

    let body_sim_a = *crate::body::get_body_sim_from_id(world, id_a);
    let body_sim_b = *crate::body::get_body_sim_from_id(world, id_b);

    let quat_a = mul_quat(body_sim_a.transform.q, local_frame_a_q);
    let quat_b = mul_quat(body_sim_b.transform.q, local_frame_b_q);

    let matrix_a = make_matrix_from_quat(quat_a);
    let matrix_b = make_matrix_from_quat(quat_b);

    // Twist around x-axis
    let cs = dot(matrix_b.cz, matrix_a.cz);
    let ss = -dot(matrix_b.cz, matrix_a.cy);

    atan2(ss, cs)
}

pub fn wheel_joint_get_steering_torque(world: &mut World, joint_id: JointId) -> f32 {
    let inv_h = world.inv_h;
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Wheel);
    inv_h * get_wheel(base).steering_spring_impulse
}

pub fn get_wheel_joint_force(world: &World, base: &JointSim) -> Vec3 {
    let transform_a = crate::body::get_body_transform(world, base.body_id_a);
    let joint = get_wheel_ref(base);

    // impulse in joint space
    let impulse = Vec3 {
        x: joint.linear_impulse.x,
        y: joint.linear_impulse.y,
        z: joint.lower_suspension_limit + joint.upper_suspension_impulse + joint.suspension_spring_impulse,
    };

    // convert impulse to force
    let mut force = mul_sv(world.inv_h, impulse);

    // convert to body space
    force = rotate_vector(base.local_frame_a.q, force);

    // convert to world space
    force = rotate_vector(transform_a.q, force);
    force
}

pub fn get_wheel_joint_torque(world: &World, base: &JointSim) -> Vec3 {
    b3_assert!(base.joint_type == JointType::Wheel);

    // chase body id to the solver set where the body lives
    let id_a = base.body_id_a;

    let body_sim_a = crate::body::get_body_sim_from_id(world, id_a);

    let q_a = mul_quat(body_sim_a.transform.q, base.local_frame_a.q);

    let matrix_a = make_matrix_from_quat(q_a);

    mul_sv(world.inv_h * get_wheel_ref(base).spin_impulse, matrix_a.cz)
}

// See constraints.pdf

pub fn prepare_wheel_joint(base: &mut JointSim, world: &World, context: &StepContext) {
    b3_assert!(base.joint_type == JointType::Wheel);

    // chase body id to the solver set where the body lives
    let body_a = &world.bodies[base.body_id_a as usize];
    let body_b = &world.bodies[base.body_id_b as usize];

    b3_assert!(body_a.set_index == AWAKE_SET || body_b.set_index == AWAKE_SET);

    let local_index_a = body_a.local_index;
    let local_index_b = body_b.local_index;

    let body_sim_a = crate::joint::get_solve_body_sim(world, context, body_a.set_index, local_index_a);
    let body_sim_b = crate::joint::get_solve_body_sim(world, context, body_b.set_index, local_index_b);

    base.inv_mass_a = body_sim_a.inv_mass;
    base.inv_mass_b = body_sim_b.inv_mass;
    base.inv_i_a = body_sim_a.inv_inertia_world;
    base.inv_i_b = body_sim_b.inv_inertia_world;

    let inv_inertia_sum = add_mm(base.inv_i_a, base.inv_i_b);
    base.fixed_rotation = det(inv_inertia_sum) < 1000.0 * f32::MIN_POSITIVE;

    let set_index_a = body_a.set_index;
    let set_index_b = body_b.set_index;
    let local_frame_a = base.local_frame_a;
    let local_frame_b = base.local_frame_b;
    let inv_mass_a = base.inv_mass_a;
    let inv_mass_b = base.inv_mass_b;
    let inv_i_a = base.inv_i_a;
    let inv_i_b = base.inv_i_b;

    let joint = get_wheel(base);

    joint.index_a = if set_index_a == AWAKE_SET { local_index_a } else { NULL_INDEX };
    joint.index_b = if set_index_b == AWAKE_SET { local_index_b } else { NULL_INDEX };

    // Compute joint anchor frames with world space rotation, relative to center of mass
    joint.frame_a.q = mul_quat(body_sim_a.transform.q, local_frame_a.q);
    joint.frame_a.p = rotate_vector(body_sim_a.transform.q, sub(local_frame_a.p, body_sim_a.local_center));
    joint.frame_b.q = mul_quat(body_sim_b.transform.q, local_frame_b.q);
    joint.frame_b.p = rotate_vector(body_sim_b.transform.q, sub(local_frame_b.p, body_sim_b.local_center));

    // Compute the initial center delta. Incremental position updates are relative to this.
    joint.delta_center = sub_pos(body_sim_b.center, body_sim_a.center);

    let r_a = joint.frame_a.p;
    let r_b = joint.frame_b.p;

    let matrix_a = make_matrix_from_quat(joint.frame_a.q);
    let matrix_b = make_matrix_from_quat(joint.frame_b.q);

    // todo use fresh effective masses in the sub-step to avoid divergence like I saw for the prismatic joint

    {
        let suspension_axis = matrix_a.cx;
        let r_an = cross(r_a, suspension_axis);
        let r_bn = cross(r_b, suspension_axis);

        let k = inv_mass_a + inv_mass_b + dot(r_an, mul_mv(inv_i_a, r_an)) + dot(r_bn, mul_mv(inv_i_b, r_bn));
        joint.suspension_mass = if k > 0.0 { 1.0 / k } else { 0.0 };
    }

    joint.suspension_softness = make_soft(joint.suspension_hertz, joint.suspension_damping_ratio, context.h);
    joint.steering_softness = make_soft(joint.steering_hertz, joint.steering_damping_ratio, context.h);

    {
        // Rotation axis is the z-axis of body A.
        let spin_axis = matrix_b.cz;
        let k = dot(spin_axis, mul_mv(inv_inertia_sum, spin_axis));
        joint.spin_mass = if k > 0.0 { 1.0 / k } else { 0.0 };
    }

    {
        // Twist constraint around x-axis
        let cs = dot(matrix_b.cz, matrix_a.cz);
        let ss = -dot(matrix_b.cz, matrix_a.cy);
        let mut den = ss.mul_add(ss, cs * cs);
        den = if den > 0.0 { 1.0 / den } else { 0.0 };
        let steering_axis = mul_sv(den, cross(matrix_b.cz, sub(mul_sv(-cs, matrix_a.cy), mul_sv(ss, matrix_a.cz))));

        let k = dot(steering_axis, mul_mv(inv_inertia_sum, steering_axis));
        joint.steering_mass = if k > 0.0 { 1.0 / k } else { 0.0 };
    }

    if !context.enable_warm_starting {
        joint.linear_impulse = vec2(0.0, 0.0);
        joint.angular_impulse = vec2(0.0, 0.0);
        joint.spin_impulse = 0.0;
        joint.suspension_spring_impulse = 0.0;
        joint.lower_suspension_impulse = 0.0;
        joint.upper_suspension_impulse = 0.0;
        joint.steering_spring_impulse = 0.0;
        joint.lower_steering_impulse = 0.0;
        joint.upper_steering_impulse = 0.0;
    }
}

pub fn warm_start_wheel_joint(base: &mut JointSim, states: &StateAccess, _context: &StepContext) {
    b3_assert!(base.joint_type == JointType::Wheel);

    let m_a = base.inv_mass_a;
    let m_b = base.inv_mass_b;
    let i_a = base.inv_i_a;
    let i_b = base.inv_i_b;

    let joint = get_wheel(base);

    // dummy state for static bodies
    let index_a = joint.index_a;
    let index_b = joint.index_b;
    // Field copies through short-lived borrows + a velocities-only
    // write-back (see spherical_joint.rs / StateAccess::set_velocities).
    let (mut v_a, mut w_a, dq_a, dp_a, flags_a) = {
        let s = if index_a == NULL_INDEX { &IDENTITY_BODY_STATE } else { states.get_ref(index_a as usize) };
        (s.linear_velocity, s.angular_velocity, s.delta_rotation, s.delta_position, s.flags)
    };
    let (mut v_b, mut w_b, dq_b, dp_b, flags_b) = {
        let s = if index_b == NULL_INDEX { &IDENTITY_BODY_STATE } else { states.get_ref(index_b as usize) };
        (s.linear_velocity, s.angular_velocity, s.delta_rotation, s.delta_position, s.flags)
    };

    let r_a = rotate_vector(dq_a, joint.frame_a.p);
    let r_b = rotate_vector(dq_b, joint.frame_b.p);

    let d = add(add(sub(dp_b, dp_a), joint.delta_center), sub(r_b, r_a));

    let quat_a = mul_quat(dq_a, joint.frame_a.q);
    let mut quat_b = mul_quat(dq_b, joint.frame_b.q);
    if dot_quat(quat_a, quat_b) < 0.0 {
        // this keeps the rotation angle in the range [-pi, pi]
        quat_b = negate_quat(quat_b);
    }

    let matrix_a = make_matrix_from_quat(quat_a);
    let matrix_b = make_matrix_from_quat(quat_b);

    let s_ax = cross(add(d, r_a), matrix_a.cx);
    let s_bx = cross(r_b, matrix_a.cx);
    let s_ay = cross(add(d, r_a), matrix_a.cy);
    let s_by = cross(r_b, matrix_a.cy);
    let s_az = cross(add(d, r_a), matrix_a.cz);
    let s_bz = cross(r_b, matrix_a.cz);

    let suspension_impulse =
        joint.suspension_spring_impulse + joint.lower_suspension_impulse - joint.upper_suspension_impulse;

    let linear_impulse_y = joint.linear_impulse.x;
    let linear_impulse_z = joint.linear_impulse.y;
    let angular_impulse_x = joint.angular_impulse.x;
    let angular_impulse_y = joint.angular_impulse.y;

    let linear_impulse = blend3(suspension_impulse, matrix_a.cx, linear_impulse_y, matrix_a.cy, linear_impulse_z, matrix_a.cz);
    let angular_impulse_a = blend3(suspension_impulse, s_ax, linear_impulse_y, s_ay, linear_impulse_z, s_az);
    let angular_impulse_b = blend3(suspension_impulse, s_bx, linear_impulse_y, s_by, linear_impulse_z, s_bz);
    let mut angular_impulse = mul_sv(joint.spin_impulse, matrix_a.cz);

    let spin_axis = matrix_b.cz;

    if joint.enable_steering {
        // Twist constraint around x-axis
        let cs = dot(matrix_b.cz, matrix_a.cz);
        let ss = -dot(matrix_b.cz, matrix_a.cy);
        let mut den = ss.mul_add(ss, cs * cs);
        den = if den > 0.0 { 1.0 / den } else { 0.0 };
        let steering_axis = mul_sv(den, cross(matrix_b.cz, sub(mul_sv(-cs, matrix_a.cy), mul_sv(ss, matrix_a.cz))));

        let perp_axis = cross(spin_axis, matrix_a.cx);
        let steering_impulse = joint.steering_spring_impulse + joint.lower_steering_impulse - joint.upper_steering_impulse;
        angular_impulse = blend3(angular_impulse_x, perp_axis, joint.spin_impulse, spin_axis, steering_impulse, steering_axis);
    } else {
        let rel_q = inv_mul_quat(quat_a, quat_b);
        let perp_axis_x =
            mul_sv(0.5, rotate_vector(quat_a, add(mul_sv(rel_q.s, Vec3::AXIS_X), cross(rel_q.v, Vec3::AXIS_X))));
        let perp_axis_y =
            mul_sv(0.5, rotate_vector(quat_a, add(mul_sv(rel_q.s, Vec3::AXIS_Y), cross(rel_q.v, Vec3::AXIS_Y))));
        angular_impulse = add(
            angular_impulse,
            blend3(angular_impulse_x, perp_axis_x, angular_impulse_y, perp_axis_y, joint.spin_impulse, spin_axis),
        );
    }

    if flags_a & DYNAMIC_FLAG != 0 {
        v_a = mul_sub(v_a, m_a, linear_impulse);
        w_a = sub(w_a, mul_mv(i_a, add(angular_impulse_a, angular_impulse)));
    }

    if flags_b & DYNAMIC_FLAG != 0 {
        v_b = mul_add(v_b, m_b, linear_impulse);
        w_b = add(w_b, mul_mv(i_b, add(angular_impulse_b, angular_impulse)));
    }

    // C stores unconditionally through the state pointer for dynamic bodies;
    // the non-dynamic write in the old code was a byte-identical no-op.
    if index_a != NULL_INDEX && (flags_a & DYNAMIC_FLAG != 0) {
        states.set_velocities(index_a as usize, v_a, w_a);
    }
    if index_b != NULL_INDEX && (flags_b & DYNAMIC_FLAG != 0) {
        states.set_velocities(index_b as usize, v_b, w_b);
    }
}

pub fn solve_wheel_joint(base: &mut JointSim, states: &StateAccess, context: &StepContext, use_bias: bool) {
    b3_assert!(base.joint_type == JointType::Wheel);

    let m_a = base.inv_mass_a;
    let m_b = base.inv_mass_b;
    let i_a = base.inv_i_a;
    let i_b = base.inv_i_b;
    let fixed_rotation = base.fixed_rotation;
    let constraint_softness = base.constraint_softness;

    let joint = get_wheel(base);

    // dummy state for static bodies
    let index_a = joint.index_a;
    let index_b = joint.index_b;
    // Field copies through short-lived borrows + a velocities-only
    // write-back (see spherical_joint.rs / StateAccess::set_velocities).
    let (mut v_a, mut w_a, dq_a, dp_a, flags_a) = {
        let s = if index_a == NULL_INDEX { &IDENTITY_BODY_STATE } else { states.get_ref(index_a as usize) };
        (s.linear_velocity, s.angular_velocity, s.delta_rotation, s.delta_position, s.flags)
    };
    let (mut v_b, mut w_b, dq_b, dp_b, flags_b) = {
        let s = if index_b == NULL_INDEX { &IDENTITY_BODY_STATE } else { states.get_ref(index_b as usize) };
        (s.linear_velocity, s.angular_velocity, s.delta_rotation, s.delta_position, s.flags)
    };

    // current anchors
    let r_a = rotate_vector(dq_a, joint.frame_a.p);
    let r_b = rotate_vector(dq_b, joint.frame_b.p);

    let quat_a = mul_quat(dq_a, joint.frame_a.q);
    let mut quat_b = mul_quat(dq_b, joint.frame_b.q);

    if dot_quat(quat_a, quat_b) < 0.0 {
        // this keeps the rotation angle in the range [-pi, pi]
        quat_b = negate_quat(quat_b);
    }

    let rel_q = inv_mul_quat(quat_a, quat_b);
    let matrix_a = make_matrix_from_quat(quat_a);
    let matrix_b = make_matrix_from_quat(quat_b);

    let d = add(add(sub(dp_b, dp_a), joint.delta_center), sub(r_b, r_a));
    let s_ax = cross(add(d, r_a), matrix_a.cx);
    let s_bx = cross(r_b, matrix_a.cx);
    let s_ay = cross(add(d, r_a), matrix_a.cy);
    let s_by = cross(r_b, matrix_a.cy);
    let s_az = cross(add(d, r_a), matrix_a.cz);
    let s_bz = cross(r_b, matrix_a.cz);

    let translation = dot(matrix_a.cx, d);

    // Steering param ib = cz_b, ia = cz_a, ja = -cy_a
    let cs = dot(matrix_b.cz, matrix_a.cz);
    let ss = -dot(matrix_b.cz, matrix_a.cy);
    let mut den = ss.mul_add(ss, cs * cs);
    den = if den > 0.0 { 1.0 / den } else { 0.0 };
    let steering_axis = mul_sv(den, cross(matrix_b.cz, sub(mul_sv(-cs, matrix_a.cy), mul_sv(ss, matrix_a.cz))));

    // motor constraint
    if joint.enable_spin_motor && !fixed_rotation {
        let spin_axis = matrix_b.cz;
        let cdot = dot(sub(w_b, w_a), spin_axis) - joint.spin_speed;
        let mut impulse = -joint.spin_mass * cdot;
        let old_impulse = joint.spin_impulse;
        let max_impulse = context.h * joint.max_spin_torque;
        joint.spin_impulse = clamp_float(joint.spin_impulse + impulse, -max_impulse, max_impulse);
        impulse = joint.spin_impulse - old_impulse;

        w_a = sub(w_a, mul_mv(i_a, mul_sv(impulse, spin_axis)));
        w_b = add(w_b, mul_mv(i_b, mul_sv(impulse, spin_axis)));
    }

    // suspension
    if joint.enable_suspension_spring {
        // This is a real spring and should be applied even during relax
        let c = translation;
        let bias = joint.suspension_softness.bias_rate * c;
        let mass_scale = joint.suspension_softness.mass_scale;
        let impulse_scale = joint.suspension_softness.impulse_scale;

        let cdot = dot(matrix_a.cx, sub(v_b, v_a)) + dot(s_bx, w_b) - dot(s_ax, w_a);
        let impulse = (-impulse_scale).mul_add(joint.suspension_spring_impulse, -mass_scale * joint.suspension_mass * (cdot + bias));
        joint.suspension_spring_impulse += impulse;

        let linear_impulse = mul_sv(impulse, matrix_a.cx);
        let angular_impulse_a = mul_sv(impulse, s_ax);
        let angular_impulse_b = mul_sv(impulse, s_bx);

        v_a = mul_sub(v_a, m_a, linear_impulse);
        w_a = sub(w_a, mul_mv(i_a, angular_impulse_a));
        v_b = mul_add(v_b, m_b, linear_impulse);
        w_b = add(w_b, mul_mv(i_b, angular_impulse_b));
    }

    // steering
    if joint.enable_steering && !fixed_rotation {
        let steering_angle = atan2(ss, cs);

        {
            // This is a real spring and should be applied even during relax
            let c = steering_angle - joint.target_steering_angle;
            let bias = joint.steering_softness.bias_rate * c;
            let mass_scale = joint.steering_softness.mass_scale;
            let impulse_scale = joint.steering_softness.impulse_scale;

            let cdot = dot(steering_axis, sub(w_b, w_a));
            let old_impulse = joint.steering_spring_impulse;
            let mut impulse = (-impulse_scale).mul_add(old_impulse, -mass_scale * joint.steering_mass * (cdot + bias));
            let max_impulse = context.h * joint.max_steering_torque;
            joint.steering_spring_impulse = clamp_float(old_impulse + impulse, -max_impulse, max_impulse);
            impulse = joint.steering_spring_impulse - old_impulse;

            w_a = sub(w_a, mul_mv(i_a, mul_sv(impulse, steering_axis)));
            w_b = add(w_b, mul_mv(i_b, mul_sv(impulse, steering_axis)));
        }

        if joint.enable_steering_limit {
            // Lower limit
            {
                let c = steering_angle - joint.lower_steering_limit;
                let mut bias = 0.0;
                let mut mass_scale = 1.0;
                let mut impulse_scale = 0.0;

                if c > 0.0 {
                    // speculation
                    bias = c * context.inv_h;
                } else if use_bias {
                    bias = constraint_softness.bias_rate * c;
                    mass_scale = constraint_softness.mass_scale;
                    impulse_scale = constraint_softness.impulse_scale;
                }

                let cdot = dot(steering_axis, sub(w_b, w_a));
                let old_impulse = joint.lower_steering_impulse;
                let mut impulse = (-impulse_scale).mul_add(old_impulse, -mass_scale * joint.steering_mass * (cdot + bias));
                joint.lower_steering_impulse = max_float(old_impulse + impulse, 0.0);
                impulse = joint.lower_steering_impulse - old_impulse;

                w_a = sub(w_a, mul_mv(i_a, mul_sv(impulse, steering_axis)));
                w_b = add(w_b, mul_mv(i_b, mul_sv(impulse, steering_axis)));
            }

            // Upper limit
            // Note: signs are flipped to keep c positive when the constraint is satisfied.
            // This also keeps the impulse positive when the limit is active.
            {
                // sign flipped
                let c = joint.upper_steering_limit - steering_angle;
                let mut bias = 0.0;
                let mut mass_scale = 1.0;
                let mut impulse_scale = 0.0;

                if c > 0.0 {
                    // speculation
                    bias = c * context.inv_h;
                } else if use_bias {
                    bias = constraint_softness.bias_rate * c;
                    mass_scale = constraint_softness.mass_scale;
                    impulse_scale = constraint_softness.impulse_scale;
                }

                // sign flipped on cdot
                let cdot = dot(steering_axis, sub(w_a, w_b));
                let old_impulse = joint.upper_steering_impulse;
                let mut impulse = (-impulse_scale).mul_add(old_impulse, -mass_scale * joint.steering_mass * (cdot + bias));
                joint.upper_steering_impulse = max_float(old_impulse + impulse, 0.0);
                impulse = joint.upper_steering_impulse - old_impulse;

                // sign flipped on applied impulse
                w_a = add(w_a, mul_mv(i_a, mul_sv(impulse, steering_axis)));
                w_b = sub(w_b, mul_mv(i_b, mul_sv(impulse, steering_axis)));
            }
        }
    }

    if joint.enable_suspension_limit {
        // Lower limit
        {
            let c = translation - joint.lower_suspension_limit;
            let mut bias = 0.0;
            let mut mass_scale = 1.0;
            let mut impulse_scale = 0.0;

            if c > 0.0 {
                // speculation
                bias = c * context.inv_h;
            } else if use_bias {
                bias = constraint_softness.bias_rate * c;
                mass_scale = constraint_softness.mass_scale;
                impulse_scale = constraint_softness.impulse_scale;
            }

            let cdot = dot(matrix_a.cx, sub(v_b, v_a)) + dot(s_bx, w_b) - dot(s_ax, w_a);
            let mut impulse = (-impulse_scale).mul_add(joint.lower_suspension_impulse, -mass_scale * joint.suspension_mass * (cdot + bias));
            let old_impulse = joint.lower_suspension_impulse;
            joint.lower_suspension_impulse = max_float(old_impulse + impulse, 0.0);
            impulse = joint.lower_suspension_impulse - old_impulse;

            let linear_impulse = mul_sv(impulse, matrix_a.cx);
            let angular_impulse_a = mul_sv(impulse, s_ax);
            let angular_impulse_b = mul_sv(impulse, s_bx);

            v_a = mul_sub(v_a, m_a, linear_impulse);
            w_a = sub(w_a, mul_mv(i_a, angular_impulse_a));
            v_b = mul_add(v_b, m_b, linear_impulse);
            w_b = add(w_b, mul_mv(i_b, angular_impulse_b));
        }

        // Upper limit
        // Note: signs are flipped to keep c positive when the constraint is satisfied.
        // This also keeps the impulse positive when the limit is active.
        {
            // sign flipped
            let c = joint.upper_suspension_limit - translation;
            let mut bias = 0.0;
            let mut mass_scale = 1.0;
            let mut impulse_scale = 0.0;

            if c > 0.0 {
                // speculation
                bias = c * context.inv_h;
            } else if use_bias {
                bias = constraint_softness.bias_rate * c;
                mass_scale = constraint_softness.mass_scale;
                impulse_scale = constraint_softness.impulse_scale;
            }

            // sign flipped on cdot
            let cdot = dot(matrix_a.cx, sub(v_a, v_b)) + dot(s_ax, w_a) - dot(s_bx, w_b);
            let mut impulse = (-impulse_scale).mul_add(joint.upper_suspension_impulse, -mass_scale * joint.suspension_mass * (cdot + bias));
            let old_impulse = joint.upper_suspension_impulse;
            joint.upper_suspension_impulse = max_float(old_impulse + impulse, 0.0);
            impulse = joint.upper_suspension_impulse - old_impulse;

            let linear_impulse = mul_sv(impulse, matrix_a.cx);
            let angular_impulse_a = mul_sv(impulse, s_ax);
            let angular_impulse_b = mul_sv(impulse, s_bx);

            // sign flipped on applied impulse
            v_a = mul_add(v_a, m_a, linear_impulse);
            w_a = add(w_a, mul_mv(i_a, angular_impulse_a));
            v_b = mul_sub(v_b, m_b, linear_impulse);
            w_b = sub(w_b, mul_mv(i_b, angular_impulse_b));
        }
    }

    // Collinearity constraint
    if !fixed_rotation {
        if joint.enable_steering {
            let mut bias = 0.0;
            let mut mass_scale = 1.0;
            let mut impulse_scale = 0.0;
            if use_bias {
                let c = dot(matrix_a.cx, matrix_b.cz);

                bias = constraint_softness.bias_rate * c;
                mass_scale = constraint_softness.mass_scale;
                impulse_scale = constraint_softness.impulse_scale;
            }

            let u = cross(matrix_b.cz, matrix_a.cx);
            let cdot = dot(sub(w_b, w_a), u);

            let inv_inertia_sum = add_mm(i_a, i_b);
            let k = dot(u, mul_mv(inv_inertia_sum, u));
            let perp_mass = if k > 0.0 { 1.0 / k } else { 0.0 };

            let delta_impulse = (-impulse_scale).mul_add(joint.angular_impulse.x, -mass_scale * perp_mass * (cdot + bias));
            joint.angular_impulse.x += delta_impulse;

            w_a = mul_sub(w_a, delta_impulse, mul_mv(i_a, u));
            w_b = mul_add(w_b, delta_impulse, mul_mv(i_b, u));
        } else {
            let mut bias = vec2(0.0, 0.0);
            let mut mass_scale = 1.0;
            let mut impulse_scale = 0.0;

            if use_bias {
                let c = vec2(rel_q.v.x, rel_q.v.y);
                bias = vec2(constraint_softness.bias_rate * c.x, constraint_softness.bias_rate * c.y);
                mass_scale = constraint_softness.mass_scale;
                impulse_scale = constraint_softness.impulse_scale;
            }

            // Collinearity constraint as 2-by-2
            let perp_axis_x =
                mul_sv(0.5, rotate_vector(quat_a, add(mul_sv(rel_q.s, Vec3::AXIS_X), cross(rel_q.v, Vec3::AXIS_X))));
            let perp_axis_y =
                mul_sv(0.5, rotate_vector(quat_a, add(mul_sv(rel_q.s, Vec3::AXIS_Y), cross(rel_q.v, Vec3::AXIS_Y))));

            let inv_inertia_sum = add_mm(i_a, i_b);
            let kxx = dot(perp_axis_x, mul_mv(inv_inertia_sum, perp_axis_x));
            let kyy = dot(perp_axis_y, mul_mv(inv_inertia_sum, perp_axis_y));
            let kxy = dot(perp_axis_x, mul_mv(inv_inertia_sum, perp_axis_y));

            let k = Matrix2 { cx: vec2(kxx, kxy), cy: vec2(kxy, kyy) };

            let w_rel = sub(w_b, w_a);
            let cdot = vec2(dot(w_rel, perp_axis_x), dot(w_rel, perp_axis_y));
            let old_impulse = joint.angular_impulse;
            let cdot_plus_bias = vec2(cdot.x + bias.x, cdot.y + bias.y);
            let sol = solve2(k, cdot_plus_bias);
            let delta_impulse = vec2(
                (-impulse_scale).mul_add(old_impulse.x, -mass_scale * sol.x),
                (-impulse_scale).mul_add(old_impulse.y, -mass_scale * sol.y),
            );
            joint.angular_impulse = vec2(old_impulse.x + delta_impulse.x, old_impulse.y + delta_impulse.y);

            let angular_impulse = blend2(delta_impulse.x, perp_axis_x, delta_impulse.y, perp_axis_y);
            w_a = sub(w_a, mul_mv(i_a, angular_impulse));
            w_b = add(w_b, mul_mv(i_b, angular_impulse));
        }
    }

    // Solve point-to-line constraint
    {
        let perp_y = matrix_a.cy;
        let perp_z = matrix_a.cz;

        let mut bias = vec2(0.0, 0.0);
        let mut mass_scale = 1.0;
        let mut impulse_scale = 0.0;
        if use_bias {
            let c = vec2(dot(perp_y, d), dot(perp_z, d));
            bias = vec2(constraint_softness.bias_rate * c.x, constraint_softness.bias_rate * c.y);
            mass_scale = constraint_softness.mass_scale;
            impulse_scale = constraint_softness.impulse_scale;
        }

        let v_rel = sub(sub(add(v_b, cross(w_b, r_b)), v_a), cross(w_a, add(r_a, d)));
        let cdot = vec2(dot(perp_y, v_rel), dot(perp_z, v_rel));

        //// K = [(1/m1 + 1/m2) * eye(2) - skew(r1) * invI1 * skew(r1) - skew(r2) * invI2 * skew(r2)]
        ///// Jx = [-perpX, -cross(d + rA, perpX), perpX, cross(rB, perpX)]

        let kyy = m_a + m_b + dot(s_ay, mul_mv(i_a, s_ay)) + dot(s_by, mul_mv(i_b, s_by));
        let kyz = dot(s_ay, mul_mv(i_a, s_az)) + dot(s_by, mul_mv(i_b, s_bz));
        let kzz = m_a + m_b + dot(s_az, mul_mv(i_a, s_az)) + dot(s_bz, mul_mv(i_b, s_bz));

        let k = Matrix2 { cx: vec2(kyy, kyz), cy: vec2(kyz, kzz) };

        let old_impulse = joint.linear_impulse;
        let cdot_plus_bias = vec2(cdot.x + bias.x, cdot.y + bias.y);
        let sol = solve2(k, cdot_plus_bias);
        let delta_impulse = vec2(
            (-impulse_scale).mul_add(old_impulse.x, -mass_scale * sol.x),
            (-impulse_scale).mul_add(old_impulse.y, -mass_scale * sol.y),
        );
        joint.linear_impulse = vec2(old_impulse.x + delta_impulse.x, old_impulse.y + delta_impulse.y);

        let linear_impulse = blend2(delta_impulse.x, perp_y, delta_impulse.y, perp_z);

        v_a = mul_sub(v_a, m_a, linear_impulse);
        w_a = sub(w_a, mul_mv(i_a, blend2(delta_impulse.x, s_ay, delta_impulse.y, s_az)));
        v_b = mul_add(v_b, m_b, linear_impulse);
        w_b = add(w_b, mul_mv(i_b, blend2(delta_impulse.x, s_by, delta_impulse.y, s_bz)));
    }

    // C stores unconditionally through the state pointer for dynamic bodies;
    // the non-dynamic write in the old code was a byte-identical no-op.
    if index_a != NULL_INDEX && (flags_a & DYNAMIC_FLAG != 0) {
        states.set_velocities(index_a as usize, v_a, w_a);
    }
    if index_b != NULL_INDEX && (flags_b & DYNAMIC_FLAG != 0) {
        states.set_velocities(index_b as usize, v_b, w_b);
    }
}

// Port of box3d/src/spherical_joint.c
//
// Point-to-point constraint
// C = p2 - p1
// Cdot = v2 - v1
//      = v2 + cross(w2, r2) - v1 - cross(w1, r1)
// J = [-I r1_skew I -r2_skew ]
// K = J * invM * transpose(J)
// transpose(skew(r)) = -skew(r)
// K = diag(1/m1 + 1/m2) - r1_skew * invI1 * r1_skew - r2_skew * invI2 * r2_skew
//
// r_skew = R * skew(r_local) * RT
// invI = R * invI_local * RT
// r_skew * invI * r_skew = R * skew(r_local) * RT * R * invI_local * RT * R * r_skew * RT
//                        = R * ( skew(r_local) * invI_local * skew(r_local) ) * RT
//
// Deviations: recording hooks (B3_REC) and b3DrawSphericalJoint are not ported.

use crate::b3_assert;
use crate::body::{DYNAMIC_FLAG, IDENTITY_BODY_STATE};
use crate::core::NULL_INDEX;
use crate::id::JointId;
use crate::joint::{JointSim, JointUnion};
use crate::math_functions::*;
use crate::math_internal::{delta_quat_to_rotation, skew};
use crate::physics_world::{World, AWAKE_SET};
use crate::solver::{make_soft, StateAccess, StepContext};
use crate::types::JointType;

fn get_spherical(base: &mut JointSim) -> &mut crate::joint::SphericalJoint {
    match &mut base.joint {
        JointUnion::Spherical(j) => j,
        _ => panic!("wrong joint type"),
    }
}

fn get_spherical_ref(base: &JointSim) -> &crate::joint::SphericalJoint {
    match &base.joint {
        JointUnion::Spherical(j) => j,
        _ => panic!("wrong joint type"),
    }
}

pub fn spherical_joint_enable_cone_limit(world: &mut World, joint_id: JointId, enable_limit: bool) {

    crate::recording::rec_op(world, crate::recording::RecOp::SphericalJointEnableConeLimit, |b| {
        b.w_jointid(joint_id);
        b.w_bool(enable_limit);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    let joint = get_spherical(base);
    if enable_limit != joint.enable_cone_limit {
        joint.swing_impulse = 0.0;
    }
    joint.enable_cone_limit = enable_limit;
}

pub fn spherical_joint_is_cone_limit_enabled(world: &mut World, joint_id: JointId) -> bool {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    get_spherical(base).enable_cone_limit
}

pub fn spherical_joint_get_cone_limit(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    get_spherical(base).cone_angle
}

pub fn spherical_joint_set_cone_limit(world: &mut World, joint_id: JointId, angle_radians: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::SphericalJointSetConeLimit, |b| {
        b.w_jointid(joint_id);
        b.w_f32(angle_radians);
    });
    b3_assert!(is_valid_float(angle_radians) && 0.0 <= angle_radians && angle_radians <= 0.5 * PI);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    get_spherical(base).cone_angle = angle_radians;
}

pub fn spherical_joint_get_cone_angle(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    let body_id_a = base.body_id_a;
    let body_id_b = base.body_id_b;
    let local_frame_a_q = base.local_frame_a.q;
    let local_frame_b_q = base.local_frame_b.q;

    let transform_a = crate::body::get_body_transform(world, body_id_a);
    let transform_b = crate::body::get_body_transform(world, body_id_b);

    let quat_a = mul_quat(transform_a.q, local_frame_a_q);
    let mut quat_b = mul_quat(transform_b.q, local_frame_b_q);

    if dot_quat(quat_a, quat_b) < 0.0 {
        // this keeps the swing angle in the range [0, pi]
        quat_b = negate_quat(quat_b);
    }

    let rel_q = inv_mul_quat(quat_a, quat_b);

    get_swing_angle(rel_q)
}

pub fn spherical_joint_enable_twist_limit(world: &mut World, joint_id: JointId, enable_limit: bool) {

    crate::recording::rec_op(world, crate::recording::RecOp::SphericalJointEnableTwistLimit, |b| {
        b.w_jointid(joint_id);
        b.w_bool(enable_limit);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    let joint = get_spherical(base);
    if enable_limit != joint.enable_twist_limit {
        joint.lower_twist_impulse = 0.0;
        joint.upper_twist_impulse = 0.0;
    }
    joint.enable_twist_limit = enable_limit;
}

pub fn spherical_joint_is_twist_limit_enabled(world: &mut World, joint_id: JointId) -> bool {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    get_spherical(base).enable_twist_limit
}

pub fn spherical_joint_get_lower_twist_limit(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    get_spherical(base).lower_twist_angle
}

pub fn spherical_joint_get_upper_twist_limit(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    get_spherical(base).upper_twist_angle
}

pub fn spherical_joint_set_twist_limits(world: &mut World, joint_id: JointId, lower_limit_radians: f32, upper_limit_radians: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::SphericalJointSetTwistLimits, |b| {
        b.w_jointid(joint_id);
        b.w_f32(lower_limit_radians);
        b.w_f32(upper_limit_radians);
    });
    b3_assert!(is_valid_float(lower_limit_radians) && is_valid_float(upper_limit_radians));

    let lower_angle = min_float(lower_limit_radians, upper_limit_radians);
    let upper_angle = max_float(lower_limit_radians, upper_limit_radians);

    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    let joint = get_spherical(base);
    joint.lower_twist_angle = clamp_float(lower_angle, -0.99 * PI, 0.99 * PI);
    joint.upper_twist_angle = clamp_float(upper_angle, -0.99 * PI, 0.99 * PI);
}

pub fn spherical_joint_get_twist_angle(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    let body_id_a = base.body_id_a;
    let body_id_b = base.body_id_b;
    let local_frame_a_q = base.local_frame_a.q;
    let local_frame_b_q = base.local_frame_b.q;

    let transform_a = crate::body::get_body_transform(world, body_id_a);
    let transform_b = crate::body::get_body_transform(world, body_id_b);

    let quat_a = mul_quat(transform_a.q, local_frame_a_q);
    let mut quat_b = mul_quat(transform_b.q, local_frame_b_q);

    if dot_quat(quat_a, quat_b) < 0.0 {
        // this keeps the twist angle in the range [-pi, pi]
        quat_b = negate_quat(quat_b);
    }

    let rel_q = inv_mul_quat(quat_a, quat_b);

    get_twist_angle(rel_q)
}

pub fn spherical_joint_enable_spring(world: &mut World, joint_id: JointId, enable_spring: bool) {

    crate::recording::rec_op(world, crate::recording::RecOp::SphericalJointEnableSpring, |b| {
        b.w_jointid(joint_id);
        b.w_bool(enable_spring);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    let joint = get_spherical(base);
    if enable_spring != joint.enable_spring {
        joint.spring_impulse = Vec3::ZERO;
    }
    joint.enable_spring = enable_spring;
}

pub fn spherical_joint_is_spring_enabled(world: &mut World, joint_id: JointId) -> bool {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    get_spherical(base).enable_spring
}

pub fn spherical_joint_set_target_rotation(world: &mut World, joint_id: JointId, target_rotation: Quat) {

    crate::recording::rec_op(world, crate::recording::RecOp::SphericalJointSetTargetRotation, |b| {
        b.w_jointid(joint_id);
        b.w_quat(target_rotation);
    });
    b3_assert!(is_valid_quat(target_rotation));
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    get_spherical(base).target_rotation = target_rotation;
}

pub fn spherical_joint_get_target_rotation(world: &mut World, joint_id: JointId) -> Quat {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    get_spherical(base).target_rotation
}

pub fn spherical_joint_set_spring_hertz(world: &mut World, joint_id: JointId, hertz: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::SphericalJointSetSpringHertz, |b| {
        b.w_jointid(joint_id);
        b.w_f32(hertz);
    });
    b3_assert!(is_valid_float(hertz) && hertz >= 0.0);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    get_spherical(base).hertz = hertz;
}

pub fn spherical_joint_get_spring_hertz(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    get_spherical(base).hertz
}

pub fn spherical_joint_set_spring_damping_ratio(world: &mut World, joint_id: JointId, damping_ratio: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::SphericalJointSetSpringDampingRatio, |b| {
        b.w_jointid(joint_id);
        b.w_f32(damping_ratio);
    });
    b3_assert!(is_valid_float(damping_ratio) && damping_ratio >= 0.0);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    get_spherical(base).damping_ratio = damping_ratio;
}

pub fn spherical_joint_get_spring_damping_ratio(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    get_spherical(base).damping_ratio
}

pub fn spherical_joint_enable_motor(world: &mut World, joint_id: JointId, enable_motor: bool) {

    crate::recording::rec_op(world, crate::recording::RecOp::SphericalJointEnableMotor, |b| {
        b.w_jointid(joint_id);
        b.w_bool(enable_motor);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    let joint = get_spherical(base);
    if enable_motor != joint.enable_motor {
        joint.motor_impulse = Vec3::ZERO;
    }
    joint.enable_motor = enable_motor;
}

pub fn spherical_joint_is_motor_enabled(world: &mut World, joint_id: JointId) -> bool {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    get_spherical(base).enable_motor
}

pub fn spherical_joint_set_motor_velocity(world: &mut World, joint_id: JointId, motor_velocity: Vec3) {

    crate::recording::rec_op(world, crate::recording::RecOp::SphericalJointSetMotorVelocity, |b| {
        b.w_jointid(joint_id);
        b.w_vec3(motor_velocity);
    });
    b3_assert!(is_valid_vec3(motor_velocity));
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    get_spherical(base).motor_velocity = motor_velocity;
}

pub fn spherical_joint_get_motor_velocity(world: &mut World, joint_id: JointId) -> Vec3 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    get_spherical(base).motor_velocity
}

pub fn spherical_joint_set_max_motor_torque(world: &mut World, joint_id: JointId, max_force: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::SphericalJointSetMaxMotorTorque, |b| {
        b.w_jointid(joint_id);
        b.w_f32(max_force);
    });
    b3_assert!(is_valid_float(max_force) && max_force >= 0.0);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    get_spherical(base).max_motor_torque = max_force;
}

pub fn spherical_joint_get_max_motor_torque(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    get_spherical(base).max_motor_torque
}

pub fn spherical_joint_get_motor_torque(world: &mut World, joint_id: JointId) -> Vec3 {
    let inv_h = world.inv_h;
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Spherical);
    mul_sv(inv_h, get_spherical(base).motor_impulse)
}

pub fn get_spherical_joint_force(world: &World, base: &JointSim) -> Vec3 {
    mul_sv(world.inv_h, get_spherical_ref(base).linear_impulse)
}

pub fn get_spherical_joint_torque(world: &World, base: &JointSim) -> Vec3 {
    let xf_a = crate::body::get_body_transform(world, base.body_id_a);
    let xf_b = crate::body::get_body_transform(world, base.body_id_b);
    let q_a = mul_quat(xf_a.q, base.local_frame_a.q);
    let q_b = mul_quat(xf_b.q, base.local_frame_b.q);

    // Cone axis is the z-axis of body A.
    let cone_axis = rotate_vector(q_a, Vec3::AXIS_Z);
    let twist_axis = rotate_vector(q_b, Vec3::AXIS_Z);
    let swing_axis = normalize(cross(cone_axis, twist_axis));

    let joint = get_spherical_ref(base);
    let mut impulse = add(joint.spring_impulse, joint.motor_impulse);
    impulse = mul_add(impulse, joint.lower_twist_impulse - joint.upper_twist_impulse, twist_axis);
    impulse = mul_add(impulse, joint.swing_impulse, swing_axis);
    mul_sv(world.inv_h, impulse)
}

pub fn prepare_spherical_joint(base: &mut JointSim, world: &World, context: &StepContext) {
    b3_assert!(base.joint_type == JointType::Spherical);

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
    let fixed_rotation = base.fixed_rotation;

    let joint = get_spherical(base);
    joint.index_a = if set_index_a == AWAKE_SET { local_index_a } else { NULL_INDEX };
    joint.index_b = if set_index_b == AWAKE_SET { local_index_b } else { NULL_INDEX };

    // Compute joint anchor frames with world space rotation, relative to center of mass
    joint.frame_a.q = mul_quat(body_sim_a.transform.q, local_frame_a.q);
    joint.frame_a.p = rotate_vector(body_sim_a.transform.q, sub(local_frame_a.p, body_sim_a.local_center));
    joint.frame_b.q = mul_quat(body_sim_b.transform.q, local_frame_b.q);
    joint.frame_b.p = rotate_vector(body_sim_b.transform.q, sub(local_frame_b.p, body_sim_b.local_center));

    joint.delta_center = sub_pos(body_sim_b.center, body_sim_a.center);

    // Cone axis is the z-axis of body A.
    let cone_axis = rotate_vector(joint.frame_a.q, Vec3::AXIS_Z);

    // Twist axis is the z-axis of body B.
    let twist_axis = rotate_vector(joint.frame_b.q, Vec3::AXIS_Z);

    if joint.enable_cone_limit {
        // Swing axis may be zero
        let swing_axis = normalize(cross(cone_axis, twist_axis));
        let k = dot(swing_axis, mul_mv(inv_inertia_sum, swing_axis));
        joint.swing_mass = if k > 0.0 { 1.0 / k } else { 0.0 };
        joint.swing_axis = swing_axis;
    }

    if joint.enable_twist_limit {
        let rel_q = inv_mul_quat(joint.frame_a.q, joint.frame_b.q);
        let tan_theta_over_2 =
            (rel_q.v.y.mul_add(rel_q.v.y, rel_q.v.x * rel_q.v.x) / rel_q.s.mul_add(rel_q.s, rel_q.v.z * rel_q.v.z)).sqrt();

        // todo verify this Jacobian using a finite difference, unit test?
        let swing_axis = normalize(cross(cone_axis, twist_axis));
        let perp_axis = cross(swing_axis, cone_axis);
        let twist_jacobian = mul_add(cone_axis, tan_theta_over_2, perp_axis);
        let k = dot(twist_jacobian, mul_mv(inv_inertia_sum, twist_jacobian));
        joint.twist_mass = if k > 0.0 { 1.0 / k } else { 0.0 };
        joint.twist_jacobian = twist_jacobian;
    }

    if !fixed_rotation {
        joint.rotation_mass = invert_matrix(inv_inertia_sum);
    } else {
        joint.rotation_mass = Matrix3::ZERO;
    }

    joint.spring_softness = make_soft(joint.hertz, joint.damping_ratio, context.h);

    if !context.enable_warm_starting {
        joint.linear_impulse = Vec3::ZERO;
        joint.motor_impulse = Vec3::ZERO;
        joint.spring_impulse = Vec3::ZERO;
        joint.swing_impulse = 0.0;
        joint.lower_twist_impulse = 0.0;
        joint.upper_twist_impulse = 0.0;
    }
}

pub fn warm_start_spherical_joint(base: &mut JointSim, states: &StateAccess, _context: &StepContext) {
    b3_assert!(base.joint_type == JointType::Spherical);

    let m_a = base.inv_mass_a;
    let m_b = base.inv_mass_b;
    let i_a = base.inv_i_a;
    let i_b = base.inv_i_b;

    let joint = get_spherical(base);

    // dummy state for static bodies
    // Field copies through short-lived borrows + a velocities-only write-back
    // (C reads in place and stores only the two velocity vectors; the full
    // 56-byte round trip kept the untouched fields live across the whole
    // function — see StateAccess::set_velocities).
    let index_a = joint.index_a;
    let index_b = joint.index_b;
    let (mut v_a, mut w_a, dq_a, flags_a) = {
        let s = if index_a == NULL_INDEX { &IDENTITY_BODY_STATE } else { states.get_ref(index_a as usize) };
        (s.linear_velocity, s.angular_velocity, s.delta_rotation, s.flags)
    };
    let (mut v_b, mut w_b, dq_b, flags_b) = {
        let s = if index_b == NULL_INDEX { &IDENTITY_BODY_STATE } else { states.get_ref(index_b as usize) };
        (s.linear_velocity, s.angular_velocity, s.delta_rotation, s.flags)
    };

    let r_a = rotate_vector(dq_a, joint.frame_a.p);
    let r_b = rotate_vector(dq_b, joint.frame_b.p);

    let mut angular_impulse = add(joint.spring_impulse, joint.motor_impulse);
    angular_impulse = mul_sub(angular_impulse, joint.swing_impulse, joint.swing_axis);
    angular_impulse = mul_add(angular_impulse, joint.lower_twist_impulse - joint.upper_twist_impulse, joint.twist_jacobian);

    v_a = mul_sub(v_a, m_a, joint.linear_impulse);
    w_a = sub(w_a, mul_mv(i_a, add(cross(r_a, joint.linear_impulse), angular_impulse)));

    v_b = mul_add(v_b, m_b, joint.linear_impulse);
    w_b = add(w_b, mul_mv(i_b, add(cross(r_b, joint.linear_impulse), angular_impulse)));

    // C stores unconditionally through the state pointer for dynamic bodies;
    // the non-dynamic write in the old code was a byte-identical no-op.
    if index_a != NULL_INDEX && (flags_a & DYNAMIC_FLAG != 0) {
        states.set_velocities(index_a as usize, v_a, w_a);
    }
    if index_b != NULL_INDEX && (flags_b & DYNAMIC_FLAG != 0) {
        states.set_velocities(index_b as usize, v_b, w_b);
    }
}

pub fn solve_spherical_joint(base: &mut JointSim, states: &StateAccess, context: &StepContext, use_bias: bool) {
    let m_a = base.inv_mass_a;
    let m_b = base.inv_mass_b;
    let i_a = base.inv_i_a;
    let i_b = base.inv_i_b;
    let inv_i_a = base.inv_i_a;
    let inv_i_b = base.inv_i_b;
    let fixed_rotation = base.fixed_rotation;
    let constraint_softness = base.constraint_softness;

    let joint = get_spherical(base);

    // dummy state for static bodies
    // Field copies through short-lived borrows + a velocities-only write-back
    // (see warm_start_spherical_joint / StateAccess::set_velocities).
    let index_a = joint.index_a;
    let index_b = joint.index_b;
    let (mut v_a, mut w_a, dq_a, dp_a, flags_a) = {
        let s = if index_a == NULL_INDEX { &IDENTITY_BODY_STATE } else { states.get_ref(index_a as usize) };
        (s.linear_velocity, s.angular_velocity, s.delta_rotation, s.delta_position, s.flags)
    };
    let (mut v_b, mut w_b, dq_b, dp_b, flags_b) = {
        let s = if index_b == NULL_INDEX { &IDENTITY_BODY_STATE } else { states.get_ref(index_b as usize) };
        (s.linear_velocity, s.angular_velocity, s.delta_rotation, s.delta_position, s.flags)
    };

    let quat_a = mul_quat(dq_a, joint.frame_a.q);
    let quat_b = mul_quat(dq_b, joint.frame_b.q);

    let rel_q = inv_mul_quat(quat_a, quat_b);

    // Solve spring
    if joint.enable_spring && !fixed_rotation {
        // Rotation constraint error
        let delta_rotation = delta_quat_to_rotation(rel_q, joint.target_rotation);
        let c = neg(rotate_vector(quat_a, delta_rotation));

        let bias = mul_sv(joint.spring_softness.bias_rate, c);
        let mass_scale = joint.spring_softness.mass_scale;
        let impulse_scale = joint.spring_softness.impulse_scale;
        let cdot = sub(w_b, w_a);

        let impulse = mul_sub(
            mul_sv(-mass_scale, mul_mv(joint.rotation_mass, add(cdot, bias))),
            impulse_scale,
            joint.spring_impulse,
        );
        joint.spring_impulse = add(joint.spring_impulse, impulse);

        w_a = sub(w_a, mul_mv(i_a, impulse));
        w_b = add(w_b, mul_mv(i_b, impulse));
    }

    if joint.enable_motor && !fixed_rotation {
        let cdot = sub(w_b, w_a);

        let mut lambda = neg(mul_mv(joint.rotation_mass, sub(cdot, joint.motor_velocity)));
        let mut new_impulse = add(joint.motor_impulse, lambda);
        let length = length(new_impulse);
        let max_impulse = joint.max_motor_torque * context.h;
        if length > max_impulse {
            new_impulse = mul_sv(max_impulse / length, new_impulse);
        }

        lambda = sub(new_impulse, joint.motor_impulse);
        joint.motor_impulse = new_impulse;

        w_a = sub(w_a, mul_mv(i_a, lambda));
        w_b = add(w_b, mul_mv(i_b, lambda));
    }

    if joint.enable_twist_limit && !fixed_rotation {
        let twist_angle = get_twist_angle(rel_q);

        // todo does an updated twist axis help?

        let twist_jacobian = joint.twist_jacobian;

        // Lower limit
        {
            let c = twist_angle - joint.lower_twist_angle;
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

            let cdot = dot(sub(w_b, w_a), twist_jacobian);
            let old_impulse = joint.lower_twist_impulse;
            let mut delta_impulse = (-impulse_scale).mul_add(old_impulse, -mass_scale * joint.twist_mass * (cdot + bias));
            joint.lower_twist_impulse = max_float(old_impulse + delta_impulse, 0.0);
            delta_impulse = joint.lower_twist_impulse - old_impulse;

            w_a = mul_sub(w_a, delta_impulse, mul_mv(i_a, twist_jacobian));
            w_b = mul_add(w_b, delta_impulse, mul_mv(i_b, twist_jacobian));
        }

        // Upper limit
        {
            let c = joint.upper_twist_angle - twist_angle;
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

            // sign flipped on Cdot
            let cdot = dot(sub(w_a, w_b), twist_jacobian);
            let old_impulse = joint.upper_twist_impulse;
            let mut delta_impulse = (-impulse_scale).mul_add(old_impulse, -mass_scale * joint.twist_mass * (cdot + bias));
            joint.upper_twist_impulse = max_float(old_impulse + delta_impulse, 0.0);
            delta_impulse = joint.upper_twist_impulse - old_impulse;

            // sign flipped on applied impulse
            w_a = mul_add(w_a, delta_impulse, mul_mv(i_a, twist_jacobian));
            w_b = mul_sub(w_b, delta_impulse, mul_mv(i_b, twist_jacobian));
        }
    }

    if joint.enable_cone_limit && !fixed_rotation {
        let swing_angle = get_swing_angle(rel_q);

        // todo does an updated swing axis help?

        let swing_axis = joint.swing_axis;

        let c = joint.cone_angle - swing_angle;
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

        // sign flipped on Cdot
        let cdot = dot(sub(w_a, w_b), swing_axis);
        let old_impulse = joint.swing_impulse;
        let mut delta_impulse = (-impulse_scale).mul_add(old_impulse, -mass_scale * joint.swing_mass * (cdot + bias));
        joint.swing_impulse = max_float(old_impulse + delta_impulse, 0.0);
        delta_impulse = joint.swing_impulse - old_impulse;

        // sign flipped on applied impulse
        w_a = mul_add(w_a, delta_impulse, mul_mv(i_a, swing_axis));
        w_b = mul_sub(w_b, delta_impulse, mul_mv(i_b, swing_axis));
    }

    // Solve point-to-point constraint
    {
        let r_a = rotate_vector(dq_a, joint.frame_a.p);
        let r_b = rotate_vector(dq_b, joint.frame_b.p);

        let cdot = sub(sub(add(v_b, cross(w_b, r_b)), v_a), cross(w_a, r_a));

        let mut bias = Vec3::ZERO;
        let mut mass_scale = 1.0;
        let mut impulse_scale = 0.0;
        if use_bias {
            let dc_a = dp_a;
            let dc_b = dp_b;

            let mut separation = add(sub(dc_b, dc_a), sub(r_b, r_a));
            separation = add(separation, joint.delta_center);

            bias = mul_sv(constraint_softness.bias_rate, separation);
            mass_scale = constraint_softness.mass_scale;
            impulse_scale = constraint_softness.impulse_scale;
        }

        //// K = [(1/m1 + 1/m2) * eye(2) - skew(r1) * invI1 * skew(r1) - skew(r2) * invI2 * skew(r2)]
        let s_a = skew(r_a);
        let s_b = skew(r_b);
        let k_a = mul_mm(s_a, mul_mm(inv_i_a, s_a));
        let k_b = mul_mm(s_b, mul_mm(inv_i_b, s_b));
        let mut k = negate_mat3(add_mm(k_a, k_b));
        k.cx.x += m_a + m_b;
        k.cy.y += m_a + m_b;
        k.cz.z += m_a + m_b;

        let b = solve3(k, add(cdot, bias));

        let impulse = mul_sub(mul_sv(-mass_scale, b), impulse_scale, joint.linear_impulse);
        joint.linear_impulse = add(joint.linear_impulse, impulse);

        v_a = mul_sub(v_a, m_a, impulse);
        w_a = sub(w_a, mul_mv(i_a, cross(r_a, impulse)));
        v_b = mul_add(v_b, m_b, impulse);
        w_b = add(w_b, mul_mv(i_b, cross(r_b, impulse)));
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

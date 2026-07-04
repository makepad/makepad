// Port of box3d/src/revolute_joint.c
//
// Point-to-point linear constraint
// C = pB - pA
// Cdot = vB - vA
//      = vB + cross(wB, rB) - vA - cross(wA, rA)
// Cdot = J * v
// J = [-E -skew(rA) E skew(rB) ]
//
// K = J * invM * JT
//   = [(1/mA + 1/mB) * E - skew(rA) * invIA * skew(rA) - skew(rB) * invIB * skew(rB)]
//
// Perpendicularity constraint
// frameA = qA * localFrameA
// frameB = qB * localFrameB
// qRel = conj(frameA) * frameB
// C = [qRel.x; qRel.y]
// qRelDot = 0.5 * conj(frameA) * (wB - wA) * frameB
// Cdot = [qRelDot.x, qRelDot.y]
// Pulling out wB and wA
// sr = qRel.s
// vr = qRel.v
// Jx = 0.5 * rotate(frameA, sr * ex + cross(vr, ex))
// Jy = 0.5 * rotate(frameA, sr * ey + cross(vr, ey))
//
// Motor constraint
// Cdot = wB - wA
// J = [0 0 -E 0 0 E]
// K = invIA + invIB
//
// Deviations: recording hooks (B3_REC) and b3DrawRevoluteJoint are not ported.
// b3GetRevoluteJointTorque's C side effect of storing perpAxisX/Y on the joint
// is dropped (prepare recomputes them before any warm start); the returned
// value is identical.

use crate::b3_assert;
use crate::body::{BodyState, DYNAMIC_FLAG, IDENTITY_BODY_STATE};
use crate::core::NULL_INDEX;
use crate::id::JointId;
use crate::joint::{JointSim, JointUnion};
use crate::math_functions::*;
use crate::math_internal::{add2, mul_sv2, skew, solve2, sub2, Matrix2};
use crate::physics_world::{World, AWAKE_SET};
use crate::solver::{make_soft, StateAccess, StepContext};
use crate::types::JointType;

fn get_revolute(base: &mut JointSim) -> &mut crate::joint::RevoluteJoint {
    match &mut base.joint {
        JointUnion::Revolute(j) => j,
        _ => panic!("wrong joint type"),
    }
}

fn get_revolute_ref(base: &JointSim) -> &crate::joint::RevoluteJoint {
    match &base.joint {
        JointUnion::Revolute(j) => j,
        _ => panic!("wrong joint type"),
    }
}

pub fn revolute_joint_enable_limit(world: &mut World, joint_id: JointId, enable_limit: bool) {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    let joint = get_revolute(base);
    if enable_limit != joint.enable_limit {
        joint.lower_impulse = 0.0;
        joint.upper_impulse = 0.0;
    }
    joint.enable_limit = enable_limit;
}

pub fn revolute_joint_is_limit_enabled(world: &mut World, joint_id: JointId) -> bool {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    get_revolute(base).enable_limit
}

pub fn revolute_joint_get_lower_limit(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    get_revolute(base).lower_angle
}

pub fn revolute_joint_get_upper_limit(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    get_revolute(base).upper_angle
}

pub fn revolute_joint_set_limits(world: &mut World, joint_id: JointId, lower_limit_radians: f32, upper_limit_radians: f32) {
    b3_assert!(is_valid_float(lower_limit_radians) && is_valid_float(upper_limit_radians));

    let lower_angle = min_float(lower_limit_radians, upper_limit_radians);
    let upper_angle = max_float(lower_limit_radians, upper_limit_radians);

    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    let joint = get_revolute(base);
    joint.lower_angle = clamp_float(lower_angle, -0.99 * PI, 0.99 * PI);
    joint.upper_angle = clamp_float(upper_angle, -0.99 * PI, 0.99 * PI);
}

pub fn revolute_joint_get_angle(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
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

pub fn revolute_joint_enable_spring(world: &mut World, joint_id: JointId, enable_spring: bool) {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    let joint = get_revolute(base);
    if enable_spring != joint.enable_spring {
        joint.spring_impulse = 0.0;
    }
    joint.enable_spring = enable_spring;
}

pub fn revolute_joint_is_spring_enabled(world: &mut World, joint_id: JointId) -> bool {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    get_revolute(base).enable_spring
}

pub fn revolute_joint_set_target_angle(world: &mut World, joint_id: JointId, target_radians: f32) {
    b3_assert!(is_valid_float(target_radians) && -PI <= target_radians && target_radians <= PI);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    get_revolute(base).target_angle = target_radians;
}

pub fn revolute_joint_get_target_angle(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    get_revolute(base).target_angle
}

pub fn revolute_joint_set_spring_hertz(world: &mut World, joint_id: JointId, hertz: f32) {
    b3_assert!(is_valid_float(hertz) && hertz >= 0.0);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    get_revolute(base).hertz = hertz;
}

pub fn revolute_joint_get_spring_hertz(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    get_revolute(base).hertz
}

pub fn revolute_joint_set_spring_damping_ratio(world: &mut World, joint_id: JointId, damping_ratio: f32) {
    b3_assert!(is_valid_float(damping_ratio) && damping_ratio >= 0.0);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    get_revolute(base).damping_ratio = damping_ratio;
}

pub fn revolute_joint_get_spring_damping_ratio(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    get_revolute(base).damping_ratio
}

pub fn revolute_joint_enable_motor(world: &mut World, joint_id: JointId, enable_motor: bool) {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    let joint = get_revolute(base);
    if enable_motor != joint.enable_motor {
        joint.motor_impulse = 0.0;
    }
    joint.enable_motor = enable_motor;
}

pub fn revolute_joint_is_motor_enabled(world: &mut World, joint_id: JointId) -> bool {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    get_revolute(base).enable_motor
}

pub fn revolute_joint_set_motor_speed(world: &mut World, joint_id: JointId, motor_speed: f32) {
    b3_assert!(is_valid_float(motor_speed));
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    get_revolute(base).motor_speed = motor_speed;
}

pub fn revolute_joint_get_motor_speed(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    get_revolute(base).motor_speed
}

pub fn revolute_joint_set_max_motor_torque(world: &mut World, joint_id: JointId, max_force: f32) {
    b3_assert!(is_valid_float(max_force) && max_force >= 0.0);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    get_revolute(base).max_motor_torque = max_force;
}

pub fn revolute_joint_get_max_motor_torque(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    get_revolute(base).max_motor_torque
}

pub fn revolute_joint_get_motor_torque(world: &mut World, joint_id: JointId) -> f32 {
    let inv_h = world.inv_h;
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Revolute);
    inv_h * get_revolute(base).motor_impulse
}

pub fn get_revolute_joint_force(world: &World, base: &JointSim) -> Vec3 {
    mul_sv(world.inv_h, get_revolute_ref(base).linear_impulse)
}

pub fn get_revolute_joint_torque(world: &World, base: &JointSim) -> Vec3 {
    let transform_a = crate::body::get_body_transform(world, base.body_id_a);
    let joint = get_revolute_ref(base);
    let mut axis = rotate_vector(base.local_frame_a.q, Vec3::AXIS_Z);
    axis = rotate_vector(transform_a.q, axis);

    let rel_q = inv_mul_quat(joint.frame_a.q, joint.frame_b.q);

    // These are needed for warm starting (C stores them back on the joint; the
    // Rust port computes locals — prepare recomputes them before warm start).
    let perp_axis_x = mul_sv(
        0.5,
        rotate_vector(joint.frame_a.q, add(mul_sv(rel_q.s, Vec3::AXIS_X), cross(rel_q.v, Vec3::AXIS_X))),
    );
    let perp_axis_y = mul_sv(
        0.5,
        rotate_vector(joint.frame_a.q, add(mul_sv(rel_q.s, Vec3::AXIS_Y), cross(rel_q.v, Vec3::AXIS_Y))),
    );

    let axial_impulse = joint.spring_impulse + joint.motor_impulse + joint.lower_impulse - joint.upper_impulse;
    let mut angular_impulse = add(mul_sv(joint.perp_impulse.x, perp_axis_x), mul_sv(joint.perp_impulse.y, perp_axis_y));
    angular_impulse = mul_add(angular_impulse, axial_impulse, joint.rotation_axis_z);

    // todo add pivot torque
    let impulse = mul_add(
        angular_impulse,
        joint.spring_impulse + joint.motor_impulse + joint.lower_impulse - joint.upper_impulse,
        axis,
    );
    mul_sv(world.inv_h, impulse)
}

pub fn prepare_revolute_joint(base: &mut JointSim, world: &World, context: &StepContext) {
    b3_assert!(base.joint_type == JointType::Revolute);

    let body_a = &world.bodies[base.body_id_a as usize];
    let body_b = &world.bodies[base.body_id_b as usize];

    b3_assert!(body_a.set_index == AWAKE_SET || body_b.set_index == AWAKE_SET);

    let local_index_a = body_a.local_index;
    let local_index_b = body_b.local_index;

    let body_sim_a = *crate::joint::get_solve_body_sim(world, context, body_a.set_index, local_index_a);
    let body_sim_b = *crate::joint::get_solve_body_sim(world, context, body_b.set_index, local_index_b);

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

    let joint = get_revolute(base);
    joint.index_a = if set_index_a == AWAKE_SET { local_index_a } else { NULL_INDEX };
    joint.index_b = if set_index_b == AWAKE_SET { local_index_b } else { NULL_INDEX };

    // Compute joint anchor frames with world space rotation, relative to center of mass
    // Avoid round-off here as much as possible.
    // b3Vec3 pf = (xf.p - c) + rot(xf.q, f.p)
    // pf = xf.p - (xf.p + rot(xf.q, lc)) + rot(xf.q, f.p)
    // pf = rot(xf.q, f.p - lc)
    joint.frame_a.q = mul_quat(body_sim_a.transform.q, local_frame_a.q);
    joint.frame_a.p = rotate_vector(body_sim_a.transform.q, sub(local_frame_a.p, body_sim_a.local_center));
    joint.frame_b.q = mul_quat(body_sim_b.transform.q, local_frame_b.q);
    joint.frame_b.p = rotate_vector(body_sim_b.transform.q, sub(local_frame_b.p, body_sim_b.local_center));

    joint.delta_center = sub_pos(body_sim_b.center, body_sim_a.center);

    {
        // Rotation axis is the z-axis of body A.
        let rotation_axis_z = rotate_vector(joint.frame_a.q, Vec3::AXIS_Z);
        let k = dot(rotation_axis_z, mul_mv(inv_inertia_sum, rotation_axis_z));
        joint.axial_mass = if k > 0.0 { 1.0 / k } else { 0.0 };
        joint.rotation_axis_z = rotation_axis_z;
    }

    let rel_q = inv_mul_quat(joint.frame_a.q, joint.frame_b.q);

    {
        // These are needed for warm starting
        joint.perp_axis_x = mul_sv(
            0.5,
            rotate_vector(joint.frame_a.q, add(mul_sv(rel_q.s, Vec3::AXIS_X), cross(rel_q.v, Vec3::AXIS_X))),
        );
        joint.perp_axis_y = mul_sv(
            0.5,
            rotate_vector(joint.frame_a.q, add(mul_sv(rel_q.s, Vec3::AXIS_Y), cross(rel_q.v, Vec3::AXIS_Y))),
        );
    }

    joint.spring_softness = make_soft(joint.hertz, joint.damping_ratio, context.h);

    if !context.enable_warm_starting {
        joint.linear_impulse = Vec3::ZERO;
        joint.perp_impulse = vec2(0.0, 0.0);
        joint.motor_impulse = 0.0;
        joint.spring_impulse = 0.0;
        joint.lower_impulse = 0.0;
        joint.upper_impulse = 0.0;
    }
}

pub fn warm_start_revolute_joint(base: &mut JointSim, states: &StateAccess, _context: &StepContext) {
    b3_assert!(base.joint_type == JointType::Revolute);

    let m_a = base.inv_mass_a;
    let m_b = base.inv_mass_b;
    let i_a = base.inv_i_a;
    let i_b = base.inv_i_b;

    let joint = get_revolute(base);

    // dummy state for static bodies
    let index_a = joint.index_a;
    let index_b = joint.index_b;
    let mut state_a: BodyState =
        if index_a == NULL_INDEX { IDENTITY_BODY_STATE } else { states.get(index_a as usize) };
    let mut state_b: BodyState =
        if index_b == NULL_INDEX { IDENTITY_BODY_STATE } else { states.get(index_b as usize) };

    let mut v_a = state_a.linear_velocity;
    let mut w_a = state_a.angular_velocity;
    let mut v_b = state_b.linear_velocity;
    let mut w_b = state_b.angular_velocity;

    let r_a = rotate_vector(state_a.delta_rotation, joint.frame_a.p);
    let r_b = rotate_vector(state_b.delta_rotation, joint.frame_b.p);

    let axial_impulse = joint.spring_impulse + joint.motor_impulse + joint.lower_impulse - joint.upper_impulse;
    let mut angular_impulse =
        add(mul_sv(joint.perp_impulse.x, joint.perp_axis_x), mul_sv(joint.perp_impulse.y, joint.perp_axis_y));
    angular_impulse = mul_add(angular_impulse, axial_impulse, joint.rotation_axis_z);

    v_a = mul_sub(v_a, m_a, joint.linear_impulse);
    w_a = sub(w_a, mul_mv(i_a, add(cross(r_a, joint.linear_impulse), angular_impulse)));

    v_b = mul_add(v_b, m_b, joint.linear_impulse);
    w_b = add(w_b, mul_mv(i_b, add(cross(r_b, joint.linear_impulse), angular_impulse)));

    if state_a.flags & DYNAMIC_FLAG != 0 {
        state_a.linear_velocity = v_a;
        state_a.angular_velocity = w_a;
    }

    if state_b.flags & DYNAMIC_FLAG != 0 {
        state_b.linear_velocity = v_b;
        state_b.angular_velocity = w_b;
    }

    if index_a != NULL_INDEX {
        states.set(index_a as usize, state_a);
    }
    if index_b != NULL_INDEX {
        states.set(index_b as usize, state_b);
    }
}

pub fn solve_revolute_joint(base: &mut JointSim, states: &StateAccess, context: &StepContext, use_bias: bool) {
    let m_a = base.inv_mass_a;
    let m_b = base.inv_mass_b;
    let i_a = base.inv_i_a;
    let i_b = base.inv_i_b;
    let inv_i_a = base.inv_i_a;
    let inv_i_b = base.inv_i_b;
    let fixed_rotation = base.fixed_rotation;
    let constraint_softness = base.constraint_softness;

    let joint = get_revolute(base);

    // dummy state for static bodies
    let index_a = joint.index_a;
    let index_b = joint.index_b;
    let mut state_a: BodyState =
        if index_a == NULL_INDEX { IDENTITY_BODY_STATE } else { states.get(index_a as usize) };
    let mut state_b: BodyState =
        if index_b == NULL_INDEX { IDENTITY_BODY_STATE } else { states.get(index_b as usize) };

    let mut v_a = state_a.linear_velocity;
    let mut w_a = state_a.angular_velocity;
    let mut v_b = state_b.linear_velocity;
    let mut w_b = state_b.angular_velocity;

    let quat_a = mul_quat(state_a.delta_rotation, joint.frame_a.q);
    let mut quat_b = mul_quat(state_b.delta_rotation, joint.frame_b.q);

    if dot_quat(quat_a, quat_b) < 0.0 {
        // this keeps the rotation angle in the range [-pi, pi]
        quat_b = negate_quat(quat_b);
    }

    let rel_q = inv_mul_quat(quat_a, quat_b);

    // Solve spring
    if joint.enable_spring && !fixed_rotation {
        // Get the substep relative rotation
        let target_angle = joint.target_angle;
        let angle = get_twist_angle(rel_q);
        let c = angle - target_angle;

        let bias = joint.spring_softness.bias_rate * c;
        let mass_scale = joint.spring_softness.mass_scale;
        let impulse_scale = joint.spring_softness.impulse_scale;
        let cdot = dot(sub(w_b, w_a), joint.rotation_axis_z);

        let delta_impulse = -mass_scale * joint.axial_mass * (cdot + bias) - impulse_scale * joint.spring_impulse;
        joint.spring_impulse += delta_impulse;

        w_a = mul_sub(w_a, delta_impulse, mul_mv(i_a, joint.rotation_axis_z));
        w_b = mul_add(w_b, delta_impulse, mul_mv(i_b, joint.rotation_axis_z));
    }

    if joint.enable_motor && !fixed_rotation {
        let cdot = dot(sub(w_b, w_a), joint.rotation_axis_z) - joint.motor_speed;

        let mut delta_impulse = -joint.axial_mass * cdot;
        let mut new_impulse = joint.motor_impulse + delta_impulse;
        let max_impulse = joint.max_motor_torque * context.h;
        new_impulse = clamp_float(new_impulse, -max_impulse, max_impulse);
        delta_impulse = new_impulse - joint.motor_impulse;
        joint.motor_impulse = new_impulse;

        w_a = mul_sub(w_a, delta_impulse, mul_mv(i_a, joint.rotation_axis_z));
        w_b = mul_add(w_b, delta_impulse, mul_mv(i_b, joint.rotation_axis_z));
    }

    if joint.enable_limit && !fixed_rotation {
        let angle = get_twist_angle(rel_q);

        // todo does an updated twist axis help?

        let axis = joint.rotation_axis_z;

        // Lower limit
        {
            let c = angle - joint.lower_angle;
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

            let cdot = dot(sub(w_b, w_a), axis);
            let old_impulse = joint.lower_impulse;
            let mut delta_impulse = -mass_scale * joint.axial_mass * (cdot + bias) - impulse_scale * old_impulse;
            joint.lower_impulse = max_float(old_impulse + delta_impulse, 0.0);
            delta_impulse = joint.lower_impulse - old_impulse;

            w_a = mul_sub(w_a, delta_impulse, mul_mv(i_a, axis));
            w_b = mul_add(w_b, delta_impulse, mul_mv(i_b, axis));
        }

        // Upper limit
        {
            let c = joint.upper_angle - angle;
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
            let cdot = dot(sub(w_a, w_b), axis);
            let old_impulse = joint.upper_impulse;
            let mut delta_impulse = -mass_scale * joint.axial_mass * (cdot + bias) - impulse_scale * old_impulse;
            joint.upper_impulse = max_float(old_impulse + delta_impulse, 0.0);
            delta_impulse = joint.upper_impulse - old_impulse;

            // sign flipped on applied impulse
            w_a = mul_add(w_a, delta_impulse, mul_mv(i_a, axis));
            w_b = mul_sub(w_b, delta_impulse, mul_mv(i_b, axis));
        }
    }

    // Collinearity constraint
    if !fixed_rotation {
        let mut bias = vec2(0.0, 0.0);
        let mut mass_scale = 1.0;
        let mut impulse_scale = 0.0;

        if use_bias {
            let c = vec2(rel_q.v.x, rel_q.v.y);
            bias = mul_sv2(constraint_softness.bias_rate, c);
            mass_scale = constraint_softness.mass_scale;
            impulse_scale = constraint_softness.impulse_scale;
        }

        // Collinearity constraint as 2-by-2
        let perp_axis_x =
            mul_sv(0.5, rotate_vector(quat_a, add(mul_sv(rel_q.s, Vec3::AXIS_X), cross(rel_q.v, Vec3::AXIS_X))));
        let perp_axis_y =
            mul_sv(0.5, rotate_vector(quat_a, add(mul_sv(rel_q.s, Vec3::AXIS_Y), cross(rel_q.v, Vec3::AXIS_Y))));
        joint.perp_axis_x = perp_axis_x;
        joint.perp_axis_y = perp_axis_y;

        let inv_inertia_sum = add_mm(i_a, i_b);
        let kxx = dot(perp_axis_x, mul_mv(inv_inertia_sum, perp_axis_x));
        let kyy = dot(perp_axis_y, mul_mv(inv_inertia_sum, perp_axis_y));
        let kxy = dot(perp_axis_x, mul_mv(inv_inertia_sum, perp_axis_y));

        let k = Matrix2 { cx: vec2(kxx, kxy), cy: vec2(kxy, kyy) };

        let w_rel = sub(w_b, w_a);
        let cdot = vec2(dot(w_rel, perp_axis_x), dot(w_rel, perp_axis_y));
        let old_impulse = joint.perp_impulse;
        let sol = solve2(k, add2(cdot, bias));
        let delta_impulse = sub2(mul_sv2(-mass_scale, sol), mul_sv2(impulse_scale, old_impulse));
        joint.perp_impulse = add2(joint.perp_impulse, delta_impulse);

        let angular_impulse = add(mul_sv(delta_impulse.x, perp_axis_x), mul_sv(delta_impulse.y, perp_axis_y));
        w_a = sub(w_a, mul_mv(i_a, angular_impulse));
        w_b = add(w_b, mul_mv(i_b, angular_impulse));
    }

    // Solve point-to-point constraint
    {
        let r_a = rotate_vector(state_a.delta_rotation, joint.frame_a.p);
        let r_b = rotate_vector(state_b.delta_rotation, joint.frame_b.p);

        let cdot = sub(sub(add(v_b, cross(w_b, r_b)), v_a), cross(w_a, r_a));

        let mut bias = Vec3::ZERO;
        let mut mass_scale = 1.0;
        let mut impulse_scale = 0.0;
        if use_bias {
            let dc_a = state_a.delta_position;
            let dc_b = state_b.delta_position;

            let separation = add(add(sub(dc_b, dc_a), sub(r_b, r_a)), joint.delta_center);

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

        let impulse = sub(mul_sv(-mass_scale, b), mul_sv(impulse_scale, joint.linear_impulse));
        joint.linear_impulse = add(joint.linear_impulse, impulse);

        v_a = mul_sub(v_a, m_a, impulse);
        w_a = sub(w_a, mul_mv(i_a, cross(r_a, impulse)));
        v_b = mul_add(v_b, m_b, impulse);
        w_b = add(w_b, mul_mv(i_b, cross(r_b, impulse)));
    }

    if state_a.flags & DYNAMIC_FLAG != 0 {
        state_a.linear_velocity = v_a;
        state_a.angular_velocity = w_a;
    }

    if state_b.flags & DYNAMIC_FLAG != 0 {
        state_b.linear_velocity = v_b;
        state_b.angular_velocity = w_b;
    }

    if index_a != NULL_INDEX {
        states.set(index_a as usize, state_a);
    }
    if index_b != NULL_INDEX {
        states.set(index_b as usize, state_b);
    }
}

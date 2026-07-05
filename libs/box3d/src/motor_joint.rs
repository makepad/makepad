// Port of box3d/src/motor_joint.c
//
// Notes:
// - b3DefaultMotorJointDef / b3CreateMotorJoint live in joint.c (ported in joint.rs).
// - Recording hooks (B3_REC) are not ported. No draw function exists in C.
// - Body states are copied to locals and written back on exit. The C warm start
//   writes states UNCONDITIONALLY (no dynamicFlag guard) — preserved.

use crate::b3_assert;
use crate::body::{DYNAMIC_FLAG, IDENTITY_BODY_STATE};
use crate::core::NULL_INDEX;
use crate::id::JointId;
use crate::joint::{JointSim, JointUnion, MotorJoint};
use crate::math_functions::{
    add, add_mm, cross, det, dot_quat, invert_matrix, length_squared, max_float, mul_add, mul_mv,
    mul_quat, mul_sub, mul_sv, negate_mat3, negate_quat, neg, normalize, rotate_vector, solve3, sub,
    sub_pos, inv_mul_quat, mul_mm, Quat, Vec3,
};
use crate::math_internal::{delta_quat_to_rotation, skew};
use crate::physics_world::{World, AWAKE_SET};
use crate::solver::{make_soft, StateAccess, StepContext};
use crate::types::JointType;

#[inline]
fn motor_joint_mut(base: &mut JointSim) -> &mut MotorJoint {
    match &mut base.joint {
        JointUnion::Motor(j) => j,
        _ => panic!("wrong joint type"),
    }
}

#[inline]
fn motor_joint_ref(base: &JointSim) -> &MotorJoint {
    match &base.joint {
        JointUnion::Motor(j) => j,
        _ => panic!("wrong joint type"),
    }
}

pub fn motor_joint_set_linear_velocity(world: &mut World, joint_id: JointId, velocity: Vec3) {

    crate::recording::rec_op(world, crate::recording::RecOp::MotorJointSetLinearVelocity, |b| {
        b.w_jointid(joint_id);
        b.w_vec3(velocity);
    });
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_mut(joint).linear_velocity = velocity;
}

pub fn motor_joint_get_linear_velocity(world: &mut World, joint_id: JointId) -> Vec3 {
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_ref(joint).linear_velocity
}

pub fn motor_joint_set_angular_velocity(world: &mut World, joint_id: JointId, velocity: Vec3) {

    crate::recording::rec_op(world, crate::recording::RecOp::MotorJointSetAngularVelocity, |b| {
        b.w_jointid(joint_id);
        b.w_vec3(velocity);
    });
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_mut(joint).angular_velocity = velocity;
}

pub fn motor_joint_get_angular_velocity(world: &mut World, joint_id: JointId) -> Vec3 {
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_ref(joint).angular_velocity
}

pub fn motor_joint_set_max_velocity_torque(world: &mut World, joint_id: JointId, max_torque: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::MotorJointSetMaxVelocityTorque, |b| {
        b.w_jointid(joint_id);
        b.w_f32(max_torque);
    });
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_mut(joint).max_velocity_torque = max_torque;
}

pub fn motor_joint_get_max_velocity_torque(world: &mut World, joint_id: JointId) -> f32 {
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_ref(joint).max_velocity_torque
}

pub fn motor_joint_set_max_velocity_force(world: &mut World, joint_id: JointId, max_force: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::MotorJointSetMaxVelocityForce, |b| {
        b.w_jointid(joint_id);
        b.w_f32(max_force);
    });
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_mut(joint).max_velocity_force = max_force;
}

pub fn motor_joint_get_max_velocity_force(world: &mut World, joint_id: JointId) -> f32 {
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_ref(joint).max_velocity_force
}

pub fn motor_joint_set_linear_hertz(world: &mut World, joint_id: JointId, hertz: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::MotorJointSetLinearHertz, |b| {
        b.w_jointid(joint_id);
        b.w_f32(hertz);
    });
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_mut(joint).linear_hertz = hertz;
}

pub fn motor_joint_get_linear_hertz(world: &mut World, joint_id: JointId) -> f32 {
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_ref(joint).linear_hertz
}

pub fn motor_joint_set_linear_damping_ratio(world: &mut World, joint_id: JointId, damping: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::MotorJointSetLinearDampingRatio, |b| {
        b.w_jointid(joint_id);
        b.w_f32(damping);
    });
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_mut(joint).linear_damping_ratio = damping;
}

pub fn motor_joint_get_linear_damping_ratio(world: &mut World, joint_id: JointId) -> f32 {
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_ref(joint).linear_damping_ratio
}

pub fn motor_joint_set_angular_hertz(world: &mut World, joint_id: JointId, hertz: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::MotorJointSetAngularHertz, |b| {
        b.w_jointid(joint_id);
        b.w_f32(hertz);
    });
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_mut(joint).angular_hertz = hertz;
}

pub fn motor_joint_get_angular_hertz(world: &mut World, joint_id: JointId) -> f32 {
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_ref(joint).angular_hertz
}

pub fn motor_joint_set_angular_damping_ratio(world: &mut World, joint_id: JointId, damping: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::MotorJointSetAngularDampingRatio, |b| {
        b.w_jointid(joint_id);
        b.w_f32(damping);
    });
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_mut(joint).angular_damping_ratio = damping;
}

pub fn motor_joint_get_angular_damping_ratio(world: &mut World, joint_id: JointId) -> f32 {
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_ref(joint).angular_damping_ratio
}

pub fn motor_joint_set_max_spring_force(world: &mut World, joint_id: JointId, max_force: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::MotorJointSetMaxSpringForce, |b| {
        b.w_jointid(joint_id);
        b.w_f32(max_force);
    });
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_mut(joint).max_spring_force = max_float(0.0, max_force);
}

pub fn motor_joint_get_max_spring_force(world: &mut World, joint_id: JointId) -> f32 {
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_ref(joint).max_spring_force
}

pub fn motor_joint_set_max_spring_torque(world: &mut World, joint_id: JointId, max_torque: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::MotorJointSetMaxSpringTorque, |b| {
        b.w_jointid(joint_id);
        b.w_f32(max_torque);
    });
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_mut(joint).max_spring_torque = max_float(0.0, max_torque);
}

pub fn motor_joint_get_max_spring_torque(world: &mut World, joint_id: JointId) -> f32 {
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Motor);
    motor_joint_ref(joint).max_spring_torque
}

pub fn get_motor_joint_force(world: &World, base: &JointSim) -> Vec3 {
    let joint = motor_joint_ref(base);
    mul_sv(world.inv_h, add(joint.linear_velocity_impulse, joint.linear_spring_impulse))
}

pub fn get_motor_joint_torque(world: &World, base: &JointSim) -> Vec3 {
    let joint = motor_joint_ref(base);
    mul_sv(world.inv_h, add(joint.angular_velocity_impulse, joint.angular_spring_impulse))
}

// Point-to-point constraint
// C = p2 - p1
// Cdot = v2 - v1
//      = v2 + cross(w2, r2) - v1 - cross(w1, r1)
// J = [-I -r1_skew I r2_skew ]
// Identity used:
// w k % (rx i + ry j) = w * (-ry i + rx j)

// Angle constraint
// C = angle2 - angle1 - referenceAngle
// Cdot = w2 - w1
// J = [0 0 -1 0 0 1]
// K = invI1 + invI2

pub fn prepare_motor_joint(base: &mut JointSim, world: &World, context: &StepContext) {
    b3_assert!(base.joint_type == JointType::Motor);

    // chase body id to the solver set where the body lives
    let id_a = base.body_id_a;
    let id_b = base.body_id_b;

    let body_a = &world.bodies[id_a as usize];
    let body_b = &world.bodies[id_b as usize];

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
    let joint = motor_joint_mut(base);
    joint.index_a = if set_index_a == AWAKE_SET { local_index_a } else { NULL_INDEX };
    joint.index_b = if set_index_b == AWAKE_SET { local_index_b } else { NULL_INDEX };

    // Compute joint anchor frames with world space rotation, relative to center of mass
    joint.frame_a.q = mul_quat(body_sim_a.transform.q, local_frame_a.q);
    joint.frame_a.p = rotate_vector(body_sim_a.transform.q, sub(local_frame_a.p, body_sim_a.local_center));
    joint.frame_b.q = mul_quat(body_sim_b.transform.q, local_frame_b.q);
    joint.frame_b.p = rotate_vector(body_sim_b.transform.q, sub(local_frame_b.p, body_sim_b.local_center));

    // Compute the initial center delta. Incremental position updates are relative to this.
    joint.delta_center = sub_pos(body_sim_b.center, body_sim_a.center);

    joint.linear_spring = make_soft(joint.linear_hertz, joint.linear_damping_ratio, context.h);
    joint.angular_spring = make_soft(joint.angular_hertz, joint.angular_damping_ratio, context.h);

    joint.angular_mass = invert_matrix(inv_inertia_sum);

    if !context.enable_warm_starting {
        joint.linear_velocity_impulse = Vec3::ZERO;
        joint.angular_velocity_impulse = Vec3::ZERO;
        joint.linear_spring_impulse = Vec3::ZERO;
        joint.angular_spring_impulse = Vec3::ZERO;
    }
}

pub fn warm_start_motor_joint(base: &mut JointSim, states: &StateAccess, _context: &StepContext) {
    b3_assert!(base.joint_type == JointType::Motor);

    let m_a = base.inv_mass_a;
    let m_b = base.inv_mass_b;
    let i_a = base.inv_i_a;
    let i_b = base.inv_i_b;

    let joint = motor_joint_mut(base);

    // dummy state for static bodies
    // Field copies through short-lived borrows + a velocities-only
    // write-back (see spherical_joint.rs / StateAccess::set_velocities).
    let (mut v_a, mut w_a, dq_a) = {
        let s = if joint.index_a == NULL_INDEX { &IDENTITY_BODY_STATE } else { states.get_ref(joint.index_a as usize) };
        (s.linear_velocity, s.angular_velocity, s.delta_rotation)
    };
    let (mut v_b, mut w_b, dq_b) = {
        let s = if joint.index_b == NULL_INDEX { &IDENTITY_BODY_STATE } else { states.get_ref(joint.index_b as usize) };
        (s.linear_velocity, s.angular_velocity, s.delta_rotation)
    };

    let r_a = rotate_vector(dq_a, joint.frame_a.p);
    let r_b = rotate_vector(dq_b, joint.frame_b.p);

    let linear_impulse = add(joint.linear_velocity_impulse, joint.linear_spring_impulse);
    let angular_impulse = add(joint.angular_velocity_impulse, joint.angular_spring_impulse);

    // C writes unconditionally here (no dynamicFlag guard)
    v_a = mul_sub(v_a, m_a, linear_impulse);
    w_a = sub(w_a, mul_mv(i_a, add(cross(r_a, linear_impulse), angular_impulse)));
    v_b = mul_add(v_b, m_b, linear_impulse);
    w_b = add(w_b, mul_mv(i_b, add(cross(r_b, linear_impulse), angular_impulse)));

    // C writes unconditionally here (no dynamicFlag guard).
    if joint.index_a != NULL_INDEX {
        states.set_velocities(joint.index_a as usize, v_a, w_a);
    }
    if joint.index_b != NULL_INDEX {
        states.set_velocities(joint.index_b as usize, v_b, w_b);
    }
}

pub fn solve_motor_joint(base: &mut JointSim, states: &StateAccess, context: &StepContext) {
    b3_assert!(base.joint_type == JointType::Motor);

    let m_a = base.inv_mass_a;
    let m_b = base.inv_mass_b;
    let i_a = base.inv_i_a;
    let i_b = base.inv_i_b;

    let joint = motor_joint_mut(base);

    // dummy state for static bodies
    // Field copies through short-lived borrows + a velocities-only
    // write-back (see spherical_joint.rs / StateAccess::set_velocities).
    let (mut v_a, mut w_a, dq_a, dp_a, flags_a) = {
        let s = if joint.index_a == NULL_INDEX { &IDENTITY_BODY_STATE } else { states.get_ref(joint.index_a as usize) };
        (s.linear_velocity, s.angular_velocity, s.delta_rotation, s.delta_position, s.flags)
    };
    let (mut v_b, mut w_b, dq_b, dp_b, flags_b) = {
        let s = if joint.index_b == NULL_INDEX { &IDENTITY_BODY_STATE } else { states.get_ref(joint.index_b as usize) };
        (s.linear_velocity, s.angular_velocity, s.delta_rotation, s.delta_position, s.flags)
    };

    let quat_a = mul_quat(dq_a, joint.frame_a.q);
    let mut quat_b = mul_quat(dq_b, joint.frame_b.q);

    if dot_quat(quat_a, quat_b) < 0.0 {
        // this keeps the rotation angle in the range [-pi, pi]
        quat_b = negate_quat(quat_b);
    }

    let rel_q = inv_mul_quat(quat_a, quat_b);

    // angular spring
    if joint.max_spring_torque > 0.0 && joint.angular_hertz > 0.0 {
        let target_quat = Quat::IDENTITY;
        let delta_rotation = delta_quat_to_rotation(rel_q, target_quat);
        let c = neg(rotate_vector(quat_a, delta_rotation));

        let bias = mul_sv(joint.angular_spring.bias_rate, c);
        let mass_scale = joint.angular_spring.mass_scale;
        let impulse_scale = joint.angular_spring.impulse_scale;

        let cdot = sub(w_b, w_a);

        let max_impulse = context.h * joint.max_spring_torque;
        let old_impulse = joint.angular_spring_impulse;
        let mut impulse = mul_sub(
            mul_sv(-mass_scale, mul_mv(joint.angular_mass, add(cdot, bias))),
            impulse_scale,
            old_impulse,
        );
        joint.angular_spring_impulse = add(old_impulse, impulse);
        if length_squared(joint.angular_spring_impulse) > max_impulse * max_impulse {
            joint.angular_spring_impulse = mul_sv(max_impulse, normalize(joint.angular_spring_impulse));
        }
        impulse = sub(joint.angular_spring_impulse, old_impulse);

        w_a = sub(w_a, mul_mv(i_a, impulse));
        w_b = add(w_b, mul_mv(i_b, impulse));
    }

    // angular velocity
    if joint.max_velocity_torque > 0.0 {
        let cdot = sub(sub(w_b, w_a), joint.angular_velocity);
        let mut impulse = neg(mul_mv(joint.angular_mass, cdot));

        let max_impulse = context.h * joint.max_velocity_torque;
        let old_impulse = joint.angular_velocity_impulse;
        joint.angular_velocity_impulse = add(old_impulse, impulse);
        if length_squared(joint.angular_velocity_impulse) > max_impulse * max_impulse {
            joint.angular_velocity_impulse = mul_sv(max_impulse, normalize(joint.angular_velocity_impulse));
        }
        impulse = sub(joint.angular_velocity_impulse, old_impulse);

        w_a = sub(w_a, mul_mv(i_a, impulse));
        w_b = add(w_b, mul_mv(i_b, impulse));
    }

    let r_a = rotate_vector(dq_a, joint.frame_a.p);
    let r_b = rotate_vector(dq_b, joint.frame_b.p);

    // linear spring
    if joint.max_spring_force > 0.0 && joint.linear_hertz > 0.0 {
        let dc_a = dp_a;
        let dc_b = dp_b;
        let c = add(add(sub(dc_b, dc_a), sub(r_b, r_a)), joint.delta_center);

        let bias = mul_sv(joint.linear_spring.bias_rate, c);
        let mass_scale = joint.linear_spring.mass_scale;
        let impulse_scale = joint.linear_spring.impulse_scale;

        let cdot = sub(add(v_b, cross(w_b, r_b)), add(v_a, cross(w_a, r_a)));

        //// K = [(1/m1 + 1/m2) * eye(2) - skew(r1) * invI1 * skew(r1) - skew(r2) * invI2 * skew(r2)]
        let s_a = skew(r_a);
        let s_b = skew(r_b);
        let k_a = mul_mm(s_a, mul_mm(i_a, s_a));
        let k_b = mul_mm(s_b, mul_mm(i_b, s_b));
        let mut k = negate_mat3(add_mm(k_a, k_b));
        k.cx.x += m_a + m_b;
        k.cy.y += m_a + m_b;
        k.cz.z += m_a + m_b;

        let b = solve3(k, add(cdot, bias));

        let old_impulse = joint.linear_spring_impulse;
        let mut impulse = mul_sub(mul_sv(-mass_scale, b), impulse_scale, old_impulse);
        let max_impulse = context.h * joint.max_spring_force;
        joint.linear_spring_impulse = add(joint.linear_spring_impulse, impulse);

        if length_squared(joint.linear_spring_impulse) > max_impulse * max_impulse {
            joint.linear_spring_impulse = mul_sv(max_impulse, normalize(joint.linear_spring_impulse));
        }

        impulse = sub(joint.linear_spring_impulse, old_impulse);

        v_a = mul_sub(v_a, m_a, impulse);
        w_a = sub(w_a, mul_mv(i_a, cross(r_a, impulse)));
        v_b = mul_add(v_b, m_b, impulse);
        w_b = add(w_b, mul_mv(i_b, cross(r_b, impulse)));
    }

    // linear velocity
    if joint.max_velocity_force > 0.0 {
        let mut cdot = sub(add(v_b, cross(w_b, r_b)), add(v_a, cross(w_a, r_a)));
        cdot = sub(cdot, joint.linear_velocity);
        //// K = [(1/m1 + 1/m2) * eye(2) - skew(r1) * invI1 * skew(r1) - skew(r2) * invI2 * skew(r2)]
        let s_a = skew(r_a);
        let s_b = skew(r_b);
        let k_a = mul_mm(s_a, mul_mm(i_a, s_a));
        let k_b = mul_mm(s_b, mul_mm(i_b, s_b));
        let mut k = negate_mat3(add_mm(k_a, k_b));
        k.cx.x += m_a + m_b;
        k.cy.y += m_a + m_b;
        k.cz.z += m_a + m_b;

        let b = solve3(k, cdot);
        let mut impulse = neg(b);

        let old_impulse = joint.linear_velocity_impulse;
        let max_impulse = context.h * joint.max_velocity_force;
        joint.linear_velocity_impulse = add(joint.linear_velocity_impulse, impulse);

        if length_squared(joint.linear_velocity_impulse) > max_impulse * max_impulse {
            joint.linear_velocity_impulse = mul_sv(max_impulse, normalize(joint.linear_velocity_impulse));
        }

        impulse = sub(joint.linear_velocity_impulse, old_impulse);

        v_a = mul_sub(v_a, m_a, impulse);
        w_a = sub(w_a, mul_mv(i_a, cross(r_a, impulse)));
        v_b = mul_add(v_b, m_b, impulse);
        w_b = add(w_b, mul_mv(i_b, cross(r_b, impulse)));
    }

    // C stores unconditionally through the state pointer for dynamic bodies;
    // the non-dynamic write in the old code was a byte-identical no-op.
    if joint.index_a != NULL_INDEX && (flags_a & DYNAMIC_FLAG != 0) {
        states.set_velocities(joint.index_a as usize, v_a, w_a);
    }
    if joint.index_b != NULL_INDEX && (flags_b & DYNAMIC_FLAG != 0) {
        states.set_velocities(joint.index_b as usize, v_b, w_b);
    }
}

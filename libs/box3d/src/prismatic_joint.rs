// Port of box3d/src/prismatic_joint.c
//
// Notes:
// - b3DefaultPrismaticJointDef / b3CreatePrismaticJoint live in joint.c (ported in joint.rs).
// - b3DrawPrismaticJoint (debug draw) is not ported.
// - Recording hooks (B3_REC) are not ported.
// - Body states are copied to locals and written back on exit; C dynamicFlag
//   guards preserved on the locals.

use crate::b3_assert;
use crate::body::{DYNAMIC_FLAG, IDENTITY_BODY_STATE};
use crate::core::NULL_INDEX;
use crate::id::JointId;
use crate::joint::{JointSim, JointUnion, PrismaticJoint};
use crate::math_functions::{
    add, add_mm, blend2, clamp_float, cross, det, dot, invert_matrix, is_valid_float, is_valid_vec3,
    make_matrix_from_quat, max_float, min_float, mul_add, mul_mv, mul_quat, mul_sub, mul_sv,
    inv_mul_quat, neg, rotate_vector, sub, sub_pos, vec2, vec3, Quat, Vec3,
};
use crate::math_internal::{add2, blend3, delta_quat_to_rotation, mul_sv2, solve2, sub2, Matrix2};
use crate::physics_world::{World, AWAKE_SET};
use crate::solver::{make_soft, StateAccess, StepContext};
use crate::types::JointType;

// Linear constraint (point-to-line)
// joint axis is along joint frame A local z-axis
// perpX and perpY are world vectors fixed in A
//
// d = pB - pA = xB + rB - xA - rA
// Cx = dot(perpX, d)
// Cy = dot(perpY, d)

// CdotX = dot(d, cross(wA, perpX)) + dot(perpX, vB + cross(wB, rB) - vA - cross(wA, rA))
//      = -dot(perpX, vA) - dot(cross(d + rA, perpX), wA) + dot(perpX, vB) + dot(cross(rB, perpX), vB)
// Jx = [-perpX, -cross(d + rA, perpX), perpX, cross(rB, perpX)]
// similar for perpY

// Motor/limit/spring linear constraint
// axis is the world joint axis fixed in A
//
// C = dot(axis, d)
// Cdot = dot(d, cross(wA, axis)) + dot(axis, vB + cross(wB, rB) - vA - cross(wA, rA))
// J = [-axis -cross(d + rA, axis) axis cross(rB, axis)]

// Predictive limit is applied even when the limit is not active.
// Prevents a constraint speed that can lead to a constraint error in one time step.

#[inline]
fn prismatic_joint_mut(base: &mut JointSim) -> &mut PrismaticJoint {
    match &mut base.joint {
        JointUnion::Prismatic(j) => j,
        _ => panic!("wrong joint type"),
    }
}

#[inline]
fn prismatic_joint_ref(base: &JointSim) -> &PrismaticJoint {
    match &base.joint {
        JointUnion::Prismatic(j) => j,
        _ => panic!("wrong joint type"),
    }
}

pub fn prismatic_joint_enable_limit(world: &mut World, joint_id: JointId, enable_limit: bool) {

    crate::recording::rec_op(world, crate::recording::RecOp::PrismaticJointEnableLimit, |b| {
        b.w_jointid(joint_id);
        b.w_bool(enable_limit);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    let joint = prismatic_joint_mut(base);
    if enable_limit != joint.enable_limit {
        joint.lower_impulse = 0.0;
        joint.upper_impulse = 0.0;
    }
    joint.enable_limit = enable_limit;
}

pub fn prismatic_joint_is_limit_enabled(world: &mut World, joint_id: JointId) -> bool {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    prismatic_joint_ref(base).enable_limit
}

pub fn prismatic_joint_get_lower_limit(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    prismatic_joint_ref(base).lower_translation
}

pub fn prismatic_joint_get_upper_limit(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    prismatic_joint_ref(base).upper_translation
}

pub fn prismatic_joint_set_limits(world: &mut World, joint_id: JointId, lower: f32, upper: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::PrismaticJointSetLimits, |b| {
        b.w_jointid(joint_id);
        b.w_f32(lower);
        b.w_f32(upper);
    });
    b3_assert!(is_valid_float(lower) && is_valid_float(upper));
    let lower_angle = min_float(lower, upper);
    let upper_angle = max_float(lower, upper);

    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    let joint = prismatic_joint_mut(base);
    joint.lower_translation = lower_angle;
    joint.upper_translation = upper_angle;
}

pub fn prismatic_joint_get_translation(world: &mut World, joint_id: JointId) -> f32 {
    let (body_id_a, body_id_b, local_frame_a, local_frame_b_p) = {
        let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
        (base.body_id_a, base.body_id_b, base.local_frame_a, base.local_frame_b.p)
    };

    let transform_a = crate::body::get_body_transform(world, body_id_a);
    let transform_b = crate::body::get_body_transform(world, body_id_b);

    let mut joint_axis = rotate_vector(local_frame_a.q, Vec3::AXIS_X);
    joint_axis = rotate_vector(transform_a.q, joint_axis);

    let anchor_a = rotate_vector(transform_a.q, local_frame_a.p);
    let anchor_b = rotate_vector(transform_b.q, local_frame_b_p);
    let d = add(sub_pos(transform_b.p, transform_a.p), sub(anchor_b, anchor_a));
    dot(d, joint_axis)
}

pub fn prismatic_joint_enable_spring(world: &mut World, joint_id: JointId, enable_spring: bool) {

    crate::recording::rec_op(world, crate::recording::RecOp::PrismaticJointEnableSpring, |b| {
        b.w_jointid(joint_id);
        b.w_bool(enable_spring);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    let joint = prismatic_joint_mut(base);
    if enable_spring != joint.enable_spring {
        joint.spring_impulse = 0.0;
    }
    joint.enable_spring = enable_spring;
}

pub fn prismatic_joint_is_spring_enabled(world: &mut World, joint_id: JointId) -> bool {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    prismatic_joint_ref(base).enable_spring
}

pub fn prismatic_joint_set_target_translation(world: &mut World, joint_id: JointId, target_translation: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::PrismaticJointSetTargetTranslation, |b| {
        b.w_jointid(joint_id);
        b.w_f32(target_translation);
    });
    b3_assert!(is_valid_float(target_translation));
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    prismatic_joint_mut(base).target_translation = target_translation;
}

pub fn prismatic_joint_get_target_translation(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    prismatic_joint_ref(base).target_translation
}

pub fn prismatic_joint_set_spring_hertz(world: &mut World, joint_id: JointId, hertz: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::PrismaticJointSetSpringHertz, |b| {
        b.w_jointid(joint_id);
        b.w_f32(hertz);
    });
    b3_assert!(is_valid_float(hertz) && hertz >= 0.0);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    prismatic_joint_mut(base).hertz = hertz;
}

pub fn prismatic_joint_get_spring_hertz(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    prismatic_joint_ref(base).hertz
}

pub fn prismatic_joint_set_spring_damping_ratio(world: &mut World, joint_id: JointId, damping_ratio: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::PrismaticJointSetSpringDampingRatio, |b| {
        b.w_jointid(joint_id);
        b.w_f32(damping_ratio);
    });
    b3_assert!(is_valid_float(damping_ratio) && damping_ratio >= 0.0);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    prismatic_joint_mut(base).damping_ratio = damping_ratio;
}

pub fn prismatic_joint_get_spring_damping_ratio(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    prismatic_joint_ref(base).damping_ratio
}

pub fn prismatic_joint_enable_motor(world: &mut World, joint_id: JointId, enable_motor: bool) {

    crate::recording::rec_op(world, crate::recording::RecOp::PrismaticJointEnableMotor, |b| {
        b.w_jointid(joint_id);
        b.w_bool(enable_motor);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    let joint = prismatic_joint_mut(base);
    if enable_motor != joint.enable_motor {
        joint.motor_impulse = 0.0;
    }
    joint.enable_motor = enable_motor;
}

pub fn prismatic_joint_is_motor_enabled(world: &mut World, joint_id: JointId) -> bool {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    prismatic_joint_ref(base).enable_motor
}

pub fn prismatic_joint_set_motor_speed(world: &mut World, joint_id: JointId, motor_speed: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::PrismaticJointSetMotorSpeed, |b| {
        b.w_jointid(joint_id);
        b.w_f32(motor_speed);
    });
    b3_assert!(is_valid_float(motor_speed));
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    prismatic_joint_mut(base).motor_speed = motor_speed;
}

pub fn prismatic_joint_get_motor_speed(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    prismatic_joint_ref(base).motor_speed
}

pub fn prismatic_joint_set_max_motor_force(world: &mut World, joint_id: JointId, max_force: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::PrismaticJointSetMaxMotorForce, |b| {
        b.w_jointid(joint_id);
        b.w_f32(max_force);
    });
    b3_assert!(is_valid_float(max_force) && max_force >= 0.0);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    prismatic_joint_mut(base).max_motor_force = max_force;
}

pub fn prismatic_joint_get_max_motor_force(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    prismatic_joint_ref(base).max_motor_force
}

pub fn prismatic_joint_get_motor_force(world: &mut World, joint_id: JointId) -> f32 {
    let inv_h = world.inv_h;
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
    inv_h * prismatic_joint_ref(base).motor_impulse
}

pub fn prismatic_joint_get_speed(world: &mut World, joint_id: JointId) -> f32 {
    let (body_id_a, body_id_b, local_frame_a, local_frame_b_p) = {
        let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Prismatic);
        (base.body_id_a, base.body_id_b, base.local_frame_a, base.local_frame_b.p)
    };

    let world = &*world;
    let body_a = &world.bodies[body_id_a as usize];
    let body_b = &world.bodies[body_id_b as usize];
    let body_sim_a = *crate::body::get_body_sim_from_id(world, body_id_a);
    let body_sim_b = *crate::body::get_body_sim_from_id(world, body_id_b);

    // C: b3GetBodyState returns NULL for non-awake bodies
    let state_a = if body_a.set_index == AWAKE_SET {
        Some(world.solver_sets[AWAKE_SET as usize].body_states[body_a.local_index as usize])
    } else {
        None
    };
    let state_b = if body_b.set_index == AWAKE_SET {
        Some(world.solver_sets[AWAKE_SET as usize].body_states[body_b.local_index as usize])
    } else {
        None
    };

    let q_a = body_sim_a.transform.q;
    let q_b = body_sim_b.transform.q;

    let axis_a = rotate_vector(q_a, rotate_vector(local_frame_a.q, Vec3::AXIS_X));
    let r_a = rotate_vector(q_a, sub(local_frame_a.p, body_sim_a.local_center));
    let r_b = rotate_vector(q_b, sub(local_frame_b_p, body_sim_b.local_center));

    // Difference the centers in double so the speed stays exact far from the origin.
    let d = add(sub_pos(body_sim_b.center, body_sim_a.center), sub(r_b, r_a));

    let v_a = state_a.map_or(Vec3::ZERO, |s| s.linear_velocity);
    let v_b = state_b.map_or(Vec3::ZERO, |s| s.linear_velocity);
    let w_a = state_a.map_or(Vec3::ZERO, |s| s.angular_velocity);
    let w_b = state_b.map_or(Vec3::ZERO, |s| s.angular_velocity);

    let v_rel = sub(add(v_b, cross(w_b, r_b)), add(v_a, cross(w_a, r_a)));

    // The axis moves with body A, so account for its rotation.
    dot(d, cross(w_a, axis_a)) + dot(axis_a, v_rel)
}

pub fn get_prismatic_joint_force(world: &World, base: &JointSim) -> Vec3 {
    let transform_a = crate::body::get_body_transform(world, base.body_id_a);
    let joint = prismatic_joint_ref(base);

    // impulse in joint space
    let impulse = vec3(
        joint.perp_impulse.x,
        joint.perp_impulse.y,
        joint.motor_impulse + joint.lower_impulse + joint.upper_impulse + joint.spring_impulse,
    );

    // convert impulse to force
    let mut force = mul_sv(world.inv_h, impulse);

    // convert to body space
    force = rotate_vector(base.local_frame_a.q, force);

    // convert to world space
    force = rotate_vector(transform_a.q, force);
    force
}

pub fn get_prismatic_joint_torque(world: &World, base: &JointSim) -> Vec3 {
    let transform_a = crate::body::get_body_transform(world, base.body_id_a);
    let joint = prismatic_joint_ref(base);

    let mut torque = mul_sv(world.inv_h, joint.angular_impulse);
    torque = rotate_vector(base.local_frame_a.q, torque);
    torque = rotate_vector(transform_a.q, torque);
    torque
}

pub fn prepare_prismatic_joint(base: &mut JointSim, world: &World, context: &StepContext) {
    b3_assert!(base.joint_type == JointType::Prismatic);

    let body_a = &world.bodies[base.body_id_a as usize];
    let body_b = &world.bodies[base.body_id_b as usize];

    b3_assert!(body_b.set_index == AWAKE_SET);

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
    let joint = prismatic_joint_mut(base);
    joint.index_a = if set_index_a == AWAKE_SET { local_index_a } else { NULL_INDEX };
    joint.index_b = if set_index_b == AWAKE_SET { local_index_b } else { NULL_INDEX };

    // Compute joint anchor frames with world space rotation, relative to center of mass
    joint.frame_a.q = mul_quat(body_sim_a.transform.q, local_frame_a.q);
    joint.frame_a.p = rotate_vector(body_sim_a.transform.q, sub(local_frame_a.p, body_sim_a.local_center));
    joint.frame_b.q = mul_quat(body_sim_b.transform.q, local_frame_b.q);
    joint.frame_b.p = rotate_vector(body_sim_b.transform.q, sub(local_frame_b.p, body_sim_b.local_center));

    joint.delta_center = sub_pos(body_sim_b.center, body_sim_a.center);
    joint.rotation_mass = invert_matrix(inv_inertia_sum);

    // Initial joint axes in world space
    let matrix_a = make_matrix_from_quat(joint.frame_a.q);
    joint.joint_axis = matrix_a.cx;
    joint.perp_axis_y = matrix_a.cy;
    joint.perp_axis_z = matrix_a.cz;

    joint.spring_softness = make_soft(joint.hertz, joint.damping_ratio, context.h);

    if !context.enable_warm_starting {
        joint.perp_impulse = vec2(0.0, 0.0);
        joint.angular_impulse = vec3(0.0, 0.0, 0.0);
        joint.motor_impulse = 0.0;
        joint.spring_impulse = 0.0;
        joint.lower_impulse = 0.0;
        joint.upper_impulse = 0.0;
    }
}

pub fn warm_start_prismatic_joint(base: &mut JointSim, states: &StateAccess, _context: &StepContext) {
    b3_assert!(base.joint_type == JointType::Prismatic);

    let m_a = base.inv_mass_a;
    let m_b = base.inv_mass_b;
    let i_a = base.inv_i_a;
    let i_b = base.inv_i_b;

    let joint = prismatic_joint_mut(base);

    // dummy state for static bodies
    let mut state_a = if joint.index_a == NULL_INDEX {
        IDENTITY_BODY_STATE
    } else {
        states.get(joint.index_a as usize)
    };
    let mut state_b = if joint.index_b == NULL_INDEX {
        IDENTITY_BODY_STATE
    } else {
        states.get(joint.index_b as usize)
    };

    // todo make this code and the wheel joint more similar

    let r_a = rotate_vector(state_a.delta_rotation, joint.frame_a.p);
    let r_b = rotate_vector(state_b.delta_rotation, joint.frame_b.p);
    let d = add(add(sub(state_b.delta_position, state_a.delta_position), joint.delta_center), sub(r_b, r_a));
    let joint_axis = rotate_vector(state_a.delta_rotation, joint.joint_axis);
    let s_ax = cross(add(r_a, d), joint_axis);
    let s_bx = cross(r_b, joint_axis);

    let perp_y = rotate_vector(state_a.delta_rotation, joint.perp_axis_y);
    let perp_z = rotate_vector(state_a.delta_rotation, joint.perp_axis_z);
    let s_ay = cross(add(r_a, d), perp_y);
    let s_by = cross(r_b, perp_y);
    let s_az = cross(add(r_a, d), perp_z);
    let s_bz = cross(r_b, perp_z);

    let axial_impulse = joint.spring_impulse + joint.motor_impulse + joint.lower_impulse - joint.upper_impulse;
    let perp_impulse = joint.perp_impulse;

    let p = blend3(axial_impulse, joint_axis, perp_impulse.x, perp_y, perp_impulse.y, perp_z);
    let l_a = add(blend3(axial_impulse, s_ax, perp_impulse.x, s_ay, perp_impulse.y, s_az), joint.angular_impulse);
    let l_b = add(blend3(axial_impulse, s_bx, perp_impulse.x, s_by, perp_impulse.y, s_bz), joint.angular_impulse);

    let mut v_a = state_a.linear_velocity;
    let mut w_a = state_a.angular_velocity;
    let mut v_b = state_b.linear_velocity;
    let mut w_b = state_b.angular_velocity;
    v_a = mul_sub(v_a, m_a, p);
    w_a = sub(w_a, mul_mv(i_a, l_a));
    v_b = mul_add(v_b, m_b, p);
    w_b = add(w_b, mul_mv(i_b, l_b));

    if state_a.flags & DYNAMIC_FLAG != 0 {
        state_a.linear_velocity = v_a;
        state_a.angular_velocity = w_a;
    }

    if state_b.flags & DYNAMIC_FLAG != 0 {
        state_b.linear_velocity = v_b;
        state_b.angular_velocity = w_b;
    }

    if joint.index_a != NULL_INDEX {
        states.set(joint.index_a as usize, state_a);
    }
    if joint.index_b != NULL_INDEX {
        states.set(joint.index_b as usize, state_b);
    }
}

pub fn solve_prismatic_joint(base: &mut JointSim, states: &StateAccess, context: &StepContext, use_bias: bool) {
    let m_a = base.inv_mass_a;
    let m_b = base.inv_mass_b;
    let i_a = base.inv_i_a;
    let i_b = base.inv_i_b;
    let fixed_rotation = base.fixed_rotation;
    let constraint_softness = base.constraint_softness;

    let joint = prismatic_joint_mut(base);

    // dummy state for static bodies
    let mut state_a = if joint.index_a == NULL_INDEX {
        IDENTITY_BODY_STATE
    } else {
        states.get(joint.index_a as usize)
    };
    let mut state_b = if joint.index_b == NULL_INDEX {
        IDENTITY_BODY_STATE
    } else {
        states.get(joint.index_b as usize)
    };

    let mut v_a = state_a.linear_velocity;
    let mut w_a = state_a.angular_velocity;
    let mut v_b = state_b.linear_velocity;
    let mut w_b = state_b.angular_velocity;

    let r_a = rotate_vector(state_a.delta_rotation, joint.frame_a.p);
    let r_b = rotate_vector(state_b.delta_rotation, joint.frame_b.p);

    let dc_a = state_a.delta_position;
    let dc_b = state_b.delta_position;
    let d = add(add(sub(dc_b, dc_a), joint.delta_center), sub(r_b, r_a));

    let joint_axis = rotate_vector(state_a.delta_rotation, joint.joint_axis);
    let s_ax = cross(add(r_a, d), joint_axis);
    let s_bx = cross(r_b, joint_axis);
    let joint_translation = dot(d, joint_axis);
    let target_translation = joint.target_translation;

    // The axial effective mass must be fresh to avoid divergence when the joint is stressed
    let ka = m_a + m_b + dot(s_ax, mul_mv(i_a, s_ax)) + dot(s_bx, mul_mv(i_b, s_bx));
    let axial_mass = if ka > 0.0 { 1.0 / ka } else { 0.0 };

    // Solve spring
    if joint.enable_spring && !fixed_rotation {
        // Get the substep relative rotation
        let c = joint_translation - target_translation;

        let bias = joint.spring_softness.bias_rate * c;
        let mass_scale = joint.spring_softness.mass_scale;
        let impulse_scale = joint.spring_softness.impulse_scale;

        let v_rel = sub(sub(add(v_b, cross(w_b, r_b)), v_a), cross(w_a, add(r_a, d)));
        let cdot = dot(v_rel, joint_axis);
        let delta_impulse = (-impulse_scale).mul_add(joint.spring_impulse, -mass_scale * axial_mass * (cdot + bias));
        joint.spring_impulse += delta_impulse;

        let p = mul_sv(delta_impulse, joint_axis);
        let l_a = mul_sv(delta_impulse, s_ax);
        let l_b = mul_sv(delta_impulse, s_bx);

        v_a = mul_sub(v_a, m_a, p);
        w_a = sub(w_a, mul_mv(i_a, l_a));
        v_b = mul_add(v_b, m_b, p);
        w_b = add(w_b, mul_mv(i_b, l_b));
    }

    if joint.enable_motor && !fixed_rotation {
        let v_rel = sub(sub(add(v_b, cross(w_b, r_b)), v_a), cross(w_a, add(r_a, d)));
        let cdot = dot(v_rel, joint_axis) - joint.motor_speed;

        let mut delta_impulse = -axial_mass * cdot;
        let mut new_impulse = joint.motor_impulse + delta_impulse;
        let max_impulse = joint.max_motor_force * context.h;
        new_impulse = clamp_float(new_impulse, -max_impulse, max_impulse);
        delta_impulse = new_impulse - joint.motor_impulse;
        joint.motor_impulse = new_impulse;

        let p = mul_sv(delta_impulse, joint_axis);
        let l_a = mul_sv(delta_impulse, s_ax);
        let l_b = mul_sv(delta_impulse, s_bx);

        v_a = mul_sub(v_a, m_a, p);
        w_a = sub(w_a, mul_mv(i_a, l_a));
        v_b = mul_add(v_b, m_b, p);
        w_b = add(w_b, mul_mv(i_b, l_b));
    }

    if joint.enable_limit && !fixed_rotation {
        let speculative_distance = 0.25 * (joint.upper_translation - joint.lower_translation);

        // Lower limit
        {
            let c = joint_translation - joint.lower_translation;

            if c < speculative_distance {
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

                let v_rel = sub(sub(add(v_b, cross(w_b, r_b)), v_a), cross(w_a, add(r_a, d)));
                let cdot = dot(v_rel, joint_axis);
                let old_impulse = joint.lower_impulse;
                let mut delta_impulse = (-impulse_scale).mul_add(old_impulse, -mass_scale * axial_mass * (cdot + bias));
                joint.lower_impulse = max_float(old_impulse + delta_impulse, 0.0);
                delta_impulse = joint.lower_impulse - old_impulse;

                let p = mul_sv(delta_impulse, joint_axis);
                let l_a = mul_sv(delta_impulse, s_ax);
                let l_b = mul_sv(delta_impulse, s_bx);

                v_a = mul_sub(v_a, m_a, p);
                w_a = sub(w_a, mul_mv(i_a, l_a));
                v_b = mul_add(v_b, m_b, p);
                w_b = add(w_b, mul_mv(i_b, l_b));
            } else {
                joint.lower_impulse = 0.0;
            }
        }

        // Upper limit
        {
            let c = joint.upper_translation - joint_translation;

            if c < speculative_distance {
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
                let v_rel = sub(sub(add(v_b, cross(w_b, r_b)), v_a), cross(w_a, add(r_a, d)));
                let cdot = -dot(v_rel, joint_axis);
                let old_impulse = joint.upper_impulse;
                let delta_impulse = (-impulse_scale).mul_add(old_impulse, -mass_scale * axial_mass * (cdot + bias));
                joint.upper_impulse = max_float(old_impulse + delta_impulse, 0.0);

                // sign flipped on applied impulse
                let neg_delta_impulse = old_impulse - joint.upper_impulse;
                let p = mul_sv(neg_delta_impulse, joint_axis);
                let l_a = mul_sv(neg_delta_impulse, s_ax);
                let l_b = mul_sv(neg_delta_impulse, s_bx);

                v_a = mul_sub(v_a, m_a, p);
                w_a = sub(w_a, mul_mv(i_a, l_a));
                v_b = mul_add(v_b, m_b, p);
                w_b = add(w_b, mul_mv(i_b, l_b));
            } else {
                joint.upper_impulse = 0.0;
            }
        }
    }

    // Rotation constraint
    if !fixed_rotation {
        let mut bias = vec3(0.0, 0.0, 0.0);
        let mut mass_scale = 1.0;
        let mut impulse_scale = 0.0;

        if use_bias {
            let quat_a = mul_quat(state_a.delta_rotation, joint.frame_a.q);
            let quat_b = mul_quat(state_b.delta_rotation, joint.frame_b.q);

            let rel_q = inv_mul_quat(quat_a, quat_b);
            let target_quat = Quat::IDENTITY;
            let delta_rotation = delta_quat_to_rotation(rel_q, target_quat);
            let c = neg(rotate_vector(quat_a, delta_rotation));

            bias = mul_sv(constraint_softness.bias_rate, c);
            mass_scale = constraint_softness.mass_scale;
            impulse_scale = constraint_softness.impulse_scale;
        }

        let cdot = sub(w_b, w_a);
        let impulse = sub(
            mul_sv(-mass_scale, mul_mv(joint.rotation_mass, add(cdot, bias))),
            mul_sv(impulse_scale, joint.angular_impulse),
        );
        joint.angular_impulse = add(joint.angular_impulse, impulse);

        w_a = sub(w_a, mul_mv(i_a, impulse));
        w_b = add(w_b, mul_mv(i_b, impulse));
    }

    // Solve point-to-line constraint
    {
        let perp_y = rotate_vector(state_a.delta_rotation, joint.perp_axis_y);
        let perp_z = rotate_vector(state_a.delta_rotation, joint.perp_axis_z);

        let mut bias = vec2(0.0, 0.0);
        let mut mass_scale = 1.0;
        let mut impulse_scale = 0.0;
        if use_bias {
            let c = vec2(dot(perp_y, d), dot(perp_z, d));
            bias = mul_sv2(constraint_softness.bias_rate, c);
            mass_scale = constraint_softness.mass_scale;
            impulse_scale = constraint_softness.impulse_scale;
        }

        let v_rel = sub(sub(add(v_b, cross(w_b, r_b)), v_a), cross(w_a, add(r_a, d)));
        let cdot = vec2(dot(perp_y, v_rel), dot(perp_z, v_rel));

        // K = [(1/mA + 1/mB) * eye(2) - skew(rA) * invIA * skew(rA) - skew(rB) * invIB * skew(rB)]
        // Jx = [-perpX, -cross(d + rA, perpX), perpX, cross(rB, perpX)]
        let s_ay = cross(add(r_a, d), perp_y);
        let s_by = cross(r_b, perp_y);
        let s_az = cross(add(r_a, d), perp_z);
        let s_bz = cross(r_b, perp_z);

        let kyy = m_a + m_b + dot(s_ay, mul_mv(i_a, s_ay)) + dot(s_by, mul_mv(i_b, s_by));
        let kyz = dot(s_ay, mul_mv(i_a, s_az)) + dot(s_by, mul_mv(i_b, s_bz));
        let kzz = m_a + m_b + dot(s_az, mul_mv(i_a, s_az)) + dot(s_bz, mul_mv(i_b, s_bz));

        let k = Matrix2 { cx: vec2(kyy, kyz), cy: vec2(kyz, kzz) };

        let old_impulse = joint.perp_impulse;
        let sol = solve2(k, add2(cdot, bias));
        let delta_impulse = sub2(mul_sv2(-mass_scale, sol), mul_sv2(impulse_scale, old_impulse));
        joint.perp_impulse = add2(old_impulse, delta_impulse);

        let p = blend2(delta_impulse.x, perp_y, delta_impulse.y, perp_z);

        v_a = mul_sub(v_a, m_a, p);
        w_a = sub(w_a, mul_mv(i_a, blend2(delta_impulse.x, s_ay, delta_impulse.y, s_az)));
        v_b = mul_add(v_b, m_b, p);
        w_b = add(w_b, mul_mv(i_b, blend2(delta_impulse.x, s_by, delta_impulse.y, s_bz)));
    }

    b3_assert!(is_valid_vec3(v_a));
    b3_assert!(is_valid_vec3(w_a));
    b3_assert!(is_valid_vec3(v_b));
    b3_assert!(is_valid_vec3(w_b));

    if state_a.flags & DYNAMIC_FLAG != 0 {
        state_a.linear_velocity = v_a;
        state_a.angular_velocity = w_a;
    }

    if state_b.flags & DYNAMIC_FLAG != 0 {
        state_b.linear_velocity = v_b;
        state_b.angular_velocity = w_b;
    }

    if joint.index_a != NULL_INDEX {
        states.set(joint.index_a as usize, state_a);
    }
    if joint.index_b != NULL_INDEX {
        states.set(joint.index_b as usize, state_b);
    }
}

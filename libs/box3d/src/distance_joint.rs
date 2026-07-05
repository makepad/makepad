// Port of box3d/src/distance_joint.c
//
// Notes:
// - b3DefaultDistanceJointDef / b3CreateDistanceJoint live in joint.c (ported in joint.rs).
// - b3DrawDistanceJoint (debug draw) is not ported.
// - Recording hooks (B3_REC) are not ported.
// - Body states are copied to locals and written back on exit (see PORTING.md);
//   the C dynamicFlag guards are applied to the locals so the write-back is exact.

use crate::b3_assert;
use crate::body::{DYNAMIC_FLAG, IDENTITY_BODY_STATE};
use crate::constants::{huge, linear_slop};
use crate::core::NULL_INDEX;
use crate::id::JointId;
use crate::joint::{DistanceJoint, JointSim, JointUnion};
use crate::math_functions::{
    add, clamp_float, cross, dot, length, max_float, min_float, mul_add, mul_mv, mul_sub, mul_sv,
    normalize, rotate_vector, sub, sub_pos, transform_world_point, Vec3,
};
use crate::physics_world::{World, AWAKE_SET};
use crate::solver::{make_soft, StateAccess, StepContext};
use crate::types::JointType;

#[inline]
fn distance_joint_mut(base: &mut JointSim) -> &mut DistanceJoint {
    match &mut base.joint {
        JointUnion::Distance(j) => j,
        _ => panic!("wrong joint type"),
    }
}

#[inline]
fn distance_joint_ref(base: &JointSim) -> &DistanceJoint {
    match &base.joint {
        JointUnion::Distance(j) => j,
        _ => panic!("wrong joint type"),
    }
}

pub fn distance_joint_set_length(world: &mut World, joint_id: JointId, length: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::DistanceJointSetLength, |b| {
        b.w_jointid(joint_id);
        b.w_f32(length);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    let joint = distance_joint_mut(base);

    joint.length = clamp_float(length, linear_slop(), huge());
    joint.impulse = 0.0;
    joint.lower_impulse = 0.0;
    joint.upper_impulse = 0.0;
}

pub fn distance_joint_get_length(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    distance_joint_ref(base).length
}

pub fn distance_joint_enable_limit(world: &mut World, joint_id: JointId, enable_limit: bool) {

    crate::recording::rec_op(world, crate::recording::RecOp::DistanceJointEnableLimit, |b| {
        b.w_jointid(joint_id);
        b.w_bool(enable_limit);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    distance_joint_mut(base).enable_limit = enable_limit;
}

pub fn distance_joint_is_limit_enabled(world: &mut World, joint_id: JointId) -> bool {
    let joint = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    distance_joint_ref(joint).enable_limit
}

pub fn distance_joint_set_length_range(world: &mut World, joint_id: JointId, min_length: f32, max_length: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::DistanceJointSetLengthRange, |b| {
        b.w_jointid(joint_id);
        b.w_f32(min_length);
        b.w_f32(max_length);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    let joint = distance_joint_mut(base);

    let min_length = clamp_float(min_length, linear_slop(), huge());
    let max_length = clamp_float(max_length, linear_slop(), huge());
    joint.min_length = min_float(min_length, max_length);
    joint.max_length = max_float(min_length, max_length);
    joint.impulse = 0.0;
    joint.lower_impulse = 0.0;
    joint.upper_impulse = 0.0;
}

pub fn distance_joint_get_min_length(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    distance_joint_ref(base).min_length
}

pub fn distance_joint_get_max_length(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    distance_joint_ref(base).max_length
}

pub fn distance_joint_get_current_length(world: &mut World, joint_id: JointId) -> f32 {
    let (body_id_a, body_id_b, local_a, local_b) = {
        let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
        (base.body_id_a, base.body_id_b, base.local_frame_a.p, base.local_frame_b.p)
    };

    // C: b3GetUnlockedWorld returns NULL when locked and the function returns 0
    if world.locked {
        return 0.0;
    }

    let transform_a = crate::body::get_body_transform(world, body_id_a);
    let transform_b = crate::body::get_body_transform(world, body_id_b);

    let p_a = transform_world_point(transform_a, local_a);
    let p_b = transform_world_point(transform_b, local_b);
    let d = sub_pos(p_b, p_a);
    length(d)
}

pub fn distance_joint_enable_spring(world: &mut World, joint_id: JointId, enable_spring: bool) {

    crate::recording::rec_op(world, crate::recording::RecOp::DistanceJointEnableSpring, |b| {
        b.w_jointid(joint_id);
        b.w_bool(enable_spring);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    distance_joint_mut(base).enable_spring = enable_spring;
}

pub fn distance_joint_is_spring_enabled(world: &mut World, joint_id: JointId) -> bool {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    distance_joint_ref(base).enable_spring
}

pub fn distance_joint_set_spring_force_range(world: &mut World, joint_id: JointId, lower_force: f32, upper_force: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::DistanceJointSetSpringForceRange, |b| {
        b.w_jointid(joint_id);
        b.w_f32(lower_force);
        b.w_f32(upper_force);
    });
    b3_assert!(lower_force <= upper_force);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    let joint = distance_joint_mut(base);
    joint.lower_spring_force = lower_force;
    joint.upper_spring_force = upper_force;
}

pub fn distance_joint_get_spring_force_range(
    world: &mut World,
    joint_id: JointId,
    lower_force: &mut f32,
    upper_force: &mut f32,
) {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    let joint = distance_joint_ref(base);
    *lower_force = joint.lower_spring_force;
    *upper_force = joint.upper_spring_force;
}

pub fn distance_joint_set_spring_hertz(world: &mut World, joint_id: JointId, hertz: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::DistanceJointSetSpringHertz, |b| {
        b.w_jointid(joint_id);
        b.w_f32(hertz);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    distance_joint_mut(base).hertz = hertz;
}

pub fn distance_joint_set_spring_damping_ratio(world: &mut World, joint_id: JointId, damping_ratio: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::DistanceJointSetSpringDampingRatio, |b| {
        b.w_jointid(joint_id);
        b.w_f32(damping_ratio);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    distance_joint_mut(base).damping_ratio = damping_ratio;
}

pub fn distance_joint_get_spring_hertz(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    distance_joint_ref(base).hertz
}

pub fn distance_joint_get_spring_damping_ratio(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    distance_joint_ref(base).damping_ratio
}

pub fn distance_joint_enable_motor(world: &mut World, joint_id: JointId, enable_motor: bool) {

    crate::recording::rec_op(world, crate::recording::RecOp::DistanceJointEnableMotor, |b| {
        b.w_jointid(joint_id);
        b.w_bool(enable_motor);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    let joint = distance_joint_mut(base);
    if enable_motor != joint.enable_motor {
        joint.enable_motor = enable_motor;
        joint.motor_impulse = 0.0;
    }
}

pub fn distance_joint_is_motor_enabled(world: &mut World, joint_id: JointId) -> bool {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    distance_joint_ref(base).enable_motor
}

pub fn distance_joint_set_motor_speed(world: &mut World, joint_id: JointId, motor_speed: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::DistanceJointSetMotorSpeed, |b| {
        b.w_jointid(joint_id);
        b.w_f32(motor_speed);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    distance_joint_mut(base).motor_speed = motor_speed;
}

pub fn distance_joint_get_motor_speed(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    distance_joint_ref(base).motor_speed
}

pub fn distance_joint_get_motor_force(world: &mut World, joint_id: JointId) -> f32 {
    let inv_h = world.inv_h;
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    inv_h * distance_joint_ref(base).motor_impulse
}

pub fn distance_joint_set_max_motor_force(world: &mut World, joint_id: JointId, force: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::DistanceJointSetMaxMotorForce, |b| {
        b.w_jointid(joint_id);
        b.w_f32(force);
    });
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    distance_joint_mut(base).max_motor_force = force;
}

pub fn distance_joint_get_max_motor_force(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Distance);
    distance_joint_ref(base).max_motor_force
}

pub fn get_distance_joint_force(world: &World, base: &JointSim) -> Vec3 {
    let joint = distance_joint_ref(base);

    let transform_a = crate::body::get_body_transform(world, base.body_id_a);
    let transform_b = crate::body::get_body_transform(world, base.body_id_b);

    let p_a = transform_world_point(transform_a, base.local_frame_a.p);
    let p_b = transform_world_point(transform_b, base.local_frame_b.p);
    let d = sub_pos(p_b, p_a);
    let axis = normalize(d);
    let force = (joint.impulse + joint.lower_impulse - joint.upper_impulse + joint.motor_impulse) * world.inv_h;
    mul_sv(force, axis)
}

// 1-D constrained system
// m (v2 - v1) = lambda
// v2 + (beta/h) * x1 + gamma * lambda = 0, gamma has units of inverse mass.
// x2 = x1 + h * v2

// 1-D mass-damper-spring system
// m (v2 - v1) + h * d * v2 + h * k *

// C = norm(p2 - p1) - L
// u = (p2 - p1) / norm(p2 - p1)
// Cdot = dot(u, v2 + cross(w2, r2) - v1 - cross(w1, r1))
// J = [-u -cross(r1, u) u cross(r2, u)]
// K = J * invM * JT
//   = invMass1 + invI1 * cross(r1, u)^2 + invMass2 + invI2 * cross(r2, u)^2

pub fn prepare_distance_joint(base: &mut JointSim, world: &World, context: &StepContext) {
    b3_assert!(base.joint_type == JointType::Distance);

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

    let m_a = body_sim_a.inv_mass;
    let i_a = body_sim_a.inv_inertia_world;
    let m_b = body_sim_b.inv_mass;
    let i_b = body_sim_b.inv_inertia_world;

    base.inv_mass_a = m_a;
    base.inv_mass_b = m_b;
    base.inv_i_a = i_a;
    base.inv_i_b = i_b;

    let set_index_a = body_a.set_index;
    let set_index_b = body_b.set_index;
    let local_frame_a_p = base.local_frame_a.p;
    let local_frame_b_p = base.local_frame_b.p;
    let joint = distance_joint_mut(base);

    joint.index_a = if set_index_a == AWAKE_SET { local_index_a } else { NULL_INDEX };
    joint.index_b = if set_index_b == AWAKE_SET { local_index_b } else { NULL_INDEX };

    // initial anchors in world space
    joint.anchor_a = rotate_vector(body_sim_a.transform.q, sub(local_frame_a_p, body_sim_a.local_center));
    joint.anchor_b = rotate_vector(body_sim_b.transform.q, sub(local_frame_b_p, body_sim_b.local_center));
    joint.delta_center = sub_pos(body_sim_b.center, body_sim_a.center);

    let r_a = joint.anchor_a;
    let r_b = joint.anchor_b;
    let separation = add(sub(r_b, r_a), joint.delta_center);
    let axis = normalize(separation);

    // compute effective mass
    let cr_a = cross(r_a, axis);
    let cr_b = cross(r_b, axis);
    let k = m_a + m_b + dot(cr_a, mul_mv(i_a, cr_a)) + dot(cr_b, mul_mv(i_b, cr_b));
    joint.axial_mass = if k > 0.0 { 1.0 / k } else { 0.0 };

    joint.distance_softness = make_soft(joint.hertz, joint.damping_ratio, context.h);

    if !context.enable_warm_starting {
        joint.impulse = 0.0;
        joint.lower_impulse = 0.0;
        joint.upper_impulse = 0.0;
        joint.motor_impulse = 0.0;
    }
}

pub fn warm_start_distance_joint(base: &mut JointSim, states: &StateAccess, _context: &StepContext) {
    b3_assert!(base.joint_type == JointType::Distance);

    let m_a = base.inv_mass_a;
    let m_b = base.inv_mass_b;
    let i_a = base.inv_i_a;
    let i_b = base.inv_i_b;

    let joint = distance_joint_mut(base);

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

    let r_a = rotate_vector(state_a.delta_rotation, joint.anchor_a);
    let r_b = rotate_vector(state_b.delta_rotation, joint.anchor_b);

    let ds = add(sub(state_b.delta_position, state_a.delta_position), sub(r_b, r_a));
    let separation = add(joint.delta_center, ds);
    let axis = normalize(separation);

    let axial_impulse = joint.impulse + joint.lower_impulse - joint.upper_impulse + joint.motor_impulse;
    let p = mul_sv(axial_impulse, axis);

    if state_a.flags & DYNAMIC_FLAG != 0 {
        state_a.linear_velocity = mul_sub(state_a.linear_velocity, m_a, p);
        state_a.angular_velocity = sub(state_a.angular_velocity, mul_mv(i_a, cross(r_a, p)));
    }

    if state_b.flags & DYNAMIC_FLAG != 0 {
        state_b.linear_velocity = mul_add(state_b.linear_velocity, m_b, p);
        state_b.angular_velocity = add(state_b.angular_velocity, mul_mv(i_b, cross(r_b, p)));
    }

    if joint.index_a != NULL_INDEX {
        states.set(joint.index_a as usize, state_a);
    }
    if joint.index_b != NULL_INDEX {
        states.set(joint.index_b as usize, state_b);
    }
}

pub fn solve_distance_joint(base: &mut JointSim, states: &StateAccess, context: &StepContext, use_bias: bool) {
    b3_assert!(base.joint_type == JointType::Distance);

    let m_a = base.inv_mass_a;
    let m_b = base.inv_mass_b;
    let i_a = base.inv_i_a;
    let i_b = base.inv_i_b;
    let constraint_softness = base.constraint_softness;

    let joint = distance_joint_mut(base);

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

    // current anchors
    let r_a = rotate_vector(state_a.delta_rotation, joint.anchor_a);
    let r_b = rotate_vector(state_b.delta_rotation, joint.anchor_b);

    // current separation
    let ds = add(sub(state_b.delta_position, state_a.delta_position), sub(r_b, r_a));
    let separation = add(joint.delta_center, ds);

    let length = length(separation);
    let axis = normalize(separation);

    // joint is soft if
    // - spring is enabled
    // - and (joint limit is disabled or limits are not equal)
    if joint.enable_spring && (joint.min_length < joint.max_length || !joint.enable_limit) {
        // spring
        if joint.hertz > 0.0 {
            // Cdot = dot(u, v + cross(w, r))
            let vr = add(sub(v_b, v_a), sub(cross(w_b, r_b), cross(w_a, r_a)));
            let cdot = dot(axis, vr);
            let c = length - joint.length;
            let bias = joint.distance_softness.bias_rate * c;

            let m = joint.distance_softness.mass_scale * joint.axial_mass;
            let old_impulse = joint.impulse;
            let mut impulse = (-joint.distance_softness.impulse_scale).mul_add(old_impulse, -m * (cdot + bias));
            let h = context.h;
            joint.impulse =
                clamp_float(joint.impulse + impulse, joint.lower_spring_force * h, joint.upper_spring_force * h);
            impulse = joint.impulse - old_impulse;

            let p = mul_sv(impulse, axis);
            v_a = mul_sub(v_a, m_a, p);
            w_a = sub(w_a, mul_mv(i_a, cross(r_a, p)));
            v_b = mul_add(v_b, m_b, p);
            w_b = add(w_b, mul_mv(i_b, cross(r_b, p)));
        }

        if joint.enable_limit {
            // lower limit
            {
                let vr = add(sub(v_b, v_a), sub(cross(w_b, r_b), cross(w_a, r_a)));
                let cdot = dot(axis, vr);

                let c = length - joint.min_length;

                let mut bias = 0.0;
                let mut mass_coeff = 1.0;
                let mut impulse_coeff = 0.0;
                if c > 0.0 {
                    // speculative
                    bias = c * context.inv_h;
                } else if use_bias {
                    bias = constraint_softness.bias_rate * c;
                    mass_coeff = constraint_softness.mass_scale;
                    impulse_coeff = constraint_softness.impulse_scale;
                }

                let mut impulse = (-impulse_coeff).mul_add(joint.lower_impulse, -mass_coeff * joint.axial_mass * (cdot + bias));
                let new_impulse = max_float(0.0, joint.lower_impulse + impulse);
                impulse = new_impulse - joint.lower_impulse;
                joint.lower_impulse = new_impulse;

                let p = mul_sv(impulse, axis);
                v_a = mul_sub(v_a, m_a, p);
                w_a = sub(w_a, mul_mv(i_a, cross(r_a, p)));
                v_b = mul_add(v_b, m_b, p);
                w_b = add(w_b, mul_mv(i_b, cross(r_b, p)));
            }

            // upper
            {
                let vr = add(sub(v_a, v_b), sub(cross(w_a, r_a), cross(w_b, r_b)));
                let cdot = dot(axis, vr);

                let c = joint.max_length - length;

                let mut bias = 0.0;
                let mut mass_scale = 1.0;
                let mut impulse_scale = 0.0;
                if c > 0.0 {
                    // speculative
                    bias = c * context.inv_h;
                } else if use_bias {
                    bias = constraint_softness.bias_rate * c;
                    mass_scale = constraint_softness.mass_scale;
                    impulse_scale = constraint_softness.impulse_scale;
                }

                let mut impulse = (-impulse_scale).mul_add(joint.upper_impulse, -mass_scale * joint.axial_mass * (cdot + bias));
                let new_impulse = max_float(0.0, joint.upper_impulse + impulse);
                impulse = new_impulse - joint.upper_impulse;
                joint.upper_impulse = new_impulse;

                let p = mul_sv(-impulse, axis);
                v_a = mul_sub(v_a, m_a, p);
                w_a = sub(w_a, mul_mv(i_a, cross(r_a, p)));
                v_b = mul_add(v_b, m_b, p);
                w_b = add(w_b, mul_mv(i_b, cross(r_b, p)));
            }
        }

        if joint.enable_motor {
            let vr = add(sub(v_b, v_a), sub(cross(w_b, r_b), cross(w_a, r_a)));
            let cdot = dot(axis, vr);
            let mut impulse = joint.axial_mass * (joint.motor_speed - cdot);
            let old_impulse = joint.motor_impulse;
            let max_impulse = context.h * joint.max_motor_force;
            joint.motor_impulse = clamp_float(joint.motor_impulse + impulse, -max_impulse, max_impulse);
            impulse = joint.motor_impulse - old_impulse;

            let p = mul_sv(impulse, axis);
            v_a = mul_sub(v_a, m_a, p);
            w_a = sub(w_a, mul_mv(i_a, cross(r_a, p)));
            v_b = mul_add(v_b, m_b, p);
            w_b = add(w_b, mul_mv(i_b, cross(r_b, p)));
        }
    } else {
        // rigid constraint
        let vr = add(sub(v_b, v_a), sub(cross(w_b, r_b), cross(w_a, r_a)));
        let cdot = dot(axis, vr);

        let c = length - joint.length;

        let mut bias = 0.0;
        let mut mass_scale = 1.0;
        let mut impulse_scale = 0.0;
        if use_bias {
            bias = constraint_softness.bias_rate * c;
            mass_scale = constraint_softness.mass_scale;
            impulse_scale = constraint_softness.impulse_scale;
        }

        let impulse = (-impulse_scale).mul_add(joint.impulse, -mass_scale * joint.axial_mass * (cdot + bias));
        joint.impulse += impulse;

        let p = mul_sv(impulse, axis);
        v_a = mul_sub(v_a, m_a, p);
        w_a = sub(w_a, mul_mv(i_a, cross(r_a, p)));
        v_b = mul_add(v_b, m_b, p);
        w_b = add(w_b, mul_mv(i_b, cross(r_b, p)));
    }

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

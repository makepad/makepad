// Port of box3d/src/weld_joint.c
//
// Deviations: recording hooks (B3_REC) and b3DrawWeldJoint are not ported.

use crate::b3_assert;
use crate::body::{BodyState, DYNAMIC_FLAG, IDENTITY_BODY_STATE};
use crate::core::NULL_INDEX;
use crate::id::JointId;
use crate::joint::{JointSim, JointUnion};
use crate::math_functions::*;
use crate::math_internal::{delta_quat_to_rotation, skew};
use crate::physics_world::{World, AWAKE_SET};
use crate::solver::{make_soft, StepContext};
use crate::types::JointType;

fn get_weld(base: &mut JointSim) -> &mut crate::joint::WeldJoint {
    match &mut base.joint {
        JointUnion::Weld(j) => j,
        _ => panic!("wrong joint type"),
    }
}

fn get_weld_ref(base: &JointSim) -> &crate::joint::WeldJoint {
    match &base.joint {
        JointUnion::Weld(j) => j,
        _ => panic!("wrong joint type"),
    }
}

pub fn weld_joint_set_linear_hertz(world: &mut World, joint_id: JointId, hertz: f32) {
    b3_assert!(is_valid_float(hertz) && hertz >= 0.0);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Weld);
    get_weld(base).linear_hertz = hertz;
}

pub fn weld_joint_get_linear_hertz(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Weld);
    get_weld(base).linear_hertz
}

pub fn weld_joint_set_linear_damping_ratio(world: &mut World, joint_id: JointId, damping_ratio: f32) {
    b3_assert!(is_valid_float(damping_ratio) && damping_ratio >= 0.0);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Weld);
    get_weld(base).linear_damping_ratio = damping_ratio;
}

pub fn weld_joint_get_linear_damping_ratio(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Weld);
    get_weld(base).linear_damping_ratio
}

pub fn weld_joint_set_angular_hertz(world: &mut World, joint_id: JointId, hertz: f32) {
    b3_assert!(is_valid_float(hertz) && hertz >= 0.0);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Weld);
    get_weld(base).angular_hertz = hertz;
}

pub fn weld_joint_get_angular_hertz(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Weld);
    get_weld(base).angular_hertz
}

pub fn weld_joint_set_angular_damping_ratio(world: &mut World, joint_id: JointId, damping_ratio: f32) {
    b3_assert!(is_valid_float(damping_ratio) && damping_ratio >= 0.0);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Weld);
    get_weld(base).angular_damping_ratio = damping_ratio;
}

pub fn weld_joint_get_angular_damping_ratio(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Weld);
    get_weld(base).angular_damping_ratio
}

pub fn get_weld_joint_force(world: &World, base: &JointSim) -> Vec3 {
    mul_sv(world.inv_h, get_weld_ref(base).linear_impulse)
}

pub fn get_weld_joint_torque(world: &World, base: &JointSim) -> Vec3 {
    mul_sv(world.inv_h, get_weld_ref(base).angular_impulse)
}

pub fn prepare_weld_joint(base: &mut JointSim, world: &World, context: &StepContext) {
    b3_assert!(base.joint_type == JointType::Weld);

    let body_a = &world.bodies[base.body_id_a as usize];
    let body_b = &world.bodies[base.body_id_b as usize];

    b3_assert!(body_b.set_index == AWAKE_SET);

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
    let constraint_softness = base.constraint_softness;

    let joint = get_weld(base);
    joint.index_a = if set_index_a == AWAKE_SET { local_index_a } else { NULL_INDEX };
    joint.index_b = if set_index_b == AWAKE_SET { local_index_b } else { NULL_INDEX };

    // Compute joint anchor frames with world space rotation, relative to center of mass
    joint.frame_a.q = mul_quat(body_sim_a.transform.q, local_frame_a.q);
    joint.frame_a.p = rotate_vector(body_sim_a.transform.q, sub(local_frame_a.p, body_sim_a.local_center));
    joint.frame_b.q = mul_quat(body_sim_b.transform.q, local_frame_b.q);
    joint.frame_b.p = rotate_vector(body_sim_b.transform.q, sub(local_frame_b.p, body_sim_b.local_center));

    joint.delta_center = sub_pos(body_sim_b.center, body_sim_a.center);
    joint.angular_mass = invert_matrix(inv_inertia_sum);

    if joint.linear_hertz == 0.0 {
        joint.linear_spring = constraint_softness;
    } else {
        joint.linear_spring = make_soft(joint.linear_hertz, joint.linear_damping_ratio, context.h);
    }

    if joint.angular_hertz == 0.0 {
        joint.angular_spring = constraint_softness;
    } else {
        joint.angular_spring = make_soft(joint.angular_hertz, joint.angular_damping_ratio, context.h);
    }

    if !context.enable_warm_starting {
        joint.linear_impulse = Vec3::ZERO;
        joint.angular_impulse = Vec3::ZERO;
    }
}

pub fn warm_start_weld_joint(base: &mut JointSim, context: &mut StepContext) {
    b3_assert!(base.joint_type == JointType::Weld);

    let m_a = base.inv_mass_a;
    let m_b = base.inv_mass_b;
    let i_a = base.inv_i_a;
    let i_b = base.inv_i_b;

    let joint = get_weld(base);

    // dummy state for static bodies
    let index_a = joint.index_a;
    let index_b = joint.index_b;
    let mut state_a: BodyState =
        if index_a == NULL_INDEX { IDENTITY_BODY_STATE } else { context.states[index_a as usize] };
    let mut state_b: BodyState =
        if index_b == NULL_INDEX { IDENTITY_BODY_STATE } else { context.states[index_b as usize] };

    let mut v_a = state_a.linear_velocity;
    let mut w_a = state_a.angular_velocity;
    let mut v_b = state_b.linear_velocity;
    let mut w_b = state_b.angular_velocity;

    let r_a = rotate_vector(state_a.delta_rotation, joint.frame_a.p);
    let r_b = rotate_vector(state_b.delta_rotation, joint.frame_b.p);

    v_a = mul_sub(v_a, m_a, joint.linear_impulse);
    w_a = sub(w_a, mul_mv(i_a, add(cross(r_a, joint.linear_impulse), joint.angular_impulse)));

    v_b = mul_add(v_b, m_b, joint.linear_impulse);
    w_b = add(w_b, mul_mv(i_b, add(cross(r_b, joint.linear_impulse), joint.angular_impulse)));

    if state_a.flags & DYNAMIC_FLAG != 0 {
        state_a.linear_velocity = v_a;
        state_a.angular_velocity = w_a;
    }

    if state_b.flags & DYNAMIC_FLAG != 0 {
        state_b.linear_velocity = v_b;
        state_b.angular_velocity = w_b;
    }

    if index_a != NULL_INDEX {
        context.states[index_a as usize] = state_a;
    }
    if index_b != NULL_INDEX {
        context.states[index_b as usize] = state_b;
    }
}

pub fn solve_weld_joint(base: &mut JointSim, context: &mut StepContext, use_bias: bool) {
    let m_a = base.inv_mass_a;
    let m_b = base.inv_mass_b;
    let i_a = base.inv_i_a;
    let i_b = base.inv_i_b;
    let inv_i_a = base.inv_i_a;
    let inv_i_b = base.inv_i_b;
    let fixed_rotation = base.fixed_rotation;

    let joint = get_weld(base);

    // dummy state for static bodies
    let index_a = joint.index_a;
    let index_b = joint.index_b;
    let mut state_a: BodyState =
        if index_a == NULL_INDEX { IDENTITY_BODY_STATE } else { context.states[index_a as usize] };
    let mut state_b: BodyState =
        if index_b == NULL_INDEX { IDENTITY_BODY_STATE } else { context.states[index_b as usize] };

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

    // angular constraint
    if !fixed_rotation {
        let mut bias = Vec3::ZERO;
        let mut mass_scale = 1.0;
        let mut impulse_scale = 0.0;
        if use_bias || joint.angular_hertz > 0.0 {
            let target_quat = Quat::IDENTITY;
            let delta_rotation = delta_quat_to_rotation(rel_q, target_quat);
            let c = neg(rotate_vector(quat_a, delta_rotation));

            bias = mul_sv(joint.angular_spring.bias_rate, c);
            mass_scale = joint.angular_spring.mass_scale;
            impulse_scale = joint.angular_spring.impulse_scale;
        }

        let cdot = sub(w_b, w_a);
        let impulse = mul_sub(
            mul_sv(-mass_scale, mul_mv(joint.angular_mass, add(cdot, bias))),
            impulse_scale,
            joint.angular_impulse,
        );
        joint.angular_impulse = add(joint.angular_impulse, impulse);

        w_a = sub(w_a, mul_mv(i_a, impulse));
        w_b = add(w_b, mul_mv(i_b, impulse));
    }

    // linear constraint
    {
        let r_a = rotate_vector(state_a.delta_rotation, joint.frame_a.p);
        let r_b = rotate_vector(state_b.delta_rotation, joint.frame_b.p);

        let cdot = sub(add(v_b, cross(w_b, r_b)), add(v_a, cross(w_a, r_a)));

        let mut bias = Vec3::ZERO;
        let mut mass_scale = 1.0;
        let mut impulse_scale = 0.0;
        if use_bias || joint.linear_hertz > 0.0 {
            let dc_a = state_a.delta_position;
            let dc_b = state_b.delta_position;

            let separation = add(add(sub(dc_b, dc_a), sub(r_b, r_a)), joint.delta_center);

            bias = mul_sv(joint.linear_spring.bias_rate, separation);
            mass_scale = joint.linear_spring.mass_scale;
            impulse_scale = joint.linear_spring.impulse_scale;
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

    if state_a.flags & DYNAMIC_FLAG != 0 {
        state_a.linear_velocity = v_a;
        state_a.angular_velocity = w_a;
    }

    if state_b.flags & DYNAMIC_FLAG != 0 {
        state_b.linear_velocity = v_b;
        state_b.angular_velocity = w_b;
    }

    if index_a != NULL_INDEX {
        context.states[index_a as usize] = state_a;
    }
    if index_b != NULL_INDEX {
        context.states[index_b as usize] = state_b;
    }
}

// Port of box3d/src/parallel_joint.c
//
// Notes:
// - b3DefaultParallelJointDef / b3CreateParallelJoint live in joint.c (ported in joint.rs).
// - b3DrawParallelJoint (debug draw) is not ported.
// - Recording hooks (B3_REC) are not ported.
// - b3GetParallelJointTorque mutates the joint sim (refreshes perp axes) like the C
//   version, so it takes &mut JointSim.
// - Body states are copied to locals and written back on exit; C dynamicFlag
//   guards preserved on the locals.

use crate::b3_assert;
use crate::body::{DYNAMIC_FLAG, IDENTITY_BODY_STATE};
use crate::core::NULL_INDEX;
use crate::id::JointId;
use crate::joint::{JointSim, JointUnion, ParallelJoint};
use crate::math_functions::{
    add, add_mm, blend2, cross, det, dot, dot_quat, inv_mul_quat, is_valid_float, mul_mv, mul_quat,
    mul_sv, negate_quat, rotate_vector, sub, vec2, Vec3,
};
use crate::math_internal::{length2, length_squared2, solve2, sub2, Matrix2};
use crate::physics_world::{World, AWAKE_SET};
use crate::solver::{make_soft, StepContext};
use crate::types::JointType;

#[inline]
fn parallel_joint_mut(base: &mut JointSim) -> &mut ParallelJoint {
    match &mut base.joint {
        JointUnion::Parallel(j) => j,
        _ => panic!("wrong joint type"),
    }
}

#[inline]
fn parallel_joint_ref(base: &JointSim) -> &ParallelJoint {
    match &base.joint {
        JointUnion::Parallel(j) => j,
        _ => panic!("wrong joint type"),
    }
}

pub fn parallel_joint_set_spring_hertz(world: &mut World, joint_id: JointId, hertz: f32) {
    b3_assert!(is_valid_float(hertz) && hertz >= 0.0);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Parallel);
    parallel_joint_mut(base).hertz = hertz;
}

pub fn parallel_joint_get_spring_hertz(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Parallel);
    parallel_joint_ref(base).hertz
}

pub fn parallel_joint_set_spring_damping_ratio(world: &mut World, joint_id: JointId, damping_ratio: f32) {
    b3_assert!(is_valid_float(damping_ratio) && damping_ratio >= 0.0);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Parallel);
    parallel_joint_mut(base).damping_ratio = damping_ratio;
}

pub fn parallel_joint_get_spring_damping_ratio(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Parallel);
    parallel_joint_ref(base).damping_ratio
}

pub fn parallel_joint_set_max_torque(world: &mut World, joint_id: JointId, max_force: f32) {
    b3_assert!(is_valid_float(max_force) && max_force >= 0.0);
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Parallel);
    parallel_joint_mut(base).max_torque = max_force;
}

pub fn parallel_joint_get_max_torque(world: &mut World, joint_id: JointId) -> f32 {
    let base = crate::joint::get_joint_sim_check_type(world, joint_id, JointType::Parallel);
    parallel_joint_ref(base).max_torque
}

/// Deviation: C refreshes joint->perpAxisX/Y in place here. Those fields are
/// always rewritten by prepare before the next reader (warm start), so the port
/// computes them into locals and leaves the sim untouched, letting this take
/// &JointSim to match the joint.rs reaction dispatch.
pub fn get_parallel_joint_torque(world: &World, base: &JointSim) -> Vec3 {
    let joint = parallel_joint_ref(base);

    let rel_q = inv_mul_quat(joint.quat_a, joint.quat_b);
    let perp_axis_x = mul_sv(
        0.5,
        rotate_vector(joint.quat_a, add(mul_sv(rel_q.s, Vec3::AXIS_X), cross(rel_q.v, Vec3::AXIS_X))),
    );
    let perp_axis_y = mul_sv(
        0.5,
        rotate_vector(joint.quat_a, add(mul_sv(rel_q.s, Vec3::AXIS_Y), cross(rel_q.v, Vec3::AXIS_Y))),
    );

    let angular_impulse = blend2(joint.perp_impulse.x, perp_axis_x, joint.perp_impulse.y, perp_axis_y);
    mul_sv(world.inv_h, angular_impulse)
}

pub fn prepare_parallel_joint(base: &mut JointSim, world: &World, context: &StepContext) {
    b3_assert!(base.joint_type == JointType::Parallel);

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
    let local_frame_a_q = base.local_frame_a.q;
    let local_frame_b_q = base.local_frame_b.q;
    let joint = parallel_joint_mut(base);
    joint.index_a = if set_index_a == AWAKE_SET { local_index_a } else { NULL_INDEX };
    joint.index_b = if set_index_b == AWAKE_SET { local_index_b } else { NULL_INDEX };

    // Compute joint anchor frames with world space rotation, relative to center of mass
    joint.quat_a = mul_quat(body_sim_a.transform.q, local_frame_a_q);
    joint.quat_b = mul_quat(body_sim_b.transform.q, local_frame_b_q);

    let rel_q = inv_mul_quat(joint.quat_a, joint.quat_b);

    {
        // These are needed for warm starting
        joint.perp_axis_x = mul_sv(
            0.5,
            rotate_vector(joint.quat_a, add(mul_sv(rel_q.s, Vec3::AXIS_X), cross(rel_q.v, Vec3::AXIS_X))),
        );
        joint.perp_axis_y = mul_sv(
            0.5,
            rotate_vector(joint.quat_a, add(mul_sv(rel_q.s, Vec3::AXIS_Y), cross(rel_q.v, Vec3::AXIS_Y))),
        );
    }

    joint.softness = make_soft(joint.hertz, joint.damping_ratio, context.h);

    if !context.enable_warm_starting {
        joint.perp_impulse = vec2(0.0, 0.0);
    }
}

pub fn warm_start_parallel_joint(base: &mut JointSim, context: &mut StepContext) {
    b3_assert!(base.joint_type == JointType::Parallel);

    let i_a = base.inv_i_a;
    let i_b = base.inv_i_b;

    let joint = parallel_joint_mut(base);

    // dummy state for static bodies
    let mut state_a = if joint.index_a == NULL_INDEX {
        IDENTITY_BODY_STATE
    } else {
        context.states[joint.index_a as usize]
    };
    let mut state_b = if joint.index_b == NULL_INDEX {
        IDENTITY_BODY_STATE
    } else {
        context.states[joint.index_b as usize]
    };

    let mut w_a = state_a.angular_velocity;
    let mut w_b = state_b.angular_velocity;

    let angular_impulse = blend2(joint.perp_impulse.x, joint.perp_axis_x, joint.perp_impulse.y, joint.perp_axis_y);

    w_a = sub(w_a, mul_mv(i_a, angular_impulse));
    w_b = add(w_b, mul_mv(i_b, angular_impulse));

    if state_a.flags & DYNAMIC_FLAG != 0 {
        state_a.angular_velocity = w_a;
    }

    if state_b.flags & DYNAMIC_FLAG != 0 {
        state_b.angular_velocity = w_b;
    }

    if joint.index_a != NULL_INDEX {
        context.states[joint.index_a as usize] = state_a;
    }
    if joint.index_b != NULL_INDEX {
        context.states[joint.index_b as usize] = state_b;
    }
}

pub fn solve_parallel_joint(base: &mut JointSim, context: &mut StepContext) {
    let i_a = base.inv_i_a;
    let i_b = base.inv_i_b;
    let fixed_rotation = base.fixed_rotation;

    let joint = parallel_joint_mut(base);

    // dummy state for static bodies
    let mut state_a = if joint.index_a == NULL_INDEX {
        IDENTITY_BODY_STATE
    } else {
        context.states[joint.index_a as usize]
    };
    let mut state_b = if joint.index_b == NULL_INDEX {
        IDENTITY_BODY_STATE
    } else {
        context.states[joint.index_b as usize]
    };

    let mut w_a = state_a.angular_velocity;
    let mut w_b = state_b.angular_velocity;

    let quat_a = mul_quat(state_a.delta_rotation, joint.quat_a);
    let mut quat_b = mul_quat(state_b.delta_rotation, joint.quat_b);

    if dot_quat(quat_a, quat_b) < 0.0 {
        // this keeps the rotation angle in the range [-pi, pi]
        quat_b = negate_quat(quat_b);
    }

    let rel_q = inv_mul_quat(quat_a, quat_b);

    if !fixed_rotation && joint.max_torque > 0.0 {
        let c = vec2(rel_q.v.x, rel_q.v.y);
        let bias = vec2(joint.softness.bias_rate * c.x, joint.softness.bias_rate * c.y);
        let mass_scale = joint.softness.mass_scale;
        let impulse_scale = joint.softness.impulse_scale;

        // Collinearity constraint as 2-by-2
        let perp_axis_x = mul_sv(
            0.5,
            rotate_vector(quat_a, add(mul_sv(rel_q.s, Vec3::AXIS_X), cross(rel_q.v, Vec3::AXIS_X))),
        );
        let perp_axis_y = mul_sv(
            0.5,
            rotate_vector(quat_a, add(mul_sv(rel_q.s, Vec3::AXIS_Y), cross(rel_q.v, Vec3::AXIS_Y))),
        );
        joint.perp_axis_x = perp_axis_x;
        joint.perp_axis_y = perp_axis_y;

        let inv_inertia_sum = add_mm(i_a, i_b);
        let kxx = dot(perp_axis_x, mul_mv(inv_inertia_sum, perp_axis_x));
        let kyy = dot(perp_axis_y, mul_mv(inv_inertia_sum, perp_axis_y));
        let kxy = dot(perp_axis_x, mul_mv(inv_inertia_sum, perp_axis_y));

        let k = Matrix2 { cx: vec2(kxx, kxy), cy: vec2(kxy, kyy) };

        let w_rel = sub(w_b, w_a);
        let cdot = vec2(dot(w_rel, perp_axis_x), dot(w_rel, perp_axis_y));

        let max_impulse = context.h * joint.max_torque;
        let old_impulse = joint.perp_impulse;
        let cdot_plus_bias = vec2(cdot.x + bias.x, cdot.y + bias.y);
        let sol = solve2(k, cdot_plus_bias);
        let mut delta_impulse = vec2(
            -mass_scale * sol.x - impulse_scale * old_impulse.x,
            -mass_scale * sol.y - impulse_scale * old_impulse.y,
        );
        joint.perp_impulse = vec2(old_impulse.x + delta_impulse.x, old_impulse.y + delta_impulse.y);
        if length_squared2(joint.perp_impulse) > max_impulse * max_impulse {
            let s = max_impulse / length2(joint.perp_impulse);
            joint.perp_impulse = vec2(s * joint.perp_impulse.x, s * joint.perp_impulse.y);
        }

        delta_impulse = sub2(joint.perp_impulse, old_impulse);

        let angular_impulse = blend2(delta_impulse.x, perp_axis_x, delta_impulse.y, perp_axis_y);
        w_a = sub(w_a, mul_mv(i_a, angular_impulse));
        w_b = add(w_b, mul_mv(i_b, angular_impulse));
    }

    if state_a.flags & DYNAMIC_FLAG != 0 {
        state_a.angular_velocity = w_a;
    }

    if state_b.flags & DYNAMIC_FLAG != 0 {
        state_b.angular_velocity = w_b;
    }

    if joint.index_a != NULL_INDEX {
        context.states[joint.index_a as usize] = state_a;
    }
    if joint.index_b != NULL_INDEX {
        context.states[joint.index_b as usize] = state_b;
    }
}

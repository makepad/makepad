// Port of box3d/src/contact_solver.h + contact_solver.c
//
// Pointer redesign: b3ContactConstraint's `b3ManifoldConstraint* constraints`
// becomes `constraint_start` (range into StepContext::manifold_constraints) and
// `struct b3Contact* contact` becomes `contact_id` (index into World::contacts).
//
// SIMD: this is the B3_SIMD_NONE path. FloatW is the C scalar-fallback struct of
// four floats; lanes hold four contacts. Per-lane float operation order matches
// the C scalar fallback exactly.
//
// Layout contract with the solver.c port (flat constraint arrays owned by
// StepContext, see solver.rs):
// - context.contact_constraints / context.manifold_constraints hold the
//   non-overflow (mesh) constraints FIRST, then the overflow constraints at the
//   tail. GraphColor.contact_constraint_start/manifold_constraint_start are flat
//   offsets into those arrays for every color, including the overflow color.
// - ContactSpec.manifold_start is a FLAT index into context.manifold_constraints
//   (for overflow specs too).
// - context.contact_prepare_spans covers the non-overflow range with flat
//   starts plus a sentinel; context.overflow_spans has starts RELATIVE to the
//   overflow range (0-based) plus a sentinel, matching the C overflow arrays.
// - Blocks: contactBlock/wideContactBlock start indices are flat;
//   overflowBlock and graph*Block start indices are relative to the
//   color's range (as in C).
// - context.wide_constraints elements must be initialized with
//   ContactConstraintWide::default() (the C memset-zero: null base-1 body
//   indices and null per-lane contacts) so remainder lanes are inert.

use crate::b3_assert;
use crate::b3_validate;
use crate::bitset::set_bit;
use crate::body::{BodyState, DYNAMIC_FLAG, IDENTITY_BODY_STATE};
use crate::constants::MAX_MANIFOLD_POINTS;
use crate::constraint_graph::OVERFLOW_INDEX;
use crate::contact::{CONTACT_STATIC_FLAG, SIM_ENABLE_HIT_EVENT};
use crate::core::NULL_INDEX;
use crate::math_functions::{
    add, add_mm, blend2, clamp_float, cross, distance, dot, invert_matrix, max_float, min_int, mul_add, mul_mv,
    mul_sub, mul_sv, neg, perp, rotate_vector, sub, vec2, Matrix3, Vec2, Vec3,
};
use crate::math_internal::{dot2, invert2, mul_mv2, sub2, Matrix2};
use crate::physics_world::World;
use crate::solver::{Softness, SolverBlock, SolverBlockType, StepContext};

// B3_SIMD_WIDTH
pub const SIMD_WIDTH: usize = 4;

#[derive(Clone, Copy, Debug, Default)]
pub struct ManifoldConstraintPoint {
    pub r_a: Vec3,
    pub r_b: Vec3,
    pub base_separation: f32,
    pub relative_velocity: f32,
    pub normal_impulse: f32,
    pub total_normal_impulse: f32,
    pub normal_mass: f32,
    pub lever_arm: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ManifoldConstraint {
    pub points: [ManifoldConstraintPoint; 4],
    pub point_count: i32,
    pub normal: Vec3,
    pub tangent1: Vec3,
    pub tangent2: Vec3,
    pub origin_a: Vec3,
    pub origin_b: Vec3,
    pub twist_mass: f32,
    pub twist_impulse: f32,
    pub tangent_mass: Matrix2,
    pub friction_impulse: Vec2,
    pub rolling_impulse: Vec3,
    pub tangent_velocity1: f32,
    pub tangent_velocity2: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ContactConstraint {
    /// Range start into StepContext::manifold_constraints (C: b3ManifoldConstraint*).
    pub constraint_start: i32,
    /// Index into World::contacts (C: b3Contact*).
    pub contact_id: i32,
    pub index_a: i32,
    pub index_b: i32,
    pub inv_mass_a: f32,
    pub inv_mass_b: f32,
    pub inv_i_a: Matrix3,
    pub inv_i_b: Matrix3,
    pub softness: Softness,
    pub rolling_mass: Matrix3,
    pub friction: f32,
    pub restitution: f32,
    pub rolling_resistance: f32,
    pub manifold_count: i32,
}

// ---------------------------------------------------------------------------
// contact_solver.c — scalar (mesh + overflow) stages
// ---------------------------------------------------------------------------

// contact separation for sub-stepping
// s = s0 + dot(cB + rB - cA - rA, normal)
// normal is held constant
// body positions c can translate and anchors r can rotate
// s(t) = s0 + dot(cB(t) + rB(t) - cA(t) - rA(t), normal)
// s_base = s0 + dot(cB0 - cA0, normal)

// Prepare mesh constraints
pub fn prepare_contacts_mesh(block: SolverBlock, world: &mut World, context: &mut StepContext) {
    let world = &*world;

    let warm_start_scale = if world.enable_warm_starting { 1.0 } else { 0.0 };

    let contact_softness = context.contact_softness;
    let static_softness = context.static_softness;

    let ctx = &mut *context;
    let body_sims = &ctx.sims;
    let body_states = &ctx.states;
    let manifold_constraints = &mut ctx.manifold_constraints;
    let contact_constraints = &mut ctx.contact_constraints;

    // Need to use spans in order to find the associated b3Contact, which is per color.
    // Overflow constraints are stored separately (at the tail of the flat arrays).
    let (spans, base_offset) = if block.block_type == SolverBlockType::Overflow {
        (
            &ctx.overflow_spans,
            world.constraint_graph.colors[OVERFLOW_INDEX as usize].contact_constraint_start,
        )
    } else {
        (&ctx.contact_prepare_spans, 0)
    };

    let mut index = block.start_index;
    let end_index = block.start_index + block.count as i32;

    // Find color for start index. Linear search but fast.
    let mut color_index = 0usize;
    while spans[color_index + 1].start <= index {
        color_index += 1;
    }

    // Loop over block
    while index < end_index {
        let color_start = spans[color_index].start;
        let color_end_index = min_int(spans[color_index + 1].start, end_index);
        let specs_color = spans[color_index].color_index as usize;

        // Loop over color
        while index < color_end_index {
            let specs = &world.constraint_graph.colors[specs_color].contacts;

            let local_index = index - color_start;
            b3_assert!(0 <= local_index && local_index < spans[color_index].count);
            let spec = specs[local_index as usize];
            let contact_id = spec.contact_id;
            let contact = &world.contacts[contact_id as usize];
            b3_assert!(contact.contact_id == contact_id);

            let index_a = contact.body_sim_index_a;
            let index_b = contact.body_sim_index_b;

            if cfg!(debug_assertions) {
                if index_a != NULL_INDEX {
                    let body_a = &world.bodies[contact.edges[0].body_id as usize];
                    b3_assert!(index_a == body_a.local_index);
                }

                if index_b != NULL_INDEX {
                    let body_b = &world.bodies[contact.edges[1].body_id as usize];
                    b3_assert!(index_b == body_b.local_index);
                }
            }

            // Body A data
            let m_a;
            let i_a;
            let v_a;
            let w_a;

            if index_a == NULL_INDEX {
                m_a = 0.0;
                i_a = Matrix3::ZERO;
                v_a = Vec3::ZERO;
                w_a = Vec3::ZERO;
            } else {
                let sim_a = &body_sims[index_a as usize];
                m_a = sim_a.inv_mass;
                i_a = sim_a.inv_inertia_world;

                let state_a = &body_states[index_a as usize];
                v_a = state_a.linear_velocity;
                w_a = state_a.angular_velocity;
            }

            // Body B data
            let m_b;
            let i_b;
            let v_b;
            let w_b;

            if index_b == NULL_INDEX {
                m_b = 0.0;
                i_b = Matrix3::ZERO;
                v_b = Vec3::ZERO;
                w_b = Vec3::ZERO;
            } else {
                let sim_b = &body_sims[index_b as usize];
                m_b = sim_b.inv_mass;
                i_b = sim_b.inv_inertia_world;

                let state_b = &body_states[index_b as usize];
                v_b = state_b.linear_velocity;
                w_b = state_b.angular_velocity;
            }

            let manifold_count = contact.manifold_count();
            let contact_constraint = ContactConstraint {
                constraint_start: spec.manifold_start,
                contact_id,
                manifold_count,
                index_a,
                index_b,
                inv_i_a: i_a,
                inv_mass_a: m_a,
                inv_i_b: i_b,
                inv_mass_b: m_b,
                rolling_mass: invert_matrix(add_mm(i_a, i_b)),
                // Stiffer for static contacts to avoid bodies getting pushed through the ground
                softness: if (contact.flags & CONTACT_STATIC_FLAG) != 0 {
                    static_softness
                } else {
                    contact_softness
                },
                friction: contact.friction,
                restitution: contact.restitution,
                rolling_resistance: contact.rolling_resistance,
            };
            contact_constraints[(base_offset + index) as usize] = contact_constraint;

            for manifold_index in 0..manifold_count {
                let manifold = &contact.manifolds[manifold_index as usize];
                let constraint = &mut manifold_constraints[(spec.manifold_start + manifold_index) as usize];
                let point_count = manifold.point_count;
                let normal = manifold.normal;
                let tangent1 = perp(normal);
                let tangent2 = cross(tangent1, normal);

                constraint.point_count = point_count;
                constraint.normal = normal;
                constraint.tangent1 = tangent1;
                constraint.tangent2 = tangent2;

                constraint.tangent_velocity1 = dot(contact.tangent_velocity, constraint.tangent1);
                constraint.tangent_velocity2 = dot(contact.tangent_velocity, constraint.tangent2);

                let mut center_a = Vec3::ZERO;
                let mut center_b = Vec3::ZERO;

                for point_index in 0..point_count {
                    let cp = &mut constraint.points[point_index as usize];

                    // Copy data from manifold point
                    let mp = &manifold.points[point_index as usize];
                    cp.r_a = mp.anchor_a;
                    cp.r_b = mp.anchor_b;
                    cp.base_separation = mp.separation - dot(sub(cp.r_b, cp.r_a), normal);
                    cp.normal_impulse = warm_start_scale * mp.normal_impulse;
                    cp.total_normal_impulse = 0.0;

                    let r_a = cp.r_a;
                    let r_b = cp.r_b;

                    let rn_a = cross(r_a, normal);
                    let rn_b = cross(r_b, normal);
                    let k_normal = m_a + m_b + dot(rn_a, mul_mv(i_a, rn_a)) + dot(rn_b, mul_mv(i_b, rn_b));
                    cp.normal_mass = if k_normal > 0.0 { 1.0 / k_normal } else { 0.0 };

                    // Save relative velocity for restitution
                    let vr_a = add(v_a, cross(w_a, r_a));
                    let vr_b = add(v_b, cross(w_b, r_b));
                    cp.relative_velocity = dot(normal, sub(vr_b, vr_a));

                    center_a = add(center_a, r_a);
                    center_b = add(center_b, r_b);
                }

                let inv_count = 1.0 / point_count as f32;
                center_a = mul_sv(inv_count, center_a);
                center_b = mul_sv(inv_count, center_b);
                constraint.origin_a = center_a;
                constraint.origin_b = center_b;

                for point_index in 0..point_count {
                    let cp = &mut constraint.points[point_index as usize];
                    cp.lever_arm = distance(cp.r_a, center_a);
                }

                let rt_a1 = cross(center_a, tangent1);
                let rt_a2 = cross(center_a, tangent2);
                let rt_b1 = cross(center_b, tangent1);
                let rt_b2 = cross(center_b, tangent2);

                {
                    let mut k = Matrix2::default();
                    k.cx.x = m_a + m_b + dot(rt_a1, mul_mv(i_a, rt_a1)) + dot(rt_b1, mul_mv(i_b, rt_b1));
                    k.cy.y = m_a + m_b + dot(rt_a2, mul_mv(i_a, rt_a2)) + dot(rt_b2, mul_mv(i_b, rt_b2));
                    k.cx.y = dot(rt_a1, mul_mv(i_a, rt_a2)) + dot(rt_b1, mul_mv(i_b, rt_b2));
                    k.cy.x = k.cx.y;

                    constraint.tangent_mass = invert2(k);
                    constraint.friction_impulse.x = warm_start_scale * dot(manifold.friction_impulse, tangent1);
                    constraint.friction_impulse.y = warm_start_scale * dot(manifold.friction_impulse, tangent2);
                }

                {
                    let k = dot(normal, mul_mv(add_mm(i_a, i_b), normal));
                    constraint.twist_mass = if k > 0.0 { 1.0 / k } else { 0.0 };
                    constraint.twist_impulse = warm_start_scale * manifold.twist_impulse;
                }

                {
                    constraint.rolling_impulse = mul_sv(warm_start_scale, manifold.rolling_impulse);
                }
            }

            index += 1;
        }

        // Advance to next color
        color_index += 1;
    }
}

pub fn warm_start_contacts_mesh(block: SolverBlock, world: &mut World, context: &mut StepContext) {
    let world = &*world;
    let color = &world.constraint_graph.colors[block.color_index as usize];
    let cc_start = color.contact_constraint_start;

    let ctx = &mut *context;
    // C reads the awake set body states; those are moved into the context
    // during the solve (see solver.rs).
    let states = &mut ctx.states;
    let manifold_constraints = &ctx.manifold_constraints;
    let constraints = &ctx.contact_constraints;

    let start_index = block.start_index;
    let end_index = start_index + block.count as i32;

    for constraint_index in start_index..end_index {
        let contact_constraint = constraints[(cc_start + constraint_index) as usize];
        let index_a = contact_constraint.index_a;
        let index_b = contact_constraint.index_b;

        // This is a dummy state to represent a static body because static bodies
        // don't have a solver body.
        let state_a = if index_a == NULL_INDEX { IDENTITY_BODY_STATE } else { states[index_a as usize] };
        let state_b = if index_b == NULL_INDEX { IDENTITY_BODY_STATE } else { states[index_b as usize] };

        let mut v_a = state_a.linear_velocity;
        let mut w_a = state_a.angular_velocity;
        let mut v_b = state_b.linear_velocity;
        let mut w_b = state_b.angular_velocity;

        let m_a = contact_constraint.inv_mass_a;
        let i_a = contact_constraint.inv_i_a;
        let m_b = contact_constraint.inv_mass_b;
        let i_b = contact_constraint.inv_i_b;

        let manifold_count = contact_constraint.manifold_count;
        for manifold_index in 0..manifold_count {
            let constraint = &manifold_constraints[(contact_constraint.constraint_start + manifold_index) as usize];

            // Normal impulses
            let normal = constraint.normal;
            let point_count = constraint.point_count;
            for j in 0..point_count {
                let cp = &constraint.points[j as usize];

                // fixed anchors
                let r_a = cp.r_a;
                let r_b = cp.r_b;

                let impulse = mul_sv(cp.normal_impulse, normal);
                w_a = sub(w_a, mul_mv(i_a, cross(r_a, impulse)));
                v_a = mul_sub(v_a, m_a, impulse);
                w_b = add(w_b, mul_mv(i_b, cross(r_b, impulse)));
                v_b = mul_add(v_b, m_b, impulse);
            }

            // Central friction
            {
                let r_a = constraint.origin_a;
                let r_b = constraint.origin_b;
                let mut impulse = mul_sv(constraint.friction_impulse.x, constraint.tangent1);
                impulse = add(impulse, mul_sv(constraint.friction_impulse.y, constraint.tangent2));

                w_a = sub(w_a, mul_mv(i_a, cross(r_a, impulse)));
                v_a = mul_sub(v_a, m_a, impulse);
                w_b = add(w_b, mul_mv(i_b, cross(r_b, impulse)));
                v_b = mul_add(v_b, m_b, impulse);
            }

            // Central twist friction
            {
                let impulse = mul_sv(constraint.twist_impulse, constraint.normal);
                w_a = sub(w_a, mul_mv(i_a, impulse));
                w_b = add(w_b, mul_mv(i_b, impulse));
            }

            // Rolling resistance
            {
                let impulse = constraint.rolling_impulse;
                w_a = sub(w_a, mul_mv(i_a, impulse));
                w_b = add(w_b, mul_mv(i_b, impulse));
            }
        }

        if index_a != NULL_INDEX && (state_a.flags & DYNAMIC_FLAG) != 0 {
            states[index_a as usize].linear_velocity = v_a;
            states[index_a as usize].angular_velocity = w_a;
        }

        if index_b != NULL_INDEX && (state_b.flags & DYNAMIC_FLAG) != 0 {
            states[index_b as usize].linear_velocity = v_b;
            states[index_b as usize].angular_velocity = w_b;
        }
    }
}

// Merged normal and friction loops. This is much more stable for the Jenga stack.
pub fn solve_contacts_mesh(block: SolverBlock, world: &mut World, context: &mut StepContext, use_bias: bool) {
    let world = &*world;
    let color = &world.constraint_graph.colors[block.color_index as usize];
    let cc_start = color.contact_constraint_start;

    let inv_h = context.inv_h;
    let contact_speed = world.contact_speed;

    let ctx = &mut *context;
    let states = &mut ctx.states;
    let manifold_constraints = &mut ctx.manifold_constraints;
    let contact_constraints = &ctx.contact_constraints;

    // The last block might not be full
    let start_index = block.start_index;
    let end_index = start_index + block.count as i32;

    for i in start_index..end_index {
        let contact_constraint = contact_constraints[(cc_start + i) as usize];
        let manifold_count = contact_constraint.manifold_count;

        let index_a = contact_constraint.index_a;
        let index_b = contact_constraint.index_b;

        let m_a = contact_constraint.inv_mass_a;
        let i_a = contact_constraint.inv_i_a;
        let m_b = contact_constraint.inv_mass_b;
        let i_b = contact_constraint.inv_i_b;

        // This is a dummy state to represent a static body because static bodies
        // don't have a solver body.
        let state_a = if index_a == NULL_INDEX { IDENTITY_BODY_STATE } else { states[index_a as usize] };
        let mut v_a = state_a.linear_velocity;
        let mut w_a = state_a.angular_velocity;
        let dq_a = state_a.delta_rotation;

        let state_b = if index_b == NULL_INDEX { IDENTITY_BODY_STATE } else { states[index_b as usize] };
        let mut v_b = state_b.linear_velocity;
        let mut w_b = state_b.angular_velocity;
        let dq_b = state_b.delta_rotation;

        let dp = sub(state_b.delta_position, state_a.delta_position);
        let softness = contact_constraint.softness;
        let friction = contact_constraint.friction;
        let rolling_resistance = contact_constraint.rolling_resistance;

        for j in 0..manifold_count {
            let constraint = &mut manifold_constraints[(contact_constraint.constraint_start + j) as usize];

            let point_count = constraint.point_count;
            let normal = constraint.normal;

            let mut total_normal_impulse = 0.0;
            let mut total_twist_limit = 0.0;

            for point_index in 0..point_count {
                let cp = &mut constraint.points[point_index as usize];

                // Fixed anchor points for applying impulses
                let r_a = cp.r_a;
                let r_b = cp.r_b;

                // compute current separation
                // this is subject to round-off error if the anchor is far from the body center of mass
                let ds = add(dp, sub(rotate_vector(dq_b, r_b), rotate_vector(dq_a, r_a)));
                let s = dot(ds, normal) + cp.base_separation;

                let mut velocity_bias = 0.0;
                let mut mass_scale = 1.0;
                let mut impulse_scale = 0.0;
                if s > 0.0 {
                    // speculative bias
                    velocity_bias = s * inv_h;
                } else if use_bias {
                    velocity_bias = max_float(softness.mass_scale * softness.bias_rate * s, -contact_speed);
                    mass_scale = softness.mass_scale;
                    impulse_scale = softness.impulse_scale;
                }

                // relative normal velocity at contact
                let vr_a = add(v_a, cross(w_a, r_a));
                let vr_b = add(v_b, cross(w_b, r_b));
                let vn = dot(sub(vr_b, vr_a), normal);

                // incremental normal impulse
                let mut delta_impulse = -cp.normal_mass * (mass_scale * vn + velocity_bias) - impulse_scale * cp.normal_impulse;

                // clamp the accumulated impulse
                let new_impulse = max_float(cp.normal_impulse + delta_impulse, 0.0);
                delta_impulse = new_impulse - cp.normal_impulse;
                cp.normal_impulse = new_impulse;
                cp.total_normal_impulse += new_impulse;

                total_normal_impulse += new_impulse;
                total_twist_limit += cp.lever_arm * cp.normal_impulse;

                // apply normal impulse
                let p = mul_sv(delta_impulse, normal);
                v_a = mul_sub(v_a, m_a, p);
                w_a = sub(w_a, mul_mv(i_a, cross(r_a, p)));

                v_b = mul_add(v_b, m_b, p);
                w_b = add(w_b, mul_mv(i_b, cross(r_b, p)));
            }

            // No friction when applying bias (C: continue to next manifold)
            if !use_bias {
                // Central twist friction
                {
                    let twist_speed = dot(constraint.normal, sub(w_b, w_a));
                    let max_impulse = friction * total_twist_limit;
                    let mut delta_impulse = -constraint.twist_mass * twist_speed;
                    let old_impulse = constraint.twist_impulse;
                    constraint.twist_impulse = clamp_float(old_impulse + delta_impulse, -max_impulse, max_impulse);
                    delta_impulse = constraint.twist_impulse - old_impulse;

                    w_a = sub(w_a, mul_mv(i_a, mul_sv(delta_impulse, constraint.normal)));
                    w_b = add(w_b, mul_mv(i_b, mul_sv(delta_impulse, constraint.normal)));
                }

                // Rolling resistance
                if rolling_resistance > 0.0 {
                    let mut delta_impulse = neg(mul_mv(contact_constraint.rolling_mass, sub(w_b, w_a)));
                    let old_impulse = constraint.rolling_impulse;
                    constraint.rolling_impulse = add(old_impulse, delta_impulse);

                    let max_impulse = rolling_resistance * total_normal_impulse;
                    let mag_sqr = dot(constraint.rolling_impulse, constraint.rolling_impulse);
                    if mag_sqr > max_impulse * max_impulse + f32::EPSILON {
                        constraint.rolling_impulse = mul_sv(max_impulse / mag_sqr.sqrt(), constraint.rolling_impulse);
                    }

                    delta_impulse = sub(constraint.rolling_impulse, old_impulse);

                    w_a = sub(w_a, mul_mv(i_a, delta_impulse));
                    w_b = add(w_b, mul_mv(i_b, delta_impulse));
                }

                // Central friction
                {
                    let tangent1 = constraint.tangent1;
                    let tangent2 = constraint.tangent2;

                    // Fixed anchor points for applying impulses
                    let r_a = constraint.origin_a;
                    let r_b = constraint.origin_b;

                    // Relative tangent velocity at contact
                    let vr_a = add(v_a, cross(w_a, r_a));
                    let vr_b = add(v_b, cross(w_b, r_b));
                    let vr = sub(vr_b, vr_a);
                    let vt = vec2(
                        dot(vr, tangent1) - constraint.tangent_velocity1,
                        dot(vr, tangent2) - constraint.tangent_velocity2,
                    );

                    // Incremental tangent impulse
                    let tm = mul_mv2(constraint.tangent_mass, vt);
                    let mut delta_impulse = vec2(-tm.x, -tm.y);
                    let mut new_impulse = vec2(
                        constraint.friction_impulse.x + delta_impulse.x,
                        constraint.friction_impulse.y + delta_impulse.y,
                    );

                    let max_impulse = friction * total_normal_impulse;

                    // Clamp the accumulated impulse
                    let length_squared = dot2(new_impulse, new_impulse);
                    if length_squared > max_impulse * max_impulse {
                        let scale = max_impulse / length_squared.sqrt();
                        new_impulse.x *= scale;
                        new_impulse.y *= scale;
                    }
                    delta_impulse = sub2(new_impulse, constraint.friction_impulse);
                    constraint.friction_impulse = new_impulse;

                    // Apply delta impulse
                    let p = blend2(delta_impulse.x, tangent1, delta_impulse.y, tangent2);
                    v_a = mul_sub(v_a, m_a, p);
                    w_a = sub(w_a, mul_mv(i_a, cross(r_a, p)));
                    v_b = mul_add(v_b, m_b, p);
                    w_b = add(w_b, mul_mv(i_b, cross(r_b, p)));
                }
            }
        }

        if index_a != NULL_INDEX && (state_a.flags & DYNAMIC_FLAG) != 0 {
            states[index_a as usize].linear_velocity = v_a;
            states[index_a as usize].angular_velocity = w_a;
        }

        if index_b != NULL_INDEX && (state_b.flags & DYNAMIC_FLAG) != 0 {
            states[index_b as usize].linear_velocity = v_b;
            states[index_b as usize].angular_velocity = w_b;
        }
    }
}

pub fn apply_restitution_mesh(block: SolverBlock, world: &mut World, context: &mut StepContext) {
    let world = &*world;
    let color = &world.constraint_graph.colors[block.color_index as usize];
    let cc_start = color.contact_constraint_start;

    let threshold = world.restitution_threshold;

    let ctx = &mut *context;
    let states = &mut ctx.states;
    let manifold_constraints = &mut ctx.manifold_constraints;
    let contact_constraints = &ctx.contact_constraints;

    let start_index = block.start_index;
    let end_index = start_index + block.count as i32;

    for constraint_index in start_index..end_index {
        let contact_constraint = contact_constraints[(cc_start + constraint_index) as usize];
        let restitution = contact_constraint.restitution;
        if restitution == 0.0 {
            continue;
        }

        let index_a = contact_constraint.index_a;
        let index_b = contact_constraint.index_b;

        let state_a = if index_a == NULL_INDEX { IDENTITY_BODY_STATE } else { states[index_a as usize] };
        let state_b = if index_b == NULL_INDEX { IDENTITY_BODY_STATE } else { states[index_b as usize] };

        let mut v_a = state_a.linear_velocity;
        let mut w_a = state_a.angular_velocity;
        let mut v_b = state_b.linear_velocity;
        let mut w_b = state_b.angular_velocity;

        let m_a = contact_constraint.inv_mass_a;
        let i_a = contact_constraint.inv_i_a;
        let m_b = contact_constraint.inv_mass_b;
        let i_b = contact_constraint.inv_i_b;

        let manifold_count = contact_constraint.manifold_count;
        for manifold_index in 0..manifold_count {
            let cm = &mut manifold_constraints[(contact_constraint.constraint_start + manifold_index) as usize];

            let normal = cm.normal;
            let point_count = cm.point_count;
            b3_assert!(0 < point_count && point_count <= MAX_MANIFOLD_POINTS as i32);

            for point_index in 0..point_count {
                let cp = &mut cm.points[point_index as usize];

                // If the total normal impulse is zero then there was no collision
                // this skips speculative contact points that didn't generate an impulse
                if cp.relative_velocity > -threshold || cp.total_normal_impulse == 0.0 {
                    continue;
                }

                // fixed anchor points
                let r_a = cp.r_a;
                let r_b = cp.r_b;

                // relative normal velocity at contact
                let vr_b = add(v_b, cross(w_b, r_b));
                let vr_a = add(v_a, cross(w_a, r_a));
                let vn = dot(sub(vr_b, vr_a), normal);

                // compute normal impulse
                let mut impulse = -cp.normal_mass * (vn + restitution * cp.relative_velocity);

                // clamp the accumulated impulse
                let new_impulse = max_float(cp.normal_impulse + impulse, 0.0);
                impulse = new_impulse - cp.normal_impulse;
                cp.normal_impulse = new_impulse;
                cp.total_normal_impulse += impulse;

                // apply contact impulse
                let p = mul_sv(impulse, normal);
                v_a = mul_sub(v_a, m_a, p);
                w_a = sub(w_a, mul_mv(i_a, cross(r_a, p)));
                v_b = mul_add(v_b, m_b, p);
                w_b = add(w_b, mul_mv(i_b, cross(r_b, p)));
            }

            // C writes the states back inside the manifold loop.
            if index_a != NULL_INDEX && (state_a.flags & DYNAMIC_FLAG) != 0 {
                states[index_a as usize].linear_velocity = v_a;
                states[index_a as usize].angular_velocity = w_a;
            }

            if index_b != NULL_INDEX && (state_b.flags & DYNAMIC_FLAG) != 0 {
                states[index_b as usize].linear_velocity = v_b;
                states[index_b as usize].angular_velocity = w_b;
            }
        }
    }
}

// Don't need to use spans for colors for this because the constraint to contact
// association is already linked (by contact id in the port).
pub fn store_impulses_mesh(block: SolverBlock, world: &mut World, context: &mut StepContext, worker_index: i32) {
    let ctx = &*context;

    // Mirror prepare_contacts_mesh: the per-color flat arrays and the overflow color
    // each have their own (base, spans).
    let (spans, base_offset) = if block.block_type == SolverBlockType::Overflow {
        (
            &ctx.overflow_spans,
            world.constraint_graph.colors[OVERFLOW_INDEX as usize].contact_constraint_start,
        )
    } else {
        (&ctx.contact_prepare_spans, 0)
    };

    let mut has_hit_events = world.task_contexts[worker_index as usize].has_hit_events;
    let neg_hit_threshold = -world.hit_event_threshold;

    let mut index = block.start_index;
    let end_index = block.start_index + block.count as i32;

    // Find color for start index. Linear search but fast.
    let mut color_index = 0usize;
    while spans[color_index + 1].start <= index {
        color_index += 1;
    }

    // Loop over block
    while index < end_index {
        let color_start = spans[color_index].start;
        let color_end_index = min_int(spans[color_index + 1].start, end_index);
        let specs_color = spans[color_index].color_index as usize;

        // Loop over color
        while index < color_end_index {
            let contact_constraint = ctx.contact_constraints[(base_offset + index) as usize];

            let local_index = index - color_start;
            b3_assert!(0 <= local_index && local_index < spans[color_index].count);

            // Having this contact id simplifies impulse storage
            let contact_id = contact_constraint.contact_id;
            b3_assert!(contact_id != NULL_INDEX);

            // Catches the wrong-(base, spans) pairing: the contact id stashed by
            // prepare_contacts_mesh at this flat slot must reference the same contact
            // the span at this slot describes.
            b3_validate!(
                contact_id
                    == world.constraint_graph.colors[specs_color].contacts[local_index as usize].contact_id
            );

            let contact = &mut world.contacts[contact_id as usize];

            let manifold_count = contact_constraint.manifold_count;
            b3_assert!(manifold_count == contact.manifolds.len() as i32);

            let check_hit_events = (contact.flags & SIM_ENABLE_HIT_EVENT) != 0;
            let mut flagged = false;
            let mut hit_contact_id = NULL_INDEX;

            for manifold_index in 0..manifold_count {
                let manifold = &mut contact.manifolds[manifold_index as usize];
                let constraint = &ctx.manifold_constraints[(contact_constraint.constraint_start + manifold_index) as usize];
                manifold.twist_impulse = constraint.twist_impulse;
                manifold.friction_impulse = blend2(
                    constraint.friction_impulse.x,
                    constraint.tangent1,
                    constraint.friction_impulse.y,
                    constraint.tangent2,
                );
                manifold.rolling_impulse = constraint.rolling_impulse;

                let count = constraint.point_count;
                b3_assert!(count == manifold.point_count);
                for point_index in 0..count {
                    let cp = &constraint.points[point_index as usize];
                    let mp = &mut manifold.points[point_index as usize];
                    mp.normal_impulse = cp.normal_impulse;
                    mp.total_normal_impulse = cp.total_normal_impulse;
                    mp.normal_velocity = cp.relative_velocity;

                    if check_hit_events
                        && !flagged
                        && mp.normal_velocity < neg_hit_threshold
                        && mp.total_normal_impulse > 0.0
                    {
                        hit_contact_id = contact_id;
                        has_hit_events = true;
                        flagged = true;
                    }
                }
            }

            if hit_contact_id != NULL_INDEX {
                let task_context = &mut world.task_contexts[worker_index as usize];
                set_bit(&mut task_context.hit_event_bit_set, hit_contact_id as u32);
            }

            index += 1;
        }

        // Advance to next color
        color_index += 1;
    }

    world.task_contexts[worker_index as usize].has_hit_events = has_hit_events;
}

// ---------------------------------------------------------------------------
// contact_solver.c — wide (4 lane) types and operations, B3_SIMD_NONE path
// ---------------------------------------------------------------------------

/// C b3FloatW (scalar fallback): four lanes, one per contact.
#[derive(Clone, Copy, Debug, Default)]
pub struct FloatW {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl FloatW {
    /// C: ((float*)&w)[lane]
    #[inline(always)]
    pub fn get(self, lane: usize) -> f32 {
        match lane {
            0 => self.x,
            1 => self.y,
            2 => self.z,
            _ => self.w,
        }
    }

    /// C: ((float*)&w)[lane] = value
    #[inline(always)]
    pub fn set(&mut self, lane: usize, value: f32) {
        match lane {
            0 => self.x = value,
            1 => self.y = value,
            2 => self.z = value,
            _ => self.w = value,
        }
    }
}

/// Wide vec2
#[derive(Clone, Copy, Debug, Default)]
pub struct Vec2W {
    pub x: FloatW,
    pub y: FloatW,
}

/// Wide vec3
#[derive(Clone, Copy, Debug, Default)]
pub struct Vec3W {
    pub x: FloatW,
    pub y: FloatW,
    pub z: FloatW,
}

/// Wide quaternion
#[derive(Clone, Copy, Debug, Default)]
pub struct QuatW {
    pub v: Vec3W,
    pub s: FloatW,
}

/// Wide symmetric matrix2
#[derive(Clone, Copy, Debug, Default)]
pub struct SymMatrix2W {
    pub cxx: FloatW,
    pub cxy: FloatW,
    pub cyy: FloatW,
}

/// Wide symmetric matrix3
#[derive(Clone, Copy, Debug, Default)]
pub struct SymMatrix3W {
    pub cxx: FloatW,
    pub cxy: FloatW,
    pub cxz: FloatW,
    pub cyy: FloatW,
    pub cyz: FloatW,
    pub czz: FloatW,
}

#[inline(always)]
fn zero_w() -> FloatW {
    FloatW { x: 0.0, y: 0.0, z: 0.0, w: 0.0 }
}

#[inline(always)]
fn splat_w(scalar: f32) -> FloatW {
    FloatW { x: scalar, y: scalar, z: scalar, w: scalar }
}

#[inline(always)]
fn neg_w(a: FloatW) -> FloatW {
    FloatW { x: -a.x, y: -a.y, z: -a.z, w: -a.w }
}

#[inline(always)]
fn add_w(a: FloatW, b: FloatW) -> FloatW {
    FloatW { x: a.x + b.x, y: a.y + b.y, z: a.z + b.z, w: a.w + b.w }
}

#[inline(always)]
fn sub_w(a: FloatW, b: FloatW) -> FloatW {
    FloatW { x: a.x - b.x, y: a.y - b.y, z: a.z - b.z, w: a.w - b.w }
}

#[inline(always)]
fn mul_w(a: FloatW, b: FloatW) -> FloatW {
    FloatW { x: a.x * b.x, y: a.y * b.y, z: a.z * b.z, w: a.w * b.w }
}

#[inline(always)]
fn div_w(a: FloatW, b: FloatW) -> FloatW {
    FloatW { x: a.x / b.x, y: a.y / b.y, z: a.z / b.z, w: a.w / b.w }
}

#[inline(always)]
fn sqrt_w(a: FloatW) -> FloatW {
    FloatW { x: a.x.sqrt(), y: a.y.sqrt(), z: a.z.sqrt(), w: a.w.sqrt() }
}

/// a + b * c
#[inline(always)]
fn mul_add_w(a: FloatW, b: FloatW, c: FloatW) -> FloatW {
    FloatW { x: a.x + b.x * c.x, y: a.y + b.y * c.y, z: a.z + b.z * c.z, w: a.w + b.w * c.w }
}

#[inline(always)]
fn max_w(a: FloatW, b: FloatW) -> FloatW {
    FloatW {
        x: if a.x >= b.x { a.x } else { b.x },
        y: if a.y >= b.y { a.y } else { b.y },
        z: if a.z >= b.z { a.z } else { b.z },
        w: if a.w >= b.w { a.w } else { b.w },
    }
}

/// clamp a to [-b, b]
#[inline(always)]
fn sym_clamp_w(a: FloatW, b: FloatW) -> FloatW {
    let mut r = FloatW {
        x: if a.x <= b.x { a.x } else { b.x },
        y: if a.y <= b.y { a.y } else { b.y },
        z: if a.z <= b.z { a.z } else { b.z },
        w: if a.w <= b.w { a.w } else { b.w },
    };
    r.x = if r.x <= -b.x { -b.x } else { r.x };
    r.y = if r.y <= -b.y { -b.y } else { r.y };
    r.z = if r.z <= -b.z { -b.z } else { r.z };
    r.w = if r.w <= -b.w { -b.w } else { r.w };
    r
}

#[inline(always)]
fn or_w(a: FloatW, b: FloatW) -> FloatW {
    FloatW {
        x: if a.x != 0.0 || b.x != 0.0 { 1.0 } else { 0.0 },
        y: if a.y != 0.0 || b.y != 0.0 { 1.0 } else { 0.0 },
        z: if a.z != 0.0 || b.z != 0.0 { 1.0 } else { 0.0 },
        w: if a.w != 0.0 || b.w != 0.0 { 1.0 } else { 0.0 },
    }
}

#[inline(always)]
fn greater_than_w(a: FloatW, b: FloatW) -> FloatW {
    FloatW {
        x: if a.x > b.x { 1.0 } else { 0.0 },
        y: if a.y > b.y { 1.0 } else { 0.0 },
        z: if a.z > b.z { 1.0 } else { 0.0 },
        w: if a.w > b.w { 1.0 } else { 0.0 },
    }
}

#[inline(always)]
fn equals_w(a: FloatW, b: FloatW) -> FloatW {
    FloatW {
        x: if a.x == b.x { 1.0 } else { 0.0 },
        y: if a.y == b.y { 1.0 } else { 0.0 },
        z: if a.z == b.z { 1.0 } else { 0.0 },
        w: if a.w == b.w { 1.0 } else { 0.0 },
    }
}

#[inline(always)]
fn all_zero_w(a: FloatW) -> bool {
    a.x == 0.0 && a.y == 0.0 && a.z == 0.0 && a.w == 0.0
}

/// component-wise returns mask ? b : a
#[inline(always)]
fn blend_w(a: FloatW, b: FloatW, mask: FloatW) -> FloatW {
    FloatW {
        x: if mask.x != 0.0 { b.x } else { a.x },
        y: if mask.y != 0.0 { b.y } else { a.y },
        z: if mask.z != 0.0 { b.z } else { a.z },
        w: if mask.w != 0.0 { b.w } else { a.w },
    }
}

/// s * a
#[inline(always)]
fn mul_svw(s: FloatW, a: Vec3W) -> Vec3W {
    Vec3W { x: mul_w(s, a.x), y: mul_w(s, a.y), z: mul_w(s, a.z) }
}

/// a - s * b
#[inline(always)]
fn mul_sub_svw(a: Vec3W, s: FloatW, b: Vec3W) -> Vec3W {
    Vec3W {
        x: sub_w(a.x, mul_w(s, b.x)),
        y: sub_w(a.y, mul_w(s, b.y)),
        z: sub_w(a.z, mul_w(s, b.z)),
    }
}

/// a + s * b
#[inline(always)]
fn mul_add_svw(a: Vec3W, s: FloatW, b: Vec3W) -> Vec3W {
    Vec3W {
        x: add_w(a.x, mul_w(s, b.x)),
        y: add_w(a.y, mul_w(s, b.y)),
        z: add_w(a.z, mul_w(s, b.z)),
    }
}

/// a + b
#[inline(always)]
fn add_v2w(a: Vec2W, b: Vec2W) -> Vec2W {
    Vec2W { x: add_w(a.x, b.x), y: add_w(a.y, b.y) }
}

/// a - b
#[inline(always)]
fn sub_vw(a: Vec3W, b: Vec3W) -> Vec3W {
    Vec3W { x: sub_w(a.x, b.x), y: sub_w(a.y, b.y), z: sub_w(a.z, b.z) }
}

/// a + b
#[inline(always)]
fn add_vw(a: Vec3W, b: Vec3W) -> Vec3W {
    Vec3W { x: add_w(a.x, b.x), y: add_w(a.y, b.y), z: add_w(a.z, b.z) }
}

/// m * a
#[inline(always)]
fn mul_mv2w(m: SymMatrix2W, a: Vec2W) -> Vec2W {
    Vec2W {
        x: add_w(mul_w(m.cxx, a.x), mul_w(m.cxy, a.y)),
        y: add_w(mul_w(m.cxy, a.x), mul_w(m.cyy, a.y)),
    }
}

/// m * a
#[inline(always)]
fn mul_mvw(m: SymMatrix3W, a: Vec3W) -> Vec3W {
    Vec3W {
        x: add_w(mul_w(m.cxx, a.x), add_w(mul_w(m.cxy, a.y), mul_w(m.cxz, a.z))),
        y: add_w(mul_w(m.cxy, a.x), add_w(mul_w(m.cyy, a.y), mul_w(m.cyz, a.z))),
        z: add_w(mul_w(m.cxz, a.x), add_w(mul_w(m.cyz, a.y), mul_w(m.czz, a.z))),
    }
}

/// a - m * b
#[inline(always)]
fn mul_sub_mvw(a: Vec3W, m: SymMatrix3W, b: Vec3W) -> Vec3W {
    let c = Vec3W {
        x: add_w(mul_w(m.cxx, b.x), add_w(mul_w(m.cxy, b.y), mul_w(m.cxz, b.z))),
        y: add_w(mul_w(m.cxy, b.x), add_w(mul_w(m.cyy, b.y), mul_w(m.cyz, b.z))),
        z: add_w(mul_w(m.cxz, b.x), add_w(mul_w(m.cyz, b.y), mul_w(m.czz, b.z))),
    };

    Vec3W { x: sub_w(a.x, c.x), y: sub_w(a.y, c.y), z: sub_w(a.z, c.z) }
}

/// a + m * b
#[inline(always)]
fn mul_add_mvw(a: Vec3W, m: SymMatrix3W, b: Vec3W) -> Vec3W {
    let c = Vec3W {
        x: add_w(mul_w(m.cxx, b.x), add_w(mul_w(m.cxy, b.y), mul_w(m.cxz, b.z))),
        y: add_w(mul_w(m.cxy, b.x), add_w(mul_w(m.cyy, b.y), mul_w(m.cyz, b.z))),
        z: add_w(mul_w(m.cxz, b.x), add_w(mul_w(m.cyz, b.y), mul_w(m.czz, b.z))),
    };

    Vec3W { x: add_w(a.x, c.x), y: add_w(a.y, c.y), z: add_w(a.z, c.z) }
}

#[inline(always)]
fn dot_w(a: Vec3W, b: Vec3W) -> FloatW {
    add_w(add_w(mul_w(a.x, b.x), mul_w(a.y, b.y)), mul_w(a.z, b.z))
}

#[inline(always)]
fn cross_w(a: Vec3W, b: Vec3W) -> Vec3W {
    Vec3W {
        x: sub_w(mul_w(a.y, b.z), mul_w(a.z, b.y)),
        y: sub_w(mul_w(a.z, b.x), mul_w(a.x, b.z)),
        z: sub_w(mul_w(a.x, b.y), mul_w(a.y, b.x)),
    }
}

#[inline(always)]
fn rotate_vector_w(q: QuatW, a: Vec3W) -> Vec3W {
    let t1 = cross_w(q.v, a);
    let t2 = Vec3W {
        x: mul_add_w(t1.x, q.s, a.x),
        y: mul_add_w(t1.y, q.s, a.y),
        z: mul_add_w(t1.z, q.s, a.z),
    };
    let t3 = cross_w(q.v, t2);
    let two = splat_w(2.0);
    Vec3W {
        x: mul_add_w(a.x, two, t3.x),
        y: mul_add_w(a.y, two, t3.y),
        z: mul_add_w(a.z, two, t3.z),
    }
}

// Soft contact constraints with sub-stepping support
// Uses fixed anchors for Jacobians for better behavior on rolling shapes (circles & capsules)
// http://mmacklin.com/smallsteps.pdf
// https://box2d.org/files/ErinCatto_SoftConstraints_GDC2011.pdf

#[derive(Clone, Copy, Debug, Default)]
pub struct ContactConstraintPointWide {
    pub anchor_as: Vec3W,
    pub anchor_bs: Vec3W,
    pub base_separations: FloatW,
    pub normal_impulses: FloatW,
    pub total_normal_impulses: FloatW,
    pub normal_masses: FloatW,
    pub lever_arms: FloatW,
    pub relative_velocities: FloatW,
}

/// Solves four contacts (one manifold each).
#[derive(Clone, Copy, Debug)]
pub struct ContactConstraintWide {
    /// These are base 1 (0 indicates null)
    pub index_a: [i32; SIMD_WIDTH],
    pub index_b: [i32; SIMD_WIDTH],

    pub inv_mass_a: FloatW,
    pub inv_mass_b: FloatW,
    pub inv_i_a: SymMatrix3W,
    pub inv_i_b: SymMatrix3W,
    pub normal: Vec3W,

    pub tangent1: Vec3W,
    pub tangent2: Vec3W,

    pub origin_a: Vec3W,
    pub origin_b: Vec3W,
    pub twist_mass: FloatW,
    pub twist_impulse: FloatW,
    pub tangent_mass: SymMatrix2W,
    pub friction_impulse: Vec2W,
    pub rolling_mass: SymMatrix3W,
    pub rolling_impulse: Vec3W,
    pub friction: FloatW,
    pub rolling_resistance: FloatW,
    pub tangent_velocity1: FloatW,
    pub tangent_velocity2: FloatW,

    pub bias_rate: FloatW,
    pub mass_scale: FloatW,
    pub impulse_scale: FloatW,
    pub restitution: FloatW,

    /// C: b3Manifold* manifolds[4] — per lane contact id (the manifold is
    /// world.contacts[id].manifolds[0]); NULL_INDEX for empty lanes.
    pub contact_ids: [i32; SIMD_WIDTH],

    pub points: [ContactConstraintPointWide; MAX_MANIFOLD_POINTS],
}

impl Default for ContactConstraintWide {
    /// Equivalent of the C memset-zero done in solver setup: null (base-1 == 0)
    /// body indices, null per-lane contacts, zero data. Remainder lanes stay inert.
    fn default() -> Self {
        ContactConstraintWide {
            index_a: [0; SIMD_WIDTH],
            index_b: [0; SIMD_WIDTH],
            inv_mass_a: FloatW::default(),
            inv_mass_b: FloatW::default(),
            inv_i_a: SymMatrix3W::default(),
            inv_i_b: SymMatrix3W::default(),
            normal: Vec3W::default(),
            tangent1: Vec3W::default(),
            tangent2: Vec3W::default(),
            origin_a: Vec3W::default(),
            origin_b: Vec3W::default(),
            twist_mass: FloatW::default(),
            twist_impulse: FloatW::default(),
            tangent_mass: SymMatrix2W::default(),
            friction_impulse: Vec2W::default(),
            rolling_mass: SymMatrix3W::default(),
            rolling_impulse: Vec3W::default(),
            friction: FloatW::default(),
            rolling_resistance: FloatW::default(),
            tangent_velocity1: FloatW::default(),
            tangent_velocity2: FloatW::default(),
            bias_rate: FloatW::default(),
            mass_scale: FloatW::default(),
            impulse_scale: FloatW::default(),
            restitution: FloatW::default(),
            contact_ids: [NULL_INDEX; SIMD_WIDTH],
            points: [ContactConstraintPointWide::default(); MAX_MANIFOLD_POINTS],
        }
    }
}

pub fn get_wide_contact_constraint_byte_count() -> i32 {
    std::mem::size_of::<ContactConstraintWide>() as i32
}

/// wide version of b3BodyState
#[derive(Clone, Copy, Debug, Default)]
struct BodyStateW {
    v: Vec3W,
    w: Vec3W,
    dp: Vec3W,
    dq: QuatW,
}

// B3_SIMD_NONE gather
fn gather_bodies(states: &[BodyState], indices: &[i32; SIMD_WIDTH]) -> BodyStateW {
    let identity = IDENTITY_BODY_STATE;

    let s1 = if indices[0] == 0 { identity } else { states[(indices[0] - 1) as usize] };
    let s2 = if indices[1] == 0 { identity } else { states[(indices[1] - 1) as usize] };
    let s3 = if indices[2] == 0 { identity } else { states[(indices[2] - 1) as usize] };
    let s4 = if indices[3] == 0 { identity } else { states[(indices[3] - 1) as usize] };

    let mut simd_body = BodyStateW::default();
    simd_body.v.x = FloatW { x: s1.linear_velocity.x, y: s2.linear_velocity.x, z: s3.linear_velocity.x, w: s4.linear_velocity.x };
    simd_body.v.y = FloatW { x: s1.linear_velocity.y, y: s2.linear_velocity.y, z: s3.linear_velocity.y, w: s4.linear_velocity.y };
    simd_body.v.z = FloatW { x: s1.linear_velocity.z, y: s2.linear_velocity.z, z: s3.linear_velocity.z, w: s4.linear_velocity.z };
    simd_body.w.x = FloatW { x: s1.angular_velocity.x, y: s2.angular_velocity.x, z: s3.angular_velocity.x, w: s4.angular_velocity.x };
    simd_body.w.y = FloatW { x: s1.angular_velocity.y, y: s2.angular_velocity.y, z: s3.angular_velocity.y, w: s4.angular_velocity.y };
    simd_body.w.z = FloatW { x: s1.angular_velocity.z, y: s2.angular_velocity.z, z: s3.angular_velocity.z, w: s4.angular_velocity.z };
    simd_body.dp.x = FloatW { x: s1.delta_position.x, y: s2.delta_position.x, z: s3.delta_position.x, w: s4.delta_position.x };
    simd_body.dp.y = FloatW { x: s1.delta_position.y, y: s2.delta_position.y, z: s3.delta_position.y, w: s4.delta_position.y };
    simd_body.dp.z = FloatW { x: s1.delta_position.z, y: s2.delta_position.z, z: s3.delta_position.z, w: s4.delta_position.z };
    simd_body.dq.v.x = FloatW { x: s1.delta_rotation.v.x, y: s2.delta_rotation.v.x, z: s3.delta_rotation.v.x, w: s4.delta_rotation.v.x };
    simd_body.dq.v.y = FloatW { x: s1.delta_rotation.v.y, y: s2.delta_rotation.v.y, z: s3.delta_rotation.v.y, w: s4.delta_rotation.v.y };
    simd_body.dq.v.z = FloatW { x: s1.delta_rotation.v.z, y: s2.delta_rotation.v.z, z: s3.delta_rotation.v.z, w: s4.delta_rotation.v.z };
    simd_body.dq.s = FloatW { x: s1.delta_rotation.s, y: s2.delta_rotation.s, z: s3.delta_rotation.s, w: s4.delta_rotation.s };

    simd_body
}

// This writes only the velocities back to the solver bodies.
// B3_SIMD_NONE scatter: note the scalar C path does not apply the lock flags
// here (the SSE2/NEON path does); locks are enforced during integration.
fn scatter_bodies(states: &mut [BodyState], indices: &[i32; SIMD_WIDTH], simd_body: &BodyStateW) {
    let index1 = indices[0] - 1;
    if index1 != -1 && (states[index1 as usize].flags & DYNAMIC_FLAG) != 0 {
        let state = &mut states[index1 as usize];
        state.linear_velocity.x = simd_body.v.x.x;
        state.linear_velocity.y = simd_body.v.y.x;
        state.linear_velocity.z = simd_body.v.z.x;
        state.angular_velocity.x = simd_body.w.x.x;
        state.angular_velocity.y = simd_body.w.y.x;
        state.angular_velocity.z = simd_body.w.z.x;
    }

    let index2 = indices[1] - 1;
    if index2 != -1 && (states[index2 as usize].flags & DYNAMIC_FLAG) != 0 {
        let state = &mut states[index2 as usize];
        state.linear_velocity.x = simd_body.v.x.y;
        state.linear_velocity.y = simd_body.v.y.y;
        state.linear_velocity.z = simd_body.v.z.y;
        state.angular_velocity.x = simd_body.w.x.y;
        state.angular_velocity.y = simd_body.w.y.y;
        state.angular_velocity.z = simd_body.w.z.y;
    }

    let index3 = indices[2] - 1;
    if index3 != -1 && (states[index3 as usize].flags & DYNAMIC_FLAG) != 0 {
        let state = &mut states[index3 as usize];
        state.linear_velocity.x = simd_body.v.x.z;
        state.linear_velocity.y = simd_body.v.y.z;
        state.linear_velocity.z = simd_body.v.z.z;
        state.angular_velocity.x = simd_body.w.x.z;
        state.angular_velocity.y = simd_body.w.y.z;
        state.angular_velocity.z = simd_body.w.z.z;
    }

    let index4 = indices[3] - 1;
    if index4 != -1 && (states[index4 as usize].flags & DYNAMIC_FLAG) != 0 {
        let state = &mut states[index4 as usize];
        state.linear_velocity.x = simd_body.v.x.w;
        state.linear_velocity.y = simd_body.v.y.w;
        state.linear_velocity.z = simd_body.v.z.w;
        state.angular_velocity.x = simd_body.w.x.w;
        state.angular_velocity.y = simd_body.w.y.w;
        state.angular_velocity.z = simd_body.w.z.w;
    }
}

// Prepare convex contact constraints
pub fn prepare_contacts_convex(block: SolverBlock, world: &mut World, context: &mut StepContext) {
    let world = &*world;

    // Stiffer for static contacts to avoid bodies getting pushed through the ground
    let contact_softness = context.contact_softness;
    let static_softness = context.static_softness;

    let warm_start_scale = if world.enable_warm_starting { 1.0 } else { 0.0 };

    let ctx = &mut *context;
    let sims = &ctx.sims;
    let states = &ctx.states;
    let wide_base = &mut ctx.wide_constraints;
    let spans = &ctx.wide_prepare_spans;

    let mut wide_index = block.start_index;
    let end_wide_index = block.start_index + block.count as i32;

    // Find color for start index. Linear search but fast.
    let mut color_index = 0usize;
    while spans[color_index + 1].start <= wide_index {
        color_index += 1;
    }

    // Loop over block
    while wide_index < end_wide_index {
        let color_wide_start = spans[color_index].start;
        let color_wide_end_index = min_int(spans[color_index + 1].start, end_wide_index);
        let color_contact_count = spans[color_index].count;
        let contacts_color = spans[color_index].color_index as usize;

        // Loop over color
        while wide_index < color_wide_end_index {
            let constraint = &mut wide_base[wide_index as usize];
            let local_wide_index = wide_index - color_wide_start;

            for lane in 0..SIMD_WIDTH {
                let contact_index = SIMD_WIDTH as i32 * local_wide_index + lane as i32;
                if contact_index >= color_contact_count {
                    // Remainder lanes were zeroed in solver setup.
                    break;
                }

                let contact_id = world.constraint_graph.colors[contacts_color].convex_contacts[contact_index as usize];
                let contact = &world.contacts[contact_id as usize];
                b3_assert!(contact.manifold_count() == 1);
                let manifold = &contact.manifolds[0];

                let index_a = contact.body_sim_index_a;
                let index_b = contact.body_sim_index_b;

                if cfg!(debug_assertions) {
                    let body_a = &world.bodies[contact.edges[0].body_id as usize];
                    let valid_index_a =
                        if body_a.set_index == crate::physics_world::AWAKE_SET { body_a.local_index } else { NULL_INDEX };
                    let body_b = &world.bodies[contact.edges[1].body_id as usize];
                    let valid_index_b =
                        if body_b.set_index == crate::physics_world::AWAKE_SET { body_b.local_index } else { NULL_INDEX };
                    b3_assert!(index_a == valid_index_a);
                    b3_assert!(index_b == valid_index_b);
                }

                // 0 for null
                constraint.index_a[lane] = index_a + 1;
                constraint.index_b[lane] = index_b + 1;
                constraint.contact_ids[lane] = contact_id;

                // Body A data
                let m_a;
                let i_a;
                let v_a;
                let w_a;

                if index_a == NULL_INDEX {
                    m_a = 0.0;
                    i_a = Matrix3::ZERO;
                    v_a = Vec3::ZERO;
                    w_a = Vec3::ZERO;
                } else {
                    let sim_a = &sims[index_a as usize];
                    m_a = sim_a.inv_mass;
                    i_a = sim_a.inv_inertia_world;

                    let state_a = &states[index_a as usize];
                    v_a = state_a.linear_velocity;
                    w_a = state_a.angular_velocity;
                }

                // Body B data
                let m_b;
                let i_b;
                let v_b;
                let w_b;

                if index_b == NULL_INDEX {
                    m_b = 0.0;
                    i_b = Matrix3::ZERO;
                    v_b = Vec3::ZERO;
                    w_b = Vec3::ZERO;
                } else {
                    let sim_b = &sims[index_b as usize];
                    m_b = sim_b.inv_mass;
                    i_b = sim_b.inv_inertia_world;

                    let state_b = &states[index_b as usize];
                    v_b = state_b.linear_velocity;
                    w_b = state_b.angular_velocity;
                }

                constraint.inv_mass_a.set(lane, m_a);
                constraint.inv_mass_b.set(lane, m_b);

                constraint.inv_i_a.cxx.set(lane, i_a.cx.x);
                constraint.inv_i_a.cxy.set(lane, i_a.cx.y);
                constraint.inv_i_a.cxz.set(lane, i_a.cx.z);
                constraint.inv_i_a.cyy.set(lane, i_a.cy.y);
                constraint.inv_i_a.cyz.set(lane, i_a.cy.z);
                constraint.inv_i_a.czz.set(lane, i_a.cz.z);

                constraint.inv_i_b.cxx.set(lane, i_b.cx.x);
                constraint.inv_i_b.cxy.set(lane, i_b.cx.y);
                constraint.inv_i_b.cxz.set(lane, i_b.cx.z);
                constraint.inv_i_b.cyy.set(lane, i_b.cy.y);
                constraint.inv_i_b.cyz.set(lane, i_b.cy.z);
                constraint.inv_i_b.czz.set(lane, i_b.cz.z);

                let soft = if index_a == NULL_INDEX || index_b == NULL_INDEX { static_softness } else { contact_softness };

                let normal = manifold.normal;
                constraint.normal.x.set(lane, normal.x);
                constraint.normal.y.set(lane, normal.y);
                constraint.normal.z.set(lane, normal.z);

                let tangent1 = perp(normal);
                constraint.tangent1.x.set(lane, tangent1.x);
                constraint.tangent1.y.set(lane, tangent1.y);
                constraint.tangent1.z.set(lane, tangent1.z);

                let tangent2 = cross(tangent1, normal);
                constraint.tangent2.x.set(lane, tangent2.x);
                constraint.tangent2.y.set(lane, tangent2.y);
                constraint.tangent2.z.set(lane, tangent2.z);

                constraint.friction.set(lane, contact.friction);
                constraint.restitution.set(lane, contact.restitution);
                constraint.rolling_resistance.set(lane, contact.rolling_resistance);

                constraint.tangent_velocity1.set(lane, dot(contact.tangent_velocity, tangent1));
                constraint.tangent_velocity2.set(lane, dot(contact.tangent_velocity, tangent2));

                constraint.bias_rate.set(lane, soft.bias_rate);
                constraint.mass_scale.set(lane, soft.mass_scale);
                constraint.impulse_scale.set(lane, soft.impulse_scale);

                let point_count = manifold.point_count;
                let mut origin_a = Vec3::ZERO;
                let mut origin_b = Vec3::ZERO;

                for point_index in 0..point_count {
                    let mp = &manifold.points[point_index as usize];
                    let cp = &mut constraint.points[point_index as usize];

                    let r_a = mp.anchor_a;
                    let r_b = mp.anchor_b;
                    origin_a = add(origin_a, r_a);
                    origin_b = add(origin_b, r_b);

                    cp.anchor_as.x.set(lane, r_a.x);
                    cp.anchor_as.y.set(lane, r_a.y);
                    cp.anchor_as.z.set(lane, r_a.z);

                    cp.anchor_bs.x.set(lane, r_b.x);
                    cp.anchor_bs.y.set(lane, r_b.y);
                    cp.anchor_bs.z.set(lane, r_b.z);

                    let base_separation = mp.separation - dot(sub(r_b, r_a), normal);
                    cp.base_separations.set(lane, base_separation);

                    cp.normal_impulses.set(lane, warm_start_scale * mp.normal_impulse);
                    cp.total_normal_impulses.set(lane, 0.0);

                    let rn_a = cross(r_a, normal);
                    let rn_b = cross(r_b, normal);
                    let k_normal = m_a + m_b + dot(rn_a, mul_mv(i_a, rn_a)) + dot(rn_b, mul_mv(i_b, rn_b));
                    cp.normal_masses.set(lane, if k_normal > 0.0 { 1.0 / k_normal } else { 0.0 });

                    // Save relative velocity for restitution
                    let vr_a = add(v_a, cross(w_a, r_a));
                    let vr_b = add(v_b, cross(w_b, r_b));
                    cp.relative_velocities.set(lane, dot(normal, sub(vr_b, vr_a)));
                }

                let inv_count = 1.0 / point_count as f32;
                origin_a = mul_sv(inv_count, origin_a);
                origin_b = mul_sv(inv_count, origin_b);

                constraint.origin_a.x.set(lane, origin_a.x);
                constraint.origin_a.y.set(lane, origin_a.y);
                constraint.origin_a.z.set(lane, origin_a.z);
                constraint.origin_b.x.set(lane, origin_b.x);
                constraint.origin_b.y.set(lane, origin_b.y);
                constraint.origin_b.z.set(lane, origin_b.z);

                for point_index in 0..point_count {
                    let mp = &manifold.points[point_index as usize];
                    let cp = &mut constraint.points[point_index as usize];
                    cp.lever_arms.set(lane, distance(mp.anchor_a, origin_a));
                }

                let rt_a1 = cross(origin_a, tangent1);
                let rt_a2 = cross(origin_a, tangent2);

                let rt_b1 = cross(origin_b, tangent1);
                let rt_b2 = cross(origin_b, tangent2);

                {
                    let mut k = Matrix2::default();
                    k.cx.x = m_a + m_b + dot(rt_a1, mul_mv(i_a, rt_a1)) + dot(rt_b1, mul_mv(i_b, rt_b1));
                    k.cy.y = m_a + m_b + dot(rt_a2, mul_mv(i_a, rt_a2)) + dot(rt_b2, mul_mv(i_b, rt_b2));
                    k.cx.y = dot(rt_a1, mul_mv(i_a, rt_a2)) + dot(rt_b1, mul_mv(i_b, rt_b2));
                    k.cy.x = k.cx.y;
                    let tangent_mass = invert2(k);

                    constraint.tangent_mass.cxx.set(lane, tangent_mass.cx.x);
                    constraint.tangent_mass.cxy.set(lane, tangent_mass.cx.y);
                    constraint.tangent_mass.cyy.set(lane, tangent_mass.cy.y);

                    constraint.friction_impulse.x.set(lane, warm_start_scale * dot(manifold.friction_impulse, tangent1));
                    constraint.friction_impulse.y.set(lane, warm_start_scale * dot(manifold.friction_impulse, tangent2));
                }

                {
                    let k = dot(normal, mul_mv(add_mm(i_a, i_b), normal));
                    constraint.twist_mass.set(lane, if k > 0.0 { 1.0 / k } else { 0.0 });
                    constraint.twist_impulse.set(lane, warm_start_scale * manifold.twist_impulse);
                }

                {
                    let rolling_mass = invert_matrix(add_mm(i_a, i_b));

                    constraint.rolling_mass.cxx.set(lane, rolling_mass.cx.x);
                    constraint.rolling_mass.cxy.set(lane, rolling_mass.cx.y);
                    constraint.rolling_mass.cxz.set(lane, rolling_mass.cx.z);
                    constraint.rolling_mass.cyy.set(lane, rolling_mass.cy.y);
                    constraint.rolling_mass.cyz.set(lane, rolling_mass.cy.z);
                    constraint.rolling_mass.czz.set(lane, rolling_mass.cz.z);

                    constraint.rolling_impulse.x.set(lane, warm_start_scale * manifold.rolling_impulse.x);
                    constraint.rolling_impulse.y.set(lane, warm_start_scale * manifold.rolling_impulse.y);
                    constraint.rolling_impulse.z.set(lane, warm_start_scale * manifold.rolling_impulse.z);
                }

                // zero remaining points
                for point_index in point_count..MAX_MANIFOLD_POINTS as i32 {
                    let cp = &mut constraint.points[point_index as usize];
                    cp.anchor_as.x.set(lane, 0.0);
                    cp.anchor_as.y.set(lane, 0.0);
                    cp.anchor_as.z.set(lane, 0.0);
                    cp.anchor_bs.x.set(lane, 0.0);
                    cp.anchor_bs.y.set(lane, 0.0);
                    cp.anchor_bs.z.set(lane, 0.0);
                    cp.base_separations.set(lane, 0.0);
                    cp.normal_impulses.set(lane, 0.0);
                    cp.total_normal_impulses.set(lane, 0.0);
                    cp.normal_masses.set(lane, 0.0);
                    cp.relative_velocities.set(lane, 0.0);
                    cp.lever_arms.set(lane, 0.0);
                }
            }

            wide_index += 1;
        }

        // Advance to next color
        color_index += 1;
    }
}

pub fn warm_start_contacts_convex(block: SolverBlock, world: &mut World, context: &mut StepContext) {
    let world = &*world;
    let wide_start = world.constraint_graph.colors[block.color_index as usize].wide_constraint_start;

    let ctx = &mut *context;
    let states = &mut ctx.states;
    let constraints = &mut ctx.wide_constraints;

    for i in block.start_index..block.start_index + block.count as i32 {
        let c = &mut constraints[(wide_start + i) as usize];
        let mut b_a = gather_bodies(states, &c.index_a);
        let mut b_b = gather_bodies(states, &c.index_b);

        // Normal impulses
        for j in 0..MAX_MANIFOLD_POINTS {
            let cp = &c.points[j];

            let r_a = cp.anchor_as;
            let r_b = cp.anchor_bs;

            let impulse = Vec3W {
                x: mul_w(cp.normal_impulses, c.normal.x),
                y: mul_w(cp.normal_impulses, c.normal.y),
                z: mul_w(cp.normal_impulses, c.normal.z),
            };

            b_a.w = mul_sub_mvw(b_a.w, c.inv_i_a, cross_w(r_a, impulse));
            b_a.v = mul_sub_svw(b_a.v, c.inv_mass_a, impulse);
            b_b.w = mul_add_mvw(b_b.w, c.inv_i_b, cross_w(r_b, impulse));
            b_b.v = mul_add_svw(b_b.v, c.inv_mass_b, impulse);
        }

        // Central friction
        {
            let r_a = c.origin_a;
            let r_b = c.origin_b;
            let mut impulse = mul_svw(c.friction_impulse.x, c.tangent1);
            impulse = mul_add_svw(impulse, c.friction_impulse.y, c.tangent2);

            b_a.w = mul_sub_mvw(b_a.w, c.inv_i_a, cross_w(r_a, impulse));
            b_a.v = mul_sub_svw(b_a.v, c.inv_mass_a, impulse);
            b_b.w = mul_add_mvw(b_b.w, c.inv_i_b, cross_w(r_b, impulse));
            b_b.v = mul_add_svw(b_b.v, c.inv_mass_b, impulse);
        }

        // Central twist friction
        {
            let impulse = mul_svw(c.twist_impulse, c.normal);
            b_a.w = mul_sub_mvw(b_a.w, c.inv_i_a, impulse);
            b_b.w = mul_add_mvw(b_b.w, c.inv_i_b, impulse);
        }

        // Rolling resistance
        {
            let impulse = c.rolling_impulse;
            b_a.w = mul_sub_mvw(b_a.w, c.inv_i_a, impulse);
            b_b.w = mul_add_mvw(b_b.w, c.inv_i_b, impulse);
        }

        scatter_bodies(states, &c.index_a, &b_a);
        scatter_bodies(states, &c.index_b, &b_b);
    }
}

pub fn solve_contacts_convex(block: SolverBlock, world: &mut World, context: &mut StepContext, use_bias: bool) {
    let world = &*world;
    let wide_start = world.constraint_graph.colors[block.color_index as usize].wide_constraint_start;

    let inv_h = splat_w(context.inv_h);
    let contact_speed = splat_w(-world.contact_speed);
    let one_w = splat_w(1.0);
    let epsilon_w = splat_w(f32::EPSILON);

    let ctx = &mut *context;
    let states = &mut ctx.states;
    let constraints = &mut ctx.wide_constraints;

    for wide_index in block.start_index..block.start_index + block.count as i32 {
        let c = &mut constraints[(wide_start + wide_index) as usize];

        let mut b_a = gather_bodies(states, &c.index_a);
        let mut b_b = gather_bodies(states, &c.index_b);

        let bias_rate;
        let mass_scale;
        let impulse_scale;
        if use_bias {
            bias_rate = mul_w(c.mass_scale, c.bias_rate);
            mass_scale = c.mass_scale;
            impulse_scale = c.impulse_scale;
        } else {
            bias_rate = zero_w();
            mass_scale = one_w;
            impulse_scale = zero_w();
        }

        let dp = sub_vw(b_b.dp, b_a.dp);

        let mut total_normal_impulse = zero_w();
        let mut total_twist_limit = zero_w();

        for point_index in 0..MAX_MANIFOLD_POINTS {
            let cp = &mut c.points[point_index];

            // Fixed anchor points for applying impulses
            let r_a = cp.anchor_as;
            let r_b = cp.anchor_bs;

            // Moving anchors for current separation
            let rs_a = rotate_vector_w(b_a.dq, r_a);
            let rs_b = rotate_vector_w(b_b.dq, r_b);

            // compute current separation
            // this is subject to round-off error if the anchor is far from the body center of mass
            let ds = add_vw(dp, sub_vw(rs_b, rs_a));
            let s = add_w(dot_w(c.normal, ds), cp.base_separations);

            // Apply speculative bias if separation is greater than zero, otherwise apply soft constraint bias
            let mask = greater_than_w(s, zero_w());
            let spec_bias = mul_w(s, inv_h);
            let soft_bias = max_w(mul_w(bias_rate, s), contact_speed);
            let bias = blend_w(soft_bias, spec_bias, mask);

            let point_mass_scale = blend_w(mass_scale, one_w, mask);
            let point_impulse_scale = blend_w(impulse_scale, zero_w(), mask);

            // Relative velocity at contact
            let vr_a = add_vw(b_a.v, cross_w(b_a.w, r_a));
            let vr_b = add_vw(b_b.v, cross_w(b_b.w, r_b));
            let vn = dot_w(sub_vw(vr_b, vr_a), c.normal);

            // Compute normal impulse
            let neg_impulse = add_w(
                mul_w(cp.normal_masses, add_w(mul_w(point_mass_scale, vn), bias)),
                mul_w(point_impulse_scale, cp.normal_impulses),
            );

            // Clamp the accumulated impulse
            let new_impulse = max_w(sub_w(cp.normal_impulses, neg_impulse), zero_w());
            let delta_impulse = sub_w(new_impulse, cp.normal_impulses);
            cp.normal_impulses = new_impulse;
            cp.total_normal_impulses = add_w(cp.total_normal_impulses, new_impulse);

            total_normal_impulse = add_w(total_normal_impulse, new_impulse);
            total_twist_limit = add_w(total_twist_limit, mul_w(cp.lever_arms, new_impulse));

            // Apply contact impulse
            let p = mul_svw(delta_impulse, c.normal);
            b_a.w = mul_sub_mvw(b_a.w, c.inv_i_a, cross_w(r_a, p));
            b_a.v = mul_sub_svw(b_a.v, c.inv_mass_a, p);
            b_b.w = mul_add_mvw(b_b.w, c.inv_i_b, cross_w(r_b, p));
            b_b.v = mul_add_svw(b_b.v, c.inv_mass_b, p);
        }

        // No friction when applying bias
        if !use_bias {
            // Rolling resistance
            if !all_zero_w(c.rolling_resistance) {
                // flip A/B order to negate
                let mut delta_impulse = mul_mvw(c.rolling_mass, sub_vw(b_a.w, b_b.w));
                let old_impulse = c.rolling_impulse;
                c.rolling_impulse = add_vw(old_impulse, delta_impulse);

                let max_impulse = mul_w(c.rolling_resistance, total_normal_impulse);
                let length_squared = dot_w(c.rolling_impulse, c.rolling_impulse);

                let mask = greater_than_w(length_squared, mul_add_w(epsilon_w, max_impulse, max_impulse));

                // No approximate _mm_rsqrt_ps here to maintain cross-platform determinism
                let normalize = div_w(max_impulse, add_w(sqrt_w(length_squared), epsilon_w));
                let mut scale = blend_w(one_w, normalize, mask);

                // Ensure zero rolling resistance yields no impulse
                let rolling_mask = greater_than_w(c.rolling_resistance, zero_w());
                scale = blend_w(zero_w(), scale, rolling_mask);

                c.rolling_impulse = mul_svw(scale, c.rolling_impulse);

                delta_impulse = sub_vw(c.rolling_impulse, old_impulse);

                b_a.w = mul_sub_mvw(b_a.w, c.inv_i_a, delta_impulse);
                b_b.w = mul_add_mvw(b_b.w, c.inv_i_b, delta_impulse);
            }

            // Central twist friction
            {
                let twist_speed = dot_w(c.normal, sub_vw(b_b.w, b_a.w));
                let max_lambda = mul_w(c.friction, total_twist_limit);
                let mut delta_impulse = neg_w(mul_w(c.twist_mass, twist_speed));
                let old_impulse = c.twist_impulse;
                c.twist_impulse = sym_clamp_w(add_w(old_impulse, delta_impulse), max_lambda);
                delta_impulse = sub_w(c.twist_impulse, old_impulse);

                let l = mul_svw(delta_impulse, c.normal);
                b_a.w = mul_sub_mvw(b_a.w, c.inv_i_a, l);
                b_b.w = mul_add_mvw(b_b.w, c.inv_i_b, l);
            }

            // Central friction
            {
                let tangent1 = c.tangent1;
                let tangent2 = c.tangent2;

                // Fixed anchor points for applying impulses
                let r_a = c.origin_a;
                let r_b = c.origin_b;

                // Relative tangent velocity at contact
                let vr_a = add_vw(b_a.v, cross_w(b_a.w, r_a));
                let vr_b = add_vw(b_b.v, cross_w(b_b.w, r_b));
                let vr = sub_vw(vr_b, vr_a);
                let vt = Vec2W {
                    x: sub_w(dot_w(vr, tangent1), c.tangent_velocity1),
                    y: sub_w(dot_w(vr, tangent2), c.tangent_velocity2),
                };

                // Incremental tangent impulse
                let mut delta_impulse = mul_mv2w(c.tangent_mass, vt);
                delta_impulse = Vec2W { x: neg_w(delta_impulse.x), y: neg_w(delta_impulse.y) };
                let mut new_impulse = add_v2w(c.friction_impulse, delta_impulse);

                let friction = c.friction;
                let max_impulse = mul_w(friction, total_normal_impulse);

                // Clamp the accumulated impulse
                let length_squared = add_w(mul_w(new_impulse.x, new_impulse.x), mul_w(new_impulse.y, new_impulse.y));

                // Max impulse can be zero
                let mask = greater_than_w(length_squared, mul_w(max_impulse, max_impulse));

                // No approximate _mm_rsqrt_ps here to maintain cross-platform determinism.
                // Add epsilon to avoid divide by zero.
                let normalize = div_w(max_impulse, add_w(sqrt_w(length_squared), epsilon_w));
                let scale = blend_w(one_w, normalize, mask);
                new_impulse = Vec2W {
                    x: mul_w(scale, new_impulse.x),
                    y: mul_w(scale, new_impulse.y),
                };

                delta_impulse = Vec2W {
                    x: sub_w(new_impulse.x, c.friction_impulse.x),
                    y: sub_w(new_impulse.y, c.friction_impulse.y),
                };

                c.friction_impulse = new_impulse;

                // Apply delta impulse
                let p = add_vw(mul_svw(delta_impulse.x, tangent1), mul_svw(delta_impulse.y, tangent2));
                b_a.w = mul_sub_mvw(b_a.w, c.inv_i_a, cross_w(r_a, p));
                b_a.v = mul_sub_svw(b_a.v, c.inv_mass_a, p);
                b_b.w = mul_add_mvw(b_b.w, c.inv_i_b, cross_w(r_b, p));
                b_b.v = mul_add_svw(b_b.v, c.inv_mass_b, p);
            }
        }

        scatter_bodies(states, &c.index_a, &b_a);
        scatter_bodies(states, &c.index_b, &b_b);
    }
}

pub fn apply_restitution_convex(block: SolverBlock, world: &mut World, context: &mut StepContext) {
    let world = &*world;
    let wide_start = world.constraint_graph.colors[block.color_index as usize].wide_constraint_start;

    let threshold = splat_w(world.restitution_threshold);
    let zero = zero_w();

    let ctx = &mut *context;
    let states = &mut ctx.states;
    let constraints = &mut ctx.wide_constraints;

    for i in block.start_index..block.start_index + block.count as i32 {
        let c = &mut constraints[(wide_start + i) as usize];

        if all_zero_w(c.restitution) {
            // No lanes have restitution. Common case.
            continue;
        }

        // Single gather for all manifolds
        let mut b_a = gather_bodies(states, &c.index_a);
        let mut b_b = gather_bodies(states, &c.index_b);

        // Create a mask based on restitution so that lanes with no restitution are not
        // affected by the calculations below.
        let restitution_mask = equals_w(c.restitution, zero);

        for point_index in 0..MAX_MANIFOLD_POINTS {
            let cp = &mut c.points[point_index];

            // Set effective mass to zero if restitution should not be applied
            let mask1 = greater_than_w(add_w(cp.relative_velocities, threshold), zero);
            let mask2 = equals_w(cp.total_normal_impulses, zero);
            let mask = or_w(or_w(mask1, mask2), restitution_mask);
            let mass = blend_w(cp.normal_masses, zero, mask);

            // Fixed anchors for impulses
            let r_a = cp.anchor_as;
            let r_b = cp.anchor_bs;

            // Relative velocity at contact
            let vr_a = add_vw(b_a.v, cross_w(b_a.w, r_a));
            let vr_b = add_vw(b_b.v, cross_w(b_b.w, r_b));
            let vn = dot_w(sub_vw(vr_b, vr_a), c.normal);

            // Compute normal impulse
            let neg_impulse = mul_w(mass, add_w(vn, mul_w(c.restitution, cp.relative_velocities)));

            // Clamp the accumulated impulse
            let new_impulse = max_w(sub_w(cp.normal_impulses, neg_impulse), zero_w());
            let delta_impulse = sub_w(new_impulse, cp.normal_impulses);
            cp.normal_impulses = new_impulse;
            cp.total_normal_impulses = add_w(cp.total_normal_impulses, delta_impulse);

            // Apply contact impulse
            let p = mul_svw(delta_impulse, c.normal);
            b_a.w = mul_sub_mvw(b_a.w, c.inv_i_a, cross_w(r_a, p));
            b_a.v = mul_sub_svw(b_a.v, c.inv_mass_a, p);
            b_b.w = mul_add_mvw(b_b.w, c.inv_i_b, cross_w(r_b, p));
            b_b.v = mul_add_svw(b_b.v, c.inv_mass_b, p);
        }

        scatter_bodies(states, &c.index_a, &b_a);
        scatter_bodies(states, &c.index_b, &b_b);
    }
}

// Store impulses by contact constraint
pub fn store_impulses_convex(block: SolverBlock, world: &mut World, context: &mut StepContext, worker_index: i32) {
    let ctx = &*context;
    let spans = &ctx.wide_prepare_spans;
    let wide_base = &ctx.wide_constraints;

    let mut has_hit_events = world.task_contexts[worker_index as usize].has_hit_events;
    let neg_hit_threshold = -world.hit_event_threshold;

    let mut wide_index = block.start_index;
    let end_wide_index = block.start_index + block.count as i32;

    // Find color for start index
    let mut color_index = 0usize;
    while spans[color_index + 1].start <= wide_index {
        color_index += 1;
    }

    while wide_index < end_wide_index {
        let color_wide_start = spans[color_index].start;
        let color_wide_end_index = min_int(spans[color_index + 1].start, end_wide_index);
        let color_contact_count = spans[color_index].count;
        let contacts_color = spans[color_index].color_index as usize;

        while wide_index < color_wide_end_index {
            let c = &wide_base[wide_index as usize];

            let local_wide_index = wide_index - color_wide_start;

            for lane in 0..SIMD_WIDTH {
                let contact_index = SIMD_WIDTH as i32 * local_wide_index + lane as i32;
                if contact_index >= color_contact_count {
                    break;
                }

                // C: b3Manifold* m = c->manifolds[lane]; NULL check for inert lanes.
                let manifold_contact_id = c.contact_ids[lane];
                if manifold_contact_id == NULL_INDEX {
                    continue;
                }

                let f1 = c.friction_impulse.x.get(lane);
                let f2 = c.friction_impulse.y.get(lane);

                let point_count;
                {
                    let m = &mut world.contacts[manifold_contact_id as usize].manifolds[0];
                    m.friction_impulse = Vec3 {
                        x: f1 * c.tangent1.x.get(lane) + f2 * c.tangent2.x.get(lane),
                        y: f1 * c.tangent1.y.get(lane) + f2 * c.tangent2.y.get(lane),
                        z: f1 * c.tangent1.z.get(lane) + f2 * c.tangent2.z.get(lane),
                    };
                    m.twist_impulse = c.twist_impulse.get(lane);
                    m.rolling_impulse = Vec3 {
                        x: c.rolling_impulse.x.get(lane),
                        y: c.rolling_impulse.y.get(lane),
                        z: c.rolling_impulse.z.get(lane),
                    };

                    point_count = m.point_count;
                    for point_index in 0..point_count {
                        let cp = &c.points[point_index as usize];
                        let mp = &mut m.points[point_index as usize];
                        mp.normal_impulse = cp.normal_impulses.get(lane);
                        mp.total_normal_impulse = cp.total_normal_impulses.get(lane);
                        mp.normal_velocity = cp.relative_velocities.get(lane);
                    }
                }

                let contact_id =
                    world.constraint_graph.colors[contacts_color].convex_contacts[contact_index as usize];
                let contact = &world.contacts[contact_id as usize];
                if (contact.flags & SIM_ENABLE_HIT_EVENT) != 0 {
                    let mut hit = false;
                    {
                        let m = &contact.manifolds[0];
                        for k in 0..point_count {
                            let mp = &m.points[k as usize];

                            // Need to check total impulse because the point may be speculative and not colliding
                            if mp.normal_velocity < neg_hit_threshold && mp.total_normal_impulse > 0.0 {
                                hit = true;
                                break;
                            }
                        }
                    }

                    if hit {
                        let contact_id = contact.contact_id;
                        let task_context = &mut world.task_contexts[worker_index as usize];
                        set_bit(&mut task_context.hit_event_bit_set, contact_id as u32);
                        has_hit_events = true;
                    }
                }
            }

            wide_index += 1;
        }

        color_index += 1;
    }

    world.task_contexts[worker_index as usize].has_hit_events = has_hit_events;
}

// ---------------------------------------------------------------------------
// Overflow wrappers: run the scalar (mesh) stages over the whole overflow range.
// ---------------------------------------------------------------------------

pub fn prepare_contacts_overflow(world: &mut World, context: &mut StepContext) {
    let count = world.constraint_graph.colors[OVERFLOW_INDEX as usize].contacts.len();
    if count == 0 {
        return;
    }
    b3_assert!(count <= u16::MAX as usize);

    let block = SolverBlock {
        start_index: 0,
        count: count as u16,
        block_type: SolverBlockType::Overflow,
        color_index: OVERFLOW_INDEX as u8,
    };

    prepare_contacts_mesh(block, world, context);
}

pub fn warm_start_contacts_overflow(world: &mut World, context: &mut StepContext) {
    let count = world.constraint_graph.colors[OVERFLOW_INDEX as usize].contacts.len();
    if count == 0 {
        return;
    }
    b3_assert!(count <= u16::MAX as usize);

    let block = SolverBlock {
        start_index: 0,
        count: count as u16,
        block_type: SolverBlockType::Overflow,
        color_index: OVERFLOW_INDEX as u8,
    };

    warm_start_contacts_mesh(block, world, context);
}

pub fn solve_contacts_overflow(world: &mut World, context: &mut StepContext, use_bias: bool) {
    let count = world.constraint_graph.colors[OVERFLOW_INDEX as usize].contacts.len();
    if count == 0 {
        return;
    }
    b3_assert!(count <= u16::MAX as usize);

    let block = SolverBlock {
        start_index: 0,
        count: count as u16,
        block_type: SolverBlockType::Overflow,
        color_index: OVERFLOW_INDEX as u8,
    };

    solve_contacts_mesh(block, world, context, use_bias);
}

pub fn apply_restitution_overflow(world: &mut World, context: &mut StepContext) {
    let count = world.constraint_graph.colors[OVERFLOW_INDEX as usize].contacts.len();
    if count == 0 {
        return;
    }
    b3_assert!(count <= u16::MAX as usize);

    let block = SolverBlock {
        start_index: 0,
        count: count as u16,
        block_type: SolverBlockType::Overflow,
        color_index: OVERFLOW_INDEX as u8,
    };

    apply_restitution_mesh(block, world, context);
}

pub fn store_impulses_overflow(world: &mut World, context: &mut StepContext) {
    let count = world.constraint_graph.colors[OVERFLOW_INDEX as usize].contacts.len();
    if count == 0 {
        return;
    }
    b3_assert!(count <= u16::MAX as usize);

    let block = SolverBlock {
        start_index: 0,
        count: count as u16,
        block_type: SolverBlockType::Overflow,
        color_index: OVERFLOW_INDEX as u8,
    };

    store_impulses_mesh(block, world, context, 0);
}

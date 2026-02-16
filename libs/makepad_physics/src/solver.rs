use crate::contact::ContactManifold;
use crate::rigid_body::RigidBody;
use makepad_math::*;

// Rapier-matching spring-damper contact softness parameters.
// natural_frequency = 30.0, damping_ratio = 5.0 (rapier defaults)
const CONTACT_NATURAL_FREQUENCY: f32 = 30.0;
const CONTACT_DAMPING_RATIO: f32 = 5.0;
/// Penetration the engine won't try to correct (rapier default: 0.001).
const ALLOWED_LINEAR_ERROR: f32 = 0.001;
/// Max velocity used for position correction (rapier default: 10.0).
const MAX_CORRECTIVE_VELOCITY: f32 = 10.0;

/// Ground body properties (infinite mass, fixed).
const GROUND_FRICTION: f32 = 0.5;
const GROUND_RESTITUTION: f32 = 0.0;
/// Only apply restitution for sufficiently fast impacts.
///
/// We don't currently persist contacts across frames, so this threshold prevents
/// resting contacts from being treated as "new bouncy contacts" every frame.
const RESTITUTION_VELOCITY_THRESHOLD: f32 = 1.0;

/// Compute erp_inv_dt from spring-damper parameters.
/// erp_inv_dt = angular_freq / (dt * angular_freq + 2 * damping_ratio)
fn erp_inv_dt(dt: f32) -> f32 {
    let ang_freq = CONTACT_NATURAL_FREQUENCY * std::f32::consts::TAU;
    ang_freq / (dt * ang_freq + 2.0 * CONTACT_DAMPING_RATIO)
}

/// Compute cfm_factor from spring-damper parameters.
/// cfm_factor = 1 / (1 + cfm_coeff)
fn cfm_factor(dt: f32) -> f32 {
    let erp_inv = erp_inv_dt(dt);
    let erp = dt * erp_inv;
    if erp == 0.0 {
        return 1.0;
    }
    let inv_erp_m1 = 1.0 / erp - 1.0;
    let cfm_coeff = inv_erp_m1 * inv_erp_m1
        / ((1.0 + inv_erp_m1) * 4.0 * CONTACT_DAMPING_RATIO * CONTACT_DAMPING_RATIO);
    1.0 / (1.0 + cfm_coeff)
}

/// Pre-computed constraint data for one contact point.
/// Follows rapier's ContactConstraintNormalPart / TangentPart layout.
#[derive(Clone, Copy, Default)]
pub struct SolverContact {
    /// Body indices. usize::MAX = ground/fixed.
    pub body_a: usize,
    pub body_b: usize,

    /// Force direction (= -contact_normal, pointing from B towards A).
    /// Matches rapier's `dir1 = -manifold.data.normal`.
    pub dir1: Vec3f,
    /// Inverse mass vectors (scalar, same for all components for uniform density).
    pub im1: f32,
    pub im2: f32,

    // Normal constraint
    pub torque_dir1: Vec3f,
    pub torque_dir2: Vec3f,
    pub ii_torque_dir1: Vec3f,
    pub ii_torque_dir2: Vec3f,
    pub r_normal: f32,
    pub rhs: f32,
    pub rhs_wo_bias: f32,
    pub impulse_normal: f32,

    // Tangent constraint (two directions for 3D)
    pub tangent1: Vec3f,
    pub tangent2: Vec3f,
    pub t1_torque_dir1: Vec3f,
    pub t1_torque_dir2: Vec3f,
    pub t1_ii_torque_dir1: Vec3f,
    pub t1_ii_torque_dir2: Vec3f,
    pub r_tangent1: f32,
    pub t2_torque_dir1: Vec3f,
    pub t2_torque_dir2: Vec3f,
    pub t2_ii_torque_dir1: Vec3f,
    pub t2_ii_torque_dir2: Vec3f,
    pub r_tangent2: f32,
    pub impulse_tangent1: f32,
    pub impulse_tangent2: f32,
    pub rhs_tangent1_wo_bias: f32,
    pub rhs_tangent2_wo_bias: f32,
    pub rhs_tangent1: f32,
    pub rhs_tangent2: f32,

    pub friction: f32,
    pub cfm_factor: f32,

    // Stored for bias recomputation during substepping (rapier's builder info pattern).
    // local_p1/local_p2 are contact points in body-local coordinates,
    // used to re-transform into world space after position integration.
    pub local_p1: Vec3f, // contact point in body A's local frame (zero for ground)
    pub local_p2: Vec3f, // contact point in body B's local frame (zero for ground)
    pub dist: f32,       // initial penetration distance (negative = penetration, rapier convention)
    pub normal_vel: f32, // restitution component
}

/// Build two tangent vectors perpendicular to a direction.
fn compute_tangents(dir: Vec3f) -> (Vec3f, Vec3f) {
    let reference = if dir.x.abs() < 0.9 {
        vec3f(1.0, 0.0, 0.0)
    } else {
        vec3f(0.0, 1.0, 0.0)
    };
    let t1 = Vec3f::cross(dir, reference).normalize();
    let t2 = Vec3f::cross(dir, t1);
    (t1, t2)
}

/// Build solver contacts from manifolds. Follows rapier's constraint generation.
///
/// Convention (matching rapier):
/// - `dir1 = -contact_normal` (force direction, points from body2 toward body1)
/// - `torque_dir1 = dp1 × dir1` (moment arm for body1)
/// - `torque_dir2 = dp2 × (-dir1)` (moment arm for body2, with negation baked in)
/// - `ii_torque_dir = inv_inertia * torque_dir` (pre-multiplied)
///
/// In the solver, relative velocity is v1 - v2 projected onto dir1.
/// Positive impulse pushes bodies apart.
pub fn prepare_contacts(
    bodies: &[RigidBody],
    manifolds: &[ContactManifold],
    dt: f32,
    solver_contacts: &mut Vec<SolverContact>,
) {
    solver_contacts.clear();
    let cfm = cfm_factor(dt);
    let erp_inv = erp_inv_dt(dt);
    let inv_dt = if dt > 0.0 { 1.0 / dt } else { 0.0 };

    for manifold in manifolds.iter() {
        let a_idx = manifold.body_a;
        let b_idx = manifold.body_b;

        // Gather body properties
        let (im1, inv_i1, rest1, fric1, linvel1, angvel1) = body_props(bodies, a_idx);
        let (im2, inv_i2, rest2, fric2, linvel2, angvel2) = body_props(bodies, b_idx);
        let restitution = rest1.min(rest2);
        let friction = (fric1 * fric2).sqrt();

        for pi in 0..manifold.num_points {
            let point = &manifold.points[pi];

            // Contact normal points from A to B. Rapier's force_dir1 = -normal.
            let normal = point.normal;
            let dir1 = -normal;

            // Moment arms from center of mass to contact point
            let dp1 = point.offset_a; // contact_world - body_a_position
            let dp2 = point.offset_b; // contact_world - body_b_position

            // Rapier convention: torque_dir2 uses -dir1 (= normal)
            let torque_dir1 = Vec3f::cross(dp1, dir1);
            let torque_dir2 = Vec3f::cross(dp2, -dir1);
            let ii_torque_dir1 = inv_i1.mul_vec3(torque_dir1);
            let ii_torque_dir2 = inv_i2.mul_vec3(torque_dir2);

            // Effective mass (projected mass)
            let inv_r =
                im1 + im2 + ii_torque_dir1.dot(torque_dir1) + ii_torque_dir2.dot(torque_dir2);
            let r_normal = if inv_r > 0.0 { 1.0 / inv_r } else { 0.0 };

            // Initial relative velocity along force direction (v1 - v2) · dir1
            let vel1 = linvel1 + Vec3f::cross(angvel1, dp1);
            let vel2 = linvel2 + Vec3f::cross(angvel2, dp2);
            let projected_velocity = (vel1 - vel2).dot(dir1);

            // Restitution.
            // Approximation of rapier's "is_bouncy" rule: only apply restitution
            // for sufficiently fast approaching impacts.
            let normal_vel = if projected_velocity < -RESTITUTION_VELOCITY_THRESHOLD {
                restitution * projected_velocity
            } else {
                0.0
            };

            // Distance: negative = penetration. Our manifold stores positive penetration.
            let dist = -point.penetration;

            // RHS bias (rapier formula)
            let rhs_wo_bias = normal_vel + dist.max(0.0) * inv_dt;
            let rhs_bias =
                ((dist + ALLOWED_LINEAR_ERROR) * erp_inv).clamp(-MAX_CORRECTIVE_VELOCITY, 0.0);
            let rhs = rhs_wo_bias + rhs_bias;

            // Tangent directions (perpendicular to dir1, same as rapier)
            let (tangent1, tangent2) = compute_tangents(dir1);

            // Tangent 1 constraint setup
            let t1_torque_dir1 = Vec3f::cross(dp1, tangent1);
            let t1_torque_dir2 = Vec3f::cross(dp2, -tangent1);
            let t1_ii_torque_dir1 = inv_i1.mul_vec3(t1_torque_dir1);
            let t1_ii_torque_dir2 = inv_i2.mul_vec3(t1_torque_dir2);
            let inv_r_t1 = im1
                + im2
                + t1_ii_torque_dir1.dot(t1_torque_dir1)
                + t1_ii_torque_dir2.dot(t1_torque_dir2);
            let r_tangent1 = if inv_r_t1 > 0.0 { 1.0 / inv_r_t1 } else { 0.0 };

            // Tangent 2 constraint setup
            let t2_torque_dir1 = Vec3f::cross(dp1, tangent2);
            let t2_torque_dir2 = Vec3f::cross(dp2, -tangent2);
            let t2_ii_torque_dir1 = inv_i1.mul_vec3(t2_torque_dir1);
            let t2_ii_torque_dir2 = inv_i2.mul_vec3(t2_torque_dir2);
            let inv_r_t2 = im1
                + im2
                + t2_ii_torque_dir1.dot(t2_torque_dir1)
                + t2_ii_torque_dir2.dot(t2_torque_dir2);
            let r_tangent2 = if inv_r_t2 > 0.0 { 1.0 / inv_r_t2 } else { 0.0 };

            // Compute body-local contact points for TGS substep updates.
            // After position integration in each substep, we re-transform these
            // to world space to recompute penetration depth (matching rapier).
            // For ground (usize::MAX), store world-space point (ground doesn't move).
            let local_p1 = if a_idx == usize::MAX {
                point.world_point
            } else {
                bodies[a_idx]
                    .pose
                    .invert()
                    .transform_vec3(&point.world_point)
            };
            let local_p2 = if b_idx == usize::MAX {
                point.world_point
            } else {
                bodies[b_idx]
                    .pose
                    .invert()
                    .transform_vec3(&point.world_point)
            };

            solver_contacts.push(SolverContact {
                body_a: a_idx,
                body_b: b_idx,
                dir1,
                im1,
                im2,
                torque_dir1,
                torque_dir2,
                ii_torque_dir1,
                ii_torque_dir2,
                r_normal,
                rhs,
                rhs_wo_bias,
                impulse_normal: 0.0,
                tangent1,
                tangent2,
                t1_torque_dir1,
                t1_torque_dir2,
                t1_ii_torque_dir1,
                t1_ii_torque_dir2,
                r_tangent1,
                t2_torque_dir1,
                t2_torque_dir2,
                t2_ii_torque_dir1,
                t2_ii_torque_dir2,
                r_tangent2,
                impulse_tangent1: 0.0,
                impulse_tangent2: 0.0,
                rhs_tangent1_wo_bias: 0.0,
                rhs_tangent2_wo_bias: 0.0,
                rhs_tangent1: 0.0,
                rhs_tangent2: 0.0,
                friction,
                cfm_factor: cfm,
                local_p1,
                local_p2,
                dist,
                normal_vel,
            });
        }
    }
}

/// Update solver contacts after position integration (for TGS substepping).
/// This is the key to making TGS work: re-transform stored local contact points
/// using current body poses to recompute penetration depth, moment arms, and
/// effective masses. Matches rapier's `contact_constraints.update()`.
pub fn update_contacts(bodies: &[RigidBody], solver_contacts: &mut [SolverContact], dt: f32) {
    let cfm = cfm_factor(dt);
    let erp_inv = erp_inv_dt(dt);
    let inv_dt = if dt > 0.0 { 1.0 / dt } else { 0.0 };

    for sc in solver_contacts.iter_mut() {
        // Re-transform local contact points to world space using current poses
        let p1 = if sc.body_a == usize::MAX {
            // Ground: contact point stays at its world position (local_p1 is world pos)
            sc.local_p1
        } else {
            bodies[sc.body_a].pose.transform_vec3(&sc.local_p1)
        };
        let p2 = if sc.body_b == usize::MAX {
            sc.local_p2
        } else {
            bodies[sc.body_b].pose.transform_vec3(&sc.local_p2)
        };

        // Recompute penetration depth along the contact normal direction
        // dist = initial_dist + (p1 - p2) · dir1
        // This captures how much the bodies have moved toward/apart from each other
        let dist = sc.dist + (p1 - p2).dot(sc.dir1);

        // Recompute moment arms from current body centers
        let dp1 = if sc.body_a == usize::MAX {
            Vec3f::default()
        } else {
            p1 - bodies[sc.body_a].pose.position
        };
        let dp2 = if sc.body_b == usize::MAX {
            Vec3f::default()
        } else {
            p2 - bodies[sc.body_b].pose.position
        };

        // Recompute world-space inverse inertia tensors
        let inv_i1 = if sc.body_a == usize::MAX {
            Mat3f::zero()
        } else {
            bodies[sc.body_a].world_inv_inertia()
        };
        let inv_i2 = if sc.body_b == usize::MAX {
            Mat3f::zero()
        } else {
            bodies[sc.body_b].world_inv_inertia()
        };

        // Recompute normal constraint geometry
        sc.torque_dir1 = Vec3f::cross(dp1, sc.dir1);
        sc.torque_dir2 = Vec3f::cross(dp2, -sc.dir1);
        sc.ii_torque_dir1 = inv_i1.mul_vec3(sc.torque_dir1);
        sc.ii_torque_dir2 = inv_i2.mul_vec3(sc.torque_dir2);
        let inv_r = sc.im1
            + sc.im2
            + sc.ii_torque_dir1.dot(sc.torque_dir1)
            + sc.ii_torque_dir2.dot(sc.torque_dir2);
        sc.r_normal = if inv_r > 0.0 { 1.0 / inv_r } else { 0.0 };

        // Recompute tangent constraint geometry
        sc.t1_torque_dir1 = Vec3f::cross(dp1, sc.tangent1);
        sc.t1_torque_dir2 = Vec3f::cross(dp2, -sc.tangent1);
        sc.t1_ii_torque_dir1 = inv_i1.mul_vec3(sc.t1_torque_dir1);
        sc.t1_ii_torque_dir2 = inv_i2.mul_vec3(sc.t1_torque_dir2);
        let inv_r_t1 = sc.im1
            + sc.im2
            + sc.t1_ii_torque_dir1.dot(sc.t1_torque_dir1)
            + sc.t1_ii_torque_dir2.dot(sc.t1_torque_dir2);
        sc.r_tangent1 = if inv_r_t1 > 0.0 { 1.0 / inv_r_t1 } else { 0.0 };

        sc.t2_torque_dir1 = Vec3f::cross(dp1, sc.tangent2);
        sc.t2_torque_dir2 = Vec3f::cross(dp2, -sc.tangent2);
        sc.t2_ii_torque_dir1 = inv_i1.mul_vec3(sc.t2_torque_dir1);
        sc.t2_ii_torque_dir2 = inv_i2.mul_vec3(sc.t2_torque_dir2);
        let inv_r_t2 = sc.im1
            + sc.im2
            + sc.t2_ii_torque_dir1.dot(sc.t2_torque_dir1)
            + sc.t2_ii_torque_dir2.dot(sc.t2_torque_dir2);
        sc.r_tangent2 = if inv_r_t2 > 0.0 { 1.0 / inv_r_t2 } else { 0.0 };

        // Recompute RHS bias terms
        let rhs_wo_bias = sc.normal_vel + dist.max(0.0) * inv_dt;
        let rhs_bias =
            ((dist + ALLOWED_LINEAR_ERROR) * erp_inv).clamp(-MAX_CORRECTIVE_VELOCITY, 0.0);
        sc.rhs = rhs_wo_bias + rhs_bias;
        sc.rhs_wo_bias = rhs_wo_bias;
        sc.cfm_factor = cfm;

        // Recompute tangent RHS from tangential drift (rapier update step).
        let tangent_bias1 = (p1 - p2).dot(sc.tangent1) * inv_dt;
        let tangent_bias2 = (p1 - p2).dot(sc.tangent2) * inv_dt;
        sc.rhs_tangent1 = sc.rhs_tangent1_wo_bias + tangent_bias1;
        sc.rhs_tangent2 = sc.rhs_tangent2_wo_bias + tangent_bias2;

        // Note: impulse scaling (warmstart coefficient) is handled by the
        // separate warmstart() call in the world step loop.
    }
}

/// Compute dvel for normal constraint (rapier formula).
/// dvel = dir1 · v1_lin + torque_dir1 · v1_ang - dir1 · v2_lin + torque_dir2 · v2_ang + rhs
fn normal_dvel(bodies: &[RigidBody], sc: &SolverContact) -> f32 {
    let (v1_lin, v1_ang) = if sc.body_a == usize::MAX {
        (Vec3f::default(), Vec3f::default())
    } else {
        let a = &bodies[sc.body_a];
        (a.linear_velocity, a.angular_velocity)
    };
    let (v2_lin, v2_ang) = if sc.body_b == usize::MAX {
        (Vec3f::default(), Vec3f::default())
    } else {
        let b = &bodies[sc.body_b];
        (b.linear_velocity, b.angular_velocity)
    };

    sc.dir1.dot(v1_lin) + sc.torque_dir1.dot(v1_ang) - sc.dir1.dot(v2_lin)
        + sc.torque_dir2.dot(v2_ang)
        + sc.rhs
}

/// Compute dvel for a tangent constraint.
fn tangent_dvel(
    bodies: &[RigidBody],
    sc: &SolverContact,
    tangent: Vec3f,
    torque_dir1: Vec3f,
    torque_dir2: Vec3f,
    rhs: f32,
) -> f32 {
    let (v1_lin, v1_ang) = if sc.body_a == usize::MAX {
        (Vec3f::default(), Vec3f::default())
    } else {
        let a = &bodies[sc.body_a];
        (a.linear_velocity, a.angular_velocity)
    };
    let (v2_lin, v2_ang) = if sc.body_b == usize::MAX {
        (Vec3f::default(), Vec3f::default())
    } else {
        let b = &bodies[sc.body_b];
        (b.linear_velocity, b.angular_velocity)
    };

    tangent.dot(v1_lin) + torque_dir1.dot(v1_ang) - tangent.dot(v2_lin)
        + torque_dir2.dot(v2_ang)
        + rhs
}

/// Apply impulse along dir1 (rapier convention).
/// body1: linear += dir1 * im1 * dlambda, angular += ii_torque_dir1 * dlambda
/// body2: linear -= dir1 * im2 * dlambda, angular += ii_torque_dir2 * dlambda
/// Note: ii_torque_dir2 already has the sign baked in.
fn apply_impulse_rapier(
    bodies: &mut [RigidBody],
    sc: &SolverContact,
    dir: Vec3f,
    ii_td1: Vec3f,
    ii_td2: Vec3f,
    dlambda: f32,
) {
    if sc.body_a != usize::MAX {
        let a = &mut bodies[sc.body_a];
        a.linear_velocity += dir * (sc.im1 * dlambda);
        a.angular_velocity += ii_td1 * dlambda;
    }
    if sc.body_b != usize::MAX {
        let b = &mut bodies[sc.body_b];
        b.linear_velocity += dir * (-sc.im2 * dlambda);
        b.angular_velocity += ii_td2 * dlambda;
    }
}

/// Apply cached impulses from previous solve as warmstart.
/// Rapier scales by warmstart_coefficient (default 1.0).
pub fn warmstart(
    bodies: &mut [RigidBody],
    solver_contacts: &mut [SolverContact],
    coefficient: f32,
) {
    for sc in solver_contacts.iter_mut() {
        // Scale accumulated impulses by warmstart coefficient
        let n_impulse = sc.impulse_normal * coefficient;
        let t1_impulse = sc.impulse_tangent1 * coefficient;
        let t2_impulse = sc.impulse_tangent2 * coefficient;

        sc.impulse_normal = n_impulse;
        sc.impulse_tangent1 = t1_impulse;
        sc.impulse_tangent2 = t2_impulse;

        // Apply normal warmstart impulse
        if n_impulse != 0.0 {
            apply_impulse_rapier(
                bodies,
                sc,
                sc.dir1,
                sc.ii_torque_dir1,
                sc.ii_torque_dir2,
                n_impulse,
            );
        }
        // Apply tangent1 warmstart impulse
        if t1_impulse != 0.0 {
            apply_impulse_rapier(
                bodies,
                sc,
                sc.tangent1,
                sc.t1_ii_torque_dir1,
                sc.t1_ii_torque_dir2,
                t1_impulse,
            );
        }
        // Apply tangent2 warmstart impulse
        if t2_impulse != 0.0 {
            apply_impulse_rapier(
                bodies,
                sc,
                sc.tangent2,
                sc.t2_ii_torque_dir1,
                sc.t2_ii_torque_dir2,
                t2_impulse,
            );
        }
    }
}

/// Run PGS iterations over solver contacts (rapier's solve formula).
pub fn solve_contacts(
    bodies: &mut [RigidBody],
    solver_contacts: &mut [SolverContact],
    iterations: usize,
) {
    for _ in 0..iterations {
        for i in 0..solver_contacts.len() {
            // --- Normal constraint ---
            let dvel = {
                let sc = &solver_contacts[i];
                normal_dvel(bodies, sc)
            };
            let sc = &mut solver_contacts[i];
            let new_impulse = sc.cfm_factor * (sc.impulse_normal - sc.r_normal * dvel).max(0.0);
            let dlambda = new_impulse - sc.impulse_normal;
            sc.impulse_normal = new_impulse;
            let dir1 = sc.dir1;
            let ii_td1 = sc.ii_torque_dir1;
            let ii_td2 = sc.ii_torque_dir2;
            let sc_copy = *sc;
            apply_impulse_rapier(bodies, &sc_copy, dir1, ii_td1, ii_td2, dlambda);

            // --- Friction tangent 1 ---
            let dvel_t1 = {
                let sc = &solver_contacts[i];
                tangent_dvel(
                    bodies,
                    sc,
                    sc.tangent1,
                    sc.t1_torque_dir1,
                    sc.t1_torque_dir2,
                    sc.rhs_tangent1,
                )
            };
            let sc = &mut solver_contacts[i];
            let limit = sc.friction * sc.impulse_normal;
            let new_t1 = (sc.impulse_tangent1 - sc.r_tangent1 * dvel_t1).clamp(-limit, limit);
            let dlambda_t1 = new_t1 - sc.impulse_tangent1;
            sc.impulse_tangent1 = new_t1;
            let tangent1 = sc.tangent1;
            let t1_ii_td1 = sc.t1_ii_torque_dir1;
            let t1_ii_td2 = sc.t1_ii_torque_dir2;
            let sc_copy = *sc;
            apply_impulse_rapier(bodies, &sc_copy, tangent1, t1_ii_td1, t1_ii_td2, dlambda_t1);

            // --- Friction tangent 2 ---
            let dvel_t2 = {
                let sc = &solver_contacts[i];
                tangent_dvel(
                    bodies,
                    sc,
                    sc.tangent2,
                    sc.t2_torque_dir1,
                    sc.t2_torque_dir2,
                    sc.rhs_tangent2,
                )
            };
            let sc = &mut solver_contacts[i];
            let limit = sc.friction * sc.impulse_normal;
            let new_t2 = (sc.impulse_tangent2 - sc.r_tangent2 * dvel_t2).clamp(-limit, limit);
            let dlambda_t2 = new_t2 - sc.impulse_tangent2;
            sc.impulse_tangent2 = new_t2;
            let tangent2 = sc.tangent2;
            let t2_ii_td1 = sc.t2_ii_torque_dir1;
            let t2_ii_td2 = sc.t2_ii_torque_dir2;
            let sc_copy = *sc;
            apply_impulse_rapier(bodies, &sc_copy, tangent2, t2_ii_td1, t2_ii_td2, dlambda_t2);
        }
    }
}

/// Run stabilization iterations (without bias) after position integration.
/// This is rapier's "solve_wo_bias" step.
pub fn solve_contacts_wo_bias(
    bodies: &mut [RigidBody],
    solver_contacts: &mut [SolverContact],
    iterations: usize,
) {
    for _ in 0..iterations {
        for i in 0..solver_contacts.len() {
            // Normal constraint without bias
            let dvel = {
                let sc = &solver_contacts[i];
                let (v1_lin, v1_ang) = if sc.body_a == usize::MAX {
                    (Vec3f::default(), Vec3f::default())
                } else {
                    let a = &bodies[sc.body_a];
                    (a.linear_velocity, a.angular_velocity)
                };
                let (v2_lin, v2_ang) = if sc.body_b == usize::MAX {
                    (Vec3f::default(), Vec3f::default())
                } else {
                    let b = &bodies[sc.body_b];
                    (b.linear_velocity, b.angular_velocity)
                };
                sc.dir1.dot(v1_lin) + sc.torque_dir1.dot(v1_ang) - sc.dir1.dot(v2_lin)
                    + sc.torque_dir2.dot(v2_ang)
                    + sc.rhs_wo_bias
            };
            let sc = &mut solver_contacts[i];
            // cfm_factor = 1.0 for stabilization (no softness)
            let new_impulse = (sc.impulse_normal - sc.r_normal * dvel).max(0.0);
            let dlambda = new_impulse - sc.impulse_normal;
            sc.impulse_normal = new_impulse;
            let dir1 = sc.dir1;
            let ii_td1 = sc.ii_torque_dir1;
            let ii_td2 = sc.ii_torque_dir2;
            let sc_copy = *sc;
            apply_impulse_rapier(bodies, &sc_copy, dir1, ii_td1, ii_td2, dlambda);

            // Friction tangent 1 without bias
            let dvel_t1 = {
                let sc = &solver_contacts[i];
                tangent_dvel(
                    bodies,
                    sc,
                    sc.tangent1,
                    sc.t1_torque_dir1,
                    sc.t1_torque_dir2,
                    sc.rhs_tangent1_wo_bias,
                )
            };
            let sc = &mut solver_contacts[i];
            let limit = sc.friction * sc.impulse_normal;
            let new_t1 = (sc.impulse_tangent1 - sc.r_tangent1 * dvel_t1).clamp(-limit, limit);
            let dlambda_t1 = new_t1 - sc.impulse_tangent1;
            sc.impulse_tangent1 = new_t1;
            let tangent1 = sc.tangent1;
            let t1_ii_td1 = sc.t1_ii_torque_dir1;
            let t1_ii_td2 = sc.t1_ii_torque_dir2;
            let sc_copy = *sc;
            apply_impulse_rapier(bodies, &sc_copy, tangent1, t1_ii_td1, t1_ii_td2, dlambda_t1);

            // Friction tangent 2 without bias
            let dvel_t2 = {
                let sc = &solver_contacts[i];
                tangent_dvel(
                    bodies,
                    sc,
                    sc.tangent2,
                    sc.t2_torque_dir1,
                    sc.t2_torque_dir2,
                    sc.rhs_tangent2_wo_bias,
                )
            };
            let sc = &mut solver_contacts[i];
            let limit = sc.friction * sc.impulse_normal;
            let new_t2 = (sc.impulse_tangent2 - sc.r_tangent2 * dvel_t2).clamp(-limit, limit);
            let dlambda_t2 = new_t2 - sc.impulse_tangent2;
            sc.impulse_tangent2 = new_t2;
            let tangent2 = sc.tangent2;
            let t2_ii_td1 = sc.t2_ii_torque_dir1;
            let t2_ii_td2 = sc.t2_ii_torque_dir2;
            let sc_copy = *sc;
            apply_impulse_rapier(bodies, &sc_copy, tangent2, t2_ii_td1, t2_ii_td2, dlambda_t2);
        }
    }
}

fn body_props(bodies: &[RigidBody], idx: usize) -> (f32, Mat3f, f32, f32, Vec3f, Vec3f) {
    if idx == usize::MAX {
        (
            0.0,
            Mat3f::zero(),
            GROUND_RESTITUTION,
            GROUND_FRICTION,
            Vec3f::default(),
            Vec3f::default(),
        )
    } else {
        let b = &bodies[idx];
        (
            b.inv_mass,
            b.world_inv_inertia(),
            b.restitution,
            b.friction,
            b.linear_velocity,
            b.angular_velocity,
        )
    }
}

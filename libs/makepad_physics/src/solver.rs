use crate::contact::{ContactManifold, MAX_CONTACTS};
use crate::narrow_phase::{select_contact_indices, PREDICTION_DISTANCE};
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
/// Maximum distance used to match contact points across consecutive frames.
const WARMSTART_POINT_MATCH_DISTANCE_SQ: f32 = 0.25 * 0.25;
/// Contact normals must remain close enough to reuse cached impulses.
const WARMSTART_NORMAL_DOT_THRESHOLD: f32 = 0.9;

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
    pub impulse_accumulator: f32,

    pub cfm_factor: f32,
    pub manifold_index: usize,
    pub point_index: usize,

    // Stored for bias recomputation during substepping (rapier's builder info pattern).
    // local_p1/local_p2 are contact points in body-local coordinates,
    // used to re-transform into world space after position integration.
    pub local_p1: Vec3f, // contact point in body A's local frame (zero for ground)
    pub local_p2: Vec3f, // contact point in body B's local frame (zero for ground)
    pub dist: f32,       // initial penetration distance (negative = penetration, rapier convention)
    pub normal_vel: f32, // restitution component
}

#[derive(Clone, Copy)]
pub struct SolverFriction {
    pub body_a: usize,
    pub body_b: usize,
    pub im1: f32,
    pub im2: f32,
    pub dir1: Vec3f,
    pub tangent1: Vec3f,
    pub tangent2: Vec3f,
    pub friction: f32,
    pub manifold_index: usize,
    pub start_contact: usize,
    pub num_contacts: usize,
    pub local_friction_center1: Vec3f,
    pub local_friction_center2: Vec3f,
    pub t1_torque_dir1: Vec3f,
    pub t1_torque_dir2: Vec3f,
    pub t1_ii_torque_dir1: Vec3f,
    pub t1_ii_torque_dir2: Vec3f,
    pub lhs_tangent1: f32,
    pub t2_torque_dir1: Vec3f,
    pub t2_torque_dir2: Vec3f,
    pub t2_ii_torque_dir1: Vec3f,
    pub t2_ii_torque_dir2: Vec3f,
    pub lhs_tangent2: f32,
    pub lhs_tangent_cross: f32,
    pub rhs_tangent1_wo_bias: f32,
    pub rhs_tangent2_wo_bias: f32,
    pub rhs_tangent1: f32,
    pub rhs_tangent2: f32,
    pub impulse_tangent1: f32,
    pub impulse_tangent2: f32,
    pub impulse_tangent1_accumulator: f32,
    pub impulse_tangent2_accumulator: f32,
    pub ii_twist_dir1: Vec3f,
    pub ii_twist_dir2: Vec3f,
    pub rhs_twist: f32,
    pub r_twist: f32,
    pub impulse_twist: f32,
    pub impulse_twist_accumulator: f32,
    pub twist_dists: [f32; MAX_CONTACTS],
}

impl Default for SolverFriction {
    fn default() -> Self {
        Self {
            body_a: usize::MAX,
            body_b: usize::MAX,
            im1: 0.0,
            im2: 0.0,
            dir1: Vec3f::default(),
            tangent1: Vec3f::default(),
            tangent2: Vec3f::default(),
            friction: 0.0,
            manifold_index: 0,
            start_contact: 0,
            num_contacts: 0,
            local_friction_center1: Vec3f::default(),
            local_friction_center2: Vec3f::default(),
            t1_torque_dir1: Vec3f::default(),
            t1_torque_dir2: Vec3f::default(),
            t1_ii_torque_dir1: Vec3f::default(),
            t1_ii_torque_dir2: Vec3f::default(),
            lhs_tangent1: 0.0,
            t2_torque_dir1: Vec3f::default(),
            t2_torque_dir2: Vec3f::default(),
            t2_ii_torque_dir1: Vec3f::default(),
            t2_ii_torque_dir2: Vec3f::default(),
            lhs_tangent2: 0.0,
            lhs_tangent_cross: 0.0,
            rhs_tangent1_wo_bias: 0.0,
            rhs_tangent2_wo_bias: 0.0,
            rhs_tangent1: 0.0,
            rhs_tangent2: 0.0,
            impulse_tangent1: 0.0,
            impulse_tangent2: 0.0,
            impulse_tangent1_accumulator: 0.0,
            impulse_tangent2_accumulator: 0.0,
            ii_twist_dir1: Vec3f::default(),
            ii_twist_dir2: Vec3f::default(),
            rhs_twist: 0.0,
            r_twist: 0.0,
            impulse_twist: 0.0,
            impulse_twist_accumulator: 0.0,
            twist_dists: [0.0; MAX_CONTACTS],
        }
    }
}

fn orthonormal_tangent(dir: Vec3f) -> Vec3f {
    // Rapier uses the Pixar branchless orthonormal-vector construction.
    let sign = 1.0f32.copysign(dir.z);
    let a = -1.0 / (sign + dir.z);
    let b = dir.x * dir.y * a;
    vec3f(b, sign + dir.y * dir.y * a, -dir.y)
}

/// Build Rapier's 3D tangent basis from the relative linear velocity.
fn compute_tangents(dir: Vec3f, linvel1: Vec3f, linvel2: Vec3f) -> (Vec3f, Vec3f) {
    let relative_linvel = linvel1 - linvel2;
    let tangent_relative_linvel = relative_linvel - dir * dir.dot(relative_linvel);

    let tangent1 = if tangent_relative_linvel.length() >= 1.0e-4 {
        tangent_relative_linvel.normalize()
    } else {
        orthonormal_tangent(dir)
    };
    let tangent2 = Vec3f::cross(dir, tangent1);
    (tangent1, tangent2)
}

fn is_bouncy(restitution: f32, is_new: bool) -> f32 {
    if is_new {
        (restitution > 0.0) as u32 as f32
    } else {
        (restitution >= 1.0) as u32 as f32
    }
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
    solver_frictions: &mut Vec<SolverFriction>,
) {
    solver_contacts.clear();
    solver_frictions.clear();
    let cfm = cfm_factor(dt);
    let erp_inv = erp_inv_dt(dt);
    let inv_dt = if dt > 0.0 { 1.0 / dt } else { 0.0 };

    for (manifold_index, manifold) in manifolds.iter().enumerate() {
        let a_idx = manifold.body_a;
        let b_idx = manifold.body_b;

        // Gather body properties
        let (im1, inv_i1, rest1, fric1, linvel1, angvel1) = body_props(bodies, a_idx);
        let (im2, inv_i2, rest2, fric2, linvel2, angvel2) = body_props(bodies, b_idx);
        let restitution = 0.5 * (rest1 + rest2);
        let friction = 0.5 * (fric1 + fric2);
        if manifold.num_points == 0 {
            continue;
        }

        let normal = manifold.points[0].normal;
        let dir1 = -normal;
        let (tangent1, tangent2) = compute_tangents(dir1, linvel1, linvel2);
        let start_contact = solver_contacts.len();
        let mut active_points = [Vec3f::default(); MAX_CONTACTS];
        let mut active_count = 0usize;
        let mut tangent_warmstart = [0.0; 2];
        let mut twist_warmstart = 0.0;

        let mut selected = [0usize; 4];
        let mut num_selected = manifold.num_points.min(4);
        select_contact_indices(manifold, 4, &mut selected, &mut num_selected);

        for &pi in selected.iter().take(num_selected) {
            let point = &manifold.points[pi];

            // Rapier builds solver constraints from the midpoint of the two shape points.
            let world_pt1 = point.world_point_a;
            let world_pt2 = point.world_point_b;
            let effective_point = (world_pt1 + world_pt2) * 0.5;

            let vel1_orig = if a_idx == usize::MAX {
                Vec3f::default()
            } else {
                linvel1 + Vec3f::cross(angvel1, world_pt1 - bodies[a_idx].pose.position)
            };
            let vel2_orig = if b_idx == usize::MAX {
                Vec3f::default()
            } else {
                linvel2 + Vec3f::cross(angvel2, world_pt2 - bodies[b_idx].pose.position)
            };

            // Moment arms from center of mass to contact point
            let dp1 = if a_idx == usize::MAX {
                Vec3f::default()
            } else {
                effective_point - bodies[a_idx].pose.position
            };
            let dp2 = if b_idx == usize::MAX {
                Vec3f::default()
            } else {
                effective_point - bodies[b_idx].pose.position
            };

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

            // Rapier stores the signed distance between the original shape points.
            let dist = (world_pt2 - world_pt1).dot(normal);
            let keep_solver_contact = dist < PREDICTION_DISTANCE
                || dist + (vel2_orig - vel1_orig).dot(normal) * dt < PREDICTION_DISTANCE;
            if !keep_solver_contact {
                continue;
            }

            active_points[active_count] = effective_point;
            tangent_warmstart[0] += point.warmstart_tangent_impulse[0];
            tangent_warmstart[1] += point.warmstart_tangent_impulse[1];
            twist_warmstart += point.warmstart_twist_impulse;
            active_count += 1;

            let normal_vel =
                is_bouncy(restitution, point.normal_impulse == 0.0) * restitution * projected_velocity;

            // RHS bias (rapier formula)
            let rhs_wo_bias = normal_vel + dist.max(0.0) * inv_dt;
            let rhs_bias =
                ((dist + ALLOWED_LINEAR_ERROR) * erp_inv).clamp(-MAX_CORRECTIVE_VELOCITY, 0.0);
            let rhs = rhs_wo_bias + rhs_bias;

            // Compute body-local contact points for TGS substep updates.
            // After position integration in each substep, we re-transform these
            // to world space to recompute penetration depth (matching rapier).
            // For ground (usize::MAX), store world-space point (ground doesn't move).
            let local_p1 = if a_idx == usize::MAX {
                effective_point
            } else {
                bodies[a_idx]
                    .pose
                    .invert()
                    .transform_vec3(&effective_point)
            };
            let local_p2 = if b_idx == usize::MAX {
                effective_point
            } else {
                bodies[b_idx]
                    .pose
                    .invert()
                    .transform_vec3(&effective_point)
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
                impulse_normal: point.warmstart_normal_impulse,
                impulse_accumulator: 0.0,
                cfm_factor: cfm,
                manifold_index,
                point_index: pi,
                local_p1,
                local_p2,
                dist,
                normal_vel,
            });
        }

        if active_count == 0 {
            continue;
        }

        let inv_active_count = 1.0 / active_count as f32;
        let mut friction_center = Vec3f::default();
        for point in active_points.iter().take(active_count) {
            friction_center += *point;
        }
        friction_center *= inv_active_count;
        tangent_warmstart[0] *= inv_active_count;
        tangent_warmstart[1] *= inv_active_count;
        twist_warmstart *= inv_active_count;

        let dp1 = if a_idx == usize::MAX {
            Vec3f::default()
        } else {
            friction_center - bodies[a_idx].pose.position
        };
        let dp2 = if b_idx == usize::MAX {
            Vec3f::default()
        } else {
            friction_center - bodies[b_idx].pose.position
        };

        let t1_torque_dir1 = Vec3f::cross(dp1, tangent1);
        let t1_torque_dir2 = Vec3f::cross(dp2, -tangent1);
        let t1_ii_torque_dir1 = inv_i1.mul_vec3(t1_torque_dir1);
        let t1_ii_torque_dir2 = inv_i2.mul_vec3(t1_torque_dir2);
        let lhs_tangent1 = im1
            + im2
            + t1_ii_torque_dir1.dot(t1_torque_dir1)
            + t1_ii_torque_dir2.dot(t1_torque_dir2);

        let t2_torque_dir1 = Vec3f::cross(dp1, tangent2);
        let t2_torque_dir2 = Vec3f::cross(dp2, -tangent2);
        let t2_ii_torque_dir1 = inv_i1.mul_vec3(t2_torque_dir1);
        let t2_ii_torque_dir2 = inv_i2.mul_vec3(t2_torque_dir2);
        let lhs_tangent2 = im1
            + im2
            + t2_ii_torque_dir1.dot(t2_torque_dir1)
            + t2_ii_torque_dir2.dot(t2_torque_dir2);
        let lhs_tangent_cross =
            2.0 * (t1_ii_torque_dir1.dot(t2_torque_dir1) + t1_ii_torque_dir2.dot(t2_torque_dir2));

        let local_friction_center1 = if a_idx == usize::MAX {
            friction_center
        } else {
            bodies[a_idx]
                .pose
                .invert()
                .transform_vec3(&friction_center)
        };
        let local_friction_center2 = if b_idx == usize::MAX {
            friction_center
        } else {
            bodies[b_idx]
                .pose
                .invert()
                .transform_vec3(&friction_center)
        };

        let mut solver_friction = SolverFriction {
            body_a: a_idx,
            body_b: b_idx,
            im1,
            im2,
            dir1,
            tangent1,
            tangent2,
            friction,
            manifold_index,
            start_contact,
            num_contacts: active_count,
            local_friction_center1,
            local_friction_center2,
            t1_torque_dir1,
            t1_torque_dir2,
            t1_ii_torque_dir1,
            t1_ii_torque_dir2,
            lhs_tangent1,
            t2_torque_dir1,
            t2_torque_dir2,
            t2_ii_torque_dir1,
            t2_ii_torque_dir2,
            lhs_tangent2,
            lhs_tangent_cross,
            rhs_tangent1_wo_bias: 0.0,
            rhs_tangent2_wo_bias: 0.0,
            rhs_tangent1: 0.0,
            rhs_tangent2: 0.0,
            impulse_tangent1: tangent_warmstart[0],
            impulse_tangent2: tangent_warmstart[1],
            impulse_tangent1_accumulator: 0.0,
            impulse_tangent2_accumulator: 0.0,
            ii_twist_dir1: Vec3f::default(),
            ii_twist_dir2: Vec3f::default(),
            rhs_twist: 0.0,
            r_twist: 0.0,
            impulse_twist: twist_warmstart,
            impulse_twist_accumulator: 0.0,
            twist_dists: [0.0; MAX_CONTACTS],
        };

        if active_count > 1 {
            for (index, point) in active_points.iter().take(active_count).enumerate() {
                solver_friction.twist_dists[index] = (friction_center - *point).length();
            }
            let ii_twist_dir1 = inv_i1.mul_vec3(dir1);
            let ii_twist_dir2 = inv_i2.mul_vec3(-dir1);
            let denom = ii_twist_dir1.dot(dir1) + ii_twist_dir2.dot(-dir1);
            solver_friction.ii_twist_dir1 = ii_twist_dir1;
            solver_friction.ii_twist_dir2 = ii_twist_dir2;
            solver_friction.r_twist = if denom > 0.0 { 1.0 / denom } else { 0.0 };
        }

        solver_frictions.push(solver_friction);
    }
}

pub fn inherit_warmstart_impulses(
    previous_manifolds: &[ContactManifold],
    manifolds: &mut [ContactManifold],
) {
    for manifold in manifolds.iter_mut() {
        let Some(previous) = previous_manifolds
            .iter()
            .find(|prev| prev.body_a == manifold.body_a && prev.body_b == manifold.body_b)
        else {
            continue;
        };

        let mut used_previous = [false; crate::contact::MAX_CONTACTS];
        for point in manifold.points[..manifold.num_points].iter_mut() {
            let point_has_feature_ids = point.feature_id_a != 0 || point.feature_id_b != 0;
            let mut best_match = None;
            let mut best_distance_sq = WARMSTART_POINT_MATCH_DISTANCE_SQ;

            for (prev_index, prev_point) in
                previous.points[..previous.num_points].iter().enumerate()
            {
                if used_previous[prev_index] {
                    continue;
                }
                if point.normal.dot(prev_point.normal) < WARMSTART_NORMAL_DOT_THRESHOLD {
                    continue;
                }
                if point_has_feature_ids {
                    if point.feature_id_a == prev_point.feature_id_a
                        && point.feature_id_b == prev_point.feature_id_b
                    {
                        best_match = Some(prev_index);
                        break;
                    }
                    continue;
                }
                if prev_point.feature_id_a != 0 || prev_point.feature_id_b != 0 {
                    continue;
                }
                let distance_sq = (point.local_point_a - prev_point.local_point_a).length_squared()
                    + (point.local_point_b - prev_point.local_point_b).length_squared();
                if distance_sq <= best_distance_sq {
                    best_distance_sq = distance_sq;
                    best_match = Some(prev_index);
                }
            }

            if let Some(prev_index) = best_match {
                used_previous[prev_index] = true;
                let prev_point = previous.points[prev_index];
                point.normal_impulse = prev_point.normal_impulse;
                point.tangent_impulse = prev_point.tangent_impulse;
                point.warmstart_normal_impulse = prev_point.warmstart_normal_impulse;
                point.warmstart_tangent_impulse = prev_point.warmstart_tangent_impulse;
                point.warmstart_twist_impulse = prev_point.warmstart_twist_impulse;
            }
        }
    }
}

pub fn writeback_impulses(
    solver_contacts: &[SolverContact],
    solver_frictions: &[SolverFriction],
    manifolds: &mut [ContactManifold],
) {
    for solver_contact in solver_contacts {
        let Some(manifold) = manifolds.get_mut(solver_contact.manifold_index) else {
            continue;
        };
        if solver_contact.point_index >= manifold.num_points {
            continue;
        }
        let point = &mut manifold.points[solver_contact.point_index];
        point.normal_impulse = solver_contact.impulse_accumulator + solver_contact.impulse_normal;
        point.warmstart_normal_impulse = solver_contact.impulse_normal;
    }

    for solver_friction in solver_frictions {
        let Some(manifold) = manifolds.get_mut(solver_friction.manifold_index) else {
            continue;
        };
        let total_tangent = [
            solver_friction.impulse_tangent1_accumulator + solver_friction.impulse_tangent1,
            solver_friction.impulse_tangent2_accumulator + solver_friction.impulse_tangent2,
        ];
        for solver_contact in solver_contacts[solver_friction.start_contact
            ..solver_friction.start_contact + solver_friction.num_contacts]
            .iter()
        {
            if solver_contact.point_index >= manifold.num_points {
                continue;
            }
            let point = &mut manifold.points[solver_contact.point_index];
            point.tangent_impulse = total_tangent;
            point.warmstart_tangent_impulse = [
                solver_friction.impulse_tangent1,
                solver_friction.impulse_tangent2,
            ];
            point.warmstart_twist_impulse = solver_friction.impulse_twist;
        }
    }
}

/// Update solver contacts after position integration (for TGS substepping).
/// Matches Rapier's contact `update()`: recompute RHS and advance the warmstart caches.
pub fn update_contacts(
    bodies: &[RigidBody],
    solver_contacts: &mut [SolverContact],
    dt: f32,
    warmstart_coefficient: f32,
) {
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

        // Recompute RHS bias terms
        let rhs_wo_bias = sc.normal_vel + dist.max(0.0) * inv_dt;
        let rhs_bias =
            ((dist + ALLOWED_LINEAR_ERROR) * erp_inv).clamp(-MAX_CORRECTIVE_VELOCITY, 0.0);
        sc.rhs = rhs_wo_bias + rhs_bias;
        sc.rhs_wo_bias = rhs_wo_bias;
        sc.cfm_factor = cfm;
        sc.impulse_accumulator += sc.impulse_normal;
        sc.impulse_normal *= warmstart_coefficient;
    }
}

pub fn update_frictions(
    bodies: &[RigidBody],
    solver_frictions: &mut [SolverFriction],
    dt: f32,
    warmstart_coefficient: f32,
) {
    let inv_dt = if dt > 0.0 { 1.0 / dt } else { 0.0 };

    for sf in solver_frictions.iter_mut() {
        let p1 = if sf.body_a == usize::MAX {
            sf.local_friction_center1
        } else {
            bodies[sf.body_a]
                .pose
                .transform_vec3(&sf.local_friction_center1)
        };
        let p2 = if sf.body_b == usize::MAX {
            sf.local_friction_center2
        } else {
            bodies[sf.body_b]
                .pose
                .transform_vec3(&sf.local_friction_center2)
        };

        sf.rhs_tangent1 = sf.rhs_tangent1_wo_bias + (p1 - p2).dot(sf.tangent1) * inv_dt;
        sf.rhs_tangent2 = sf.rhs_tangent2_wo_bias + (p1 - p2).dot(sf.tangent2) * inv_dt;
        sf.impulse_tangent1_accumulator += sf.impulse_tangent1;
        sf.impulse_tangent2_accumulator += sf.impulse_tangent2;
        sf.impulse_twist_accumulator += sf.impulse_twist;
        sf.impulse_tangent1 *= warmstart_coefficient;
        sf.impulse_tangent2 *= warmstart_coefficient;
        sf.impulse_twist *= warmstart_coefficient;
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

fn tangent_dvel(
    bodies: &[RigidBody],
    sf: &SolverFriction,
    tangent: Vec3f,
    torque_dir1: Vec3f,
    torque_dir2: Vec3f,
    rhs: f32,
) -> f32 {
    let (v1_lin, v1_ang) = if sf.body_a == usize::MAX {
        (Vec3f::default(), Vec3f::default())
    } else {
        let a = &bodies[sf.body_a];
        (a.linear_velocity, a.angular_velocity)
    };
    let (v2_lin, v2_ang) = if sf.body_b == usize::MAX {
        (Vec3f::default(), Vec3f::default())
    } else {
        let b = &bodies[sf.body_b];
        (b.linear_velocity, b.angular_velocity)
    };

    tangent.dot(v1_lin) + torque_dir1.dot(v1_ang) - tangent.dot(v2_lin)
        + torque_dir2.dot(v2_ang)
        + rhs
}

fn apply_manifold_impulse(
    bodies: &mut [RigidBody],
    sf: &SolverFriction,
    dir: Vec3f,
    ii_td1: Vec3f,
    ii_td2: Vec3f,
    dlambda: f32,
) {
    if sf.body_a != usize::MAX {
        let a = &mut bodies[sf.body_a];
        a.linear_velocity += dir * (sf.im1 * dlambda);
        a.angular_velocity += ii_td1 * dlambda;
    }
    if sf.body_b != usize::MAX {
        let b = &mut bodies[sf.body_b];
        b.linear_velocity += dir * (-sf.im2 * dlambda);
        b.angular_velocity += ii_td2 * dlambda;
    }
}

fn solve_manifold_friction(bodies: &mut [RigidBody], sf: &mut SolverFriction, limit: f32, use_bias: bool) {
    let rhs_t1 = if use_bias { sf.rhs_tangent1 } else { sf.rhs_tangent1_wo_bias };
    let rhs_t2 = if use_bias { sf.rhs_tangent2 } else { sf.rhs_tangent2_wo_bias };
    let dvel_t1 = tangent_dvel(
        bodies,
        sf,
        sf.tangent1,
        sf.t1_torque_dir1,
        sf.t1_torque_dir2,
        rhs_t1,
    );
    let dvel_t2 = tangent_dvel(
        bodies,
        sf,
        sf.tangent2,
        sf.t2_torque_dir1,
        sf.t2_torque_dir2,
        rhs_t2,
    );

    let dvel_00 = dvel_t1 * dvel_t1;
    let dvel_11 = dvel_t2 * dvel_t2;
    let dvel_01 = dvel_t1 * dvel_t2;
    let denom =
        dvel_00 * sf.lhs_tangent1 + dvel_11 * sf.lhs_tangent2 + dvel_01 * sf.lhs_tangent_cross;
    if denom <= 0.0 {
        return;
    }

    let inv_lhs = (dvel_00 + dvel_11) / denom;
    let delta_impulse1 = inv_lhs * dvel_t1;
    let delta_impulse2 = inv_lhs * dvel_t2;

    let mut new_t1 = sf.impulse_tangent1 - delta_impulse1;
    let mut new_t2 = sf.impulse_tangent2 - delta_impulse2;
    let impulse_len = (new_t1 * new_t1 + new_t2 * new_t2).sqrt();
    if impulse_len > limit && impulse_len > 0.0 {
        let scale = limit / impulse_len;
        new_t1 *= scale;
        new_t2 *= scale;
    }

    let dlambda_t1 = new_t1 - sf.impulse_tangent1;
    let dlambda_t2 = new_t2 - sf.impulse_tangent2;
    sf.impulse_tangent1 = new_t1;
    sf.impulse_tangent2 = new_t2;

    if dlambda_t1 != 0.0 {
        apply_manifold_impulse(
            bodies,
            sf,
            sf.tangent1,
            sf.t1_ii_torque_dir1,
            sf.t1_ii_torque_dir2,
            dlambda_t1,
        );
    }
    if dlambda_t2 != 0.0 {
        apply_manifold_impulse(
            bodies,
            sf,
            sf.tangent2,
            sf.t2_ii_torque_dir1,
            sf.t2_ii_torque_dir2,
            dlambda_t2,
        );
    }
}

fn solve_twist(bodies: &mut [RigidBody], sf: &mut SolverFriction, limit: f32) {
    if sf.num_contacts <= 1 || sf.r_twist == 0.0 {
        return;
    }

    let v1_ang = if sf.body_a == usize::MAX {
        Vec3f::default()
    } else {
        bodies[sf.body_a].angular_velocity
    };
    let v2_ang = if sf.body_b == usize::MAX {
        Vec3f::default()
    } else {
        bodies[sf.body_b].angular_velocity
    };

    let dvel = sf.dir1.dot(v1_ang - v2_ang) + sf.rhs_twist;
    let new_impulse = (sf.impulse_twist - sf.r_twist * dvel).clamp(-limit, limit);
    let dlambda = new_impulse - sf.impulse_twist;
    sf.impulse_twist = new_impulse;

    if sf.body_a != usize::MAX {
        bodies[sf.body_a].angular_velocity += sf.ii_twist_dir1 * dlambda;
    }
    if sf.body_b != usize::MAX {
        bodies[sf.body_b].angular_velocity += sf.ii_twist_dir2 * dlambda;
    }
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
    solver_frictions: &mut [SolverFriction],
    _coefficient: f32,
) {
    for sf in solver_frictions.iter_mut() {
        let end = sf.start_contact + sf.num_contacts;
        for sc in solver_contacts[sf.start_contact..end].iter_mut() {
            if sc.impulse_normal != 0.0 {
                apply_impulse_rapier(
                    bodies,
                    sc,
                    sc.dir1,
                    sc.ii_torque_dir1,
                    sc.ii_torque_dir2,
                    sc.impulse_normal,
                );
            }
        }

        if sf.impulse_tangent1 != 0.0 {
            apply_manifold_impulse(
                bodies,
                sf,
                sf.tangent1,
                sf.t1_ii_torque_dir1,
                sf.t1_ii_torque_dir2,
                sf.impulse_tangent1,
            );
        }
        if sf.impulse_tangent2 != 0.0 {
            apply_manifold_impulse(
                bodies,
                sf,
                sf.tangent2,
                sf.t2_ii_torque_dir1,
                sf.t2_ii_torque_dir2,
                sf.impulse_tangent2,
            );
        }
        if sf.impulse_twist != 0.0 {
            if sf.body_a != usize::MAX {
                bodies[sf.body_a].angular_velocity += sf.ii_twist_dir1 * sf.impulse_twist;
            }
            if sf.body_b != usize::MAX {
                bodies[sf.body_b].angular_velocity += sf.ii_twist_dir2 * sf.impulse_twist;
            }
        }
    }
}

/// Run PGS iterations over solver contacts (rapier's solve formula).
pub fn solve_contacts(
    bodies: &mut [RigidBody],
    solver_contacts: &mut [SolverContact],
    solver_frictions: &mut [SolverFriction],
    iterations: usize,
) {
    for _ in 0..iterations {
        for sf in solver_frictions.iter_mut() {
            let end = sf.start_contact + sf.num_contacts;
            for i in sf.start_contact..end {
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
            }

            let mut tangent_limit = 0.0f32;
            let mut twist_limit = 0.0f32;
            for (offset, i) in (sf.start_contact..end).enumerate() {
                let impulse = solver_contacts[i].impulse_normal;
                tangent_limit += impulse;
                twist_limit += impulse * sf.twist_dists[offset];
            }
            tangent_limit *= sf.friction;
            twist_limit *= sf.friction;

            solve_manifold_friction(bodies, sf, tangent_limit, true);
            solve_twist(bodies, sf, twist_limit);
        }
    }
}

/// Run stabilization iterations (without bias) after position integration.
/// This is rapier's "solve_wo_bias" step.
pub fn solve_contacts_wo_bias(
    bodies: &mut [RigidBody],
    solver_contacts: &mut [SolverContact],
    solver_frictions: &mut [SolverFriction],
    iterations: usize,
) {
    for _ in 0..iterations {
        for sf in solver_frictions.iter_mut() {
            let end = sf.start_contact + sf.num_contacts;
            for i in sf.start_contact..end {
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
                let new_impulse = (sc.impulse_normal - sc.r_normal * dvel).max(0.0);
                let dlambda = new_impulse - sc.impulse_normal;
                sc.impulse_normal = new_impulse;
                let dir1 = sc.dir1;
                let ii_td1 = sc.ii_torque_dir1;
                let ii_td2 = sc.ii_torque_dir2;
                let sc_copy = *sc;
                apply_impulse_rapier(bodies, &sc_copy, dir1, ii_td1, ii_td2, dlambda);
            }

            let mut tangent_limit = 0.0f32;
            let mut twist_limit = 0.0f32;
            for (offset, i) in (sf.start_contact..end).enumerate() {
                let impulse = solver_contacts[i].impulse_normal;
                tangent_limit += impulse;
                twist_limit += impulse * sf.twist_dists[offset];
            }
            tangent_limit *= sf.friction;
            twist_limit *= sf.friction;

            solve_manifold_friction(bodies, sf, tangent_limit, false);
            solve_twist(bodies, sf, twist_limit);
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

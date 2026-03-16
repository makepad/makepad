use super::{
    ContactConstraintNormalPart, ContactConstraintTangentPart, ContactConstraintTwistPart,
};
use crate::dynamics::solver::solver_body::SolverBodies;
use crate::dynamics::{IntegrationParameters, MultibodyJointSet, RigidBodySet};
use crate::geometry::{ContactManifold, ContactManifoldIndex, SimdSolverContact};
use crate::math::{DIM, MAX_MANIFOLD_POINTS, Real, SIMD_WIDTH, SimdReal};
#[cfg(not(feature = "simd-is-enabled"))]
use crate::utils::ComponentMul;
use crate::utils::{self, AngularInertiaOps, CrossProduct, DotProduct, ScalarType, SimdLength};
use num::Zero;
use simba::simd::{SimdPartialOrd, SimdValue};

#[derive(Copy, Clone, Debug)]
pub struct TwistContactPointInfos<N: ScalarType> {
    // This is different from the Coulomb version because it doesn’t
    // have the `tangent_vel` per-contact here.
    pub normal_vel: N,
    pub local_p1: N::Vector,
    pub local_p2: N::Vector,
    pub dist: N,
}

impl<N: ScalarType> Default for TwistContactPointInfos<N> {
    fn default() -> Self {
        Self {
            normal_vel: N::zero(),
            local_p1: Default::default(),
            local_p2: Default::default(),
            dist: N::zero(),
        }
    }
}

/*
 * FIXME: this involves a lot of duplicate code wrt. the contact with coulomb friction.
 *        Find a way to refactor so we can at least share the code for the ristution part.
 */
#[derive(Copy, Clone, Debug)]
pub(crate) struct ContactWithTwistFrictionBuilder<N: ScalarType> {
    infos: [TwistContactPointInfos<N>; MAX_MANIFOLD_POINTS],
    local_friction_center1: N::Vector,
    local_friction_center2: N::Vector,
    tangent_vel: N::Vector,
}

impl ContactWithTwistFrictionBuilder<SimdReal> {
    pub fn generate(
        manifold_id: [ContactManifoldIndex; SIMD_WIDTH],
        manifolds: [&ContactManifold; SIMD_WIDTH],
        bodies: &RigidBodySet,
        solver_bodies: &SolverBodies,
        out_builder: &mut ContactWithTwistFrictionBuilder<SimdReal>,
        out_constraint: &mut ContactWithTwistFriction<SimdReal>,
    ) {
        // TODO: could we avoid having to fetch the ids here? It’s the only thing we
        //       read from the original rigid-bodies.
        let ids1: [u32; SIMD_WIDTH] = array![|ii| if manifolds[ii].data.relative_dominance <= 0
            && manifold_id[ii] != usize::MAX
        {
            let handle = manifolds[ii].data.rigid_body1.unwrap(); // Can unwrap thanks to the dominance check.
            bodies[handle].ids.active_set_id as u32
        } else {
            u32::MAX
        }];
        let ids2: [u32; SIMD_WIDTH] = array![|ii| if manifolds[ii].data.relative_dominance >= 0
            && manifold_id[ii] != usize::MAX
        {
            let handle = manifolds[ii].data.rigid_body2.unwrap(); // Can unwrap thanks to the dominance check.
            bodies[handle].ids.active_set_id as u32
        } else {
            u32::MAX
        }];

        let vels1 = solver_bodies.gather_vels(ids1);
        let poses1 = solver_bodies.gather_poses(ids1);
        let vels2 = solver_bodies.gather_vels(ids2);
        let poses2 = solver_bodies.gather_poses(ids2);

        let world_com1 = poses1.translation;
        let world_com2 = poses2.translation;

        // TODO PERF: implement SIMD gather
        #[cfg(feature = "simd-is-enabled")]
        let force_dir1 =
            -<SimdReal as ScalarType>::Vector::from(gather![|ii| manifolds[ii].data.normal.into()]);
        #[cfg(not(feature = "simd-is-enabled"))]
        let force_dir1 = -manifolds[0].data.normal;
        let num_active_contacts = manifolds[0].data.num_active_contacts();

        #[cfg(feature = "dim2")]
        let tangents1 = force_dir1.orthonormal_basis();
        #[cfg(feature = "dim3")]
        let tangents1 = super::compute_tangent_contact_directions::<SimdReal>(
            &force_dir1,
            &vels1.linear,
            &vels2.linear,
        );

        let manifold_points =
            array![|ii| &manifolds[ii].data.solver_contacts[..num_active_contacts]];
        let num_points = manifold_points[0].len().min(MAX_MANIFOLD_POINTS);

        let inv_num_points = SimdReal::splat(1.0 / num_points as Real);

        out_constraint.dir1 = force_dir1;
        out_constraint.im1 = poses1.im;
        out_constraint.im2 = poses2.im;
        out_constraint.solver_vel1 = ids1;
        out_constraint.solver_vel2 = ids2;
        out_constraint.manifold_id = manifold_id;
        out_constraint.num_contacts = num_points as u8;
        #[cfg(feature = "dim3")]
        {
            out_constraint.tangent1 = tangents1[0];
        }

        let mut friction_center = Default::default();
        let mut twist_warmstart = Default::default();
        let mut tangent_warmstart = Default::default();
        let mut tangent_vel: <SimdReal as ScalarType>::Vector = Default::default();

        for k in 0..num_points {
            // SAFETY: we already know that the `manifold_points` has `num_points` elements
            //         so `k` isn’t out of bounds.
            let solver_contact =
                unsafe { SimdSolverContact::gather_unchecked(&manifold_points, k) };

            let is_bouncy = solver_contact.is_bouncy();

            friction_center += solver_contact.point * inv_num_points;

            let dp1 = solver_contact.point - world_com1;
            let dp2 = solver_contact.point - world_com2;

            let vel1 = vels1.linear + vels1.angular.gcross(dp1);
            let vel2 = vels2.linear + vels2.angular.gcross(dp2);

            twist_warmstart += solver_contact.warmstart_twist_impulse * inv_num_points;
            tangent_warmstart += solver_contact.warmstart_tangent_impulse * inv_num_points;
            tangent_vel += solver_contact.tangent_velocity * inv_num_points;

            out_constraint.limit = solver_contact.friction;
            out_constraint.manifold_contact_id[k] = solver_contact.contact_id.map(|id| id as u8);

            // Normal part.
            let normal_rhs_wo_bias;
            {
                let torque_dir1 = dp1.gcross(force_dir1);
                let torque_dir2 = dp2.gcross(-force_dir1);
                let ii_torque_dir1 = poses1.ii.transform_vector(torque_dir1);
                let ii_torque_dir2 = poses2.ii.transform_vector(torque_dir2);

                let imsum = poses1.im + poses2.im;
                let projected_mass = utils::simd_inv(
                    force_dir1.gdot(imsum.component_mul(&force_dir1))
                        + ii_torque_dir1.gdot(torque_dir1)
                        + ii_torque_dir2.gdot(torque_dir2),
                );

                let projected_velocity = (vel1 - vel2).gdot(force_dir1);
                normal_rhs_wo_bias = is_bouncy * solver_contact.restitution * projected_velocity;

                out_constraint.normal_part[k].torque_dir1 = torque_dir1;
                out_constraint.normal_part[k].torque_dir2 = torque_dir2;
                out_constraint.normal_part[k].ii_torque_dir1 = ii_torque_dir1;
                out_constraint.normal_part[k].ii_torque_dir2 = ii_torque_dir2;
                out_constraint.normal_part[k].impulse = solver_contact.warmstart_impulse;
                out_constraint.normal_part[k].r = projected_mass;
            }

            // Builder.
            out_builder.infos[k].local_p1 = poses1.inverse_transform_point(solver_contact.point);
            out_builder.infos[k].local_p2 = poses2.inverse_transform_point(solver_contact.point);
            out_builder.infos[k].dist = solver_contact.dist;
            out_builder.infos[k].normal_vel = normal_rhs_wo_bias;
        }

        /*
         * Tangent/twist part
         */
        out_constraint.tangent_part.impulse = tangent_warmstart;
        out_constraint.twist_part.impulse = twist_warmstart;

        out_builder.local_friction_center1 = poses1.inverse_transform_point(friction_center);
        out_builder.local_friction_center2 = poses2.inverse_transform_point(friction_center);

        let dp1 = friction_center - world_com1;
        let dp2 = friction_center - world_com2;

        // Twist part. It has no effect when there is only one point.
        if num_points > 1 {
            let mut twist_dists = [SimdReal::zero(); MAX_MANIFOLD_POINTS];
            for k in 0..num_points {
                // FIXME PERF: we don’t want to re-fetch here just to get the solver contact point!
                let solver_contact =
                    unsafe { SimdSolverContact::gather_unchecked(&manifold_points, k) };
                twist_dists[k] = (friction_center - solver_contact.point).simd_length();
            }

            let ii_twist_dir1 = poses1.ii.transform_vector(force_dir1);
            let ii_twist_dir2 = poses2.ii.transform_vector(-force_dir1);
            out_constraint.twist_part.rhs = SimdReal::zero();
            out_constraint.twist_part.ii_twist_dir1 = ii_twist_dir1;
            out_constraint.twist_part.ii_twist_dir2 = ii_twist_dir2;
            out_constraint.twist_part.r =
                utils::simd_inv(ii_twist_dir1.gdot(force_dir1) + ii_twist_dir2.gdot(-force_dir1));
            out_constraint.twist_dists = twist_dists;
        }

        // Tangent part.
        for j in 0..2 {
            let torque_dir1 = dp1.gcross(tangents1[j]);
            let torque_dir2 = dp2.gcross(-tangents1[j]);
            let ii_torque_dir1 = poses1.ii.transform_vector(torque_dir1);
            let ii_torque_dir2 = poses2.ii.transform_vector(torque_dir2);

            let imsum = poses1.im + poses2.im;

            let r = tangents1[j].gdot(imsum.component_mul(&tangents1[j]))
                + ii_torque_dir1.gdot(torque_dir1)
                + ii_torque_dir2.gdot(torque_dir2);

            // TODO: add something similar to tangent velocity to the twist
            //       constraint for the case where the different points don’t
            //       have the same tangent vel?
            let rhs_wo_bias = tangent_vel.gdot(tangents1[j]);

            out_constraint.tangent_part.torque_dir1[j] = torque_dir1;
            out_constraint.tangent_part.torque_dir2[j] = torque_dir2;
            out_constraint.tangent_part.ii_torque_dir1[j] = ii_torque_dir1;
            out_constraint.tangent_part.ii_torque_dir2[j] = ii_torque_dir2;
            out_constraint.tangent_part.rhs_wo_bias[j] = rhs_wo_bias;
            out_constraint.tangent_part.rhs[j] = rhs_wo_bias;
            out_constraint.tangent_part.r[j] = if cfg!(feature = "dim2") {
                utils::simd_inv(r)
            } else {
                r
            };
        }

        #[cfg(feature = "dim3")]
        {
            // TODO PERF: we already applied the inverse inertia to the torque
            //            dire before. Could we reuse the value instead of retransforming?
            out_constraint.tangent_part.r[2] = SimdReal::splat(2.0)
                * (out_constraint.tangent_part.ii_torque_dir1[0]
                    .gdot(out_constraint.tangent_part.torque_dir1[1])
                    + out_constraint.tangent_part.ii_torque_dir2[0]
                        .gdot(out_constraint.tangent_part.torque_dir2[1]));
        }
    }

    pub fn update(
        &self,
        params: &IntegrationParameters,
        solved_dt: Real,
        bodies: &SolverBodies,
        _multibodies: &MultibodyJointSet,
        constraint: &mut ContactWithTwistFriction<SimdReal>,
    ) {
        let cfm_factor = SimdReal::splat(params.contact_softness.cfm_factor(params.dt));
        let inv_dt = SimdReal::splat(params.inv_dt());
        let allowed_lin_err = SimdReal::splat(params.allowed_linear_error());
        let erp_inv_dt = SimdReal::splat(params.contact_softness.erp_inv_dt(params.dt));
        let max_corrective_velocity = SimdReal::splat(params.max_corrective_velocity());
        let warmstart_coeff = SimdReal::splat(params.warmstart_coefficient);

        let poses1 = bodies.gather_poses(constraint.solver_vel1);
        let poses2 = bodies.gather_poses(constraint.solver_vel2);
        let all_infos = &self.infos[..constraint.num_contacts as usize];
        let normal_parts = &mut constraint.normal_part[..constraint.num_contacts as usize];
        let tangent_part = &mut constraint.tangent_part;
        let twist_part = &mut constraint.twist_part;

        #[cfg(feature = "dim2")]
        let tangents1 = constraint.dir1.orthonormal_basis();
        #[cfg(feature = "dim3")]
        let tangents1 = [
            constraint.tangent1,
            constraint.dir1.gcross(constraint.tangent1),
        ];

        let solved_dt = SimdReal::splat(solved_dt);
        let tangent_delta = self.tangent_vel * solved_dt;

        for (info, normal_part) in all_infos.iter().zip(normal_parts.iter_mut()) {
            // NOTE: the tangent velocity is equivalent to an additional movement of the first body’s surface.
            let p1 = poses1.transform_point(info.local_p1) + tangent_delta;
            let p2 = poses2.transform_point(info.local_p2);
            let dist = info.dist + (p1 - p2).gdot(constraint.dir1);

            // Normal part.
            {
                let rhs_wo_bias = info.normal_vel + dist.simd_max(SimdReal::zero()) * inv_dt;
                let rhs_bias = ((dist + allowed_lin_err) * erp_inv_dt)
                    .simd_clamp(-max_corrective_velocity, SimdReal::zero());
                let new_rhs = rhs_wo_bias + rhs_bias;

                normal_part.rhs_wo_bias = rhs_wo_bias;
                normal_part.rhs = new_rhs;
                normal_part.impulse_accumulator += normal_part.impulse;
                normal_part.impulse *= warmstart_coeff;
            }
        }

        // tangent parts.
        {
            let p1 = poses1.transform_point(self.local_friction_center1) + tangent_delta;
            let p2 = poses2.transform_point(self.local_friction_center2);

            for j in 0..DIM - 1 {
                let bias = (p1 - p2).gdot(tangents1[j]) * inv_dt;
                tangent_part.rhs[j] = tangent_part.rhs_wo_bias[j] + bias;
            }
            tangent_part.impulse_accumulator += tangent_part.impulse;
            tangent_part.impulse *= warmstart_coeff;
            twist_part.impulse_accumulator += twist_part.impulse;
            twist_part.impulse *= warmstart_coeff;
        }

        constraint.cfm_factor = cfm_factor;
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub(crate) struct ContactWithTwistFriction<N: ScalarType> {
    pub dir1: N::Vector, // Non-penetration force direction for the first body.
    pub im1: N::Vector,
    pub im2: N::Vector,
    pub cfm_factor: N,
    pub limit: N,

    #[cfg(feature = "dim3")]
    pub tangent1: N::Vector, // One of the friction force directions.
    pub normal_part: [ContactConstraintNormalPart<N>; MAX_MANIFOLD_POINTS],
    // The twist friction model emulates coulomb with only one tangent
    // constraint + one twist constraint per manifold.
    pub tangent_part: ContactConstraintTangentPart<N>,
    // Twist constraint (angular-only) to compensate the lack of angular resistance on the tangent plane.
    pub twist_part: ContactConstraintTwistPart<N>,
    // Distances between the friction center and the contact point.
    pub twist_dists: [N; MAX_MANIFOLD_POINTS],

    pub solver_vel1: [u32; SIMD_WIDTH],
    pub solver_vel2: [u32; SIMD_WIDTH],
    pub manifold_id: [ContactManifoldIndex; SIMD_WIDTH],
    pub num_contacts: u8,
    pub manifold_contact_id: [[u8; SIMD_WIDTH]; MAX_MANIFOLD_POINTS],
}

impl ContactWithTwistFriction<SimdReal> {
    pub fn warmstart(&mut self, bodies: &mut SolverBodies) {
        let mut solver_vel1 = bodies.gather_vels(self.solver_vel1);
        let mut solver_vel2 = bodies.gather_vels(self.solver_vel2);

        let normal_parts = &mut self.normal_part[..self.num_contacts as usize];

        /*
         * Warmstart restitution.
         */
        for normal_part in normal_parts.iter_mut() {
            normal_part.warmstart(
                &self.dir1,
                &self.im1,
                &self.im2,
                &mut solver_vel1,
                &mut solver_vel2,
            );
        }

        /*
         * Warmstart friction.
         */
        #[cfg(feature = "dim3")]
        let tangents1 = [&self.tangent1, &self.dir1.gcross(self.tangent1)];
        #[cfg(feature = "dim2")]
        let tangents1 = [&self.dir1.orthonormal_vector()];

        self.tangent_part.warmstart(
            tangents1,
            &self.im1,
            &self.im2,
            &mut solver_vel1,
            &mut solver_vel2,
        );
        self.twist_part
            .warmstart(&mut solver_vel1, &mut solver_vel2);

        bodies.scatter_vels(self.solver_vel1, solver_vel1);
        bodies.scatter_vels(self.solver_vel2, solver_vel2);
    }

    pub fn solve(
        &mut self,
        bodies: &mut SolverBodies,
        solve_restitution: bool,
        solve_friction: bool,
    ) {
        let mut solver_vel1 = bodies.gather_vels(self.solver_vel1);
        let mut solver_vel2 = bodies.gather_vels(self.solver_vel2);

        let normal_parts = &mut self.normal_part[..self.num_contacts as usize];

        /*
         * Solve restitution.
         */
        if solve_restitution {
            for normal_part in normal_parts.iter_mut() {
                normal_part.solve(
                    self.cfm_factor,
                    &self.dir1,
                    &self.im1,
                    &self.im2,
                    &mut solver_vel1,
                    &mut solver_vel2,
                );
            }
        }

        /*
         * Solve friction.
         */
        if solve_friction {
            #[cfg(feature = "dim3")]
            let tangents1 = [&self.tangent1, &self.dir1.gcross(self.tangent1)];
            #[cfg(feature = "dim2")]
            let tangents1 = [&self.dir1.orthonormal_vector()];

            let mut tangent_limit = SimdReal::zero();
            let mut twist_limit = SimdReal::zero();
            for (normal_part, dist) in normal_parts.iter().zip(self.twist_dists.iter()) {
                tangent_limit += normal_part.impulse;
                // The twist limit is computed as the sum of impulses multiplied by the
                // lever-arm length relative to the friction center. The rational is that
                // the further the point is from the friction center, the stronger angular
                // resistance it can offer.
                twist_limit += normal_part.impulse * *dist;
            }

            // Multiply by the friction coefficient.
            tangent_limit *= self.limit;
            twist_limit *= self.limit;

            self.tangent_part.solve(
                tangents1,
                &self.im1,
                &self.im2,
                tangent_limit,
                &mut solver_vel1,
                &mut solver_vel2,
            );

            // NOTE: if there is only 1 contact, the twist part has no effect.
            if self.num_contacts > 1 {
                self.twist_part
                    .solve(&self.dir1, twist_limit, &mut solver_vel1, &mut solver_vel2);
            }
        }

        bodies.scatter_vels(self.solver_vel1, solver_vel1);
        bodies.scatter_vels(self.solver_vel2, solver_vel2);
    }

    pub fn writeback_impulses(&self, manifolds_all: &mut [&mut ContactManifold]) {
        let warmstart_tangent_impulses = self.tangent_part.impulse;
        #[cfg(feature = "simd-is-enabled")]
        let warmstart_twist_impulses: [_; SIMD_WIDTH] = self.twist_part.impulse.into();
        #[cfg(not(feature = "simd-is-enabled"))]
        let warmstart_twist_impulses: Real = self.twist_part.impulse;

        for k in 0..self.num_contacts as usize {
            #[cfg(not(feature = "simd-is-enabled"))]
            let warmstart_impulses: [_; SIMD_WIDTH] = [self.normal_part[k].impulse];
            #[cfg(feature = "simd-is-enabled")]
            let warmstart_impulses: [_; SIMD_WIDTH] = self.normal_part[k].impulse.into();
            #[cfg(not(feature = "simd-is-enabled"))]
            let impulses: [_; SIMD_WIDTH] = [self.normal_part[k].total_impulse()];
            #[cfg(feature = "simd-is-enabled")]
            let impulses: [_; SIMD_WIDTH] = self.normal_part[k].total_impulse().into();

            for ii in 0..SIMD_WIDTH {
                if self.manifold_id[ii] != usize::MAX {
                    let manifold = &mut manifolds_all[self.manifold_id[ii]];
                    let contact_id = self.manifold_contact_id[k][ii];
                    let active_contact = &mut manifold.points[contact_id as usize];
                    active_contact.data.warmstart_impulse = warmstart_impulses[ii];
                    active_contact.data.impulse = impulses[ii];
                    active_contact.data.warmstart_tangent_impulse =
                        warmstart_tangent_impulses.extract(ii);
                    #[cfg(feature = "simd-is-enabled")]
                    {
                        active_contact.data.warmstart_twist_impulse = warmstart_twist_impulses[ii];
                    }
                    #[cfg(not(feature = "simd-is-enabled"))]
                    {
                        active_contact.data.warmstart_twist_impulse = warmstart_twist_impulses;
                    }
                }
            }
        }
    }

    pub fn remove_cfm_and_bias_from_rhs(&mut self) {
        self.cfm_factor = SimdReal::splat(1.0);
        for elt in &mut self.normal_part {
            elt.rhs = elt.rhs_wo_bias;
        }

        self.tangent_part.rhs = self.tangent_part.rhs_wo_bias;
    }
}

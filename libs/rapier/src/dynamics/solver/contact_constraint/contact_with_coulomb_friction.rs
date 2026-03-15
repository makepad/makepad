use super::{ContactConstraintNormalPart, ContactConstraintTangentPart};
use crate::dynamics::integration_parameters::BLOCK_SOLVER_ENABLED;
use crate::dynamics::solver::solver_body::SolverBodies;
use crate::dynamics::{IntegrationParameters, MultibodyJointSet, RigidBodySet};
use crate::geometry::{ContactManifold, ContactManifoldIndex, SimdSolverContact};
use crate::math::{DIM, MAX_MANIFOLD_POINTS, Real, SIMD_WIDTH, SimdReal};
#[cfg(not(feature = "simd-is-enabled"))]
use crate::utils::ComponentMul;
#[cfg(feature = "dim2")]
use crate::utils::OrthonormalBasis;
use crate::utils::{self, AngularInertiaOps, CrossProduct, DotProduct, ScalarType};
use num::Zero;
use parry::utils::SdpMatrix2;
use simba::simd::{SimdPartialOrd, SimdValue};

#[derive(Copy, Clone, Debug)]
pub struct CoulombContactPointInfos<N: ScalarType> {
    pub tangent_vel: N::Vector, // PERF: could be one float less, be shared by both contact point infos?
    pub normal_vel: N,
    pub local_p1: N::Vector,
    pub local_p2: N::Vector,
    pub dist: N,
}

impl<N: ScalarType> Default for CoulombContactPointInfos<N> {
    fn default() -> Self {
        Self {
            tangent_vel: Default::default(),
            normal_vel: N::zero(),
            local_p1: Default::default(),
            local_p2: Default::default(),
            dist: N::zero(),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct ContactWithCoulombFrictionBuilder {
    infos: [CoulombContactPointInfos<SimdReal>; MAX_MANIFOLD_POINTS],
}

impl ContactWithCoulombFrictionBuilder {
    pub fn generate(
        manifold_id: [ContactManifoldIndex; SIMD_WIDTH],
        manifolds: [&ContactManifold; SIMD_WIDTH],
        bodies: &RigidBodySet,
        solver_bodies: &SolverBodies,
        out_builder: &mut ContactWithCoulombFrictionBuilder,
        out_constraint: &mut ContactWithCoulombFriction<SimdReal>,
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

        for k in 0..num_points {
            // SAFETY: we already know that the `manifold_points` has `num_points` elements
            //         so `k` isn’t out of bounds.
            let solver_contact =
                unsafe { SimdSolverContact::gather_unchecked(&manifold_points, k) };

            let is_bouncy = solver_contact.is_bouncy();

            let dp1 = solver_contact.point - world_com1;
            let dp2 = solver_contact.point - world_com2;

            let vel1 = vels1.linear + vels1.angular.gcross(dp1);
            let vel2 = vels2.linear + vels2.angular.gcross(dp2);

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

            // tangent parts.
            out_constraint.tangent_part[k].impulse = solver_contact.warmstart_tangent_impulse;

            for j in 0..DIM - 1 {
                let torque_dir1 = dp1.gcross(tangents1[j]);
                let torque_dir2 = dp2.gcross(-tangents1[j]);
                let ii_torque_dir1 = poses1.ii.transform_vector(torque_dir1);
                let ii_torque_dir2 = poses2.ii.transform_vector(torque_dir2);

                let imsum = poses1.im + poses2.im;

                let r = tangents1[j].gdot(imsum.component_mul(&tangents1[j]))
                    + ii_torque_dir1.gdot(torque_dir1)
                    + ii_torque_dir2.gdot(torque_dir2);
                let rhs_wo_bias = solver_contact.tangent_velocity.gdot(tangents1[j]);

                out_constraint.tangent_part[k].torque_dir1[j] = torque_dir1;
                out_constraint.tangent_part[k].torque_dir2[j] = torque_dir2;
                out_constraint.tangent_part[k].ii_torque_dir1[j] = ii_torque_dir1;
                out_constraint.tangent_part[k].ii_torque_dir2[j] = ii_torque_dir2;
                out_constraint.tangent_part[k].rhs_wo_bias[j] = rhs_wo_bias;
                out_constraint.tangent_part[k].rhs[j] = rhs_wo_bias;
                out_constraint.tangent_part[k].r[j] = if cfg!(feature = "dim2") {
                    utils::simd_inv(r)
                } else {
                    r
                };
            }

            #[cfg(feature = "dim3")]
            {
                // TODO PERF: we already applied the inverse inertia to the torque
                //            dire before. Could we reuse the value instead of retransforming?
                out_constraint.tangent_part[k].r[2] = SimdReal::splat(2.0)
                    * (out_constraint.tangent_part[k].ii_torque_dir1[0]
                        .gdot(out_constraint.tangent_part[k].torque_dir1[1])
                        + out_constraint.tangent_part[k].ii_torque_dir2[0]
                            .gdot(out_constraint.tangent_part[k].torque_dir2[1]));
            }

            // Builder.
            out_builder.infos[k].local_p1 = poses1.inverse_transform_point(solver_contact.point);
            out_builder.infos[k].local_p2 = poses2.inverse_transform_point(solver_contact.point);
            out_builder.infos[k].tangent_vel = solver_contact.tangent_velocity;
            out_builder.infos[k].dist = solver_contact.dist;
            out_builder.infos[k].normal_vel = normal_rhs_wo_bias;
        }

        if BLOCK_SOLVER_ENABLED {
            // Coupling between consecutive pairs.
            for k in 0..num_points / 2 {
                let k0 = k * 2;
                let k1 = k * 2 + 1;

                let imsum = poses1.im + poses2.im;
                let r0 = out_constraint.normal_part[k0].r;
                let r1 = out_constraint.normal_part[k1].r;

                let mut r_mat = SdpMatrix2::zero();

                // TODO PERF: we already applied the inverse inertia to the torque
                //            dire before. Could we reuse the value instead of retransforming?
                r_mat.m12 = force_dir1.gdot(imsum.component_mul(&force_dir1))
                    + out_constraint.normal_part[k0]
                        .ii_torque_dir1
                        .gdot(out_constraint.normal_part[k1].torque_dir1)
                    + out_constraint.normal_part[k0]
                        .ii_torque_dir2
                        .gdot(out_constraint.normal_part[k1].torque_dir2);
                r_mat.m11 = utils::simd_inv(r0);
                r_mat.m22 = utils::simd_inv(r1);

                let (inv, det) = {
                    let _disable_fe_except =
                            crate::utils::DisableFloatingPointExceptionsFlags::
                            disable_floating_point_exceptions();
                    r_mat.inverse_and_get_determinant_unchecked()
                };
                let is_invertible = det.simd_gt(SimdReal::zero());

                // If inversion failed, the contacts are redundant.
                // Ignore the one with the smallest depth (it is too late to
                // have the constraint removed from the constraint set, so just
                // set the mass (r) matrix elements to 0.
                out_constraint.normal_part[k0].r_mat_elts = [
                    inv.m11.select(is_invertible, r0),
                    inv.m22.select(is_invertible, SimdReal::zero()),
                ];
                out_constraint.normal_part[k1].r_mat_elts = [
                    inv.m12.select(is_invertible, SimdReal::zero()),
                    r_mat.m12.select(is_invertible, SimdReal::zero()),
                ];
            }
        }
    }

    pub fn update(
        &self,
        params: &IntegrationParameters,
        solved_dt: Real,
        bodies: &SolverBodies,
        _multibodies: &MultibodyJointSet,
        constraint: &mut ContactWithCoulombFriction<SimdReal>,
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
        let tangent_parts = &mut constraint.tangent_part[..constraint.num_contacts as usize];

        #[cfg(feature = "dim2")]
        let tangents1 = constraint.dir1.orthonormal_basis();
        #[cfg(feature = "dim3")]
        let tangents1 = [
            constraint.tangent1,
            constraint.dir1.gcross(constraint.tangent1),
        ];

        let solved_dt = SimdReal::splat(solved_dt);

        for ((info, normal_part), tangent_part) in all_infos
            .iter()
            .zip(normal_parts.iter_mut())
            .zip(tangent_parts.iter_mut())
        {
            // NOTE: the tangent velocity is equivalent to an additional movement of the first body’s surface.
            let p1 = poses1.transform_point(info.local_p1) + info.tangent_vel * solved_dt;
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

            // tangent parts.
            {
                tangent_part.impulse_accumulator += tangent_part.impulse;
                tangent_part.impulse *= warmstart_coeff;

                for j in 0..DIM - 1 {
                    let bias = (p1 - p2).gdot(tangents1[j]) * inv_dt;
                    tangent_part.rhs[j] = tangent_part.rhs_wo_bias[j] + bias;
                }
            }
        }

        constraint.cfm_factor = cfm_factor;
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub(crate) struct ContactWithCoulombFriction<N: ScalarType> {
    pub dir1: N::Vector, // Non-penetration force direction for the first body.
    pub im1: N::Vector,
    pub im2: N::Vector,
    pub cfm_factor: N,
    pub limit: N,

    #[cfg(feature = "dim3")]
    pub tangent1: N::Vector, // One of the friction force directions.
    pub normal_part: [ContactConstraintNormalPart<N>; MAX_MANIFOLD_POINTS],
    pub tangent_part: [ContactConstraintTangentPart<N>; MAX_MANIFOLD_POINTS],
    pub solver_vel1: [u32; SIMD_WIDTH],
    pub solver_vel2: [u32; SIMD_WIDTH],
    pub manifold_id: [ContactManifoldIndex; SIMD_WIDTH],
    pub num_contacts: u8,
    pub manifold_contact_id: [[u8; SIMD_WIDTH]; MAX_MANIFOLD_POINTS],
}

impl ContactWithCoulombFriction<SimdReal> {
    pub fn warmstart(&mut self, bodies: &mut SolverBodies) {
        let mut solver_vel1 = bodies.gather_vels(self.solver_vel1);
        let mut solver_vel2 = bodies.gather_vels(self.solver_vel2);

        let normal_parts = &mut self.normal_part[..self.num_contacts as usize];
        let tangent_parts = &mut self.tangent_part[..self.num_contacts as usize];

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

        for tangent_part in tangent_parts.iter_mut() {
            tangent_part.warmstart(
                tangents1,
                &self.im1,
                &self.im2,
                &mut solver_vel1,
                &mut solver_vel2,
            );
        }

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
        let tangent_parts = &mut self.tangent_part[..self.num_contacts as usize];

        /*
         * Solve restitution.
         */
        if solve_restitution {
            if BLOCK_SOLVER_ENABLED {
                for normal_part in normal_parts.chunks_exact_mut(2) {
                    let [normal_part_a, normal_part_b] = normal_part else {
                        unreachable!()
                    };

                    ContactConstraintNormalPart::solve_pair(
                        normal_part_a,
                        normal_part_b,
                        self.cfm_factor,
                        &self.dir1,
                        &self.im1,
                        &self.im2,
                        &mut solver_vel1,
                        &mut solver_vel2,
                    );
                }

                // There is one constraint left to solve if there isn’t an even number.
                if normal_parts.len() % 2 == 1 {
                    let normal_part = normal_parts.last_mut().unwrap();
                    normal_part.solve(
                        self.cfm_factor,
                        &self.dir1,
                        &self.im1,
                        &self.im2,
                        &mut solver_vel1,
                        &mut solver_vel2,
                    );
                }
            } else {
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
        }

        /*
         * Solve friction.
         */
        if solve_friction {
            #[cfg(feature = "dim3")]
            let tangents1 = [&self.tangent1, &self.dir1.gcross(self.tangent1)];
            #[cfg(feature = "dim2")]
            let tangents1 = [&self.dir1.orthonormal_vector()];

            for (tangent_part, normal_part) in tangent_parts.iter_mut().zip(normal_parts.iter()) {
                let limit = self.limit * normal_part.impulse;
                tangent_part.solve(
                    tangents1,
                    &self.im1,
                    &self.im2,
                    limit,
                    &mut solver_vel1,
                    &mut solver_vel2,
                );
            }
        }

        bodies.scatter_vels(self.solver_vel1, solver_vel1);
        bodies.scatter_vels(self.solver_vel2, solver_vel2);
    }

    pub fn writeback_impulses(&self, manifolds_all: &mut [&mut ContactManifold]) {
        for k in 0..self.num_contacts as usize {
            #[cfg(not(feature = "simd-is-enabled"))]
            let warmstart_impulses: [_; SIMD_WIDTH] = [self.normal_part[k].impulse];
            #[cfg(feature = "simd-is-enabled")]
            let warmstart_impulses: [_; SIMD_WIDTH] = self.normal_part[k].impulse.into();
            let warmstart_tangent_impulses = self.tangent_part[k].impulse;
            #[cfg(not(feature = "simd-is-enabled"))]
            let impulses: [_; SIMD_WIDTH] = [self.normal_part[k].total_impulse()];
            #[cfg(feature = "simd-is-enabled")]
            let impulses: [_; SIMD_WIDTH] = self.normal_part[k].total_impulse().into();
            let tangent_impulses = self.tangent_part[k].total_impulse();

            for ii in 0..SIMD_WIDTH {
                if self.manifold_id[ii] != usize::MAX {
                    let manifold = &mut manifolds_all[self.manifold_id[ii]];
                    let contact_id = self.manifold_contact_id[k][ii];
                    let active_contact = &mut manifold.points[contact_id as usize];
                    active_contact.data.warmstart_impulse = warmstart_impulses[ii];
                    active_contact.data.warmstart_tangent_impulse =
                        warmstart_tangent_impulses.extract(ii);
                    active_contact.data.impulse = impulses[ii];
                    active_contact.data.tangent_impulse = tangent_impulses.extract(ii);
                }
            }
        }
    }

    pub fn remove_cfm_and_bias_from_rhs(&mut self) {
        self.cfm_factor = SimdReal::splat(1.0);
        for elt in &mut self.normal_part {
            elt.rhs = elt.rhs_wo_bias;
        }
        for elt in &mut self.tangent_part {
            elt.rhs = elt.rhs_wo_bias;
        }
    }
}

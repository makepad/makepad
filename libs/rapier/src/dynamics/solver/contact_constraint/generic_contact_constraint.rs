use crate::dynamics::solver::GenericRhs;
use crate::dynamics::{IntegrationParameters, MultibodyJointSet, RigidBodySet};
use crate::geometry::{ContactManifold, ContactManifoldIndex};
use crate::math::{DIM, DVector, MAX_MANIFOLD_POINTS, Real};
use crate::utils::{AngularInertiaOps, CrossProduct, DotProduct};

use super::{ContactConstraintNormalPart, ContactConstraintTangentPart};
use crate::dynamics::solver::CoulombContactPointInfos;
use crate::dynamics::solver::solver_body::SolverBodies;
use crate::prelude::RigidBodyHandle;
#[cfg(feature = "dim2")]
use crate::utils::OrthonormalBasis;
use parry::math::Vector;

#[derive(Copy, Clone)]
pub(crate) struct GenericContactConstraintBuilder {
    infos: [CoulombContactPointInfos<Real>; MAX_MANIFOLD_POINTS],
    handle1: RigidBodyHandle,
    handle2: RigidBodyHandle,
    ccd_thickness: Real,
}

impl GenericContactConstraintBuilder {
    pub fn invalid() -> Self {
        Self {
            infos: [CoulombContactPointInfos::default(); MAX_MANIFOLD_POINTS],
            handle1: RigidBodyHandle::invalid(),
            handle2: RigidBodyHandle::invalid(),
            ccd_thickness: Real::MAX,
        }
    }

    pub fn generate(
        manifold_id: ContactManifoldIndex,
        manifold: &ContactManifold,
        bodies: &RigidBodySet,
        multibodies: &MultibodyJointSet,
        out_builder: &mut GenericContactConstraintBuilder,
        out_constraint: &mut GenericContactConstraint,
        jacobians: &mut DVector,
        jacobian_id: &mut usize,
    ) {
        // TODO PERF: we haven’t tried to optimized this codepath yet (since it relies
        //            on multibodies which are already much slower than regular bodies).
        let handle1 = manifold
            .data
            .rigid_body1
            .unwrap_or(RigidBodyHandle::invalid());
        let handle2 = manifold
            .data
            .rigid_body2
            .unwrap_or(RigidBodyHandle::invalid());

        let rb1 = &bodies.get(handle1).unwrap_or(&bodies.default_fixed);
        let rb2 = &bodies.get(handle2).unwrap_or(&bodies.default_fixed);

        let (vels1, mprops1, type1) = (&rb1.vels, &rb1.mprops, &rb1.body_type);
        let (vels2, mprops2, type2) = (&rb2.vels, &rb2.mprops, &rb2.body_type);

        let multibody1 = multibodies
            .rigid_body_link(handle1)
            .map(|m| (&multibodies[m.multibody], m.id));
        let multibody2 = multibodies
            .rigid_body_link(handle2)
            .map(|m| (&multibodies[m.multibody], m.id));
        let solver_vel1 =
            multibody1
                .map(|mb| mb.0.solver_id)
                .unwrap_or(if type1.is_dynamic_or_kinematic() {
                    rb1.ids.active_set_id as u32
                } else {
                    u32::MAX
                });
        let solver_vel2 =
            multibody2
                .map(|mb| mb.0.solver_id)
                .unwrap_or(if type2.is_dynamic_or_kinematic() {
                    rb2.ids.active_set_id as u32
                } else {
                    u32::MAX
                });
        let force_dir1 = -manifold.data.normal;

        #[cfg(feature = "dim2")]
        let tangents1 = force_dir1.orthonormal_basis();
        #[cfg(feature = "dim3")]
        let tangents1 = super::compute_tangent_contact_directions::<Real>(
            &force_dir1,
            &vels1.linvel,
            &vels2.linvel,
        );

        let multibodies_ndof = multibody1.map(|m| m.0.ndofs()).unwrap_or(0)
            + multibody2.map(|m| m.0.ndofs()).unwrap_or(0);
        // For each solver contact we generate DIM constraints, and each constraints appends
        // the multibodies jacobian and weighted jacobians
        let required_jacobian_len =
            *jacobian_id + manifold.data.solver_contacts.len() * multibodies_ndof * 2 * DIM;

        if jacobians.nrows() < required_jacobian_len && !cfg!(feature = "parallel") {
            jacobians.resize_vertically_mut(required_jacobian_len, 0.0);
        }

        let chunk_j_id = *jacobian_id;

        let manifold_points = &manifold.data.solver_contacts;
        out_constraint.dir1 = force_dir1;
        out_constraint.im1 = if type1.is_dynamic_or_kinematic() {
            mprops1.effective_inv_mass
        } else {
            Vector::ZERO
        };
        out_constraint.im2 = if type2.is_dynamic_or_kinematic() {
            mprops2.effective_inv_mass
        } else {
            Vector::ZERO
        };
        out_constraint.solver_vel1 = solver_vel1;
        out_constraint.solver_vel2 = solver_vel2;
        out_constraint.manifold_id = manifold_id;
        out_constraint.num_contacts = manifold_points.len() as u8;
        #[cfg(feature = "dim3")]
        {
            out_constraint.tangent1 = tangents1[0];
        }

        for k in 0..manifold_points.len() {
            let manifold_point = &manifold_points[k];
            let point = manifold_point.point;
            let dp1 = point - mprops1.world_com;
            let dp2 = point - mprops2.world_com;

            let vel1 = vels1.linvel + vels1.angvel.gcross(dp1);
            let vel2 = vels2.linvel + vels2.angvel.gcross(dp2);

            out_constraint.limit = manifold_point.friction;
            out_constraint.manifold_contact_id[k] = manifold_point.contact_id[0] as u8;

            // Normal part.
            let normal_rhs_wo_bias;
            {
                let torque_dir1 = dp1.gcross(force_dir1);
                let torque_dir2 = dp2.gcross(-force_dir1);

                let ii_torque_dir1 = if type1.is_dynamic_or_kinematic() {
                    mprops1
                        .effective_world_inv_inertia
                        .transform_vector(torque_dir1)
                } else {
                    Default::default()
                };
                let ii_torque_dir2 = if type2.is_dynamic_or_kinematic() {
                    mprops2
                        .effective_world_inv_inertia
                        .transform_vector(torque_dir2)
                } else {
                    Default::default()
                };

                let inv_r1 = if let Some((mb1, link_id1)) = multibody1.as_ref() {
                    mb1.fill_jacobians(*link_id1, force_dir1, torque_dir1, jacobian_id, jacobians)
                        .0
                } else if type1.is_dynamic_or_kinematic() {
                    force_dir1.dot(mprops1.effective_inv_mass * force_dir1)
                        + ii_torque_dir1.gdot(torque_dir1)
                } else {
                    0.0
                };

                let inv_r2 = if let Some((mb2, link_id2)) = multibody2.as_ref() {
                    mb2.fill_jacobians(*link_id2, -force_dir1, torque_dir2, jacobian_id, jacobians)
                        .0
                } else if type2.is_dynamic_or_kinematic() {
                    force_dir1.dot(mprops2.effective_inv_mass * force_dir1)
                        + ii_torque_dir2.gdot(torque_dir2)
                } else {
                    0.0
                };

                let r = crate::utils::inv(inv_r1 + inv_r2);

                let is_bouncy = manifold_point.is_bouncy() as u32 as Real;

                normal_rhs_wo_bias =
                    (is_bouncy * manifold_point.restitution) * (vel1 - vel2).dot(force_dir1);

                out_constraint.normal_part[k] = ContactConstraintNormalPart {
                    torque_dir1,
                    torque_dir2,
                    ii_torque_dir1,
                    ii_torque_dir2,
                    rhs: Default::default(),
                    rhs_wo_bias: Default::default(),
                    impulse_accumulator: Default::default(),
                    impulse: manifold_point.warmstart_impulse,
                    r,
                    r_mat_elts: [0.0; 2],
                };
            }

            // Tangent parts.
            {
                out_constraint.tangent_part[k].impulse = manifold_point.warmstart_tangent_impulse;

                for j in 0..DIM - 1 {
                    let torque_dir1 = dp1.gcross(tangents1[j]);
                    let ii_torque_dir1 = if type1.is_dynamic_or_kinematic() {
                        mprops1
                            .effective_world_inv_inertia
                            .transform_vector(torque_dir1)
                    } else {
                        Default::default()
                    };
                    out_constraint.tangent_part[k].torque_dir1[j] = torque_dir1;
                    out_constraint.tangent_part[k].ii_torque_dir1[j] = ii_torque_dir1;

                    let torque_dir2 = dp2.gcross(-tangents1[j]);
                    let ii_torque_dir2 = if type2.is_dynamic_or_kinematic() {
                        mprops2
                            .effective_world_inv_inertia
                            .transform_vector(torque_dir2)
                    } else {
                        Default::default()
                    };
                    out_constraint.tangent_part[k].torque_dir2[j] = torque_dir2;
                    out_constraint.tangent_part[k].ii_torque_dir2[j] = ii_torque_dir2;

                    let tangent_glam = tangents1[j];
                    let inv_r1 = if let Some((mb1, link_id1)) = multibody1.as_ref() {
                        mb1.fill_jacobians(
                            *link_id1,
                            tangent_glam,
                            torque_dir1,
                            jacobian_id,
                            jacobians,
                        )
                        .0
                    } else if type1.is_dynamic_or_kinematic() {
                        tangent_glam.dot(mprops1.effective_inv_mass * tangent_glam)
                            + ii_torque_dir1.gdot(torque_dir1)
                    } else {
                        0.0
                    };

                    let inv_r2 = if let Some((mb2, link_id2)) = multibody2.as_ref() {
                        mb2.fill_jacobians(
                            *link_id2,
                            -tangent_glam,
                            torque_dir2,
                            jacobian_id,
                            jacobians,
                        )
                        .0
                    } else if type2.is_dynamic_or_kinematic() {
                        tangent_glam.dot(mprops2.effective_inv_mass * tangent_glam)
                            + ii_torque_dir2.gdot(torque_dir2)
                    } else {
                        0.0
                    };

                    let r = crate::utils::inv(inv_r1 + inv_r2);
                    let rhs_wo_bias = manifold_point.tangent_velocity.gdot(tangents1[j]);

                    out_constraint.tangent_part[k].rhs_wo_bias[j] = rhs_wo_bias;
                    out_constraint.tangent_part[k].rhs[j] = rhs_wo_bias;

                    // TODO: in 3D, we should take into account gcross[0].dot(gcross[1])
                    // in lhs. See the corresponding code on the `velocity_constraint.rs`
                    // file.
                    out_constraint.tangent_part[k].r[j] = r;
                }
            }

            // Builder.
            let infos = CoulombContactPointInfos {
                local_p1: rb1
                    .pos
                    .position
                    .inverse_transform_point(manifold_point.point),
                local_p2: rb2
                    .pos
                    .position
                    .inverse_transform_point(manifold_point.point),
                tangent_vel: manifold_point.tangent_velocity,
                dist: manifold_point.dist,
                normal_vel: normal_rhs_wo_bias,
            };

            out_builder.handle1 = handle1;
            out_builder.handle2 = handle2;
            out_builder.ccd_thickness = rb1.ccd.ccd_thickness + rb2.ccd.ccd_thickness;
            out_builder.infos[k] = infos;
            out_constraint.manifold_contact_id[k] = manifold_point.contact_id[0] as u8;
        }

        let ndofs1 = multibody1.map(|mb| mb.0.ndofs()).unwrap_or(0);
        let ndofs2 = multibody2.map(|mb| mb.0.ndofs()).unwrap_or(0);

        // NOTE: we use the generic constraint for non-dynamic bodies because this will
        //       reduce all ops to nothing because its ndofs will be zero.
        let generic_constraint_mask = (multibody1.is_some() as u8)
            | ((multibody2.is_some() as u8) << 1)
            | (!type1.is_dynamic_or_kinematic() as u8)
            | ((!type2.is_dynamic_or_kinematic() as u8) << 1);

        out_constraint.j_id = chunk_j_id;
        out_constraint.ndofs1 = ndofs1;
        out_constraint.ndofs2 = ndofs2;
        out_constraint.generic_constraint_mask = generic_constraint_mask;
    }

    pub fn update(
        &self,
        params: &IntegrationParameters,
        solved_dt: Real,
        bodies: &SolverBodies,
        multibodies: &MultibodyJointSet,
        constraint: &mut GenericContactConstraint,
    ) {
        let cfm_factor = params.contact_softness.cfm_factor(params.dt);
        let inv_dt = params.inv_dt();
        let erp_inv_dt = params.contact_softness.erp_inv_dt(params.dt);

        // We don't update jacobians so the update is mostly identical to the non-generic velocity constraint.
        let pose1 = multibodies
            .rigid_body_link(self.handle1)
            .map(|m| multibodies[m.multibody].link(m.id).unwrap().local_to_world)
            .unwrap_or_else(|| bodies.get_pose(constraint.solver_vel1).pose());
        let pose2 = multibodies
            .rigid_body_link(self.handle2)
            .map(|m| multibodies[m.multibody].link(m.id).unwrap().local_to_world)
            .unwrap_or_else(|| bodies.get_pose(constraint.solver_vel2).pose());
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

        let dir1_na = constraint.dir1;
        for ((info, normal_part), tangent_part) in all_infos
            .iter()
            .zip(normal_parts.iter_mut())
            .zip(tangent_parts.iter_mut())
        {
            // Tangent velocity is equivalent to the first body's surface moving artificially.
            let p1 = pose1 * info.local_p1 + info.tangent_vel * solved_dt;
            let p2 = pose2 * info.local_p2;
            let dist = info.dist + (p1 - p2).gdot(dir1_na);

            // Normal part.
            {
                let rhs_wo_bias = info.normal_vel + dist.max(0.0) * inv_dt;
                let rhs_bias = (erp_inv_dt * (dist + params.allowed_linear_error()))
                    .clamp(-params.max_corrective_velocity(), 0.0);
                let new_rhs = rhs_wo_bias + rhs_bias;

                normal_part.rhs_wo_bias = rhs_wo_bias;
                normal_part.rhs = new_rhs;
                normal_part.impulse_accumulator += normal_part.impulse;
                normal_part.impulse *= params.warmstart_coefficient;
            }

            // Tangent part.
            {
                tangent_part.impulse_accumulator += tangent_part.impulse;
                tangent_part.impulse *= params.warmstart_coefficient;

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
pub(crate) struct GenericContactConstraint {
    /*
     * Fields specific to multibodies.
     */
    pub j_id: usize,
    pub ndofs1: usize,
    pub ndofs2: usize,
    pub generic_constraint_mask: u8,

    /*
     * Fields similar to the rigid-body constraints.
     */
    pub dir1: Vector, // Non-penetration force direction for the first body.
    #[cfg(feature = "dim3")]
    pub tangent1: Vector, // One of the friction force directions.
    pub im1: Vector,
    pub im2: Vector,
    pub cfm_factor: Real,
    pub limit: Real,
    pub solver_vel1: u32,
    pub solver_vel2: u32,
    pub manifold_id: ContactManifoldIndex,
    pub manifold_contact_id: [u8; MAX_MANIFOLD_POINTS],
    pub num_contacts: u8,
    pub normal_part: [ContactConstraintNormalPart<Real>; MAX_MANIFOLD_POINTS],
    pub tangent_part: [ContactConstraintTangentPart<Real>; MAX_MANIFOLD_POINTS],
}

impl GenericContactConstraint {
    pub fn invalid() -> Self {
        Self {
            j_id: usize::MAX,
            ndofs1: usize::MAX,
            ndofs2: usize::MAX,
            generic_constraint_mask: u8::MAX,
            dir1: Vector::ZERO,
            #[cfg(feature = "dim3")]
            tangent1: Vector::ZERO,
            im1: Vector::ZERO,
            im2: Vector::ZERO,
            cfm_factor: 0.0,
            limit: 0.0,
            solver_vel1: u32::MAX,
            solver_vel2: u32::MAX,
            manifold_id: ContactManifoldIndex::MAX,
            manifold_contact_id: [u8::MAX; MAX_MANIFOLD_POINTS],
            num_contacts: u8::MAX,
            normal_part: [ContactConstraintNormalPart::zero(); MAX_MANIFOLD_POINTS],
            tangent_part: [ContactConstraintTangentPart::zero(); MAX_MANIFOLD_POINTS],
        }
    }

    pub fn warmstart(
        &mut self,
        jacobians: &DVector,
        bodies: &mut SolverBodies,
        generic_solver_vels: &mut DVector,
    ) {
        let mut solver_vel1 = if self.solver_vel1 == u32::MAX {
            GenericRhs::Fixed
        } else if self.generic_constraint_mask & 0b01 == 0 {
            GenericRhs::SolverVel(bodies.vels[self.solver_vel1 as usize])
        } else {
            GenericRhs::GenericId(self.solver_vel1)
        };

        let mut solver_vel2 = if self.solver_vel2 == u32::MAX {
            GenericRhs::Fixed
        } else if self.generic_constraint_mask & 0b10 == 0 {
            GenericRhs::SolverVel(bodies.vels[self.solver_vel2 as usize])
        } else {
            GenericRhs::GenericId(self.solver_vel2)
        };

        let tangent_parts = &mut self.tangent_part[..self.num_contacts as usize];
        let normal_parts = &mut self.normal_part[..self.num_contacts as usize];
        Self::generic_warmstart_group(
            normal_parts,
            tangent_parts,
            jacobians,
            self.dir1,
            #[cfg(feature = "dim3")]
            self.tangent1,
            self.im1,
            self.im2,
            self.ndofs1,
            self.ndofs2,
            self.j_id,
            &mut solver_vel1,
            &mut solver_vel2,
            generic_solver_vels,
        );

        if let GenericRhs::SolverVel(solver_vel1) = solver_vel1 {
            bodies.vels[self.solver_vel1 as usize] = solver_vel1;
        }

        if let GenericRhs::SolverVel(solver_vel2) = solver_vel2 {
            bodies.vels[self.solver_vel2 as usize] = solver_vel2;
        }
    }

    pub fn solve(
        &mut self,
        jacobians: &DVector,
        bodies: &mut SolverBodies,
        generic_solver_vels: &mut DVector,
        solve_restitution: bool,
        solve_friction: bool,
    ) {
        let mut solver_vel1 = if self.solver_vel1 == u32::MAX {
            GenericRhs::Fixed
        } else if self.generic_constraint_mask & 0b01 == 0 {
            GenericRhs::SolverVel(bodies.vels[self.solver_vel1 as usize])
        } else {
            GenericRhs::GenericId(self.solver_vel1)
        };

        let mut solver_vel2 = if self.solver_vel2 == u32::MAX {
            GenericRhs::Fixed
        } else if self.generic_constraint_mask & 0b10 == 0 {
            GenericRhs::SolverVel(bodies.vels[self.solver_vel2 as usize])
        } else {
            GenericRhs::GenericId(self.solver_vel2)
        };

        let normal_parts = &mut self.normal_part[..self.num_contacts as usize];
        let tangent_parts = &mut self.tangent_part[..self.num_contacts as usize];
        Self::generic_solve_group(
            self.cfm_factor,
            normal_parts,
            tangent_parts,
            jacobians,
            self.dir1,
            #[cfg(feature = "dim3")]
            self.tangent1,
            self.im1,
            self.im2,
            self.limit,
            self.ndofs1,
            self.ndofs2,
            self.j_id,
            &mut solver_vel1,
            &mut solver_vel2,
            generic_solver_vels,
            solve_restitution,
            solve_friction,
        );

        if let GenericRhs::SolverVel(solver_vel1) = solver_vel1 {
            bodies.vels[self.solver_vel1 as usize] = solver_vel1;
        }

        if let GenericRhs::SolverVel(solver_vel2) = solver_vel2 {
            bodies.vels[self.solver_vel2 as usize] = solver_vel2;
        }
    }

    pub fn writeback_impulses(&self, manifolds_all: &mut [&mut ContactManifold]) {
        let manifold = &mut manifolds_all[self.manifold_id];

        for k in 0..self.num_contacts as usize {
            let contact_id = self.manifold_contact_id[k];
            let active_contact = &mut manifold.points[contact_id as usize];
            active_contact.data.warmstart_impulse = self.normal_part[k].impulse;
            active_contact.data.warmstart_tangent_impulse = self.tangent_part[k].impulse;
            active_contact.data.impulse = self.normal_part[k].total_impulse();
            active_contact.data.tangent_impulse = self.tangent_part[k].total_impulse();
        }
    }

    pub fn remove_cfm_and_bias_from_rhs(&mut self) {
        self.cfm_factor = 1.0;
        for normal_part in &mut self.normal_part {
            normal_part.rhs = normal_part.rhs_wo_bias;
        }
        for tangent_part in &mut self.tangent_part {
            tangent_part.rhs = tangent_part.rhs_wo_bias;
        }
    }
}

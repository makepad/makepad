use crate::dynamics::solver::JointConstraintsSet;
use crate::dynamics::solver::contact_constraint::ContactConstraintsSet;
use crate::dynamics::solver::solver_body::SolverBodies;
use crate::dynamics::{
    IntegrationParameters, IslandManager, JointGraphEdge, JointIndex, MultibodyJointSet,
    MultibodyLinkId, RigidBodySet, RigidBodyType, solver::SolverVel,
};
use crate::geometry::{ContactManifold, ContactManifoldIndex};
use crate::math::{DVector, Real};
use crate::prelude::RigidBodyVelocity;
use parry::math::SIMD_WIDTH;

#[cfg(feature = "dim3")]
use crate::dynamics::FrictionModel;

pub(crate) struct VelocitySolver {
    pub solver_bodies: SolverBodies,
    pub solver_vels_increment: Vec<SolverVel<Real>>,
    pub generic_solver_vels: DVector,
    pub generic_solver_vels_increment: DVector,
    pub multibody_roots: Vec<MultibodyLinkId>,
}

impl VelocitySolver {
    pub fn new() -> Self {
        Self {
            solver_bodies: SolverBodies::default(),
            solver_vels_increment: Vec::new(),
            generic_solver_vels: DVector::zeros(0),
            generic_solver_vels_increment: DVector::zeros(0),
            multibody_roots: Vec::new(),
        }
    }

    pub fn init_constraints(
        &self,
        island_id: usize,
        islands: &IslandManager,
        bodies: &mut RigidBodySet,
        multibodies: &mut MultibodyJointSet,
        manifolds_all: &mut [&mut ContactManifold],
        manifold_indices: &[ContactManifoldIndex],
        joints_all: &mut [JointGraphEdge],
        joint_indices: &[JointIndex],
        contact_constraints: &mut ContactConstraintsSet,
        joint_constraints: &mut JointConstraintsSet,
        #[cfg(feature = "dim3")] friction_model: FrictionModel,
    ) {
        contact_constraints.init(
            island_id,
            islands,
            bodies,
            &self.solver_bodies,
            multibodies,
            manifolds_all,
            manifold_indices,
            #[cfg(feature = "dim3")]
            friction_model,
        );

        joint_constraints.init(
            island_id,
            islands,
            bodies,
            multibodies,
            joints_all,
            joint_indices,
        );
    }

    pub fn init_solver_velocities_and_solver_bodies(
        &mut self,
        total_step_dt: Real,
        params: &IntegrationParameters,
        island_id: usize,
        islands: &IslandManager,
        bodies: &mut RigidBodySet,
        multibodies: &mut MultibodyJointSet,
    ) {
        self.multibody_roots.clear();
        self.solver_bodies.clear();

        let aligned_solver_bodies_len =
            islands.island(island_id).len().div_ceil(SIMD_WIDTH) * SIMD_WIDTH;
        self.solver_bodies.resize(aligned_solver_bodies_len);

        self.solver_vels_increment.clear();
        self.solver_vels_increment
            .resize(aligned_solver_bodies_len, SolverVel::zero());

        /*
         * Initialize solver bodies and delta-velocities (`solver_vels_increment`) with external forces (gravity etc):
         * NOTE: we compute this only once by neglecting changes of mass matrices.
         */

        // Assign solver ids to multibodies, and collect the relevant roots.
        // And init solver_vels for rigid-bodies.
        let mut multibody_solver_id = 0;

        for (offset, handle) in islands.island(island_id).bodies().iter().enumerate() {
            if let Some(link) = multibodies.rigid_body_link(*handle).copied() {
                let multibody = multibodies
                    .get_multibody_mut_internal(link.multibody)
                    .unwrap();

                if link.id == 0 || link.id == 1 && !multibody.root_is_dynamic {
                    multibody.solver_id = multibody_solver_id;
                    multibody_solver_id += multibody.ndofs() as u32;
                    self.multibody_roots.push(link);
                }
            } else {
                let rb = &bodies[*handle];
                assert_eq!(offset, rb.ids.active_set_id);
                let solver_vel_incr = &mut self.solver_vels_increment[rb.ids.active_set_id];
                self.solver_bodies
                    .copy_from(total_step_dt, rb.ids.active_set_id, rb);

                solver_vel_incr.angular =
                    rb.mprops.effective_world_inv_inertia * rb.forces.torque * params.dt;
                solver_vel_incr.linear = rb.forces.force * rb.mprops.effective_inv_mass * params.dt;
            }
        }

        // TODO PERF: don’t reallocate at each iteration.
        self.generic_solver_vels_increment = DVector::zeros(multibody_solver_id as usize);
        self.generic_solver_vels = DVector::zeros(multibody_solver_id as usize);

        // init solver_vels for multibodies.
        for link in &self.multibody_roots {
            let multibody = multibodies
                .get_multibody_mut_internal(link.multibody)
                .unwrap();
            multibody.update_dynamics(params.dt, bodies);
            multibody.update_acceleration(bodies);

            let mut solver_vels_incr = self
                .generic_solver_vels_increment
                .rows_mut(multibody.solver_id as usize, multibody.ndofs());
            let mut solver_vels = self
                .generic_solver_vels
                .rows_mut(multibody.solver_id as usize, multibody.ndofs());

            solver_vels_incr.axpy(params.dt, &multibody.accelerations, 0.0);
            solver_vels.copy_from(&multibody.velocities);
        }
    }
    pub fn solve_constraints(
        &mut self,
        params: &IntegrationParameters,
        num_substeps: usize,
        bodies: &mut RigidBodySet,
        multibodies: &mut MultibodyJointSet,
        contact_constraints: &mut ContactConstraintsSet,
        joint_constraints: &mut JointConstraintsSet,
    ) {
        for substep_id in 0..num_substeps {
            let is_last_substep = substep_id == num_substeps - 1;

            // TODO PERF: could easily use SIMD.
            for (solver_vels, incr) in self
                .solver_bodies
                .vels
                .iter_mut()
                .zip(self.solver_vels_increment.iter())
            {
                solver_vels.linear += incr.linear;
                solver_vels.angular += incr.angular;
            }

            self.generic_solver_vels += &self.generic_solver_vels_increment;

            /*
             * Update & solve constraints with bias.
             */
            joint_constraints.update(params, multibodies, &self.solver_bodies);
            contact_constraints.update(params, substep_id, multibodies, &self.solver_bodies);

            if params.warmstart_coefficient != 0.0 {
                // TODO PERF: we could probably figure out a way to avoid this warmstart when
                //            step_id > 0? Maybe for that to happen `solver_vels` needs to
                //            represent velocity changes instead of total rigid-body velocities.
                //            Need to be careful wrt. multibody and joints too.
                contact_constraints
                    .warmstart(&mut self.solver_bodies, &mut self.generic_solver_vels);
            }

            for _ in 0..params.num_internal_pgs_iterations {
                joint_constraints.solve(&mut self.solver_bodies, &mut self.generic_solver_vels);
                contact_constraints.solve(&mut self.solver_bodies, &mut self.generic_solver_vels);
            }

            /*
             * Integrate positions.
             */
            self.integrate_positions(params, is_last_substep, bodies, multibodies);

            /*
             * Resolution without bias.
             */
            for _ in 0..params.num_internal_stabilization_iterations {
                joint_constraints
                    .solve_wo_bias(&mut self.solver_bodies, &mut self.generic_solver_vels);
                contact_constraints
                    .solve_wo_bias(&mut self.solver_bodies, &mut self.generic_solver_vels);
            }
        }
    }
    pub fn integrate_positions(
        &mut self,
        params: &IntegrationParameters,
        is_last_substep: bool,
        bodies: &mut RigidBodySet,
        multibodies: &mut MultibodyJointSet,
    ) {
        for (solver_vels, solver_pose) in self
            .solver_bodies
            .vels
            .iter()
            .zip(self.solver_bodies.poses.iter_mut())
        {
            let linvel = solver_vels.linear;
            let angvel = solver_vels.angular;

            // TODO: should we add a compile flag (or a simulation parameter)
            //       to disable the rotation linearization?
            let new_vels = RigidBodyVelocity { linvel, angvel };
            new_vels.integrate_linearized(
                params.dt,
                &mut solver_pose.translation,
                &mut solver_pose.rotation,
            );
        }

        // TODO PERF: SIMD-optimized integration. Works fine, but doesn’t run faster than the scalar
        //            one (tested on Apple Silicon/Neon, might be worth double-checking on x86_64/SSE2).
        // // SAFETY: this assertion ensures the unchecked gathers are sound.
        // assert_eq!(self.solver_bodies.len() % SIMD_WIDTH, 0);
        // let dt = SimdReal::splat(params.dt);
        // for i in (0..self.solver_bodies.len()).step_by(SIMD_WIDTH) {
        //     let idx = [i, i + 1, i + 2, i + 3];
        //     let solver_vels = unsafe { self.solver_bodies.gather_vels_unchecked(idx) };
        //     let mut solver_poses = unsafe { self.solver_bodies.gather_poses_unchecked(idx) };
        //     // let solver_consts = unsafe { self.solver_bodies.gather_consts_unchecked(idx) };
        //
        //     let linvel = solver_vels.linear;
        //     let angvel = solver_poses.ii_sqrt.transform_vector(solver_vels.angular);
        //
        //     let mut new_vels = RigidBodyVelocity { linvel, angvel };
        //     // TODO: store the post-damping velocity?
        //     // new_vels = new_vels.apply_damping(dt, &solver_consts.damping);
        //     new_vels.integrate_linearized(dt, &mut solver_poses.pose);
        //     self.solver_bodies
        //         .scatter_poses_unchecked(idx, solver_poses);
        // }

        // Integrate multibody positions.
        for link in &self.multibody_roots {
            let multibody = multibodies
                .get_multibody_mut_internal(link.multibody)
                .unwrap();
            let solver_vels = self
                .generic_solver_vels
                .rows(multibody.solver_id as usize, multibody.ndofs());
            multibody.velocities.copy_from(&solver_vels);
            multibody.integrate(params.dt);
            // PERF: don’t write back to the rigid-body poses `bodies` before the last step?
            multibody.forward_kinematics(bodies, false);
            multibody.update_rigid_bodies_internal(bodies, !is_last_substep, true, false);

            if !is_last_substep {
                // These are very expensive and not needed if we don’t
                // have to run another step.
                multibody.update_dynamics(params.dt, bodies);
                multibody.update_acceleration(bodies);

                let mut solver_vels_incr = self
                    .generic_solver_vels_increment
                    .rows_mut(multibody.solver_id as usize, multibody.ndofs());
                solver_vels_incr.axpy(params.dt, &multibody.accelerations, 0.0);
            }
        }
    }

    pub fn writeback_bodies(
        &mut self,
        params: &IntegrationParameters,
        islands: &IslandManager,
        island_id: usize,
        bodies: &mut RigidBodySet,
        multibodies: &mut MultibodyJointSet,
    ) {
        for handle in islands.island(island_id).bodies() {
            let link = if self.multibody_roots.is_empty() {
                None
            } else {
                multibodies.rigid_body_link(*handle).copied()
            };

            if let Some(link) = link {
                let multibody = multibodies
                    .get_multibody_mut_internal(link.multibody)
                    .unwrap();

                if link.id == 0 || link.id == 1 && !multibody.root_is_dynamic {
                    let solver_vels = self
                        .generic_solver_vels
                        .rows(multibody.solver_id as usize, multibody.ndofs());
                    multibody.velocities.copy_from(&solver_vels);
                }
            } else {
                let rb = bodies.index_mut_internal(*handle);
                let solver_vels = &self.solver_bodies.vels[rb.ids.active_set_id];
                let solver_poses = &self.solver_bodies.poses[rb.ids.active_set_id];

                let dangvel = solver_vels.angular;

                let mut new_vels = RigidBodyVelocity {
                    linvel: solver_vels.linear,
                    angvel: dangvel,
                };
                new_vels = new_vels.apply_damping(params.dt, &rb.damping);

                rb.vels = new_vels;

                // NOTE: if it's a position-based kinematic body, don't writeback as we want
                //       to preserve exactly the value given by the user (it might not be exactly
                //       equal to the integrated position because of rounding errors).
                if rb.body_type != RigidBodyType::KinematicPositionBased {
                    let local_com = -rb.mprops.local_mprops.local_com;
                    rb.pos.next_position = solver_poses.pose().prepend_translation(local_com);
                }

                if rb.ccd.ccd_enabled {
                    // TODO: Is storing this still necessary instead of just recomputing it
                    //       during CCD?
                    rb.ccd_vels = rb
                        .pos
                        .interpolate_velocity(params.inv_dt(), rb.local_center_of_mass());
                } else {
                    rb.ccd_vels = RigidBodyVelocity::zero();
                }
            }
        }
    }
}

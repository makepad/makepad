use crate::dynamics::solver::categorization::categorize_joints;
use crate::dynamics::solver::{
    AnyJointConstraintMut, GenericJointConstraint, JointGenericExternalConstraintBuilder,
    JointGenericInternalConstraintBuilder, reset_buffer,
};
use crate::dynamics::{
    IntegrationParameters, IslandManager, JointGraphEdge, JointIndex, MultibodyJointSet,
    RigidBodySet,
};
use crate::math::DVector;
use parry::math::Real;

use crate::dynamics::solver::interaction_groups::InteractionGroups;
use crate::dynamics::solver::joint_constraint::generic_joint_constraint_builder::GenericJointConstraintBuilder;
use crate::dynamics::solver::joint_constraint::joint_constraint_builder::JointConstraintBuilder;
use crate::dynamics::solver::joint_constraint::joint_velocity_constraint::JointConstraint;
use crate::dynamics::solver::solver_body::SolverBodies;
#[cfg(feature = "simd-is-enabled")]
use {
    crate::dynamics::solver::joint_constraint::joint_constraint_builder::JointConstraintBuilderSimd,
    crate::math::{SIMD_WIDTH, SimdReal},
};

pub struct JointConstraintsSet {
    pub generic_jacobians: DVector,
    pub two_body_interactions: Vec<usize>,
    pub generic_two_body_interactions: Vec<usize>,
    pub interaction_groups: InteractionGroups,

    pub generic_velocity_constraints: Vec<GenericJointConstraint>,
    pub velocity_constraints: Vec<JointConstraint<Real, 1>>,
    #[cfg(feature = "simd-is-enabled")]
    pub simd_velocity_constraints: Vec<JointConstraint<SimdReal, SIMD_WIDTH>>,

    pub generic_velocity_constraints_builder: Vec<GenericJointConstraintBuilder>,
    pub velocity_constraints_builder: Vec<JointConstraintBuilder>,
    #[cfg(feature = "simd-is-enabled")]
    pub simd_velocity_constraints_builder: Vec<JointConstraintBuilderSimd>,
}

impl JointConstraintsSet {
    pub fn new() -> Self {
        Self {
            generic_jacobians: DVector::zeros(0),
            two_body_interactions: vec![],
            generic_two_body_interactions: vec![],
            interaction_groups: InteractionGroups::new(),
            velocity_constraints: vec![],
            generic_velocity_constraints: vec![],
            #[cfg(feature = "simd-is-enabled")]
            simd_velocity_constraints: vec![],
            velocity_constraints_builder: vec![],
            generic_velocity_constraints_builder: vec![],
            #[cfg(feature = "simd-is-enabled")]
            simd_velocity_constraints_builder: vec![],
        }
    }

    pub fn clear_constraints(&mut self) {
        self.generic_jacobians.fill(0.0);
        self.generic_velocity_constraints.clear();
        #[cfg(feature = "simd-is-enabled")]
        self.simd_velocity_constraints.clear();
    }

    pub fn clear_builders(&mut self) {
        self.generic_velocity_constraints_builder.clear();
        #[cfg(feature = "simd-is-enabled")]
        self.simd_velocity_constraints_builder.clear();
    }

    // Returns the generic jacobians and a mutable iterator through all the constraints.
    pub fn iter_constraints_mut(
        &mut self,
    ) -> (&DVector, impl Iterator<Item = AnyJointConstraintMut<'_>>) {
        let jac = &self.generic_jacobians;
        let a = self
            .generic_velocity_constraints
            .iter_mut()
            .map(AnyJointConstraintMut::Generic);
        let b = self
            .velocity_constraints
            .iter_mut()
            .map(AnyJointConstraintMut::Rigid);
        #[cfg(feature = "simd-is-enabled")]
        let c = self
            .simd_velocity_constraints
            .iter_mut()
            .map(AnyJointConstraintMut::SimdRigid);
        #[cfg(not(feature = "simd-is-enabled"))]
        return (jac, a.chain(b));
        #[cfg(feature = "simd-is-enabled")]
        return (jac, a.chain(b).chain(c));
    }
}

impl JointConstraintsSet {
    pub fn init(
        &mut self,
        island_id: usize,
        islands: &IslandManager,
        bodies: &RigidBodySet,
        multibody_joints: &MultibodyJointSet,
        impulse_joints: &[JointGraphEdge],
        joint_constraint_indices: &[JointIndex],
    ) {
        // Generate constraints for impulse_joints.
        self.two_body_interactions.clear();
        self.generic_two_body_interactions.clear();

        categorize_joints(
            multibody_joints,
            impulse_joints,
            joint_constraint_indices,
            &mut self.two_body_interactions,
            &mut self.generic_two_body_interactions,
        );

        self.clear_constraints();
        self.clear_builders();

        self.interaction_groups.clear_groups();
        self.interaction_groups.group_joints(
            island_id,
            islands,
            bodies,
            impulse_joints,
            &self.two_body_interactions,
        );

        // NOTE: uncomment this do disable SIMD joint resolution.
        // self.interaction_groups
        //     .nongrouped_interactions
        //     .append(&mut self.interaction_groups.simd_interactions);

        let mut j_id = 0;
        self.compute_joint_constraints(bodies, impulse_joints);
        #[cfg(feature = "simd-is-enabled")]
        {
            self.simd_compute_joint_constraints(bodies, impulse_joints);
        }
        self.compute_generic_joint_constraints(
            island_id,
            islands,
            bodies,
            multibody_joints,
            impulse_joints,
            &mut j_id,
        );
    }

    fn compute_joint_constraints(&mut self, bodies: &RigidBodySet, joints_all: &[JointGraphEdge]) {
        let total_num_builders = self.interaction_groups.nongrouped_interactions.len();

        unsafe {
            reset_buffer(&mut self.velocity_constraints_builder, total_num_builders);
        }

        let mut num_constraints = 0;
        for (joint_i, builder) in self
            .interaction_groups
            .nongrouped_interactions
            .iter()
            .zip(self.velocity_constraints_builder.iter_mut())
        {
            let joint = &joints_all[*joint_i].weight;
            JointConstraintBuilder::generate(
                joint,
                bodies,
                *joint_i,
                builder,
                &mut num_constraints,
            );
        }

        unsafe {
            reset_buffer(&mut self.velocity_constraints, num_constraints);
        }
    }

    fn compute_generic_joint_constraints(
        &mut self,
        // TODO: pass around the &Island directly.
        island_id: usize,
        islands: &IslandManager,
        bodies: &RigidBodySet,
        multibodies: &MultibodyJointSet,
        joints_all: &[JointGraphEdge],
        j_id: &mut usize,
    ) {
        // Count the internal and external constraints builder.
        let num_external_constraint_builders = self.generic_two_body_interactions.len();
        let mut num_internal_constraint_builders = 0;
        for handle in islands.island(island_id).bodies() {
            if let Some(link_id) = multibodies.rigid_body_link(*handle) {
                if JointGenericInternalConstraintBuilder::num_constraints(multibodies, link_id) > 0
                {
                    num_internal_constraint_builders += 1;
                }
            }
        }
        let total_num_builders =
            num_external_constraint_builders + num_internal_constraint_builders;

        // Preallocate builders buffer.
        self.generic_velocity_constraints_builder
            .resize(total_num_builders, GenericJointConstraintBuilder::Empty);

        // Generate external constraints builders.
        let mut num_constraints = 0;
        for (joint_i, builder) in self
            .generic_two_body_interactions
            .iter()
            .zip(self.generic_velocity_constraints_builder.iter_mut())
        {
            let joint = &joints_all[*joint_i].weight;
            JointGenericExternalConstraintBuilder::generate(
                *joint_i,
                joint,
                bodies,
                multibodies,
                builder,
                j_id,
                &mut self.generic_jacobians,
                &mut num_constraints,
            );
        }

        // Generate internal constraints builder. They are indexed after the
        let mut curr_builder = self.generic_two_body_interactions.len();
        for handle in islands.island(island_id).bodies() {
            if curr_builder >= self.generic_velocity_constraints_builder.len() {
                break; // No more builder need to be generated.
            }

            if let Some(link_id) = multibodies.rigid_body_link(*handle) {
                let prev_num_constraints = num_constraints;
                JointGenericInternalConstraintBuilder::generate(
                    multibodies,
                    link_id,
                    &mut self.generic_velocity_constraints_builder[curr_builder],
                    j_id,
                    &mut self.generic_jacobians,
                    &mut num_constraints,
                );
                if num_constraints != prev_num_constraints {
                    curr_builder += 1;
                }
            }
        }

        // Resize constraints buffer now that we know the total count.
        self.generic_velocity_constraints
            .resize(num_constraints, GenericJointConstraint::invalid());
    }

    #[cfg(feature = "simd-is-enabled")]
    fn simd_compute_joint_constraints(
        &mut self,
        bodies: &RigidBodySet,
        joints_all: &[JointGraphEdge],
    ) {
        let total_num_builders = self.interaction_groups.simd_interactions.len() / SIMD_WIDTH;

        unsafe {
            reset_buffer(
                &mut self.simd_velocity_constraints_builder,
                total_num_builders,
            );
        }

        let mut num_constraints = 0;
        for (joints_i, builder) in self
            .interaction_groups
            .simd_interactions
            .chunks_exact(SIMD_WIDTH)
            .zip(self.simd_velocity_constraints_builder.iter_mut())
        {
            let joints_id = array![|ii| joints_i[ii]];
            let impulse_joints = array![|ii| &joints_all[joints_i[ii]].weight];
            JointConstraintBuilderSimd::generate(
                impulse_joints,
                bodies,
                joints_id,
                builder,
                &mut num_constraints,
            );
        }

        unsafe {
            reset_buffer(&mut self.simd_velocity_constraints, num_constraints);
        }
    }
    pub fn solve(&mut self, solver_vels: &mut SolverBodies, generic_solver_vels: &mut DVector) {
        let (jac, constraints) = self.iter_constraints_mut();
        for mut c in constraints {
            c.solve(jac, solver_vels, generic_solver_vels);
        }
    }

    pub fn solve_wo_bias(
        &mut self,
        solver_vels: &mut SolverBodies,
        generic_solver_vels: &mut DVector,
    ) {
        let (jac, constraints) = self.iter_constraints_mut();
        for mut c in constraints {
            c.remove_bias();
            c.solve(jac, solver_vels, generic_solver_vels);
        }
    }

    pub fn writeback_impulses(&mut self, joints_all: &mut [JointGraphEdge]) {
        let (_, constraints) = self.iter_constraints_mut();
        for mut c in constraints {
            c.writeback_impulses(joints_all);
        }
    }
    pub fn update(
        &mut self,
        params: &IntegrationParameters,
        multibodies: &MultibodyJointSet,
        solver_bodies: &SolverBodies,
    ) {
        for builder in &mut self.generic_velocity_constraints_builder {
            match builder {
                GenericJointConstraintBuilder::External(builder) => {
                    builder.update(
                        params,
                        multibodies,
                        solver_bodies,
                        &mut self.generic_jacobians,
                        &mut self.generic_velocity_constraints,
                    );
                }
                GenericJointConstraintBuilder::Internal(builder) => {
                    builder.update(
                        params,
                        multibodies,
                        &mut self.generic_jacobians,
                        &mut self.generic_velocity_constraints,
                    );
                }
                GenericJointConstraintBuilder::Empty => {}
            }
        }

        for builder in &mut self.velocity_constraints_builder {
            builder.update(params, solver_bodies, &mut self.velocity_constraints);
        }

        #[cfg(feature = "simd-is-enabled")]
        for builder in &mut self.simd_velocity_constraints_builder {
            builder.update(params, solver_bodies, &mut self.simd_velocity_constraints);
        }
    }
}

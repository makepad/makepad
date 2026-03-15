use crate::dynamics::solver::categorization::categorize_contacts;
use crate::dynamics::solver::contact_constraint::{
    ContactWithCoulombFriction, ContactWithCoulombFrictionBuilder, GenericContactConstraint,
    GenericContactConstraintBuilder,
};
use crate::dynamics::solver::interaction_groups::InteractionGroups;
use crate::dynamics::solver::reset_buffer;
use crate::dynamics::solver::solver_body::SolverBodies;
use crate::dynamics::{
    ImpulseJoint, IntegrationParameters, IslandManager, JointAxesMask, MultibodyJointSet,
    RigidBodySet,
};
use crate::geometry::{ContactManifold, ContactManifoldIndex};
use crate::math::{DVector, Real, SIMD_WIDTH};
use parry::math::SimdReal;

use crate::dynamics::solver::contact_constraint::any_contact_constraint::AnyContactConstraintMut;
#[cfg(feature = "dim3")]
use crate::dynamics::{
    FrictionModel,
    solver::contact_constraint::{ContactWithTwistFriction, ContactWithTwistFrictionBuilder},
};

#[derive(Debug)]
pub struct ConstraintsCounts {
    pub num_constraints: usize,
    #[allow(dead_code)] // Keep this around for now. Might be useful once we rework parallelism.
    pub num_jacobian_lines: usize,
}

impl ConstraintsCounts {
    // NOTE: constraints count from contacts is always 1 since the max number of solver contacts
    //       matches the max number of contact per constraint.

    pub fn from_joint(joint: &ImpulseJoint) -> Self {
        let joint = &joint.data;
        let locked_axes = joint.locked_axes.bits();
        let motor_axes = joint.motor_axes.bits() & !locked_axes;
        let limit_axes = joint.limit_axes.bits() & !locked_axes;
        let coupled_axes = joint.coupled_axes.bits();

        let num_constraints = (motor_axes & !coupled_axes).count_ones() as usize
            + ((motor_axes & coupled_axes) & JointAxesMask::ANG_AXES.bits() != 0) as usize
            + ((motor_axes & coupled_axes) & JointAxesMask::LIN_AXES.bits() != 0) as usize
            + locked_axes.count_ones() as usize
            + (limit_axes & !coupled_axes).count_ones() as usize
            + ((limit_axes & coupled_axes) & JointAxesMask::ANG_AXES.bits() != 0) as usize
            + ((limit_axes & coupled_axes) & JointAxesMask::LIN_AXES.bits() != 0) as usize;
        Self {
            num_constraints,
            num_jacobian_lines: num_constraints,
        }
    }
}

pub(crate) struct ContactConstraintsSet {
    pub generic_jacobians: DVector,
    pub two_body_interactions: Vec<ContactManifoldIndex>,
    pub generic_two_body_interactions: Vec<ContactManifoldIndex>,
    pub interaction_groups: InteractionGroups,

    pub generic_velocity_constraints: Vec<GenericContactConstraint>,
    pub simd_velocity_coulomb_constraints: Vec<ContactWithCoulombFriction<SimdReal>>,
    #[cfg(feature = "dim3")]
    pub simd_velocity_twist_constraints: Vec<ContactWithTwistFriction<SimdReal>>,

    pub generic_velocity_constraints_builder: Vec<GenericContactConstraintBuilder>,
    pub simd_velocity_coulomb_constraints_builder: Vec<ContactWithCoulombFrictionBuilder>,
    #[cfg(feature = "dim3")]
    pub simd_velocity_twist_constraints_builder: Vec<ContactWithTwistFrictionBuilder<SimdReal>>,
}

impl ContactConstraintsSet {
    pub fn new() -> Self {
        Self {
            generic_jacobians: DVector::zeros(0),
            two_body_interactions: vec![],
            generic_two_body_interactions: vec![],
            interaction_groups: InteractionGroups::new(),
            generic_velocity_constraints: vec![],
            simd_velocity_coulomb_constraints: vec![],
            generic_velocity_constraints_builder: vec![],
            simd_velocity_coulomb_constraints_builder: vec![],
            #[cfg(feature = "dim3")]
            simd_velocity_twist_constraints: vec![],
            #[cfg(feature = "dim3")]
            simd_velocity_twist_constraints_builder: vec![],
        }
    }

    pub fn clear_constraints(&mut self) {
        self.generic_jacobians.fill(0.0);
        self.generic_velocity_constraints.clear();
        self.simd_velocity_coulomb_constraints.clear();
        #[cfg(feature = "dim3")]
        self.simd_velocity_twist_constraints.clear();
    }

    pub fn clear_builders(&mut self) {
        self.generic_velocity_constraints_builder.clear();
        self.simd_velocity_coulomb_constraints_builder.clear();
        #[cfg(feature = "dim3")]
        self.simd_velocity_twist_constraints_builder.clear();
    }

    // Returns the generic jacobians and a mutable iterator through all the constraints.
    pub fn iter_constraints_mut(
        &mut self,
    ) -> (&DVector, impl Iterator<Item = AnyContactConstraintMut<'_>>) {
        let jac = &self.generic_jacobians;
        let a = self
            .generic_velocity_constraints
            .iter_mut()
            .map(AnyContactConstraintMut::Generic);
        let b = self
            .simd_velocity_coulomb_constraints
            .iter_mut()
            .map(AnyContactConstraintMut::WithCoulombFriction);
        #[cfg(feature = "dim3")]
        {
            let c = self
                .simd_velocity_twist_constraints
                .iter_mut()
                .map(AnyContactConstraintMut::WithTwistFriction);
            (jac, a.chain(b).chain(c))
        }

        #[cfg(feature = "dim2")]
        return (jac, a.chain(b));
    }
}

impl ContactConstraintsSet {
    pub fn init_constraint_groups(
        &mut self,
        island_id: usize,
        islands: &IslandManager,
        bodies: &RigidBodySet,
        multibody_joints: &MultibodyJointSet,
        manifolds: &[&mut ContactManifold],
        manifold_indices: &[ContactManifoldIndex],
    ) {
        self.two_body_interactions.clear();
        self.generic_two_body_interactions.clear();

        categorize_contacts(
            bodies,
            multibody_joints,
            manifolds,
            manifold_indices,
            &mut self.two_body_interactions,
            &mut self.generic_two_body_interactions,
        );

        self.interaction_groups.clear_groups();
        self.interaction_groups.group_manifolds(
            island_id,
            islands,
            bodies,
            manifolds,
            &self.two_body_interactions,
        );

        // NOTE: uncomment this do disable SIMD contact resolution.
        // self.interaction_groups
        //     .nongrouped_interactions
        //     .append(&mut self.interaction_groups.simd_interactions);
        // self.one_body_interaction_groups
        //     .nongrouped_interactions
        //     .append(&mut self.one_body_interaction_groups.simd_interactions);
    }

    pub fn init(
        &mut self,
        island_id: usize,
        islands: &IslandManager,
        bodies: &RigidBodySet,
        solver_bodies: &SolverBodies,
        multibody_joints: &MultibodyJointSet,
        manifolds: &[&mut ContactManifold],
        manifold_indices: &[ContactManifoldIndex],
        #[cfg(feature = "dim3")] friction_model: FrictionModel,
    ) {
        // let t0 = std::time::Instant::now();
        self.clear_constraints();
        self.clear_builders();

        self.init_constraint_groups(
            island_id,
            islands,
            bodies,
            multibody_joints,
            manifolds,
            manifold_indices,
        );

        // let t_init_groups = t0.elapsed().as_secs_f32();
        // let t0 = std::time::Instant::now();
        let mut jacobian_id = 0;

        self.compute_generic_constraints(bodies, multibody_joints, manifolds, &mut jacobian_id);
        // let t_init_constraints = t0.elapsed().as_secs_f32();

        // let t0 = std::time::Instant::now();
        // #[cfg(feature = "simd-is-enabled")]
        // {
        //     self.simd_compute_constraints_bench(bodies, solver_bodies, manifolds);
        // }
        // let t_init_constraint_bench = t0.elapsed().as_secs_f32();

        // let t0 = std::time::Instant::now();
        #[cfg(feature = "dim2")]
        self.simd_compute_coulomb_constraints(bodies, solver_bodies, manifolds);

        #[cfg(feature = "dim3")]
        match friction_model {
            FrictionModel::Simplified => {
                self.simd_compute_twist_constraints(bodies, solver_bodies, manifolds)
            }
            FrictionModel::Coulomb => {
                self.simd_compute_coulomb_constraints(bodies, solver_bodies, manifolds)
            }
        }

        // let t_init_constraints_simd = t0.elapsed().as_secs_f32();
        // let num_simd_constraints = self.simd_velocity_constraints.len();
        // println!(
        //     "t_init_group: {:?}, t_init_constraints_simd: {}: {:?}, t_debug: {:?}",
        //     t_init_groups * 1000.0,
        //     num_simd_constraints,
        //     t_init_constraints_simd * 1000.0,
        //     t_init_constraint_bench * 1000.0,
        // );
        // println!(
        //     "Solver constraints init: {}",
        //     t0.elapsed().as_secs_f32() * 1000.0
        // );
    }

    // #[cfg(feature = "simd-is-enabled")]
    // fn simd_compute_constraints_bench(
    //     &mut self,
    //     bodies: &RigidBodySet,
    //     solver_bodies: &SolverBodies,
    //     manifolds_all: &[&mut ContactManifold],
    // ) {
    //     let total_num_constraints = self
    //         .interaction_groups
    //         .simd_interactions
    //         .chunks_exact(SIMD_WIDTH)
    //         .map(|i| ConstraintsCounts::from_contacts(manifolds_all[i[0] as usize]).num_constraints)
    //         .sum::<usize>();
    //
    //     unsafe {
    //         reset_buffer(
    //             &mut self.simd_velocity_constraints_builder,
    //             total_num_constraints as usize,
    //         );
    //         reset_buffer(
    //             &mut self.simd_velocity_constraints,
    //             total_num_constraints as usize,
    //         );
    //     }
    //
    //     let mut curr_start = 0;
    //
    //     let t0 = std::time::Instant::now();
    //     let preload = TwoBodyConstraintBuilderSimd::collect_constraint_gen_data(
    //         bodies,
    //         &*manifolds_all,
    //         &self.interaction_groups.simd_interactions,
    //     );
    //     println!("Preload: {:?}", t0.elapsed().as_secs_f32() * 1000.0);
    //
    //     let t0 = std::time::Instant::now();
    //     for i in (0..self.interaction_groups.simd_interactions.len()).step_by(SIMD_WIDTH) {
    //         let num_to_add = 1; // preload.solver_contact_headers[i].num_contacts;
    //         TwoBodyConstraintBuilderSimd::generate_bench_preloaded(
    //             &preload,
    //             i,
    //             solver_bodies,
    //             &mut self.simd_velocity_constraints_builder[curr_start..],
    //             &mut self.simd_velocity_constraints[curr_start..],
    //         );
    //
    //         curr_start += num_to_add;
    //     }
    //     println!("Preloaded init: {:?}", t0.elapsed().as_secs_f32() * 1000.0);
    //
    //     /*
    //     for manifolds_i in self
    //         .interaction_groups
    //         .simd_interactions
    //         .chunks_exact(SIMD_WIDTH)
    //     {
    //         let num_to_add =
    //             ConstraintsCounts::from_contacts(manifolds_all[manifolds_i[0]]).num_constraints;
    //         let manifold_id = array![|ii| manifolds_i[ii]];
    //         let manifolds = array![|ii| &*manifolds_all[manifolds_i[ii]]];
    //
    //         TwoBodyConstraintBuilderSimd::generate_bench(
    //             manifold_id,
    //             manifolds,
    //             bodies,
    //             solver_bodies,
    //             &mut self.simd_velocity_constraints_builder[curr_start..],
    //             &mut self.simd_velocity_constraints[curr_start..],
    //         );
    //
    //         curr_start += num_to_add;
    //     }
    //      */
    //
    //     // assert_eq!(curr_start, total_num_constraints);
    // }

    // TODO: could we somehow combine that with the simd_compute_coulomb_constraints function since
    //       both are very similar and mutually exclusive?
    #[cfg(feature = "dim3")]
    fn simd_compute_twist_constraints(
        &mut self,
        bodies: &RigidBodySet,
        solver_bodies: &SolverBodies,
        manifolds_all: &[&mut ContactManifold],
    ) {
        let total_num_constraints = (self.interaction_groups.simd_interactions.len() / SIMD_WIDTH)
            + self.interaction_groups.nongrouped_interactions.len();

        unsafe {
            reset_buffer(
                &mut self.simd_velocity_twist_constraints_builder,
                total_num_constraints,
            );
            reset_buffer(
                &mut self.simd_velocity_twist_constraints,
                total_num_constraints,
            );
        }

        // TODO PERF: could avoid this index using zip.
        let mut curr_id = 0;

        for manifolds_i in self
            .interaction_groups
            .simd_interactions
            .chunks_exact(SIMD_WIDTH)
        {
            let manifold_id = array![|ii| manifolds_i[ii]];
            let manifolds = array![|ii| &*manifolds_all[manifolds_i[ii]]];

            ContactWithTwistFrictionBuilder::generate(
                manifold_id,
                manifolds,
                bodies,
                solver_bodies,
                &mut self.simd_velocity_twist_constraints_builder[curr_id],
                &mut self.simd_velocity_twist_constraints[curr_id],
            );

            curr_id += 1;
        }

        for manifolds_i in self.interaction_groups.nongrouped_interactions.iter() {
            let mut manifold_id = [usize::MAX; SIMD_WIDTH];
            manifold_id[0] = *manifolds_i;
            let manifolds = [&*manifolds_all[*manifolds_i]; SIMD_WIDTH];

            ContactWithTwistFrictionBuilder::generate(
                manifold_id,
                manifolds,
                bodies,
                solver_bodies,
                &mut self.simd_velocity_twist_constraints_builder[curr_id],
                &mut self.simd_velocity_twist_constraints[curr_id],
            );

            curr_id += 1;
        }

        assert_eq!(curr_id, total_num_constraints);
    }

    fn simd_compute_coulomb_constraints(
        &mut self,
        bodies: &RigidBodySet,
        solver_bodies: &SolverBodies,
        manifolds_all: &[&mut ContactManifold],
    ) {
        let total_num_constraints = self.interaction_groups.simd_interactions.len() / SIMD_WIDTH
            + self.interaction_groups.nongrouped_interactions.len();

        unsafe {
            reset_buffer(
                &mut self.simd_velocity_coulomb_constraints_builder,
                total_num_constraints,
            );
            reset_buffer(
                &mut self.simd_velocity_coulomb_constraints,
                total_num_constraints,
            );
        }

        // TODO PERF: could avoid this index using zip.
        let mut curr_id = 0;

        for manifolds_i in self
            .interaction_groups
            .simd_interactions
            .chunks_exact(SIMD_WIDTH)
        {
            let manifold_id = array![|ii| manifolds_i[ii]];
            let manifolds = array![|ii| &*manifolds_all[manifolds_i[ii]]];

            ContactWithCoulombFrictionBuilder::generate(
                manifold_id,
                manifolds,
                bodies,
                solver_bodies,
                &mut self.simd_velocity_coulomb_constraints_builder[curr_id],
                &mut self.simd_velocity_coulomb_constraints[curr_id],
            );

            curr_id += 1;
        }

        for manifolds_i in self.interaction_groups.nongrouped_interactions.iter() {
            let mut manifold_id = [usize::MAX; SIMD_WIDTH];
            manifold_id[0] = *manifolds_i;
            let manifolds = [&*manifolds_all[*manifolds_i]; SIMD_WIDTH];

            ContactWithCoulombFrictionBuilder::generate(
                manifold_id,
                manifolds,
                bodies,
                solver_bodies,
                &mut self.simd_velocity_coulomb_constraints_builder[curr_id],
                &mut self.simd_velocity_coulomb_constraints[curr_id],
            );

            curr_id += 1;
        }

        assert_eq!(curr_id, total_num_constraints);
    }

    fn compute_generic_constraints(
        &mut self,
        bodies: &RigidBodySet,
        multibody_joints: &MultibodyJointSet,
        manifolds_all: &[&mut ContactManifold],
        jacobian_id: &mut usize,
    ) {
        let total_num_constraints = self.generic_two_body_interactions.len();

        self.generic_velocity_constraints_builder.resize(
            total_num_constraints,
            GenericContactConstraintBuilder::invalid(),
        );
        self.generic_velocity_constraints
            .resize(total_num_constraints, GenericContactConstraint::invalid());

        // TODO PERF: could avoid this index using zip.
        let mut curr_id = 0;

        for manifold_i in &self.generic_two_body_interactions {
            let manifold = &manifolds_all[*manifold_i];

            GenericContactConstraintBuilder::generate(
                *manifold_i,
                manifold,
                bodies,
                multibody_joints,
                &mut self.generic_velocity_constraints_builder[curr_id],
                &mut self.generic_velocity_constraints[curr_id],
                &mut self.generic_jacobians,
                jacobian_id,
            );

            curr_id += 1;
        }

        assert_eq!(curr_id, total_num_constraints);
    }

    pub fn warmstart(
        &mut self,
        solver_bodies: &mut SolverBodies,
        generic_solver_vels: &mut DVector,
    ) {
        let (jac, constraints) = self.iter_constraints_mut();
        for mut c in constraints {
            c.warmstart(jac, solver_bodies, generic_solver_vels);
        }
    }
    pub fn solve(&mut self, solver_bodies: &mut SolverBodies, generic_solver_vels: &mut DVector) {
        let (jac, constraints) = self.iter_constraints_mut();
        for mut c in constraints {
            c.solve(jac, solver_bodies, generic_solver_vels);
        }
    }
    pub fn solve_wo_bias(
        &mut self,
        solver_bodies: &mut SolverBodies,
        generic_solver_vels: &mut DVector,
    ) {
        let (jac, constraints) = self.iter_constraints_mut();
        for mut c in constraints {
            c.remove_bias();
            c.solve(jac, solver_bodies, generic_solver_vels);
        }
    }

    pub fn writeback_impulses(&mut self, manifolds_all: &mut [&mut ContactManifold]) {
        let (_, constraints) = self.iter_constraints_mut();
        for mut c in constraints {
            c.writeback_impulses(manifolds_all);
        }
    }
    pub fn update(
        &mut self,
        params: &IntegrationParameters,
        small_step_id: usize,
        multibodies: &MultibodyJointSet,
        solver_bodies: &SolverBodies,
    ) {
        macro_rules! update_contacts(
            ($builders: ident, $constraints: ident) => {
                for (builder, constraint) in self.$builders.iter().zip(self.$constraints.iter_mut()) {
                    builder.update(
                        &params,
                        small_step_id as Real * params.dt,
                        solver_bodies,
                        multibodies,
                        constraint,
                    );
                }
            }
        );

        update_contacts!(
            generic_velocity_constraints_builder,
            generic_velocity_constraints
        );
        update_contacts!(
            simd_velocity_coulomb_constraints_builder,
            simd_velocity_coulomb_constraints
        );
        #[cfg(feature = "dim3")]
        update_contacts!(
            simd_velocity_twist_constraints_builder,
            simd_velocity_twist_constraints
        );
    }
}

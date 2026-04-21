use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::Scope;

use crate::dynamics::solver::{
    ContactConstraintTypes, JointConstraintTypes, ParallelSolverConstraints,
};
use crate::dynamics::{
    IntegrationParameters, IslandManager, JointGraphEdge, JointIndex, MultibodyJointSet,
    RigidBodySet,
};
use crate::geometry::{ContactManifold, ContactManifoldIndex};
use crate::math::DVector;

use super::{ParallelInteractionGroups, ParallelVelocitySolver, SolverVel};

#[macro_export]
#[doc(hidden)]
macro_rules! concurrent_loop {
    (let batch_size = $batch_size: expr;
     for $elt: ident in $array: ident[$index_stream:expr,$index_count:expr] $f: expr) => {
        let max_index = $array.len();

        if max_index > 0 {
            loop {
                let start_index = $index_stream.fetch_add($batch_size, Ordering::SeqCst);
                if start_index > max_index {
                    break;
                }

                let end_index = (start_index + $batch_size).min(max_index);
                for $elt in &$array[start_index..end_index] {
                    $f
                }

                $index_count.fetch_add(end_index - start_index, Ordering::SeqCst);
            }
        }
    };

    (let batch_size = $batch_size: expr;
     for $elt: ident in $array: ident[$index_stream:expr] $f: expr) => {
        let max_index = $array.len();

        if max_index > 0 {
            loop {
                let start_index = $index_stream.fetch_add($batch_size, Ordering::SeqCst);
                if start_index > max_index {
                    break;
                }

                let end_index = (start_index + $batch_size).min(max_index);
                for $elt in &$array[start_index..end_index] {
                    $f
                }
            }
        }
    };

    (let batch_size = $batch_size: expr;
        for $elt: ident in &mut $array: ident[$index_stream:expr] $f: expr) => {
        let max_index = $array.len();

        if max_index > 0 {
            loop {
                let start_index = $index_stream.fetch_add($batch_size, Ordering::SeqCst);
                if start_index > max_index {
                    break;
                }

                let end_index = (start_index + $batch_size).min(max_index);
                for $elt in &mut $array[start_index..end_index] {
                    $f
                }
            }
        }
    };
}

pub(crate) struct ThreadContext {
    pub batch_size: usize,
    // Velocity solver.
    pub constraint_initialization_index: AtomicUsize,
    pub num_initialized_constraints: AtomicUsize,
    pub joint_constraint_initialization_index: AtomicUsize,
    pub num_initialized_joint_constraints: AtomicUsize,
    pub solve_interaction_index: AtomicUsize,
    pub num_solved_interactions: AtomicUsize,
    pub impulse_writeback_index: AtomicUsize,
    pub joint_writeback_index: AtomicUsize,
    pub impulse_rm_bias_index: AtomicUsize,
    pub joint_rm_bias_index: AtomicUsize,
    pub body_integration_pos_index: AtomicUsize,
    pub body_integration_vel_index: AtomicUsize,
    pub body_force_integration_index: AtomicUsize,
    pub num_force_integrated_bodies: AtomicUsize,
    pub num_integrated_pos_bodies: AtomicUsize,
    pub num_integrated_vel_bodies: AtomicUsize,
}

impl ThreadContext {
    pub fn new(batch_size: usize) -> Self {
        ThreadContext {
            batch_size, // TODO perhaps there is some optimal value we can compute depending on the island size?
            constraint_initialization_index: AtomicUsize::new(0),
            num_initialized_constraints: AtomicUsize::new(0),
            joint_constraint_initialization_index: AtomicUsize::new(0),
            num_initialized_joint_constraints: AtomicUsize::new(0),
            solve_interaction_index: AtomicUsize::new(0),
            num_solved_interactions: AtomicUsize::new(0),
            impulse_writeback_index: AtomicUsize::new(0),
            joint_writeback_index: AtomicUsize::new(0),
            impulse_rm_bias_index: AtomicUsize::new(0),
            joint_rm_bias_index: AtomicUsize::new(0),
            body_force_integration_index: AtomicUsize::new(0),
            num_force_integrated_bodies: AtomicUsize::new(0),
            body_integration_pos_index: AtomicUsize::new(0),
            body_integration_vel_index: AtomicUsize::new(0),
            num_integrated_pos_bodies: AtomicUsize::new(0),
            num_integrated_vel_bodies: AtomicUsize::new(0),
        }
    }

    pub fn lock_until_ge(val: &AtomicUsize, target: usize) {
        if target > 0 {
            //        let backoff = crossbeam::utils::Backoff::new();
            std::sync::atomic::fence(Ordering::SeqCst);
            while val.load(Ordering::Relaxed) < target {
                //  backoff.spin();
                // std::thread::yield_now();
            }
        }
    }
}

pub struct ParallelIslandSolver {
    velocity_solver: ParallelVelocitySolver,
    parallel_groups: ParallelInteractionGroups,
    parallel_joint_groups: ParallelInteractionGroups,
    parallel_contact_constraints: ParallelSolverConstraints<ContactConstraintTypes>,
    parallel_joint_constraints: ParallelSolverConstraints<JointConstraintTypes>,
    thread: ThreadContext,
}

impl Default for ParallelIslandSolver {
    fn default() -> Self {
        Self::new()
    }
}

impl ParallelIslandSolver {
    pub fn new() -> Self {
        Self {
            velocity_solver: ParallelVelocitySolver::new(),
            parallel_groups: ParallelInteractionGroups::new(),
            parallel_joint_groups: ParallelInteractionGroups::new(),
            parallel_contact_constraints: ParallelSolverConstraints::new(),
            parallel_joint_constraints: ParallelSolverConstraints::new(),
            thread: ThreadContext::new(8),
        }
    }
    pub fn init_and_solve<'s>(
        &'s mut self,
        scope: &Scope<'s>,
        island_id: usize,
        islands: &'s IslandManager,
        params: &'s IntegrationParameters,
        bodies: &'s mut RigidBodySet,
        manifolds: &'s mut Vec<&'s mut ContactManifold>,
        manifold_indices: &'s [ContactManifoldIndex],
        impulse_joints: &'s mut Vec<JointGraphEdge>,
        joint_indices: &[JointIndex],
        multibodies: &mut MultibodyJointSet,
    ) {
        let num_threads = rayon::current_num_threads();
        let num_task_per_island = num_threads; // (num_threads / num_islands).max(1); // TODO: not sure this is the best value. Also, perhaps it is better to interleave tasks of each island?
        self.thread = ThreadContext::new(8); // TODO: could we compute some kind of optimal value here?

        // Interactions grouping.
        self.parallel_groups.group_interactions(
            island_id,
            islands,
            bodies,
            multibodies,
            manifolds,
            manifold_indices,
        );
        self.parallel_joint_groups.group_interactions(
            island_id,
            islands,
            bodies,
            multibodies,
            impulse_joints,
            joint_indices,
        );

        let mut contact_j_id = 0;
        self.parallel_contact_constraints.init_constraint_groups(
            island_id,
            islands,
            bodies,
            multibodies,
            manifolds,
            &self.parallel_groups,
            &mut contact_j_id,
        );
        let mut joint_j_id = 0;
        self.parallel_joint_constraints.init_constraint_groups(
            island_id,
            islands,
            bodies,
            multibodies,
            impulse_joints,
            &self.parallel_joint_groups,
            &mut joint_j_id,
        );

        if self.parallel_contact_constraints.generic_jacobians.len() < contact_j_id {
            self.parallel_contact_constraints.generic_jacobians = DVector::zeros(contact_j_id);
        } else {
            self.parallel_contact_constraints
                .generic_jacobians
                .fill(0.0);
        }

        if self.parallel_joint_constraints.generic_jacobians.len() < joint_j_id {
            self.parallel_joint_constraints.generic_jacobians = DVector::zeros(joint_j_id);
        } else {
            self.parallel_joint_constraints.generic_jacobians.fill(0.0);
        }

        // Init solver ids for multibodies.
        {
            let mut solver_id = 0;
            let island_range = islands.active_island_range(island_id);
            let active_bodies = &islands.active_set[island_range];
            for handle in active_bodies {
                if let Some(link) = multibodies.rigid_body_link(*handle).copied() {
                    let multibody = multibodies
                        .get_multibody_mut_internal(link.multibody)
                        .unwrap();
                    if link.id == 0 || link.id == 1 && !multibody.root_is_dynamic {
                        multibody.solver_id = solver_id;
                        solver_id += multibody.ndofs();
                    }
                }
            }

            if self.velocity_solver.generic_solver_vels.len() < solver_id {
                self.velocity_solver.generic_solver_vels = DVector::zeros(solver_id);
            } else {
                self.velocity_solver.generic_solver_vels.fill(0.0);
            }

            self.velocity_solver.solver_vels.clear();
            self.velocity_solver
                .solver_vels
                .resize(islands.active_island(island_id).len(), SolverVel::zero());
        }

        for _ in 0..num_task_per_island {
            // We use AtomicPtr because it is Send+Sync while *mut is not.
            // See https://internals.rust-lang.org/t/shouldnt-pointers-be-send-sync-or/8818
            let thread = &self.thread;
            let velocity_solver =
                std::sync::atomic::AtomicPtr::new(&mut self.velocity_solver as *mut _);
            let bodies = std::sync::atomic::AtomicPtr::new(bodies as *mut _);
            let multibodies = std::sync::atomic::AtomicPtr::new(multibodies as *mut _);
            let manifolds = std::sync::atomic::AtomicPtr::new(manifolds as *mut _);
            let impulse_joints = std::sync::atomic::AtomicPtr::new(impulse_joints as *mut _);
            let parallel_contact_constraints =
                std::sync::atomic::AtomicPtr::new(&mut self.parallel_contact_constraints as *mut _);
            let parallel_joint_constraints =
                std::sync::atomic::AtomicPtr::new(&mut self.parallel_joint_constraints as *mut _);

            scope.spawn(move |_| {
                // Transmute *mut -> &mut
                let velocity_solver: &mut ParallelVelocitySolver =
                    unsafe { std::mem::transmute(velocity_solver.load(Ordering::Relaxed)) };
                let bodies: &mut RigidBodySet =
                    unsafe { std::mem::transmute(bodies.load(Ordering::Relaxed)) };
                let multibodies: &mut MultibodyJointSet =
                    unsafe { std::mem::transmute(multibodies.load(Ordering::Relaxed)) };
                let manifolds: &mut Vec<&mut ContactManifold> =
                    unsafe { std::mem::transmute(manifolds.load(Ordering::Relaxed)) };
                let impulse_joints: &mut Vec<JointGraphEdge> =
                    unsafe { std::mem::transmute(impulse_joints.load(Ordering::Relaxed)) };
                let parallel_contact_constraints: &mut ParallelSolverConstraints<ContactConstraintTypes> = unsafe {
                    std::mem::transmute(parallel_contact_constraints.load(Ordering::Relaxed))
                };
                let parallel_joint_constraints: &mut ParallelSolverConstraints<JointConstraintTypes> = unsafe {
                    std::mem::transmute(parallel_joint_constraints.load(Ordering::Relaxed))
                };

                enable_flush_to_zero!(); // Ensure this is enabled on each thread.

                // Initialize `solver_vels` (per-body velocity deltas) with external accelerations (gravity etc):
                {
                    let island_range = islands.active_island_range(island_id);
                    let active_bodies = &islands.active_set[island_range];

                    concurrent_loop! {
                        let batch_size = thread.batch_size;
                        for handle in active_bodies[thread.body_force_integration_index, thread.num_force_integrated_bodies] {
                            if let Some(link) = multibodies.rigid_body_link(*handle).copied() {
                                let multibody = multibodies
                                    .get_multibody_mut_internal(link.multibody)
                                    .unwrap();

                                if link.id == 0 || link.id == 1 && !multibody.root_is_dynamic {
                                    let mut solver_vels = velocity_solver
                                        .generic_solver_vels
                                        .rows_mut(multibody.solver_id, multibody.ndofs());
                                    solver_vels.axpy(params.dt, &multibody.accelerations, 0.0);
                                }
                            } else {
                                let rb = &bodies[*handle];
                                let dvel = &mut velocity_solver.solver_vels[rb.ids.active_set_offset];

                                // NOTE: `dvel.angular` is actually storing angular velocity delta multiplied
                                //       by the square root of the inertia tensor:
                                dvel.angular += rb.mprops.effective_world_inv_inertia * rb.forces.torque * params.dt;
                                dvel.linear += rb.forces.force * rb.mprops.effective_inv_mass * params.dt;
                            }
                        }
                    }

                    // We need to wait for every body to be force-integrated because their
                    // angular and linear velocities are needed by the constraints initialization.
                    ThreadContext::lock_until_ge(&thread.num_force_integrated_bodies, active_bodies.len());
                }


                parallel_contact_constraints.fill_constraints(&thread, params, bodies, multibodies, manifolds);
                parallel_joint_constraints.fill_constraints(&thread, params, bodies, multibodies, impulse_joints);
                ThreadContext::lock_until_ge(
                    &thread.num_initialized_constraints,
                    parallel_contact_constraints.constraint_descs.len(),
                );
                ThreadContext::lock_until_ge(
                    &thread.num_initialized_joint_constraints,
                    parallel_joint_constraints.constraint_descs.len(),
                );

                velocity_solver.solve(
                        &thread,
                        params,
                        island_id,
                        islands,
                        bodies,
                        multibodies,
                        manifolds,
                        impulse_joints,
                        parallel_contact_constraints,
                        parallel_joint_constraints,
                );
            })
        }
    }
}

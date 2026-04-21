//! Physics pipeline structures.

use crate::counters::Counters;
// #[cfg(not(feature = "parallel"))]
use crate::dynamics::IslandSolver;
#[cfg(feature = "parallel")]
use crate::dynamics::JointGraphEdge;
use crate::dynamics::{
    CCDSolver, ImpulseJointSet, IntegrationParameters, IslandManager, MultibodyJointSet,
    RigidBodyChanges, RigidBodyType,
};
use crate::geometry::{
    BroadPhaseBvh, BroadPhasePairEvent, ColliderChanges, ColliderHandle, ColliderPair,
    ContactManifoldIndex, ModifiedColliders, NarrowPhase, TemporaryInteractionIndex,
};
use crate::math::{Real, Vector};
use crate::pipeline::{EventHandler, PhysicsHooks};
use crate::prelude::ModifiedRigidBodies;
use {crate::dynamics::RigidBodySet, crate::geometry::ColliderSet};

/// The main physics simulation engine that runs your physics world forward in time.
///
/// Think of this as the "game loop" for your physics simulation. Each frame, you call
/// [`PhysicsPipeline::step`] to advance the simulation by one timestep. This structure
/// handles all the complex physics calculations: detecting collisions between objects,
/// resolving contacts so objects don't overlap, and updating positions and velocities.
///
/// ## Performance note
/// This structure only contains temporary working memory (scratch buffers). You can create
/// a new one anytime, but it's more efficient to reuse the same instance across frames
/// since Rapier can reuse allocated memory.
///
/// ## How it works (simplified)
/// Rapier uses a time-stepping approach where each step involves:
/// 1. **Collision detection**: Find which objects are touching or overlapping
/// 2. **Constraint solving**: Calculate forces to prevent overlaps and enforce joint constraints
/// 3. **Integration**: Update object positions and velocities based on forces and gravity
/// 4. **Position correction**: Fix any remaining overlaps that might have occurred
// NOTE: this contains only workspace data, so there is no point in making this serializable.
pub struct PhysicsPipeline {
    /// Counters used for benchmarking only.
    pub counters: Counters,
    contact_pair_indices: Vec<TemporaryInteractionIndex>,
    manifold_indices: Vec<Vec<ContactManifoldIndex>>,
    joint_constraint_indices: Vec<Vec<ContactManifoldIndex>>,
    broadphase_collider_pairs: Vec<ColliderPair>,
    broad_phase_events: Vec<BroadPhasePairEvent>,
    solvers: Vec<IslandSolver>,
}

impl Default for PhysicsPipeline {
    fn default() -> Self {
        PhysicsPipeline::new()
    }
}

#[allow(dead_code)]
fn check_pipeline_send_sync() {
    fn do_test<T: Sync>() {}
    do_test::<PhysicsPipeline>();
}

impl PhysicsPipeline {
    /// Creates a new physics pipeline.
    ///
    /// Call this once when setting up your physics world. The pipeline can be reused
    /// across multiple frames for better performance.
    pub fn new() -> PhysicsPipeline {
        PhysicsPipeline {
            counters: Counters::new(true),
            solvers: vec![],
            contact_pair_indices: vec![],
            manifold_indices: vec![],
            joint_constraint_indices: vec![],
            broadphase_collider_pairs: vec![],
            broad_phase_events: vec![],
        }
    }

    fn clear_modified_colliders(
        &mut self,
        colliders: &mut ColliderSet,
        modified_colliders: &mut ModifiedColliders,
    ) {
        // TODO: we can’t just iterate on `modified_colliders` here to clear the
        //       flags because the last substep will leave some colliders with
        //       changes flags set after solving, but without the collider being
        //       part of the `ModifiedColliders` set. This is a bit error-prone but
        //       is necessary for the modified information to carry on to the
        //       next frame’s narrow-phase for updating.
        for co in colliders.colliders.iter_mut() {
            co.1.changes = ColliderChanges::empty();
        }
        // for handle in modified_colliders.iter() {
        //     if let Some(co) = colliders.get_mut_internal(*handle) {
        //         co.changes = ColliderChanges::empty();
        //     }
        // }

        modified_colliders.clear();
    }

    fn clear_modified_bodies(
        &mut self,
        bodies: &mut RigidBodySet,
        modified_bodies: &mut ModifiedRigidBodies,
    ) {
        for handle in modified_bodies.iter() {
            if let Some(rb) = bodies.get_mut_internal(*handle) {
                rb.changes = RigidBodyChanges::empty();
            }
        }

        modified_bodies.clear();
    }

    fn detect_collisions(
        &mut self,
        integration_parameters: &IntegrationParameters,
        islands: &mut IslandManager,
        broad_phase: &mut BroadPhaseBvh,
        narrow_phase: &mut NarrowPhase,
        bodies: &mut RigidBodySet,
        colliders: &mut ColliderSet,
        impulse_joints: &ImpulseJointSet,
        multibody_joints: &MultibodyJointSet,
        modified_colliders: &[ColliderHandle],
        removed_colliders: &[ColliderHandle],
        hooks: &dyn PhysicsHooks,
        events: &dyn EventHandler,
        handle_user_changes: bool,
    ) {
        self.counters.stages.collision_detection_time.resume();
        self.counters.cd.broad_phase_time.resume();

        // Update broad-phase.
        self.broad_phase_events.clear();
        self.broadphase_collider_pairs.clear();
        broad_phase.update(
            integration_parameters,
            colliders,
            bodies,
            modified_colliders,
            removed_colliders,
            &mut self.broad_phase_events,
        );

        self.counters.cd.broad_phase_time.pause();
        self.counters.cd.narrow_phase_time.resume();

        // Update narrow-phase.
        if handle_user_changes {
            narrow_phase.handle_user_changes(
                Some(islands),
                modified_colliders,
                removed_colliders,
                colliders,
                bodies,
                events,
            );
        }
        narrow_phase.register_pairs(
            Some(islands),
            colliders,
            bodies,
            &self.broad_phase_events,
            events,
        );
        narrow_phase.compute_contacts(
            integration_parameters.prediction_distance(),
            integration_parameters.dt,
            islands,
            bodies,
            colliders,
            impulse_joints,
            multibody_joints,
            hooks,
            events,
        );
        narrow_phase.compute_intersections(bodies, colliders, hooks, events);

        self.counters.cd.narrow_phase_time.pause();
        self.counters.stages.collision_detection_time.pause();
    }

    fn build_islands_and_solve_velocity_constraints(
        &mut self,
        gravity: Vector,
        integration_parameters: &IntegrationParameters,
        islands: &mut IslandManager,
        narrow_phase: &mut NarrowPhase,
        bodies: &mut RigidBodySet,
        colliders: &mut ColliderSet,
        impulse_joints: &mut ImpulseJointSet,
        multibody_joints: &mut MultibodyJointSet,
        events: &dyn EventHandler,
    ) {
        self.counters.stages.island_construction_time.resume();
        // NOTE: islands update must be done after the narrow-phase.
        islands.update_islands(
            integration_parameters.dt,
            integration_parameters.length_unit,
            bodies,
            colliders,
            narrow_phase,
            impulse_joints,
            multibody_joints,
        );

        let num_active_islands = islands.active_islands().len();
        if self.manifold_indices.len() < num_active_islands {
            self.manifold_indices.resize(num_active_islands, Vec::new());
        }

        if self.joint_constraint_indices.len() < num_active_islands {
            self.joint_constraint_indices
                .resize(num_active_islands, Vec::new());
        }
        self.counters.stages.island_construction_time.pause();

        self.counters
            .stages
            .island_constraints_collection_time
            .resume();
        let mut manifolds = Vec::new();
        narrow_phase.select_active_contacts(
            islands,
            bodies,
            &mut self.contact_pair_indices,
            &mut manifolds,
            &mut self.manifold_indices,
        );
        impulse_joints.select_active_interactions(
            islands,
            bodies,
            &mut self.joint_constraint_indices,
        );
        self.counters
            .stages
            .island_constraints_collection_time
            .pause();

        self.counters.stages.update_time.resume();
        for handle in islands.active_bodies() {
            // TODO: should that be moved to the solver (just like we moved
            //       the multibody dynamics update) since it depends on dt?
            let rb = bodies.index_mut_internal(handle);
            rb.mprops
                .update_world_mass_properties(rb.body_type, &rb.pos.position);
            let effective_mass = rb.mprops.effective_mass();
            rb.forces
                .compute_effective_force_and_torque(gravity, effective_mass);
        }
        self.counters.stages.update_time.pause();

        self.counters.stages.solver_time.resume();
        if self.solvers.len() < num_active_islands {
            self.solvers
                .resize_with(num_active_islands, IslandSolver::new);
        }

        #[cfg(not(feature = "parallel"))]
        {
            enable_flush_to_zero!();

            for (island_awake_id, island_id) in islands.active_islands().iter().enumerate() {
                self.solvers[island_awake_id].init_and_solve(
                    *island_id,
                    &mut self.counters,
                    integration_parameters,
                    islands,
                    bodies,
                    &mut manifolds[..],
                    &self.manifold_indices[island_awake_id],
                    impulse_joints.joints_mut(),
                    &self.joint_constraint_indices[island_awake_id],
                    multibody_joints,
                )
            }
        }

        #[cfg(feature = "parallel")]
        {
            use crate::geometry::ContactManifold;
            use rayon::prelude::*;
            use std::sync::atomic::Ordering;

            let solvers = &mut self.solvers[..num_active_islands];
            let bodies = &std::sync::atomic::AtomicPtr::new(bodies as *mut _);
            let manifolds = &std::sync::atomic::AtomicPtr::new(&mut manifolds as *mut _);
            let impulse_joints =
                &std::sync::atomic::AtomicPtr::new(impulse_joints.joints_vec_mut() as *mut _);
            let multibody_joints = &std::sync::atomic::AtomicPtr::new(multibody_joints as *mut _);
            let manifold_indices = &self.manifold_indices[..];
            let joint_constraint_indices = &self.joint_constraint_indices[..];

            // PERF: right now, we are only doing islands-based parallelism.
            //       Intra-island parallelism (that hasn’t been ported to the new
            //       solver yet) will be supported in the future.
            self.counters.solver.velocity_resolution_time.resume();
            rayon::scope(|_scope| {
                enable_flush_to_zero!();

                solvers
                    .par_iter_mut()
                    .enumerate()
                    .for_each(|(island_awake_id, solver)| {
                        let island_id = islands.active_islands()[island_awake_id];
                        let bodies: &mut RigidBodySet =
                            unsafe { &mut *bodies.load(Ordering::Relaxed) };
                        let manifolds: &mut Vec<&mut ContactManifold> =
                            unsafe { &mut *manifolds.load(Ordering::Relaxed) };
                        let impulse_joints: &mut Vec<JointGraphEdge> =
                            unsafe { &mut *impulse_joints.load(Ordering::Relaxed) };
                        let multibody_joints: &mut MultibodyJointSet =
                            unsafe { &mut *multibody_joints.load(Ordering::Relaxed) };

                        let mut counters = Counters::new(false);
                        solver.init_and_solve(
                            island_id,
                            &mut counters,
                            integration_parameters,
                            islands,
                            bodies,
                            &mut manifolds[..],
                            &manifold_indices[island_awake_id],
                            impulse_joints,
                            &joint_constraint_indices[island_awake_id],
                            multibody_joints,
                        )
                    });
            });
            self.counters.solver.velocity_resolution_time.pause();
        }

        // Generate contact force events if needed.
        let inv_dt = crate::utils::inv(integration_parameters.dt);
        for pair_id in self.contact_pair_indices.drain(..) {
            let pair = narrow_phase.contact_pair_at_index(pair_id);
            let co1 = &colliders[pair.collider1];
            let co2 = &colliders[pair.collider2];
            let threshold = co1
                .effective_contact_force_event_threshold()
                .min(co2.effective_contact_force_event_threshold());

            if threshold < Real::MAX {
                let total_magnitude = pair.total_impulse_magnitude() * inv_dt;

                // NOTE: the strict inequality is important here, so we don’t
                //       trigger an event if the force is 0.0 and the threshold is 0.0.
                if total_magnitude > threshold {
                    events.handle_contact_force_event(
                        integration_parameters.dt,
                        bodies,
                        colliders,
                        pair,
                        total_magnitude,
                    );
                }
            }
        }

        self.counters.stages.solver_time.pause();
    }

    fn run_ccd_motion_clamping(
        &mut self,
        integration_parameters: &IntegrationParameters,
        islands: &IslandManager,
        bodies: &mut RigidBodySet,
        colliders: &mut ColliderSet,
        broad_phase: &mut BroadPhaseBvh,
        narrow_phase: &NarrowPhase,
        ccd_solver: &mut CCDSolver,
        events: &dyn EventHandler,
    ) {
        self.counters.ccd.toi_computation_time.start();
        // Handle CCD
        let impacts = ccd_solver.predict_impacts_at_next_positions(
            integration_parameters,
            islands,
            bodies,
            colliders,
            broad_phase,
            narrow_phase,
            events,
        );
        ccd_solver.clamp_motions(integration_parameters.dt, bodies, &impacts);
        self.counters.ccd.toi_computation_time.pause();
    }

    fn advance_to_final_positions(
        &mut self,
        islands: &IslandManager,
        bodies: &mut RigidBodySet,
        colliders: &mut ColliderSet,
        modified_colliders: &mut ModifiedColliders,
    ) {
        // Set the rigid-bodies and kinematic bodies to their final position.
        for handle in islands.active_bodies() {
            let rb = bodies.index_mut_internal(handle);
            rb.pos.position = rb.pos.next_position;
            rb.colliders
                .update_positions(colliders, modified_colliders, &rb.pos.position);
        }
    }

    fn interpolate_kinematic_velocities(
        &mut self,
        integration_parameters: &IntegrationParameters,
        islands: &IslandManager,
        bodies: &mut RigidBodySet,
    ) {
        // Update kinematic bodies velocities.
        // TODO: what is the best place for this? It should at least be
        // located before the island computation because we test the velocity
        // there to determine if this kinematic body should wake-up dynamic
        // bodies it is touching.
        for handle in islands.active_bodies() {
            // TODO PERF: only iterate on kinematic position-based bodies
            let rb = bodies.index_mut_internal(handle);

            match rb.body_type {
                RigidBodyType::KinematicPositionBased => {
                    rb.vels = rb.pos.interpolate_velocity(
                        integration_parameters.inv_dt(),
                        rb.mprops.local_mprops.local_com,
                    );
                }
                RigidBodyType::KinematicVelocityBased => {}
                _ => {}
            }
        }
    }

    /// Advances the physics simulation by one timestep.
    ///
    /// This is the main function you'll call every frame in your game loop. It performs all
    /// physics calculations: collision detection, constraint solving, and updating object positions.
    ///
    /// # Parameters
    ///
    /// * `gravity` - The gravity vector applied to all dynamic bodies (e.g., `vector![0.0, -9.81, 0.0]` for Earth gravity pointing down)
    /// * `integration_parameters` - Controls the simulation quality and timestep size (typically 60 Hz = 1/60 second per step)
    /// * `islands` - Internal system that groups connected objects together for efficient solving (automatically managed)
    /// * `broad_phase` - Fast collision detection phase that filters out distant object pairs (automatically managed)
    /// * `narrow_phase` - Precise collision detection that computes exact contact points (automatically managed)
    /// * `bodies` - Your collection of rigid bodies (the physical objects that move and collide)
    /// * `colliders` - The collision shapes attached to your bodies (boxes, spheres, meshes, etc.)
    /// * `impulse_joints` - Regular joints connecting bodies (hinges, sliders, etc.)
    /// * `multibody_joints` - Articulated joints for robot-like structures (optional, can be empty)
    /// * `ccd_solver` - Continuous collision detection to prevent fast objects from tunneling through thin walls
    /// * `hooks` - Optional callbacks to customize collision filtering and contact modification
    /// * `events` - Optional handler to receive collision events (when objects start/stop touching)
    ///
    /// # Example
    ///
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let mut colliders = ColliderSet::new();
    /// # let mut impulse_joints = ImpulseJointSet::new();
    /// # let mut multibody_joints = MultibodyJointSet::new();
    /// # let mut islands = IslandManager::new();
    /// # let mut broad_phase = BroadPhaseBvh::new();
    /// # let mut narrow_phase = NarrowPhase::new();
    /// # let mut ccd_solver = CCDSolver::new();
    /// # let mut physics_pipeline = PhysicsPipeline::new();
    /// # let integration_parameters = IntegrationParameters::default();
    /// // In your game loop:
    /// physics_pipeline.step(
    ///     Vector::new(0.0, -9.81, 0.0),  // Gravity pointing down
    ///     &integration_parameters,
    ///     &mut islands,
    ///     &mut broad_phase,
    ///     &mut narrow_phase,
    ///     &mut bodies,
    ///     &mut colliders,
    ///     &mut impulse_joints,
    ///     &mut multibody_joints,
    ///     &mut ccd_solver,
    ///     &(),  // No custom hooks
    ///     &(),  // No event handler
    /// );
    /// ```
    pub fn step(
        &mut self,
        gravity: Vector,
        integration_parameters: &IntegrationParameters,
        islands: &mut IslandManager,
        broad_phase: &mut BroadPhaseBvh,
        narrow_phase: &mut NarrowPhase,
        bodies: &mut RigidBodySet,
        colliders: &mut ColliderSet,
        impulse_joints: &mut ImpulseJointSet,
        multibody_joints: &mut MultibodyJointSet,
        ccd_solver: &mut CCDSolver,
        hooks: &dyn PhysicsHooks,
        events: &dyn EventHandler,
    ) {
        self.counters.reset();
        self.counters.step_started();

        // Apply some of delayed wake-ups.
        self.counters.stages.user_changes.start();
        #[cfg(feature = "enhanced-determinism")]
        let to_wake_up_iterator = impulse_joints
            .to_wake_up
            .drain(..)
            .chain(multibody_joints.to_wake_up.drain(..));
        #[cfg(not(feature = "enhanced-determinism"))]
        let to_wake_up_iterator = impulse_joints
            .to_wake_up
            .drain()
            .chain(multibody_joints.to_wake_up.drain());
        for handle in to_wake_up_iterator {
            islands.wake_up(bodies, handle, true);
        }

        // Apply modifications.
        let mut modified_colliders = colliders.take_modified();
        let mut removed_colliders = colliders.take_removed();

        super::user_changes::handle_user_changes_to_colliders(
            bodies,
            colliders,
            &modified_colliders[..],
        );

        let mut modified_bodies = bodies.take_modified();
        super::user_changes::handle_user_changes_to_rigid_bodies(
            Some(islands),
            bodies,
            colliders,
            impulse_joints,
            multibody_joints,
            &modified_bodies,
            &mut modified_colliders,
        );

        // Disabled colliders are treated as if they were removed.
        // NOTE: this must be called here, after handle_user_changes_to_rigid_bodies to take into
        //       account colliders disabled because of their parent rigid-body.
        removed_colliders.extend(
            modified_colliders
                .iter()
                .copied()
                .filter(|h| colliders.get(*h).map(|c| !c.is_enabled()).unwrap_or(false)),
        );

        // Join islands based on new joints.
        #[cfg(feature = "enhanced-determinism")]
        let to_join_iterator = impulse_joints
            .to_join
            .drain(..)
            .chain(multibody_joints.to_join.drain(..));
        #[cfg(not(feature = "enhanced-determinism"))]
        let to_join_iterator = impulse_joints
            .to_join
            .drain()
            .chain(multibody_joints.to_join.drain());
        for (handle1, handle2) in to_join_iterator {
            islands.interaction_started_or_stopped(
                bodies,
                Some(handle1),
                Some(handle2),
                true,
                false,
            );
        }
        self.counters.stages.user_changes.pause();

        // TODO: do this only on user-change.
        // TODO: do we want some kind of automatic inverse kinematics?
        for multibody in &mut multibody_joints.multibodies {
            multibody.1.forward_kinematics(bodies, true);
            multibody
                .1
                .update_rigid_bodies_internal(bodies, true, false, false);
        }

        self.detect_collisions(
            integration_parameters,
            islands,
            broad_phase,
            narrow_phase,
            bodies,
            colliders,
            impulse_joints,
            multibody_joints,
            &modified_colliders,
            &removed_colliders,
            hooks,
            events,
            true,
        );

        self.counters.stages.user_changes.resume();
        self.clear_modified_colliders(colliders, &mut modified_colliders);
        self.clear_modified_bodies(bodies, &mut modified_bodies);
        removed_colliders.clear();
        self.counters.stages.user_changes.pause();

        let mut remaining_time = integration_parameters.dt;
        let mut integration_parameters = *integration_parameters;

        let (ccd_is_enabled, mut remaining_substeps) =
            if integration_parameters.max_ccd_substeps == 0 {
                (false, 1)
            } else {
                (true, integration_parameters.max_ccd_substeps)
            };

        while remaining_substeps > 0 {
            // If there are more than one CCD substep, we need to split
            // the timestep into multiple intervals. First, estimate the
            // size of the time slice we will integrate for this substep.
            //
            // Note that we must do this now, before the constraints resolution
            // because we need to use the correct timestep length for the
            // integration of external forces.
            //
            // If there is only one or zero CCD substep, there is no need
            // to split the timestep interval. So we can just skip this part.
            if ccd_is_enabled && remaining_substeps > 1 {
                // NOTE: Take forces into account when updating the bodies CCD activation flags
                //       these forces have not been integrated to the body's velocity yet.
                let ccd_active =
                    ccd_solver.update_ccd_active_flags(islands, bodies, remaining_time, true);
                let first_impact = if ccd_active {
                    ccd_solver.find_first_impact(
                        remaining_time,
                        &integration_parameters,
                        islands,
                        bodies,
                        colliders,
                        broad_phase,
                        narrow_phase,
                    )
                } else {
                    None
                };

                if let Some(toi) = first_impact {
                    let original_interval = remaining_time / (remaining_substeps as Real);

                    if toi < original_interval {
                        integration_parameters.dt = original_interval;
                    } else {
                        integration_parameters.dt =
                            toi + (remaining_time - toi) / (remaining_substeps as Real);
                    }

                    remaining_substeps -= 1;
                } else {
                    // No impact, don't do any other substep after this one.
                    integration_parameters.dt = remaining_time;
                    remaining_substeps = 0;
                }

                remaining_time -= integration_parameters.dt;

                // Avoid substep length that are too small.
                if remaining_time <= integration_parameters.min_ccd_dt {
                    integration_parameters.dt += remaining_time;
                    remaining_substeps = 0;
                }
            } else {
                integration_parameters.dt = remaining_time;
                remaining_time = 0.0;
                remaining_substeps = 0;
            }

            self.counters.ccd.num_substeps += 1;

            self.counters.custom.resume();
            self.interpolate_kinematic_velocities(&integration_parameters, islands, bodies);
            self.counters.custom.pause();
            self.build_islands_and_solve_velocity_constraints(
                gravity,
                &integration_parameters,
                islands,
                narrow_phase,
                bodies,
                colliders,
                impulse_joints,
                multibody_joints,
                events,
            );

            // If CCD is enabled, execute the CCD motion clamping.
            if ccd_is_enabled {
                // NOTE: don't the forces into account when updating the CCD active flags because
                //       they have already been integrated into the velocities by the solver.
                let ccd_active = ccd_solver.update_ccd_active_flags(
                    islands,
                    bodies,
                    integration_parameters.dt,
                    false,
                );
                if ccd_active {
                    self.run_ccd_motion_clamping(
                        &integration_parameters,
                        islands,
                        bodies,
                        colliders,
                        broad_phase,
                        narrow_phase,
                        ccd_solver,
                        events,
                    );
                }
            }

            self.counters.stages.update_time.resume();
            self.advance_to_final_positions(islands, bodies, colliders, &mut modified_colliders);
            self.counters.stages.update_time.pause();

            if remaining_substeps > 0 {
                self.detect_collisions(
                    &integration_parameters,
                    islands,
                    broad_phase,
                    narrow_phase,
                    bodies,
                    colliders,
                    impulse_joints,
                    multibody_joints,
                    &modified_colliders,
                    &[],
                    hooks,
                    events,
                    false,
                );

                self.clear_modified_colliders(colliders, &mut modified_colliders);
            } else {
                // If we ran the last substep, just update the broad-phase bvh instead
                // of a full collision-detection step.
                self.counters.stages.collision_detection_time.resume();
                self.counters.cd.final_broad_phase_time.resume();
                for handle in modified_colliders.iter() {
                    let co = colliders.index_mut_internal(*handle);
                    // NOTE: `advance_to_final_positions` might have added disabled colliders to
                    //       `modified_colliders`. This raises the question: do we want
                    //       rigid-body transform propagation to happen on disabled colliders if
                    //       their parent rigid-body is enabled? For now, we are propagating as
                    //       it feels less surprising to the user and makes handling collider
                    //       re-enable less awkward.
                    if co.is_enabled() {
                        let aabb = co.compute_broad_phase_aabb(&integration_parameters, bodies);
                        broad_phase.set_aabb(&integration_parameters, *handle, aabb);
                    }

                    // Clear the modified collider set, but keep the other collider changes flags.
                    // This is needed so that the narrow-phase at the next timestep knows it must
                    // not skip these colliders for its update.
                    // TODO: this doesn’t feel very clean, but leaving the collider in the modified
                    //       set would be expensive as this will be traversed by all the user-changes
                    //       functions. An alternative would be to maintain a second modified set,
                    //       one for user changes, and one for changes applied by the solver but that
                    //       feels a bit too much. Let’s keep it simple for now and we’ll see how it
                    //       goes after the persistent island rework.
                    co.changes.remove(ColliderChanges::IN_MODIFIED_SET);
                }

                // Empty the modified colliders set. See comment for `co.change.remove(..)` above.
                modified_colliders.clear();
                self.counters.cd.final_broad_phase_time.pause();
                self.counters.stages.collision_detection_time.pause();
            }
        }

        // Finally, make sure we update the world mass-properties of the rigid-bodies
        // that moved. Otherwise, users may end up applying forces with respect to an
        // outdated center of mass.
        // TODO: avoid updating the world mass properties twice (here, and
        //       at the beginning of the next timestep) for bodies that were
        //       not modified by the user in the mean time.
        self.counters.stages.update_time.resume();
        for handle in islands.active_bodies() {
            let rb = bodies.index_mut_internal(handle);
            rb.mprops
                .update_world_mass_properties(rb.body_type, &rb.pos.position);
        }
        self.counters.stages.update_time.pause();

        // Re-insert the modified vector we extracted for the borrow-checker.
        colliders.set_modified(modified_colliders);

        self.counters.step_completed();
    }
}

#[cfg(test)]
mod test {
    use crate::dynamics::{
        CCDSolver, ImpulseJointSet, IntegrationParameters, IslandManager, RigidBodyBuilder,
        RigidBodySet,
    };
    use crate::geometry::{BroadPhaseBvh, ColliderBuilder, ColliderSet, NarrowPhase};
    #[cfg(feature = "dim2")]
    use crate::math::Rotation;
    use crate::math::Vector;
    use crate::pipeline::PhysicsPipeline;
    use crate::prelude::{MultibodyJointSet, RevoluteJointBuilder, RigidBodyType};

    #[test]
    fn kinematic_and_fixed_contact_crash() {
        let mut colliders = ColliderSet::new();
        let mut impulse_joints = ImpulseJointSet::new();
        let mut multibody_joints = MultibodyJointSet::new();
        let mut pipeline = PhysicsPipeline::new();
        let mut bf = BroadPhaseBvh::new();
        let mut nf = NarrowPhase::new();
        let mut bodies = RigidBodySet::new();
        let mut islands = IslandManager::new();

        let rb = RigidBodyBuilder::fixed().build();
        let h1 = bodies.insert(rb.clone());
        let co = ColliderBuilder::ball(10.0).build();
        colliders.insert_with_parent(co.clone(), h1, &mut bodies);

        // The same but with a kinematic body.
        let rb = RigidBodyBuilder::kinematic_position_based().build();
        let h2 = bodies.insert(rb.clone());
        colliders.insert_with_parent(co, h2, &mut bodies);

        pipeline.step(
            Vector::ZERO,
            &IntegrationParameters::default(),
            &mut islands,
            &mut bf,
            &mut nf,
            &mut bodies,
            &mut colliders,
            &mut impulse_joints,
            &mut multibody_joints,
            &mut CCDSolver::new(),
            &(),
            &(),
        );
    }

    #[test]
    fn rigid_body_removal_before_step() {
        let mut colliders = ColliderSet::new();
        let mut impulse_joints = ImpulseJointSet::new();
        let mut multibody_joints = MultibodyJointSet::new();
        let mut pipeline = PhysicsPipeline::new();
        let mut bf = BroadPhaseBvh::new();
        let mut nf = NarrowPhase::new();
        let mut islands = IslandManager::new();

        let mut bodies = RigidBodySet::new();

        // Check that removing the body right after inserting it works.
        // We add two dynamic bodies, one kinematic body and one fixed body before removing
        // them. This include a non-regression test where deleting a kinematic body crashes.
        let rb = RigidBodyBuilder::dynamic().build();
        let h1 = bodies.insert(rb.clone());
        let h2 = bodies.insert(rb.clone());

        // The same but with a kinematic body.
        let rb = RigidBodyBuilder::kinematic_position_based().build();
        let h3 = bodies.insert(rb.clone());

        // The same but with a fixed body.
        let rb = RigidBodyBuilder::fixed().build();
        let h4 = bodies.insert(rb.clone());

        let to_delete = [h1, h2, h3, h4];
        for h in &to_delete {
            bodies.remove(
                *h,
                &mut islands,
                &mut colliders,
                &mut impulse_joints,
                &mut multibody_joints,
                true,
            );
        }

        pipeline.step(
            Vector::ZERO,
            &IntegrationParameters::default(),
            &mut islands,
            &mut bf,
            &mut nf,
            &mut bodies,
            &mut colliders,
            &mut impulse_joints,
            &mut multibody_joints,
            &mut CCDSolver::new(),
            &(),
            &(),
        );
    }

    #[cfg(feature = "serde-serialize")]
    #[test]
    fn rigid_body_removal_snapshot_handle_determinism() {
        let mut colliders = ColliderSet::new();
        let mut impulse_joints = ImpulseJointSet::new();
        let mut multibody_joints = MultibodyJointSet::new();
        let mut islands = IslandManager::new();

        let mut bodies = RigidBodySet::new();
        let rb = RigidBodyBuilder::dynamic().build();
        let h1 = bodies.insert(rb.clone());
        let h2 = bodies.insert(rb.clone());
        let h3 = bodies.insert(rb.clone());

        bodies.remove(
            h1,
            &mut islands,
            &mut colliders,
            &mut impulse_joints,
            &mut multibody_joints,
            true,
        );
        bodies.remove(
            h3,
            &mut islands,
            &mut colliders,
            &mut impulse_joints,
            &mut multibody_joints,
            true,
        );
        bodies.remove(
            h2,
            &mut islands,
            &mut colliders,
            &mut impulse_joints,
            &mut multibody_joints,
            true,
        );

        let ser_bodies = bincode::serialize(&bodies).unwrap();
        let mut bodies2: RigidBodySet = bincode::deserialize(&ser_bodies).unwrap();

        let h1a = bodies.insert(rb.clone());
        let h2a = bodies.insert(rb.clone());
        let h3a = bodies.insert(rb.clone());

        let h1b = bodies2.insert(rb.clone());
        let h2b = bodies2.insert(rb.clone());
        let h3b = bodies2.insert(rb.clone());

        assert_eq!(h1a, h1b);
        assert_eq!(h2a, h2b);
        assert_eq!(h3a, h3b);
    }

    #[test]
    fn collider_removal_before_step() {
        let mut pipeline = PhysicsPipeline::new();
        let gravity = Vector::Y * -9.81;
        let integration_parameters = IntegrationParameters::default();
        let mut broad_phase = BroadPhaseBvh::new();
        let mut narrow_phase = NarrowPhase::new();
        let mut bodies = RigidBodySet::new();
        let mut colliders = ColliderSet::new();
        let mut ccd = CCDSolver::new();
        let mut impulse_joints = ImpulseJointSet::new();
        let mut multibody_joints = MultibodyJointSet::new();
        let mut islands = IslandManager::new();
        let physics_hooks = ();
        let event_handler = ();

        let body = RigidBodyBuilder::dynamic().build();
        let b_handle = bodies.insert(body);
        let collider = ColliderBuilder::ball(1.0).build();
        let c_handle = colliders.insert_with_parent(collider, b_handle, &mut bodies);
        colliders.remove(c_handle, &mut islands, &mut bodies, true);
        bodies.remove(
            b_handle,
            &mut islands,
            &mut colliders,
            &mut impulse_joints,
            &mut multibody_joints,
            true,
        );

        for _ in 0..10 {
            pipeline.step(
                gravity,
                &integration_parameters,
                &mut islands,
                &mut broad_phase,
                &mut narrow_phase,
                &mut bodies,
                &mut colliders,
                &mut impulse_joints,
                &mut multibody_joints,
                &mut ccd,
                &physics_hooks,
                &event_handler,
            );
        }
    }

    #[test]
    fn rigid_body_type_changed_dynamic_is_in_active_set() {
        let mut colliders = ColliderSet::new();
        let mut impulse_joints = ImpulseJointSet::new();
        let mut multibody_joints = MultibodyJointSet::new();
        let mut pipeline = PhysicsPipeline::new();
        let mut bf = BroadPhaseBvh::new();
        let mut nf = NarrowPhase::new();
        let mut islands = IslandManager::new();

        let mut bodies = RigidBodySet::new();

        // Initialize body as kinematic with mass
        let rb = RigidBodyBuilder::kinematic_position_based()
            .additional_mass(1.0)
            .build();
        let h = bodies.insert(rb.clone());

        // Step once
        let gravity = Vector::Y * -9.81;
        pipeline.step(
            gravity,
            &IntegrationParameters::default(),
            &mut islands,
            &mut bf,
            &mut nf,
            &mut bodies,
            &mut colliders,
            &mut impulse_joints,
            &mut multibody_joints,
            &mut CCDSolver::new(),
            &(),
            &(),
        );

        // Switch body type to Dynamic
        bodies
            .get_mut(h)
            .unwrap()
            .set_body_type(RigidBodyType::Dynamic, true);

        // Step again
        pipeline.step(
            gravity,
            &IntegrationParameters::default(),
            &mut islands,
            &mut bf,
            &mut nf,
            &mut bodies,
            &mut colliders,
            &mut impulse_joints,
            &mut multibody_joints,
            &mut CCDSolver::new(),
            &(),
            &(),
        );

        let body = bodies.get(h).unwrap();
        let h_y = body.pos.position.translation.y;

        // Expect gravity to be applied on second step after switching to Dynamic
        assert!(h_y < 0.0);

        // Expect body to now be awake (not sleeping)
        assert!(!body.is_sleeping());
    }

    #[test]
    fn joint_step_delta_time_0() {
        let mut colliders = ColliderSet::new();
        let mut impulse_joints = ImpulseJointSet::new();
        let mut multibody_joints = MultibodyJointSet::new();
        let mut pipeline = PhysicsPipeline::new();
        let mut bf = BroadPhaseBvh::new();
        let mut nf = NarrowPhase::new();
        let mut islands = IslandManager::new();

        let mut bodies = RigidBodySet::new();

        // Initialize bodies
        let rb = RigidBodyBuilder::fixed().additional_mass(1.0).build();
        let h = bodies.insert(rb.clone());
        let rb_dynamic = RigidBodyBuilder::dynamic().additional_mass(1.0).build();
        let h_dynamic = bodies.insert(rb_dynamic.clone());

        // Add joint
        #[cfg(feature = "dim2")]
        let joint = RevoluteJointBuilder::new()
            .local_anchor1(Vector::new(0.0, 1.0))
            .local_anchor2(Vector::new(0.0, -3.0));
        #[cfg(feature = "dim3")]
        let joint = RevoluteJointBuilder::new(Vector::Z)
            .local_anchor1(Vector::new(0.0, 1.0, 0.0))
            .local_anchor2(Vector::new(0.0, -3.0, 0.0));
        impulse_joints.insert(h, h_dynamic, joint, true);

        let parameters = IntegrationParameters {
            dt: 0.0,
            ..Default::default()
        };
        // Step once
        let gravity = Vector::Y * -9.81;
        pipeline.step(
            gravity,
            &parameters,
            &mut islands,
            &mut bf,
            &mut nf,
            &mut bodies,
            &mut colliders,
            &mut impulse_joints,
            &mut multibody_joints,
            &mut CCDSolver::new(),
            &(),
            &(),
        );
        let translation = bodies[h_dynamic].translation();
        let rotation = bodies[h_dynamic].rotation();
        assert!(translation.x.is_finite());
        assert!(translation.y.is_finite());
        #[cfg(feature = "dim2")]
        {
            assert!(rotation.re.is_finite());
            assert!(rotation.im.is_finite());
        }
        #[cfg(feature = "dim3")]
        {
            assert!(translation.z.is_finite());
            assert!(rotation.x.is_finite());
            assert!(rotation.y.is_finite());
            assert!(rotation.z.is_finite());
            assert!(rotation.w.is_finite());
        }
    }

    #[test]
    #[cfg(feature = "dim2")]
    fn test_multi_sap_disable_body() {
        let mut rigid_body_set = RigidBodySet::new();
        let mut collider_set = ColliderSet::new();

        /* Create the ground. */
        let collider = ColliderBuilder::cuboid(100.0, 0.1);
        collider_set.insert(collider);

        /* Create the bouncing ball. */
        let rigid_body = RigidBodyBuilder::dynamic().translation(Vector::new(0.0, 10.0));
        let collider = ColliderBuilder::ball(0.5).restitution(0.7);
        let ball_body_handle = rigid_body_set.insert(rigid_body);
        collider_set.insert_with_parent(collider, ball_body_handle, &mut rigid_body_set);

        /* Create other structures necessary for the simulation. */
        let gravity = Vector::new(0.0, -9.81);
        let integration_parameters = IntegrationParameters::default();
        let mut physics_pipeline = PhysicsPipeline::new();
        let mut island_manager = IslandManager::new();
        let mut broad_phase = BroadPhaseBvh::new();
        let mut narrow_phase = NarrowPhase::new();
        let mut impulse_joint_set = ImpulseJointSet::new();
        let mut multibody_joint_set = MultibodyJointSet::new();
        let mut ccd_solver = CCDSolver::new();
        let physics_hooks = ();
        let event_handler = ();

        physics_pipeline.step(
            gravity,
            &integration_parameters,
            &mut island_manager,
            &mut broad_phase,
            &mut narrow_phase,
            &mut rigid_body_set,
            &mut collider_set,
            &mut impulse_joint_set,
            &mut multibody_joint_set,
            &mut ccd_solver,
            &physics_hooks,
            &event_handler,
        );

        // Test RigidBodyChanges::POSITION and disable
        {
            let ball_body = &mut rigid_body_set[ball_body_handle];

            // Also, change the translation and rotation to different values
            ball_body.set_translation(Vector::new(1.0, 1.0), true);
            ball_body.set_rotation(Rotation::from_angle(1.0), true);
            ball_body.set_enabled(false);
        }

        physics_pipeline.step(
            gravity,
            &integration_parameters,
            &mut island_manager,
            &mut broad_phase,
            &mut narrow_phase,
            &mut rigid_body_set,
            &mut collider_set,
            &mut impulse_joint_set,
            &mut multibody_joint_set,
            &mut ccd_solver,
            &physics_hooks,
            &event_handler,
        );

        // Test RigidBodyChanges::POSITION and enable
        {
            let ball_body = &mut rigid_body_set[ball_body_handle];

            // Also, change the translation and rotation to different values
            ball_body.set_translation(Vector::new(0.0, 0.0), true);
            ball_body.set_rotation(Rotation::from_angle(0.0), true);
            ball_body.set_enabled(true);
        }

        physics_pipeline.step(
            gravity,
            &integration_parameters,
            &mut island_manager,
            &mut broad_phase,
            &mut narrow_phase,
            &mut rigid_body_set,
            &mut collider_set,
            &mut impulse_joint_set,
            &mut multibody_joint_set,
            &mut ccd_solver,
            &physics_hooks,
            &event_handler,
        );
    }
}

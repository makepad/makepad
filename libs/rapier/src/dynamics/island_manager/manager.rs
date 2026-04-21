use super::{Island, IslandsOptimizer};
use crate::dynamics::{
    ImpulseJointSet, MultibodyJointSet, RigidBodyChanges, RigidBodyHandle, RigidBodyIds,
    RigidBodySet,
};
use crate::geometry::{ColliderSet, NarrowPhase};
use crate::math::Real;
use crate::prelude::SleepRootState;
use crate::utils::DotProduct;
use std::collections::VecDeque;
use vec_map::VecMap;

/// An island starting at this rigid-body might be eligible for sleeping.
#[derive(Copy, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub(super) struct SleepCandidate(RigidBodyHandle);

/// System that manages which bodies are active (awake) vs sleeping to optimize performance.
///
/// ## Sleeping Optimization
///
/// Bodies at rest automatically "sleep" - they're excluded from simulation until something
/// disturbs them (collision, joint connection to moving body, manual wake-up). This can
/// dramatically improve performance in scenes with many static/resting objects.
///
/// ## Islands
///
/// Connected bodies (via contacts or joints) are grouped into "islands" that are solved together.
/// This allows parallel solving and better organization.
///
/// You rarely interact with this directly - it's automatically managed by [`PhysicsPipeline`](crate::pipeline::PhysicsPipeline).
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Default)]
pub struct IslandManager {
    pub(crate) islands: VecMap<Island>,
    pub(crate) awake_islands: Vec<usize>,
    // TODO PERF: should this be `Vec<(usize, Island)>` to reuse the allocation?
    pub(crate) free_islands: Vec<usize>,
    /// Potential candidate roots for graph traversal to identify a sleeping
    /// connected component or to split an island in two.
    pub(super) traversal_candidates: VecDeque<SleepCandidate>,
    pub(super) traversal_timestamp: u32,
    pub(super) optimizer: IslandsOptimizer,
    #[cfg_attr(feature = "serde-serialize", serde(skip))]
    pub(super) stack: Vec<RigidBodyHandle>, // Workspace.
}

impl IslandManager {
    /// Creates a new empty island manager.
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn active_islands(&self) -> &[usize] {
        &self.awake_islands
    }

    pub(crate) fn rigid_body_removed_or_disabled(
        &mut self,
        removed_handle: RigidBodyHandle,
        removed_ids: &RigidBodyIds,
        bodies: &mut RigidBodySet,
    ) {
        let Some(island) = self.islands.get_mut(removed_ids.active_island_id) else {
            // The island already doesn’t exist.
            return;
        };

        // If the rigid-body was disabled, it is still in the body set. Invalid its islands ids.
        if let Some(body) = bodies.get_mut_internal(removed_handle) {
            body.ids.active_island_id = usize::MAX;
            body.ids.active_set_id = usize::MAX;
        }

        let swapped_handle = island.bodies.last().copied().unwrap_or(removed_handle);
        island.bodies.swap_remove(removed_ids.active_set_id);

        // Remap the active_set_id of the body we moved with the `swap_remove`.
        if swapped_handle != removed_handle {
            let swapped_body = bodies
                .get_mut(swapped_handle)
                .expect("Internal error: bodies must be removed from islands on at a times");
            swapped_body.ids.active_set_id = removed_ids.active_set_id;
        }

        // If we deleted the last body from this island, delete the island.
        if island.bodies.is_empty() {
            if let Some(awake_id) = island.id_in_awake_list {
                // Remove it from the free island list.
                self.awake_islands.swap_remove(awake_id);
                // Update the awake list index of the awake island id we moved.
                if let Some(moved_id) = self.awake_islands.get(awake_id) {
                    self.islands[*moved_id].id_in_awake_list = Some(awake_id);
                }
            }
            self.islands.remove(removed_ids.active_island_id);
            self.free_islands.push(removed_ids.active_island_id);
        }
    }

    pub(crate) fn interaction_started_or_stopped(
        &mut self,
        bodies: &mut RigidBodySet,
        handle1: Option<RigidBodyHandle>,
        handle2: Option<RigidBodyHandle>,
        started: bool,
        wake_up: bool,
    ) {
        match (handle1, handle2) {
            (Some(handle1), Some(handle2)) => {
                if wake_up {
                    self.wake_up(bodies, handle1, false);
                    self.wake_up(bodies, handle2, false);
                }

                if started {
                    if let (Some(rb1), Some(rb2)) = (bodies.get(handle1), bodies.get(handle2)) {
                        assert!(rb1.is_fixed() || rb1.ids.active_island_id != usize::MAX);
                        assert!(rb2.is_fixed() || rb2.ids.active_island_id != usize::MAX);

                        // If both bodies are not part of the same island, merge the islands.
                        if !rb1.is_fixed()
                            && !rb2.is_fixed()
                            && rb1.ids.active_island_id != rb2.ids.active_island_id
                        {
                            self.merge_islands(
                                bodies,
                                rb1.ids.active_island_id,
                                rb2.ids.active_island_id,
                            );
                        }
                    }
                }
            }
            (Some(handle1), None) => {
                if wake_up {
                    // NOTE: see NOTE of the Some(_), Some(_) case.
                    self.wake_up(bodies, handle1, false);
                }
            }
            (None, Some(handle2)) => {
                if wake_up {
                    // NOTE: see NOTE of the Some(_), Some(_) case.
                    self.wake_up(bodies, handle2, false);
                }
            }
            (None, None) => { /* Nothing to do. */ }
        }
    }

    pub(crate) fn island(&self, island_id: usize) -> &Island {
        &self.islands[island_id]
    }

    /// Handles of dynamic and kinematic rigid-bodies that are currently active (i.e. not sleeping).
    #[inline]
    pub fn active_bodies(&self) -> impl Iterator<Item = RigidBodyHandle> + '_ {
        self.awake_islands
            .iter()
            .flat_map(|i| self.islands[*i].bodies.iter().copied())
    }

    pub(crate) fn rigid_body_updated(
        &mut self,
        handle: RigidBodyHandle,
        bodies: &mut RigidBodySet,
    ) {
        let Some(rb) = bodies.get_mut(handle) else {
            return;
        };

        if rb.is_fixed() {
            return;
        }

        // Check if this is the first time we see this rigid-body.
        if rb.ids.active_island_id == usize::MAX {
            // Check if there is room in the last awake island to add this body.
            // NOTE: only checking the last is suboptimal. Perhaps we should keep vec of
            //       small islands ids?
            let insert_in_last_island = self.awake_islands.last().map(|id| {
                self.islands[*id].bodies.len() < self.optimizer.min_island_size
                    && self.islands[*id].is_sleeping() == rb.is_sleeping()
            });
            // let insert_in_last_island = insert_in_last_island.is_some().then_some(true);

            if !rb.is_sleeping() && insert_in_last_island == Some(true) {
                let id = *self.awake_islands.last().unwrap_or_else(|| unreachable!());
                let target_island = &mut self.islands[id];

                rb.ids.active_island_id = id;
                rb.ids.active_set_id = target_island.bodies.len();
                target_island.bodies.push(handle);
            } else {
                let mut new_island = Island::singleton(handle, rb);
                let id = self.free_islands.pop().unwrap_or(self.islands.len());

                if !rb.is_sleeping() {
                    new_island.id_in_awake_list = Some(self.awake_islands.len());
                    self.awake_islands.push(id);
                }

                self.islands.insert(id, new_island);
                rb.ids.active_island_id = id;
                rb.ids.active_set_id = 0;
            }
        }

        // Push the body to the active set if it is not inside the active set yet, and
        // is not longer sleeping or became dynamic.
        if (rb.changes.contains(RigidBodyChanges::SLEEP)
            || rb.changes.contains(RigidBodyChanges::TYPE))
            && rb.is_enabled()
            // Don’t wake up if the user put it to sleep manually.
            && !rb.activation.sleeping
        {
            self.wake_up(bodies, handle, false);
        }
    }

    pub(crate) fn update_islands(
        &mut self,
        dt: Real,
        length_unit: Real,
        bodies: &mut RigidBodySet,
        colliders: &ColliderSet,
        narrow_phase: &NarrowPhase,
        impulse_joints: &ImpulseJointSet,
        multibody_joints: &MultibodyJointSet,
    ) {
        // 1. Update active rigid-bodies energy.
        // TODO PERF: should this done by the velocity solver after solving the constraints?
        // let t0 = std::time::Instant::now();
        for handle in self
            .awake_islands
            .iter()
            .flat_map(|i| self.islands[*i].bodies.iter().copied())
        {
            let Some(rb) = bodies.get_mut_internal(handle) else {
                // This branch happens if the rigid-body no longer exists.
                continue;
            };
            let sq_linvel = rb.vels.linvel.length_squared();
            let sq_angvel = rb.vels.angvel.gdot(rb.vels.angvel);
            rb.activation
                .update_energy(rb.body_type, length_unit, sq_linvel, sq_angvel, dt);

            let can_sleep_now = rb.activation.is_eligible_for_sleep();

            // 2. Identify active rigid-bodies that transition from "awake" to "can_sleep"
            //    and push the sleep root candidate if applicable.
            if can_sleep_now && rb.activation.sleep_root_state == SleepRootState::Unknown {
                // This is a new candidate for island extraction.
                self.traversal_candidates.push_back(SleepCandidate(handle));
                rb.activation.sleep_root_state = SleepRootState::TraversalPending;
            } else if !can_sleep_now {
                rb.activation.sleep_root_state = SleepRootState::Unknown;
            }
        }
        // println!("Update energy: {}", t0.elapsed().as_secs_f32() * 1000.0);

        let mut cost = 0;

        // 3. Perform one, or multiple, sleeping islands extraction (graph traversal).
        //    Limit the traversal cost by not traversing all the known sleeping roots if
        //    there are too many.
        const MAX_PER_FRAME_COST: usize = 1000; // TODO: find the best value.
        while let Some(sleep_root) = self.traversal_candidates.pop_front() {
            cost += self.extract_sleeping_island(
                bodies,
                colliders,
                impulse_joints,
                multibody_joints,
                narrow_phase,
                sleep_root.0,
            );

            if cost > MAX_PER_FRAME_COST {
                // Early-break if we consider we have done enough island extraction work.
                break;
            }
        }

        self.update_optimizer(
            bodies,
            colliders,
            impulse_joints,
            multibody_joints,
            narrow_phase,
        );
        // println!("Island extraction: {}", t0.elapsed().as_secs_f32() * 1000.0);

        // NOTE: uncomment for debugging.
        // self.assert_state_is_valid(bodies, colliders, narrow_phase);
    }
}

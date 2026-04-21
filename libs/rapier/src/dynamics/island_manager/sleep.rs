use crate::dynamics::{
    ImpulseJointSet, MultibodyJointSet, RigidBodyHandle, RigidBodySet, SleepRootState,
};
use crate::geometry::{ColliderSet, NarrowPhase};

use super::{Island, IslandManager};

impl IslandManager {
    /// Wakes up a sleeping body, forcing it back into the active simulation.
    ///
    /// Use this when you want to ensure a body is active (useful after manually moving
    /// a sleeping body, or to prevent it from sleeping in the next few frames).
    ///
    /// # Parameters
    /// * `strong` - If `true`, the body is guaranteed to stay awake for multiple frames.
    ///   If `false`, it might sleep again immediately if conditions are met.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let mut islands = IslandManager::new();
    /// # let body_handle = bodies.insert(RigidBodyBuilder::dynamic());
    /// islands.wake_up(&mut bodies, body_handle, true);
    /// let body = bodies.get_mut(body_handle).unwrap();
    /// // Wake up a body before applying force to it
    /// body.add_force(Vector::new(100.0, 0.0, 0.0), false);
    /// ```
    ///
    /// Only affects dynamic bodies (kinematic and fixed bodies don't sleep).
    pub fn wake_up(&mut self, bodies: &mut RigidBodySet, handle: RigidBodyHandle, strong: bool) {
        // NOTE: the use an Option here because there are many legitimate cases (like when
        //       deleting a joint attached to an already-removed body) where we could be
        //       attempting to wake-up a rigid-body that has already been deleted.
        if bodies.get(handle).map(|rb| !rb.is_fixed()) == Some(true) {
            let rb = bodies.index_mut_internal(handle);

            // TODO: not sure if this is still relevant:
            // // Check that the user didn’t change the sleeping state explicitly, in which
            // // case we don’t overwrite it.
            // if rb.changes.contains(RigidBodyChanges::SLEEP) {
            //     return;
            // }

            rb.activation.wake_up(strong);
            let island_to_wake_up = rb.ids.active_island_id;
            self.wake_up_island(bodies, island_to_wake_up);
        }
    }

    /// Returns the number of iterations run by the graph traversal so we can balance load across
    /// frames.
    pub(super) fn extract_sleeping_island(
        &mut self,
        bodies: &mut RigidBodySet,
        colliders: &ColliderSet,
        impulse_joints: &ImpulseJointSet,
        multibody_joints: &MultibodyJointSet,
        narrow_phase: &NarrowPhase,
        sleep_root: RigidBodyHandle,
    ) -> usize {
        let Some(rb) = bodies.get_mut_internal(sleep_root) else {
            // This branch happens if the rigid-body no longer exists.
            return 0;
        };

        if rb.activation.sleep_root_state != SleepRootState::TraversalPending {
            // We already traversed this sleep root.
            return 0;
        }

        rb.activation.sleep_root_state = SleepRootState::Traversed;

        let active_island_id = rb.ids.active_island_id;
        let active_island = &mut self.islands[active_island_id];
        if active_island.is_sleeping() {
            // This rigid-body is already part of a sleeping island.
            return 0;
        }

        // TODO: implement recycling islands to avoid repeated allocations?
        let mut new_island = Island::default();
        self.stack.clear();
        self.stack.push(sleep_root);

        let mut niter = 0;
        self.traversal_timestamp += 1;

        while let Some(handle) = self.stack.pop() {
            let rb = bodies.index_mut_internal(handle);

            if rb.is_fixed() {
                // Don’t propagate islands through fixed bodies.
                continue;
            }

            if rb.ids.active_set_timestamp == self.traversal_timestamp {
                // We already visited this body and its neighbors.
                continue;
            }

            // if rb.ids.active_set_timestamp >= frame_base_timestamp {
            //     // We already visited this body and its neighbors during this frame.
            //     // So we already know this islands cannot sleep (otherwise the bodies
            //     // currently being traversed would already have been marked as sleeping).
            //     return niter;
            // }

            niter += 1;
            rb.ids.active_set_timestamp = self.traversal_timestamp;

            if rb.activation.is_eligible_for_sleep() {
                rb.activation.sleep_root_state = SleepRootState::Traversed;
            }

            assert_eq!(
                rb.ids.active_island_id,
                active_island_id,
                "handle: {:?}, note niter: {}, isl size: {}",
                handle,
                niter,
                active_island.len()
            );
            assert!(
                !rb.activation.sleeping,
                "is sleeping: {:?} note niter: {}, isl size: {}",
                handle,
                niter,
                active_island.len()
            );

            if !rb.activation.is_eligible_for_sleep() {
                // If this body cannot sleep, abort the traversal, we are not traversing
                // yet an island that can sleep.
                self.stack.clear();
                return niter;
            }

            // Traverse bodies that are interacting with the current one either through
            // contacts or a joint.
            super::utils::push_contacting_bodies(
                &rb.colliders,
                colliders,
                narrow_phase,
                &mut self.stack,
            );
            super::utils::push_linked_bodies(
                impulse_joints,
                multibody_joints,
                handle,
                &mut self.stack,
            );
            new_island.bodies.push(handle);
        }

        // If we reached this line, we completed a sleeping island traversal.
        // - Put its bodies to sleep.
        // - Remove them from the active set.
        // - Push the sleeping island.
        if active_island.len() == new_island.len() {
            // The whole island is asleep. No need to insert a new one.
            // Put all its bodies to sleep.
            for handle in &active_island.bodies {
                let rb = bodies.index_mut_internal(*handle);
                rb.sleep();
            }

            // Mark the existing island as sleeping (by clearing its `id_in_awake_list`)
            // and remove it from the awake list.
            let island_awake_id = active_island
                .id_in_awake_list
                .take()
                .unwrap_or_else(|| unreachable!());
            self.awake_islands.swap_remove(island_awake_id);

            if let Some(moved_id) = self.awake_islands.get(island_awake_id) {
                self.islands[*moved_id].id_in_awake_list = Some(island_awake_id);
            }
        } else {
            niter += new_island.len(); // Include this part into the cost estimate for this function.
            self.extract_sub_island(bodies, active_island_id, new_island, true);
        }
        niter
    }

    fn wake_up_island(&mut self, bodies: &mut RigidBodySet, island_id: usize) {
        let Some(island) = self.islands.get_mut(island_id) else {
            return;
        };

        if island.is_sleeping() {
            island.id_in_awake_list = Some(self.awake_islands.len());
            self.awake_islands.push(island_id);

            // Wake up all the bodies from this island.
            for handle in &island.bodies {
                if let Some(rb) = bodies.get_mut(*handle) {
                    rb.wake_up(false);
                }
            }
        }
    }
}

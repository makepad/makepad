use crate::dynamics::{ImpulseJointSet, MultibodyJointSet, RigidBodySet};
use crate::geometry::{ColliderSet, NarrowPhase};
use std::ops::IndexMut;

use super::{Island, IslandManager};

#[derive(Copy, Clone, Default)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub(super) struct IslandsOptimizerMergeState {
    curr_awake_id: usize,
}

#[derive(Copy, Clone, Default)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub(super) struct IslandsOptimizerSplitState {
    curr_awake_id: usize,
    #[allow(dead_code)]
    curr_body_id: usize,
}

/// Configuration of the awake islands optimization strategy.
///
/// Note that this currently only affects active islands. Sleeping islands are always kept minimal.
#[derive(Copy, Clone)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub(super) struct IslandsOptimizer {
    /// The optimizer will try to merge small islands so their size exceed this minimum.
    ///
    /// Note that it will never merge incompatible islands (currently define as islands with
    /// different additional solver iteration counts).
    pub(super) min_island_size: usize,
    /// The optimizer will try to split large islands so their size do not exceed this maximum.
    ///
    /// IMPORTANT: Must be greater than `2 * min_island_size` to avoid conflict between the splits
    ///            and merges.
    pub(super) max_island_size: usize,
    /// Indicates if the optimizer is in split or merge mode. Swaps between modes every step.
    pub(super) mode: usize,
    pub(super) merge_state: IslandsOptimizerMergeState,
    pub(super) split_state: IslandsOptimizerSplitState,
}

impl Default for IslandsOptimizer {
    fn default() -> Self {
        // TODO: figure out the best values.
        Self {
            min_island_size: 1024,
            max_island_size: 4096,
            mode: 0,
            merge_state: Default::default(),
            split_state: Default::default(),
        }
    }
}

impl IslandManager {
    pub(super) fn update_optimizer(
        &mut self,
        bodies: &mut RigidBodySet,
        colliders: &ColliderSet,
        impulse_joints: &ImpulseJointSet,
        multibody_joints: &MultibodyJointSet,
        narrow_phase: &NarrowPhase,
    ) {
        if self.optimizer.mode % 2 == 0 {
            self.incremental_merge(bodies);
        } else {
            self.incremental_split(
                bodies,
                colliders,
                impulse_joints,
                multibody_joints,
                narrow_phase,
            );
        }

        self.optimizer.mode = self.optimizer.mode.wrapping_add(1);
    }

    /// Attempts to merge awake islands that are too small.
    fn incremental_merge(&mut self, bodies: &mut RigidBodySet) {
        struct IslandData {
            id: usize,
            awake_id: usize,
            solver_iters: usize,
        }

        // Ensure the awake id is still in bounds.
        if self.optimizer.merge_state.curr_awake_id >= self.awake_islands.len() {
            self.optimizer.merge_state.curr_awake_id = 0;
        }

        // Find a first candidate for a merge.
        let mut island1 = None;
        for awake_id in self.optimizer.merge_state.curr_awake_id..self.awake_islands.len() {
            let id = self.awake_islands[awake_id];
            let island = &self.islands[id];
            if island.len() < self.optimizer.min_island_size {
                island1 = Some(IslandData {
                    awake_id,
                    id,
                    solver_iters: island.additional_solver_iterations,
                });
                break;
            }
        }

        if let Some(island1) = island1 {
            // Indicates if we found a merge candidate for the next incremental update.
            let mut found_next = false;
            self.optimizer.merge_state.curr_awake_id = island1.awake_id + 1;

            // Find a second candidate for a merge.
            for awake_id2 in island1.awake_id + 1..self.awake_islands.len() {
                let id2 = self.awake_islands[awake_id2];
                let island2 = &self.islands[id2];

                if island1.solver_iters == island2.additional_solver_iterations
                    && island2.len() < self.optimizer.min_island_size
                {
                    // Found a second candidate! Merge them.
                    self.merge_islands(bodies, island1.id, id2);

                    // TODO: support doing more than a single merge per frame.
                    return;
                } else if island2.len() < self.optimizer.min_island_size && !found_next {
                    // We found a good candidate for the next incremental merge (we can’t just
                    // merge it now because it’s not compatible with the current island).
                    self.optimizer.merge_state.curr_awake_id = awake_id2;
                    found_next = true;
                }
            }
        } else {
            self.optimizer.merge_state.curr_awake_id = 0;
        }
    }

    /// Attempts to split awake islands that are too big.
    fn incremental_split(
        &mut self,
        bodies: &mut RigidBodySet,
        colliders: &ColliderSet,
        impulse_joints: &ImpulseJointSet,
        multibody_joints: &MultibodyJointSet,
        narrow_phase: &NarrowPhase,
    ) {
        if self.optimizer.split_state.curr_awake_id >= self.awake_islands.len() {
            self.optimizer.split_state.curr_awake_id = 0;
        }

        for awake_id in self.optimizer.split_state.curr_awake_id..self.awake_islands.len() {
            let id = self.awake_islands[awake_id];
            if self.islands[id].len() > self.optimizer.max_island_size {
                // Try to split this island.
                // Note that the traversal logic is similar to the sleeping island
                // extraction, except that we have different stopping criteria.
                self.stack.clear();

                // TODO: implement islands recycling to avoid reallocating every time.
                let mut new_island = Island::default();
                self.traversal_timestamp += 1;

                for root in &self.islands[id].bodies {
                    self.stack.push(*root);

                    let len_before_traversal = new_island.len();
                    while let Some(handle) = self.stack.pop() {
                        let rb = bodies.index_mut(handle);
                        if rb.is_fixed() {
                            // Don’t propagate islands through rigid-bodies.
                            continue;
                        }

                        if rb.ids.active_set_timestamp == self.traversal_timestamp {
                            // We already visited this body.
                            continue;
                        }

                        rb.ids.active_set_timestamp = self.traversal_timestamp;
                        assert!(!rb.activation.sleeping);

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

                        // Our new island cannot grow any further.
                        if new_island.bodies.len() > self.optimizer.max_island_size {
                            new_island.bodies.truncate(len_before_traversal);
                            self.stack.clear();
                            break;
                        }
                    }

                    // Extract this island.
                    if new_island.len() == 0 {
                        // println!("Failed to split island.");
                        return;
                    } else if new_island.bodies.len() >= self.optimizer.min_island_size {
                        // println!(
                        //     "Split an island: {}/{} ({} islands)",
                        //     new_island.len(),
                        //     self.islands[id].len(),
                        //     self.awake_islands.len(),
                        // );
                        self.extract_sub_island(bodies, id, new_island, false);
                        return; // TODO: support extracting more than one island per frame.
                    }
                }
            } else {
                self.optimizer.split_state.curr_awake_id = awake_id + 1;
            }
        }
    }
}

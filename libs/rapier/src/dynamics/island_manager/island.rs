use crate::dynamics::{RigidBody, RigidBodyHandle, RigidBodySet};

use super::IslandManager;

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Default, Debug)]
pub(crate) struct Island {
    /// The rigid-bodies part of this island.
    pub(super) bodies: Vec<RigidBodyHandle>,
    /// The additional solver iterations needed by this island.
    pub(super) additional_solver_iterations: usize,
    /// Index of this island in `IslandManager::awake_islands`.
    ///
    /// If `None`, the island is sleeping.
    pub(super) id_in_awake_list: Option<usize>,
}

impl Island {
    pub fn singleton(handle: RigidBodyHandle, rb: &RigidBody) -> Self {
        Self {
            bodies: vec![handle],
            additional_solver_iterations: rb.additional_solver_iterations,
            id_in_awake_list: None,
        }
    }

    pub fn bodies(&self) -> &[RigidBodyHandle] {
        &self.bodies
    }

    pub fn additional_solver_iterations(&self) -> usize {
        self.additional_solver_iterations
    }

    pub fn is_sleeping(&self) -> bool {
        self.id_in_awake_list.is_none()
    }

    pub fn len(&self) -> usize {
        self.bodies.len()
    }

    pub(crate) fn id_in_awake_list(&self) -> Option<usize> {
        self.id_in_awake_list
    }
}

impl IslandManager {
    /// Remove from the island at `source_id` all the rigid-body that are in `new_island`, and
    /// insert `new_island` into the islands set.
    ///
    /// **All** rigid-bodies from `new_island` must currently be part of the island at `source_id`.
    pub(super) fn extract_sub_island(
        &mut self,
        bodies: &mut RigidBodySet,
        source_id: usize,
        mut new_island: Island,
        sleep: bool,
    ) {
        let new_island_id = self.free_islands.pop().unwrap_or(self.islands.len());
        let source_island = &mut self.islands[source_id];

        for (id, handle) in new_island.bodies.iter().enumerate() {
            let rb = bodies.index_mut_internal(*handle);

            // If the new island is sleeping, ensure all its bodies are sleeping.
            if sleep {
                rb.sleep();
            }

            let id_to_remove = rb.ids.active_set_id;

            assert_eq!(
                rb.ids.active_island_id, source_id,
                "note, new id: {}",
                new_island_id
            );
            rb.ids.active_island_id = new_island_id;
            rb.ids.active_set_id = id;

            new_island.additional_solver_iterations = new_island
                .additional_solver_iterations
                .max(rb.additional_solver_iterations);
            source_island.bodies.swap_remove(id_to_remove);

            if let Some(moved_handle) = source_island.bodies.get(id_to_remove).copied() {
                let moved_rb = bodies.index_mut_internal(moved_handle);
                moved_rb.ids.active_set_id = id_to_remove;
            }
        }

        // If the new island is awake, add it to the awake list.
        if !sleep {
            new_island.id_in_awake_list = Some(self.awake_islands.len());
            self.awake_islands.push(new_island_id);
        } else {
            new_island.id_in_awake_list = None;
        }

        self.islands.insert(new_island_id, new_island);
    }

    pub(super) fn merge_islands(
        &mut self,
        bodies: &mut RigidBodySet,
        island_id1: usize,
        island_id2: usize,
    ) {
        if island_id1 == island_id2 {
            return;
        }

        let island1 = &self.islands[island_id1];
        let island2 = &self.islands[island_id2];

        assert_eq!(
            island1.id_in_awake_list.is_some(),
            island2.id_in_awake_list.is_some(),
            "Internal error: cannot merge two island with different sleeping statuses."
        );

        // Prefer removing the smallest island to reduce the amount of memory to move.
        let (to_keep, to_remove) = if island1.bodies.len() < island2.bodies.len() {
            (island_id2, island_id1)
        } else {
            (island_id1, island_id2)
        };

        // println!("Merging: {} <- {}", to_keep, to_remove);

        let Some(removed_island) = self.islands.remove(to_remove) else {
            // TODO: the island doesn’t exist is that an internal error?
            return;
        };

        self.free_islands.push(to_remove);

        // TODO: if we switched to linked list, we could avoid moving around all this memory.
        let target_island = &mut self.islands[to_keep];
        for handle in &removed_island.bodies {
            let Some(rb) = bodies.get_mut_internal(*handle) else {
                // This body no longer exists.
                continue;
            };
            rb.wake_up(false);
            rb.ids.active_island_id = to_keep;
            rb.ids.active_set_id = target_island.bodies.len();
            target_island.bodies.push(*handle);
            target_island.additional_solver_iterations = target_island
                .additional_solver_iterations
                .max(rb.additional_solver_iterations);
        }

        // Update the awake_islands list.
        if let Some(awake_id_to_remove) = removed_island.id_in_awake_list {
            self.awake_islands.swap_remove(awake_id_to_remove);
            // Update the awake list index of the awake island id we moved.
            if let Some(moved_id) = self.awake_islands.get(awake_id_to_remove) {
                self.islands[*moved_id].id_in_awake_list = Some(awake_id_to_remove);
            }
        }
    }
}

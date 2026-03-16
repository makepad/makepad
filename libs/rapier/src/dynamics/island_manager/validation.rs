use crate::dynamics::{IslandManager, RigidBodySet};
use crate::geometry::NarrowPhase;
use crate::prelude::ColliderSet;

impl IslandManager {
    #[allow(dead_code)]
    pub(super) fn assert_state_is_valid(
        &self,
        bodies: &RigidBodySet,
        colliders: &ColliderSet,
        nf: &NarrowPhase,
    ) {
        for (island_id, island) in self.islands.iter() {
            // Sleeping island must not be in the awake list.
            if island.is_sleeping() {
                assert!(!self.awake_islands.contains(&island_id));
            } else {
                // If the island is awake, the awake id must match.
                let awake_id = island.id_in_awake_list.unwrap();
                assert_eq!(self.awake_islands[awake_id], island_id);
            }

            for (body_id, handle) in island.bodies.iter().enumerate() {
                if let Some(rb) = bodies.get(*handle) {
                    // The body’s sleeping status must match the island’s status.
                    assert_eq!(rb.is_sleeping(), island.is_sleeping());
                    // The body’s island id must match the island id.
                    assert_eq!(rb.ids.active_island_id, island_id);
                    // The body’s active set id must match its handle’s position in island.bodies.
                    assert_eq!(body_id, rb.ids.active_set_id);
                }
            }
        }

        // Free island ids must actually be free.
        for id in self.free_islands.iter() {
            assert!(self.islands.get(*id).is_none());
        }

        // The awake islands list must not have duplicates.
        let mut awake_islands_dedup = self.awake_islands.clone();
        awake_islands_dedup.sort();
        awake_islands_dedup.dedup();
        assert_eq!(self.awake_islands.len(), awake_islands_dedup.len());

        // If two bodies have solver contacts, they must be in the same island.
        for pair in nf.contact_pairs() {
            let Some(body_handle1) = colliders[pair.collider1].parent.map(|p| p.handle) else {
                continue;
            };
            let Some(body_handle2) = colliders[pair.collider2].parent.map(|p| p.handle) else {
                continue;
            };

            let body1 = &bodies[body_handle1];
            let body2 = &bodies[body_handle2];

            if body1.is_fixed() || body2.is_fixed() {
                continue;
            }

            if pair.has_any_active_contact() {
                assert_eq!(body1.ids.active_island_id, body2.ids.active_island_id);
            }
        }

        println!(
            "`IslandManager::assert_state_is_valid` validation checks passed. This is slow. Only enable for debugging."
        );
    }
}

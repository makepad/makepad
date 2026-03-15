use crate::dynamics::{IntegrationParameters, RigidBodySet};
use crate::geometry::{Aabb, BroadPhasePairEvent, ColliderHandle, ColliderPair, ColliderSet};
use crate::math::Real;
use parry::partitioning::{Bvh, BvhWorkspace};
use parry::utils::hashmap::{Entry, HashMap};

/// The broad-phase collision detector that quickly filters out distant object pairs.
///
/// The broad-phase is the "first pass" of collision detection. It uses a hierarchical
/// bounding volume tree (BVH) to quickly identify which collider pairs are close enough
/// to potentially collide, avoiding expensive narrow-phase checks for distant objects.
///
/// Think of it as a "spatial index" that answers: "Which objects are near each other?"
///
/// You typically don't interact with this directly - it's managed by [`PhysicsPipeline`](crate::pipeline::PhysicsPipeline).
/// However, you can use it to create a [`QueryPipeline`](crate::pipeline::QueryPipeline) for spatial queries.
#[derive(Default, Clone)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub struct BroadPhaseBvh {
    pub(crate) tree: Bvh,
    #[cfg_attr(feature = "serde-serialize", serde(skip))]
    workspace: BvhWorkspace,
    #[cfg_attr(
        feature = "serde-serialize",
        serde(
            serialize_with = "crate::utils::serde::serialize_to_vec_tuple",
            deserialize_with = "crate::utils::serde::deserialize_from_vec_tuple"
        )
    )]
    pairs: HashMap<(ColliderHandle, ColliderHandle), u32>,
    frame_index: u32,
    optimization_strategy: BvhOptimizationStrategy,
}

// TODO: would be interesting to try out:
// "Fast Insertion-Based Optimization of Bounding Volume Hierarchies"
// by Bittner et al.
/// Selection of strategies to maintain through time the broad-phase BVH in shape that remains
/// efficient for collision-detection and scene queries.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Default, PartialEq, Eq, Copy, Clone)]
pub enum BvhOptimizationStrategy {
    /// Different sub-trees of the BVH will be optimized at each frame.
    #[default]
    SubtreeOptimizer,
    /// Disables incremental BVH optimization (discouraged).
    ///
    /// This should not be used except for debugging purpose.
    None,
}

const ENABLE_TREE_VALIDITY_CHECK: bool = false;

impl BroadPhaseBvh {
    const CHANGE_DETECTION_ENABLED: bool = true;
    const CHANGE_DETECTION_FACTOR: Real = 1.0e-2;

    /// Initializes a new empty broad-phase.
    pub fn new() -> Self {
        Self::default()
    }

    /// Initializes a new empty broad-phase with the specified strategy for incremental
    /// BVH optimization.
    pub fn with_optimization_strategy(optimization_strategy: BvhOptimizationStrategy) -> Self {
        Self {
            optimization_strategy,
            ..Default::default()
        }
    }

    /// Updates the broad-phase.
    ///
    /// The results are output through the `events` struct. The broad-phase algorithm is only
    /// required to generate new events (i.e. no need to re-send an `AddPair` event if it was already
    /// sent previously and no `RemovePair` happened since then). Sending redundant events is allowed
    /// but can result in a slight computational overhead.
    ///
    /// # Parameters
    /// - `params`: the integration parameters governing the simulation.
    /// - `colliders`: the set of colliders. Change detection with `collider.needs_broad_phase_update()`
    ///   can be relied on at this stage.
    /// - `modified_colliders`: colliders that are know to be modified since the last update.
    /// - `removed_colliders`: colliders that got removed since the last update. Any associated data
    ///   in the broad-phase should be removed by this call to `update`.
    /// - `events`: the broad-phase’s output. They indicate what collision pairs need to be created
    ///   and what pairs need to be removed. It is OK to create pairs for colliders that don’t
    ///   actually collide (though this can increase computational overhead in the narrow-phase)
    ///   but it is important not to indicate removal of a collision pair if the underlying colliders
    ///   are still touching or closer than `prediction_distance`.
    pub fn update(
        &mut self,
        params: &IntegrationParameters,
        colliders: &ColliderSet,
        bodies: &RigidBodySet,
        modified_colliders: &[ColliderHandle],
        removed_colliders: &[ColliderHandle],
        events: &mut Vec<BroadPhasePairEvent>,
    ) {
        self.frame_index = self.frame_index.overflowing_add(1).0;

        // Removals must be handled first, in case another collider in
        // `modified_colliders` shares the same index.
        for handle in removed_colliders {
            self.tree.remove(handle.into_raw_parts().0);
        }

        // if modified_colliders.is_empty() {
        //     return;
        // }

        let first_pass = self.tree.is_empty();

        // let t0 = std::time::Instant::now();
        for modified in modified_colliders {
            if let Some(collider) = colliders.get(*modified) {
                if !collider.is_enabled() || !collider.changes.needs_broad_phase_update() {
                    continue;
                }

                let aabb = collider.compute_broad_phase_aabb(params, bodies);

                let change_detection_skin = if Self::CHANGE_DETECTION_ENABLED {
                    Self::CHANGE_DETECTION_FACTOR * params.length_unit
                } else {
                    0.0
                };

                self.tree.insert_or_update_partially(
                    aabb,
                    modified.into_raw_parts().0,
                    change_detection_skin,
                );
            }
        }

        if ENABLE_TREE_VALIDITY_CHECK {
            if first_pass {
                self.tree.assert_well_formed();
            }

            self.tree.assert_well_formed_topology_only();
        }

        // let t0 = std::time::Instant::now();
        match self.optimization_strategy {
            BvhOptimizationStrategy::SubtreeOptimizer => {
                self.tree.optimize_incremental(&mut self.workspace);
            }
            BvhOptimizationStrategy::None => {}
        };
        // println!(
        //     "Incremental optimization: {}",
        //     t0.elapsed().as_secs_f32() * 1000.0
        // );

        // NOTE: we run refit after optimization so we can skip updating internal nodes during
        //       optimization, and so we can reorder the tree in memory (in depth-first order)
        //       to make it more cache friendly after the rebuild shuffling everything around.
        // let t0 = std::time::Instant::now();
        self.tree.refit(&mut self.workspace);

        if ENABLE_TREE_VALIDITY_CHECK {
            self.tree.assert_well_formed();
        }

        // println!("Refit: {}", t0.elapsed().as_secs_f32() * 1000.0);
        // println!(
        //     "leaf count: {}/{} (changed: {})",
        //     self.tree.leaf_count(),
        //     self.tree.reachable_leaf_count(0),
        //     self.tree.changed_leaf_count(0),
        // );
        // self.tree.assert_is_depth_first();
        // self.tree.assert_well_formed();
        // println!(
        //     "Is well formed. Tree height: {}",
        //     self.tree.subtree_height(0),
        // );
        // // println!("Tree quality: {}", self.tree.quality_metric());

        let mut pairs_collector = |co1: u32, co2: u32| {
            assert_ne!(co1, co2);

            let Some((_, mut handle1)) = colliders.get_unknown_gen(co1) else {
                return;
            };
            let Some((_, mut handle2)) = colliders.get_unknown_gen(co2) else {
                return;
            };

            if co1 > co2 {
                std::mem::swap(&mut handle1, &mut handle2);
            }

            match self.pairs.entry((handle1, handle2)) {
                Entry::Occupied(e) => *e.into_mut() = self.frame_index,
                Entry::Vacant(e) => {
                    e.insert(self.frame_index);
                    events.push(BroadPhasePairEvent::AddPair(ColliderPair::new(
                        handle1, handle2,
                    )));
                }
            }
        };

        // let t0 = std::time::Instant::now();
        self.tree
            .traverse_bvtt_single_tree::<{ Self::CHANGE_DETECTION_ENABLED }>(
                &mut self.workspace,
                &mut pairs_collector,
            );
        // println!("Detection: {}", t0.elapsed().as_secs_f32() * 1000.0);
        // println!(">>>>>> Num events: {}", events.iter().len());

        // Find outdated entries.
        // TODO PERF:
        // Currently, the narrow-phase isn’t capable of removing its own outdated
        // collision pairs. So we need to run a pass here to find aabbs that are
        // no longer overlapping. This, and the pair deduplication happening in
        // the `pairs_collector` is expensive and should be done more efficiently
        // by the narrow-phase itself (or islands) once we rework it.
        //
        // let t0 = std::time::Instant::now();
        self.pairs.retain(|(h0, h1), timestamp| {
            if *timestamp != self.frame_index {
                if !colliders.contains(*h0) || !colliders.contains(*h1) {
                    // At least one of the colliders no longer exist, don’t retain the pair.
                    return false;
                }

                let Some(node0) = self.tree.leaf_node(h0.into_raw_parts().0) else {
                    return false;
                };
                let Some(node1) = self.tree.leaf_node(h1.into_raw_parts().0) else {
                    return false;
                };

                if (!Self::CHANGE_DETECTION_ENABLED || node0.is_changed() || node1.is_changed())
                    && !node0.intersects(node1)
                {
                    events.push(BroadPhasePairEvent::DeletePair(ColliderPair::new(*h0, *h1)));
                    false
                } else {
                    true
                }
            } else {
                // If the timestamps match, we already saw this pair during traversal.
                // There can be rare occurrences where the timestamp will be equal
                // even though we didn’t see the pair during traversal. This happens
                // if the frame index overflowed. But this is fine, we’ll catch it
                // in another frame.
                true
            }
        });

        // println!(
        //     "Post-filtering: {} (added pairs: {}, removed pairs: {})",
        //     t0.elapsed().as_secs_f32() * 1000.0,
        //     added_pairs,
        //     removed_pairs
        // );
    }

    /// Sets the AABB associated to the given collider.
    ///
    /// The AABB change will be immediately applied and propagated through the underlying BVH.
    /// Change detection will automatically take it into account during the next broad-phase update.
    pub fn set_aabb(&mut self, params: &IntegrationParameters, handle: ColliderHandle, aabb: Aabb) {
        let change_detection_skin = if Self::CHANGE_DETECTION_ENABLED {
            Self::CHANGE_DETECTION_FACTOR * params.length_unit
        } else {
            0.0
        };
        self.tree.insert_with_change_detection(
            aabb,
            handle.into_raw_parts().0,
            change_detection_skin,
        );
    }
}

use super::bvh_tree::{BvhNodeIndex, BvhNodeWide};
use super::BvhNode;
use crate::bounding_volume::{Aabb, BoundingVolume};
use crate::math::Real;
use crate::partitioning::Bvh;
use crate::utils::morton;
use alloc::{vec, vec::Vec};

impl Bvh {
    pub(crate) fn rebuild_range_ploc(&mut self, target_node_id: u32, leaves: &mut Vec<BvhNode>) {
        // Compute the centroids aabb.
        let aabb = Aabb::from_points(leaves.iter().map(|l| l.center()));
        let inv_extents = aabb.extents().map(|e| 1.0 / e);

        // Sort the leaves.
        leaves.sort_by_cached_key(|node| {
            let center = (node.center() - aabb.mins) * (inv_extents);
            morton::morton_encode_u64_unorm(center)
        });

        // Build all the levels.
        const SEARCH_RADIUS: usize = 16;
        let mut merge_candidates = vec![usize::MAX; leaves.len()];
        let mut next_leaves = Vec::with_capacity(leaves.len());

        while leaves.len() > 1 {
            // Find merge candidates.
            for i in 0..leaves.len() {
                let mut best_sah = Real::MAX;
                let mut best_candidate = usize::MAX;
                for k in i.saturating_sub(SEARCH_RADIUS)..=(i + SEARCH_RADIUS).min(leaves.len() - 1)
                {
                    if k != i {
                        let node_i = &leaves[i];
                        let node_k = &leaves[k];
                        let sah = node_i
                            .aabb()
                            .merged(&node_k.aabb())
                            .half_area_or_perimeter();
                        if sah < best_sah {
                            best_sah = sah;
                            best_candidate = k;
                        }
                    }
                }
                merge_candidates[i] = best_candidate;
            }

            // Group nodes with matching merge candidates.
            for i in 0..leaves.len() {
                let k = merge_candidates[i];
                if merge_candidates[k] == i {
                    if i > k {
                        continue;
                    }

                    // Merge nodes k and i:
                    let left = leaves[i];
                    let right = leaves[k];
                    let wide_node = BvhNodeWide { left, right };

                    let id = if leaves.len() == 2 {
                        self.nodes[target_node_id as usize] = wide_node;
                        target_node_id
                    } else {
                        let id = self.nodes.len() as u32;
                        let parent = wide_node.merged(id);
                        self.nodes.push(wide_node);
                        self.parents.push(BvhNodeIndex::default()); // Will be set when the parent is created.
                        next_leaves.push(parent);
                        id
                    };

                    if left.is_leaf() {
                        self.leaf_node_indices[left.children as usize] = BvhNodeIndex::left(id);
                    } else {
                        self.parents[left.children as usize] = BvhNodeIndex::left(id);
                    }
                    if right.is_leaf() {
                        self.leaf_node_indices[right.children as usize] = BvhNodeIndex::right(id);
                    } else {
                        self.parents[right.children as usize] = BvhNodeIndex::right(id);
                    }
                } else {
                    next_leaves.push(leaves[i]);
                }
            }

            // Swap for next step.
            core::mem::swap(leaves, &mut next_leaves);
            next_leaves.clear();
        }
    }
}

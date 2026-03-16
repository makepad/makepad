use super::bvh_tree::{BvhNodeIndex, BvhNodeVec, BvhNodeWide};
use super::{Bvh, BvhNode, BvhWorkspace};
use crate::utils::VecMap;
use alloc::vec::Vec;

impl Bvh {
    /// Updates the BVH's internal node AABBs after leaf changes.
    ///
    /// Refitting ensures that every internal node's AABB tightly encloses the AABBs of its
    /// children. This operation is essential after updating leaf positions with
    /// [`insert_or_update_partially`] and is much faster than rebuilding the entire tree.
    ///
    /// In addition to updating AABBs, this method:
    /// - Reorders nodes in depth-first order for better cache locality during queries
    /// - Ensures leaf counts on each node are correct
    /// - Propagates change flags from leaves to ancestors (for change detection)
    ///
    /// # When to Use
    ///
    /// Call `refit` after:
    /// - Bulk updates with [`insert_or_update_partially`]
    /// - Any operation that modifies leaf AABBs without updating ancestor nodes
    /// - When you want to optimize tree layout for better query performance
    ///
    /// **Don't call `refit` after**:
    /// - Regular [`insert`] calls (they already update ancestors)
    /// - [`remove`] calls (they already maintain tree validity)
    ///
    /// # Arguments
    ///
    /// * `workspace` - A reusable workspace to avoid allocations. Can be shared across
    ///   multiple BVH operations for better performance.
    ///
    /// # Performance
    ///
    /// - **Time**: O(n) where n is the number of nodes
    /// - **Space**: O(n) temporary storage in workspace
    /// - Much faster than rebuilding the tree from scratch
    /// - Essential for maintaining good query performance in dynamic scenes
    ///
    /// # Examples
    ///
    /// ## After bulk updates
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhWorkspace};
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let mut bvh = Bvh::new();
    /// let mut workspace = BvhWorkspace::default();
    ///
    /// // Insert initial objects
    /// for i in 0..100 {
    ///     let aabb = Aabb::new(
    ///         Vector::new(i as f32, 0.0, 0.0),
    ///         Vector::new(i as f32 + 1.0, 1.0, 1.0)
    ///     );
    ///     bvh.insert(aabb, i);
    /// }
    ///
    /// // Update all objects without tree propagation (faster)
    /// for i in 0..100 {
    ///     let offset = 0.1;
    ///     let aabb = Aabb::new(
    ///         Vector::new(i as f32 + offset, 0.0, 0.0),
    ///         Vector::new(i as f32 + 1.0 + offset, 1.0, 1.0)
    ///     );
    ///     bvh.insert_or_update_partially(aabb, i, 0.0);
    /// }
    ///
    /// // Now update the tree in one efficient pass
    /// bvh.refit(&mut workspace);
    /// # }
    /// ```
    ///
    /// ## In a game loop
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhWorkspace};
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let mut bvh = Bvh::new();
    /// let mut workspace = BvhWorkspace::default();
    ///
    /// // Game initialization - add objects
    /// for i in 0..1000 {
    ///     let aabb = Aabb::new(
    ///         Vector::new(i as f32, 0.0, 0.0),
    ///         Vector::new(i as f32 + 1.0, 1.0, 1.0)
    ///     );
    ///     bvh.insert(aabb, i);
    /// }
    ///
    /// // Game loop - update objects each frame
    /// for frame in 0..100 {
    ///     // Update physics, AI, etc.
    ///     for i in 0..1000 {
    ///         let time = frame as f32 * 0.016; // ~60 FPS
    ///         let pos = time.sin() * 10.0;
    ///         let aabb = Aabb::new(
    ///             Vector::new(i as f32 + pos, 0.0, 0.0),
    ///             Vector::new(i as f32 + pos + 1.0, 1.0, 1.0)
    ///         );
    ///         bvh.insert_or_update_partially(aabb, i, 0.0);
    ///     }
    ///
    ///     // Refit once per frame for all updates
    ///     bvh.refit(&mut workspace);
    ///
    ///     // Now perform collision detection queries...
    /// }
    /// # }
    /// ```
    ///
    /// ## With change detection margin
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhWorkspace};
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let mut bvh = Bvh::new();
    /// let mut workspace = BvhWorkspace::default();
    ///
    /// // Add an object
    /// let aabb = Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0));
    /// bvh.insert(aabb, 0);
    ///
    /// // Update with a margin - tree won't update if movement is small
    /// let margin = 0.5;
    /// let new_aabb = Aabb::new(Vector::new(0.1, 0.0, 0.0), Vector::new(1.1, 1.0, 1.0));
    /// bvh.insert_or_update_partially(new_aabb, 0, margin);
    ///
    /// // Refit propagates the change detection flags
    /// bvh.refit(&mut workspace);
    /// # }
    /// ```
    ///
    /// # Comparison with `refit_without_opt`
    ///
    /// This method reorganizes the tree in memory for better cache performance.
    /// If you only need to update AABBs without reordering, use [`refit_without_opt`](Self::refit_without_opt)
    /// which is faster but doesn't improve memory layout.
    ///
    /// # Notes
    ///
    /// - Reuses the provided `workspace` to avoid allocations
    /// - Safe to call even if no leaves were modified (just reorganizes tree)
    /// - Does not change the tree's topology, only AABBs and layout
    /// - Call this before [`optimize_incremental`] for best results
    ///
    /// # See Also
    ///
    /// - [`insert_or_update_partially`](Bvh::insert_or_update_partially) - Update leaves
    ///   without propagation
    /// - [`refit_without_opt`](Self::refit_without_opt) - Faster refit without memory
    ///   reorganization
    /// - [`optimize_incremental`](Bvh::optimize_incremental) - Improve tree quality
    /// - [`BvhWorkspace`] - Reusable workspace for operations
    ///
    /// [`insert_or_update_partially`]: Bvh::insert_or_update_partially
    /// [`insert`]: Bvh::insert
    /// [`remove`]: Bvh::remove
    /// [`optimize_incremental`]: Bvh::optimize_incremental
    pub fn refit(&mut self, workspace: &mut BvhWorkspace) {
        Self::refit_buffers(
            &mut self.nodes,
            &mut workspace.refit_tmp,
            &mut self.leaf_node_indices,
            &mut self.parents,
        );

        // Swap the old nodes with the refitted ones.
        core::mem::swap(&mut self.nodes, &mut workspace.refit_tmp);
    }

    pub(super) fn refit_buffers(
        source: &mut BvhNodeVec,
        target: &mut BvhNodeVec,
        leaf_data: &mut VecMap<BvhNodeIndex>,
        parents: &mut Vec<BvhNodeIndex>,
    ) {
        if source.is_empty() {
            target.clear();
            parents.clear();
        } else if source[0].leaf_count() <= 2 {
            // No actual refit to apply, just copy the root wide node.
            target.clear();
            parents.clear();
            target.push(source[0]);
            target[0].left.data.resolve_pending_change();
            if target[0].right.leaf_count() > 0 {
                target[0].right.data.resolve_pending_change();
            }
            parents.push(BvhNodeIndex::default());
        } else if !source.is_empty() && source[0].leaf_count() > 2 {
            target.resize(
                source.len(),
                BvhNodeWide {
                    left: BvhNode::zeros(),
                    right: BvhNode::zeros(),
                },
            );

            let mut len = 1;

            // Start with a special case for the root then recurse.
            let left_child_id = source[0].left.children;
            let right_child_id = source[0].right.children;

            if !source[0].left.is_leaf() {
                Self::refit_recurse(
                    source,
                    target,
                    leaf_data,
                    parents,
                    left_child_id,
                    &mut len,
                    BvhNodeIndex::left(0),
                );
            } else {
                target[0].left = source[0].left;
                target[0].left.data.resolve_pending_change();

                // NOTE: updating the leaf_data shouldn’t be needed here since the root
                //       is always at 0.
                // *self.leaf_data.get_mut_unknown_gen(left_child_id).unwrap() = BvhNodeIndex::left(0);
            }

            if !source[0].right.is_leaf() {
                Self::refit_recurse(
                    source,
                    target,
                    leaf_data,
                    parents,
                    right_child_id,
                    &mut len,
                    BvhNodeIndex::right(0),
                );
            } else {
                target[0].right = source[0].right;
                target[0].right.data.resolve_pending_change();
                // NOTE: updating the leaf_data shouldn’t be needed here since the root
                //       is always at 0.
                // *self.leaf_data.get_mut_unknown_gen(right_child_id).unwrap() = BvhNodeIndex::right(0);
            }

            source.truncate(len as usize);
            target.truncate(len as usize);
            parents.truncate(len as usize);
        }
    }

    fn refit_recurse(
        source: &BvhNodeVec,
        target: &mut BvhNodeVec,
        leaf_data: &mut VecMap<BvhNodeIndex>,
        parents: &mut [BvhNodeIndex],
        source_id: u32,
        target_id_mut: &mut u32,
        parent: BvhNodeIndex,
    ) {
        let target_id = *target_id_mut;
        *target_id_mut += 1;

        let node = &source[source_id as usize];
        let left_is_leaf = node.left.is_leaf();
        let right_is_leaf = node.right.is_leaf();
        let left_source_id = node.left.children;
        let right_source_id = node.right.children;

        if !left_is_leaf {
            Self::refit_recurse(
                source,
                target,
                leaf_data,
                parents,
                left_source_id,
                target_id_mut,
                BvhNodeIndex::left(target_id),
            );
        } else {
            let node = &source[source_id as usize];
            target[target_id as usize].left = node.left;
            target[target_id as usize]
                .left
                .data
                .resolve_pending_change();
            leaf_data[node.left.children as usize] = BvhNodeIndex::left(target_id);
        }

        if !right_is_leaf {
            Self::refit_recurse(
                source,
                target,
                leaf_data,
                parents,
                right_source_id,
                target_id_mut,
                BvhNodeIndex::right(target_id),
            );
        } else {
            let node = &source[source_id as usize];
            target[target_id as usize].right = node.right;
            target[target_id as usize]
                .right
                .data
                .resolve_pending_change();
            leaf_data[node.right.children as usize] = BvhNodeIndex::right(target_id);
        }

        let node = &target[target_id as usize];
        target[parent] = node.left.merged(&node.right, target_id);
        parents[target_id as usize] = parent;
    }

    /// Similar to [`Self::refit`] but without any optimization of the internal node storage layout.
    ///
    /// This can be faster than [`Self::refit`] but doesn’t reorder node to be more cache-efficient
    /// on tree traversals.
    pub fn refit_without_opt(&mut self) {
        if self.leaf_count() > 2 {
            let root = &self.nodes[0];
            let left = root.left.children;
            let right = root.right.children;
            let left_is_leaf = root.left.is_leaf();
            let right_is_leaf = root.right.is_leaf();

            if !left_is_leaf {
                self.recurse_refit_without_opt(left, BvhNodeIndex::left(0));
            }
            if !right_is_leaf {
                self.recurse_refit_without_opt(right, BvhNodeIndex::right(0));
            }
        }
    }

    fn recurse_refit_without_opt(&mut self, node_id: u32, parent: BvhNodeIndex) {
        let node = &self.nodes[node_id as usize];
        let left = &node.left;
        let right = &node.right;
        let left_is_leaf = left.is_leaf();
        let right_is_leaf = right.is_leaf();
        let left_children = left.children;
        let right_children = right.children;

        if !left_is_leaf {
            self.recurse_refit_without_opt(left_children, BvhNodeIndex::left(node_id));
        } else {
            self.nodes[node_id as usize]
                .left
                .data
                .resolve_pending_change();
        }
        if !right_is_leaf {
            self.recurse_refit_without_opt(right_children, BvhNodeIndex::right(node_id));
        } else {
            self.nodes[node_id as usize]
                .right
                .data
                .resolve_pending_change();
        }

        let node = &self.nodes[node_id as usize];
        let left = &node.left;
        let right = &node.right;
        let merged = left.merged(right, node_id);

        self.nodes[parent] = merged;
    }
}

use super::bvh_tree::{BvhNodeIndex, BvhNodeWide};
use super::BvhNode;
use crate::bounding_volume::{Aabb, BoundingVolume};
use crate::math::{Real, Vector};
use crate::partitioning::Bvh;
use alloc::vec;

impl Bvh {
    /// Inserts a new leaf into the BVH or updates an existing one.
    ///
    /// If a leaf with the given `leaf_index` already exists in the tree, its AABB is updated
    /// to the new value and the tree's internal nodes are adjusted to maintain correctness.
    /// If the leaf doesn't exist, it's inserted into an optimal position based on the
    /// Surface Area Heuristic (SAH).
    ///
    /// This operation automatically propagates AABB changes up the tree to maintain the
    /// invariant that each internal node's AABB encloses all its descendants. For better
    /// performance when updating many leaves, consider using [`insert_or_update_partially`]
    /// followed by a single [`refit`] call.
    ///
    /// # Arguments
    ///
    /// * `aabb` - The Axis-Aligned Bounding Box for this leaf
    /// * `leaf_index` - A unique identifier for this leaf (typically an object ID)
    ///
    /// # Performance
    ///
    /// - **Insert new leaf**: O(log n) average, O(n) worst case
    /// - **Update existing leaf**: O(log n) for propagation up the tree
    ///
    /// # Examples
    ///
    /// ## Adding new objects
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::Bvh;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let mut bvh = Bvh::new();
    ///
    /// // Insert objects with custom IDs
    /// bvh.insert(Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)), 100);
    /// bvh.insert(Aabb::new(Vector::new(5.0, 0.0, 0.0), Vector::new(6.0, 1.0, 1.0)), 200);
    /// bvh.insert(Aabb::new(Vector::new(10.0, 0.0, 0.0), Vector::new(11.0, 1.0, 1.0)), 300);
    ///
    /// assert_eq!(bvh.leaf_count(), 3);
    /// # }
    /// ```
    ///
    /// ## Updating object positions
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::Bvh;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let mut bvh = Bvh::new();
    ///
    /// // Insert an object
    /// bvh.insert(Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)), 42);
    ///
    /// // Simulate the object moving - just insert with the same ID
    /// bvh.insert(Aabb::new(Vector::new(5.0, 0.0, 0.0), Vector::new(6.0, 1.0, 1.0)), 42);
    ///
    /// // The BVH still has only 1 leaf, but at the new position
    /// assert_eq!(bvh.leaf_count(), 1);
    /// # }
    /// ```
    ///
    /// ## Bulk updates with better performance
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
    /// // Add initial objects
    /// for i in 0..100 {
    ///     let aabb = Aabb::new(
    ///         Vector::new(i as f32, 0.0, 0.0),
    ///         Vector::new(i as f32 + 1.0, 1.0, 1.0)
    ///     );
    ///     bvh.insert(aabb, i);
    /// }
    ///
    /// // For better performance on bulk updates, use insert_or_update_partially
    /// // then refit once at the end
    /// for i in 0..100 {
    ///     let aabb = Aabb::new(
    ///         Vector::new(i as f32 + 0.1, 0.0, 0.0),
    ///         Vector::new(i as f32 + 1.1, 1.0, 1.0)
    ///     );
    ///     bvh.insert_or_update_partially(aabb, i, 0.0);
    /// }
    /// bvh.refit(&mut workspace); // Update tree in one pass
    /// # }
    /// ```
    ///
    /// # Notes
    ///
    /// - Leaf indices can be any `u32` value - they don't need to be contiguous
    /// - The same leaf index can only exist once in the tree
    /// - For dynamic scenes, call this every frame for moving objects
    /// - Consider calling [`refit`] or [`optimize_incremental`] periodically for best
    ///   query performance
    ///
    /// # See Also
    ///
    /// - [`insert_with_change_detection`](Self::insert_with_change_detection) - Insert with
    ///   margin for motion prediction
    /// - [`insert_or_update_partially`](Self::insert_or_update_partially) - Insert without
    ///   propagation (faster for bulk updates)
    /// - [`remove`](Self::remove) - Remove a leaf from the tree
    /// - [`refit`](Self::refit) - Update tree after bulk modifications
    ///
    /// [`insert_or_update_partially`]: Self::insert_or_update_partially
    /// [`refit`]: Self::refit
    /// [`optimize_incremental`]: Self::optimize_incremental
    pub fn insert(&mut self, aabb: Aabb, leaf_index: u32) {
        self.insert_with_change_detection(aabb, leaf_index, 0.0)
    }

    /// Inserts a leaf into this BVH, or updates it if already exists.
    ///
    /// If the `aabb` is already contained by the existing leaf node AABB, nothing is modified.
    /// Otherwise, the aabb being effectively inserted is equal to `aabb` enlarged by the
    /// `change_detection_margin`.
    pub fn insert_with_change_detection(
        &mut self,
        aabb: Aabb,
        leaf_index: u32,
        change_detection_margin: Real,
    ) {
        if let Some(leaf) = self.leaf_node_indices.get(leaf_index as usize) {
            let node = &mut self.nodes[*leaf];

            if change_detection_margin > 0.0 {
                if !node.contains_aabb(&aabb) {
                    node.mins = aabb.mins - Vector::splat(change_detection_margin);
                    node.maxs = aabb.maxs + Vector::splat(change_detection_margin);
                    node.data.set_change_pending();
                } else {
                    // No change detected, no propagation needed.
                    return;
                }
            } else {
                node.mins = aabb.mins;
                node.maxs = aabb.maxs;
            }

            // Propagate up.
            // TODO: maybe we should offer multiple propagation strategy.
            //       The one we currently implement simply stops as soon as a
            //       parent node contains the given `aabb`, but it won’t try
            //       to make the parent AABBs smaller even if we could.
            //       There could be two additional strategies that are slower but would leave the
            //       tree in a tighter state:
            //       - Make the parent smaller if possible by merging the aabb with
            //         the sibling.
            //       - In addition to merging with the sibling, we could apply bottom-up
            //         tree rotations to optimize part of the tree on our way up to the
            //         root.
            let wide_node_id = leaf.decompose().0;
            if wide_node_id == 0 {
                // Already at the root, no propagation possible.
                return;
            }

            let mut parent = self.parents[wide_node_id];
            loop {
                let node = &mut self.nodes[parent];
                if node.contains_aabb(&aabb) {
                    // No more propagation needed, the parent is big enough.
                    break;
                }

                node.mins = node.mins.min(aabb.mins);
                node.maxs = node.maxs.max(aabb.maxs);

                let wide_node_id = parent.decompose().0;
                if wide_node_id == 0 {
                    break;
                }

                parent = self.parents[wide_node_id];
            }
        } else {
            self.insert_new_unchecked(aabb, leaf_index);
        }
    }

    /// Either inserts a node on this tree, or, if it already exists, updates its associated bounding
    /// but doesn’t update its ascendant nodes.
    ///
    /// This method is primarily designed to be called for inserting new nodes or updating existing
    /// ones, and then running a [`Bvh::refit`]. Until [`Bvh::refit`] or [`Bvh::refit_without_opt`]
    /// is called, the BVH will effectively be left in an invalid state where some internal nodes
    /// might no longer enclose their children.
    ///
    /// For an alternative that inserts a node while also making sure all its ascendants are
    /// up to date, see [`Bvh::insert`].
    pub fn insert_or_update_partially(
        &mut self,
        aabb: Aabb,
        leaf_index: u32,
        change_detection_margin: Real,
    ) {
        if let Some(leaf) = self.leaf_node_indices.get(leaf_index as usize) {
            let node = &mut self.nodes[*leaf];

            if change_detection_margin > 0.0 {
                if !node.contains_aabb(&aabb) {
                    node.mins = aabb.mins - Vector::splat(change_detection_margin);
                    node.maxs = aabb.maxs + Vector::splat(change_detection_margin);
                    node.data.set_change_pending();
                }
            } else {
                node.mins = aabb.mins;
                node.maxs = aabb.maxs;
            }
        } else {
            self.insert_new_unchecked(aabb, leaf_index);
        }
    }

    /// Inserts a new leaf into this BVH without checking if it already exists.
    fn insert_new_unchecked(&mut self, aabb: Aabb, leaf_index: u32) {
        let _ = self
            .leaf_node_indices
            .insert(leaf_index as usize, BvhNodeIndex::default());
        let leaf_index_mut = &mut self.leaf_node_indices[leaf_index as usize];

        // If the tree is empty, create the root.
        if self.nodes.is_empty() {
            self.nodes.push(BvhNodeWide {
                left: BvhNode::leaf(aabb, leaf_index),
                right: BvhNode::zeros(),
            });
            self.parents.push(BvhNodeIndex::default());
            *leaf_index_mut = BvhNodeIndex::left(0);
            return;
        }

        // If we have a root, but it is partial, just complete it.
        if self.nodes[0].right.leaf_count() == 0 {
            self.nodes[0].right = BvhNode::leaf(aabb, leaf_index);
            *leaf_index_mut = BvhNodeIndex::right(0);
            return;
        }

        // General case: traverse the tree to find room for the new leaf.
        let mut curr_id = 0u32;
        let mut path_taken = vec![];

        const APPLY_ROTATIONS_DOWN: bool = true;
        const APPLY_ROTATIONS_UP: bool = false;

        loop {
            if APPLY_ROTATIONS_UP {
                path_taken.push(curr_id);
            }

            if APPLY_ROTATIONS_DOWN {
                self.maybe_apply_rotation(curr_id);
            }

            let curr_node = &self.nodes[curr_id as usize];

            // Need to determine the best side to insert our node.
            let left = &curr_node.left;
            let right = &curr_node.right;

            let left_merged_aabb = left.aabb().merged(&aabb);
            let right_merged_aabb = right.aabb().merged(&aabb);

            let left_merged_vol = left_merged_aabb.volume();
            let right_merged_vol = right_merged_aabb.volume();
            let left_vol = left.aabb().volume();
            let right_vol = right.aabb().volume();
            let left_count = left.leaf_count();
            let right_count = right.leaf_count();

            // NOTE: when calculating the SAH cost, we don’t care about dividing by the
            //       parent’s volume since both compared costs use the same factor so
            //       ignoring it doesn’t affect the comparison.
            let left_cost =
                left_merged_vol * (left_count + 1) as Real + right_vol * right_count as Real;
            let right_cost =
                right_merged_vol * (right_count + 1) as Real + left_vol * left_count as Real;

            // Insert into the branch with lowest post-insertion SAH cost.
            // If the costs are equal, just pick the branch with the smallest leaf count.
            if left_cost < right_cost || (left_cost == right_cost && left_count < right_count) {
                // Insert left. The `left` node will become an internal node.
                // We create a new wide leaf containing the current and new leaves and
                // attach it to `left`.
                if left.is_leaf() {
                    let new_leaf_id = self.nodes.len();
                    let wide_node = BvhNodeWide {
                        left: *left,
                        right: BvhNode::leaf(aabb, leaf_index),
                    };
                    self.nodes.push(wide_node);
                    self.parents.push(BvhNodeIndex::left(curr_id));

                    let left = &mut self.nodes[curr_id as usize].left;
                    self.leaf_node_indices[left.children as usize] =
                        BvhNodeIndex::left(new_leaf_id as u32);
                    self.leaf_node_indices[leaf_index as usize] =
                        BvhNodeIndex::right(new_leaf_id as u32);

                    left.children = new_leaf_id as u32;
                    left.data.add_leaf_count(1);
                    left.mins = left.mins.min(aabb.mins);
                    left.maxs = left.maxs.max(aabb.maxs);
                    break;
                } else {
                    let left = &mut self.nodes[curr_id as usize].left;
                    curr_id = left.children;
                    left.data.add_leaf_count(1);
                    left.mins = left.mins.min(aabb.mins);
                    left.maxs = left.maxs.max(aabb.maxs);
                }
            } else {
                // Insert right. The `right` node will become an internal node.
                // We create a new wide leaf containing the current and new leaves and
                // attach it to `right`.
                if right.is_leaf() {
                    let new_leaf_id = self.nodes.len();
                    let new_node = BvhNodeWide {
                        left: BvhNode::leaf(aabb, leaf_index),
                        right: *right,
                    };
                    self.nodes.push(new_node);
                    self.parents.push(BvhNodeIndex::right(curr_id));

                    let right = &mut self.nodes[curr_id as usize].right;
                    self.leaf_node_indices[leaf_index as usize] =
                        BvhNodeIndex::left(new_leaf_id as u32);
                    self.leaf_node_indices[right.children as usize] =
                        BvhNodeIndex::right(new_leaf_id as u32);

                    right.children = new_leaf_id as u32;
                    right.data.add_leaf_count(1);
                    right.mins = right.mins.min(aabb.mins);
                    right.maxs = right.maxs.max(aabb.maxs);
                    break;
                } else {
                    let right = &mut self.nodes[curr_id as usize].right;
                    curr_id = right.children;
                    right.data.add_leaf_count(1);
                    right.mins = right.mins.min(aabb.mins);
                    right.maxs = right.maxs.max(aabb.maxs);
                }
            }
        }

        if APPLY_ROTATIONS_UP {
            while let Some(node) = path_taken.pop() {
                self.maybe_apply_rotation(node);
            }
        }
    }

    // Applies a tree rotation at the given `node` if this improves the SAH metric at that node.
    fn maybe_apply_rotation(&mut self, node_id: u32) {
        let node = self.nodes[node_id as usize];
        let left = &node.left;
        let right = &node.right;

        let curr_score =
            left.volume() * left.leaf_count() as Real + right.volume() * right.leaf_count() as Real;

        macro_rules! eval_costs {
            ($left: ident, $right: ident) => {
                if !$left.is_leaf() {
                    let children = self.nodes[$left.children as usize];
                    let left_child = &children.left;
                    let right_child = &children.right;

                    // New SAH score after transforming [{left_child, right_child}, right]
                    // into [left_child, {right_child, right}].
                    let new_score1 = left_child.volume() * left_child.leaf_count() as Real
                        + right_child.merged_volume($right)
                            * (right_child.leaf_count() + $right.leaf_count()) as Real;

                    // New SAH score after transforming [{left_child, right_child}, right]
                    // into [right_child, {left_child, right}].
                    let new_score2 = right_child.volume() * right_child.leaf_count() as Real
                        + left_child.merged_volume($right)
                            * (left_child.leaf_count() + $right.leaf_count()) as Real;

                    if new_score1 < new_score2 {
                        (new_score1 - curr_score, true)
                    } else {
                        (new_score2 - curr_score, false)
                    }
                } else {
                    (Real::MAX, false)
                }
            };
        }

        // Because of the rotation some leaves might have changed location.
        // This a helper to update the `leaf_data` map accordingly.
        macro_rules! set_leaf_data {
            ($leaf_data_id: ident, $node_id: ident, $left_or_right: expr) => {
                self.leaf_node_indices[$leaf_data_id as usize] =
                    BvhNodeIndex::new($node_id, $left_or_right);
            };
        }

        // For right rotation.
        let (rotation_score0, left_child_moves_up0) = eval_costs!(left, right);
        // For left rotation.
        let (rotation_score1, left_child_moves_up1) = eval_costs!(right, left);

        if rotation_score0 < 0.0 || rotation_score1 < 0.0 {
            // At least one of the rotations is worth it, apply the one with
            // the best impact on SAH scoring.
            if rotation_score0 < rotation_score1 {
                // Apply RIGHT rotation.
                let children_id = left.children;
                let children = self.nodes[children_id as usize];
                let left_child = &children.left;
                let right_child = &children.right;

                let right_is_leaf = right.is_leaf();
                let left_child_is_leaf = left_child.is_leaf();
                let right_child_is_leaf = right_child.is_leaf();

                let right_leaf_data = right.children;
                let left_child_leaf_data = left_child.children;
                let right_child_leaf_data = right_child.children;

                self.parents[children_id as usize] = BvhNodeIndex::right(node_id);

                if left_child_moves_up0 {
                    // The left child moves into `left`, and `right` takes it place.
                    self.nodes[node_id as usize].left = *left_child;
                    self.nodes[children_id as usize].left = *right;
                    self.nodes[node_id as usize].right =
                        self.nodes[children_id as usize].merged(children_id);

                    if left_child_is_leaf {
                        self.leaf_node_indices[left_child_leaf_data as usize] =
                            BvhNodeIndex::left(node_id);
                    } else {
                        self.parents[left_child_leaf_data as usize] = BvhNodeIndex::left(node_id);
                    }
                    if right_is_leaf {
                        self.leaf_node_indices[right_leaf_data as usize] =
                            BvhNodeIndex::left(children_id);
                    } else {
                        self.parents[right_leaf_data as usize] = BvhNodeIndex::left(children_id);
                    }
                } else {
                    // The right child moves into `left`, and `right` takes it place.
                    self.nodes[node_id as usize].left = *right_child;
                    self.nodes[children_id as usize].right = *right;
                    self.nodes[node_id as usize].right =
                        self.nodes[children_id as usize].merged(children_id);
                    if right_child_is_leaf {
                        self.leaf_node_indices[right_child_leaf_data as usize] =
                            BvhNodeIndex::left(node_id);
                    } else {
                        self.parents[right_child_leaf_data as usize] = BvhNodeIndex::left(node_id);
                    }
                    if right_is_leaf {
                        self.leaf_node_indices[right_leaf_data as usize] =
                            BvhNodeIndex::right(children_id);
                    } else {
                        self.parents[right_leaf_data as usize] = BvhNodeIndex::right(children_id);
                    }
                }
            } else {
                // Apply LEFT rotation.
                let children_id = right.children;
                let children = self.nodes[children_id as usize];
                let left_child = &children.left;
                let right_child = &children.right;

                let left_is_leaf = left.is_leaf();
                let left_child_is_leaf = left_child.is_leaf();
                let right_child_is_leaf = right_child.is_leaf();

                let left_leaf_data = left.children;
                let left_child_leaf_data = left_child.children;
                let right_child_leaf_data = right_child.children;

                self.parents[children_id as usize] = BvhNodeIndex::left(node_id);

                if left_child_moves_up1 {
                    // The left child moves into `right`, and `left` takes it place.
                    self.nodes[node_id as usize].right = *left_child;
                    self.nodes[children_id as usize].left = *left;
                    self.nodes[node_id as usize].left =
                        self.nodes[children_id as usize].merged(children_id);
                    if left_child_is_leaf {
                        self.leaf_node_indices[left_child_leaf_data as usize] =
                            BvhNodeIndex::right(node_id);
                    } else {
                        self.parents[left_child_leaf_data as usize] = BvhNodeIndex::right(node_id);
                    }
                    if left_is_leaf {
                        self.leaf_node_indices[left_leaf_data as usize] =
                            BvhNodeIndex::left(children_id);
                    } else {
                        self.parents[left_leaf_data as usize] = BvhNodeIndex::left(children_id);
                    }
                } else {
                    // The right child moves into `right`, and `left` takes it place.
                    self.nodes[node_id as usize].right = *right_child;
                    self.nodes[children_id as usize].right = *left;
                    self.nodes[node_id as usize].left =
                        self.nodes[children_id as usize].merged(children_id);
                    if right_child_is_leaf {
                        set_leaf_data!(right_child_leaf_data, node_id, BvhNodeIndex::RIGHT);
                    } else {
                        self.parents[right_child_leaf_data as usize] = BvhNodeIndex::right(node_id);
                    }
                    if left_is_leaf {
                        set_leaf_data!(left_leaf_data, children_id, BvhNodeIndex::RIGHT);
                    } else {
                        self.parents[left_leaf_data as usize] = BvhNodeIndex::right(children_id);
                    }
                }
            }
        }
    }
}

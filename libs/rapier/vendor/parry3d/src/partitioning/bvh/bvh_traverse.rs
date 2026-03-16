use super::BvhNode;
use crate::math::Real;
use crate::partitioning::Bvh;
use smallvec::SmallVec;

const TRAVERSAL_STACK_SIZE: usize = 32;

pub struct Leaves<'a, Check: Fn(&BvhNode) -> bool> {
    tree: &'a Bvh,
    next: Option<&'a BvhNode>,
    stack: SmallVec<[&'a BvhNode; TRAVERSAL_STACK_SIZE]>,
    check: Check,
}

impl<'a, Check: Fn(&BvhNode) -> bool> Leaves<'a, Check> {
    pub fn new(tree: &'a Bvh, check: Check) -> Leaves<'a, Check> {
        let mut stack = SmallVec::default();
        let mut next = None;

        if let Some(root) = tree.nodes.first() {
            if check(&root.left) {
                next = Some(&root.left);
            }

            if root.right.leaf_count() > 0 && check(&root.right) {
                stack.push(&root.right);
            }
        }

        Leaves {
            tree,
            next,
            stack,
            check,
        }
    }
}

impl<'a, Check: Fn(&BvhNode) -> bool> Iterator for Leaves<'a, Check> {
    type Item = u32;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.next.is_none() {
                self.next = self.stack.pop();
            }

            let node = self.next.take()?;

            if node.is_leaf() {
                return Some(node.children);
            }

            let children = &self.tree.nodes[node.children as usize];
            let left = &children.left;
            let right = &children.right;

            if (self.check)(left) {
                self.next = Some(left);
            }

            if (self.check)(right) {
                if self.next.is_none() {
                    self.next = Some(right);
                } else {
                    self.stack.push(right);
                }
            }
        }
    }
}

/// Cost associated to a BVH leaf during best-first traversal.
pub trait BvhLeafCost {
    /// The cost value associated to the leaf.
    ///
    /// Best-first searches for the leaf with the lowest cost.
    fn cost(&self) -> Real;
}

impl BvhLeafCost for Real {
    #[inline(always)]
    fn cost(&self) -> Real {
        *self
    }
}

impl<T> BvhLeafCost for (Real, T) {
    #[inline(always)]
    fn cost(&self) -> Real {
        self.0
    }
}

impl Bvh {
    /// Iterates through the leaves, in depth-first order.
    ///
    /// The `check_node` closure is called on every traversed node. If it returns `false` then the
    /// node and all its descendants won’t be iterated on. This is useful for pruning whole
    /// sub-trees based on a geometric predicate on the node’s AABB.
    ///
    /// See also the [`Bvh::traverse`] function which is slightly less convenient since it doesn’t
    /// rely on the iterator system, but takes a closure that implements [`FnMut`] instead of [`Fn`].
    pub fn leaves<F: Fn(&BvhNode) -> bool>(&self, check_node: F) -> Leaves<'_, F> {
        Leaves::new(self, check_node)
    }
}

/// Controls the execution flow of [`Bvh::traverse`].
pub enum TraversalAction {
    /// The traversal will continue on the children of the tested node.
    Continue,
    /// The traversal will skip all descendants of the tested node.
    Prune,
    /// The traversal will exit immediately.
    EarlyExit,
}

impl Bvh {
    #[inline(always)]
    pub(crate) fn traversal_stack() -> SmallVec<[u32; 32]> {
        Default::default()
    }

    /// Traverses the BVH in depth-first order with full control over traversal.
    ///
    /// This method provides low-level control over tree traversal. For each node visited,
    /// it calls your closure which can decide whether to continue traversing, prune that
    /// subtree, or exit early.
    ///
    /// # Arguments
    ///
    /// * `check_node` - A mutable closure called for each node. It receives a [`BvhNode`]
    ///   and returns a [`TraversalAction`] to control the traversal:
    ///   - `Continue`: Visit this node's children (if not a leaf)
    ///   - `Prune`: Skip this node's subtree entirely
    ///   - `EarlyExit`: Stop traversal immediately
    ///
    /// # Traversal Order
    ///
    /// The tree is traversed in **depth-first** order:
    /// 1. Check current node with `check_node`
    /// 2. If `Continue`, descend into left child first
    /// 3. Then descend into right child
    /// 4. Backtrack when both subtrees are processed
    ///
    /// This order is cache-friendly and matches the tree's memory layout.
    ///
    /// # When to Use
    ///
    /// Use `traverse` when you need:
    /// - **Mutable state** during traversal
    /// - **Early exit** conditions
    /// - **Full control** over pruning decisions
    /// - **Custom query logic** that doesn't fit standard patterns
    ///
    /// For simpler cases, consider:
    /// - [`leaves`](Self::leaves) - Iterator over leaves matching a predicate
    /// - [`intersect_aabb`](Self::intersect_aabb) - Find leaves intersecting an AABB
    /// - [`cast_ray`](Self::cast_ray) - Ray casting queries
    ///
    /// # Performance
    ///
    /// - **Best case**: O(log n) when pruning effectively
    /// - **Worst case**: O(n) when visiting all nodes
    /// - Cache-friendly due to depth-first order
    /// - No allocations beyond a small stack (~32 entries)
    ///
    /// # Examples
    ///
    /// ## Count leaves in a region
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhBuildStrategy, TraversalAction};
    /// use parry3d::bounding_volume::{Aabb, BoundingVolume};
    /// use parry3d::math::Vector;
    ///
    /// let aabbs = vec![
    ///     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(5.0, 0.0, 0.0), Vector::new(6.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(10.0, 0.0, 0.0), Vector::new(11.0, 1.0, 1.0)),
    /// ];
    ///
    /// let bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &aabbs);
    ///
    /// let query_region = Aabb::new(
    ///     Vector::new(-1.0, -1.0, -1.0),
    ///     Vector::new(7.0, 2.0, 2.0)
    /// );
    ///
    /// let mut count = 0;
    /// bvh.traverse(|node| {
    ///     if !node.aabb().intersects(&query_region) {
    ///         // Prune: this entire subtree is outside the region
    ///         return TraversalAction::Prune;
    ///     }
    ///
    ///     if node.is_leaf() {
    ///         count += 1;
    ///     }
    ///
    ///     TraversalAction::Continue
    /// });
    ///
    /// assert_eq!(count, 2); // First two objects intersect the region
    /// # }
    /// ```
    ///
    /// ## Find first leaf matching a condition
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhBuildStrategy, TraversalAction};
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabbs = vec![
    ///     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(5.0, 0.0, 0.0), Vector::new(6.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(10.0, 0.0, 0.0), Vector::new(11.0, 1.0, 1.0)),
    /// ];
    ///
    /// let bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &aabbs);
    ///
    /// let target_point = Vector::new(5.5, 0.5, 0.5);
    /// let mut found_leaf = None;
    ///
    /// bvh.traverse(|node| {
    ///     if !node.aabb().contains_local_point(target_point) {
    ///         return TraversalAction::Prune;
    ///     }
    ///
    ///     if let Some(leaf_id) = node.leaf_data() {
    ///         found_leaf = Some(leaf_id);
    ///         return TraversalAction::EarlyExit; // Found it, stop searching
    ///     }
    ///
    ///     TraversalAction::Continue
    /// });
    ///
    /// assert_eq!(found_leaf, Some(1));
    /// # }
    /// ```
    ///
    /// ## Collect statistics with mutable state
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhBuildStrategy, TraversalAction};
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let aabbs = vec![
    ///     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(2.0, 0.0, 0.0), Vector::new(3.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(4.0, 0.0, 0.0), Vector::new(5.0, 1.0, 1.0)),
    /// ];
    ///
    /// let bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &aabbs);
    ///
    /// let mut stats = (0, 0); // (num_internal_nodes, num_leaves)
    ///
    /// bvh.traverse(|node| {
    ///     if node.is_leaf() {
    ///         stats.1 += 1;
    ///     } else {
    ///         stats.0 += 1;
    ///     }
    ///     TraversalAction::Continue
    /// });
    ///
    /// assert_eq!(stats.1, 3); // 3 leaves
    /// # }
    /// ```
    ///
    /// # Notes
    ///
    /// - The closure is called for **every** visited node (internal and leaf)
    /// - Pruning a node skips all its descendants
    /// - Use `node.is_leaf()` to distinguish leaves from internal nodes
    /// - Use `node.leaf_data()` to get the leaf's index (returns `None` for internal nodes)
    /// - The traversal uses a small fixed-size stack internally
    ///
    /// # See Also
    ///
    /// - [`leaves`](Self::leaves) - Simpler iterator interface for leaf traversal
    /// - [`intersect_aabb`](Self::intersect_aabb) - Specialized AABB intersection query
    /// - [`cast_ray`](Self::cast_ray) - Ray casting with best-first traversal
    /// - [`TraversalAction`] - Controls traversal flow
    pub fn traverse(&self, mut check_node: impl FnMut(&BvhNode) -> TraversalAction) {
        let mut stack = Self::traversal_stack();
        let mut curr_id = 0;

        if self.nodes.is_empty() {
            return;
        } else if self.nodes[0].right.leaf_count() == 0 {
            // Special case for partial root.
            let _ = check_node(&self.nodes[0].left);
            return;
        }

        loop {
            let node = &self.nodes[curr_id as usize];
            let left = &node.left;
            let right = &node.right;
            let go_left = match check_node(left) {
                TraversalAction::Continue => !left.is_leaf(),
                TraversalAction::Prune => false,
                TraversalAction::EarlyExit => return,
            };
            let go_right = match check_node(right) {
                TraversalAction::Continue => !right.is_leaf(),
                TraversalAction::Prune => false,
                TraversalAction::EarlyExit => return,
            };

            match (go_left, go_right) {
                (true, true) => {
                    curr_id = left.children;
                    stack.push(right.children);
                }
                (true, false) => curr_id = left.children,
                (false, true) => curr_id = right.children,
                (false, false) => {
                    let Some(next) = stack.pop() else {
                        return;
                    };
                    curr_id = next;
                }
            }
        }
    }

    /// Find the leaf that minimizes their associated cost.
    pub fn find_best<L: BvhLeafCost>(
        &self,
        max_cost: Real,
        aabb_cost: impl Fn(&BvhNode, Real) -> Real,
        leaf_cost: impl Fn(u32, Real) -> Option<L>,
    ) -> Option<(u32, L)> {
        // A stack with 32 elements should be more than enough in most cases.
        let mut stack = Self::traversal_stack();
        let mut best_val = None;
        let mut best_cost = max_cost;
        let mut best_id = u32::MAX;
        let mut curr_id = 0;

        if self.nodes.is_empty() {
            return None;
        } else if self.nodes[0].right.leaf_count() == 0 {
            // Special case for partial root.
            let leaf = &self.nodes[0].left;
            if aabb_cost(leaf, max_cost) < max_cost {
                let cost = leaf_cost(leaf.children, best_cost)?;
                return (cost.cost() < max_cost).then_some((leaf.children, cost));
            } else {
                return None;
            }
        }

        loop {
            let node = &self.nodes[curr_id as usize];
            let mut left = &node.left;
            let mut right = &node.right;

            let mut left_score = aabb_cost(left, best_cost);
            let mut right_score = aabb_cost(right, best_cost);

            if left_score > right_score {
                core::mem::swap(&mut left_score, &mut right_score);
                core::mem::swap(&mut left, &mut right);
            }

            let mut found_next = false;
            if left_score < best_cost && left_score != Real::MAX {
                if left.is_leaf() {
                    if let Some(primitive_val) = leaf_cost(left.children, best_cost) {
                        let primitive_score = primitive_val.cost();
                        if primitive_score < best_cost {
                            best_val = Some(primitive_val);
                            best_cost = primitive_score;
                            best_id = left.children;
                        }
                    }
                } else {
                    curr_id = left.children;
                    found_next = true;
                }
            }

            if right_score < best_cost && right_score != Real::MAX {
                if right.is_leaf() {
                    if let Some(primitive_val) = leaf_cost(right.children, best_cost) {
                        let primitive_score = primitive_val.cost();
                        if primitive_score < best_cost {
                            best_val = Some(primitive_val);
                            best_cost = primitive_score;
                            best_id = right.children;
                        }
                    }
                } else if found_next {
                    stack.push(right.children);
                } else {
                    curr_id = right.children;
                    found_next = true;
                }
            }

            if !found_next {
                if let Some(next) = stack.pop() {
                    curr_id = next;
                } else {
                    return best_val.map(|val| (best_id, val));
                }
            }
        }
    }
}

use super::{Bvh, BvhWorkspace};
use crate::math::Real;
use core::cmp::Ordering;

#[cfg(not(feature = "std"))]
use crate::math::ComplexField; // For `round` and `sqrt` in no-std+alloc builds.

#[derive(Copy, Clone, Debug)]
struct TotalOrdReal(Real);

impl PartialEq for TotalOrdReal {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for TotalOrdReal {}

impl PartialOrd for TotalOrdReal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TotalOrdReal {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl Bvh {
    fn optimization_config(&self, frame_index: u32) -> OptimizationConfig {
        const TARGET_REBUILD_NODE_PERCENTAGE: u32 = 5;
        let num_leaves = self.nodes[0].leaf_count();
        let num_optimized_leaves = (num_leaves * TARGET_REBUILD_NODE_PERCENTAGE).div_ceil(100);

        let num_leaves_sqrt = (num_leaves as Real).sqrt();
        let root_mode = if frame_index.is_multiple_of(2) {
            RootOptimizationMode::Skip
        } else if (frame_index / 2).is_multiple_of(16) {
            RootOptimizationMode::BreadthFirst
        } else {
            RootOptimizationMode::PriorityQueue
        };

        let target_root_node_count = num_leaves_sqrt.ceil();
        let target_subtree_leaf_count = (num_leaves_sqrt * 4.0).ceil();
        let root_refinement_cost = target_root_node_count * target_root_node_count.log2()
            / (target_subtree_leaf_count * target_subtree_leaf_count.log2());
        let mut target_optimized_subtree_count =
            (num_optimized_leaves as Real / target_subtree_leaf_count - root_refinement_cost)
                .round()
                .max(0.0) as usize;

        if root_mode == RootOptimizationMode::Skip {
            target_optimized_subtree_count = target_optimized_subtree_count.max(1)
        }

        OptimizationConfig {
            target_root_node_count: target_root_node_count as usize,
            target_subtree_leaf_count: target_subtree_leaf_count as usize,
            target_optimized_subtree_count,
            root_mode,
        }
    }

    /// Incrementally improves tree quality after dynamic updates.
    ///
    /// This method performs one step of tree optimization by rebuilding small portions of
    /// the BVH to maintain good query performance. As objects move around, the tree's structure
    /// can become suboptimal even after refitting. This method gradually fixes these issues
    /// without the cost of rebuilding the entire tree.
    ///
    /// # What It Does
    ///
    /// Each call optimizes:
    /// - A small percentage of the tree (typically ~5% of leaves)
    /// - The root region (periodically)
    /// - Subtrees that have degraded the most
    ///
    /// The optimization is **incremental** - it's designed to be called repeatedly (e.g.,
    /// every few frames) to continuously maintain tree quality without large frame-time spikes.
    ///
    /// # When to Use
    ///
    /// Call `optimize_incremental`:
    /// - Every 5-10 frames for highly dynamic scenes
    /// - After many [`insert`] and [`remove`] operations
    /// - When query performance degrades over time
    /// - In physics engines, once per simulation step
    ///
    /// **Don't call this**:
    /// - Every frame (too frequent - diminishing returns)
    /// - For static scenes (not needed)
    /// - Immediately after [`from_leaves`] (tree is already optimal)
    ///
    /// # Arguments
    ///
    /// * `workspace` - A reusable workspace to avoid allocations. The same workspace
    ///   can be shared across multiple BVH operations.
    ///
    /// # Performance
    ///
    /// - **Time**: O(k log k) where k is the number of leaves optimized (~5% of total)
    /// - **Target**: <1ms for trees with 10,000 leaves (on modern hardware)
    /// - Spreads optimization cost over many frames
    /// - Much cheaper than full rebuild (O(n log n))
    ///
    /// # Examples
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
    /// // Add initial objects
    /// for i in 0..1000 {
    ///     let aabb = Aabb::new(
    ///         Vector::new(i as f32, 0.0, 0.0),
    ///         Vector::new(i as f32 + 1.0, 1.0, 1.0)
    ///     );
    ///     bvh.insert(aabb, i);
    /// }
    ///
    /// // Game loop
    /// for frame in 0..1000 {
    ///     // Update object positions every frame
    ///     for i in 0..1000 {
    ///         let time = frame as f32 * 0.016;
    ///         let offset = (time * (i as f32 + 1.0)).sin() * 5.0;
    ///         let aabb = Aabb::new(
    ///             Vector::new(i as f32 + offset, 0.0, 0.0),
    ///             Vector::new(i as f32 + offset + 1.0, 1.0, 1.0)
    ///         );
    ///         bvh.insert_or_update_partially(aabb, i, 0.0);
    ///     }
    ///
    ///     // Update AABBs every frame (fast)
    ///     bvh.refit(&mut workspace);
    ///
    ///     // Optimize tree quality every 5 frames (slower but important)
    ///     if frame % 5 == 0 {
    ///         bvh.optimize_incremental(&mut workspace);
    ///     }
    ///
    ///     // Perform queries (ray casts, collision detection, etc.)
    /// }
    /// # }
    /// ```
    ///
    /// ## Physics engine broad-phase
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
    /// // Simulation loop
    /// for step in 0..100 {
    ///     // Physics update: integrate velocities, forces, etc.
    ///     // Update BVH with new rigid body positions
    ///     for body_id in 0..100 {
    ///         let aabb = get_body_aabb(body_id);
    ///         bvh.insert_or_update_partially(aabb, body_id, 0.0);
    ///     }
    ///
    ///     // Refit tree for new positions
    ///     bvh.refit(&mut workspace);
    ///
    ///     // Optimize tree quality once per step
    ///     bvh.optimize_incremental(&mut workspace);
    ///
    ///     // Broad-phase collision detection
    ///     // let pairs = find_overlapping_pairs(&bvh);
    ///     // ...
    /// }
    ///
    /// # fn get_body_aabb(id: u32) -> Aabb {
    /// #     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0))
    /// # }
    /// # }
    /// ```
    ///
    /// ## After many insertions/removals
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
    /// // Dynamically add many objects over time
    /// for i in 0..1000 {
    ///     let aabb = Aabb::new(
    ///         Vector::new(i as f32, 0.0, 0.0),
    ///         Vector::new(i as f32 + 1.0, 1.0, 1.0)
    ///     );
    ///     bvh.insert(aabb, i);
    ///
    ///     // Periodically optimize while building
    ///     if i % 100 == 0 && i > 0 {
    ///         bvh.optimize_incremental(&mut workspace);
    ///     }
    /// }
    ///
    /// // Tree is now well-optimized despite incremental construction
    /// # }
    /// ```
    ///
    /// # How It Works
    ///
    /// The method uses a sophisticated strategy:
    ///
    /// 1. **Subtree Selection**: Identifies small subtrees that would benefit most from rebuilding
    /// 2. **Local Rebuild**: Rebuilds selected subtrees using the binned construction algorithm
    /// 3. **Root Optimization**: Periodically optimizes the top levels of the tree
    /// 4. **Rotation**: Tracks changes and rotates subtrees incrementally
    ///
    /// The optimization rotates through different parts of the tree on successive calls,
    /// ensuring the entire tree is eventually optimized.
    ///
    /// # Comparison with Full Rebuild
    ///
    /// | Operation | Time | When to Use |
    /// |-----------|------|-------------|
    /// | `optimize_incremental` | O(k log k), k ≈ 5% of n | Every few frames in dynamic scenes |
    /// | `from_leaves` (rebuild) | O(n log n) | When tree is very poor or scene is static |
    ///
    /// # Notes
    ///
    /// - State persists across calls - each call continues from where the last left off
    /// - Call frequency can be adjusted based on performance budget
    /// - Complementary to [`refit`] - use both for best results
    /// - The workspace stores optimization state between calls
    ///
    /// # See Also
    ///
    /// - [`refit`](Bvh::refit) - Update AABBs after leaf movements (call more frequently)
    /// - [`from_leaves`](Bvh::from_leaves) - Full rebuild (for major scene changes)
    /// - [`BvhWorkspace`] - Reusable workspace for operations
    ///
    /// [`insert`]: Bvh::insert
    /// [`remove`]: Bvh::remove
    /// [`from_leaves`]: Bvh::from_leaves
    /// [`refit`]: Bvh::refit
    pub fn optimize_incremental(&mut self, workspace: &mut BvhWorkspace) {
        if self.nodes.is_empty() {
            return;
        }

        workspace.rebuild_leaves.clear();
        workspace.rebuild_frame_index = workspace.rebuild_frame_index.overflowing_add(1).0;
        let config = self.optimization_config(workspace.rebuild_frame_index);

        /*
         * Subtree optimizations.
         */
        // let t0 = core::time::Instant::now();
        let num_leaves = self.nodes[0].leaf_count();
        let mut start_index = workspace.rebuild_start_index;

        // println!("Max candidate leaf count = {}", max_candidate_leaf_count);
        self.find_optimization_roots(
            workspace,
            0,
            &mut start_index,
            config.target_optimized_subtree_count as u32,
            0,
            config.target_subtree_leaf_count as u32,
        );

        if start_index >= num_leaves {
            start_index = 0;
            // TODO: if we hit the end of the tree, wrap back at the beginning
            //       to reach the target subtree count.
        }

        workspace.rebuild_start_index = start_index;

        // println!(
        //     "Num refinement candidates: {}, list: {:?}",
        //     workspace.optimization_roots.len(),
        //     workspace.optimization_roots
        // );

        /*
         * Root optimization.
         */
        workspace.rebuild_leaves.clear();

        // let t0 = core::time::Instant::now();
        match config.root_mode {
            RootOptimizationMode::BreadthFirst => self
                .find_root_optimization_pseudo_leaves_breadth_first(
                    workspace,
                    config.target_root_node_count,
                ),
            RootOptimizationMode::PriorityQueue => self
                .find_root_optimization_pseudo_leaves_pqueue(
                    workspace,
                    config.target_root_node_count,
                ),
            RootOptimizationMode::Skip => {}
        }

        if !workspace.rebuild_leaves.is_empty() {
            self.rebuild_range_binned(0, &mut workspace.rebuild_leaves);
        }
        // println!("Root optimization: {}", t0.elapsed().as_secs_f32() * 1000.0);

        /*
         * Subtree leaf optimizations.
         */
        for i in 0..workspace.optimization_roots.len() {
            let subtree_root_id = workspace.optimization_roots[i];
            workspace.rebuild_leaves.clear();
            self.collect_leaves(workspace, subtree_root_id);

            // let t1 = core::time::Instant::now();
            self.rebuild_range_binned(subtree_root_id, &mut workspace.rebuild_leaves);
            // println!(
            //     "Optimized leaves: {}, time: {}",
            //     workspace.rebuild_leaves.len(),
            //     t1.elapsed().as_secs_f32() * 1000.0
            // );
        }

        // println!(
        //     "Leaf optimization: {}, optimization roots: {}, config: {:?}",
        //     t0.elapsed().as_secs_f32() * 1000.0,
        //     workspace.optimization_roots.len(),
        //     config
        // );
        workspace.optimization_roots.clear();
    }

    fn collect_leaves(&self, workspace: &mut BvhWorkspace, subtree_root: u32) {
        let node = &self.nodes[subtree_root as usize];
        let left = &node.left;
        let right = &node.right;

        if left.is_leaf() {
            workspace.rebuild_leaves.push(*left);
        } else {
            self.collect_leaves(workspace, left.children);
        }

        if right.is_leaf() {
            workspace.rebuild_leaves.push(*right);
        } else {
            self.collect_leaves(workspace, right.children);
        }
    }

    fn find_root_optimization_pseudo_leaves_breadth_first(
        &mut self,
        workspace: &mut BvhWorkspace,
        target_count: usize,
    ) {
        if self.nodes.len() < 2 {
            return;
        }

        workspace.dequeue.push_back(0);

        while workspace.dequeue.len() + workspace.rebuild_leaves.len() < target_count {
            let Some(curr_node) = workspace.dequeue.pop_front() else {
                break;
            };

            let node = &self.nodes[curr_node as usize];
            let left = &node.left;
            let right = &node.right;

            if left.is_leaf() || left.data.is_change_pending() {
                workspace.rebuild_leaves.push(*left);
            } else {
                workspace.dequeue.push_back(left.children);
            }

            if right.is_leaf() || right.data.is_change_pending() {
                workspace.rebuild_leaves.push(*right);
            } else {
                workspace.dequeue.push_back(right.children);
            }
        }

        for id in workspace.dequeue.drain(..) {
            let node = &self.nodes[id as usize];
            workspace.rebuild_leaves.push(node.left);
            workspace.rebuild_leaves.push(node.right);
        }
    }

    fn find_root_optimization_pseudo_leaves_pqueue(
        &mut self,
        workspace: &mut BvhWorkspace,
        target_count: usize,
    ) {
        if self.nodes.len() < 2 {
            return;
        }

        workspace.queue.push(BvhOptimizationHeapEntry {
            score: TotalOrdReal(Real::MAX),
            id: 0,
        });

        while workspace.queue.len() + workspace.rebuild_leaves.len() < target_count {
            let Some(curr_node) = workspace.queue.pop() else {
                break;
            };

            let node = &self.nodes[curr_node.id as usize];
            let left = &node.left;
            let right = &node.right;

            if left.is_leaf() || left.data.is_change_pending() {
                workspace.rebuild_leaves.push(*left);
            } else {
                let children = self.nodes[left.children as usize];
                let left_score = children.left.volume() * children.left.leaf_count() as Real
                    + children.right.volume() * children.right.leaf_count() as Real;
                workspace.queue.push(BvhOptimizationHeapEntry {
                    score: TotalOrdReal(left_score),
                    id: left.children,
                });
            }

            if right.is_leaf() || right.data.is_change_pending() {
                workspace.rebuild_leaves.push(*right);
            } else {
                let children = self.nodes[right.children as usize];
                let right_score = children.left.volume() * children.left.leaf_count() as Real
                    + children.right.volume() * children.right.leaf_count() as Real;
                workspace.queue.push(BvhOptimizationHeapEntry {
                    score: TotalOrdReal(right_score),
                    id: right.children,
                });
            }
        }

        for id in workspace.queue.as_slice() {
            let node = &self.nodes[id.id as usize];
            workspace.rebuild_leaves.push(node.left);
            workspace.rebuild_leaves.push(node.right);
        }
        workspace.queue.clear();
    }

    fn find_optimization_roots(
        &mut self,
        workspace: &mut BvhWorkspace,
        curr_node: u32,
        start_index: &mut u32,
        max_optimization_roots: u32,
        mut leaf_count_before: u32,
        max_candidate_leaf_count: u32,
    ) {
        if workspace.optimization_roots.len() == max_optimization_roots as usize {
            // We reached the desired number of collected leaves. Just exit.
            return;
        }

        let node = &mut self.nodes[curr_node as usize];
        let left = &mut node.left;
        let left_leaf_count = left.leaf_count();
        let left_children = left.children;

        if leaf_count_before + left_leaf_count > *start_index {
            // Traverse the left children.
            if left_leaf_count < max_candidate_leaf_count {
                // If the node doesn’t have at least the leaves, it can’t be rebalanced.
                if left_leaf_count > 2 {
                    // Mark the optimization root so that the root pseudo-leaf
                    // extraction knows not to cross this node.
                    // This won’t disturb bvtt traversal because refit will get
                    // rid of this flag.
                    left.data.set_change_pending();
                    workspace.optimization_roots.push(left_children);
                    *start_index += left_leaf_count;
                }
            } else {
                // This node has too many leaves. Recurse.
                self.find_optimization_roots(
                    workspace,
                    left_children,
                    start_index,
                    max_optimization_roots,
                    leaf_count_before,
                    max_candidate_leaf_count,
                );
            }
        }

        leaf_count_before += left_leaf_count;

        if workspace.optimization_roots.len() == max_optimization_roots as usize {
            // We reached the desired number of collected leaves. Just exit.
            return;
        }

        let node = &mut self.nodes[curr_node as usize];
        let right = &mut node.right;
        let right_leaf_count = right.leaf_count();
        let right_children = right.children;

        if leaf_count_before + right_leaf_count > *start_index {
            // Traverse the right children.
            if right_leaf_count < max_candidate_leaf_count {
                if right_leaf_count > 2 {
                    // Mark the optimization root so that the root pseudo-leaf
                    // extraction knows not to cross this node.
                    // This won’t disturb bvtt traversal because refit will get
                    // rid of this flag.
                    right.data.set_change_pending();
                    workspace.optimization_roots.push(right_children);
                    *start_index += right_leaf_count;
                }
            } else {
                self.find_optimization_roots(
                    workspace,
                    right_children,
                    start_index,
                    max_optimization_roots,
                    leaf_count_before,
                    max_candidate_leaf_count,
                );
            }
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum RootOptimizationMode {
    PriorityQueue,
    BreadthFirst,
    Skip,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
struct OptimizationConfig {
    target_root_node_count: usize,
    target_subtree_leaf_count: usize,
    target_optimized_subtree_count: usize,
    root_mode: RootOptimizationMode,
}

#[derive(Copy, Clone, Debug)]
pub struct BvhOptimizationHeapEntry {
    score: TotalOrdReal,
    id: u32,
}

impl PartialOrd for BvhOptimizationHeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BvhOptimizationHeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score.cmp(&other.score)
    }
}

impl PartialEq for BvhOptimizationHeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score
    }
}

impl Eq for BvhOptimizationHeapEntry {}

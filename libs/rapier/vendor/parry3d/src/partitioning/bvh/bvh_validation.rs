use crate::partitioning::bvh::bvh_tree::BvhNodeIndex;
use crate::partitioning::Bvh;
use crate::utils::hashset::HashSet;

impl Bvh {
    /// Counts the number of leaves that can be reached from the node at index `id`.
    ///
    /// This is mostly a utility for debugging.
    pub fn reachable_leaf_count(&self, id: u32) -> u32 {
        if self.nodes.is_empty() {
            0
        } else if self.nodes[0].right.leaf_count() == 0 {
            (id == 0) as u32
        } else {
            let node = &self.nodes[id as usize];
            let left_count = if node.left.is_leaf() {
                1
            } else {
                self.reachable_leaf_count(node.left.children)
            };
            let right_count = if node.right.is_leaf() {
                1
            } else {
                self.reachable_leaf_count(node.right.children)
            };
            left_count + right_count
        }
    }

    /// Counts the number of leaves of `self`, starting with the subtree indexed at
    /// `id`, that are marked as changed.
    ///
    /// This mostly a utility for debugging.
    pub fn changed_leaf_count(&self, id: u32) -> u32 {
        if self.nodes.is_empty() {
            0
        } else if self.nodes[0].right.leaf_count() == 0 {
            (id == 0 && self.nodes[0].left.data.is_changed()) as u32
        } else {
            let node = &self.nodes[id as usize];
            let left_count = if node.left.is_leaf() {
                node.left.is_changed() as u32
            } else {
                self.changed_leaf_count(node.left.children)
            };
            let right_count = if node.right.is_leaf() {
                node.right.is_changed() as u32
            } else {
                self.changed_leaf_count(node.right.children)
            };
            left_count + right_count
        }
    }

    /// Panics if the tree isn’t well-formed.
    ///
    /// The tree is well-formed if it is topologically correct (internal indices are all valid) and
    /// geometrically correct (node metadata of a parent bound the ones of the children).
    ///
    /// Returns the calculated leaf count.
    pub fn assert_well_formed(&self) {
        if self.is_empty() {
            return;
        } else if self.nodes[0].right.leaf_count() == 0 {
            assert_eq!(self.nodes[0].leaf_count(), 1);
            assert!(self.nodes[0].left.is_leaf());
            return;
        }

        let mut loop_detection = HashSet::default();
        let _ = self.assert_well_formed_recurse(0, &mut loop_detection);
    }

    fn assert_well_formed_recurse(&self, node_id: u32, loop_detection: &mut HashSet<u32>) -> u32 {
        let node = &self.nodes[node_id as usize];

        if !loop_detection.insert(node_id) {
            panic!("Detected loop. Node {} visited twice.", node_id);
        }

        let left_count = if node.left.is_leaf() {
            let leaf_data = self.leaf_node_indices[node.left.children as usize];
            assert_eq!(leaf_data, BvhNodeIndex::left(node_id));
            1
        } else {
            let calculated_leaf_count =
                self.assert_well_formed_recurse(node.left.children, loop_detection);
            let child = &self.nodes[node.left.children as usize];
            assert_eq!(
                self.parents[node.left.children as usize],
                BvhNodeIndex::left(node_id)
            );
            assert_eq!(
                child.right.is_changed() || child.left.is_changed(),
                node.left.is_changed()
            );
            assert!(node.left.contains(&child.left));
            assert!(node.left.contains(&child.right));
            assert_eq!(
                node.left.leaf_count(),
                child.left.leaf_count() + child.right.leaf_count()
            );
            assert_eq!(node.left.leaf_count(), calculated_leaf_count);
            calculated_leaf_count
        };

        let right_count = if node.right.is_leaf() {
            let leaf_data = self.leaf_node_indices[node.right.children as usize];
            assert_eq!(leaf_data, BvhNodeIndex::right(node_id));
            1
        } else {
            let calculated_leaf_count =
                self.assert_well_formed_recurse(node.right.children, loop_detection);
            let child = &self.nodes[node.right.children as usize];
            assert_eq!(
                self.parents[node.right.children as usize],
                BvhNodeIndex::right(node_id)
            );
            assert_eq!(
                child.right.is_changed() || child.left.is_changed(),
                node.right.is_changed()
            );
            assert!(node.right.contains(&child.left));
            assert!(node.right.contains(&child.right));
            assert_eq!(
                node.right.leaf_count(),
                child.left.leaf_count() + child.right.leaf_count()
            );
            assert_eq!(calculated_leaf_count, node.right.leaf_count());
            calculated_leaf_count
        };

        left_count + right_count
    }

    /// Similar to [`Self::assert_well_formed`] but doesn’t check the geometry (i.e. it won’t
    /// check that parent AABBs enclose child AABBs).
    ///
    /// This can be useful for checking intermediate states of the tree after topological changes
    /// but before refitting.
    pub fn assert_well_formed_topology_only(&self) {
        let _ = self.assert_well_formed_topology_only_recurse(0);
    }

    fn assert_well_formed_topology_only_recurse(&self, node_id: u32) -> u32 {
        let node = &self.nodes[node_id as usize];

        let left_count = if node.left.is_leaf() {
            assert!(self
                .leaf_node_indices
                .contains_key(node.left.children as usize));
            1
        } else {
            let calculated_leaf_count =
                self.assert_well_formed_topology_only_recurse(node.left.children);
            let child = &self.nodes[node.left.children as usize];
            assert_eq!(
                self.parents[node.left.children as usize],
                BvhNodeIndex::left(node_id)
            );
            assert_eq!(
                node.left.leaf_count(),
                child.left.leaf_count() + child.right.leaf_count()
            );
            assert_eq!(node.left.leaf_count(), calculated_leaf_count);
            calculated_leaf_count
        };

        let right_count = if node.right.is_leaf() {
            assert!(self
                .leaf_node_indices
                .contains_key(node.right.children as usize));
            1
        } else {
            let calculated_leaf_count =
                self.assert_well_formed_topology_only_recurse(node.right.children);
            let child = &self.nodes[node.right.children as usize];
            assert_eq!(
                self.parents[node.right.children as usize],
                BvhNodeIndex::right(node_id)
            );
            assert_eq!(
                node.right.leaf_count(),
                child.left.leaf_count() + child.right.leaf_count()
            );
            assert_eq!(calculated_leaf_count, node.right.leaf_count());
            calculated_leaf_count
        };

        left_count + right_count
    }

    /// Panics if the nodes of `self` are not stored in depth-first order on its internal storage.
    ///
    /// Depth-first ordering of `self`’s internals are guaranteed by [`Self::refit`].
    pub fn assert_is_depth_first(&self) {
        if self.is_empty() || self.nodes[0].right.leaf_count() == 0 {
            // Trivially correct and avoids special-cases afterward.
            return;
        }

        let mut stack = alloc::vec![0];
        let mut loop_id = 0;
        while let Some(id) = stack.pop() {
            assert_eq!(loop_id, id);
            loop_id += 1;
            let node = &self.nodes[id as usize];

            if !node.right.is_leaf() {
                stack.push(node.right.children);
            }

            if !node.left.is_leaf() {
                stack.push(node.left.children);
            }
        }
    }
}

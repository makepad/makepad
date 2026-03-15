use super::{Bvh, BvhNode, BvhWorkspace};
use smallvec::SmallVec;

const TRAVERSAL_STACK_SIZE: usize = 32;

impl Bvh {
    /*
     * Traversal of a tree against itself.
     */
    // NOTE PERF: change detection doesn’t make a huge difference in 2D (it can
    //            occasionally even make it slower!) Once we support a static/dynamic tree
    //            instead of a single tree, we might want to fully disable change detection
    //            in 2D.
    /// Traverses the Bounding Volume Test Tree of a tree against itself.
    ///
    /// The closure `f` will be called on each pair of leaf that passed the AABB intersection checks.
    /// If `CHANGE_DETECTION` is `true`, then only pairs of leaves where at least one was detected
    /// as changed during [`Self::insert_or_update_partially`] will be traversed.
    pub fn traverse_bvtt_single_tree<const CHANGE_DETECTION: bool>(
        &self,
        workspace: &mut BvhWorkspace,
        f: &mut impl FnMut(u32, u32),
    ) {
        if self.nodes.is_empty() || self.nodes[0].right.leaf_count() == 0 {
            // Not enough nodes for any overlap.
            return;
        }

        workspace.traversal_stack.clear();
        self.self_intersect_node::<CHANGE_DETECTION>(workspace, 0, f)
    }

    // Traverses overlaps of a single node with itself.
    // This as special case to:
    // - Ensure we don’t traverse the same branch twice.
    // - Only check the left/right overlap. Left/left and right/right checks trivially pass.
    // TODO: take change detection into account.
    fn self_intersect_node<const CHANGE_DETECTION: bool>(
        &self,
        workspace: &mut BvhWorkspace,
        id: u32,
        f: &mut impl FnMut(u32, u32),
    ) {
        let node = &self.nodes[id as usize];

        if CHANGE_DETECTION && !node.right.is_changed() && !node.left.is_changed() {
            return;
        }

        let left_right_intersect = node.left.intersects(&node.right);
        let left_child = node.left.children;
        let right_child = node.right.children;
        let left_is_leaf = node.left.is_leaf();
        let right_is_leaf = node.right.is_leaf();

        if (!CHANGE_DETECTION || node.left.is_changed()) && !left_is_leaf {
            self.self_intersect_node::<CHANGE_DETECTION>(workspace, left_child, f);
        }

        if (!CHANGE_DETECTION || node.right.is_changed()) && !right_is_leaf {
            self.self_intersect_node::<CHANGE_DETECTION>(workspace, right_child, f);
        }

        if left_right_intersect {
            match (left_is_leaf, right_is_leaf) {
                (true, true) => f(left_child, right_child),
                (true, false) => self.traverse_single_subtree::<CHANGE_DETECTION>(
                    workspace,
                    &node.left,
                    right_child,
                    f,
                ),
                (false, true) => self.traverse_single_subtree::<CHANGE_DETECTION>(
                    workspace,
                    &node.right,
                    left_child,
                    f,
                ),
                (false, false) => self.traverse_two_branches::<CHANGE_DETECTION>(
                    workspace,
                    left_child,
                    right_child,
                    f,
                ),
            }
        }
    }

    fn traverse_two_branches<const CHANGE_DETECTION: bool>(
        &self,
        workspace: &mut BvhWorkspace,
        a: u32,
        b: u32,
        f: &mut impl FnMut(u32, u32),
    ) {
        let node1 = &self.nodes[a as usize];
        let node2 = &self.nodes[b as usize];

        let left1 = &node1.left;
        let right1 = &node1.right;
        let left2 = &node2.left;
        let right2 = &node2.right;

        let left_left = (!CHANGE_DETECTION || left1.is_changed() || left2.is_changed())
            && left1.intersects(left2);
        let left_right = (!CHANGE_DETECTION || left1.is_changed() || right2.is_changed())
            && left1.intersects(right2);
        let right_left = (!CHANGE_DETECTION || right1.is_changed() || left2.is_changed())
            && right1.intersects(left2);
        let right_right = (!CHANGE_DETECTION || right1.is_changed() || right2.is_changed())
            && right1.intersects(right2);

        macro_rules! dispatch(
            ($check: ident, $child_a: ident, $child_b: ident) => {
                if $check {
                    match ($child_a.is_leaf(), $child_b.is_leaf()) {
                        (true, true) => f($child_a.children, $child_b.children),
                        (true, false) => {
                            self.traverse_single_subtree::<CHANGE_DETECTION>(workspace, $child_a, $child_b.children, f)
                        }
                        (false, true) => self.traverse_single_subtree::<CHANGE_DETECTION>(
                            workspace,
                            $child_b,
                            $child_a.children,
                            f,
                        ),
                        (false, false) => self.traverse_two_branches::<CHANGE_DETECTION>(
                            workspace,
                            $child_a.children,
                            $child_b.children,
                            f,
                        ),
                    }
                }
            }
        );

        dispatch!(left_left, left1, left2);
        dispatch!(left_right, left1, right2);
        dispatch!(right_left, right1, left2);
        dispatch!(right_right, right1, right2);
    }

    // Checks overlap between a single node and a subtree.
    fn traverse_single_subtree<const CHANGE_DETECTION: bool>(
        &self,
        workspace: &mut BvhWorkspace,
        node: &BvhNode,
        subtree: u32,
        f: &mut impl FnMut(u32, u32),
    ) {
        debug_assert!(workspace.traversal_stack.is_empty());

        // Since this is traversing against a single node it is more efficient to keep the leaf reference
        // around and traverse the branch using a manual stack. Left branches are traversed by the main
        // loop whereas the right branches are pushed to the stack.
        let mut curr_id = subtree;
        let node_changed = node.is_changed();

        loop {
            let curr = &self.nodes[curr_id as usize];
            let left = &curr.left;
            let right = &curr.right;
            let left_check =
                (!CHANGE_DETECTION || node_changed || left.is_changed()) && node.intersects(left);
            let right_check =
                (!CHANGE_DETECTION || node_changed || right.is_changed()) && node.intersects(right);
            let left_is_leaf = left.is_leaf();
            let right_is_leaf = right.is_leaf();
            let mut found_next = false;

            if left_check {
                if left_is_leaf {
                    f(node.children, left.children)
                } else {
                    curr_id = left.children;
                    found_next = true;
                }
            }

            if right_check {
                if right_is_leaf {
                    f(node.children, right.children)
                } else if !found_next {
                    curr_id = right.children;
                    found_next = true;
                } else {
                    // We already advanced in curr_id once, push the other
                    // branch to the stack.
                    workspace.traversal_stack.push(right.children);
                }
            }

            if !found_next {
                // Pop the stack to find the next candidate.
                if let Some(next_id) = workspace.traversal_stack.pop() {
                    curr_id = next_id;
                } else {
                    // Traversal is finished.
                    return;
                }
            }
        }
    }

    /// Performs a simultaneous traversal of the BVHs `self` and `other`, and yields the pairs
    /// of leaves it reached.
    ///
    /// Any node pairs failing the given `check` will be excluded from the traversal.
    pub fn leaf_pairs<'a, F: Fn(&BvhNode, &BvhNode) -> bool>(
        &'a self,
        other: &'a Self,
        check: F,
    ) -> LeafPairs<'a, F> {
        if let (Some(root1), Some(root2)) = (self.nodes.first(), other.nodes.first()) {
            let mut stack = SmallVec::default();

            if root1.left.leaf_count() > 0 && root2.right.leaf_count() > 0 {
                stack.push((&root1.left, &root2.right));
                // NOTE: we don’t need to push (&root1.left, &root2.left), it is already given as
                //       the initial value of `LeafPairs::next`.
                // stack.push((&root1.left, &root2.left))
            }

            if root1.right.leaf_count() > 0 {
                if root2.right.leaf_count() > 0 {
                    stack.push((&root1.right, &root2.right));
                }
                stack.push((&root1.right, &root2.left))
            }

            LeafPairs {
                tree1: self,
                tree2: other,
                next: Some((&root1.left, &root2.left)),
                stack,
                check,
            }
        } else {
            LeafPairs {
                tree1: self,
                tree2: other,
                next: None,
                stack: Default::default(),
                check,
            }
        }
    }
}

pub struct LeafPairs<'a, Check: Fn(&BvhNode, &BvhNode) -> bool> {
    tree1: &'a Bvh,
    tree2: &'a Bvh,
    next: Option<(&'a BvhNode, &'a BvhNode)>,
    stack: SmallVec<[(&'a BvhNode, &'a BvhNode); TRAVERSAL_STACK_SIZE]>,
    check: Check,
}

impl<'a, Check: Fn(&BvhNode, &BvhNode) -> bool> Iterator for LeafPairs<'a, Check> {
    type Item = (u32, u32);
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.next.is_none() {
                self.next = self.stack.pop();
            }

            let (node1, node2) = self.next.take()?;

            match (node1.is_leaf(), node2.is_leaf()) {
                (true, true) => return Some((node1.children, node2.children)),
                (true, false) => {
                    let child2 = &self.tree2.nodes[node2.children as usize];
                    if (self.check)(node1, &child2.left) {
                        self.next = Some((node1, &child2.left));
                    }
                    if (self.check)(node1, &child2.right) {
                        if self.next.is_none() {
                            self.next = Some((node1, &child2.right));
                        } else {
                            self.stack.push((node1, &child2.right));
                        }
                    }
                }
                (false, true) => {
                    let child1 = &self.tree1.nodes[node1.children as usize];
                    if (self.check)(&child1.left, node2) {
                        self.next = Some((&child1.left, node2));
                    }
                    if (self.check)(&child1.right, node2) {
                        if self.next.is_none() {
                            self.next = Some((&child1.right, node2));
                        } else {
                            self.stack.push((&child1.right, node2));
                        }
                    }
                }
                (false, false) => {
                    let child1 = &self.tree1.nodes[node1.children as usize];
                    let child2 = &self.tree2.nodes[node2.children as usize];
                    if (self.check)(&child1.left, &child2.left) {
                        self.stack.push((&child1.left, &child2.left));
                    }
                    if (self.check)(&child1.right, &child2.left) {
                        self.stack.push((&child1.right, &child2.left));
                    }
                    if (self.check)(&child1.left, &child2.right) {
                        self.stack.push((&child1.left, &child2.right));
                    }
                    if (self.check)(&child1.right, &child2.right) {
                        self.stack.push((&child1.right, &child2.right));
                    }
                }
            }
        }
    }
}

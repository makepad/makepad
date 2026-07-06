// Port of box3d/src/dynamic_tree.c
// A dynamic AABB tree broad-phase, inspired by Nathanael Presson's btDbvt.
//
// The DynamicTree/TreeNode structs live in crate::types. The C TreeNode unions
// (children|userData, parent|next) are flattened there: `parent` doubles as the
// free-list `next`, and `children`/`user_data` are separate fields.
//
// b3DynamicTree_Save / b3DynamicTree_Load (debug file I/O) are not ported.

use crate::aabb::{enlarge_aabb, perimeter};
use crate::b3_assert;
use crate::b3_validate;
use crate::core::NULL_INDEX;
use crate::math_functions::*;
use crate::math_internal::modified_cross;
use crate::types::{
    BoxCastInput, DynamicTree, RayCastInput, TreeNode, TreeNodeChildren, TreeStats,
    ALLOCATED_NODE, DEFAULT_CATEGORY_BITS, ENLARGED_NODE, LEAF_NODE,
};

pub const TREE_STACK_SIZE: usize = 1024;

const DEFAULT_TREE_NODE: TreeNode = TreeNode {
    aabb: AABB {
        lower_bound: Vec3::ZERO,
        upper_bound: Vec3::ZERO,
    },
    category_bits: DEFAULT_CATEGORY_BITS,
    children: TreeNodeChildren {
        child1: NULL_INDEX,
        child2: NULL_INDEX,
    },
    // In C this shares storage with `children`; internal nodes get UINT64_MAX
    // in b3InsertLeaf and leaves overwrite it in create_proxy.
    user_data: 0,
    parent: NULL_INDEX,
    height: 0,
    flags: ALLOCATED_NODE,
};

#[inline]
fn is_leaf(node: &TreeNode) -> bool {
    node.flags & LEAF_NODE != 0
}

#[inline]
fn is_allocated(node: &TreeNode) -> bool {
    node.flags & ALLOCATED_NODE != 0
}

#[inline]
fn max_u16(a: u16, b: u16) -> u16 {
    if a > b {
        a
    } else {
        b
    }
}

/// Constructing the tree initializes the node pool.
pub fn dynamic_tree_create(proxy_capacity: i32) -> DynamicTree {
    let capacity = max_int(proxy_capacity, 16);

    let mut tree = DynamicTree::default();
    tree.root = NULL_INDEX;

    // maximum node count for a full binary tree is 2 * leafCount - 1
    tree.node_capacity = 2 * capacity - 1;
    tree.node_count = 0;

    tree.nodes = vec![TreeNode::default(); tree.node_capacity as usize];

    // Build a linked list for the free list. The parent pointer becomes the "next" pointer.
    for i in 0..tree.node_capacity - 1 {
        tree.nodes[i as usize].parent = i + 1;
    }

    tree.nodes[(tree.node_capacity - 1) as usize].parent = NULL_INDEX;
    tree.free_list = 0;

    tree.proxy_count = 0;

    tree.rebuild_capacity = 0;

    tree
}

/// Destroy the tree, freeing the node pool.
pub fn dynamic_tree_destroy(tree: &mut DynamicTree) {
    *tree = DynamicTree::default();
}

// Allocate a node from the pool. Grow the pool if necessary.
fn allocate_node(tree: &mut DynamicTree) -> i32 {
    // Expand the node pool as needed.
    if tree.free_list == NULL_INDEX {
        b3_assert!(tree.node_count == tree.node_capacity);

        // The free list is empty. Rebuild a bigger pool.
        let old_capacity = tree.node_capacity;
        tree.node_capacity += old_capacity >> 1;
        tree.nodes.resize(tree.node_capacity as usize, TreeNode::default());

        // Build a linked list for the free list. The parent pointer becomes the "next" pointer.
        for i in tree.node_count..tree.node_capacity - 1 {
            tree.nodes[i as usize].parent = i + 1;
        }

        tree.nodes[(tree.node_capacity - 1) as usize].parent = NULL_INDEX;
        tree.free_list = tree.node_count;
    }

    // Peel a node off the free list.
    let node_index = tree.free_list;
    tree.free_list = tree.nodes[node_index as usize].parent;
    tree.nodes[node_index as usize] = DEFAULT_TREE_NODE;
    tree.node_count += 1;
    node_index
}

// Return a node to the pool.
fn free_node(tree: &mut DynamicTree, node_id: i32) {
    b3_assert!(0 <= node_id && node_id < tree.node_capacity);
    b3_assert!(0 < tree.node_count);
    tree.nodes[node_id as usize].parent = tree.free_list;
    tree.nodes[node_id as usize].flags = 0;
    tree.free_list = node_id;
    tree.node_count -= 1;
}

// Greedy algorithm for sibling selection using the SAH
// We have three nodes A-(B,C) and want to add a leaf D, there are three choices.
// 1: make a new parent for A and D : E-(A-(B,C), D)
// 2: associate D with B
//   a: B is a leaf : A-(E-(B,D), C)
//   b: B is an internal node: A-(B{D},C)
// 3: associate D with C
//   a: C is a leaf : A-(B, E-(C,D))
//   b: C is an internal node: A-(B, C{D})
// All of these have a clear cost except when B or C is an internal node. Hence we need to be greedy.
//
// The cost for cases 1, 2a, and 3a can be computed using the sibling cost formula.
// cost of sibling H = area(union(H, D)) + increased area of ancestors
//
// Suppose B (or C) is an internal node, then the lowest cost would be one of two cases:
// case1: D becomes a sibling of B
// case2: D becomes a descendant of B along with a new internal node of area(D).
fn find_best_sibling(tree: &DynamicTree, box_d: AABB) -> i32 {
    let center_d = aabb_center(box_d);
    let area_d = perimeter(box_d);

    let nodes = &tree.nodes;
    let root_index = tree.root;

    let root_box = nodes[root_index as usize].aabb;

    // Area of current node
    let mut area_base = perimeter(root_box);

    // Area of inflated node
    let mut direct_cost = perimeter(aabb_union(root_box, box_d));
    let mut inherited_cost = 0.0f32;

    let mut best_sibling = root_index;
    let mut best_cost = direct_cost;

    // Descend the tree from root, following a single greedy path.
    let mut index = root_index;
    while !is_leaf(&nodes[index as usize]) {
        let child1 = nodes[index as usize].children.child1;
        let child2 = nodes[index as usize].children.child2;

        // Cost of creating a new parent for this node and the new leaf
        let cost = direct_cost + inherited_cost;

        // Sometimes there are multiple identical costs within tolerance.
        // This breaks the ties using the centroid distance.
        if cost < best_cost {
            best_sibling = index;
            best_cost = cost;
        }

        // Inheritance cost seen by children
        inherited_cost += direct_cost - area_base;

        let leaf1 = is_leaf(&nodes[child1 as usize]);
        let leaf2 = is_leaf(&nodes[child2 as usize]);

        // Cost of descending into child 1
        let mut lower_cost1 = f32::MAX;
        let box1 = nodes[child1 as usize].aabb;
        let direct_cost1 = perimeter(aabb_union(box1, box_d));
        let mut area1 = 0.0f32;
        if leaf1 {
            // Child 1 is a leaf
            // Cost of creating new node and increasing area of node P
            let cost1 = direct_cost1 + inherited_cost;

            // Need this here due to while condition above
            if cost1 < best_cost {
                best_sibling = child1;
                best_cost = cost1;
            }
        } else {
            // Child 1 is an internal node
            area1 = perimeter(box1);

            // Lower bound cost of inserting under child 1.
            lower_cost1 = inherited_cost + direct_cost1 + min_float(area_d - area1, 0.0);
        }

        // Cost of descending into child 2
        let mut lower_cost2 = f32::MAX;
        let box2 = nodes[child2 as usize].aabb;
        let direct_cost2 = perimeter(aabb_union(box2, box_d));
        let mut area2 = 0.0f32;
        if leaf2 {
            // Child 2 is a leaf
            // Cost of creating new node and increasing area of node P
            let cost2 = direct_cost2 + inherited_cost;

            // Need this here due to while condition above
            if cost2 < best_cost {
                best_sibling = child2;
                best_cost = cost2;
            }
        } else {
            // Child 2 is an internal node
            area2 = perimeter(box2);

            // Lower bound cost of inserting under child 2. This is not the cost
            // of child 2, it is the best we can hope for under child 2.
            lower_cost2 = inherited_cost + direct_cost2 + min_float(area_d - area2, 0.0);
        }

        if leaf1 && leaf2 {
            break;
        }

        // Can the cost possibly be decreased?
        if best_cost <= lower_cost1 && best_cost <= lower_cost2 {
            break;
        }

        if lower_cost1 == lower_cost2 && !leaf1 {
            b3_assert!(lower_cost1 < f32::MAX);
            b3_assert!(lower_cost2 < f32::MAX);

            // No clear choice based on lower bound surface area. This can happen when both
            // children fully contain D. Fall back to node distance.
            let d1 = sub(aabb_center(box1), center_d);
            let d2 = sub(aabb_center(box2), center_d);
            lower_cost1 = length_squared(d1);
            lower_cost2 = length_squared(d2);
        }

        // Descend
        if lower_cost1 < lower_cost2 && !leaf1 {
            index = child1;
            area_base = area1;
            direct_cost = direct_cost1;
        } else {
            index = child2;
            area_base = area2;
            direct_cost = direct_cost2;
        }

        b3_assert!(!is_leaf(&nodes[index as usize]));
    }

    best_sibling
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RotateType {
    None,
    BF,
    BG,
    CD,
    CE,
}

// Perform a left or right rotation if node A is imbalanced.
fn rotate_nodes(tree: &mut DynamicTree, i_a: i32) {
    b3_assert!(i_a != NULL_INDEX);

    let node_capacity = tree.node_capacity;
    let nodes = &mut tree.nodes;

    let a = i_a as usize;
    if is_leaf(&nodes[a]) {
        return;
    }

    let i_b = nodes[a].children.child1;
    let i_c = nodes[a].children.child2;
    b3_assert!(0 <= i_b && i_b < node_capacity);
    b3_assert!(0 <= i_c && i_c < node_capacity);

    let b = i_b as usize;
    let c = i_c as usize;

    let is_leaf_b = is_leaf(&nodes[b]);
    let is_leaf_c = is_leaf(&nodes[c]);

    if is_leaf_b && !is_leaf_c {
        let i_f = nodes[c].children.child1;
        let i_g = nodes[c].children.child2;
        let f = i_f as usize;
        let g = i_g as usize;
        b3_assert!(0 <= i_f && i_f < node_capacity);
        b3_assert!(0 <= i_g && i_g < node_capacity);

        // Base cost
        let cost_base = perimeter(nodes[c].aabb);

        // Cost of swapping B and F
        let aabb_bg = aabb_union(nodes[b].aabb, nodes[g].aabb);
        let cost_bf = perimeter(aabb_bg);

        // Cost of swapping B and G
        let aabb_bf = aabb_union(nodes[b].aabb, nodes[f].aabb);
        let cost_bg = perimeter(aabb_bf);

        if cost_base < cost_bf && cost_base < cost_bg {
            // Rotation does not improve cost
            return;
        }

        if cost_bf < cost_bg {
            // Swap B and F
            nodes[a].children.child1 = i_f;
            nodes[c].children.child1 = i_b;

            nodes[b].parent = i_c;
            nodes[f].parent = i_a;

            nodes[c].aabb = aabb_bg;

            nodes[c].height = 1 + max_u16(nodes[b].height, nodes[g].height);
            nodes[a].height = 1 + max_u16(nodes[c].height, nodes[f].height);
            nodes[c].category_bits = nodes[b].category_bits | nodes[g].category_bits;
            nodes[a].category_bits = nodes[c].category_bits | nodes[f].category_bits;
            nodes[c].flags |= (nodes[b].flags | nodes[g].flags) & ENLARGED_NODE;
            nodes[a].flags |= (nodes[c].flags | nodes[f].flags) & ENLARGED_NODE;
        } else {
            // Swap B and G
            nodes[a].children.child1 = i_g;
            nodes[c].children.child2 = i_b;

            nodes[b].parent = i_c;
            nodes[g].parent = i_a;

            nodes[c].aabb = aabb_bf;

            nodes[c].height = 1 + max_u16(nodes[b].height, nodes[f].height);
            nodes[a].height = 1 + max_u16(nodes[c].height, nodes[g].height);
            nodes[c].category_bits = nodes[b].category_bits | nodes[f].category_bits;
            nodes[a].category_bits = nodes[c].category_bits | nodes[g].category_bits;
            nodes[c].flags |= (nodes[b].flags | nodes[f].flags) & ENLARGED_NODE;
            nodes[a].flags |= (nodes[c].flags | nodes[g].flags) & ENLARGED_NODE;
        }
    } else if is_leaf_c && !is_leaf_b {
        // C is a leaf and B is internal

        let i_d = nodes[b].children.child1;
        let i_e = nodes[b].children.child2;
        let d = i_d as usize;
        let e = i_e as usize;
        b3_assert!(0 <= i_d && i_d < node_capacity);
        b3_assert!(0 <= i_e && i_e < node_capacity);

        // Base cost
        let cost_base = perimeter(nodes[b].aabb);

        // Cost of swapping C and D
        let aabb_ce = aabb_union(nodes[c].aabb, nodes[e].aabb);
        let cost_cd = perimeter(aabb_ce);

        // Cost of swapping C and E
        let aabb_cd = aabb_union(nodes[c].aabb, nodes[d].aabb);
        let cost_ce = perimeter(aabb_cd);

        if cost_base < cost_cd && cost_base < cost_ce {
            // Rotation does not improve cost
            return;
        }

        if cost_cd < cost_ce {
            // Swap C and D
            nodes[a].children.child2 = i_d;
            nodes[b].children.child1 = i_c;

            nodes[c].parent = i_b;
            nodes[d].parent = i_a;

            nodes[b].aabb = aabb_ce;

            nodes[b].height = 1 + max_u16(nodes[c].height, nodes[e].height);
            nodes[a].height = 1 + max_u16(nodes[b].height, nodes[d].height);
            nodes[b].category_bits = nodes[c].category_bits | nodes[e].category_bits;
            nodes[a].category_bits = nodes[b].category_bits | nodes[d].category_bits;
            nodes[b].flags |= (nodes[c].flags | nodes[e].flags) & ENLARGED_NODE;
            nodes[a].flags |= (nodes[b].flags | nodes[d].flags) & ENLARGED_NODE;
        } else {
            // Swap C and E
            nodes[a].children.child2 = i_e;
            nodes[b].children.child2 = i_c;

            nodes[c].parent = i_b;
            nodes[e].parent = i_a;

            nodes[b].aabb = aabb_cd;

            nodes[b].height = 1 + max_u16(nodes[c].height, nodes[d].height);
            nodes[a].height = 1 + max_u16(nodes[b].height, nodes[e].height);
            nodes[b].category_bits = nodes[c].category_bits | nodes[d].category_bits;
            nodes[a].category_bits = nodes[b].category_bits | nodes[e].category_bits;
            nodes[b].flags |= (nodes[c].flags | nodes[d].flags) & ENLARGED_NODE;
            nodes[a].flags |= (nodes[b].flags | nodes[e].flags) & ENLARGED_NODE;
        }
    } else if !is_leaf_b && !is_leaf_c {
        // All grand children exist so there are many options for rotation
        let i_d = nodes[b].children.child1;
        let i_e = nodes[b].children.child2;
        let i_f = nodes[c].children.child1;
        let i_g = nodes[c].children.child2;

        b3_assert!(0 <= i_d && i_d < node_capacity);
        b3_assert!(0 <= i_e && i_e < node_capacity);
        b3_assert!(0 <= i_f && i_f < node_capacity);
        b3_assert!(0 <= i_g && i_g < node_capacity);

        let d = i_d as usize;
        let e = i_e as usize;
        let f = i_f as usize;
        let g = i_g as usize;

        // Base cost
        let area_b = perimeter(nodes[b].aabb);
        let area_c = perimeter(nodes[c].aabb);
        let cost_base = area_b + area_c;
        let mut best_rotation = RotateType::None;
        let mut best_cost = cost_base;

        // Cost of swapping B and F
        let aabb_bg = aabb_union(nodes[b].aabb, nodes[g].aabb);
        let cost_bf = area_b + perimeter(aabb_bg);
        if cost_bf < best_cost {
            best_rotation = RotateType::BF;
            best_cost = cost_bf;
        }

        // Cost of swapping B and G
        let aabb_bf = aabb_union(nodes[b].aabb, nodes[f].aabb);
        let cost_bg = area_b + perimeter(aabb_bf);
        if cost_bg < best_cost {
            best_rotation = RotateType::BG;
            best_cost = cost_bg;
        }

        // Cost of swapping C and D
        let aabb_ce = aabb_union(nodes[c].aabb, nodes[e].aabb);
        let cost_cd = area_c + perimeter(aabb_ce);
        if cost_cd < best_cost {
            best_rotation = RotateType::CD;
            best_cost = cost_cd;
        }

        // Cost of swapping C and E
        let aabb_cd = aabb_union(nodes[c].aabb, nodes[d].aabb);
        let cost_ce = area_c + perimeter(aabb_cd);
        if cost_ce < best_cost {
            best_rotation = RotateType::CE;
            // best_cost = cost_ce;
        }

        match best_rotation {
            RotateType::None => {}

            RotateType::BF => {
                nodes[a].children.child1 = i_f;
                nodes[c].children.child1 = i_b;

                nodes[b].parent = i_c;
                nodes[f].parent = i_a;

                nodes[c].aabb = aabb_bg;

                nodes[c].height = 1 + max_u16(nodes[b].height, nodes[g].height);
                nodes[a].height = 1 + max_u16(nodes[c].height, nodes[f].height);
                nodes[c].category_bits = nodes[b].category_bits | nodes[g].category_bits;
                nodes[a].category_bits = nodes[c].category_bits | nodes[f].category_bits;
                nodes[c].flags |= (nodes[b].flags | nodes[g].flags) & ENLARGED_NODE;
                nodes[a].flags |= (nodes[c].flags | nodes[f].flags) & ENLARGED_NODE;
            }

            RotateType::BG => {
                nodes[a].children.child1 = i_g;
                nodes[c].children.child2 = i_b;

                nodes[b].parent = i_c;
                nodes[g].parent = i_a;

                nodes[c].aabb = aabb_bf;

                nodes[c].height = 1 + max_u16(nodes[b].height, nodes[f].height);
                nodes[a].height = 1 + max_u16(nodes[c].height, nodes[g].height);
                nodes[c].category_bits = nodes[b].category_bits | nodes[f].category_bits;
                nodes[a].category_bits = nodes[c].category_bits | nodes[g].category_bits;
                nodes[c].flags |= (nodes[b].flags | nodes[f].flags) & ENLARGED_NODE;
                nodes[a].flags |= (nodes[c].flags | nodes[g].flags) & ENLARGED_NODE;
            }

            RotateType::CD => {
                nodes[a].children.child2 = i_d;
                nodes[b].children.child1 = i_c;

                nodes[c].parent = i_b;
                nodes[d].parent = i_a;

                nodes[b].aabb = aabb_ce;

                nodes[b].height = 1 + max_u16(nodes[c].height, nodes[e].height);
                nodes[a].height = 1 + max_u16(nodes[b].height, nodes[d].height);
                nodes[b].category_bits = nodes[c].category_bits | nodes[e].category_bits;
                nodes[a].category_bits = nodes[b].category_bits | nodes[d].category_bits;
                nodes[b].flags |= (nodes[c].flags | nodes[e].flags) & ENLARGED_NODE;
                nodes[a].flags |= (nodes[b].flags | nodes[d].flags) & ENLARGED_NODE;
            }

            RotateType::CE => {
                nodes[a].children.child2 = i_e;
                nodes[b].children.child2 = i_c;

                nodes[c].parent = i_b;
                nodes[e].parent = i_a;

                nodes[b].aabb = aabb_cd;

                nodes[b].height = 1 + max_u16(nodes[c].height, nodes[d].height);
                nodes[a].height = 1 + max_u16(nodes[b].height, nodes[e].height);
                nodes[b].category_bits = nodes[c].category_bits | nodes[d].category_bits;
                nodes[a].category_bits = nodes[b].category_bits | nodes[e].category_bits;
                nodes[b].flags |= (nodes[c].flags | nodes[d].flags) & ENLARGED_NODE;
                nodes[a].flags |= (nodes[b].flags | nodes[e].flags) & ENLARGED_NODE;
            }
        }
    }
}

// It would be nicer if the root had zero height but maintaining this would drastically increase
// insertion cost because whole sub-trees would need the height to be updated.
fn insert_leaf(tree: &mut DynamicTree, leaf: i32, should_rotate: bool) {
    if tree.root == NULL_INDEX {
        tree.root = leaf;
        tree.nodes[tree.root as usize].parent = NULL_INDEX;
        return;
    }

    // Stage 1: find the best sibling for this node
    let leaf_aabb = tree.nodes[leaf as usize].aabb;
    let sibling = find_best_sibling(tree, leaf_aabb);

    // Stage 2: create a new parent for the leaf and sibling
    let old_parent = tree.nodes[sibling as usize].parent;
    let new_parent = allocate_node(tree);

    // warning: node pool can grow after allocation
    let nodes = &mut tree.nodes;
    let np = new_parent as usize;
    nodes[np].parent = old_parent;
    nodes[np].user_data = u64::MAX;
    nodes[np].aabb = aabb_union(leaf_aabb, nodes[sibling as usize].aabb);
    nodes[np].category_bits = nodes[leaf as usize].category_bits | nodes[sibling as usize].category_bits;
    nodes[np].height = nodes[sibling as usize].height + 1;

    if old_parent != NULL_INDEX {
        // The sibling was not the root.
        if nodes[old_parent as usize].children.child1 == sibling {
            nodes[old_parent as usize].children.child1 = new_parent;
        } else {
            nodes[old_parent as usize].children.child2 = new_parent;
        }

        nodes[np].children.child1 = sibling;
        nodes[np].children.child2 = leaf;
        nodes[sibling as usize].parent = new_parent;
        nodes[leaf as usize].parent = new_parent;
    } else {
        // The sibling was the root.
        nodes[np].children.child1 = sibling;
        nodes[np].children.child2 = leaf;
        nodes[sibling as usize].parent = new_parent;
        nodes[leaf as usize].parent = new_parent;
        tree.root = new_parent;
    }

    // Stage 3: walk back up the tree fixing heights and AABBs
    let mut index = tree.nodes[leaf as usize].parent;
    while index != NULL_INDEX {
        let i = index as usize;
        let child1 = tree.nodes[i].children.child1;
        let child2 = tree.nodes[i].children.child2;

        b3_assert!(child1 != NULL_INDEX);
        b3_assert!(child2 != NULL_INDEX);

        tree.nodes[i].aabb = aabb_union(tree.nodes[child1 as usize].aabb, tree.nodes[child2 as usize].aabb);
        tree.nodes[i].category_bits =
            tree.nodes[child1 as usize].category_bits | tree.nodes[child2 as usize].category_bits;
        tree.nodes[i].height = 1 + max_u16(tree.nodes[child1 as usize].height, tree.nodes[child2 as usize].height);
        tree.nodes[i].flags |= (tree.nodes[child1 as usize].flags | tree.nodes[child2 as usize].flags) & ENLARGED_NODE;

        if should_rotate {
            rotate_nodes(tree, index);
        }

        index = tree.nodes[i].parent;
    }
}

fn remove_leaf(tree: &mut DynamicTree, leaf: i32) {
    if leaf == tree.root {
        tree.root = NULL_INDEX;
        return;
    }

    let parent = tree.nodes[leaf as usize].parent;
    let grand_parent = tree.nodes[parent as usize].parent;
    let sibling = if tree.nodes[parent as usize].children.child1 == leaf {
        tree.nodes[parent as usize].children.child2
    } else {
        tree.nodes[parent as usize].children.child1
    };

    if grand_parent != NULL_INDEX {
        // Destroy parent and connect sibling to grandParent.
        if tree.nodes[grand_parent as usize].children.child1 == parent {
            tree.nodes[grand_parent as usize].children.child1 = sibling;
        } else {
            tree.nodes[grand_parent as usize].children.child2 = sibling;
        }
        tree.nodes[sibling as usize].parent = grand_parent;
        free_node(tree, parent);

        // Adjust ancestor bounds.
        let mut index = grand_parent;
        while index != NULL_INDEX {
            let i = index as usize;
            let child1 = tree.nodes[i].children.child1;
            let child2 = tree.nodes[i].children.child2;

            tree.nodes[i].aabb = aabb_union(tree.nodes[child1 as usize].aabb, tree.nodes[child2 as usize].aabb);
            tree.nodes[i].category_bits =
                tree.nodes[child1 as usize].category_bits | tree.nodes[child2 as usize].category_bits;
            tree.nodes[i].height =
                1 + max_u16(tree.nodes[child1 as usize].height, tree.nodes[child2 as usize].height);

            index = tree.nodes[i].parent;
        }
    } else {
        tree.root = sibling;
        tree.nodes[sibling as usize].parent = NULL_INDEX;
        free_node(tree, parent);
    }
}

/// Create a proxy in the tree as a leaf node. We return the index of the node instead
/// of a pointer so that we can grow the node pool.
pub fn dynamic_tree_create_proxy(tree: &mut DynamicTree, aabb: AABB, category_bits: u64, user_data: u64) -> i32 {
    b3_assert!(is_valid_aabb(aabb));

    let proxy_id = allocate_node(tree);
    let node = &mut tree.nodes[proxy_id as usize];

    node.aabb = aabb;
    node.user_data = user_data;
    node.category_bits = category_bits;
    node.height = 0;
    node.flags = ALLOCATED_NODE | LEAF_NODE;

    let should_rotate = true;
    insert_leaf(tree, proxy_id, should_rotate);

    tree.proxy_count += 1;

    proxy_id
}

/// Destroy a proxy. This asserts if the id is invalid.
pub fn dynamic_tree_destroy_proxy(tree: &mut DynamicTree, proxy_id: i32) {
    b3_assert!(0 <= proxy_id && proxy_id < tree.node_capacity);
    b3_assert!(is_leaf(&tree.nodes[proxy_id as usize]));

    remove_leaf(tree, proxy_id);
    free_node(tree, proxy_id);

    b3_assert!(tree.proxy_count > 0);
    tree.proxy_count -= 1;
}

/// Get the number of proxies created.
pub fn dynamic_tree_get_proxy_count(tree: &DynamicTree) -> i32 {
    tree.proxy_count
}

/// Move a proxy to a new AABB by removing and reinserting into the tree.
pub fn dynamic_tree_move_proxy(tree: &mut DynamicTree, proxy_id: i32, aabb: AABB) {
    b3_assert!(is_valid_aabb(aabb));
    b3_assert!(0 <= proxy_id && proxy_id < tree.node_capacity);
    b3_assert!(is_leaf(&tree.nodes[proxy_id as usize]));

    remove_leaf(tree, proxy_id);

    tree.nodes[proxy_id as usize].aabb = aabb;

    let should_rotate = false;
    insert_leaf(tree, proxy_id, should_rotate);
}

/// Enlarge a proxy and enlarge ancestors as necessary.
pub fn dynamic_tree_enlarge_proxy(tree: &mut DynamicTree, proxy_id: i32, aabb: AABB) {
    b3_validate!(is_valid_aabb(aabb));
    b3_assert!(0 <= proxy_id && proxy_id < tree.node_capacity);
    b3_validate!(is_leaf(&tree.nodes[proxy_id as usize]));

    // Caller must ensure this
    b3_validate!(!aabb_contains(tree.nodes[proxy_id as usize].aabb, aabb));

    tree.nodes[proxy_id as usize].aabb = aabb;

    let mut parent_index = tree.nodes[proxy_id as usize].parent;
    while parent_index != NULL_INDEX {
        let i = parent_index as usize;
        let changed = enlarge_aabb(&mut tree.nodes[i].aabb, aabb);

        // todo not sure why this node is marked as enlarged even if it didn't change
        tree.nodes[i].flags |= ENLARGED_NODE;

        parent_index = tree.nodes[i].parent;

        if !changed {
            break;
        }
    }

    while parent_index != NULL_INDEX {
        let i = parent_index as usize;
        if tree.nodes[i].flags & ENLARGED_NODE != 0 {
            // early out because this ancestor was previously ascended and marked as enlarged
            break;
        }

        tree.nodes[i].flags |= ENLARGED_NODE;
        parent_index = tree.nodes[i].parent;
    }
}

/// Modify the category bits on a proxy. This is an expensive operation.
pub fn dynamic_tree_set_category_bits(tree: &mut DynamicTree, proxy_id: i32, category_bits: u64) {
    b3_assert!(is_leaf(&tree.nodes[proxy_id as usize]));

    tree.nodes[proxy_id as usize].category_bits = category_bits;

    // Fix up category bits in ancestor internal nodes
    let mut node_index = tree.nodes[proxy_id as usize].parent;
    while node_index != NULL_INDEX {
        let i = node_index as usize;
        let child1 = tree.nodes[i].children.child1;
        b3_assert!(child1 != NULL_INDEX);
        let child2 = tree.nodes[i].children.child2;
        b3_assert!(child2 != NULL_INDEX);
        tree.nodes[i].category_bits =
            tree.nodes[child1 as usize].category_bits | tree.nodes[child2 as usize].category_bits;

        node_index = tree.nodes[i].parent;
    }
}

/// Get the category bits on a proxy.
pub fn dynamic_tree_get_category_bits(tree: &DynamicTree, proxy_id: i32) -> u64 {
    b3_assert!(0 <= proxy_id && proxy_id < tree.node_capacity);
    tree.nodes[proxy_id as usize].category_bits
}

/// Get the height of the binary tree.
pub fn dynamic_tree_get_height(tree: &DynamicTree) -> i32 {
    if tree.root == NULL_INDEX {
        return 0;
    }

    tree.nodes[tree.root as usize].height as i32
}

/// Get the ratio of the sum of the node areas to the root area.
pub fn dynamic_tree_get_area_ratio(tree: &DynamicTree) -> f32 {
    if tree.root == NULL_INDEX {
        return 0.0;
    }

    let root = &tree.nodes[tree.root as usize];
    let root_area = perimeter(root.aabb);

    let mut total_area = 0.0f32;
    for i in 0..tree.node_capacity {
        let node = &tree.nodes[i as usize];
        if !is_allocated(node) || is_leaf(node) || i == tree.root {
            continue;
        }

        total_area += perimeter(node.aabb);
    }

    total_area / root_area
}

/// Get the bounding box that contains the entire tree.
pub fn dynamic_tree_get_root_bounds(tree: &DynamicTree) -> AABB {
    if tree.root != NULL_INDEX {
        return tree.nodes[tree.root as usize].aabb;
    }

    AABB {
        lower_bound: Vec3::ZERO,
        upper_bound: Vec3::ZERO,
    }
}

// Compute the height of a sub-tree.
fn compute_height_recurse(tree: &DynamicTree, node_id: i32) -> i32 {
    b3_assert!(0 <= node_id && node_id < tree.node_capacity);
    let node = &tree.nodes[node_id as usize];

    if is_leaf(node) {
        return 0;
    }

    let height1 = compute_height_recurse(tree, node.children.child1);
    let height2 = compute_height_recurse(tree, node.children.child2);
    1 + max_int(height1, height2)
}

fn compute_height(tree: &DynamicTree) -> i32 {
    compute_height_recurse(tree, tree.root)
}

fn validate_structure(tree: &DynamicTree, index: i32) {
    if index == NULL_INDEX {
        return;
    }

    if index == tree.root {
        b3_assert!(tree.nodes[index as usize].parent == NULL_INDEX);
    }

    let node = &tree.nodes[index as usize];

    b3_assert!(node.flags == 0 || (node.flags & ALLOCATED_NODE) != 0);

    if is_leaf(node) {
        b3_assert!(node.height == 0);
        return;
    }

    let child1 = node.children.child1;
    let child2 = node.children.child2;

    b3_assert!(0 <= child1 && child1 < tree.node_capacity);
    b3_assert!(0 <= child2 && child2 < tree.node_capacity);

    b3_assert!(tree.nodes[child1 as usize].parent == index);
    b3_assert!(tree.nodes[child2 as usize].parent == index);

    if (tree.nodes[child1 as usize].flags | tree.nodes[child2 as usize].flags) & ENLARGED_NODE != 0 {
        b3_assert!(node.flags & ENLARGED_NODE != 0);
    }

    validate_structure(tree, child1);
    validate_structure(tree, child2);
}

fn validate_metrics(tree: &DynamicTree, index: i32) {
    if index == NULL_INDEX {
        return;
    }

    let node = &tree.nodes[index as usize];

    b3_validate!(is_valid_aabb(node.aabb));

    if is_leaf(node) {
        b3_assert!(node.height == 0);
        return;
    }

    let child1 = node.children.child1;
    let child2 = node.children.child2;

    b3_assert!(0 <= child1 && child1 < tree.node_capacity);
    b3_assert!(0 <= child2 && child2 < tree.node_capacity);

    let height1 = tree.nodes[child1 as usize].height;
    let height2 = tree.nodes[child2 as usize].height;
    let height = 1 + max_u16(height1, height2);
    b3_assert!(node.height == height);

    b3_assert!(aabb_contains(node.aabb, tree.nodes[child1 as usize].aabb));
    b3_assert!(aabb_contains(node.aabb, tree.nodes[child2 as usize].aabb));

    let category_bits = tree.nodes[child1 as usize].category_bits | tree.nodes[child2 as usize].category_bits;
    b3_assert!(node.category_bits == category_bits);

    validate_metrics(tree, child1);
    validate_metrics(tree, child2);
}

/// Validate this tree. For testing. Active in debug builds only (C: B3_ENABLE_VALIDATION).
pub fn dynamic_tree_validate(tree: &DynamicTree) {
    if !cfg!(debug_assertions) {
        return;
    }

    if tree.root == NULL_INDEX {
        return;
    }

    validate_structure(tree, tree.root);
    validate_metrics(tree, tree.root);

    let mut free_count = 0;
    let mut free_index = tree.free_list;
    while free_index != NULL_INDEX {
        b3_assert!(0 <= free_index && free_index < tree.node_capacity);
        free_index = tree.nodes[free_index as usize].parent;
        free_count += 1;
    }

    let height = dynamic_tree_get_height(tree);
    let computed_height = compute_height(tree);
    b3_assert!(height == computed_height);

    b3_assert!(tree.node_count + free_count == tree.node_capacity);
}

/// Validate this tree has no enlarged AABBs. For testing. Active in debug builds only.
pub fn dynamic_tree_validate_no_enlarged(tree: &DynamicTree) {
    if !cfg!(debug_assertions) {
        return;
    }

    let capacity = tree.node_capacity;
    for i in 0..capacity {
        let node = &tree.nodes[i as usize];
        if node.flags & ALLOCATED_NODE != 0 {
            b3_assert!((node.flags & ENLARGED_NODE) == 0);
        }
    }
}

/// Get the number of bytes used by this tree.
pub fn dynamic_tree_get_byte_count(tree: &DynamicTree) -> i32 {
    let size = std::mem::size_of::<DynamicTree>()
        + std::mem::size_of::<TreeNode>() * tree.node_capacity as usize
        + tree.rebuild_capacity as usize
            * (std::mem::size_of::<i32>()
                + std::mem::size_of::<AABB>()
                + std::mem::size_of::<Vec3>()
                + std::mem::size_of::<i32>());

    size as i32
}

/// Query an AABB for overlapping proxies. The callback is called for each proxy
/// that overlaps the supplied AABB. Return false from the callback to terminate.
pub fn dynamic_tree_query(
    tree: &DynamicTree,
    aabb: AABB,
    mask_bits: u64,
    require_all_bits: bool,
    callback: &mut dyn FnMut(i32, u64) -> bool,
) -> TreeStats {
    let mut result = TreeStats::default();

    if tree.node_count == 0 {
        return result;
    }

    let mut stack = [0i32; TREE_STACK_SIZE];
    let mut stack_count = 0usize;
    stack[stack_count] = tree.root;
    stack_count += 1;

    while stack_count > 0 {
        stack_count -= 1;
        let node_id = stack[stack_count];
        if node_id == NULL_INDEX {
            // todo huh?
            b3_assert!(false);
            continue;
        }

        let node = &tree.nodes[node_id as usize];
        result.node_visits += 1;

        // Assuming branch prediction deals with requireAllBits well
        let bit_match = if require_all_bits {
            ((node.category_bits & mask_bits) == mask_bits) as u64
        } else {
            node.category_bits & mask_bits
        };

        if bit_match != 0 && aabb_overlaps(node.aabb, aabb) {
            if is_leaf(node) {
                // callback to user code with proxy id
                let proceed = callback(node_id, node.user_data);
                result.leaf_visits += 1;

                if !proceed {
                    return result;
                }
            } else {
                b3_assert!(stack_count < TREE_STACK_SIZE - 1);
                if stack_count < TREE_STACK_SIZE - 1 {
                    stack[stack_count] = node.children.child1;
                    stack_count += 1;
                    stack[stack_count] = node.children.child2;
                    stack_count += 1;
                }
            }
        }
    }

    result
}

#[inline(always)]
fn distance_to_node_sqr(point: Vec3, node: &TreeNode) -> f32 {
    let r = sub(point, clamp(point, node.aabb.lower_bound, node.aabb.upper_bound));
    dot(r, r)
}

#[derive(Clone, Copy, Default)]
struct QueryClosestItem {
    node_index: i32,
    distance_to_node_sqr: f32,
}

/// Query an AABB for the closest object. The callback receives the minimum distance
/// squared so far and the proxy to check, and returns the new minimum distance squared.
pub fn dynamic_tree_query_closest(
    tree: &DynamicTree,
    point: Vec3,
    mask_bits: u64,
    require_all_bits: bool,
    callback: &mut dyn FnMut(f32, i32, u64) -> f32,
    min_distance_sqr: &mut f32,
) -> TreeStats {
    let mut result = TreeStats::default();

    if tree.node_count == 0 {
        return result;
    }

    let mut min_sqr = *min_distance_sqr;
    let mut stack = [QueryClosestItem::default(); TREE_STACK_SIZE];
    let mut stack_count = 0usize;

    let root_distance_sqr = distance_to_node_sqr(point, &tree.nodes[tree.root as usize]);
    stack[stack_count] = QueryClosestItem {
        node_index: tree.root,
        distance_to_node_sqr: root_distance_sqr,
    };
    stack_count += 1;

    while stack_count > 0 {
        stack_count -= 1;
        let item = stack[stack_count];
        let node = &tree.nodes[item.node_index as usize];
        result.node_visits += 1;

        let bit_match = if require_all_bits {
            ((node.category_bits & mask_bits) == mask_bits) as u64
        } else {
            node.category_bits & mask_bits
        };

        if bit_match != 0 && item.distance_to_node_sqr < min_sqr {
            if is_leaf(node) {
                // callback to user code with minimum distance squared so far and proxy id
                let dd = callback(min_sqr, item.node_index, node.user_data);

                if dd < min_sqr {
                    min_sqr = dd;
                }

                result.leaf_visits += 1;
            } else {
                b3_assert!(stack_count < TREE_STACK_SIZE - 1);
                if stack_count < TREE_STACK_SIZE - 1 {
                    let child1 = node.children.child1;
                    let child2 = node.children.child2;

                    // Store the distance to node in the stack instead of recomputing after pop
                    let item1 = QueryClosestItem {
                        node_index: child1,
                        distance_to_node_sqr: distance_to_node_sqr(point, &tree.nodes[child1 as usize]),
                    };

                    let item2 = QueryClosestItem {
                        node_index: child2,
                        distance_to_node_sqr: distance_to_node_sqr(point, &tree.nodes[child2 as usize]),
                    };

                    // Ensure we iterate the closest child first as we pop off the stack
                    if item2.distance_to_node_sqr < item1.distance_to_node_sqr {
                        stack[stack_count] = item1;
                        stack_count += 1;
                        stack[stack_count] = item2;
                        stack_count += 1;
                    } else {
                        stack[stack_count] = item2;
                        stack_count += 1;
                        stack[stack_count] = item1;
                        stack_count += 1;
                    }
                }
            }
        }
    }

    *min_distance_sqr = min_sqr;

    result
}

// Test a ray for edge separation with an AABB (Gino, p80).
// Scalar port of b3TestBoundsRayOverlap from simd.h (B3_SIMD_NONE path).
fn test_bounds_ray_overlap(node_min: Vec3, node_max: Vec3, ray_start: Vec3, ray_delta: Vec3) -> bool {
    // Setup node
    let node_center = mul_sv(0.5, add(node_min, node_max));
    let node_extent = sub(node_max, node_center);

    // Setup ray
    let ray_start = sub(ray_start, node_center);

    // SAT: Edge separation
    let edge_separation = sub(
        abs(cross(ray_delta, ray_start)),
        modified_cross(abs(ray_delta), node_extent),
    );
    edge_separation.x <= 0.0 && edge_separation.y <= 0.0 && edge_separation.z <= 0.0
}

/// Ray cast against the proxies in the tree. This relies on the callback
/// to perform an exact ray cast in the case where the proxy contains a shape.
/// The callback also performs any collision filtering.
/// Callback return semantics: 0 terminates, a value in (0, max_fraction] clips the
/// ray, -1 (or any other value) skips the shape and continues.
pub fn dynamic_tree_ray_cast(
    tree: &DynamicTree,
    input: &RayCastInput,
    mask_bits: u64,
    require_all_bits: bool,
    callback: &mut dyn FnMut(&RayCastInput, i32, u64) -> f32,
) -> TreeStats {
    let mut result = TreeStats::default();

    if tree.node_count == 0 {
        return result;
    }

    let p1 = input.origin;
    let d = input.translation;

    // In C these are SIMD registers (b3LoadV); scalar mode uses the Vec3 directly.
    let pv1 = p1;
    let dv = d;

    let mut max_fraction = input.max_fraction;

    let mut p2 = mul_add(p1, max_fraction, d);

    // Build a bounding box for the segment.
    let mut segment_aabb = AABB {
        lower_bound: min(p1, p2),
        upper_bound: max(p1, p2),
    };

    let mut stack = [0i32; TREE_STACK_SIZE];
    let mut stack_count = 0usize;
    stack[stack_count] = tree.root;
    stack_count += 1;

    let nodes = &tree.nodes;

    let mut sub_input = *input;

    while stack_count > 0 {
        stack_count -= 1;
        let node_id = stack[stack_count];
        if node_id == NULL_INDEX {
            // todo is this possible?
            b3_assert!(false);
            continue;
        }

        let node = &nodes[node_id as usize];
        result.node_visits += 1;

        let node_aabb = node.aabb;

        let bit_match = if require_all_bits {
            ((node.category_bits & mask_bits) == mask_bits) as u64
        } else {
            node.category_bits & mask_bits
        };

        if bit_match == 0 || !aabb_overlaps(node_aabb, segment_aabb) {
            continue;
        }

        let lower = node_aabb.lower_bound;
        let upper = node_aabb.upper_bound;

        let edge_overlap = test_bounds_ray_overlap(lower, upper, pv1, dv);
        if !edge_overlap {
            continue;
        }

        if is_leaf(node) {
            sub_input.max_fraction = max_fraction;

            let value = callback(&sub_input, node_id, node.user_data);
            result.leaf_visits += 1;

            // The user may return -1 to indicate this shape should be skipped

            if value == 0.0 {
                // The client has terminated the ray cast.
                return result;
            }

            if 0.0 < value && value <= max_fraction {
                // Update segment bounding box.
                max_fraction = value;
                p2 = mul_add(p1, max_fraction, d);
                segment_aabb.lower_bound = min(p1, p2);
                segment_aabb.upper_bound = max(p1, p2);
            }
        } else {
            b3_assert!(stack_count < TREE_STACK_SIZE - 1);
            if stack_count < TREE_STACK_SIZE - 1 {
                let c1 = aabb_center(nodes[node.children.child1 as usize].aabb);
                let c2 = aabb_center(nodes[node.children.child2 as usize].aabb);
                if distance_squared(c1, p1) < distance_squared(c2, p1) {
                    stack[stack_count] = node.children.child2;
                    stack_count += 1;
                    stack[stack_count] = node.children.child1;
                    stack_count += 1;
                } else {
                    stack[stack_count] = node.children.child1;
                    stack_count += 1;
                    stack[stack_count] = node.children.child2;
                    stack_count += 1;
                }
            }
        }
    }

    result
}

/// Sweep an AABB through the tree. The caller folds the cast shape radius and any
/// world origin into the box, so the tree traversal stays a conservative box sweep
/// and the precise narrow phase happens per shape in the callback.
pub fn dynamic_tree_box_cast(
    tree: &DynamicTree,
    input: &BoxCastInput,
    mask_bits: u64,
    require_all_bits: bool,
    callback: &mut dyn FnMut(&BoxCastInput, i32, u64) -> f32,
) -> TreeStats {
    let mut stats = TreeStats::default();

    if tree.node_count == 0 {
        return stats;
    }

    // The caller folds the shape radius and the world origin into the box
    let origin_aabb = input.box_;

    let p1 = aabb_center(origin_aabb);
    let extension = aabb_extents(origin_aabb);

    let d = input.translation;

    let pv1 = p1;
    let dv = d;
    let ev = extension;

    let mut max_fraction = input.max_fraction;

    // Build total box for the cast
    let mut t = mul_sv(max_fraction, input.translation);
    let mut total_aabb = AABB {
        lower_bound: min(origin_aabb.lower_bound, add(origin_aabb.lower_bound, t)),
        upper_bound: max(origin_aabb.upper_bound, add(origin_aabb.upper_bound, t)),
    };

    let mut sub_input = *input;
    let nodes = &tree.nodes;

    let mut stack = [0i32; TREE_STACK_SIZE];
    let mut stack_count = 0usize;
    stack[stack_count] = tree.root;
    stack_count += 1;

    while stack_count > 0 {
        stack_count -= 1;
        let node_id = stack[stack_count];
        if node_id == NULL_INDEX {
            b3_assert!(false);
            continue;
        }

        let node = &nodes[node_id as usize];
        stats.node_visits += 1;

        let bit_match = if require_all_bits {
            ((node.category_bits & mask_bits) == mask_bits) as u64
        } else {
            node.category_bits & mask_bits
        };

        if bit_match == 0 || !aabb_overlaps(node.aabb, total_aabb) {
            continue;
        }

        // radius extension is added to the node in this case
        let lower = sub(node.aabb.lower_bound, ev);
        let upper = add(node.aabb.upper_bound, ev);
        let edge_overlap = test_bounds_ray_overlap(lower, upper, pv1, dv);
        if !edge_overlap {
            continue;
        }

        if is_leaf(node) {
            sub_input.max_fraction = max_fraction;

            let value = callback(&sub_input, node_id, node.user_data);
            stats.leaf_visits += 1;

            if value == 0.0 {
                // The client has terminated the cast.
                return stats;
            }

            if 0.0 < value && value < max_fraction {
                max_fraction = value;
                t = mul_sv(max_fraction, input.translation);
                total_aabb.lower_bound = min(origin_aabb.lower_bound, add(origin_aabb.lower_bound, t));
                total_aabb.upper_bound = max(origin_aabb.upper_bound, add(origin_aabb.upper_bound, t));
            }
        } else {
            b3_assert!(stack_count < TREE_STACK_SIZE - 1);
            if stack_count < TREE_STACK_SIZE - 1 {
                let c1 = aabb_center(nodes[node.children.child1 as usize].aabb);
                let c2 = aabb_center(nodes[node.children.child2 as usize].aabb);
                if distance_squared(c1, p1) < distance_squared(c2, p1) {
                    stack[stack_count] = node.children.child2;
                    stack_count += 1;
                    stack[stack_count] = node.children.child1;
                    stack_count += 1;
                } else {
                    stack[stack_count] = node.children.child1;
                    stack_count += 1;
                    stack[stack_count] = node.children.child2;
                    stack_count += 1;
                }
            }
        }
    }

    stats
}

// Median split heuristic (B3_TREE_HEURISTIC == 0 in C; the SAH path is compiled out
// upstream and is not ported).
fn partition_mid(indices: &mut [i32], centers: &mut [Vec3]) -> i32 {
    let count = indices.len();

    // Handle trivial case
    if count <= 2 {
        return (count / 2) as i32;
    }

    let mut lower_bound = centers[0];
    let mut upper_bound = centers[0];

    for i in 1..count {
        lower_bound = min(lower_bound, centers[i]);
        upper_bound = max(upper_bound, centers[i]);
    }

    let d = sub(upper_bound, lower_bound);
    let c = mul_sv(0.5, add(lower_bound, upper_bound));

    // Partition longest axis using the Hoare partition scheme
    // https://en.wikipedia.org/wiki/Quicksort
    // https://nicholasvadivelu.com/2021/01/11/array-partition/
    let (axis, pivot) = if d.x >= d.y && d.x >= d.z {
        (0usize, c.x)
    } else if d.y >= d.z {
        (1usize, c.y)
    } else {
        (2usize, c.z)
    };

    let get = |v: Vec3| -> f32 {
        match axis {
            0 => v.x,
            1 => v.y,
            _ => v.z,
        }
    };

    let mut i1 = 0usize;
    let mut i2 = count;

    while i1 < i2 {
        while i1 < i2 && get(centers[i1]) < pivot {
            i1 += 1;
        }

        while i1 < i2 && get(centers[i2 - 1]) >= pivot {
            i2 -= 1;
        }

        if i1 < i2 {
            // Swap indices
            indices.swap(i1, i2 - 1);

            // Swap centers
            centers.swap(i1, i2 - 1);

            i1 += 1;
            i2 -= 1;
        }
    }
    b3_assert!(i1 == i2);

    if i1 > 0 && i1 < count {
        i1 as i32
    } else {
        (count / 2) as i32
    }
}

// Temporary data used to track the rebuild of a tree node
#[derive(Clone, Copy, Default)]
struct RebuildItem {
    node_index: i32,
    child_count: i32,

    // Leaf indices
    start_index: i32,
    split_index: i32,
    end_index: i32,
}

// Returns root node index
fn build_tree(tree: &mut DynamicTree, leaf_count: i32) -> i32 {
    // The leaf arrays are owned by the tree but the node pool may grow during the
    // build; take them out to sidestep the aliasing (C uses raw pointers that
    // remain valid because these arrays do not grow during the build).
    let mut leaf_indices = std::mem::take(&mut tree.leaf_indices);
    let mut leaf_centers = std::mem::take(&mut tree.leaf_centers);

    if leaf_count == 1 {
        tree.nodes[leaf_indices[0] as usize].parent = NULL_INDEX;
        let root = leaf_indices[0];
        tree.leaf_indices = leaf_indices;
        tree.leaf_centers = leaf_centers;
        return root;
    }

    // todo large stack item
    let mut stack = [RebuildItem::default(); TREE_STACK_SIZE];
    let mut top = 0usize;

    stack[0] = RebuildItem {
        node_index: allocate_node(tree),
        child_count: -1,
        start_index: 0,
        end_index: leaf_count,
        split_index: partition_mid(
            &mut leaf_indices[..leaf_count as usize],
            &mut leaf_centers[..leaf_count as usize],
        ),
    };

    loop {
        stack[top].child_count += 1;

        if stack[top].child_count == 2 {
            // This internal node has both children established

            if top == 0 {
                // all done
                break;
            }

            let item = stack[top];
            let parent_item = stack[top - 1];

            if parent_item.child_count == 0 {
                b3_assert!(tree.nodes[parent_item.node_index as usize].children.child1 == NULL_INDEX);
                tree.nodes[parent_item.node_index as usize].children.child1 = item.node_index;
            } else {
                b3_assert!(parent_item.child_count == 1);
                b3_assert!(tree.nodes[parent_item.node_index as usize].children.child2 == NULL_INDEX);
                tree.nodes[parent_item.node_index as usize].children.child2 = item.node_index;
            }

            let ni = item.node_index as usize;

            b3_assert!(tree.nodes[ni].parent == NULL_INDEX);
            tree.nodes[ni].parent = parent_item.node_index;

            b3_assert!(tree.nodes[ni].children.child1 != NULL_INDEX);
            b3_assert!(tree.nodes[ni].children.child2 != NULL_INDEX);
            let child1 = tree.nodes[ni].children.child1 as usize;
            let child2 = tree.nodes[ni].children.child2 as usize;

            tree.nodes[ni].aabb = aabb_union(tree.nodes[child1].aabb, tree.nodes[child2].aabb);
            tree.nodes[ni].height = 1 + max_u16(tree.nodes[child1].height, tree.nodes[child2].height);
            tree.nodes[ni].category_bits = tree.nodes[child1].category_bits | tree.nodes[child2].category_bits;

            // Pop stack
            top -= 1;
        } else {
            let item = stack[top];

            let (start_index, end_index) = if item.child_count == 0 {
                (item.start_index, item.split_index)
            } else {
                b3_assert!(item.child_count == 1);
                (item.split_index, item.end_index)
            };

            let count = end_index - start_index;

            if count == 1 {
                let child_index = leaf_indices[start_index as usize];
                let ni = item.node_index as usize;

                if item.child_count == 0 {
                    b3_assert!(tree.nodes[ni].children.child1 == NULL_INDEX);
                    tree.nodes[ni].children.child1 = child_index;
                } else {
                    b3_assert!(item.child_count == 1);
                    b3_assert!(tree.nodes[ni].children.child2 == NULL_INDEX);
                    tree.nodes[ni].children.child2 = child_index;
                }

                b3_assert!(tree.nodes[child_index as usize].parent == NULL_INDEX);
                tree.nodes[child_index as usize].parent = item.node_index;
            } else {
                b3_assert!(count > 0);
                b3_assert!(top < TREE_STACK_SIZE);

                top += 1;
                let split = partition_mid(
                    &mut leaf_indices[start_index as usize..end_index as usize],
                    &mut leaf_centers[start_index as usize..end_index as usize],
                ) + start_index;
                stack[top] = RebuildItem {
                    node_index: allocate_node(tree),
                    child_count: -1,
                    start_index,
                    end_index,
                    split_index: split,
                };
            }
        }
    }

    let root_index = stack[0].node_index as usize;
    b3_assert!(tree.nodes[root_index].parent == NULL_INDEX);
    b3_assert!(tree.nodes[root_index].children.child1 != NULL_INDEX);
    b3_assert!(tree.nodes[root_index].children.child2 != NULL_INDEX);

    let child1 = tree.nodes[root_index].children.child1 as usize;
    let child2 = tree.nodes[root_index].children.child2 as usize;

    tree.nodes[root_index].aabb = aabb_union(tree.nodes[child1].aabb, tree.nodes[child2].aabb);
    tree.nodes[root_index].height = 1 + max_u16(tree.nodes[child1].height, tree.nodes[child2].height);
    tree.nodes[root_index].category_bits =
        tree.nodes[child1].category_bits | tree.nodes[child2].category_bits;

    tree.leaf_indices = leaf_indices;
    tree.leaf_centers = leaf_centers;

    stack[0].node_index
}

/// Rebuild the tree while retaining subtrees that haven't changed. Returns the
/// number of boxes sorted. Not safe to access the tree during this operation
/// because it may grow.
pub fn dynamic_tree_rebuild(tree: &mut DynamicTree, full_build: bool) -> i32 {
    let proxy_count = tree.proxy_count;
    if proxy_count == 0 {
        return 0;
    }

    // Ensure capacity for rebuild space
    if proxy_count > tree.rebuild_capacity {
        let new_capacity = proxy_count + proxy_count / 2;

        tree.leaf_indices = vec![0; new_capacity as usize];
        tree.leaf_centers = vec![Vec3::ZERO; new_capacity as usize];
        tree.rebuild_capacity = new_capacity;
    }

    let mut leaf_count = 0i32;
    let mut stack = [0i32; TREE_STACK_SIZE];
    let mut stack_count = 0usize;

    let mut node_index = tree.root;

    // These are the nodes that get sorted to rebuild the tree.
    // I'm using indices because the node pool may grow during the build.
    let mut leaf_indices = std::mem::take(&mut tree.leaf_indices);
    let mut leaf_centers = std::mem::take(&mut tree.leaf_centers);

    // Gather all proxy nodes that have grown and all internal nodes that haven't grown. Both are
    // considered leaves in the tree rebuild.
    // Free all internal nodes that have grown.
    // todo use a node growth metric instead of simply enlarged to reduce rebuild size and frequency
    // this should be weighed against B3_MAX_AABB_MARGIN
    loop {
        let node = tree.nodes[node_index as usize];
        if is_leaf(&node) || ((node.flags & ENLARGED_NODE) == 0 && !full_build) {
            leaf_indices[leaf_count as usize] = node_index;
            leaf_centers[leaf_count as usize] = aabb_center(node.aabb);
            leaf_count += 1;

            // Detach
            tree.nodes[node_index as usize].parent = NULL_INDEX;
        } else {
            let doomed_node_index = node_index;

            // Handle children
            node_index = node.children.child1;

            b3_assert!(stack_count < TREE_STACK_SIZE);
            if stack_count < TREE_STACK_SIZE {
                stack[stack_count] = node.children.child2;
                stack_count += 1;
            }

            // Remove doomed node
            free_node(tree, doomed_node_index);

            continue;
        }

        if stack_count == 0 {
            break;
        }

        stack_count -= 1;
        node_index = stack[stack_count];
    }

    if cfg!(debug_assertions) {
        let capacity = tree.node_capacity;
        for i in 0..capacity {
            if tree.nodes[i as usize].flags & ALLOCATED_NODE != 0 {
                b3_assert!((tree.nodes[i as usize].flags & ENLARGED_NODE) == 0);
            }
        }
    }

    b3_assert!(leaf_count <= proxy_count);

    tree.leaf_indices = leaf_indices;
    tree.leaf_centers = leaf_centers;

    tree.root = build_tree(tree, leaf_count);

    dynamic_tree_validate(tree);

    leaf_count
}

/// PORT EXTENSION — not in upstream C. Bottom-up refit: recompute every
/// internal node's AABB as the union of its children and clear ENLARGED_NODE
/// on all nodes, keeping the existing topology. This is the cheap
/// (single O(n) post-order pass) alternative to the median `dynamic_tree_rebuild`
/// used by the adaptive broad-phase hybrid on high-churn steps. It preserves
/// query CORRECTNESS — the leaf (fat) AABBs are untouched, and each internal
/// AABB becomes the exact union of its descendants, which can never cause a
/// query to miss an overlapping leaf. It does NOT rebalance, so a periodic
/// full rebuild (the hybrid's cadence) is needed to keep query cost low.
pub fn refit_and_clear_enlarged(tree: &mut DynamicTree) {
    if tree.root == NULL_INDEX || tree.node_count == 0 {
        return;
    }

    let mut stack = std::mem::take(&mut tree.refit_stack);
    let mut order = std::mem::take(&mut tree.refit_order);
    stack.clear();
    order.clear();

    // Pre-order DFS: clear ENLARGED on every node; record internal nodes so we
    // can recompute their AABBs children-first (reverse pre-order is a valid
    // post-order — a node always precedes its descendants in pre-order).
    stack.push(tree.root);
    while let Some(node_id) = stack.pop() {
        let node = &mut tree.nodes[node_id as usize];
        node.flags &= !ENLARGED_NODE;
        if node.flags & LEAF_NODE == 0 {
            let child1 = node.children.child1;
            let child2 = node.children.child2;
            order.push(node_id);
            stack.push(child1);
            stack.push(child2);
        }
    }

    for i in (0..order.len()).rev() {
        let node_id = order[i];
        let (child1, child2) = {
            let node = &tree.nodes[node_id as usize];
            (node.children.child1, node.children.child2)
        };
        let a = tree.nodes[child1 as usize].aabb;
        let b = tree.nodes[child2 as usize].aabb;
        tree.nodes[node_id as usize].aabb = aabb_union(a, b);
    }

    tree.refit_stack = stack;
    tree.refit_order = order;
}

/// PORT EXTENSION — not in upstream C. Enumerate every unordered pair of leaves
/// in `tree` whose (fat) AABBs overlap, invoking `emit(leaf_a, leaf_b)` once per
/// pair (leaf ids are proxy ids). This is the self-BVTT used by the adaptive
/// broad-phase hybrid: one tree self-traversal in place of a per-moved-proxy
/// query. `stack` is caller-owned scratch (reused across calls).
pub fn dynamic_tree_self_pairs(tree: &DynamicTree, stack: &mut Vec<(i32, i32, bool)>, emit: &mut dyn FnMut(i32, i32)) {
    if tree.root == NULL_INDEX || tree.node_count == 0 {
        return;
    }
    stack.clear();
    stack.push((tree.root, tree.root, true));
    while let Some((a, b, is_self)) = stack.pop() {
        if is_self {
            let node = &tree.nodes[a as usize];
            if is_leaf(node) {
                continue;
            }
            let (c1, c2) = (node.children.child1, node.children.child2);
            stack.push((c1, c1, true));
            stack.push((c2, c2, true));
            stack.push((c1, c2, false));
        } else {
            let na = &tree.nodes[a as usize];
            let nb = &tree.nodes[b as usize];
            if !aabb_overlaps(na.aabb, nb.aabb) {
                continue;
            }
            let a_leaf = is_leaf(na);
            let b_leaf = is_leaf(nb);
            if a_leaf && b_leaf {
                emit(a, b);
            } else if b_leaf || (!a_leaf && perimeter(na.aabb) >= perimeter(nb.aabb)) {
                stack.push((na.children.child1, b, false));
                stack.push((na.children.child2, b, false));
            } else {
                stack.push((a, nb.children.child1, false));
                stack.push((a, nb.children.child2, false));
            }
        }
    }
}

/// PORT EXTENSION — not in upstream C. Enumerate overlapping leaf pairs across
/// two trees (`leaf_a` in `tree_a`, `leaf_b` in `tree_b`), `emit(leaf_a, leaf_b)`
/// once each. The cross-BVTT for dynamic-vs-static and dynamic-vs-kinematic.
pub fn dynamic_tree_cross_pairs(
    tree_a: &DynamicTree,
    tree_b: &DynamicTree,
    stack: &mut Vec<(i32, i32, bool)>,
    emit: &mut dyn FnMut(i32, i32),
) {
    if tree_a.root == NULL_INDEX || tree_a.node_count == 0 || tree_b.root == NULL_INDEX || tree_b.node_count == 0 {
        return;
    }
    stack.clear();
    stack.push((tree_a.root, tree_b.root, false));
    while let Some((a, b, _)) = stack.pop() {
        let na = &tree_a.nodes[a as usize];
        let nb = &tree_b.nodes[b as usize];
        if !aabb_overlaps(na.aabb, nb.aabb) {
            continue;
        }
        let a_leaf = is_leaf(na);
        let b_leaf = is_leaf(nb);
        if a_leaf && b_leaf {
            emit(a, b);
        } else if b_leaf || (!a_leaf && perimeter(na.aabb) >= perimeter(nb.aabb)) {
            stack.push((na.children.child1, b, false));
            stack.push((na.children.child2, b, false));
        } else {
            stack.push((a, nb.children.child1, false));
            stack.push((a, nb.children.child2, false));
        }
    }
}

// PORT EXTENSION — not in upstream C. One BVTT descent step, shared by the
// self- and cross-traversals above and by the parallel frontier machinery
// below. For `is_self` (single-node self work, `a` in `tree_a`, `b` ignored)
// it expands to the two child self-problems + the cross-problem, exactly like
// dynamic_tree_self_pairs. Otherwise (`a` in `tree_a`, `b` in `tree_b`) it
// descends the larger node, exactly like dynamic_tree_cross_pairs, reporting
// overlapping leaf pairs via `on_leaf`. For a self-traversal callers pass
// `tree_a == tree_b`.
#[inline]
fn bvtt_step(
    tree_a: &DynamicTree,
    tree_b: &DynamicTree,
    a: i32,
    b: i32,
    is_self: bool,
    stack: &mut Vec<(i32, i32, bool)>,
    on_leaf: &mut dyn FnMut(i32, i32),
) {
    if is_self {
        let node = &tree_a.nodes[a as usize];
        if is_leaf(node) {
            return;
        }
        let (c1, c2) = (node.children.child1, node.children.child2);
        stack.push((c1, c1, true));
        stack.push((c2, c2, true));
        stack.push((c1, c2, false));
    } else {
        let na = &tree_a.nodes[a as usize];
        let nb = &tree_b.nodes[b as usize];
        if !aabb_overlaps(na.aabb, nb.aabb) {
            return;
        }
        let a_leaf = is_leaf(na);
        let b_leaf = is_leaf(nb);
        if a_leaf && b_leaf {
            on_leaf(a, b);
        } else if b_leaf || (!a_leaf && perimeter(na.aabb) >= perimeter(nb.aabb)) {
            stack.push((na.children.child1, b, false));
            stack.push((na.children.child2, b, false));
        } else {
            stack.push((a, nb.children.child1, false));
            stack.push((a, nb.children.child2, false));
        }
    }
}

/// PORT EXTENSION — not in upstream C. Drain a PRE-SEEDED work stack to
/// completion, reporting every overlapping leaf pair via `on_leaf`. Used by
/// the parallel batch drain: each worker seeds the stack with one frontier
/// work item and drains it. Produces exactly the leaf pairs contained under
/// that work item's subtree pair.
pub fn dynamic_tree_bvtt_drain(
    tree_a: &DynamicTree,
    tree_b: &DynamicTree,
    stack: &mut Vec<(i32, i32, bool)>,
    on_leaf: &mut dyn FnMut(i32, i32),
) {
    while let Some((a, b, is_self)) = stack.pop() {
        bvtt_step(tree_a, tree_b, a, b, is_self, stack, on_leaf);
    }
}

/// PORT EXTENSION — not in upstream C. Expand a PRE-SEEDED work stack until it
/// holds at least `target` items (or empties), reporting any terminal leaf
/// pairs found during expansion via `on_leaf`. The leftover stack is a
/// frontier: a set of independent subtree-pair work items that together cover
/// exactly the not-yet-reported leaf pairs, with no overlap — so draining each
/// (via `dynamic_tree_bvtt_drain`) in parallel and unioning the results
/// reproduces the full serial traversal set exactly. Distribution-independent
/// by construction.
pub fn dynamic_tree_bvtt_expand(
    tree_a: &DynamicTree,
    tree_b: &DynamicTree,
    stack: &mut Vec<(i32, i32, bool)>,
    target: usize,
    on_leaf: &mut dyn FnMut(i32, i32),
) {
    while stack.len() < target {
        let Some((a, b, is_self)) = stack.pop() else {
            break;
        };
        bvtt_step(tree_a, tree_b, a, b, is_self, stack, on_leaf);
    }
}

/// Get proxy user data.
#[inline]
pub fn dynamic_tree_get_user_data(tree: &DynamicTree, proxy_id: i32) -> u64 {
    tree.nodes[proxy_id as usize].user_data
}

/// Get the AABB of a proxy.
#[inline]
pub fn dynamic_tree_get_aabb(tree: &DynamicTree, proxy_id: i32) -> AABB {
    tree.nodes[proxy_id as usize].aabb
}

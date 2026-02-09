// AABB tree for broad-phase triangle-triangle collision detection.
//
// Builds a binary tree of axis-aligned bounding boxes over a set of triangles.
// Enables fast O(n log n + k) overlap queries between two meshes, where k is
// the number of overlapping triangle pairs.

use makepad_csg_math::{BBox3d, Vec3d};

/// A node in the AABB tree. Internal nodes have children; leaves have a triangle index.
#[derive(Clone, Debug)]
struct AabbNode {
    bbox: BBox3d,
    /// Left child index (or u32::MAX if leaf).
    left: u32,
    /// Right child index (or u32::MAX if leaf).
    right: u32,
    /// Triangle index (only valid for leaves).
    tri_idx: u32,
}

impl AabbNode {
    fn is_leaf(&self) -> bool {
        self.left == u32::MAX
    }
}

/// An AABB tree over a set of triangles.
pub struct AabbTree {
    nodes: Vec<AabbNode>,
    root: u32,
}

/// A triangle with precomputed bounding box for tree construction.
struct TriEntry {
    tri_idx: u32,
    bbox: BBox3d,
    centroid: Vec3d,
}

impl AabbTree {
    /// Build an AABB tree from triangle bounding boxes.
    /// `triangles`: each entry is (v0, v1, v2) for a triangle.
    pub fn build(triangles: &[(Vec3d, Vec3d, Vec3d)]) -> AabbTree {
        if triangles.is_empty() {
            return AabbTree {
                nodes: Vec::new(),
                root: u32::MAX,
            };
        }

        let mut entries: Vec<TriEntry> = triangles
            .iter()
            .enumerate()
            .map(|(i, &(a, b, c))| {
                let bbox = BBox3d::from_triangle(a, b, c);
                let centroid = bbox.center();
                TriEntry {
                    tri_idx: i as u32,
                    bbox,
                    centroid,
                }
            })
            .collect();

        let mut nodes = Vec::with_capacity(entries.len() * 2);
        let len = entries.len();
        let root = build_recursive(&mut entries, &mut nodes, 0, len);
        AabbTree { nodes, root }
    }

    /// Find all pairs of overlapping triangles between this tree and another.
    /// Returns pairs (tri_idx_from_self, tri_idx_from_other).
    pub fn find_overlaps(&self, other: &AabbTree) -> Vec<(u32, u32)> {
        let mut pairs = Vec::new();
        if self.root == u32::MAX || other.root == u32::MAX {
            return pairs;
        }
        find_overlaps_recursive(&self.nodes, self.root, &other.nodes, other.root, &mut pairs);
        pairs
    }

    /// Number of nodes in the tree.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

fn build_recursive(
    entries: &mut [TriEntry],
    nodes: &mut Vec<AabbNode>,
    start: usize,
    end: usize,
) -> u32 {
    if start >= end {
        return u32::MAX;
    }

    // Compute bounding box of all entries in range
    let mut bbox = entries[start].bbox;
    for e in &entries[start + 1..end] {
        bbox = bbox.union(e.bbox);
    }

    let count = end - start;

    // Leaf node
    if count == 1 {
        let node_idx = nodes.len() as u32;
        nodes.push(AabbNode {
            bbox,
            left: u32::MAX,
            right: u32::MAX,
            tri_idx: entries[start].tri_idx,
        });
        return node_idx;
    }

    // Choose split axis: longest axis of the bounding box
    let axis = bbox.longest_axis();

    // Sort entries by centroid along the chosen axis
    entries[start..end].sort_by(|a, b| {
        let ca = match axis {
            0 => a.centroid.x,
            1 => a.centroid.y,
            _ => a.centroid.z,
        };
        let cb = match axis {
            0 => b.centroid.x,
            1 => b.centroid.y,
            _ => b.centroid.z,
        };
        ca.partial_cmp(&cb).unwrap_or(std::cmp::Ordering::Equal)
    });

    let mid = start + count / 2;

    // Reserve space for this node
    let node_idx = nodes.len() as u32;
    nodes.push(AabbNode {
        bbox,
        left: u32::MAX,
        right: u32::MAX,
        tri_idx: u32::MAX,
    });

    let left = build_recursive(entries, nodes, start, mid);
    let right = build_recursive(entries, nodes, mid, end);

    nodes[node_idx as usize].left = left;
    nodes[node_idx as usize].right = right;

    node_idx
}

fn find_overlaps_recursive(
    a_nodes: &[AabbNode],
    a_idx: u32,
    b_nodes: &[AabbNode],
    b_idx: u32,
    pairs: &mut Vec<(u32, u32)>,
) {
    if a_idx == u32::MAX || b_idx == u32::MAX {
        return;
    }

    let a = &a_nodes[a_idx as usize];
    let b = &b_nodes[b_idx as usize];

    if !a.bbox.intersects(b.bbox) {
        return;
    }

    if a.is_leaf() && b.is_leaf() {
        pairs.push((a.tri_idx, b.tri_idx));
        return;
    }

    // Descend into the larger node (heuristic for balance)
    if a.is_leaf() || (!b.is_leaf() && a.bbox.surface_area() <= b.bbox.surface_area()) {
        find_overlaps_recursive(a_nodes, a_idx, b_nodes, b.left, pairs);
        find_overlaps_recursive(a_nodes, a_idx, b_nodes, b.right, pairs);
    } else {
        find_overlaps_recursive(a_nodes, a.left, b_nodes, b_idx, pairs);
        find_overlaps_recursive(a_nodes, a.right, b_nodes, b_idx, pairs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_csg_math::dvec3;

    fn tri(
        ax: f64,
        ay: f64,
        az: f64,
        bx: f64,
        by: f64,
        bz: f64,
        cx: f64,
        cy: f64,
        cz: f64,
    ) -> (Vec3d, Vec3d, Vec3d) {
        (dvec3(ax, ay, az), dvec3(bx, by, bz), dvec3(cx, cy, cz))
    }

    #[test]
    fn test_build_empty() {
        let tree = AabbTree::build(&[]);
        assert_eq!(tree.node_count(), 0);
    }

    #[test]
    fn test_build_single() {
        let tris = vec![tri(0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0)];
        let tree = AabbTree::build(&tris);
        assert_eq!(tree.node_count(), 1);
    }

    #[test]
    fn test_overlapping_triangles() {
        let a = vec![tri(0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0)];
        let b = vec![tri(0.5, 0.0, -0.5, 0.5, 0.0, 0.5, 0.5, 1.0, 0.0)];
        let ta = AabbTree::build(&a);
        let tb = AabbTree::build(&b);
        let pairs = ta.find_overlaps(&tb);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0], (0, 0));
    }

    #[test]
    fn test_non_overlapping_triangles() {
        let a = vec![tri(0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0)];
        let b = vec![tri(10.0, 0.0, 0.0, 11.0, 0.0, 0.0, 10.0, 1.0, 0.0)];
        let ta = AabbTree::build(&a);
        let tb = AabbTree::build(&b);
        let pairs = ta.find_overlaps(&tb);
        assert!(pairs.is_empty());
    }

    #[test]
    fn test_multiple_triangles() {
        // Two groups of triangles: one at origin, one at x=10
        let a = vec![
            tri(0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0),
            tri(0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0),
        ];
        let b = vec![
            tri(0.5, 0.0, -0.5, 0.5, 0.0, 0.5, 0.5, 1.0, 0.0), // overlaps both a tris
            tri(10.0, 0.0, 0.0, 11.0, 0.0, 0.0, 10.0, 1.0, 0.0), // far away
        ];
        let ta = AabbTree::build(&a);
        let tb = AabbTree::build(&b);
        let pairs = ta.find_overlaps(&tb);
        // b[0] should overlap with both a[0] and a[1] (bboxes overlap)
        // b[1] should overlap with neither
        assert!(pairs.len() >= 1);
        // All pairs should involve b_idx=0
        for &(_, bi) in &pairs {
            assert_eq!(bi, 0);
        }
    }

    #[test]
    fn test_cube_vs_cube() {
        // Build two cubes' triangles and check overlaps
        use makepad_csg_mesh::mesh::make_unit_cube;

        let cube_a = make_unit_cube();
        let tris_a: Vec<_> = (0..cube_a.triangle_count())
            .map(|i| cube_a.triangle_vertices(i))
            .collect();

        let mut cube_b = make_unit_cube();
        cube_b.transform(makepad_csg_math::Mat4d::translation(dvec3(0.5, 0.0, 0.0)));
        let tris_b: Vec<_> = (0..cube_b.triangle_count())
            .map(|i| cube_b.triangle_vertices(i))
            .collect();

        let ta = AabbTree::build(&tris_a);
        let tb = AabbTree::build(&tris_b);
        let pairs = ta.find_overlaps(&tb);

        // Overlapping cubes should produce many candidate pairs
        assert!(
            pairs.len() > 0,
            "overlapping cubes should have AABB overlaps"
        );
    }
}

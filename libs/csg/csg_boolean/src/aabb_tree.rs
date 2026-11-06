// AABB tree for broad-phase triangle-triangle collision detection.
//
// Builds a binary tree of axis-aligned bounding boxes over a set of triangles.
// Enables fast O(n log n + k) overlap queries between two meshes, where k is
// the number of overlapping triangle pairs.

use makepad_csg_math::{orient3d, thread_pool, BBox3d, Vec3d};

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
        self.find_overlaps_with_tolerance(other, 0.0)
    }

    pub(crate) fn find_overlaps_with_tolerance(&self, other: &AabbTree, tolerance: f64) -> Vec<(u32, u32)> {
        let mut pairs = Vec::new();
        if self.root == u32::MAX || other.root == u32::MAX {
            return pairs;
        }
        find_overlaps_recursive(&self.nodes, self.root, &other.nodes, other.root, tolerance, &mut pairs);
        pairs
    }

    /// Number of nodes in the tree.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Count the number of triangles intersected by a ray, using BVH traversal.
    ///
    /// `origin`: ray origin
    /// `dir`: ray direction (need not be normalized)
    /// `triangles`: the triangle vertex data indexed by `tri_idx` in leaves
    ///
    /// Returns the number of forward-hitting triangles (t > epsilon).
    /// This replaces the brute-force O(n) scan with O(log n) average.
    pub fn ray_cast_count(
        &self,
        origin: Vec3d,
        dir: Vec3d,
        triangles: &[(Vec3d, Vec3d, Vec3d)],
    ) -> u32 {
        if self.root == u32::MAX {
            return 0;
        }
        let inv_dir = Vec3d::new(1.0 / dir.x, 1.0 / dir.y, 1.0 / dir.z);
        let mut count = 0u32;
        // Use an explicit stack to avoid recursion overhead.
        let mut stack = Vec::with_capacity(64);
        stack.push(self.root);
        while let Some(idx) = stack.pop() {
            if thread_pool::cancelled() {
                return count;
            }
            let node = &self.nodes[idx as usize];
            if !node.bbox.ray_intersects(origin, inv_dir) {
                continue;
            }
            if node.is_leaf() {
                let (v0, v1, v2) = triangles[node.tri_idx as usize];
                if ray_intersects_triangle(origin, dir, v0, v1, v2) {
                    count += 1;
                }
            } else {
                stack.push(node.left);
                stack.push(node.right);
            }
        }
        count
    }

    /// Count the triangles the segment `p`..`q` crosses: `p` and `q` strictly on opposite sides of a triangle's
    /// plane, with the same exact edge sidedness as [`AabbTree::ray_cast_count`]. Within a region holding every
    /// triangle of a closed mesh that meets it, the count's parity tells whether `p` and `q` lie on the same side
    /// of the mesh's surface.
    pub fn segment_cross_count(&self, p: Vec3d, q: Vec3d, triangles: &[(Vec3d, Vec3d, Vec3d)]) -> u32 {
        if self.root == u32::MAX {
            return 0;
        }
        let d = q - p;
        let inv_dir = Vec3d::new(1.0 / d.x, 1.0 / d.y, 1.0 / d.z);
        let mut count = 0u32;
        let mut stack = Vec::with_capacity(64);
        stack.push(self.root);
        while let Some(idx) = stack.pop() {
            if thread_pool::cancelled() {
                return count;
            }
            let node = &self.nodes[idx as usize];
            if !segment_meets_box(node.bbox, p, inv_dir) {
                continue;
            }
            if node.is_leaf() {
                let (v0, v1, v2) = triangles[node.tri_idx as usize];
                if segment_crosses_triangle(p, q, v0, v1, v2) {
                    count += 1;
                }
            } else {
                stack.push(node.left);
                stack.push(node.right);
            }
        }
        count
    }
}

/// The slab test of [`BBox3d::ray_intersects`] for `p + t * (q - p)`, `t` in 0..=1 (`inv_dir` = 1 / (q - p)).
#[inline]
fn segment_meets_box(b: BBox3d, p: Vec3d, inv_dir: Vec3d) -> bool {
    let t1x = (b.min.x - p.x) * inv_dir.x;
    let t2x = (b.max.x - p.x) * inv_dir.x;
    let t1y = (b.min.y - p.y) * inv_dir.y;
    let t2y = (b.max.y - p.y) * inv_dir.y;
    let t1z = (b.min.z - p.z) * inv_dir.z;
    let t2z = (b.max.z - p.z) * inv_dir.z;
    let tmin = t1x.min(t2x).max(t1y.min(t2y)).max(t1z.min(t2z));
    let tmax = t1x.max(t2x).min(t1y.max(t2y)).min(t1z.max(t2z));
    tmax.min(1.0) >= tmin.max(0.0)
}

/// `p`..`q` crosses the triangle: its ends strictly on opposite sides of the plane, its line through the
/// triangle (an edge or corner hit counts, as for a ray).
#[inline]
fn segment_crosses_triangle(p: Vec3d, q: Vec3d, v0: Vec3d, v1: Vec3d, v2: Vec3d) -> bool {
    let (sp, sq) = (orient3d(v0, v1, v2, p), orient3d(v0, v1, v2, q));
    if sp == 0.0 || sq == 0.0 || (sp > 0.0) == (sq > 0.0) { return false; }
    let sides = [orient3d(p, q, v0, v1), orient3d(p, q, v1, v2), orient3d(p, q, v2, v0)];
    !sides.iter().any(|&s| s < 0.0) || !sides.iter().any(|&s| s > 0.0)
}

/// Forward ray hits with exact plane/edge sidedness. A positive hit must
/// not be discarded just because the origin is very close to the surface.
#[inline]
pub(crate) fn ray_intersects_triangle(origin: Vec3d, dir: Vec3d, v0: Vec3d, v1: Vec3d, v2: Vec3d) -> bool {
    let edge1 = v1 - v0;
    let edge2 = v2 - v0;
    let h = dir.cross(edge2);
    let a = edge1.dot(h);

    if a == 0.0 { return false; }
    let side = orient3d(v0, v1, v2, origin);
    if side == 0.0 || (side > 0.0) != (a > 0.0) { return false; }
    let end = origin + dir;
    let sides = [orient3d(origin, end, v0, v1), orient3d(origin, end, v1, v2), orient3d(origin, end, v2, v0)];
    !sides.iter().any(|&s| s < 0.0) || !sides.iter().any(|&s| s > 0.0)
}

fn build_recursive(
    entries: &mut [TriEntry],
    nodes: &mut Vec<AabbNode>,
    start: usize,
    end: usize,
) -> u32 {
    if start >= end || thread_pool::cancelled() {
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
    tolerance: f64,
    pairs: &mut Vec<(u32, u32)>,
) {
    if a_idx == u32::MAX || b_idx == u32::MAX || thread_pool::cancelled() {
        return;
    }

    let a = &a_nodes[a_idx as usize];
    let b = &b_nodes[b_idx as usize];

    if !a.bbox.intersects(b.bbox) {
        // Broad phase must admit the same rounded coincidence as the
        // triangle-plane test; otherwise almost coplanar caps never reach it.
        let tol = crate::tri_tri::plane_tolerance(
            a.bbox.min, a.bbox.max, a.bbox.min,
            b.bbox.min, b.bbox.max, b.bbox.min,
        ).max(tolerance);
        if a.bbox.min.x - b.bbox.max.x > tol || b.bbox.min.x - a.bbox.max.x > tol
            || a.bbox.min.y - b.bbox.max.y > tol || b.bbox.min.y - a.bbox.max.y > tol
            || a.bbox.min.z - b.bbox.max.z > tol || b.bbox.min.z - a.bbox.max.z > tol
        {
            return;
        }
    }

    if a.is_leaf() && b.is_leaf() {
        pairs.push((a.tri_idx, b.tri_idx));
        return;
    }

    // Descend into the larger node (heuristic for balance)
    if a.is_leaf() || (!b.is_leaf() && a.bbox.surface_area() <= b.bbox.surface_area()) {
        find_overlaps_recursive(a_nodes, a_idx, b_nodes, b.left, tolerance, pairs);
        find_overlaps_recursive(a_nodes, a_idx, b_nodes, b.right, tolerance, pairs);
    } else {
        find_overlaps_recursive(a_nodes, a.left, b_nodes, b_idx, tolerance, pairs);
        find_overlaps_recursive(a_nodes, a.right, b_nodes, b_idx, tolerance, pairs);
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
}

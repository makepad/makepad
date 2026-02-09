// BspNode: Binary Space Partition tree for CSG boolean operations.
//
// Based on Laidlaw, Trumbore, Hughes 1986 and the csg.js implementation.
// Each node stores a splitting plane, coplanar polygons, and front/back subtrees.
// Key operations: build, invert, clip_polygons, clip_to, all_polygons.

use crate::polygon::{Polygon, SplitResult};
use makepad_csg_math::{Planed, Vec3d};

pub struct BspNode {
    pub plane: Planed,
    pub polygons: Vec<Polygon>, // coplanar polygons at this node
    pub front: Option<Box<BspNode>>,
    pub back: Option<Box<BspNode>>,
}

impl BspNode {
    /// Build a BSP tree from a list of polygons.
    /// Returns None if the polygon list is empty.
    pub fn build(polygons: Vec<Polygon>) -> Option<BspNode> {
        Self::build_inner(polygons, 0)
    }

    fn build_inner(mut polygons: Vec<Polygon>, depth: usize) -> Option<BspNode> {
        if polygons.is_empty() {
            return None;
        }

        // Safety: cap recursion depth to prevent exponential blowup.
        // When a sphere-triangle vertex lands exactly on a cube-face plane,
        // orient3d can classify it as coplanar on one side and spanning on
        // the other, producing an endless chain of single-polygon nodes.
        // Depth 200 is far more than any well-behaved BSP needs.
        if polygons.len() == 1 || depth > 200 {
            let plane = polygons[0].plane;
            return Some(BspNode {
                plane,
                polygons,
                front: None,
                back: None,
            });
        }

        // Pick splitting plane using heuristic
        let split_idx = pick_splitting_plane(&polygons);
        let plane = polygons[split_idx].plane;

        // Get three points for robust orient3d classification
        let plane_tri = get_plane_tri(&polygons[split_idx]);

        let mut coplanar = Vec::new();
        let mut front_polys = Vec::new();
        let mut back_polys = Vec::new();

        for poly in polygons.drain(..) {
            match poly.split_by_plane(&plane, Some(plane_tri)) {
                SplitResult::Coplanar(p) => {
                    // Coplanar polygons (same or opposite facing) are stored at this node.
                    // Orientation is resolved later during clip_polygons.
                    coplanar.push(p);
                }
                SplitResult::Front(p) => front_polys.push(p),
                SplitResult::Back(p) => back_polys.push(p),
                SplitResult::Split { front, back } => {
                    front_polys.push(front);
                    back_polys.push(back);
                }
            }
        }

        Some(BspNode {
            plane,
            polygons: coplanar,
            front: BspNode::build_inner(front_polys, depth + 1).map(Box::new),
            back: BspNode::build_inner(back_polys, depth + 1).map(Box::new),
        })
    }

    /// Add polygons to an existing BSP tree.
    pub fn add_polygons(&mut self, polygons: Vec<Polygon>) {
        let plane_tri = self.get_plane_tri_from_polygons();

        let mut front_polys = Vec::new();
        let mut back_polys = Vec::new();

        for poly in polygons {
            match poly.split_by_plane(&self.plane, Some(plane_tri)) {
                SplitResult::Coplanar(p) => self.polygons.push(p),
                SplitResult::Front(p) => front_polys.push(p),
                SplitResult::Back(p) => back_polys.push(p),
                SplitResult::Split { front, back } => {
                    front_polys.push(front);
                    back_polys.push(back);
                }
            }
        }

        if !front_polys.is_empty() {
            if let Some(ref mut front) = self.front {
                front.add_polygons(front_polys);
            } else {
                self.front = BspNode::build(front_polys).map(Box::new);
            }
        }

        if !back_polys.is_empty() {
            if let Some(ref mut back) = self.back {
                back.add_polygons(back_polys);
            } else {
                self.back = BspNode::build(back_polys).map(Box::new);
            }
        }
    }

    /// Invert this BSP tree: flip all polygons and swap front/back.
    /// This converts solid space to empty space and vice versa.
    pub fn invert(&mut self) {
        // Flip all polygons at this node
        for p in &mut self.polygons {
            p.flip();
        }
        // Flip the splitting plane
        self.plane = self.plane.flip();
        // Recursively invert children
        if let Some(ref mut front) = self.front {
            front.invert();
        }
        if let Some(ref mut back) = self.back {
            back.invert();
        }
        // Swap front and back
        std::mem::swap(&mut self.front, &mut self.back);
    }

    /// Clip a list of polygons against this BSP tree.
    /// Returns only polygon fragments that are in front of (outside) the solid.
    pub fn clip_polygons(&self, polygons: &[Polygon]) -> Vec<Polygon> {
        let plane_tri = self.get_plane_tri_from_polygons();
        let mut front_result = Vec::new();
        let mut back_result = Vec::new();

        for poly in polygons {
            match poly.split_by_plane(&self.plane, Some(plane_tri)) {
                SplitResult::Coplanar(p) => {
                    // Coplanar: classify by normal orientation.
                    // Same-facing normals -> front (keep), opposite -> back (clip).
                    // This is critical for identical/touching face handling.
                    if p.plane.normal.dot(self.plane.normal) > 0.0 {
                        front_result.push(p);
                    } else {
                        back_result.push(p);
                    }
                }
                SplitResult::Front(p) => front_result.push(p),
                SplitResult::Back(p) => back_result.push(p),
                SplitResult::Split { front, back } => {
                    front_result.push(front);
                    back_result.push(back);
                }
            }
        }

        // Recurse into children
        let front_result = if let Some(ref front) = self.front {
            front.clip_polygons(&front_result)
        } else {
            front_result
        };

        let back_result = if let Some(ref back) = self.back {
            back.clip_polygons(&back_result)
        } else {
            // No back node -> these polygons are inside the solid -> discard
            Vec::new()
        };

        // Combine results
        let mut result = front_result;
        result.extend(back_result);
        result
    }

    /// Clip all polygons in this tree against another BSP tree.
    /// Removes polygons (or fragments) that are inside the other solid.
    pub fn clip_to(&mut self, other: &BspNode) {
        self.polygons = other.clip_polygons(&self.polygons);
        if let Some(ref mut front) = self.front {
            front.clip_to(other);
        }
        if let Some(ref mut back) = self.back {
            back.clip_to(other);
        }
    }

    /// Collect all polygons from the entire tree.
    pub fn all_polygons(&self) -> Vec<Polygon> {
        let mut result = self.polygons.clone();
        if let Some(ref front) = self.front {
            result.extend(front.all_polygons());
        }
        if let Some(ref back) = self.back {
            result.extend(back.all_polygons());
        }
        result
    }

    /// Count all polygons in the tree.
    pub fn polygon_count(&self) -> usize {
        let mut count = self.polygons.len();
        if let Some(ref front) = self.front {
            count += front.polygon_count();
        }
        if let Some(ref back) = self.back {
            count += back.polygon_count();
        }
        count
    }

    /// Get three points from the node's polygons for orient3d.
    fn get_plane_tri_from_polygons(&self) -> (Vec3d, Vec3d, Vec3d) {
        // Use the node's plane to construct three points
        plane_to_tri(&self.plane)
    }
}

/// Pick the best splitting plane from the polygon list.
/// Uses the heuristic from csgrs: sample up to MAX_CANDIDATES polygons,
/// score by split count + imbalance, pick the minimum.
fn pick_splitting_plane(polygons: &[Polygon]) -> usize {
    let n = polygons.len();
    if n <= 1 {
        return 0;
    }

    let max_candidates = 20.min(n);
    let step = if n > max_candidates {
        n / max_candidates
    } else {
        1
    };

    let mut best_idx = 0;
    let mut best_score = i64::MAX;

    for candidate in (0..n).step_by(step).take(max_candidates) {
        let plane = &polygons[candidate].plane;
        let plane_tri = get_plane_tri(&polygons[candidate]);
        let mut front_count = 0i64;
        let mut back_count = 0i64;
        let mut split_count = 0i64;

        for (i, poly) in polygons.iter().enumerate() {
            if i == candidate {
                continue;
            }
            match classify_polygon_vs_plane(poly, plane, plane_tri) {
                0 => {} // coplanar
                1 => front_count += 1,
                2 => back_count += 1,
                3 => split_count += 1,
                _ => unreachable!(),
            }
        }

        // Score: penalize splits heavily, also penalize imbalance
        let score = 5 * split_count + (front_count - back_count).abs();
        if score < best_score {
            best_score = score;
            best_idx = candidate;
        }
    }

    best_idx
}

/// Classify a polygon against a plane: returns 0=coplanar, 1=front, 2=back, 3=spanning.
fn classify_polygon_vs_plane(
    poly: &Polygon,
    plane: &Planed,
    plane_tri: (Vec3d, Vec3d, Vec3d),
) -> u8 {
    let mut result: u8 = 0;
    let (pa, pb, pc) = plane_tri;
    for &v in &poly.vertices {
        let o = makepad_csg_math::orient3d(pa, pb, pc, v);
        if o > 0.0 {
            result |= 1;
        } else if o < 0.0 {
            result |= 2;
        }
        if result == 3 {
            break; // spanning, no need to check more
        }
    }
    result
}

/// Get three representative points from a polygon for orient3d.
fn get_plane_tri(poly: &Polygon) -> (Vec3d, Vec3d, Vec3d) {
    let v = &poly.vertices;
    if v.len() >= 3 {
        (v[0], v[1], v[2])
    } else {
        // Fallback: shouldn't happen since polygons have >= 3 verts
        plane_to_tri(&poly.plane)
    }
}

/// Construct three points on a plane for orient3d.
fn plane_to_tri(plane: &Planed) -> (Vec3d, Vec3d, Vec3d) {
    let n = plane.normal;
    let d = plane.dist;
    let origin = n * d; // closest point on plane to origin

    // Find a vector not parallel to the normal
    let helper = if n.x.abs() < 0.9 {
        Vec3d::new(1.0, 0.0, 0.0)
    } else {
        Vec3d::new(0.0, 1.0, 0.0)
    };
    let u_raw = helper.cross(n);
    let u_len_sq = u_raw.length_sq();
    let u = if u_len_sq > 1e-30 {
        u_raw * (1.0 / u_len_sq.sqrt())
    } else {
        // Fallback: try the third axis
        let fallback = Vec3d::new(0.0, 0.0, 1.0).cross(n);
        fallback.normalize()
    };
    let v = n.cross(u);

    (origin, origin + u, origin + v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_csg_math::dvec3;

    fn make_cube_polygons() -> Vec<Polygon> {
        let mesh = makepad_csg_mesh::mesh::make_unit_cube();
        let mut polys = Vec::new();
        for i in 0..mesh.triangle_count() {
            let (a, b, c) = mesh.triangle_vertices(i);
            if let Some(p) = Polygon::from_triangle(a, b, c) {
                polys.push(p);
            }
        }
        polys
    }

    #[test]
    fn test_build_from_cube() {
        let polys = make_cube_polygons();
        assert_eq!(polys.len(), 12);
        let node = BspNode::build(polys).unwrap();
        let all = node.all_polygons();
        assert_eq!(all.len(), 12);
    }

    #[test]
    fn test_invert() {
        let polys = make_cube_polygons();
        let mut node = BspNode::build(polys).unwrap();
        let orig_count = node.polygon_count();
        node.invert();
        assert_eq!(node.polygon_count(), orig_count);
        // After inverting, all polygon normals should be flipped
        for p in node.all_polygons() {
            // The normal should now point inward for a cube
            // (original normals point outward)
            let center = dvec3(0.0, 0.0, 0.0);
            let tri_center = p.vertices.iter().copied().fold(Vec3d::ZERO, |a, b| a + b)
                / p.vertices.len() as f64;
            let to_center = center - tri_center;
            assert!(
                p.plane.normal.dot(to_center) > 0.0,
                "inverted normal should point inward"
            );
        }
    }

    #[test]
    fn test_double_invert() {
        let polys = make_cube_polygons();
        let mut node = BspNode::build(polys.clone()).unwrap();
        node.invert();
        node.invert();
        // After double invert, polygon count should be the same
        assert_eq!(node.polygon_count(), polys.len());
    }

    #[test]
    fn test_clip_polygons_outside() {
        // Clip a polygon that's entirely outside the cube -> should be kept
        let cube_polys = make_cube_polygons();
        let node = BspNode::build(cube_polys).unwrap();

        let outside_tri = Polygon::from_triangle(
            dvec3(5.0, 0.0, 0.0),
            dvec3(6.0, 0.0, 0.0),
            dvec3(5.5, 1.0, 0.0),
        )
        .unwrap();

        let result = node.clip_polygons(&[outside_tri]);
        assert!(!result.is_empty(), "outside polygon should be kept");
    }

    #[test]
    fn test_clip_polygons_inside() {
        // Clip a polygon that's entirely inside the cube -> should be removed
        let cube_polys = make_cube_polygons();
        let node = BspNode::build(cube_polys).unwrap();

        let inside_tri = Polygon::from_triangle(
            dvec3(0.0, 0.0, 0.0),
            dvec3(0.1, 0.0, 0.0),
            dvec3(0.0, 0.1, 0.0),
        )
        .unwrap();

        let result = node.clip_polygons(&[inside_tri]);
        assert!(
            result.is_empty(),
            "inside polygon should be clipped away, got {} polygons",
            result.len()
        );
    }
}

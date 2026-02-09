// ConvexPolygon: the fundamental unit of the BSP algorithm.
// Each polygon has 3+ vertices, a supporting plane, and can be split by another plane.
// Uses robust orient3d predicates for classification.

use makepad_csg_math::{orient3d, Planed, Side, Vec3d};

#[derive(Clone, Debug)]
pub struct Polygon {
    pub vertices: Vec<Vec3d>,
    pub plane: Planed,
}

pub enum SplitResult {
    Coplanar(Polygon),
    Front(Polygon),
    Back(Polygon),
    Split { front: Polygon, back: Polygon },
}

/// Classification of a polygon relative to a splitting plane.
/// Uses bitmask: FRONT=1, BACK=2, SPANNING=3, COPLANAR=0.
const COPLANAR: u8 = 0;
const FRONT: u8 = 1;
const BACK: u8 = 2;
const SPANNING: u8 = 3;

impl Polygon {
    /// Create a polygon from vertices. Computes the plane from the first 3 vertices.
    /// Returns None if the polygon is degenerate (< 3 verts or collinear).
    pub fn new(vertices: Vec<Vec3d>) -> Option<Polygon> {
        if vertices.len() < 3 {
            return None;
        }
        // Find the best triangle for plane computation (largest area)
        let plane = best_plane_from_vertices(&vertices)?;
        Some(Polygon { vertices, plane })
    }

    /// Create a polygon from a triangle.
    pub fn from_triangle(a: Vec3d, b: Vec3d, c: Vec3d) -> Option<Polygon> {
        let plane = Planed::from_points(a, b, c)?;
        Some(Polygon {
            vertices: vec![a, b, c],
            plane,
        })
    }

    /// Create a polygon from vertices with a known plane.
    pub fn with_plane(vertices: Vec<Vec3d>, plane: Planed) -> Polygon {
        Polygon { vertices, plane }
    }

    /// Flip the polygon (reverse winding, flip plane).
    pub fn flip(&mut self) {
        self.vertices.reverse();
        self.plane = self.plane.flip();
    }

    /// Return a flipped copy.
    pub fn flipped(&self) -> Polygon {
        let mut p = self.clone();
        p.flip();
        p
    }

    /// Split this polygon by a plane.
    ///
    /// Uses robust orient3d to classify each vertex, then clips the polygon
    /// against the plane using the Sutherland-Hodgman approach.
    ///
    /// The `plane_tri` provides three points defining the splitting plane
    /// for use with orient3d. If not available, falls back to epsilon-based
    /// classification using the plane's signed_distance.
    pub fn split_by_plane(
        &self,
        split_plane: &Planed,
        plane_tri: Option<(Vec3d, Vec3d, Vec3d)>,
    ) -> SplitResult {
        let n = self.vertices.len();

        // Classify each vertex
        let mut sides = Vec::with_capacity(n);
        let mut polygon_type: u8 = 0;

        for &v in &self.vertices {
            let side = if let Some((pa, pb, pc)) = plane_tri {
                // Use robust orient3d
                let o = orient3d(pa, pb, pc, v);
                if o > 0.0 {
                    FRONT
                } else if o < 0.0 {
                    BACK
                } else {
                    COPLANAR
                }
            } else {
                // Fallback to epsilon classification
                let d = split_plane.signed_distance(v);
                if d > 1e-10 {
                    FRONT
                } else if d < -1e-10 {
                    BACK
                } else {
                    COPLANAR
                }
            };
            polygon_type |= side;
            sides.push(side);
        }

        match polygon_type {
            COPLANAR => SplitResult::Coplanar(self.clone()),
            FRONT => SplitResult::Front(self.clone()),
            BACK => SplitResult::Back(self.clone()),
            SPANNING => {
                let mut front_verts = Vec::new();
                let mut back_verts = Vec::new();

                for i in 0..n {
                    let j = (i + 1) % n;
                    let si = sides[i];
                    let sj = sides[j];
                    let vi = self.vertices[i];
                    let vj = self.vertices[j];

                    if si != BACK {
                        front_verts.push(vi);
                    }
                    if si != FRONT {
                        back_verts.push(vi);
                    }

                    // If edge crosses the plane, compute intersection point
                    if (si | sj) == SPANNING {
                        let t = split_plane.intersect_line_t(vi, vj);
                        let intersection = vi.lerp(vj, t);
                        front_verts.push(intersection);
                        back_verts.push(intersection);
                    }
                }

                // Build polygons from collected vertices
                let front = if front_verts.len() >= 3 {
                    Some(Polygon::with_plane(front_verts, self.plane))
                } else {
                    None
                };

                let back = if back_verts.len() >= 3 {
                    Some(Polygon::with_plane(back_verts, self.plane))
                } else {
                    None
                };

                match (front, back) {
                    (Some(f), Some(b)) => SplitResult::Split { front: f, back: b },
                    (Some(f), None) => SplitResult::Front(f),
                    (None, Some(b)) => SplitResult::Back(b),
                    (None, None) => SplitResult::Coplanar(self.clone()),
                }
            }
            _ => unreachable!(),
        }
    }

    /// Fan triangulate this polygon into triangles.
    /// For a convex polygon with N vertices, produces N-2 triangles.
    pub fn triangulate(&self) -> Vec<[Vec3d; 3]> {
        let n = self.vertices.len();
        if n < 3 {
            return Vec::new();
        }
        let mut tris = Vec::with_capacity(n - 2);
        for i in 1..n - 1 {
            tris.push([self.vertices[0], self.vertices[i], self.vertices[i + 1]]);
        }
        tris
    }
}

/// Find the best plane from a set of vertices by picking the triangle
/// with the largest area (most robust plane computation).
fn best_plane_from_vertices(verts: &[Vec3d]) -> Option<Planed> {
    if verts.len() < 3 {
        return None;
    }
    if verts.len() == 3 {
        return Planed::from_points(verts[0], verts[1], verts[2]);
    }

    let mut best_plane = None;
    let mut best_area_sq = 0.0;

    for i in 0..verts.len() {
        for j in i + 1..verts.len() {
            for k in j + 1..verts.len() {
                let cross = (verts[j] - verts[i]).cross(verts[k] - verts[i]);
                let area_sq = cross.length_sq();
                if area_sq > best_area_sq {
                    best_area_sq = area_sq;
                    best_plane = Planed::from_points(verts[i], verts[j], verts[k]);
                }
            }
        }
    }

    best_plane
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_csg_math::dvec3;

    #[test]
    fn test_polygon_from_triangle() {
        let p = Polygon::from_triangle(
            dvec3(0.0, 0.0, 0.0),
            dvec3(1.0, 0.0, 0.0),
            dvec3(0.0, 1.0, 0.0),
        )
        .unwrap();
        assert_eq!(p.vertices.len(), 3);
        assert!((p.plane.normal.z - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_polygon_flip() {
        let mut p = Polygon::from_triangle(
            dvec3(0.0, 0.0, 0.0),
            dvec3(1.0, 0.0, 0.0),
            dvec3(0.0, 1.0, 0.0),
        )
        .unwrap();
        assert!(p.plane.normal.z > 0.0);
        p.flip();
        assert!(p.plane.normal.z < 0.0);
    }

    #[test]
    fn test_split_front() {
        // Triangle in z=1 plane, split by z=0 plane -> entirely front
        let p = Polygon::from_triangle(
            dvec3(0.0, 0.0, 1.0),
            dvec3(1.0, 0.0, 1.0),
            dvec3(0.0, 1.0, 1.0),
        )
        .unwrap();
        let split_plane = Planed::from_normal_and_point(Vec3d::Z, Vec3d::ZERO);
        match p.split_by_plane(&split_plane, None) {
            SplitResult::Front(_) => {} // expected
            _ => panic!("expected Front"),
        }
    }

    #[test]
    fn test_split_back() {
        // Triangle in z=-1 plane, split by z=0 plane -> entirely back
        let p = Polygon::from_triangle(
            dvec3(0.0, 0.0, -1.0),
            dvec3(1.0, 0.0, -1.0),
            dvec3(0.0, 1.0, -1.0),
        )
        .unwrap();
        let split_plane = Planed::from_normal_and_point(Vec3d::Z, Vec3d::ZERO);
        match p.split_by_plane(&split_plane, None) {
            SplitResult::Back(_) => {} // expected
            _ => panic!("expected Back"),
        }
    }

    #[test]
    fn test_split_spanning() {
        // Triangle straddles z=0 plane
        let p = Polygon::from_triangle(
            dvec3(0.0, 0.0, -1.0),
            dvec3(1.0, 0.0, -1.0),
            dvec3(0.5, 0.0, 1.0),
        )
        .unwrap();
        let split_plane = Planed::from_normal_and_point(Vec3d::Z, Vec3d::ZERO);
        match p.split_by_plane(&split_plane, None) {
            SplitResult::Split { front, back } => {
                assert!(front.vertices.len() >= 3);
                assert!(back.vertices.len() >= 3);
                // All front vertices should have z >= 0
                for v in &front.vertices {
                    assert!(v.z >= -1e-10, "front vertex has z={}", v.z);
                }
                // All back vertices should have z <= 0
                for v in &back.vertices {
                    assert!(v.z <= 1e-10, "back vertex has z={}", v.z);
                }
            }
            _ => panic!("expected Split"),
        }
    }

    #[test]
    fn test_split_coplanar() {
        // Triangle in z=0 plane, split by z=0 plane -> coplanar
        let p = Polygon::from_triangle(
            dvec3(0.0, 0.0, 0.0),
            dvec3(1.0, 0.0, 0.0),
            dvec3(0.0, 1.0, 0.0),
        )
        .unwrap();
        let split_plane = Planed::from_normal_and_point(Vec3d::Z, Vec3d::ZERO);
        match p.split_by_plane(&split_plane, None) {
            SplitResult::Coplanar(_) => {} // expected
            _ => panic!("expected Coplanar"),
        }
    }

    #[test]
    fn test_split_area_conservation() {
        // When we split a triangle, the total area should be preserved
        let p = Polygon::from_triangle(
            dvec3(0.0, 0.0, -1.0),
            dvec3(2.0, 0.0, -1.0),
            dvec3(1.0, 0.0, 1.0),
        )
        .unwrap();

        let original_area = polygon_area(&p);

        let split_plane = Planed::from_normal_and_point(Vec3d::Z, Vec3d::ZERO);
        match p.split_by_plane(&split_plane, None) {
            SplitResult::Split { front, back } => {
                let front_area = polygon_area(&front);
                let back_area = polygon_area(&back);
                let total = front_area + back_area;
                assert!(
                    (total - original_area).abs() < 1e-10,
                    "area not conserved: {} + {} = {} vs {}",
                    front_area,
                    back_area,
                    total,
                    original_area
                );
            }
            _ => panic!("expected Split"),
        }
    }

    #[test]
    fn test_triangulate() {
        // A quad should produce 2 triangles
        let p = Polygon::new(vec![
            dvec3(0.0, 0.0, 0.0),
            dvec3(1.0, 0.0, 0.0),
            dvec3(1.0, 1.0, 0.0),
            dvec3(0.0, 1.0, 0.0),
        ])
        .unwrap();
        let tris = p.triangulate();
        assert_eq!(tris.len(), 2);
    }

    fn polygon_area(p: &Polygon) -> f64 {
        let tris = p.triangulate();
        tris.iter()
            .map(|[a, b, c]| (*b - *a).cross(*c - *a).length() * 0.5)
            .sum()
    }
}

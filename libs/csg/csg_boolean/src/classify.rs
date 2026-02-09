// Inside/outside classification for corefined meshes.
//
// After corefinement, each triangle in mesh A needs to be classified as
// inside or outside mesh B (and vice versa). We use ray casting / winding
// number to classify.
//
// Algorithm:
// 1. For each triangle, shoot a ray from its centroid and count crossings
//    with the other mesh.
// 2. Odd crossings = inside, even crossings = outside.
// 3. Use adjacency propagation to speed up: only classify one "seed" triangle
//    per connected component, then propagate through edge-adjacency.

use makepad_csg_math::{dvec3, Vec3d};
use makepad_csg_mesh::mesh::TriMesh;

/// Classification of a triangle relative to another solid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TriLocation {
    Inside,
    Outside,
    OnBoundary,
}

/// Classify all triangles of mesh A as inside/outside mesh B using ray casting.
///
/// For boundary triangles, we test both sides (+normal and -normal) to detect:
/// - Non-coplanar boundary faces: only interior side is inside → Inside
/// - Coplanar overlapping faces: both sides report inside → OnBoundary
///   (kept from mesh A only to avoid double-counting)
/// - Faces outside the other mesh: neither side inside → Outside
pub fn classify_triangles(
    mesh_to_classify: &TriMesh,
    other_mesh: &TriMesh,
    on_boundary: &[bool],
) -> Vec<TriLocation> {
    let mut result = vec![TriLocation::Outside; mesh_to_classify.triangle_count()];

    for ti in 0..mesh_to_classify.triangle_count() {
        let centroid = tri_centroid(mesh_to_classify, ti);

        if on_boundary[ti] {
            let normal = mesh_to_classify.triangle_normal(ti);
            let eps = 1e-6;
            let inside_point = centroid - normal * eps;
            let outside_point = centroid + normal * eps;
            let interior_in = point_inside_mesh(inside_point, other_mesh);
            let exterior_in = point_inside_mesh(outside_point, other_mesh);

            result[ti] = if interior_in && exterior_in {
                // Both sides inside: face lies within/on the other mesh's surface.
                // This happens when the face is on a shared plane that sits
                // inside the other solid's volume (e.g., B's -X face at x=0
                // which is inside A). Mark as OnBoundary for dedup.
                TriLocation::OnBoundary
            } else if interior_in {
                // Only interior is inside: standard overlapping boundary face
                TriLocation::Inside
            } else {
                TriLocation::Outside
            };
        } else {
            // For non-boundary faces, test the centroid. If the centroid lies
            // exactly on the other mesh's surface, ray casting may be inconsistent.
            // Test a point slightly behind the face (interior side) as well.
            let inside = point_inside_mesh(centroid, other_mesh);
            result[ti] = if inside {
                TriLocation::Inside
            } else {
                // Double-check: nudge inward to handle surface-coincident faces
                let normal = mesh_to_classify.triangle_normal(ti);
                let inward = centroid - normal * 1e-6;
                if point_inside_mesh(inward, other_mesh) {
                    TriLocation::Inside
                } else {
                    TriLocation::Outside
                }
            };
        }
    }

    result
}

/// Test if a point is inside a closed triangle mesh using ray casting.
/// Uses multiple ray directions to avoid edge/vertex degeneracies.
pub fn point_inside_mesh(point: Vec3d, mesh: &TriMesh) -> bool {
    // Try multiple non-axis-aligned ray directions to avoid degeneracies
    // where the ray passes exactly through an edge or vertex.
    let directions = [
        dvec3(0.8726, 0.3517, 0.1943),  // arbitrary direction 1
        dvec3(-0.4123, 0.7891, 0.2345), // arbitrary direction 2
        dvec3(0.1234, -0.5678, 0.8901), // arbitrary direction 3
    ];

    // Use majority vote across directions
    let mut inside_votes = 0u32;
    for &ray_dir in &directions {
        let mut crossings = 0u32;
        for ti in 0..mesh.triangle_count() {
            let (v0, v1, v2) = mesh.triangle_vertices(ti);
            if ray_intersects_triangle(point, ray_dir, v0, v1, v2) {
                crossings += 1;
            }
        }
        if crossings % 2 == 1 {
            inside_votes += 1;
        }
    }

    inside_votes >= 2 // majority vote
}

/// Moller-Trumbore ray-triangle intersection test.
fn ray_intersects_triangle(origin: Vec3d, dir: Vec3d, v0: Vec3d, v1: Vec3d, v2: Vec3d) -> bool {
    let edge1 = v1 - v0;
    let edge2 = v2 - v0;
    let h = dir.cross(edge2);
    let a = edge1.dot(h);

    if a.abs() < 1e-12 {
        return false; // Ray parallel to triangle
    }

    let f = 1.0 / a;
    let s = origin - v0;
    let u = f * s.dot(h);
    if u < 0.0 || u > 1.0 {
        return false;
    }

    let q = s.cross(edge1);
    let v = f * dir.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return false;
    }

    let t = f * edge2.dot(q);
    t > 1e-10 // Only count forward intersections (with small epsilon to avoid self-intersection)
}

fn tri_centroid(mesh: &TriMesh, ti: usize) -> Vec3d {
    let (a, b, c) = mesh.triangle_vertices(ti);
    (a + b + c) / 3.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_csg_math::{dvec3, Mat4d};
    use makepad_csg_mesh::mesh::make_unit_cube;

    #[test]
    fn test_point_inside_cube() {
        let cube = make_unit_cube();
        assert!(point_inside_mesh(dvec3(0.0, 0.0, 0.0), &cube));
        assert!(point_inside_mesh(dvec3(0.1, 0.1, 0.1), &cube));
    }

    #[test]
    fn test_point_outside_cube() {
        let cube = make_unit_cube();
        assert!(!point_inside_mesh(dvec3(2.0, 0.0, 0.0), &cube));
        assert!(!point_inside_mesh(dvec3(0.0, 2.0, 0.0), &cube));
        assert!(!point_inside_mesh(dvec3(0.0, 0.0, 2.0), &cube));
    }

    #[test]
    fn test_point_inside_translated_cube() {
        let mut cube = make_unit_cube();
        cube.transform(Mat4d::translation(dvec3(10.0, 0.0, 0.0)));
        assert!(point_inside_mesh(dvec3(10.0, 0.0, 0.0), &cube));
        assert!(!point_inside_mesh(dvec3(0.0, 0.0, 0.0), &cube));
    }

    #[test]
    fn test_classify_non_overlapping() {
        let cube_a = make_unit_cube();
        let mut cube_b = make_unit_cube();
        cube_b.transform(Mat4d::translation(dvec3(5.0, 0.0, 0.0)));

        let on_boundary = vec![false; cube_a.triangle_count()];
        let classes = classify_triangles(&cube_a, &cube_b, &on_boundary);

        // All triangles of A should be outside B
        for &c in &classes {
            assert_eq!(c, TriLocation::Outside);
        }
    }

    #[test]
    fn test_classify_fully_inside() {
        // Small cube inside a big cube
        let mut small_cube = make_unit_cube();
        small_cube.transform(Mat4d::scale_uniform(0.1));

        let big_cube = make_unit_cube();

        let on_boundary = vec![false; small_cube.triangle_count()];
        let classes = classify_triangles(&small_cube, &big_cube, &on_boundary);

        // All triangles of small cube should be inside big cube
        for &c in &classes {
            assert_eq!(
                c,
                TriLocation::Inside,
                "small cube triangle should be inside big cube"
            );
        }
    }

    #[test]
    fn test_ray_triangle_basic() {
        let v0 = dvec3(-1.0, -1.0, 1.0);
        let v1 = dvec3(1.0, -1.0, 1.0);
        let v2 = dvec3(0.0, 1.0, 1.0);

        // Ray from origin along +Z
        assert!(ray_intersects_triangle(
            dvec3(0.0, 0.0, 0.0),
            dvec3(0.0, 0.0, 1.0),
            v0,
            v1,
            v2
        ));

        // Ray missing the triangle
        assert!(!ray_intersects_triangle(
            dvec3(5.0, 0.0, 0.0),
            dvec3(0.0, 0.0, 1.0),
            v0,
            v1,
            v2
        ));
    }
}

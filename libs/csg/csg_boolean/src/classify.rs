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
//
// Optimizations:
// - AABB tree accelerated ray casting: O(log n) per ray instead of O(n)
// - Thread pool for parallel classification (opt-out via `threads` feature)

use std::sync::Arc;

use crate::aabb_tree::AabbTree;
use crate::thread_pool;
use makepad_csg_math::{dvec3, Vec3d};
use makepad_csg_mesh::mesh::TriMesh;

/// Classification of a triangle relative to another solid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TriLocation {
    Inside,
    Outside,
    OnBoundary,
}

/// Work item for parallel classification.
#[derive(Clone, Copy)]
struct ClassifyWork {
    centroid: Vec3d,
    normal: Vec3d,
    is_boundary: bool,
}

/// Shared data for AABB-accelerated classification.
struct ClassifyContext {
    tree: AabbTree,
    tris: Vec<(Vec3d, Vec3d, Vec3d)>,
}

/// Classify all triangles of mesh A as inside/outside mesh B using ray casting.
///
/// For boundary triangles, we test both sides (+normal and -normal) to detect:
/// - Non-coplanar boundary faces: only interior side is inside → Inside
/// - Coplanar overlapping faces: both sides report inside → OnBoundary
///   (kept from mesh A only to avoid double-counting)
/// - Faces outside the other mesh: neither side inside → Outside
///
/// Uses an AABB tree for O(log n) ray casting and thread pool for parallelism.
pub fn classify_triangles(
    mesh_to_classify: &TriMesh,
    other_mesh: &TriMesh,
    on_boundary: &[bool],
) -> Vec<TriLocation> {
    let n = mesh_to_classify.triangle_count();
    if n == 0 {
        return Vec::new();
    }

    // Build AABB tree over other mesh for fast ray casting
    let other_tris: Vec<(Vec3d, Vec3d, Vec3d)> = (0..other_mesh.triangle_count())
        .map(|i| other_mesh.triangle_vertices(i))
        .collect();
    let tree = AabbTree::build(&other_tris);

    let ctx = Arc::new(ClassifyContext {
        tree,
        tris: other_tris,
    });

    // Pre-compute work items
    let work: Vec<ClassifyWork> = (0..n)
        .map(|ti| ClassifyWork {
            centroid: tri_centroid(mesh_to_classify, ti),
            normal: mesh_to_classify.triangle_normal(ti),
            is_boundary: on_boundary[ti],
        })
        .collect();

    // Parallel classification — Arc is cloned cheaply per thread.
    thread_pool::parallel_map(&work, move |chunk: &[ClassifyWork]| {
        chunk
            .iter()
            .map(|w| classify_one(w.centroid, w.normal, w.is_boundary, &ctx))
            .collect()
    })
}

/// Classify a single triangle given its centroid, normal, and boundary status.
fn classify_one(
    centroid: Vec3d,
    normal: Vec3d,
    is_boundary: bool,
    ctx: &ClassifyContext,
) -> TriLocation {
    if is_boundary {
        let eps = 1e-6;
        let inside_point = centroid - normal * eps;
        let outside_point = centroid + normal * eps;
        let interior_in = point_inside_bvh(inside_point, &ctx.tree, &ctx.tris);
        let exterior_in = point_inside_bvh(outside_point, &ctx.tree, &ctx.tris);

        if interior_in && exterior_in {
            TriLocation::OnBoundary
        } else if interior_in {
            TriLocation::Inside
        } else {
            TriLocation::Outside
        }
    } else {
        let inside = point_inside_bvh(centroid, &ctx.tree, &ctx.tris);
        if inside {
            TriLocation::Inside
        } else {
            let inward = centroid - normal * 1e-6;
            if point_inside_bvh(inward, &ctx.tree, &ctx.tris) {
                TriLocation::Inside
            } else {
                TriLocation::Outside
            }
        }
    }
}

/// Pre-built acceleration structure for repeated point-in-mesh queries.
pub struct MeshAccel {
    tree: AabbTree,
    tris: Vec<(Vec3d, Vec3d, Vec3d)>,
}

impl MeshAccel {
    /// Build an acceleration structure from a triangle mesh.
    pub fn build(mesh: &TriMesh) -> Self {
        let tris: Vec<(Vec3d, Vec3d, Vec3d)> = (0..mesh.triangle_count())
            .map(|i| mesh.triangle_vertices(i))
            .collect();
        let tree = AabbTree::build(&tris);
        MeshAccel { tree, tris }
    }

    /// Test if a point is inside the mesh.
    pub fn point_inside(&self, point: Vec3d) -> bool {
        point_inside_bvh(point, &self.tree, &self.tris)
    }
}

/// Test if a point is inside a closed triangle mesh using ray casting.
/// Uses AABB-accelerated ray-triangle intersection and multiple ray directions.
/// Note: rebuilds the AABB tree each call. For repeated queries, use MeshAccel.
pub fn point_inside_mesh(point: Vec3d, mesh: &TriMesh) -> bool {
    MeshAccel::build(mesh).point_inside(point)
}

/// Test if a point is inside a mesh using AABB tree traversal with majority vote.
fn point_inside_bvh(point: Vec3d, tree: &AabbTree, tris: &[(Vec3d, Vec3d, Vec3d)]) -> bool {
    let directions = [
        dvec3(0.8726, 0.3517, 0.1943),
        dvec3(-0.4123, 0.7891, 0.2345),
        dvec3(0.1234, -0.5678, 0.8901),
    ];

    let mut inside_votes = 0u32;
    for &ray_dir in &directions {
        let crossings = tree.ray_cast_count(point, ray_dir, tris);
        if crossings % 2 == 1 {
            inside_votes += 1;
        }
    }

    inside_votes >= 2
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

        for &c in &classes {
            assert_eq!(c, TriLocation::Outside);
        }
    }

    #[test]
    fn test_classify_fully_inside() {
        let mut small_cube = make_unit_cube();
        small_cube.transform(Mat4d::scale_uniform(0.1));

        let big_cube = make_unit_cube();

        let on_boundary = vec![false; small_cube.triangle_count()];
        let classes = classify_triangles(&small_cube, &big_cube, &on_boundary);

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
        use crate::aabb_tree::AabbTree;

        let v0 = dvec3(-1.0, -1.0, 1.0);
        let v1 = dvec3(1.0, -1.0, 1.0);
        let v2 = dvec3(0.0, 1.0, 1.0);

        let tris = vec![(v0, v1, v2)];
        let tree = AabbTree::build(&tris);

        let crossings = tree.ray_cast_count(dvec3(0.0, 0.0, 0.0), dvec3(0.0, 0.0, 1.0), &tris);
        assert_eq!(crossings, 1);

        let crossings = tree.ray_cast_count(dvec3(5.0, 0.0, 0.0), dvec3(0.0, 0.0, 1.0), &tris);
        assert_eq!(crossings, 0);
    }
}

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
    /// A face lying on the other mesh's surface and facing into it: two
    /// solids touching face to face (a box standing on a plate, letters on
    /// a face, parts side by side). Its interior side is outside the other
    /// mesh, its exterior side inside.
    Touching,
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
/// - Faces on the other mesh's surface facing into it (solids touching face
///   to face): only the exterior side inside → Touching
/// - Faces outside the other mesh: neither side inside → Outside
///
/// Uses an AABB tree for O(log n) ray casting and thread pool for parallelism.
pub fn classify_triangles(
    mesh_to_classify: &TriMesh,
    other_mesh: &TriMesh,
    on_boundary: &[bool],
) -> Vec<TriLocation> {
    classify_triangles_in(mesh_to_classify, &MeshAccel::build(other_mesh), on_boundary)
}

/// [`classify_triangles`] against a prepared mesh (its triangles and tree
/// built once, for every boolean the mesh takes part in).
pub fn classify_triangles_in(mesh_to_classify: &TriMesh, other: &MeshAccel, on_boundary: &[bool]) -> Vec<TriLocation> {
    let n = mesh_to_classify.triangle_count();
    if n == 0 {
        return Vec::new();
    }
    let ctx = other.ctx.clone();

    // A connected patch of non-boundary triangles (joined across shared
    // edges) does not cross the other mesh's surface, so it lies wholly
    // inside or outside: classify a few of its triangles (majority) and give
    // the patch their answer. Boundary triangles are classified one by one.
    let (patch, patches) = non_boundary_patches(mesh_to_classify, on_boundary);
    let mut members: Vec<Vec<u32>> = vec![Vec::new(); patches];
    let mut work_tris: Vec<u32> = Vec::new();
    for ti in 0..n {
        match patch[ti] {
            u32::MAX => work_tris.push(ti as u32),
            p => members[p as usize].push(ti as u32),
        }
    }
    // Up to three samples per patch, spread over its triangles.
    let mut samples: Vec<(u32, usize)> = Vec::new(); // (patch, first work index)
    for (p, m) in members.iter().enumerate() {
        samples.push((p as u32, work_tris.len()));
        let mut picks = [0, m.len() / 2, m.len() - 1];
        picks.sort_unstable();
        let mut last = usize::MAX;
        for k in picks {
            if k != last {
                work_tris.push(m[k]);
                last = k;
            }
        }
    }
    let work: Vec<ClassifyWork> = work_tris
        .iter()
        .map(|&ti| {
            let ti = ti as usize;
            ClassifyWork { centroid: tri_centroid(mesh_to_classify, ti), normal: mesh_to_classify.triangle_normal(ti), is_boundary: on_boundary[ti] }
        })
        .collect();

    // Parallel classification — Arc is cloned cheaply per thread.
    let found = thread_pool::parallel_map(&work, move |chunk: &[ClassifyWork]| {
        chunk
            .iter()
            .take_while(|_| !thread_pool::cancelled())
            .map(|w| classify_one(w.centroid, w.normal, w.is_boundary, &ctx))
            .collect()
    });
    if found.len() != work.len() {
        return Vec::new(); // cancelled
    }
    let mut out = vec![TriLocation::Outside; n];
    let first_sample = samples.first().map_or(work_tris.len(), |s| s.1);
    for k in 0..first_sample {
        out[work_tris[k] as usize] = found[k];
    }
    for (i, &(p, start)) in samples.iter().enumerate() {
        let end = samples.get(i + 1).map_or(work_tris.len(), |s| s.1);
        let inside = found[start..end].iter().filter(|&&c| c == TriLocation::Inside).count();
        let class = if inside * 2 > end - start { TriLocation::Inside } else { TriLocation::Outside };
        for &ti in &members[p as usize] {
            out[ti as usize] = class;
        }
    }
    out
}

/// Connected patches of non-boundary triangles over shared edges: per
/// triangle its patch (`u32::MAX` for boundary triangles), and the count.
fn non_boundary_patches(mesh: &TriMesh, on_boundary: &[bool]) -> (Vec<u32>, usize) {
    let n = mesh.triangle_count();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    fn find(parent: &mut [u32], mut x: u32) -> u32 {
        while parent[x as usize] != x {
            parent[x as usize] = parent[parent[x as usize] as usize];
            x = parent[x as usize];
        }
        x
    }
    let mut edges: Vec<(u64, u32)> = Vec::with_capacity(n * 3);
    for (ti, &[a, b, c]) in mesh.triangles.iter().enumerate() {
        if on_boundary[ti] {
            continue;
        }
        for (u, v) in [(a, b), (b, c), (c, a)] {
            let (lo, hi) = if u < v { (u, v) } else { (v, u) };
            edges.push((((lo as u64) << 32) | hi as u64, ti as u32));
        }
    }
    edges.sort_unstable();
    for w in edges.windows(2) {
        if w[0].0 == w[1].0 {
            let (x, y) = (find(&mut parent, w[0].1), find(&mut parent, w[1].1));
            if x != y {
                let (lo, hi) = if x < y { (x, y) } else { (y, x) };
                parent[hi as usize] = lo;
            }
        }
    }
    let mut patch = vec![u32::MAX; n];
    let mut ids = vec![u32::MAX; n];
    let mut count = 0usize;
    for ti in 0..n {
        if on_boundary[ti] {
            continue;
        }
        let r = find(&mut parent, ti as u32) as usize;
        if ids[r] == u32::MAX {
            ids[r] = count as u32;
            count += 1;
        }
        patch[ti] = ids[r];
    }
    (patch, count)
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
        } else if exterior_in {
            TriLocation::Touching
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

/// A mesh prepared for booleans: its triangles' corners and their AABB
/// tree, built once and shared (cheaply cloned) by the overlap search, the
/// classification and point-in-mesh queries of every boolean it takes part in.
#[derive(Clone)]
pub struct MeshAccel {
    ctx: Arc<ClassifyContext>,
}

impl MeshAccel {
    /// Build an acceleration structure from a triangle mesh.
    pub fn build(mesh: &TriMesh) -> Self {
        let tris: Vec<(Vec3d, Vec3d, Vec3d)> = (0..mesh.triangle_count())
            .map(|i| mesh.triangle_vertices(i))
            .collect();
        let tree = AabbTree::build(&tris);
        MeshAccel { ctx: Arc::new(ClassifyContext { tree, tris }) }
    }

    /// Test if a point is inside the mesh.
    pub fn point_inside(&self, point: Vec3d) -> bool {
        point_inside_bvh(point, &self.ctx.tree, &self.ctx.tris)
    }

    pub fn tree(&self) -> &AabbTree {
        &self.ctx.tree
    }

    pub fn triangles(&self) -> &[(Vec3d, Vec3d, Vec3d)] {
        &self.ctx.tris
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
        if thread_pool::cancelled() {
            return false;
        }
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

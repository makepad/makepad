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

use crate::aabb_tree::{ray_intersects_triangle, AabbTree};
use crate::thread_pool;
use makepad_csg_math::{dvec3, BBox3d, Vec3d};
use makepad_csg_mesh::mesh::TriMesh;

/// Classification of a triangle relative to another solid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TriLocation {
    Inside,
    Outside,
    /// A shared surface with the same outward winding.
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
    coplanar: Option<bool>,
}

/// Shared data for AABB-accelerated classification.
struct ClassifyContext {
    tree: AabbTree,
    tris: Vec<(Vec3d, Vec3d, Vec3d)>,
}

/// Classify all triangles of mesh A as inside/outside mesh B using ray casting.
///
/// Coplanar regions are classified by projected containment and winding.
/// Other corefined regions are queried at their centroid, without offsets
/// that could cross a nearby surface. Non-boundary patches share a result.
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
    // The boolean pipeline already has these pairs from corefinement. This
    // standalone entry point discovers them for callers with a refined mesh.
    let prepared = MeshAccel::build(mesh_to_classify);
    let source = prepared.triangles();
    let target = other.triangles();
    let mut pairs = vec![Vec::new(); source.len()];
    for (a, b) in prepared.tree().find_overlaps(other.tree()) {
        if !on_boundary[a as usize] { continue; }
        let (a0,a1,a2) = source[a as usize];
        let (b0,b1,b2) = target[b as usize];
        if crate::tri_tri::tri_tri_intersection(a0,a1,a2,b0,b1,b2).coplanar {
            pairs[a as usize].push(b);
        }
    }
    let origins: Vec<_> = (0..source.len() as u32).collect();
    let coplanar = crate::corefine::coplanar_regions(mesh_to_classify, &origins, &pairs, source, target);
    classify_triangles_with_coplanar(mesh_to_classify, &InsideTest::whole(other), on_boundary, &coplanar)
}

/// The solid a closed mesh bounds, as the classification queries it: by rays to infinity against the whole mesh,
/// or, for points of a region, by segments within it against the mesh's triangles that meet it.
#[derive(Clone)]
pub(crate) struct InsideTest {
    ctx: Arc<ClassifyContext>,
    /// Three points of the region and whether each is inside: a point's side is a reference's, flipped by each
    /// crossing of the segment between them.
    refs: Option<[(Vec3d, bool); 3]>,
}

impl InsideTest {
    /// Rays against `accel`, the whole mesh ([`point_inside_bvh`]).
    pub(crate) fn whole(accel: &MeshAccel) -> Self {
        InsideTest { ctx: accel.ctx.clone(), refs: None }
    }

    /// Points of `region` against the solid `mesh` bounds, with `near` the accelerator of every triangle of
    /// `mesh` that meets `region`. Inside the (convex) region the surface is those triangles, so the crossings of
    /// a segment from a point to a reference point of the region flip the reference's side. Each of the three
    /// references is classified once, against the whole mesh by the three rays of [`point_inside_bvh`]; a point
    /// takes the majority of its three segments, as a whole query takes the majority of its rays.
    pub(crate) fn local(near: &MeshAccel, region: BBox3d, mesh: &TriMesh) -> Self {
        // Fixed points at irrational fractions of the region: off the axis-aligned planes a model's faces lie in.
        let (lo, size) = (region.min, region.size());
        let at = |f: [f64; 3]| dvec3(lo.x + size.x * f[0], lo.y + size.y * f[1], lo.z + size.z * f[2]);
        let points = [at([0.3183, 0.5772, 0.6931]), at([0.7071, 0.3679, 0.4142]), at([0.4472, 0.8284, 0.2361])];
        let dirs = RAY_DIRECTIONS;
        let inv = dirs.map(|d| dvec3(1.0 / d.x, 1.0 / d.y, 1.0 / d.z));
        // One pass over the whole mesh for the nine rays, each hit test as the tree's leaf test: in parallel
        // chunks of triangles, their hit counts summed.
        let counts = ray_counts(mesh, move |(a, b, c), counts| {
            let bbox = BBox3d::from_triangle(a, b, c);
            for (k, &p) in points.iter().enumerate() {
                for j in 0..3 {
                    if bbox.ray_intersects(p, inv[j]) && ray_intersects_triangle(p, dirs[j], a, b, c) {
                        counts[k][j] += 1;
                    }
                }
            }
        });
        let refs = std::array::from_fn(|k| (points[k], counts[k].iter().filter(|&&c| c % 2 == 1).count() >= 2));
        InsideTest { ctx: near.ctx.clone(), refs: Some(refs) }
    }

    fn inside(&self, point: Vec3d) -> bool {
        let Some(refs) = &self.refs else { return point_inside_bvh(point, &self.ctx.tree, &self.ctx.tris) };
        let mut inside_votes = 0u32;
        for &(r, r_inside) in refs {
            if thread_pool::cancelled() {
                return false;
            }
            let odd = self.ctx.tree.segment_cross_count(point, r, &self.ctx.tris) % 2 == 1;
            if odd != r_inside {
                inside_votes += 1;
            }
        }
        inside_votes >= 2
    }
}

/// `count` over every triangle of `mesh`, in parallel chunks, its counts summed.
fn ray_counts(mesh: &TriMesh, count: impl Fn((Vec3d, Vec3d, Vec3d), &mut [[u32; 3]; 3]) + Send + Sync + Copy + 'static) -> [[u32; 3]; 3] {
    // Nine slab tests a triangle, some 40 ns: chunks below 16384 cost more to hand off than they save.
    const CHUNK: usize = 16384;
    let n = mesh.triangle_count();
    let chunks = n.div_ceil(CHUNK).min(thread_pool::thread_count()).max(1);
    let size = n.div_ceil(chunks);
    let chunk = move |vertices: &[Vec3d], triangles: &[[u32; 3]]| {
        let mut counts = [[0u32; 3]; 3];
        for (i, t) in triangles.iter().enumerate() {
            if i % 4096 == 0 && thread_pool::cancelled() {
                break;
            }
            count((vertices[t[0] as usize], vertices[t[1] as usize], vertices[t[2] as usize]), &mut counts);
        }
        counts
    };
    let parts = if chunks == 1 {
        vec![chunk(&mesh.vertices, &mesh.triangles)]
    } else {
        let shared: Arc<(Vec<Vec3d>, Vec<[u32; 3]>)> = Arc::new((mesh.vertices.clone(), mesh.triangles.clone()));
        thread_pool::parallel_for(
            (0..chunks)
                .map(|c| {
                    let shared = shared.clone();
                    move || chunk(&shared.0, &shared.1[(c * size).min(n)..((c + 1) * size).min(n)])
                })
                .collect(),
        )
    };
    let mut total = [[0u32; 3]; 3];
    for counts in parts {
        for k in 0..3 {
            for j in 0..3 {
                total[k][j] += counts[k][j];
            }
        }
    }
    total
}

pub(crate) fn classify_triangles_with_coplanar(mesh_to_classify: &TriMesh, other: &InsideTest, on_boundary: &[bool], coplanar: &[Option<bool>]) -> Vec<TriLocation> {
    let n = mesh_to_classify.triangle_count();
    if n == 0 {
        return Vec::new();
    }
    let ctx = other.clone();

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
            ClassifyWork { centroid: tri_centroid(mesh_to_classify, ti), coplanar: coplanar.get(ti).copied().flatten() }
        })
        .collect();

    // Parallel classification — Arc is cloned cheaply per thread.
    let found = thread_pool::parallel_map(&work, move |chunk: &[ClassifyWork]| {
        chunk
            .iter()
            .take_while(|_| !thread_pool::cancelled())
            .map(|w| match w.coplanar {
                Some(true) => TriLocation::OnBoundary,
                Some(false) => TriLocation::Touching,
                None => classify_one(w.centroid, &ctx),
            })
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
/// A union keeps the lower root, so a patch's root is its lowest triangle
/// whichever order its shared edges join it in: the patches are numbered in
/// the order of their lowest triangles.
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
    // The non-boundary triangles round each vertex.
    let vertices = mesh.vertex_count();
    let mut first = vec![0u32; vertices + 1];
    for (ti, t) in mesh.triangles.iter().enumerate() {
        if !on_boundary[ti] {
            for &v in t {
                first[v as usize + 1] += 1;
            }
        }
    }
    for v in 0..vertices {
        first[v + 1] += first[v];
    }
    let mut next = first.clone();
    let mut around = vec![0u32; first[vertices] as usize];
    for (ti, t) in mesh.triangles.iter().enumerate() {
        if !on_boundary[ti] {
            for &v in t {
                around[next[v as usize] as usize] = ti as u32;
                next[v as usize] += 1;
            }
        }
    }
    let edges = |[a, b, c]: [u32; 3]| [(a, b), (b, c), (c, a)].map(|(u, v)| if u < v { (u, v) } else { (v, u) });
    // A triangle joins each later one with an edge of its own: one round the edge's lower end.
    for (ti, &t) in mesh.triangles.iter().enumerate() {
        if on_boundary[ti] {
            continue;
        }
        for e in edges(t) {
            for &si in &around[first[e.0 as usize] as usize..first[e.0 as usize + 1] as usize] {
                if si as usize > ti && edges(mesh.triangles[si as usize]).contains(&e) {
                    let (x, y) = (find(&mut parent, ti as u32), find(&mut parent, si));
                    if x != y {
                        let (lo, hi) = if x < y { (x, y) } else { (y, x) };
                        parent[hi as usize] = lo;
                    }
                }
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

/// A corefined region not covered by a coplanar face lies wholly inside or
/// outside. Query the region itself: an offset may cross a nearby surface.
fn classify_one(centroid: Vec3d, ctx: &InsideTest) -> TriLocation {
    if ctx.inside(centroid) {
        TriLocation::Inside
    } else {
        TriLocation::Outside
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

/// The rays of a point query: off the axes, so axis-aligned faces and edges are not grazed.
const RAY_DIRECTIONS: [Vec3d; 3] = [
    dvec3(0.8726, 0.3517, 0.1943),
    dvec3(-0.4123, 0.7891, 0.2345),
    dvec3(0.1234, -0.5678, 0.8901),
];

/// Test if a point is inside a mesh using AABB tree traversal with majority vote.
fn point_inside_bvh(point: Vec3d, tree: &AabbTree, tris: &[(Vec3d, Vec3d, Vec3d)]) -> bool {
    let mut inside_votes = 0u32;
    for ray_dir in RAY_DIRECTIONS {
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

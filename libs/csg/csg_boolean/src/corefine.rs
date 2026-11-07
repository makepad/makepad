// Mesh corefinement: split triangles at intersection curves.
//
// Given two triangle meshes A and B, corefinement modifies both meshes so that
// the intersection curve is represented as edges in both meshes. After corefinement,
// both meshes share edges along their intersection, enabling clean boolean operations.
//
// Algorithm:
// 1. AABB broad phase: find candidate triangle pairs.
// 2. Triangle-triangle intersection: compute intersection segments.
// 3. Collect all intersection points and segments per triangle.
// 4. Re-triangulate affected triangles using CDT.

use crate::aabb_tree::AabbTree;
use crate::classify::MeshAccel;
use crate::cdt::{Point2, CDT};
use crate::tri_tri::{construct_edge_plane_intersection, tri_tri_intersection_indexed_with_tolerance, EdgeIsect};
use makepad_csg_math::{orient2d, thread_pool, Vec3d};
use makepad_csg_mesh::mesh::TriMesh;
use makepad_csg_math::fast_hash::FastMap as HashMap;
use std::sync::Arc;

/// Information about an intersection point.
#[derive(Clone, Debug)]
pub struct IntersectionPoint {
    pub pos: Vec3d,
    /// Which mesh (0=A, 1=B) this point is associated with.
    pub mesh_id: u32,
    /// Triangle index in the source mesh.
    pub tri_idx: u32,
}

/// An intersection segment between two triangles.
#[derive(Clone, Debug)]
pub struct IntersectionSegment {
    /// Triangle index in mesh A.
    pub tri_a: u32,
    /// Triangle index in mesh B.
    pub tri_b: u32,
    /// Endpoints of the intersection segment.
    pub p0: Vec3d,
    pub p1: Vec3d,
    /// Indices into the shared vertex pool.
    pub v0_idx: u32,
    pub v1_idx: u32,
}

/// Result of corefinement: two refined meshes that share edges along their intersection.
#[derive(Clone, Debug)]
pub struct CorefinementResult {
    /// Refined mesh A.
    pub mesh_a: TriMesh,
    /// Refined mesh B.
    pub mesh_b: TriMesh,
    /// For each triangle in mesh_a, which original triangle it came from.
    pub origin_a: Vec<u32>,
    /// For each triangle in mesh_b, which original triangle it came from.
    pub origin_b: Vec<u32>,
    /// For each triangle in mesh_a, is it on the intersection boundary?
    pub on_boundary_a: Vec<bool>,
    /// For each triangle in mesh_b, is it on the intersection boundary?
    pub on_boundary_b: Vec<bool>,
    /// Coplanar coverage per refined face: true for equal winding, false
    /// for opposite winding, None outside the shared surface.
    pub coplanar_a: Vec<Option<bool>>,
    pub coplanar_b: Vec<Option<bool>>,
    /// Number of intersection segments found between the two meshes.
    /// Zero means the meshes were copied through untouched (no triangle was
    /// split); the boolean's finishing passes can then be skipped entirely.
    pub num_segments: usize,
}

/// Perform mesh corefinement on two triangle meshes.
///
/// Both meshes are split along their intersection curve so that the intersection
/// is represented as edges in both output meshes.
pub fn corefine(mesh_a: &TriMesh, mesh_b: &TriMesh) -> CorefinementResult {
    corefine_in(mesh_a, &MeshAccel::build(mesh_a), mesh_b, &MeshAccel::build(mesh_b))
}

/// [`corefine`] of prepared meshes (their triangle lists and trees built once).
pub fn corefine_in(mesh_a: &TriMesh, prep_a: &MeshAccel, mesh_b: &TriMesh, prep_b: &MeshAccel) -> CorefinementResult {
    // Step 1: the AABB trees (prepared)
    let (tris_a, tris_b) = (prep_a.triangles(), prep_b.triangles());
    let (tree_a, tree_b): (&AabbTree, &AabbTree) = (prep_a.tree(), prep_b.tree());

    // Step 2: Find candidate pairs
    let bounds = mesh_a.bounding_box().union(mesh_b.bounding_box());
    let coincidence = crate::tri_tri::plane_tolerance(
        bounds.min, bounds.max, bounds.min, bounds.max, bounds.min, bounds.max,
    );
    let pairs = tree_a.find_overlaps_with_tolerance(tree_b, coincidence);

    // Step 3: Compute intersection segments for each pair.
    //
    // Edge-centric deduplication: when two triangles from mesh A share an edge
    // and both intersect the same triangle from mesh B, the intersection point
    // on that shared edge should be identical. Without deduplication, floating-
    // point differences produce two slightly different points (~1e-5 apart),
    // creating sliver triangles and non-manifold edges. Each (edge,
    // other_triangle) intersection is constructed exactly once, for every
    // pair that shares it.
    //
    // The pairs' tests and the constructions are each a function of their own
    // triangles alone, so they run in parallel ([`meeting_pairs`],
    // [`edge_points`]); the shared points and segments are built from them in
    // the pairs' order.
    let meeting = meeting_pairs(pairs, (mesh_a, prep_a), (mesh_b, prep_b), coincidence);
    let mut edge_slots: HashMap<EdgeIsect, u32> = HashMap::default();
    let mut edge_jobs: Vec<EdgePlane> = Vec::new();
    let mut slot = |eid: EdgeIsect| -> u32 {
        *edge_slots.entry(eid).or_insert_with(|| {
            let (mesh, tris) = if eid.mesh_id == 0 { (mesh_a, tris_b) } else { (mesh_b, tris_a) };
            edge_jobs.push((mesh.vertices[eid.edge_v0 as usize], mesh.vertices[eid.edge_v1 as usize], tris[eid.other_tri as usize]));
            (edge_jobs.len() - 1) as u32
        })
    };
    let ends: Vec<[u32; 2]> = meeting
        .iter()
        .map(|(_, _, m)| match m {
            Meeting::Crossing(eid0, eid1) => [slot(*eid0), slot(*eid1)],
            Meeting::Coplanar(_) => [u32::MAX; 2],
        })
        .collect();
    let edge_points = edge_points(edge_jobs);

    let mut segments: Vec<IntersectionSegment> = Vec::new();
    let mut shared_vert_grid = VertexGrid::new(1e-10);
    let mut coplanar_a = vec![Vec::new(); mesh_a.triangle_count()];
    let mut coplanar_b = vec![Vec::new(); mesh_b.triangle_count()];
    for ((ai, bi, meeting), [end0, end1]) in meeting.into_iter().zip(ends) {
        if thread_pool::cancelled() {
            break;
        }
        match meeting {
            Meeting::Coplanar(clip_edges) => {
                coplanar_a[ai as usize].push(bi);
                coplanar_b[bi as usize].push(ai);
                // Coplanar overlapping triangles: the boundary of their 2D
                // intersection polygon (Sutherland-Hodgman clipping), each
                // edge a constraint segment for both triangles.
                for (p0, p1) in clip_edges {
                    if p0.distance(p1) < 1e-12 {
                        continue;
                    }
                    let v0_idx = shared_vert_grid.add(p0);
                    let v1_idx = shared_vert_grid.add(p1);
                    if v0_idx != v1_idx {
                        segments.push(IntersectionSegment {
                            tri_a: ai,
                            tri_b: bi,
                            p0,
                            p1,
                            v0_idx,
                            v1_idx,
                        });
                    }
                }
            }
            Meeting::Crossing(..) => {
                let (p0, p1) = (edge_points[end0 as usize], edge_points[end1 as usize]);
                // Skip degenerate segments
                if p0.distance(p1) < 1e-12 {
                    continue;
                }
                let v0_idx = shared_vert_grid.add(p0);
                let v1_idx = shared_vert_grid.add(p1);
                if v0_idx != v1_idx {
                    segments.push(IntersectionSegment {
                        tri_a: ai,
                        tri_b: bi,
                        p0,
                        p1,
                        v0_idx,
                        v1_idx,
                    });
                }
            }
        }
    }

    // Step 4: Collect intersection data per triangle
    let mut tri_a_segments: Vec<Vec<usize>> = vec![Vec::new(); mesh_a.triangle_count()];
    let mut tri_b_segments: Vec<Vec<usize>> = vec![Vec::new(); mesh_b.triangle_count()];

    for (si, seg) in segments.iter().enumerate() {
        if thread_pool::cancelled() {
            break;
        }
        tri_a_segments[seg.tri_a as usize].push(si);
        tri_b_segments[seg.tri_b as usize].push(si);
    }

    // Step 5: Re-triangulate affected triangles
    let shared_verts: Arc<[Vec3d]> = shared_vert_grid.into_verts().into();
    let segments: Arc<[IntersectionSegment]> = segments.into();
    let [pieces_a, pieces_b] = cut_pieces([(mesh_a, &tri_a_segments), (mesh_b, &tri_b_segments)], &segments, &shared_verts);
    let (result_a, origin_a, boundary_a) = retriangulate_mesh(mesh_a, &tri_a_segments, pieces_a);
    let (result_b, origin_b, boundary_b) = retriangulate_mesh(mesh_b, &tri_b_segments, pieces_b);

    let coplanar_a = coplanar_regions(&result_a, &origin_a, &coplanar_a, tris_a, tris_b);
    let coplanar_b = coplanar_regions(&result_b, &origin_b, &coplanar_b, tris_b, tris_a);
    CorefinementResult {
        mesh_a: result_a,
        mesh_b: result_b,
        origin_a,
        origin_b,
        on_boundary_a: boundary_a,
        on_boundary_b: boundary_b,
        coplanar_a,
        coplanar_b,
        num_segments: segments.len(),
    }
}

/// What a candidate pair's triangles share.
enum Meeting {
    /// They are coplanar and overlap: the edges of their overlap.
    Coplanar(Vec<(Vec3d, Vec3d)>),
    /// They cross: the edges their segment's ends lie on, each with the
    /// other triangle whose plane it crosses.
    Crossing(EdgeIsect, EdgeIsect),
}

/// An edge's ends and the plane (a triangle) it crosses.
type EdgePlane = (Vec3d, Vec3d, (Vec3d, Vec3d, Vec3d));

/// The candidate pairs whose triangles meet, with what they share, in the
/// pairs' order. Each test depends on its two triangles alone, so they run in
/// parallel chunks.
fn meeting_pairs(
    pairs: Vec<(u32, u32)>,
    (mesh_a, prep_a): (&TriMesh, &MeshAccel),
    (mesh_b, prep_b): (&TriMesh, &MeshAccel),
    coincidence: f64,
) -> Vec<(u32, u32, Meeting)> {
    // A chunk's tests take some 0.5 ms: below that a pool hand-off costs more than it saves.
    const CHUNK: usize = 1024;
    let corners: Arc<(Vec<[u32; 3]>, Vec<[u32; 3]>)> = Arc::new((mesh_a.triangles.clone(), mesh_b.triangles.clone()));
    let (prep_a, prep_b) = (prep_a.clone(), prep_b.clone());
    parallel_chunks(pairs, CHUNK, move |chunk| {
        let (tris_a, tris_b) = (prep_a.triangles(), prep_b.triangles());
        let mut meeting = Vec::new();
        for &(ai, bi) in chunk {
            if thread_pool::cancelled() {
                break;
            }
            let (a0, a1, a2) = tris_a[ai as usize];
            let (b0, b1, b2) = tris_b[bi as usize];
            let (a_vi, b_vi) = (corners.0[ai as usize], corners.1[bi as usize]);
            let result = tri_tri_intersection_indexed_with_tolerance(a0, a1, a2, a_vi, ai, b0, b1, b2, b_vi, bi, coincidence);
            if !result.intersects {
                continue;
            }
            if result.coplanar {
                meeting.push((ai, bi, Meeting::Coplanar(coplanar_intersection_edges(a0, a1, a2, b0, b1, b2))));
            } else if let (Some(_), Some((eid0, eid1))) = (result.segment, result.edge_ids) {
                meeting.push((ai, bi, Meeting::Crossing(eid0, eid1)));
            }
        }
        meeting
    })
}

/// Each edge's point on the plane it crosses
/// ([`construct_edge_plane_intersection`]), in parallel chunks.
fn edge_points(jobs: Vec<EdgePlane>) -> Vec<Vec3d> {
    // A construction is exact arithmetic, some 10 us.
    const CHUNK: usize = 64;
    parallel_chunks(jobs, CHUNK, |chunk| chunk.iter().map(|&(a, b, plane)| construct_edge_plane_intersection(a, b, plane)).collect())
}

/// `work` over `items` in chunks of at least `chunk` items, as many as the
/// pool has threads, their results concatenated in the items' order.
fn parallel_chunks<T, R, F>(items: Vec<T>, chunk: usize, work: F) -> Vec<R>
where
    T: Send + Sync + 'static,
    R: Send + 'static,
    F: Fn(&[T]) -> Vec<R> + Send + Sync + 'static,
{
    let chunks = items.len().div_ceil(chunk).min(thread_pool::thread_count()).max(1);
    if chunks == 1 {
        return work(&items);
    }
    let size = items.len().div_ceil(chunks);
    let (items, work): (Arc<[T]>, Arc<F>) = (items.into(), Arc::new(work));
    let tasks: Vec<_> = (0..chunks)
        .map(|c| {
            let (items, work) = (items.clone(), work.clone());
            move || work(&items[(c * size).min(items.len())..((c + 1) * size).min(items.len())])
        })
        .collect();
    thread_pool::parallel_for(tasks).into_iter().flatten().collect()
}

/// Corefinement has already found the coplanar pairs and cut their boundaries.
/// Classify each resulting region in that plane, retaining its source winding.
/// A centroid query is sufficient only AFTER those constraints have been cut.
pub(crate) fn coplanar_regions(
    refined: &TriMesh,
    origins: &[u32],
    pairs: &[Vec<u32>],
    source: &[(Vec3d, Vec3d, Vec3d)],
    other: &[(Vec3d, Vec3d, Vec3d)],
) -> Vec<Option<bool>> {
    origins.iter().enumerate().map(|(ti, &origin)| {
        let candidates = &pairs[origin as usize];
        if candidates.is_empty() { return None; }
        let (a, b, c) = source[origin as usize];
        let normal = (b - a).cross(c - a);
        let (i, j) = if normal.x.abs() >= normal.y.abs() && normal.x.abs() >= normal.z.abs() {
            (1, 2)
        } else if normal.y.abs() >= normal.z.abs() { (0, 2) } else { (0, 1) };
        let project = |v: Vec3d| { let v = [v.x, v.y, v.z]; (v[i], v[j]) };
        let winding = orient_2d_tuple(project(a), project(b), project(c));
        let (u, v, w) = refined.triangle_vertices(ti);
        let centroid = project((u + v + w) / 3.0);
        candidates.iter().find_map(|&bi| {
            let (a, b, c) = other[bi as usize];
            let (a, b, c) = (project(a), project(b), project(c));
            let other_winding = orient_2d_tuple(a, b, c);
            (other_winding != 0.0 && point_in_triangle_2d(centroid, a, b, c))
                .then_some((winding > 0.0) == (other_winding > 0.0))
        })
    }).collect()
}

/// Spatial hash grid for fast vertex deduplication during corefinement.
struct VertexGrid {
    verts: Vec<Vec3d>,
    grid: HashMap<(i64, i64, i64), Vec<u32>>,
    inv_cell: f64,
    tol_sq: f64,
}

impl VertexGrid {
    fn new(tolerance: f64) -> Self {
        let cell = tolerance * 2.0;
        VertexGrid {
            verts: Vec::new(),
            grid: HashMap::default(),
            inv_cell: 1.0 / cell,
            tol_sq: tolerance * tolerance,
        }
    }

    /// Add a vertex, merging with existing if within tolerance. Returns index.
    fn add(&mut self, p: Vec3d) -> u32 {
        let cx = (p.x * self.inv_cell).floor() as i64;
        let cy = (p.y * self.inv_cell).floor() as i64;
        let cz = (p.z * self.inv_cell).floor() as i64;

        // Search 3x3x3 neighborhood
        for dx in -1i64..=1 {
            for dy in -1i64..=1 {
                for dz in -1i64..=1 {
                    if let Some(bucket) = self.grid.get(&(cx + dx, cy + dy, cz + dz)) {
                        for &idx in bucket {
                            if self.verts[idx as usize].distance_sq(p) < self.tol_sq {
                                return idx;
                            }
                        }
                    }
                }
            }
        }

        let idx = self.verts.len() as u32;
        self.verts.push(p);
        self.grid.entry((cx, cy, cz)).or_default().push(idx);
        idx
    }

    fn into_verts(self) -> Vec<Vec3d> {
        self.verts
    }
}

/// Re-triangulate a mesh, splitting triangles that are intersected.
///
/// Emit triangle-local vertices here and let the boolean finisher weld them.
/// That historical representation is intentional: exact-position indexing
/// before selection lost cascading intersection seams in staged booleans.
/// Keeps the mesh indexed: the input's vertex array is preserved (untouched
/// triangles keep their original index triples) and only vertices introduced
/// by the CDT re-triangulation are appended, deduplicated by exact position
/// bits. Every position emitted by `retriangulate_triangle` is bitwise equal
/// to either one of the triangle's own corners or an entry of the shared
/// intersection-vertex pool, so exact-bits deduplication is lossless. This
/// matters twice: downstream welding runs on ~6x fewer vertices, and a
/// zero-segment corefinement can skip finishing entirely because the copied
/// meshes are still properly indexed.
///
/// `pieces`: each cut triangle's new triangles ([`cut_pieces`]), in triangle order.
fn retriangulate_mesh(
    mesh: &TriMesh,
    tri_segments: &[Vec<usize>],
    pieces: Vec<Vec<[Vec3d; 3]>>,
) -> (TriMesh, Vec<u32>, Vec<bool>) {
    let mut pieces = pieces.into_iter();
    let mut result = TriMesh::with_capacity(mesh.vertex_count(), mesh.triangle_count());
    result.vertices.extend_from_slice(&mesh.vertices);
    let mut origins: Vec<u32> = Vec::with_capacity(mesh.triangle_count());
    let mut boundaries: Vec<bool> = Vec::with_capacity(mesh.triangle_count());

    // Exact-position map for vertices appended beyond the original array.
    let mut new_vert_map: HashMap<[u64; 3], u32> = HashMap::default();
    let key = |v: Vec3d| -> [u64; 3] { [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()] };

    for ti in 0..mesh.triangle_count() {
        if thread_pool::cancelled() {
            break;
        }
        let segs = &tri_segments[ti];
        let [i0, i1, i2] = mesh.triangles[ti];
        let (v0, v1, v2) = mesh.triangle_vertices(ti);

        if segs.is_empty() {
            // No intersection: keep the original indexed triangle as-is.
            result.add_triangle(i0, i1, i2);
            origins.push(ti as u32);
            boundaries.push(false);
        } else {
            // Triangle is intersected: re-triangulated using CDT
            let new_tris = pieces.next().expect("every cut triangle has its pieces");
            for [a, b, c] in new_tris {
                let mut map_vert = |p: Vec3d| -> u32 {
                    // Original corners come through bitwise-unchanged.
                    if key(p) == key(v0) {
                        return i0;
                    }
                    if key(p) == key(v1) {
                        return i1;
                    }
                    if key(p) == key(v2) {
                        return i2;
                    }
                    match new_vert_map.entry(key(p)) {
                        std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                        std::collections::hash_map::Entry::Vacant(e) => {
                            let idx = result.vertices.len() as u32;
                            result.vertices.push(p);
                            e.insert(idx);
                            idx
                        }
                    }
                };
                let ia = map_vert(a);
                let ib = map_vert(b);
                let ic = map_vert(c);
                result.add_triangle(ia, ib, ic);
                origins.push(ti as u32);
                boundaries.push(true);
            }
        }
    }

    (result, origins, boundaries)
}

/// The new triangles of each cut triangle (one with segments) of both meshes
/// ([`retriangulate_triangle`]), per mesh in triangle order. Each depends on its
/// own triangle and segments alone, so they are made in parallel chunks.
fn cut_pieces(
    meshes: [(&TriMesh, &[Vec<usize>]); 2],
    segments: &Arc<[IntersectionSegment]>,
    shared_verts: &Arc<[Vec3d]>,
) -> [Vec<Vec<[Vec3d; 3]>>; 2] {
    // Each cut triangle: its mesh (A first), its corners and its segments.
    let cut: Vec<(bool, [Vec3d; 3], Vec<usize>)> = meshes
        .iter()
        .enumerate()
        .flat_map(|(m, &(mesh, tri_segments))| {
            tri_segments.iter().enumerate().filter(|(_, segs)| !segs.is_empty()).map(move |(ti, segs)| {
                let (v0, v1, v2) = mesh.triangle_vertices(ti);
                (m == 0, [v0, v1, v2], segs.clone())
            })
        })
        .collect();
    // A cut triangle's CDT takes some 5 us.
    const CHUNK: usize = 64;
    let (segments, shared_verts) = (segments.clone(), shared_verts.clone());
    let made = parallel_chunks(cut, CHUNK, move |chunk| {
        chunk.iter().map(|(is_a, [v0, v1, v2], segs)| (*is_a, retriangulate_triangle(*v0, *v1, *v2, segs, &segments, &shared_verts, *is_a))).collect()
    });
    let mut pieces = [Vec::new(), Vec::new()];
    for (is_a, tris) in made {
        pieces[if is_a { 0 } else { 1 }].push(tris);
    }
    pieces
}

/// Re-triangulate a single triangle that has intersection segments.
/// Projects to 2D, runs CDT, and maps back to 3D.
fn retriangulate_triangle(
    v0: Vec3d,
    v1: Vec3d,
    v2: Vec3d,
    seg_indices: &[usize],
    segments: &[IntersectionSegment],
    _shared_verts: &[Vec3d],
    _is_mesh_a: bool,
) -> Vec<[Vec3d; 3]> {
    let normal = (v1 - v0).cross(v2 - v0);
    let normal_len = normal.length();
    if normal_len < 1e-15 {
        return vec![[v0, v1, v2]]; // degenerate triangle, return as-is
    }

    // Choose projection plane: drop the axis most aligned with the normal
    let ax = normal.x.abs();
    let ay = normal.y.abs();
    let az = normal.z.abs();
    let (proj_i, proj_j) = if ax >= ay && ax >= az {
        (1, 2)
    } else if ay >= az {
        (0, 2)
    } else {
        (0, 1)
    };

    let project = |v: Vec3d| -> (f64, f64) {
        let coords = [v.x, v.y, v.z];
        (coords[proj_i], coords[proj_j])
    };

    // Collect all points: triangle vertices + intersection points
    let mut all_points_3d: Vec<Vec3d> = vec![v0, v1, v2];
    let mut constraint_pairs: Vec<(usize, usize)> = Vec::new();

    for &si in seg_indices {
        let seg = &segments[si];
        // Use canonical positions from the shared vertex pool so that
        // both meshes get identical coordinates for the same intersection point.
        let pts = [
            _shared_verts[seg.v0_idx as usize],
            _shared_verts[seg.v1_idx as usize],
        ];
        let mut pt_indices = [0usize; 2];

        for k in 0..2 {
            let p = pts[k];
            // Check if this point is very close to an existing point
            let mut found = None;
            for (i, &existing) in all_points_3d.iter().enumerate() {
                if existing.distance(p) < 1e-10 {
                    found = Some(i);
                    break;
                }
            }
            pt_indices[k] = match found {
                Some(i) => i,
                None => {
                    let i = all_points_3d.len();
                    all_points_3d.push(p);
                    i
                }
            };
        }

        if pt_indices[0] != pt_indices[1] {
            constraint_pairs.push((pt_indices[0], pt_indices[1]));
        }
    }

    // If we only have the original 3 points and no interior points, return the original triangle
    if all_points_3d.len() == 3 && constraint_pairs.is_empty() {
        return vec![[v0, v1, v2]];
    }

    // Project all points to 2D
    let pts_2d: Vec<(f64, f64)> = all_points_3d.iter().map(|&v| project(v)).collect();

    // Find bounds for CDT
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;
    for &(x, y) in &pts_2d {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }

    // Run CDT
    let mut cdt = CDT::new(Point2 { x: min_x, y: min_y }, Point2 { x: max_x, y: max_y });

    // Insert all points
    let mut cdt_indices: Vec<u32> = Vec::new();
    for &(x, y) in &pts_2d {
        cdt_indices.push(cdt.insert_point(x, y));
    }

    // Add constraint edges
    for &(a, b) in &constraint_pairs {
        cdt.add_constraint(cdt_indices[a], cdt_indices[b]);
    }

    // Add triangle boundary edges as constraints.
    // If intersection points lie on a boundary edge, split the constraint
    // at those points. Without this, the CDT marks the full edge as constrained
    // and cannot properly split it, producing degenerate triangles.
    for &(e0, e1) in &[(0usize, 1usize), (1, 2), (2, 0)] {
        add_split_boundary_constraint(&mut cdt, &cdt_indices, &pts_2d, e0, e1);
    }

    cdt.finalize();

    // Get result triangles
    let cdt_tris = cdt.get_triangles();

    // Filter: only keep triangles inside the original triangle
    let (p0_2d, p1_2d, p2_2d) = (pts_2d[0], pts_2d[1], pts_2d[2]);
    let tri_orient = orient_2d_tuple(p0_2d, p1_2d, p2_2d);

    let mut result: Vec<[Vec3d; 3]> = Vec::new();
    for &[a, b, c] in &cdt_tris {
        if a as usize >= all_points_3d.len()
            || b as usize >= all_points_3d.len()
            || c as usize >= all_points_3d.len()
        {
            continue;
        }
        // Check if centroid of this sub-triangle is inside the original triangle
        let pa = pts_2d[a as usize];
        let pb = pts_2d[b as usize];
        let pc = pts_2d[c as usize];

        // Skip degenerate (zero-area) triangles — these arise when 3 collinear
        // intersection points form a triangle. Their centroid lies on the boundary,
        // passing the point_in_triangle test, but they create non-manifold edges.
        let sub_area = orient_2d_tuple(pa, pb, pc).abs();
        if sub_area < 1e-14 {
            continue;
        }

        let centroid = ((pa.0 + pb.0 + pc.0) / 3.0, (pa.1 + pb.1 + pc.1) / 3.0);

        if point_in_triangle_2d(centroid, p0_2d, p1_2d, p2_2d) {
            let va = all_points_3d[a as usize];
            let vb = all_points_3d[b as usize];
            let vc = all_points_3d[c as usize];

            // Ensure winding matches original triangle
            let sub_orient = orient_2d_tuple(pa, pb, pc);
            if (tri_orient > 0.0) == (sub_orient > 0.0) {
                result.push([va, vb, vc]);
            } else {
                result.push([va, vc, vb]);
            }
        }
    }

    // If CDT produced nothing valid, fall back to original triangle
    if result.is_empty() {
        result.push([v0, v1, v2]);
    }

    result
}

/// Add a boundary edge constraint, splitting it at any interior points that lie on it.
/// This prevents the CDT from marking the full edge as a single constraint when
/// intersection points split it.
fn add_split_boundary_constraint(
    cdt: &mut CDT,
    cdt_indices: &[u32],
    pts_2d: &[(f64, f64)],
    e0: usize,
    e1: usize,
) {
    let a = pts_2d[e0];
    let b = pts_2d[e1];
    let ab_len_sq = (b.0 - a.0) * (b.0 - a.0) + (b.1 - a.1) * (b.1 - a.1);
    if ab_len_sq < 1e-30 {
        return;
    }

    // Find all points (beyond the triangle vertices 0,1,2) that lie on edge e0→e1
    let mut on_edge: Vec<(usize, f64)> = Vec::new(); // (point_index, t_parameter)
    for i in 3..pts_2d.len() {
        if i == e0 || i == e1 {
            continue;
        }
        let p = pts_2d[i];
        let ap = (p.0 - a.0, p.1 - a.1);
        let t = (ap.0 * (b.0 - a.0) + ap.1 * (b.1 - a.1)) / ab_len_sq;
        if t <= 1e-10 || t >= 1.0 - 1e-10 {
            continue; // at or beyond endpoints
        }
        let proj = (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1));
        let dist_sq = (p.0 - proj.0) * (p.0 - proj.0) + (p.1 - proj.1) * (p.1 - proj.1);
        if dist_sq < 1e-16 {
            on_edge.push((i, t));
        }
    }

    if on_edge.is_empty() {
        // No splits needed
        cdt.add_constraint(cdt_indices[e0], cdt_indices[e1]);
    } else {
        // Sort by parameter t along the edge
        on_edge.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        // Add chain of constraints: e0→p1→p2→...→e1
        let mut prev = e0;
        for &(pi, _) in &on_edge {
            cdt.add_constraint(cdt_indices[prev], cdt_indices[pi]);
            prev = pi;
        }
        cdt.add_constraint(cdt_indices[prev], cdt_indices[e1]);
    }
}

/// Compute the edges of the intersection polygon of two coplanar triangles.
///
/// Uses Sutherland-Hodgman clipping: clips triangle A against each halfplane
/// defined by the edges of triangle B. Returns the edges of the resulting
/// polygon as 3D line segments. If the triangles don't overlap, returns empty.
fn coplanar_intersection_edges(
    a0: Vec3d,
    a1: Vec3d,
    a2: Vec3d,
    b0: Vec3d,
    b1: Vec3d,
    b2: Vec3d,
) -> Vec<(Vec3d, Vec3d)> {
    // Project to 2D (drop the axis most aligned with the shared normal)
    let normal = (a1 - a0).cross(a2 - a0);
    let ax = normal.x.abs();
    let ay = normal.y.abs();
    let az = normal.z.abs();
    let (pi, pj) = if ax >= ay && ax >= az {
        (1, 2)
    } else if ay >= az {
        (0, 2)
    } else {
        (0, 1)
    };
    let project = |v: Vec3d| -> [f64; 2] {
        let c = [v.x, v.y, v.z];
        [c[pi], c[pj]]
    };

    // Construct the homogeneous crossing before rounding, as for the
    // non-coplanar edge/plane intersections. A rounded interpolation
    // parameter can put a clipping vertex across a later constraint.
    let intersect = |a: Vec3d, b: Vec3d, e0: [f64; 2], e1: [f64; 2]| {
        use makepad_csg_exact::{ExactVec3, Expansion};
        let cross = |p: Vec3d| {
            let p = project(p);
            let dx = Expansion::from_f64(e1[0]).sub_f64(e0[0]);
            let dy = Expansion::from_f64(e1[1]).sub_f64(e0[1]);
            dx.mul(&Expansion::from_f64(p[1]).sub_f64(e0[1]))
                .sub(&dy.mul(&Expansion::from_f64(p[0]).sub_f64(e0[0])))
        };
        let da = cross(a);
        let db = cross(b);
        if da.is_zero() { return a; }
        if db.is_zero() { return b; }
        let a = ExactVec3::from_vec3d(a);
        let b = ExactVec3::from_vec3d(b);
        let num = b.scale(&da).sub(&a.scale(&db));
        let den = da.sub(&db).estimate();
        Vec3d::new(num.x.estimate()/den, num.y.estimate()/den, num.z.estimate()/den)
    };

    // Sutherland-Hodgman: start with triangle A's vertices as polygon,
    // clip against each edge of B.
    let mut polygon: Vec<Vec3d> = vec![a0, a1, a2];
    let clip_edges = [(b0, b1), (b1, b2), (b2, b0)];

    // Determine winding of B in 2D to know which side is "inside"
    let bp0 = project(b0);
    let bp1 = project(b1);
    let bp2 = project(b2);
    let b_orient = orient2d(bp0[0], bp0[1], bp1[0], bp1[1], bp2[0], bp2[1]);
    // sign > 0 means CCW; for each clip edge, "inside" is the left side

    // A half-plane that leaves just one vertex on its boundary and the
    // other two strictly outside can intersect this triangle only at that
    // vertex. There is no constraint segment to construct. This avoids
    // repeatedly clipping point contacts between coplanar triangle fans.
    let original = [project(a0), project(a1), project(a2)];
    for &(ce0, ce1) in &clip_edges {
        let e0 = project(ce0);
        let e1 = project(ce1);
        let sides = original.map(|p| {
            if p == e0 || p == e1 {
                0.0
            } else {
                orient2d(e0[0], e0[1], e1[0], e1[1], p[0], p[1])
                    * b_orient.signum()
            }
        });
        if sides.iter().filter(|&&d| d < 0.0).count() == 2
            && sides.iter().filter(|&&d| d == 0.0).count() == 1
        {
            return Vec::new();
        }
    }

    for &(ce0, ce1) in &clip_edges {
        if polygon.is_empty() {
            break;
        }
        let e0 = project(ce0);
        let e1 = project(ce1);
        let mut output = Vec::new();

        for i in 0..polygon.len() {
            let curr = polygon[i];
            let prev = polygon[(i + polygon.len() - 1) % polygon.len()];
            let curr_p = project(curr);
            let prev_p = project(prev);

            // Signed distance from the clip edge (positive = inside for CCW B)
            let d_curr =
                orient2d(e0[0], e0[1], e1[0], e1[1], curr_p[0], curr_p[1]);
            let d_prev =
                orient2d(e0[0], e0[1], e1[0], e1[1], prev_p[0], prev_p[1]);

            // Adjust sign based on B's winding
            let d_curr = d_curr * b_orient.signum();
            let d_prev = d_prev * b_orient.signum();

            if d_curr >= 0.0 {
                // Current is inside
                if d_prev < 0.0 {
                    // Previous was outside: add intersection
                    output.push(intersect(prev, curr, e0, e1));
                }
                output.push(curr);
            } else if d_prev >= 0.0 {
                // Current is outside, previous was inside: add intersection
                output.push(intersect(prev, curr, e0, e1));
            }
        }
        polygon = output;
    }

    if polygon.len() < 3 {
        return Vec::new(); // Degenerate or no intersection
    }

    // Return edges of the intersection polygon
    let mut edges = Vec::with_capacity(polygon.len());
    for i in 0..polygon.len() {
        let j = (i + 1) % polygon.len();
        edges.push((polygon[i], polygon[j]));
    }
    edges
}

fn orient_2d_tuple(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
    orient2d(a.0, a.1, b.0, b.1, c.0, c.1)
}

fn point_in_triangle_2d(p: (f64, f64), a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> bool {
    let d1 = orient_2d_tuple(a, b, p);
    let d2 = orient_2d_tuple(b, c, p);
    let d3 = orient_2d_tuple(c, a, p);

    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_csg_math::{dvec3, Mat4d};
    use makepad_csg_mesh::mesh::make_unit_cube;

    #[test]
    fn test_corefine_non_overlapping() {
        let a = make_unit_cube();
        let mut b = make_unit_cube();
        b.transform(Mat4d::translation(dvec3(5.0, 0.0, 0.0)));

        let result = corefine(&a, &b);
        // Non-overlapping: meshes should be unchanged
        assert_eq!(result.mesh_a.triangle_count(), 12);
        assert_eq!(result.mesh_b.triangle_count(), 12);
    }

    #[test]
    fn test_corefine_overlapping_cubes() {
        let a = make_unit_cube();
        let mut b = make_unit_cube();
        b.transform(Mat4d::translation(dvec3(0.5, 0.0, 0.0)));

        let result = corefine(&a, &b);
        // Overlapping: meshes should have more triangles
        assert!(
            result.mesh_a.triangle_count() >= 12,
            "mesh A should have >= 12 triangles, got {}",
            result.mesh_a.triangle_count()
        );
        assert!(
            result.mesh_b.triangle_count() >= 12,
            "mesh B should have >= 12 triangles, got {}",
            result.mesh_b.triangle_count()
        );

        // Origin tracking should be valid
        assert_eq!(result.origin_a.len(), result.mesh_a.triangle_count());
        assert_eq!(result.origin_b.len(), result.mesh_b.triangle_count());
    }

    #[test]
    fn test_corefine_preserves_volume() {
        let a = make_unit_cube();
        let mut b = make_unit_cube();
        b.transform(Mat4d::translation(dvec3(0.5, 0.0, 0.0)));

        let vol_a_before = makepad_csg_mesh::volume::mesh_volume(&a);
        let vol_b_before = makepad_csg_mesh::volume::mesh_volume(&b);

        let result = corefine(&a, &b);

        let vol_a_after = makepad_csg_mesh::volume::mesh_volume(&result.mesh_a);
        let vol_b_after = makepad_csg_mesh::volume::mesh_volume(&result.mesh_b);

        // Corefinement should preserve volume (within tolerance)
        assert!(
            (vol_a_before - vol_a_after).abs() < 0.1,
            "mesh A volume changed: {} -> {}",
            vol_a_before,
            vol_a_after
        );
        assert!(
            (vol_b_before - vol_b_after).abs() < 0.1,
            "mesh B volume changed: {} -> {}",
            vol_b_before,
            vol_b_after
        );
    }
}

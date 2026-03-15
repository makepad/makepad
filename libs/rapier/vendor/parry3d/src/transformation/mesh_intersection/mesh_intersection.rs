use super::{MeshIntersectionError, TriangleTriangleIntersection};
use crate::bounding_volume::BoundingVolume;
use crate::math::{Pose, Real, Vector3};
use crate::partitioning::BvhNode;
use crate::query::point::point_query::PointQueryWithLocation;
use crate::query::PointQuery;
use crate::shape::{TriMesh, Triangle};
use crate::utils;
use crate::utils::hashmap::Entry;
use crate::utils::hashmap::HashMap;
use crate::utils::hashset::HashSet;
use alloc::collections::BTreeMap;
use alloc::{vec, vec::Vec};
use rstar::RTree;
use spade::{ConstrainedDelaunayTriangulation, InsertionError, Triangulation as _};
#[cfg(feature = "wavefront")]
use std::path::PathBuf;

/// A triangle with indices sorted in increasing order for deduplication in a hashmap.
///
/// Note that when converting a `[u32; 3]` into a `HashableTriangleIndices`, the result’s orientation
/// might not match the input’s.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
struct HashableTriangleIndices([u32; 3]);

impl From<[u32; 3]> for HashableTriangleIndices {
    fn from([a, b, c]: [u32; 3]) -> Self {
        let (sa, sb, sc) = utils::sort3(&a, &b, &c);
        HashableTriangleIndices([*sa, *sb, *sc])
    }
}

/// Metadata that specifies thresholds to use when making construction choices
/// in mesh intersections.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct MeshIntersectionTolerances {
    /// The smallest angle (in radians) that will be tolerated. A triangle with
    /// a smaller angle is considered degenerate and will be deleted.
    pub angle_epsilon: Real,
    /// The maximum distance at which two points are considered to overlap in space.
    /// If `||p1 - p2|| < global_insertion_epsilon` then p1 and p2 are considered
    /// to be the same point.
    pub global_insertion_epsilon: Real,
    /// A multiplier coefficient to scale [`Self::global_insertion_epsilon`] when checking for
    /// point duplication within a single triangle.
    ///
    /// Inside an individual triangle the distance at which two points are considered
    /// to be the same is `global_insertion_epsilon * local_insertion_epsilon_mod`.
    pub local_insertion_epsilon_scale: Real,
    /// Three points forming a triangle with an area smaller than this epsilon are considered collinear.
    pub collinearity_epsilon: Real,
}

impl Default for MeshIntersectionTolerances {
    fn default() -> Self {
        Self {
            #[expect(clippy::unnecessary_cast)]
            angle_epsilon: (0.005 as Real).to_radians(), // 0.005 degrees
            global_insertion_epsilon: Real::EPSILON * 100.0,
            local_insertion_epsilon_scale: 10.,
            collinearity_epsilon: Real::EPSILON * 100.0,
        }
    }
}

/// Computes the intersection of two meshes.
///
/// The meshes must be oriented, have their half-edge topology computed, and must not be self-intersecting.
/// The result mesh vertex coordinates are given in the local-space of `mesh1`.
pub fn intersect_meshes(
    pos1: &Pose,
    mesh1: &TriMesh,
    flip1: bool,
    pos2: &Pose,
    mesh2: &TriMesh,
    flip2: bool,
) -> Result<Option<TriMesh>, MeshIntersectionError> {
    intersect_meshes_with_tolerances(
        pos1,
        mesh1,
        flip1,
        pos2,
        mesh2,
        flip2,
        MeshIntersectionTolerances::default(),
    )
}

/// Similar to `intersect_meshes`.
///
/// It allows to specify epsilons for how the algorithm will behave.
/// See `MeshIntersectionTolerances` for details.
pub fn intersect_meshes_with_tolerances(
    pos1: &Pose,
    mesh1: &TriMesh,
    flip1: bool,
    pos2: &Pose,
    mesh2: &TriMesh,
    flip2: bool,
    tolerances: MeshIntersectionTolerances,
) -> Result<Option<TriMesh>, MeshIntersectionError> {
    if cfg!(debug_assertions) {
        mesh1.assert_half_edge_topology_is_valid();
        mesh2.assert_half_edge_topology_is_valid();
    }

    if mesh1.topology().is_none() || mesh2.topology().is_none() {
        return Err(MeshIntersectionError::MissingTopology);
    }

    if mesh1.pseudo_normals_if_oriented().is_none() || mesh2.pseudo_normals_if_oriented().is_none()
    {
        return Err(MeshIntersectionError::MissingPseudoNormals);
    }

    let pos12 = pos1.inv_mul(pos2);

    // 1: collect all the potential triangle-triangle intersections.
    let intersection_candidates: Vec<_> = mesh1
        .bvh()
        .leaf_pairs(mesh2.bvh(), |node1: &BvhNode, node2: &BvhNode| {
            let aabb1 = node1.aabb();
            let aabb2 = node2.aabb().transform_by(&pos12);
            aabb1.intersects(&aabb2)
        })
        .collect();

    let mut deleted_faces1: HashSet<u32> = HashSet::default();
    let mut deleted_faces2: HashSet<u32> = HashSet::default();
    let mut new_indices1 = vec![];
    let mut new_indices2 = vec![];

    // 2: Identify all triangles that do actually intersect.
    let mut intersections = vec![];
    for (fid1, fid2) in &intersection_candidates {
        let tri1 = mesh1.triangle(*fid1);
        let tri2 = mesh2.triangle(*fid2).transformed(&pos12);

        if super::triangle_triangle_intersection(&tri1, &tri2, tolerances.collinearity_epsilon)
            .is_some()
        {
            intersections.push((*fid1, *fid2));
            let _ = deleted_faces1.insert(*fid1);
            let _ = deleted_faces2.insert(*fid2);
        }
    }

    // 3: Grab all triangles that are inside the other mesh but do not intersect it.
    extract_connected_components(
        &pos12,
        mesh1,
        mesh2,
        flip2,
        &deleted_faces1,
        &mut new_indices1,
    );
    extract_connected_components(
        &pos12.inverse(),
        mesh2,
        mesh1,
        flip1,
        &deleted_faces2,
        &mut new_indices2,
    );

    // 4: Initialize a new mesh by inserting points into a set. Duplicate points should
    // hash to the same index.
    let mut point_set = RTree::<TreeVector, _>::new();
    let mut topology_indices = HashMap::default();
    {
        let mut insert_point = |position: Vector3| {
            insert_into_set(
                position,
                &mut point_set,
                tolerances.global_insertion_epsilon,
            ) as u32
        };
        // Add the inside vertices and triangles from mesh1
        for mut face in new_indices1 {
            if flip1 {
                face.swap(0, 1);
            }

            let idx = [
                insert_point(pos1 * mesh1.vertices()[face[0] as usize]),
                insert_point(pos1 * mesh1.vertices()[face[1] as usize]),
                insert_point(pos1 * mesh1.vertices()[face[2] as usize]),
            ];

            if !is_topologically_degenerate(idx) {
                insert_topology_indices(&mut topology_indices, idx);
            }
        }

        // Add the inside vertices and triangles from mesh2
        for mut face in new_indices2 {
            if flip2 {
                face.swap(0, 1);
            }
            let idx = [
                insert_point(pos2 * mesh2.vertices()[face[0] as usize]),
                insert_point(pos2 * mesh2.vertices()[face[1] as usize]),
                insert_point(pos2 * mesh2.vertices()[face[2] as usize]),
            ];

            if !is_topologically_degenerate(idx) {
                insert_topology_indices(&mut topology_indices, idx);
            }
        }
    }

    // 5: Associate constraint edges generated by a triangle-triangle intersection
    // to each intersecting triangle where they occur.
    let mut constraints1 = BTreeMap::<_, Vec<_>>::new();
    let mut constraints2 = BTreeMap::<_, Vec<_>>::new();
    for (fid1, fid2) in &intersections {
        let tri1 = mesh1.triangle(*fid1).transformed(pos1);
        let tri2 = mesh2.triangle(*fid2).transformed(pos2);

        let list1 = constraints1.entry(fid1).or_default();
        let list2 = constraints2.entry(fid2).or_default();

        let intersection =
            super::triangle_triangle_intersection(&tri1, &tri2, tolerances.collinearity_epsilon);
        if let Some(intersection) = intersection {
            match intersection {
                TriangleTriangleIntersection::Segment { a, b } => {
                    // For both triangles, add the points in the intersection
                    // and their associated edge to the set.
                    // Note this necessarily introduces duplicate points to the
                    // set that need to be filtered out.
                    list1.push([a.p1, b.p1]);
                    list2.push([a.p1, b.p1]);
                }
                TriangleTriangleIntersection::Polygon(polygon) => {
                    for i in 0..polygon.len() {
                        let a = polygon[i];
                        let b = polygon[(i + 1) % polygon.len()];

                        // Triangles overlap in space, so only one constraint is needed.
                        list1.push([a.p1, b.p1]);
                    }
                }
            }
        }
    }

    // 6: Collect all triangles that intersect and their associated constraint edges.
    // For each such triangle, compute a CDT of its constraints. For each face in this CDT,
    // if the face is contained in the opposite mesh, add it to the intersection mesh.
    merge_triangle_sets(
        mesh1,
        mesh2,
        &constraints1,
        pos1,
        pos2,
        flip1,
        flip2,
        &tolerances,
        &mut point_set,
        &mut topology_indices,
    )?;

    merge_triangle_sets(
        mesh2,
        mesh1,
        &constraints2,
        pos2,
        pos1,
        flip2,
        flip1,
        &tolerances,
        &mut point_set,
        &mut topology_indices,
    )?;

    // 7: Sort the output points by insertion order.
    let mut vertices: Vec<_> = point_set.iter().copied().collect();
    vertices.sort_by(|a, b| a.id.cmp(&b.id));
    let vertices: Vec<_> = vertices.iter().map(|p| Vector3::from(p.point)).collect();

    if !topology_indices.is_empty() {
        Ok(Some(TriMesh::new(
            vertices,
            topology_indices.into_values().collect(),
        )?))
    } else {
        Ok(None)
    }
}

fn extract_connected_components(
    pos12: &Pose,
    mesh1: &TriMesh,
    mesh2: &TriMesh,
    flip2: bool,
    deleted_faces1: &HashSet<u32>,
    new_indices1: &mut Vec<[u32; 3]>,
) {
    let topo1 = mesh1.topology().unwrap();
    let mut visited: HashSet<u32> = HashSet::default();
    let mut to_visit = vec![];
    let mut visited_conn_comp = if let Some(cc) = mesh1.connected_components() {
        vec![false; cc.ranges.len()] // TODO: use a Vob instead?
    } else {
        vec![]
    };

    for face in deleted_faces1 {
        if let Some(cc) = mesh1.connected_components() {
            visited_conn_comp[cc.face_colors[*face as usize] as usize] = true;
        }

        let eid = topo1.faces[*face as usize].half_edge;
        let edge_a = &topo1.half_edges[eid as usize];
        let edge_b = &topo1.half_edges[edge_a.next as usize];
        let edge_c = &topo1.half_edges[edge_b.next as usize];
        let edges = [edge_a, edge_b, edge_c];

        for edge in edges {
            if let Some(twin) = topo1.half_edges.get(edge.twin as usize) {
                if !deleted_faces1.contains(&twin.face) {
                    let tri1 = mesh1.triangle(twin.face);

                    if flip2
                        ^ mesh2.contains_local_point(pos12.inverse_transform_point(tri1.center()))
                    {
                        to_visit.push(twin.face);
                    }
                }
            }
        }
    }

    // Propagate.
    while let Some(face) = to_visit.pop() {
        if !visited.insert(face) {
            continue; // Already visited.
        }

        new_indices1.push(mesh1.indices()[face as usize]);

        let eid = topo1.faces[face as usize].half_edge;
        let edge_a = &topo1.half_edges[eid as usize];
        let edge_b = &topo1.half_edges[edge_a.next as usize];
        let edge_c = &topo1.half_edges[edge_b.next as usize];
        let edges = [edge_a, edge_b, edge_c];

        for edge in edges {
            if let Some(twin) = topo1.half_edges.get(edge.twin as usize) {
                if !deleted_faces1.contains(&twin.face) {
                    to_visit.push(twin.face);
                }
            }
        }
    }

    /*
     * Deal with connected components that don’t intersect the other mesh.
     */
    if let Some(cc) = mesh1.connected_components() {
        for (i, range) in cc.ranges.windows(2).enumerate() {
            if !visited_conn_comp[i] {
                // This connected component doesn’t intersect the second mesh.
                // Classify one of its face (the "representative face", can be any
                // face of the connected component) to determine
                // if the whole thing is inside or outside.
                let repr_face = cc.grouped_faces[range[0]];
                let repr_pt = mesh1.triangle(repr_face).center();
                let indices = mesh1.indices();

                if flip2 ^ mesh2.contains_local_point(pos12.inverse_transform_point(repr_pt)) {
                    new_indices1.extend(
                        cc.grouped_faces[range[0]..range[1]]
                            .iter()
                            .map(|fid| indices[*fid as usize]),
                    )
                }
            }
        }
    } else if deleted_faces1.is_empty() {
        // Deal with the case where there is no intersection between the meshes.
        let repr_pt = mesh1.triangle(0).center();

        if flip2 ^ mesh2.contains_local_point(pos12.inverse_transform_point(repr_pt)) {
            new_indices1.extend_from_slice(mesh1.indices());
        }
    }
}

fn triangulate_constraints_and_merge_duplicates(
    tri: &Triangle,
    constraints: &[[Vector3; 2]],
    epsilon: Real,
) -> Result<
    (
        ConstrainedDelaunayTriangulation<spade::Point2<Real>>,
        Vec<Vector3>,
    ),
    InsertionError,
> {
    let mut constraints = constraints.to_vec();
    // Add the triangle points to the triangulation.
    let mut point_set = RTree::<TreeVector, _>::new();
    let _ = insert_into_set(tri.a, &mut point_set, epsilon);
    let _ = insert_into_set(tri.b, &mut point_set, epsilon);
    let _ = insert_into_set(tri.c, &mut point_set, epsilon);

    // Sometimes, points on the edge of a triangle are slightly off, and this makes
    // spade think that there is a super thin triangle. Project points close to an edge
    // onto the edge to get better results.
    let tri_vtx = tri.vertices();
    for point_pair in constraints.iter_mut() {
        let p1 = point_pair[0];
        let p2 = point_pair[1];

        for i in 0..3 {
            let q1 = tri_vtx[i];
            let q2 = tri_vtx[(i + 1) % 3];

            let proj1 = project_point_to_segment(p1, &[q1, q2]);
            if (p1 - proj1).length() < epsilon {
                point_pair[0] = Vector3::from(proj1);
            }

            let proj2 = project_point_to_segment(p2, &[q1, q2]);
            if (p2 - proj2).length() < epsilon {
                point_pair[1] = Vector3::from(proj2);
            }
        }
    }

    // Generate edge, taking care to merge duplicate vertices.
    let mut edges = Vec::new();
    for point_pair in constraints {
        let p1_id = insert_into_set(point_pair[0], &mut point_set, epsilon);
        let p2_id = insert_into_set(point_pair[1], &mut point_set, epsilon);

        if p1_id != p2_id {
            edges.push([p1_id, p2_id]);
        }
    }

    let mut points: Vec<_> = point_set.iter().cloned().collect();
    points.sort_by(|a, b| a.id.cmp(&b.id));

    let tri_points = tri.vertices();
    let best_source = tri.angle_closest_to_90();
    let d1 = tri_points[(best_source + 2) % 3] - tri_points[(best_source + 1) % 3];
    let d2 = tri_points[best_source] - tri_points[(best_source + 1) % 3];
    let (e1, e2) = planar_gram_schmidt(d1, d2);

    let project = |p: Vector3| spade::Point2::new(e1.dot(p), e2.dot(p));

    // Project points into 2D and triangulate the resulting set.
    let planar_points: Vec<_> = points
        .iter()
        .copied()
        .map(|point| {
            let point_proj = project(point.point);
            utils::sanitize_spade_point(point_proj)
        })
        .collect();
    let cdt_triangulation = ConstrainedDelaunayTriangulation::bulk_load_cdt(planar_points, edges)?;
    debug_assert!(cdt_triangulation.vertices().len() == points.len());

    let points = points.into_iter().map(|p| Vector3::from(p.point)).collect();
    Ok((cdt_triangulation, points))
}

// We heavily recommend that this is left here in case one needs to debug the above code.
#[cfg(feature = "wavefront")]
fn _points_to_obj(mesh: &[Vector3], path: &PathBuf) {
    use std::io::Write;
    let mut file = std::fs::File::create(path).unwrap();

    for p in mesh {
        writeln!(file, "v {} {} {}", p.x, p.y, p.z).unwrap();
    }
}

// We heavily recommend that this is left here in case one needs to debug the above code.
#[cfg(feature = "wavefront")]
fn _points_and_edges_to_obj(mesh: &[Vector3], edges: &[[usize; 2]], path: &PathBuf) {
    use std::io::Write;
    let mut file = std::fs::File::create(path).unwrap();

    for p in mesh {
        writeln!(file, "v {} {} {}", p.x, p.y, p.z).unwrap();
    }

    for e in edges {
        writeln!(file, "l {} {}", e[0] + 1, e[1] + 1).unwrap();
    }
}

#[derive(Copy, Clone, PartialEq, Debug, Default)]
struct TreeVector {
    point: Vector3,
    id: usize,
}

impl rstar::Point for TreeVector {
    type Scalar = Real;
    const DIMENSIONS: usize = 3;

    fn generate(mut generator: impl FnMut(usize) -> Self::Scalar) -> Self {
        TreeVector {
            point: Vector3::new(generator(0), generator(1), generator(2)),
            id: usize::MAX,
        }
    }

    fn nth(&self, index: usize) -> Self::Scalar {
        self.point[index]
    }

    fn nth_mut(&mut self, index: usize) -> &mut Self::Scalar {
        &mut self.point[index]
    }
}

fn insert_into_set(position: Vector3, point_set: &mut RTree<TreeVector>, epsilon: Real) -> usize {
    let point_count = point_set.size();
    let point_to_insert = TreeVector {
        point: position,
        id: point_count,
    };

    match point_set.nearest_neighbor(&point_to_insert) {
        Some(tree_point) => {
            if (tree_point.point - position).length_squared() <= epsilon {
                tree_point.id
            } else {
                point_set.insert(point_to_insert);
                debug_assert!(point_set.size() == point_count + 1);
                point_count
            }
        }
        None => {
            point_set.insert(point_to_insert);
            debug_assert!(point_set.size() == point_count + 1);
            point_count
        }
    }
}

fn smallest_angle(points: &[Vector3]) -> Real {
    let n = points.len();

    let mut worst_cos: Real = -2.0;
    for i in 0..points.len() {
        let d1 = (points[i] - points[(i + 1) % n]).normalize();
        let d2 = (points[(i + 2) % n] - points[(i + 1) % n]).normalize();

        let cos = d1.dot(d2);
        if cos > worst_cos {
            worst_cos = cos;
        }
    }

    worst_cos.acos()
}

fn planar_gram_schmidt(v1: Vector3, v2: Vector3) -> (Vector3, Vector3) {
    let u1 = v1;
    let u2 = v2 - (v2.dot(u1) / u1.length_squared()) * u1;

    let e1 = u1.normalize();
    let e2 = u2.normalize();

    (e1, e2)
}

fn project_point_to_segment(point: Vector3, segment: &[Vector3; 2]) -> Vector3 {
    let dir = segment[1] - segment[0];
    let local = point - segment[0];

    let norm = dir.length();
    // Restrict the result to the segment portion of the line.
    let coeff = (dir.dot(local) / norm).clamp(0., norm);

    segment[0] + coeff * dir.normalize()
}

/// No matter how smart we are about computing intersections. It is always possible
/// to create ultra thin triangles when a point lies on an edge of a triangle. These
/// are degenerate and need to be removed.
fn is_triangle_degenerate(
    triangle: &[Vector3; 3],
    epsilon_angle: Real,
    epsilon_distance: Real,
) -> bool {
    if smallest_angle(triangle) < epsilon_angle {
        return true;
    }

    for i in 0..3 {
        let (dir, dist) = (triangle[(i + 1) % 3] - triangle[(i + 2) % 3]).normalize_and_length();
        if dist < epsilon_distance {
            return true;
        }

        let proj = triangle[(i + 2) % 3] + (triangle[i] - triangle[(i + 2) % 3]).dot(dir) * dir;

        if (proj - triangle[i]).length() < epsilon_distance {
            return true;
        }
    }

    false
}

fn merge_triangle_sets(
    mesh1: &TriMesh,
    mesh2: &TriMesh,
    triangle_constraints: &BTreeMap<&u32, Vec<[Vector3; 2]>>,
    pos1: &Pose,
    pos2: &Pose,
    flip1: bool,
    flip2: bool,
    metadata: &MeshIntersectionTolerances,
    point_set: &mut RTree<TreeVector>,
    topology_indices: &mut HashMap<HashableTriangleIndices, [u32; 3]>,
) -> Result<(), MeshIntersectionError> {
    // For each triangle, and each constraint edge associated to that triangle,
    // make a triangulation of the face and sort whether each generated
    // sub-triangle is part of the intersection.
    // For each sub-triangle that is part of the intersection, add them to the
    // output mesh.
    for (triangle_id, constraints) in triangle_constraints.iter() {
        let tri = mesh1.triangle(**triangle_id).transformed(pos1);

        let (delaunay, points) = triangulate_constraints_and_merge_duplicates(
            &tri,
            constraints,
            metadata.global_insertion_epsilon * metadata.local_insertion_epsilon_scale,
        )
        .or(Err(MeshIntersectionError::TriangulationError))?;

        for face in delaunay.inner_faces() {
            let verts = face.vertices();
            let p1 = points[verts[0].index()];
            let p2 = points[verts[1].index()];
            let p3 = points[verts[2].index()];

            // Sometimes the triangulation is messed up due to numerical errors. If
            // a triangle does not survive this test it should be deleted.
            if is_triangle_degenerate(
                &[p1, p2, p3],
                metadata.angle_epsilon,
                metadata.global_insertion_epsilon,
            ) {
                continue;
            }

            let center = Triangle {
                a: p1,
                b: p2,
                c: p3,
            }
            .center();

            let epsilon = metadata.global_insertion_epsilon;
            let projection = mesh2
                .project_local_point_and_get_location(pos2.inverse_transform_point(center), true)
                .0;

            if flip2 ^ (projection.is_inside_eps(center, epsilon)) {
                let mut new_tri_idx = [
                    insert_into_set(p1, point_set, epsilon) as u32,
                    insert_into_set(p2, point_set, epsilon) as u32,
                    insert_into_set(p3, point_set, epsilon) as u32,
                ];

                if flip1 {
                    new_tri_idx.swap(0, 1)
                }

                // This should *never* trigger. If it does
                // it means the code has created a triangle with duplicate vertices,
                // which means we encountered an unaccounted for edge case.
                if is_topologically_degenerate(new_tri_idx) {
                    return Err(MeshIntersectionError::DuplicateVertices);
                }

                insert_topology_indices(topology_indices, new_tri_idx);
            }
        }
    }

    Ok(())
}

// Insert in the hashmap with sorted indices to avoid adding duplicates.
//
// We also check if we don’t keep pairs of triangles that have the same
// set of indices but opposite orientations. If this happens, both the new triangle, and the one it
// matched with are removed (because they describe a degenerate piece of volume).
fn insert_topology_indices(
    topology_indices: &mut HashMap<HashableTriangleIndices, [u32; 3]>,
    new_tri_idx: [u32; 3],
) {
    match topology_indices.entry(new_tri_idx.into()) {
        Entry::Vacant(e) => {
            let _ = e.insert(new_tri_idx);
        }
        Entry::Occupied(e) => {
            fn same_orientation(a: &[u32; 3], b: &[u32; 3]) -> bool {
                let ib = if a[0] == b[0] {
                    0
                } else if a[0] == b[1] {
                    1
                } else {
                    2
                };
                a[1] == b[(ib + 1) % 3]
            }

            if !same_orientation(e.get(), &new_tri_idx) {
                // If we are inserting two identical triangles but with mismatching
                // orientations, we can just ignore both because they cover a degenerate
                // 2D plane.
                #[cfg(feature = "enhanced-determinism")]
                let _ = e.swap_remove();
                #[cfg(not(feature = "enhanced-determinism"))]
                let _ = e.remove();
            }
        }
    }
}

fn is_topologically_degenerate(tri_idx: [u32; 3]) -> bool {
    tri_idx[0] == tri_idx[1] || tri_idx[0] == tri_idx[2] || tri_idx[1] == tri_idx[2]
}

#[cfg(feature = "wavefront")]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vector;
    use crate::shape::{Ball, Cuboid, TriMeshFlags};
    use obj::Obj;
    use obj::ObjData;

    #[test]
    fn test_same_mesh_intersection() {
        let Obj {
            data: ObjData {
                position, objects, ..
            },
            ..
        } = Obj::load("../../assets/tests/low_poly_bunny.obj").unwrap();

        let mesh = TriMesh::with_flags(
            position
                .iter()
                .map(|v| Vector::new(v[0] as Real, v[1] as Real, v[2] as Real))
                .collect::<Vec<_>>(),
            objects[0].groups[0]
                .polys
                .iter()
                .map(|p| [p.0[0].0 as u32, p.0[1].0 as u32, p.0[2].0 as u32])
                .collect::<Vec<_>>(),
            TriMeshFlags::all(),
        )
        .unwrap();

        let _ = intersect_meshes(
            &Pose::identity(),
            &mesh,
            false,
            &Pose::identity(),
            &mesh,
            false,
        )
        .unwrap()
        .unwrap();

        let _ = mesh.to_obj_file(&PathBuf::from("same_test.obj"));
    }

    #[test]
    fn test_non_origin_pos1_pos2_intersection() {
        let ball = Ball::new(2f32 as Real).to_trimesh(10, 10);
        let cuboid = Cuboid::new(Vector::new(2.0, 1.0, 1.0)).to_trimesh();
        let mut sphere_mesh = TriMesh::new(ball.0, ball.1).unwrap();
        sphere_mesh.set_flags(TriMeshFlags::all()).unwrap();
        let mut cuboid_mesh = TriMesh::new(cuboid.0, cuboid.1).unwrap();
        cuboid_mesh.set_flags(TriMeshFlags::all()).unwrap();

        let res = intersect_meshes(
            &Pose::translation(1.0, 0.0, 0.0),
            &cuboid_mesh,
            false,
            &Pose::translation(2.0, 0.0, 0.0),
            &sphere_mesh,
            false,
        )
        .unwrap()
        .unwrap();

        let _ = res.to_obj_file(&PathBuf::from("test_non_origin_pos1_pos2_intersection.obj"));

        let bounding_sphere = res.local_bounding_sphere();
        assert!(bounding_sphere.center == Vector::new(1.5, 0.0, 0.0));
        assert_relative_eq!(2.0615528, bounding_sphere.radius, epsilon = 1.0e-5);
    }

    #[test]
    fn test_offset_cylinder_intersection() {
        let Obj {
            data: ObjData {
                position, objects, ..
            },
            ..
        } = Obj::load("../../assets/tests/offset_cylinder.obj").unwrap();

        let offset_mesh = TriMesh::with_flags(
            position
                .iter()
                .map(|v| Vector::new(v[0] as Real, v[1] as Real, v[2] as Real))
                .collect::<Vec<_>>(),
            objects[0].groups[0]
                .polys
                .iter()
                .map(|p| [p.0[0].0 as u32, p.0[1].0 as u32, p.0[2].0 as u32])
                .collect::<Vec<_>>(),
            TriMeshFlags::all(),
        )
        .unwrap();

        let Obj {
            data: ObjData {
                position, objects, ..
            },
            ..
        } = Obj::load("../../assets/tests/center_cylinder.obj").unwrap();

        let center_mesh = TriMesh::with_flags(
            position
                .iter()
                .map(|v| Vector::new(v[0] as Real, v[1] as Real, v[2] as Real))
                .collect::<Vec<_>>(),
            objects[0].groups[0]
                .polys
                .iter()
                .map(|p| [p.0[0].0 as u32, p.0[1].0 as u32, p.0[2].0 as u32])
                .collect::<Vec<_>>(),
            TriMeshFlags::all(),
        )
        .unwrap();

        let res = intersect_meshes(
            &Pose::identity(),
            &center_mesh,
            false,
            &Pose::identity(),
            &offset_mesh,
            false,
        )
        .unwrap()
        .unwrap();

        let _ = res.to_obj_file(&PathBuf::from("offset_test.obj"));
    }

    #[test]
    fn test_stair_bar_intersection() {
        let Obj {
            data: ObjData {
                position, objects, ..
            },
            ..
        } = Obj::load("../../assets/tests/stairs.obj").unwrap();

        let stair_mesh = TriMesh::with_flags(
            position
                .iter()
                .map(|v| Vector::new(v[0] as Real, v[1] as Real, v[2] as Real))
                .collect::<Vec<_>>(),
            objects[0].groups[0]
                .polys
                .iter()
                .map(|p| [p.0[0].0 as u32, p.0[1].0 as u32, p.0[2].0 as u32])
                .collect::<Vec<_>>(),
            TriMeshFlags::all(),
        )
        .unwrap();

        let Obj {
            data: ObjData {
                position, objects, ..
            },
            ..
        } = Obj::load("../../assets/tests/bar.obj").unwrap();

        let bar_mesh = TriMesh::with_flags(
            position
                .iter()
                .map(|v| Vector::new(v[0] as Real, v[1] as Real, v[2] as Real))
                .collect::<Vec<_>>(),
            objects[0].groups[0]
                .polys
                .iter()
                .map(|p| [p.0[0].0 as u32, p.0[1].0 as u32, p.0[2].0 as u32])
                .collect::<Vec<_>>(),
            TriMeshFlags::all(),
        )
        .unwrap();

        let res = intersect_meshes(
            &Pose::identity(),
            &stair_mesh,
            false,
            &Pose::identity(),
            &bar_mesh,
            false,
        )
        .unwrap()
        .unwrap();

        let _ = res.to_obj_file(&PathBuf::from("stair_test.obj"));
    }

    #[test]
    fn test_complex_intersection() {
        let Obj {
            data: ObjData {
                position, objects, ..
            },
            ..
        } = Obj::load("../../assets/tests/low_poly_bunny.obj").unwrap();

        let bunny_mesh = TriMesh::with_flags(
            position
                .iter()
                .map(|v| Vector::new(v[0] as Real, v[1] as Real, v[2] as Real))
                .collect::<Vec<_>>(),
            objects[0].groups[0]
                .polys
                .iter()
                .map(|p| [p.0[0].0 as u32, p.0[1].0 as u32, p.0[2].0 as u32])
                .collect::<Vec<_>>(),
            TriMeshFlags::all(),
        )
        .unwrap();

        let Obj {
            data: ObjData {
                position, objects, ..
            },
            ..
        } = Obj::load("../../assets/tests/poly_cylinder.obj").unwrap();

        let cylinder_mesh = TriMesh::with_flags(
            position
                .iter()
                .map(|v| Vector::new(v[0] as Real, v[1] as Real, v[2] as Real))
                .collect::<Vec<_>>(),
            objects[0].groups[0]
                .polys
                .iter()
                .map(|p| [p.0[0].0 as u32, p.0[1].0 as u32, p.0[2].0 as u32])
                .collect::<Vec<_>>(),
            TriMeshFlags::all(),
        )
        .unwrap();

        let res = intersect_meshes(
            &Pose::identity(),
            &bunny_mesh,
            false,
            &Pose::identity(),
            &cylinder_mesh,
            true,
        )
        .unwrap()
        .unwrap();

        let _ = res.to_obj_file(&PathBuf::from("complex_test.obj"));
    }
}

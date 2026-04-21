use crate::bounding_volume::Aabb;
use crate::math::{Pose, Real, Vector, VectorExt};
use crate::query::{IntersectResult, PointQuery, SplitResult};
use crate::shape::{Cuboid, FeatureId, Polyline, Segment, Shape, TriMesh, TriMeshFlags, Triangle};
use crate::transformation::{intersect_meshes, MeshIntersectionError};
use crate::utils::{hashmap::HashMap, SortedPair, WBasis};
use alloc::{vec, vec::Vec};
use core::cmp::Ordering;
use spade::{handles::FixedVertexHandle, ConstrainedDelaunayTriangulation, Triangulation as _};

struct Triangulation {
    delaunay: ConstrainedDelaunayTriangulation<spade::Point2<Real>>,
    basis: [Vector; 2],
    basis_origin: Vector,
    spade2index: HashMap<FixedVertexHandle, u32>,
    index2spade: HashMap<u32, FixedVertexHandle>,
}

impl Triangulation {
    fn new(axis: Vector, basis_origin: Vector) -> Self {
        Triangulation {
            delaunay: ConstrainedDelaunayTriangulation::new(),
            basis: axis.orthonormal_basis(),
            basis_origin,
            spade2index: HashMap::default(),
            index2spade: HashMap::default(),
        }
    }

    fn project(&self, pt: Vector) -> spade::Point2<Real> {
        let dpt = pt - self.basis_origin;
        spade::Point2::new(dpt.dot(self.basis[0]), dpt.dot(self.basis[1]))
    }

    fn add_edge(&mut self, id1: u32, id2: u32, points: &[Vector]) {
        let proj1 = self.project(points[id1 as usize]);
        let proj2 = self.project(points[id2 as usize]);

        let handle1 = *self.index2spade.entry(id1).or_insert_with(|| {
            let h = self.delaunay.insert(proj1).unwrap();
            let _ = self.spade2index.insert(h, id1);
            h
        });

        let handle2 = *self.index2spade.entry(id2).or_insert_with(|| {
            let h = self.delaunay.insert(proj2).unwrap();
            let _ = self.spade2index.insert(h, id2);
            h
        });

        // NOTE: the naming of the `ConstrainedDelaunayTriangulation::can_add_constraint` method is misleading.
        if !self.delaunay.can_add_constraint(handle1, handle2) {
            let _ = self.delaunay.add_constraint(handle1, handle2);
        }
    }
}

impl TriMesh {
    /// Splits this `TriMesh` along the given canonical axis.
    ///
    /// This will split the Aabb by a plane with a normal with it’s `axis`-th component set to 1.
    /// The splitting plane is shifted wrt. the origin by the `bias` (i.e. it passes through the point
    /// equal to `normal * bias`).
    ///
    /// # Result
    /// Returns the result of the split. The first mesh returned is the piece lying on the negative
    /// half-space delimited by the splitting plane. The second mesh returned is the piece lying on the
    /// positive half-space delimited by the splitting plane.
    pub fn canonical_split(&self, axis: usize, bias: Real, epsilon: Real) -> SplitResult<Self> {
        // TODO: optimize this.
        self.local_split(Vector::ith(axis, 1.0), bias, epsilon)
    }

    /// Splits this mesh, transformed by `position` by a plane identified by its normal `local_axis`
    /// and the `bias` (i.e. the plane passes through the point equal to `normal * bias`).
    pub fn split(
        &self,
        position: &Pose,
        axis: Vector,
        bias: Real,
        epsilon: Real,
    ) -> SplitResult<Self> {
        let local_axis = position.rotation.inverse() * axis;
        let added_bias = -position.translation.dot(axis);
        self.local_split(local_axis, bias + added_bias, epsilon)
    }

    /// Splits this mesh by a plane identified by its normal `local_axis`
    /// and the `bias` (i.e. the plane passes through the point equal to `normal * bias`).
    pub fn local_split(&self, local_axis: Vector, bias: Real, epsilon: Real) -> SplitResult<Self> {
        let mut triangulation = if self.pseudo_normals_if_oriented().is_some() {
            Some(Triangulation::new(local_axis, self.vertices()[0]))
        } else {
            None
        };

        // 1. Partition the vertices.
        let vertices = self.vertices();
        let indices = self.indices();
        let mut colors = vec![0u8; self.vertices().len()];

        // Color 0 = on plane.
        //       1 = on negative half-space.
        //       2 = on positive half-space.
        let mut found_negative = false;
        let mut found_positive = false;
        for (i, pt) in vertices.iter().enumerate() {
            let dist_to_plane = pt.dot(local_axis) - bias;
            if dist_to_plane < -epsilon {
                found_negative = true;
                colors[i] = 1;
            } else if dist_to_plane > epsilon {
                found_positive = true;
                colors[i] = 2;
            }
        }

        // Exit early if `self` isn’t crossed by the plane.
        if !found_negative {
            return SplitResult::Positive;
        }

        if !found_positive {
            return SplitResult::Negative;
        }

        // 2. Split the triangles.
        let mut intersections_found = HashMap::default();
        let mut new_indices = indices.to_vec();
        let mut new_vertices = vertices.to_vec();

        for (tri_id, idx) in indices.iter().enumerate() {
            let mut intersection_features = (FeatureId::Unknown, FeatureId::Unknown);

            // First, find where the plane intersects the triangle.
            for ia in 0..3 {
                let ib = (ia + 1) % 3;
                let idx_a = idx[ia as usize];
                let idx_b = idx[ib as usize];

                let fid = match (colors[idx_a as usize], colors[idx_b as usize]) {
                    (1, 2) | (2, 1) => FeatureId::Edge(ia),
                    // NOTE: the case (_, 0) will be dealt with in the next loop iteration.
                    (0, _) => FeatureId::Vertex(ia),
                    _ => continue,
                };

                if intersection_features.0 == FeatureId::Unknown {
                    intersection_features.0 = fid;
                } else {
                    // TODO: this assertion may fire if the triangle is coplanar with the edge?
                    // assert_eq!(intersection_features.1, FeatureId::Unknown);
                    intersection_features.1 = fid;
                }
            }

            // Helper that intersects an edge with the plane.
            let mut intersect_edge = |idx_a, idx_b| {
                *intersections_found
                    .entry(SortedPair::new(idx_a, idx_b))
                    .or_insert_with(|| {
                        let segment = Segment::new(
                            new_vertices[idx_a as usize],
                            new_vertices[idx_b as usize],
                        );
                        // Intersect the segment with the plane.
                        if let Some((intersection, _)) = segment
                            .local_split_and_get_intersection(local_axis, bias, epsilon)
                            .1
                        {
                            new_vertices.push(intersection);
                            colors.push(0);
                            (new_vertices.len() - 1) as u32
                        } else {
                            unreachable!()
                        }
                    })
            };

            // Perform the intersection, push new triangles, and update
            // triangulation constraints if needed.
            match intersection_features {
                (_, FeatureId::Unknown) => {
                    // The plane doesn’t intersect the triangle, or intersects it at
                    // a single vertex, so we don’t have anything to do.
                    assert!(
                        matches!(intersection_features.0, FeatureId::Unknown)
                            || matches!(intersection_features.0, FeatureId::Vertex(_))
                    );
                }
                (FeatureId::Vertex(v1), FeatureId::Vertex(v2)) => {
                    // The plane intersects the triangle along one of its edge.
                    // We don’t have to split the triangle, but we need to add
                    // a constraint to the triangulation.
                    if let Some(triangulation) = &mut triangulation {
                        let id1 = idx[v1 as usize];
                        let id2 = idx[v2 as usize];
                        triangulation.add_edge(id1, id2, &new_vertices);
                    }
                }
                (FeatureId::Vertex(iv), FeatureId::Edge(ie))
                | (FeatureId::Edge(ie), FeatureId::Vertex(iv)) => {
                    // The plane splits the triangle into exactly two triangles.
                    let ia = ie;
                    let ib = (ie + 1) % 3;
                    let ic = (ie + 2) % 3;
                    let idx_a = idx[ia as usize];
                    let idx_b = idx[ib as usize];
                    let idx_c = idx[ic as usize];
                    assert_eq!(iv, ic);

                    let intersection_idx = intersect_edge(idx_a, idx_b);

                    // Compute the indices of the two triangles.
                    let new_tri_a = [idx_c, idx_a, intersection_idx];
                    let new_tri_b = [idx_b, idx_c, intersection_idx];

                    new_indices[tri_id] = new_tri_a;
                    new_indices.push(new_tri_b);

                    if let Some(triangulation) = &mut triangulation {
                        triangulation.add_edge(intersection_idx, idx_c, &new_vertices);
                    }
                }
                (FeatureId::Edge(mut e1), FeatureId::Edge(mut e2)) => {
                    // The plane splits the triangle into 1 + 2 triangles.
                    // First, make sure the edge indices are consecutive.
                    if e2 != (e1 + 1) % 3 {
                        core::mem::swap(&mut e1, &mut e2);
                    }

                    let ia = e2; // The first point of the second edge is the vertex shared by both edges.
                    let ib = (e2 + 1) % 3;
                    let ic = (e2 + 2) % 3;
                    let idx_a = idx[ia as usize];
                    let idx_b = idx[ib as usize];
                    let idx_c = idx[ic as usize];

                    let intersection1 = intersect_edge(idx_c, idx_a);
                    let intersection2 = intersect_edge(idx_a, idx_b);

                    let new_tri1 = [idx_a, intersection2, intersection1];
                    let new_tri2 = [intersection2, idx_b, idx_c];
                    let new_tri3 = [intersection2, idx_c, intersection1];
                    new_indices[tri_id] = new_tri1;
                    new_indices.push(new_tri2);
                    new_indices.push(new_tri3);

                    if let Some(triangulation) = &mut triangulation {
                        triangulation.add_edge(intersection1, intersection2, &new_vertices);
                    }
                }
                _ => unreachable!(),
            }
        }

        // 3. Partition the new triangles into two trimeshes.
        let mut vertices_lhs = vec![];
        let mut vertices_rhs = vec![];
        let mut indices_lhs = vec![];
        let mut indices_rhs = vec![];
        let mut remap = vec![];

        for i in 0..new_vertices.len() {
            match colors[i] {
                0 => {
                    remap.push((vertices_lhs.len() as u32, vertices_rhs.len() as u32));
                    vertices_lhs.push(new_vertices[i]);
                    vertices_rhs.push(new_vertices[i]);
                }
                1 => {
                    remap.push((vertices_lhs.len() as u32, u32::MAX));
                    vertices_lhs.push(new_vertices[i]);
                }
                2 => {
                    remap.push((u32::MAX, vertices_rhs.len() as u32));
                    vertices_rhs.push(new_vertices[i]);
                }
                _ => unreachable!(),
            }
        }

        for idx in new_indices {
            let idx = [idx[0] as usize, idx[1] as usize, idx[2] as usize]; // Convert to usize.
            let colors = [colors[idx[0]], colors[idx[1]], colors[idx[2]]];
            let remap = [remap[idx[0]], remap[idx[1]], remap[idx[2]]];

            if colors[0] == 1 || colors[1] == 1 || colors[2] == 1 {
                assert!(colors[0] != 2 && colors[1] != 2 && colors[2] != 2);
                indices_lhs.push([remap[0].0, remap[1].0, remap[2].0]);
            } else if colors[0] == 2 || colors[1] == 2 || colors[2] == 2 {
                assert!(colors[0] != 1 && colors[1] != 1 && colors[2] != 1);
                indices_rhs.push([remap[0].1, remap[1].1, remap[2].1]);
            } else {
                // The colors are all 0, so push into both trimeshes.
                indices_lhs.push([remap[0].0, remap[1].0, remap[2].0]);
                indices_rhs.push([remap[0].1, remap[1].1, remap[2].1]);
            }
        }

        // Push the triangulation if there is one.
        if let Some(triangulation) = triangulation {
            for face in triangulation.delaunay.inner_faces() {
                let vtx = face.vertices();
                let mut idx1 = [0; 3];
                let mut idx2 = [0; 3];
                for k in 0..3 {
                    let vid = triangulation.spade2index[&vtx[k].fix()];
                    assert_eq!(colors[vid as usize], 0);
                    idx1[k] = remap[vid as usize].0;
                    idx2[k] = remap[vid as usize].1;
                }

                let tri = Triangle::new(
                    vertices_lhs[idx1[0] as usize],
                    vertices_lhs[idx1[1] as usize],
                    vertices_lhs[idx1[2] as usize],
                );

                if self.contains_local_point(tri.center()) {
                    indices_lhs.push(idx1);

                    idx2.swap(1, 2); // Flip orientation for the second half of the split.
                    indices_rhs.push(idx2);
                }
            }
        }

        // TODO: none of the index buffers should be empty at this point unless perhaps
        //       because of some rounding errors?
        //       Should we just panic if they are empty?
        if indices_rhs.is_empty() {
            SplitResult::Negative
        } else if indices_lhs.is_empty() {
            SplitResult::Positive
        } else {
            let mesh_lhs = TriMesh::new(vertices_lhs, indices_lhs).unwrap();
            let mesh_rhs = TriMesh::new(vertices_rhs, indices_rhs).unwrap();
            SplitResult::Pair(mesh_lhs, mesh_rhs)
        }
    }

    /// Computes the intersection [`Polyline`]s between this mesh and the plane identified by
    /// the given canonical axis.
    ///
    /// This will intersect the mesh by a plane with a normal with it’s `axis`-th component set to 1.
    /// The splitting plane is shifted wrt. the origin by the `bias` (i.e. it passes through the point
    /// equal to `normal * bias`).
    ///
    /// Note that the resultant polyline may have multiple connected components
    pub fn canonical_intersection_with_plane(
        &self,
        axis: usize,
        bias: Real,
        epsilon: Real,
    ) -> IntersectResult<Polyline> {
        self.intersection_with_local_plane(Vector::ith(axis, 1.0), bias, epsilon)
    }

    /// Computes the intersection [`Polyline`]s between this mesh, transformed by `position`,
    /// and a plane identified by its normal `axis` and the `bias`
    /// (i.e. the plane passes through the point equal to `normal * bias`).
    pub fn intersection_with_plane(
        &self,
        position: &Pose,
        axis: Vector,
        bias: Real,
        epsilon: Real,
    ) -> IntersectResult<Polyline> {
        let local_axis = position.rotation.inverse() * axis;
        let added_bias = -position.translation.dot(axis);
        self.intersection_with_local_plane(local_axis, bias + added_bias, epsilon)
    }

    /// Computes the intersection [`Polyline`]s between this mesh
    /// and a plane identified by its normal `local_axis`
    /// and the `bias` (i.e. the plane passes through the point equal to `normal * bias`).
    pub fn intersection_with_local_plane(
        &self,
        local_axis: Vector,
        bias: Real,
        epsilon: Real,
    ) -> IntersectResult<Polyline> {
        // 1. Partition the vertices.
        let vertices = self.vertices();
        let indices = self.indices();
        let mut colors = vec![0u8; self.vertices().len()];

        // Color 0 = on plane.
        //       1 = on negative half-space.
        //       2 = on positive half-space.
        let mut found_negative = false;
        let mut found_positive = false;
        for (i, pt) in vertices.iter().enumerate() {
            let dist_to_plane = pt.dot(local_axis) - bias;
            if dist_to_plane < -epsilon {
                found_negative = true;
                colors[i] = 1;
            } else if dist_to_plane > epsilon {
                found_positive = true;
                colors[i] = 2;
            }
        }

        // Exit early if `self` isn’t crossed by the plane.
        if !found_negative {
            return IntersectResult::Positive;
        }

        if !found_positive {
            return IntersectResult::Negative;
        }

        // 2. Split the triangles.
        let mut index_adjacencies: Vec<Vec<usize>> = Vec::new(); // Adjacency list of indices

        // Helper functions for adding polyline segments to the adjacency list
        let mut add_segment_adjacencies = |idx_a: usize, idx_b| {
            assert!(idx_a <= index_adjacencies.len());

            match idx_a.cmp(&index_adjacencies.len()) {
                Ordering::Less => index_adjacencies[idx_a].push(idx_b),
                Ordering::Equal => index_adjacencies.push(vec![idx_b]),
                Ordering::Greater => {}
            }
        };
        let mut add_segment_adjacencies_symmetric = |idx_a: usize, idx_b| {
            if idx_a < idx_b {
                add_segment_adjacencies(idx_a, idx_b);
                add_segment_adjacencies(idx_b, idx_a);
            } else {
                add_segment_adjacencies(idx_b, idx_a);
                add_segment_adjacencies(idx_a, idx_b);
            }
        };

        let mut intersections_found = HashMap::default();
        let mut existing_vertices_found = HashMap::default();
        let mut new_vertices = Vec::new();

        for idx in indices.iter() {
            let mut intersection_features = (FeatureId::Unknown, FeatureId::Unknown);

            // First, find where the plane intersects the triangle.
            for ia in 0..3 {
                let ib = (ia + 1) % 3;
                let idx_a = idx[ia as usize];
                let idx_b = idx[ib as usize];

                let fid = match (colors[idx_a as usize], colors[idx_b as usize]) {
                    (1, 2) | (2, 1) => FeatureId::Edge(ia),
                    // NOTE: the case (_, 0) will be dealt with in the next loop iteration.
                    (0, _) => FeatureId::Vertex(ia),
                    _ => continue,
                };

                if intersection_features.0 == FeatureId::Unknown {
                    intersection_features.0 = fid;
                } else {
                    // TODO: this assertion may fire if the triangle is coplanar with the edge?
                    // assert_eq!(intersection_features.1, FeatureId::Unknown);
                    intersection_features.1 = fid;
                }
            }

            // Helper that intersects an edge with the plane.
            let mut intersect_edge = |idx_a, idx_b| {
                *intersections_found
                    .entry(SortedPair::new(idx_a, idx_b))
                    .or_insert_with(|| {
                        let segment =
                            Segment::new(vertices[idx_a as usize], vertices[idx_b as usize]);
                        // Intersect the segment with the plane.
                        if let Some((intersection, _)) = segment
                            .local_split_and_get_intersection(local_axis, bias, epsilon)
                            .1
                        {
                            new_vertices.push(intersection);
                            colors.push(0);
                            new_vertices.len() - 1
                        } else {
                            unreachable!()
                        }
                    })
            };

            // Perform the intersection, push new triangles, and update
            // triangulation constraints if needed.
            match intersection_features {
                (_, FeatureId::Unknown) => {
                    // The plane doesn’t intersect the triangle, or intersects it at
                    // a single vertex, so we don’t have anything to do.
                    assert!(
                        matches!(intersection_features.0, FeatureId::Unknown)
                            || matches!(intersection_features.0, FeatureId::Vertex(_))
                    );
                }
                (FeatureId::Vertex(iv1), FeatureId::Vertex(iv2)) => {
                    // The plane intersects the triangle along one of its edge.
                    // We don’t have to split the triangle, but we need to add
                    // the edge to the polyline indices

                    let id1 = idx[iv1 as usize];
                    let id2 = idx[iv2 as usize];

                    let out_id1 = *existing_vertices_found.entry(id1).or_insert_with(|| {
                        let v1 = vertices[id1 as usize];

                        new_vertices.push(v1);
                        new_vertices.len() - 1
                    });
                    let out_id2 = *existing_vertices_found.entry(id2).or_insert_with(|| {
                        let v2 = vertices[id2 as usize];

                        new_vertices.push(v2);
                        new_vertices.len() - 1
                    });

                    add_segment_adjacencies_symmetric(out_id1, out_id2);
                }
                (FeatureId::Vertex(iv), FeatureId::Edge(ie))
                | (FeatureId::Edge(ie), FeatureId::Vertex(iv)) => {
                    // The plane splits the triangle into exactly two triangles.
                    let ia = ie;
                    let ib = (ie + 1) % 3;
                    let ic = (ie + 2) % 3;
                    let idx_a = idx[ia as usize];
                    let idx_b = idx[ib as usize];
                    let idx_c = idx[ic as usize];
                    assert_eq!(iv, ic);

                    let intersection_idx = intersect_edge(idx_a, idx_b);

                    let out_idx_c = *existing_vertices_found.entry(idx_c).or_insert_with(|| {
                        let v2 = vertices[idx_c as usize];

                        new_vertices.push(v2);
                        new_vertices.len() - 1
                    });

                    add_segment_adjacencies_symmetric(out_idx_c, intersection_idx);
                }
                (FeatureId::Edge(mut e1), FeatureId::Edge(mut e2)) => {
                    // The plane splits the triangle into 1 + 2 triangles.
                    // First, make sure the edge indices are consecutive.
                    if e2 != (e1 + 1) % 3 {
                        core::mem::swap(&mut e1, &mut e2);
                    }

                    let ia = e2; // The first point of the second edge is the vertex shared by both edges.
                    let ib = (e2 + 1) % 3;
                    let ic = (e2 + 2) % 3;
                    let idx_a = idx[ia as usize];
                    let idx_b = idx[ib as usize];
                    let idx_c = idx[ic as usize];

                    let intersection1 = intersect_edge(idx_c, idx_a);
                    let intersection2 = intersect_edge(idx_a, idx_b);

                    add_segment_adjacencies_symmetric(intersection1, intersection2);
                }
                _ => unreachable!(),
            }
        }

        // 3. Ensure consistent edge orientation by traversing the adjacency list
        let mut polyline_indices: Vec<[u32; 2]> = Vec::with_capacity(index_adjacencies.len() + 1);

        let mut seen = vec![false; index_adjacencies.len()];
        for (idx, neighbors) in index_adjacencies.iter().enumerate() {
            if !seen[idx] {
                // Start a new component
                // Traverse the adjencies until the loop closes

                let first = idx;
                let mut prev = first;
                let mut next = neighbors.first(); // Arbitrary neighbor

                'traversal: while let Some(current) = next {
                    seen[*current] = true;
                    polyline_indices.push([prev as u32, *current as u32]);

                    for neighbor in index_adjacencies[*current].iter() {
                        if *neighbor != prev && *neighbor != first {
                            prev = *current;
                            next = Some(neighbor);
                            continue 'traversal;
                        } else if *neighbor != prev && *neighbor == first {
                            // If the next index is same as the first, close the polyline and exit
                            polyline_indices.push([*current as u32, first as u32]);
                            next = None;
                            continue 'traversal;
                        }
                    }
                }
            }
        }

        IntersectResult::Intersect(Polyline::new(new_vertices, Some(polyline_indices)))
    }

    /// Computes the intersection mesh between an Aabb and this mesh.
    pub fn intersection_with_aabb(
        &self,
        position: &Pose,
        flip_mesh: bool,
        aabb: &Aabb,
        flip_cuboid: bool,
        epsilon: Real,
    ) -> Result<Option<Self>, MeshIntersectionError> {
        let cuboid = Cuboid::new(aabb.half_extents());
        let cuboid_pos = Pose::from_translation(aabb.center());
        self.intersection_with_cuboid(
            position,
            flip_mesh,
            &cuboid,
            &cuboid_pos,
            flip_cuboid,
            epsilon,
        )
    }

    /// Computes the intersection mesh between a cuboid and this mesh transformed by `position`.
    pub fn intersection_with_cuboid(
        &self,
        position: &Pose,
        flip_mesh: bool,
        cuboid: &Cuboid,
        cuboid_position: &Pose,
        flip_cuboid: bool,
        epsilon: Real,
    ) -> Result<Option<Self>, MeshIntersectionError> {
        self.intersection_with_local_cuboid(
            flip_mesh,
            cuboid,
            &position.inv_mul(cuboid_position),
            flip_cuboid,
            epsilon,
        )
    }

    /// Computes the intersection mesh between a cuboid and this mesh.
    pub fn intersection_with_local_cuboid(
        &self,
        flip_mesh: bool,
        cuboid: &Cuboid,
        cuboid_position: &Pose,
        flip_cuboid: bool,
        _epsilon: Real,
    ) -> Result<Option<Self>, MeshIntersectionError> {
        if self.topology().is_some() && self.pseudo_normals_if_oriented().is_some() {
            let (cuboid_vtx, cuboid_idx) = cuboid.to_trimesh();
            let cuboid_trimesh = TriMesh::with_flags(
                cuboid_vtx,
                cuboid_idx,
                TriMeshFlags::HALF_EDGE_TOPOLOGY | TriMeshFlags::ORIENTED,
            )
            // `TriMesh::with_flags` can't fail for `cuboid.to_trimesh()`.
            .unwrap();

            let intersect_meshes = intersect_meshes(
                &Pose::IDENTITY,
                self,
                flip_mesh,
                cuboid_position,
                &cuboid_trimesh,
                flip_cuboid,
            );
            return intersect_meshes;
        }

        let cuboid_aabb = cuboid.compute_aabb(cuboid_position);
        let intersecting_tris: Vec<_> = self.bvh().intersect_aabb(&cuboid_aabb).collect();

        if intersecting_tris.is_empty() {
            return Ok(None);
        }

        // First, very naive version that outputs a triangle soup without
        // index buffer (shared vertices are duplicated).
        let vertices = self.vertices();
        let indices = self.indices();

        let mut clip_workspace = vec![];
        let mut new_vertices = vec![];
        let mut new_indices = vec![];
        let aabb = cuboid.local_aabb();
        let inv_pos = cuboid_position.inverse();
        let mut to_clip = vec![];

        for tri in intersecting_tris {
            let idx = indices[tri as usize];
            to_clip.extend_from_slice(&[
                inv_pos * vertices[idx[0] as usize],
                inv_pos * vertices[idx[1] as usize],
                inv_pos * vertices[idx[2] as usize],
            ]);

            // There is no need to clip if the triangle is fully inside of the Aabb.
            // Note that we can’t take a shortcut for the case where all the vertices are
            // outside of the Aabb, because the Aabb can still instersect the edges or face.
            if !(aabb.contains_local_point(to_clip[0])
                && aabb.contains_local_point(to_clip[1])
                && aabb.contains_local_point(to_clip[2]))
            {
                aabb.clip_polygon_with_workspace(&mut to_clip, &mut clip_workspace);
            }

            if to_clip.len() >= 3 {
                let base_i = new_vertices.len();
                for i in 1..to_clip.len() - 1 {
                    new_indices.push([base_i as u32, (base_i + i) as u32, (base_i + i + 1) as u32]);
                }
                new_vertices.append(&mut to_clip);
            }
        }

        // The clipping outputs points in the local-space of the cuboid.
        // So we need to transform it back.
        for pt in &mut new_vertices {
            *pt = cuboid_position * *pt;
        }

        Ok(if new_vertices.len() >= 3 {
            Some(TriMesh::new(new_vertices, new_indices).unwrap())
        } else {
            None
        })
    }
}

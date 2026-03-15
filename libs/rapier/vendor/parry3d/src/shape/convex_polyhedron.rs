use crate::math::{Real, Vector, DIM};
use crate::shape::{FeatureId, PackedFeatureId, PolygonalFeature, PolygonalFeatureMap, SupportMap};
// use crate::transformation;
#[cfg(not(feature = "std"))]
use crate::math::ComplexField;
use crate::utils::hashmap::{Entry, HashMap};
use crate::utils::{self, SortedPair};
#[cfg(feature = "alloc")]
use alloc::vec::Vec;
use core::f64; // for .abs(), .sqrt(), and .sin_cos()

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(PartialEq, Debug, Copy, Clone)]
pub struct Vertex {
    pub first_adj_face_or_edge: u32,
    pub num_adj_faces_or_edge: u32,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(PartialEq, Debug, Copy, Clone)]
pub struct Edge {
    pub vertices: [u32; 2],
    pub faces: [u32; 2],
    pub dir: Vector,
    deleted: bool,
}

impl Edge {
    fn other_triangle(&self, id: u32) -> u32 {
        if id == self.faces[0] {
            self.faces[1]
        } else {
            self.faces[0]
        }
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(PartialEq, Debug, Copy, Clone)]
pub struct Face {
    pub first_vertex_or_edge: u32,
    pub num_vertices_or_edges: u32,
    pub normal: Vector,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(PartialEq, Debug, Copy, Clone)]
struct Triangle {
    vertices: [u32; 3],
    edges: [u32; 3],
    normal: Vector,
    parent_face: Option<u32>,
    is_degenerate: bool,
}

impl Triangle {
    fn next_edge_id(&self, id: u32) -> u32 {
        for i in 0..3 {
            if self.edges[i] == id {
                return (i as u32 + 1) % 3;
            }
        }

        unreachable!()
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(PartialEq, Debug, Clone)]
/// A 3D convex polyhedron without degenerate faces.
///
/// A convex polyhedron is a 3D solid shape where all faces are flat polygons, all interior angles
/// are less than 180 degrees, and any line segment drawn between two points inside the polyhedron
/// stays entirely within it. Common examples include cubes, tetrahedra (pyramids), and prisms.
///
/// # What is a convex polyhedron?
///
/// In 3D space, a polyhedron is **convex** if:
/// - All faces are flat, convex polygons
/// - All dihedral angles (angles between adjacent faces) are less than or equal to 180 degrees
/// - The line segment between any two points inside the polyhedron lies entirely inside
/// - All vertices "bulge outward" - there are no indentations or concave regions
///
/// Examples of **convex** polyhedra: cube, tetrahedron, octahedron, rectangular prism, pyramid
/// Examples of **non-convex** polyhedra: star shapes, torus, L-shapes, any shape with holes
///
/// # Use cases
///
/// Convex polyhedra are widely used in:
/// - **Game development**: Collision hulls for characters, vehicles, and complex objects
/// - **Physics simulations**: Rigid body dynamics (more efficient than arbitrary meshes)
/// - **Robotics**: Simplified robot link shapes, workspace boundaries
/// - **Computer graphics**: Level of detail (LOD) representations, occlusion culling
/// - **Computational geometry**: Building blocks for complex mesh operations
/// - **3D printing**: Simplified collision detection for print validation
///
/// # Representation
///
/// This structure stores the complete topological information:
/// - **Vectors**: The 3D coordinates of all vertices
/// - **Faces**: Polygonal faces with their outward-pointing normals
/// - **Edges**: Connections between vertices, shared by exactly two faces
/// - **Adjacency information**: Which faces/edges connect to each vertex, and vice versa
///
/// This rich topology enables efficient collision detection algorithms like GJK, EPA, and SAT.
///
/// # Important: No degenerate faces
///
/// This structure ensures that there are no degenerate (flat or zero-area) faces. Coplanar
/// adjacent triangles are automatically merged into larger polygonal faces during construction.
///
/// # Example: Creating a simple tetrahedron (pyramid)
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::ConvexPolyhedron;
/// use parry3d::math::Vector;
///
/// // Define the 4 vertices of a tetrahedron
/// let points = vec![
///     Vector::ZERO,      // base vertex 1
///     Vector::new(1.0, 0.0, 0.0),      // base vertex 2
///     Vector::new(0.5, 1.0, 0.0),      // base vertex 3
///     Vector::new(0.5, 0.5, 1.0),      // apex
/// ];
///
/// // Define the 4 triangular faces (indices into the points array)
/// let indices = vec![
///     [0u32, 1, 2],  // base triangle
///     [0, 1, 3],     // side 1
///     [1, 2, 3],     // side 2
///     [2, 0, 3],     // side 3
/// ];
///
/// let tetrahedron = ConvexPolyhedron::from_convex_mesh(points, &indices)
///     .expect("Failed to create tetrahedron");
///
/// // A tetrahedron has 4 vertices and 4 faces
/// assert_eq!(tetrahedron.points().len(), 4);
/// assert_eq!(tetrahedron.faces().len(), 4);
/// # }
/// ```
pub struct ConvexPolyhedron {
    points: Vec<Vector>,
    vertices: Vec<Vertex>,
    faces: Vec<Face>,
    edges: Vec<Edge>,
    // Faces adjacent to a vertex.
    faces_adj_to_vertex: Vec<u32>,
    // Edges adjacent to a vertex.
    edges_adj_to_vertex: Vec<u32>,
    // Edges adjacent to a face.
    edges_adj_to_face: Vec<u32>,
    // Vertices adjacent to a face.
    vertices_adj_to_face: Vec<u32>,
}

impl ConvexPolyhedron {
    /// Creates a new convex polyhedron from an arbitrary set of points by computing their convex hull.
    ///
    /// This is the most flexible constructor - it automatically computes the **convex hull** of the
    /// given 3D points, which is the smallest convex polyhedron that contains all the input points.
    /// Think of it as shrink-wrapping the points with an elastic surface.
    ///
    /// Use this when:
    /// - You have an arbitrary collection of 3D points and want the convex boundary
    /// - You're not sure if your points form a convex shape
    /// - You want to simplify a point cloud to its convex outer surface
    /// - You need to create a collision hull from a detailed mesh
    ///
    /// # Returns
    ///
    /// - `Some(ConvexPolyhedron)` if successful
    /// - `None` if the convex hull computation failed (e.g., all points are coplanar, collinear,
    ///   or coincident, or if the mesh is not manifold)
    ///
    /// # Example: Creating a convex hull from point cloud
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::ConvexPolyhedron;
    /// use parry3d::math::Vector;
    ///
    /// // Vectors defining a cube, plus some interior points
    /// let points = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(1.0, 1.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    ///     Vector::new(0.0, 0.0, 1.0),
    ///     Vector::new(1.0, 0.0, 1.0),
    ///     Vector::new(1.0, 1.0, 1.0),
    ///     Vector::new(0.0, 1.0, 1.0),
    ///     Vector::new(0.5, 0.5, 0.5),  // Interior point - will be excluded
    /// ];
    ///
    /// let polyhedron = ConvexPolyhedron::from_convex_hull(&points)
    ///     .expect("Failed to create convex hull");
    ///
    /// // The cube has 8 vertices (interior point excluded)
    /// assert_eq!(polyhedron.points().len(), 8);
    /// // And 6 faces (one per side of the cube)
    /// assert_eq!(polyhedron.faces().len(), 6);
    /// # }
    /// ```
    ///
    /// # Example: Simplifying a complex mesh
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::ConvexPolyhedron;
    /// use parry3d::math::Vector;
    ///
    /// // Imagine these are vertices from a detailed character mesh
    /// let detailed_mesh_vertices = vec![
    ///     Vector::new(-1.0, -1.0, -1.0),
    ///     Vector::new(1.0, -1.0, -1.0),
    ///     Vector::new(1.0, 1.0, -1.0),
    ///     Vector::new(-1.0, 1.0, -1.0),
    ///     Vector::new(-1.0, -1.0, 1.0),
    ///     Vector::new(1.0, -1.0, 1.0),
    ///     Vector::new(1.0, 1.0, 1.0),
    ///     Vector::new(-1.0, 1.0, 1.0),
    ///     // ... many more vertices in the original mesh
    /// ];
    ///
    /// // Create a simplified collision hull
    /// let collision_hull = ConvexPolyhedron::from_convex_hull(&detailed_mesh_vertices)
    ///     .expect("Failed to create collision hull");
    ///
    /// // The hull is much simpler than the original mesh
    /// println!("Simplified to {} vertices", collision_hull.points().len());
    /// # }
    /// ```
    pub fn from_convex_hull(points: &[Vector]) -> Option<ConvexPolyhedron> {
        crate::transformation::try_convex_hull(points)
            .ok()
            .and_then(|(vertices, indices)| Self::from_convex_mesh(vertices, &indices))
    }

    /// Creates a new convex polyhedron from vertices and face indices, assuming the mesh is already convex.
    ///
    /// This constructor is more efficient than [`from_convex_hull`] because it **assumes** the input
    /// mesh is already convex and manifold. The convexity is **not verified** - if you pass a non-convex
    /// mesh, the resulting shape may behave incorrectly in collision detection.
    ///
    /// The method automatically:
    /// - Merges coplanar adjacent triangular faces into larger polygonal faces
    /// - Removes degenerate (zero-area) faces
    /// - Builds complete topological connectivity information (vertices, edges, faces)
    ///
    /// # Important requirements
    ///
    /// The input mesh must be:
    /// - **Manifold**: Each edge is shared by exactly 2 faces, no T-junctions, no holes
    /// - **Closed**: The mesh completely encloses a volume with no gaps
    /// - **Convex** (not enforced, but assumed): All faces bulge outward
    /// - **Valid**: Faces use counter-clockwise winding when viewed from outside
    ///
    /// # When to use this
    ///
    /// Use this constructor when:
    /// - You already have a triangulated convex mesh
    /// - The mesh comes from a trusted source (e.g., generated procedurally)
    /// - You want better performance by skipping convex hull computation
    /// - You're converting from another 3D format (OBJ, STL, etc.)
    ///
    /// # Arguments
    ///
    /// * `points` - The vertex positions (3D coordinates)
    /// * `indices` - The face triangles, each containing 3 indices into the `points` array
    ///
    /// # Returns
    ///
    /// - `Some(ConvexPolyhedron)` if successful
    /// - `None` if the mesh is not manifold, has T-junctions, is not closed, or has invalid topology
    ///
    /// # Example: Creating a cube from vertices and faces
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::ConvexPolyhedron;
    /// use parry3d::math::Vector;
    ///
    /// // Define the 8 vertices of a unit cube
    /// let vertices = vec![
    ///     Vector::ZERO,  // 0: bottom-left-front
    ///     Vector::new(1.0, 0.0, 0.0),  // 1: bottom-right-front
    ///     Vector::new(1.0, 1.0, 0.0),  // 2: bottom-right-back
    ///     Vector::new(0.0, 1.0, 0.0),  // 3: bottom-left-back
    ///     Vector::new(0.0, 0.0, 1.0),  // 4: top-left-front
    ///     Vector::new(1.0, 0.0, 1.0),  // 5: top-right-front
    ///     Vector::new(1.0, 1.0, 1.0),  // 6: top-right-back
    ///     Vector::new(0.0, 1.0, 1.0),  // 7: top-left-back
    /// ];
    ///
    /// // Define the faces as triangles (2 triangles per cube face)
    /// let indices = vec![
    ///     // Bottom face (z = 0)
    ///     [0, 2, 1], [0, 3, 2],
    ///     // Top face (z = 1)
    ///     [4, 5, 6], [4, 6, 7],
    ///     // Front face (y = 0)
    ///     [0, 1, 5], [0, 5, 4],
    ///     // Back face (y = 1)
    ///     [2, 3, 7], [2, 7, 6],
    ///     // Left face (x = 0)
    ///     [0, 4, 7], [0, 7, 3],
    ///     // Right face (x = 1)
    ///     [1, 2, 6], [1, 6, 5],
    /// ];
    ///
    /// let cube = ConvexPolyhedron::from_convex_mesh(vertices, &indices)
    ///     .expect("Failed to create cube");
    ///
    /// // The cube has 8 vertices
    /// assert_eq!(cube.points().len(), 8);
    /// // The 12 triangles are merged into 6 square faces
    /// assert_eq!(cube.faces().len(), 6);
    /// # }
    /// ```
    ///
    /// # Example: Creating a triangular prism
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::ConvexPolyhedron;
    /// use parry3d::math::Vector;
    ///
    /// // 6 vertices: 3 on bottom, 3 on top
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.5, 1.0, 0.0),
    ///     Vector::new(0.0, 0.0, 2.0),
    ///     Vector::new(1.0, 0.0, 2.0),
    ///     Vector::new(0.5, 1.0, 2.0),
    /// ];
    ///
    /// let indices = vec![
    ///     // Bottom triangle
    ///     [0, 2, 1],
    ///     // Top triangle
    ///     [3, 4, 5],
    ///     // Side faces (2 triangles each)
    ///     [0, 1, 4], [0, 4, 3],
    ///     [1, 2, 5], [1, 5, 4],
    ///     [2, 0, 3], [2, 3, 5],
    /// ];
    ///
    /// let prism = ConvexPolyhedron::from_convex_mesh(vertices, &indices)
    ///     .expect("Failed to create prism");
    ///
    /// assert_eq!(prism.points().len(), 6);
    /// // 2 triangular faces + 3 rectangular faces = 5 faces
    /// assert_eq!(prism.faces().len(), 5);
    /// # }
    /// ```
    ///
    /// [`from_convex_hull`]: ConvexPolyhedron::from_convex_hull
    pub fn from_convex_mesh(
        points: Vec<Vector>,
        indices: &[[u32; DIM]],
    ) -> Option<ConvexPolyhedron> {
        let eps = crate::math::DEFAULT_EPSILON.sqrt();

        let mut vertices = Vec::new();
        let mut edges = Vec::<Edge>::new();
        let mut faces = Vec::<Face>::new();
        let mut triangles = Vec::new();
        let mut edge_map = HashMap::default();

        let mut faces_adj_to_vertex = Vec::new();
        let mut edges_adj_to_vertex = Vec::new();
        let mut edges_adj_to_face = Vec::new();
        let mut vertices_adj_to_face = Vec::new();

        if points.len() + indices.len() <= 2 {
            return None;
        }

        //// Euler characteristic.
        let nedges = points.len() + indices.len() - 2;
        edges.reserve(nedges);

        /*
         *  Initialize triangles and edges adjacency information.
         */
        for idx in indices {
            let mut edges_id = [u32::MAX; DIM];
            let face_id = triangles.len();

            if idx[0] == idx[1] || idx[0] == idx[2] || idx[1] == idx[2] {
                return None;
            }

            for i1 in 0..3 {
                // Deal with edges.
                let i2 = (i1 + 1) % 3;
                let key = SortedPair::new(idx[i1], idx[i2]);

                match edge_map.entry(key) {
                    Entry::Occupied(e) => {
                        let edge = &mut edges[*e.get() as usize];
                        let out_face_id = &mut edge.faces[1];

                        if *out_face_id == u32::MAX {
                            edges_id[i1] = *e.get();
                            *out_face_id = face_id as u32
                        } else {
                            // We have a t-junction.
                            return None;
                        }
                    }
                    Entry::Vacant(e) => {
                        edges_id[i1] = *e.insert(edges.len() as u32);

                        let dir =
                            (points[idx[i2] as usize] - points[idx[i1] as usize]).try_normalize();

                        edges.push(Edge {
                            vertices: [idx[i1], idx[i2]],
                            faces: [face_id as u32, u32::MAX],
                            dir: dir.unwrap_or(Vector::X),
                            deleted: dir.is_none(),
                        });
                    }
                }
            }

            let normal = utils::ccw_face_normal([
                points[idx[0] as usize],
                points[idx[1] as usize],
                points[idx[2] as usize],
            ]);
            let triangle = Triangle {
                vertices: *idx,
                edges: edges_id,
                normal: normal.unwrap_or(Vector::ZERO),
                parent_face: None,
                is_degenerate: normal.is_none(),
            };

            triangles.push(triangle);
        }

        // Find edges that must be deleted.

        for e in &mut edges {
            let tri1 = triangles.get(e.faces[0] as usize)?;
            let tri2 = triangles.get(e.faces[1] as usize)?;
            if tri1.normal.dot(tri2.normal) > 1.0 - eps {
                e.deleted = true;
            }
        }

        /*
         * Extract faces by following  contours.
         */
        for i in 0..triangles.len() {
            if triangles[i].parent_face.is_none() {
                for j1 in 0..3 {
                    if !edges[triangles[i].edges[j1] as usize].deleted {
                        // Create a new face, setup its first edge/vertex and construct it.
                        let new_face_id = faces.len();
                        let mut new_face = Face {
                            first_vertex_or_edge: edges_adj_to_face.len() as u32,
                            num_vertices_or_edges: 1,
                            normal: triangles[i].normal,
                        };

                        edges_adj_to_face.push(triangles[i].edges[j1]);
                        vertices_adj_to_face.push(triangles[i].vertices[j1]);

                        let j2 = (j1 + 1) % 3;
                        let start_vertex = triangles[i].vertices[j1];

                        // NOTE: variables ending with _id are identifier on the
                        // fields of a triangle. Other variables are identifier on
                        // the triangles/edges/vertices arrays.
                        let mut curr_triangle = i;
                        let mut curr_edge_id = j2;

                        while triangles[curr_triangle].vertices[curr_edge_id] != start_vertex {
                            let curr_edge = triangles[curr_triangle].edges[curr_edge_id];
                            let curr_vertex = triangles[curr_triangle].vertices[curr_edge_id];
                            // NOTE: we should use this assertion. However, it can currently
                            // happen if there are some isolated non-deleted edges due to
                            // rounding errors.
                            //
                            // assert!(triangles[curr_triangle].parent_face.is_none());
                            triangles[curr_triangle].parent_face = Some(new_face_id as u32);

                            if !edges[curr_edge as usize].deleted {
                                edges_adj_to_face.push(curr_edge);
                                vertices_adj_to_face.push(curr_vertex);
                                new_face.num_vertices_or_edges += 1;

                                curr_edge_id = (curr_edge_id + 1) % 3;
                            } else {
                                // Find adjacent edge on the next triangle.
                                curr_triangle = edges[curr_edge as usize]
                                    .other_triangle(curr_triangle as u32)
                                    as usize;
                                curr_edge_id =
                                    triangles[curr_triangle].next_edge_id(curr_edge) as usize;
                                assert!(
                                    triangles[curr_triangle].vertices[curr_edge_id] == curr_vertex
                                );
                            }
                        }

                        if new_face.num_vertices_or_edges > 2 {
                            // Sometimes degenerate faces may be generated
                            // due to numerical errors resulting in an isolated
                            // edge not being deleted.
                            //
                            // This kind of degenerate faces are not valid.
                            faces.push(new_face);
                        }
                        break;
                    }
                }
            }
        }

        // Update face ids inside edges so that they point to the faces instead of the triangles.
        for e in &mut edges {
            if let Some(fid) = triangles.get(e.faces[0] as usize)?.parent_face {
                e.faces[0] = fid;
            }

            if let Some(fid) = triangles.get(e.faces[1] as usize)?.parent_face {
                e.faces[1] = fid;
            }
        }

        /*
         * Initialize vertices
         */
        let empty_vertex = Vertex {
            first_adj_face_or_edge: 0,
            num_adj_faces_or_edge: 0,
        };

        vertices.resize(points.len(), empty_vertex);

        // First, find their multiplicities.
        for face in &faces {
            let first_vid = face.first_vertex_or_edge;
            let last_vid = face.first_vertex_or_edge + face.num_vertices_or_edges;

            for i in &vertices_adj_to_face[first_vid as usize..last_vid as usize] {
                vertices[*i as usize].num_adj_faces_or_edge += 1;
            }
        }

        // Now, find their starting id.
        let mut total_num_adj_faces = 0;
        for v in &mut vertices {
            v.first_adj_face_or_edge = total_num_adj_faces;
            total_num_adj_faces += v.num_adj_faces_or_edge;
        }
        faces_adj_to_vertex.resize(total_num_adj_faces as usize, 0);
        edges_adj_to_vertex.resize(total_num_adj_faces as usize, 0);

        // Reset the number of adjacent faces.
        // It will be set again to the right value as
        // the adjacent face list is filled.
        for v in &mut vertices {
            v.num_adj_faces_or_edge = 0;
        }

        for (face_id, face) in faces.iter().enumerate() {
            let first_vid = face.first_vertex_or_edge;
            let last_vid = face.first_vertex_or_edge + face.num_vertices_or_edges;

            for vid in first_vid..last_vid {
                let v = &mut vertices[vertices_adj_to_face[vid as usize] as usize];
                faces_adj_to_vertex
                    [(v.first_adj_face_or_edge + v.num_adj_faces_or_edge) as usize] =
                    face_id as u32;
                edges_adj_to_vertex
                    [(v.first_adj_face_or_edge + v.num_adj_faces_or_edge) as usize] =
                    edges_adj_to_face[vid as usize];
                v.num_adj_faces_or_edge += 1;
            }
        }

        // Note numerical errors may throw off the Euler characteristic.
        // So we don't check it right now.

        let res = ConvexPolyhedron {
            points,
            vertices,
            faces,
            edges,
            faces_adj_to_vertex,
            edges_adj_to_vertex,
            edges_adj_to_face,
            vertices_adj_to_face,
        };

        // TODO: for debug.
        // res.check_geometry();

        Some(res)
    }

    /// Verify if this convex polyhedron is actually convex.
    #[inline]
    pub fn check_geometry(&self) {
        for face in &self.faces {
            let p0 =
                self.points[self.vertices_adj_to_face[face.first_vertex_or_edge as usize] as usize];

            for v in &self.points {
                assert!((v - p0).dot(face.normal) <= crate::math::DEFAULT_EPSILON);
            }
        }
    }

    /// Returns the 3D coordinates of all vertices in this convex polyhedron.
    ///
    /// Each point represents a corner of the polyhedron where three or more edges meet.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::ConvexPolyhedron;
    /// use parry3d::math::Vector;
    ///
    /// let points = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.5, 1.0, 0.0),
    ///     Vector::new(0.5, 0.5, 1.0),
    /// ];
    /// let indices = vec![[0u32, 1, 2], [0, 1, 3], [1, 2, 3], [2, 0, 3]];
    ///
    /// let tetrahedron = ConvexPolyhedron::from_convex_mesh(points, &indices).unwrap();
    ///
    /// assert_eq!(tetrahedron.points().len(), 4);
    /// assert_eq!(tetrahedron.points()[0], Vector::ZERO);
    /// # }
    /// ```
    #[inline]
    pub fn points(&self) -> &[Vector] {
        &self.points[..]
    }

    /// Returns the topology information for all vertices.
    ///
    /// Each `Vertex` contains indices into the adjacency arrays, telling you which
    /// faces and edges are connected to that vertex. This is useful for advanced
    /// topological queries and mesh processing algorithms.
    ///
    /// Most users will want [`points()`] instead to get the 3D coordinates.
    ///
    /// [`points()`]: ConvexPolyhedron::points
    #[inline]
    pub fn vertices(&self) -> &[Vertex] {
        &self.vertices[..]
    }

    /// Returns the topology information for all edges.
    ///
    /// Each `Edge` contains:
    /// - The two vertex indices it connects
    /// - The two face indices it borders
    /// - The edge direction as a unit vector
    ///
    /// This is useful for advanced geometric queries and rendering wireframes.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::ConvexPolyhedron;
    /// use parry3d::math::Vector;
    ///
    /// let points = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.5, 1.0, 0.0),
    ///     Vector::new(0.5, 0.5, 1.0),
    /// ];
    /// let indices = vec![[0u32, 1, 2], [0, 1, 3], [1, 2, 3], [2, 0, 3]];
    ///
    /// let tetrahedron = ConvexPolyhedron::from_convex_mesh(points, &indices).unwrap();
    ///
    /// // A tetrahedron has 6 edges (4 vertices choose 2)
    /// assert_eq!(tetrahedron.edges().len(), 6);
    ///
    /// // Each edge connects two vertices
    /// let edge = &tetrahedron.edges()[0];
    /// println!("Edge connects vertices {} and {}", edge.vertices[0], edge.vertices[1]);
    /// # }
    /// ```
    #[inline]
    pub fn edges(&self) -> &[Edge] {
        &self.edges[..]
    }

    /// Returns the topology information for all faces.
    ///
    /// Each `Face` contains:
    /// - Indices into the vertex and edge adjacency arrays
    /// - The number of vertices/edges in the face (faces can be triangles, quads, or larger polygons)
    /// - The outward-pointing unit normal vector
    ///
    /// Faces are polygonal and can have 3 or more vertices. Adjacent coplanar triangles
    /// are automatically merged into larger polygonal faces.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::ConvexPolyhedron;
    /// use parry3d::math::Vector;
    ///
    /// // Create a cube (8 vertices, 12 triangular input faces)
    /// let vertices = vec![
    ///     Vector::ZERO, Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(1.0, 1.0, 0.0), Vector::new(0.0, 1.0, 0.0),
    ///     Vector::new(0.0, 0.0, 1.0), Vector::new(1.0, 0.0, 1.0),
    ///     Vector::new(1.0, 1.0, 1.0), Vector::new(0.0, 1.0, 1.0),
    /// ];
    /// let indices = vec![
    ///     [0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7],
    ///     [0, 1, 5], [0, 5, 4], [2, 3, 7], [2, 7, 6],
    ///     [0, 4, 7], [0, 7, 3], [1, 2, 6], [1, 6, 5],
    /// ];
    ///
    /// let cube = ConvexPolyhedron::from_convex_mesh(vertices, &indices).unwrap();
    ///
    /// // The 12 triangles are merged into 6 square faces
    /// assert_eq!(cube.faces().len(), 6);
    ///
    /// // Each face has 4 vertices (it's a square)
    /// assert_eq!(cube.faces()[0].num_vertices_or_edges, 4);
    /// # }
    /// ```
    #[inline]
    pub fn faces(&self) -> &[Face] {
        &self.faces[..]
    }

    /// The array containing the indices of the vertices adjacent to each face.
    #[inline]
    pub fn vertices_adj_to_face(&self) -> &[u32] {
        &self.vertices_adj_to_face[..]
    }

    /// The array containing the indices of the edges adjacent to each face.
    #[inline]
    pub fn edges_adj_to_face(&self) -> &[u32] {
        &self.edges_adj_to_face[..]
    }

    /// The array containing the indices of the faces adjacent to each vertex.
    #[inline]
    pub fn faces_adj_to_vertex(&self) -> &[u32] {
        &self.faces_adj_to_vertex[..]
    }

    /// Computes a scaled version of this convex polyhedron.
    ///
    /// This method scales the polyhedron by multiplying each vertex coordinate by the corresponding
    /// component of the `scale` vector. This allows for non-uniform scaling (different scale factors
    /// for x, y, and z axes).
    ///
    /// The face normals and edge directions are also updated to reflect the scaling transformation.
    ///
    /// # Returns
    ///
    /// - `Some(ConvexPolyhedron)` with the scaled shape
    /// - `None` if the scaling results in degenerate normals (e.g., if the scale factor along
    ///   one axis is zero or nearly zero)
    ///
    /// # Example: Uniform scaling
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::ConvexPolyhedron;
    /// use parry3d::math::Vector;
    ///
    /// let points = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.5, 1.0, 0.0),
    ///     Vector::new(0.5, 0.5, 1.0),
    /// ];
    /// let indices = vec![[0u32, 1, 2], [0, 1, 3], [1, 2, 3], [2, 0, 3]];
    ///
    /// let tetrahedron = ConvexPolyhedron::from_convex_mesh(points, &indices).unwrap();
    ///
    /// // Scale uniformly by 2x
    /// let scaled = tetrahedron.scaled(Vector::new(2.0, 2.0, 2.0))
    ///     .expect("Failed to scale");
    ///
    /// // All coordinates are doubled
    /// assert_eq!(scaled.points()[1], Vector::new(2.0, 0.0, 0.0));
    /// assert_eq!(scaled.points()[3], Vector::new(1.0, 1.0, 2.0));
    /// # }
    /// ```
    ///
    /// # Example: Non-uniform scaling to create a rectangular prism
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::ConvexPolyhedron;
    /// use parry3d::math::Vector;
    ///
    /// // Start with a unit cube
    /// let vertices = vec![
    ///     Vector::ZERO, Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(1.0, 1.0, 0.0), Vector::new(0.0, 1.0, 0.0),
    ///     Vector::new(0.0, 0.0, 1.0), Vector::new(1.0, 0.0, 1.0),
    ///     Vector::new(1.0, 1.0, 1.0), Vector::new(0.0, 1.0, 1.0),
    /// ];
    /// let indices = vec![
    ///     [0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7],
    ///     [0, 1, 5], [0, 5, 4], [2, 3, 7], [2, 7, 6],
    ///     [0, 4, 7], [0, 7, 3], [1, 2, 6], [1, 6, 5],
    /// ];
    ///
    /// let cube = ConvexPolyhedron::from_convex_mesh(vertices, &indices).unwrap();
    ///
    /// // Scale to make it wider (3x), deeper (2x), and taller (4x)
    /// let box_shape = cube.scaled(Vector::new(3.0, 2.0, 4.0))
    ///     .expect("Failed to scale");
    ///
    /// assert_eq!(box_shape.points()[6], Vector::new(3.0, 2.0, 4.0));
    /// # }
    /// ```
    pub fn scaled(mut self, scale: Vector) -> Option<Self> {
        self.points.iter_mut().for_each(|pt| *pt *= scale);

        for f in &mut self.faces {
            f.normal = (f.normal * scale).try_normalize().unwrap_or(f.normal);
        }

        for e in &mut self.edges {
            e.dir = (e.dir * scale).try_normalize().unwrap_or(e.dir);
        }

        Some(self)
    }

    fn support_feature_id_toward_eps(&self, local_dir: Vector, eps: Real) -> FeatureId {
        let (seps, ceps) = eps.sin_cos();
        let support_pt_id = utils::point_cloud_support_point_id(local_dir, &self.points);
        let vertex = &self.vertices[support_pt_id];

        // Check faces.
        for i in 0..vertex.num_adj_faces_or_edge {
            let face_id = self.faces_adj_to_vertex[(vertex.first_adj_face_or_edge + i) as usize];
            let face = &self.faces[face_id as usize];

            if face.normal.dot(local_dir) >= ceps {
                return FeatureId::Face(face_id);
            }
        }

        // Check edges.
        for i in 0..vertex.num_adj_faces_or_edge {
            let edge_id = self.edges_adj_to_vertex[(vertex.first_adj_face_or_edge + i) as usize];
            let edge = &self.edges[edge_id as usize];

            if edge.dir.dot(local_dir).abs() <= seps {
                return FeatureId::Edge(edge_id);
            }
        }

        // The vertex is the support feature.
        FeatureId::Vertex(support_pt_id as u32)
    }

    /// Computes the ID of the features with a normal that maximize the dot-product with `local_dir`.
    pub fn support_feature_id_toward(&self, local_dir: Vector) -> FeatureId {
        #[cfg_attr(feature = "f64", expect(clippy::unnecessary_cast))]
        let eps: Real = (f64::consts::PI / 180.0) as Real;
        self.support_feature_id_toward_eps(local_dir, eps)
    }

    /// The normal of the given feature.
    pub fn feature_normal(&self, feature: FeatureId) -> Option<Vector> {
        match feature {
            FeatureId::Face(id) => Some(self.faces[id as usize].normal),
            FeatureId::Edge(id) => {
                let edge = &self.edges[id as usize];
                Some(
                    (self.faces[edge.faces[0] as usize].normal
                        + self.faces[edge.faces[1] as usize].normal)
                        .normalize(),
                )
            }
            FeatureId::Vertex(id) => {
                let vertex = &self.vertices[id as usize];
                let first = vertex.first_adj_face_or_edge;
                let last = vertex.first_adj_face_or_edge + vertex.num_adj_faces_or_edge;
                let mut normal = Vector::ZERO;

                for face in &self.faces_adj_to_vertex[first as usize..last as usize] {
                    normal += self.faces[*face as usize].normal
                }

                Some((normal).normalize())
            }
            FeatureId::Unknown => None,
        }
    }
}

impl SupportMap for ConvexPolyhedron {
    #[inline]
    fn local_support_point(&self, dir: Vector) -> Vector {
        utils::point_cloud_support_point(dir, self.points())
    }
}

impl PolygonalFeatureMap for ConvexPolyhedron {
    fn local_support_feature(&self, dir: Vector, out_feature: &mut PolygonalFeature) {
        let mut best_fid = 0;
        let mut best_dot = self.faces[0].normal.dot(dir);

        for (fid, face) in self.faces[1..].iter().enumerate() {
            let new_dot = face.normal.dot(dir);

            if new_dot > best_dot {
                best_fid = fid + 1;
                best_dot = new_dot;
            }
        }

        let face = &self.faces[best_fid];
        let i1 = face.first_vertex_or_edge;
        // TODO: if there are more than 4 vertices, we need to select four vertices that maximize the area.
        let num_vertices = face.num_vertices_or_edges.min(4);
        let i2 = i1 + num_vertices;

        for (i, (vid, eid)) in self.vertices_adj_to_face[i1 as usize..i2 as usize]
            .iter()
            .zip(self.edges_adj_to_face[i1 as usize..i2 as usize].iter())
            .enumerate()
        {
            out_feature.vertices[i] = self.points[*vid as usize];
            out_feature.vids[i] = PackedFeatureId::vertex(*vid);
            out_feature.eids[i] = PackedFeatureId::edge(*eid);
        }

        out_feature.fid = PackedFeatureId::face(best_fid as u32);
        out_feature.num_vertices = num_vertices as usize;
    }

    fn is_convex_polyhedron(&self) -> bool {
        true
    }
}

/*
impl ConvexPolyhedron for ConvexPolyhedron {
    fn vertex(&self, id: FeatureId) -> Vector {
        self.points[id.unwrap_vertex() as usize]
    }

    fn edge(&self, id: FeatureId) -> (Vector, Vector, FeatureId, FeatureId) {
        let edge = &self.edges[id.unwrap_edge() as usize];
        let v1 = edge.vertices[0];
        let v2 = edge.vertices[1];

        (
            self.points[v1 as usize],
            self.points[v2 as usize],
            FeatureId::Vertex(v1),
            FeatureId::Vertex(v2),
        )
    }

    fn face(&self, id: FeatureId, out: &mut ConvexPolygonalFeature) {
        out.clear();

        let face = &self.faces[id.unwrap_face() as usize];
        let first_vertex = face.first_vertex_or_edge;
        let last_vertex = face.first_vertex_or_edge + face.num_vertices_or_edges;

        for i in first_vertex..last_vertex {
            let vid = self.vertices_adj_to_face[i];
            let eid = self.edges_adj_to_face[i];
            out.push(self.points[vid], FeatureId::Vertex(vid));
            out.push_edge_feature_id(FeatureId::Edge(eid));
        }

        out.set_normal(face.normal);
        out.set_feature_id(id);
        out.recompute_edge_normals();
    }

    fn support_face_toward(
        &self,
        m: &Pose,
        dir: Vector,
        out: &mut ConvexPolygonalFeature,
    ) {
        let ls_dir = m.inverse_transform_vector(dir);
        let mut best_face = 0;
        let mut max_dot = self.faces[0].normal.dot(ls_dir);

        for i in 1..self.faces.len() {
            let face = &self.faces[i];
            let dot = face.normal.dot(ls_dir);

            if dot > max_dot {
                max_dot = dot;
                best_face = i;
            }
        }

        self.face(FeatureId::Face(best_face), out);
        out.transform_by(m);
    }

    fn support_feature_toward(
        &self,
        transform: &Pose,
        dir: Vector,
        angle: Real,
        out: &mut ConvexPolygonalFeature,
    ) {
        out.clear();
        let local_dir = transform.inverse_transform_unit_vector(dir);
        let fid = self.support_feature_id_toward_eps(&local_dir, angle);

        match fid {
            FeatureId::Vertex(_) => {
                let v = self.vertex(fid);
                out.push(v, fid);
                out.set_feature_id(fid);
            }
            FeatureId::Edge(_) => {
                let edge = self.edge(fid);
                out.push(edge.0, edge.2);
                out.push(edge.1, edge.3);
                out.set_feature_id(fid);
                out.push_edge_feature_id(fid);
            }
            FeatureId::Face(_) => self.face(fid, out),
            FeatureId::Unknown => unreachable!(),
        }

        out.transform_by(transform);
    }
}
*/

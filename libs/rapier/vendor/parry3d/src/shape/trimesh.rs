use crate::bounding_volume::Aabb;
#[cfg(feature = "dim3")]
use crate::math::VectorExt;
use crate::math::{Pose, Vector};
use crate::partitioning::{Bvh, BvhBuildStrategy};
use crate::shape::{FeatureId, Shape, Triangle, TrianglePseudoNormals, TypedCompositeShape};
use crate::utils::HashablePartialEq;
use alloc::{vec, vec::Vec};
use core::fmt;
#[cfg(feature = "dim3")]
use {crate::shape::Cuboid, crate::utils::SortedPair};

use {
    crate::shape::composite_shape::CompositeShape,
    crate::utils::hashmap::{Entry, HashMap},
    crate::utils::hashset::HashSet,
};

#[cfg(feature = "dim2")]
use crate::transformation::ear_clipping::triangulate_ear_clipping;

use crate::query::details::NormalConstraints;

/// Errors that occur when computing or validating triangle mesh topology.
///
/// Triangle mesh topology describes the connectivity and adjacency relationships between
/// vertices, edges, and triangles. When constructing a mesh with [`TriMeshFlags::HALF_EDGE_TOPOLOGY`],
/// Parry validates these relationships and returns this error if inconsistencies are found.
///
/// # When This Occurs
///
/// Topology errors typically occur when:
/// - The mesh is non-manifold (edges with more than 2 adjacent faces)
/// - Adjacent triangles have inconsistent winding order
/// - Triangles have degenerate geometry (duplicate vertices)
///
/// [`TriMeshFlags::HALF_EDGE_TOPOLOGY`]: crate::shape::TriMeshFlags::HALF_EDGE_TOPOLOGY
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TopologyError {
    /// A triangle has two or three identical vertices (degenerate triangle).
    ///
    /// This error indicates that a triangle in the mesh references the same vertex
    /// multiple times, creating a degenerate triangle (zero area). For example,
    /// a triangle with indices `[5, 5, 7]` or `[1, 2, 1]`.
    ///
    /// Degenerate triangles cannot be part of a valid mesh topology because they
    /// don't have proper edges or face normals.
    ///
    //    /// TODO: figure out why this doc-test fails.
    //    /// # How to Fix
    //    ///
    //    /// ```
    //    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    //    /// # use parry3d::shape::{TriMesh, TriMeshFlags};
    //    /// # use parry3d::math::Vector;
    //    /// let vertices = vec![
    //    ///     Vector::ZERO,
    //    ///     Vector::new(1.0, 0.0, 0.0),
    //    ///     Vector::new(0.0, 1.0, 0.0),
    //    /// ];
    //    ///
    //    /// // BAD: Triangle with duplicate vertices
    //    /// let bad_indices = vec![[0, 0, 1]];  // vertex 0 appears twice!
    //    ///
    //    /// let result = TriMesh::with_flags(
    //    ///     vertices.clone(),
    //    ///     bad_indices,
    //    ///     TriMeshFlags::HALF_EDGE_TOPOLOGY
    //    /// );
    //    /// assert!(result.is_err());
    //    ///
    //    /// // GOOD: All three vertices are distinct
    //    /// let good_indices = vec![[0, 1, 2]];
    //    /// let mesh = TriMesh::with_flags(
    //    ///     vertices,
    //    ///     good_indices,
    //    ///     TriMeshFlags::HALF_EDGE_TOPOLOGY
    //    /// ).expect("Valid mesh");
    //    /// # }
    //    /// ```
    ///
    /// Alternatively, use [`TriMeshFlags::DELETE_DEGENERATE_TRIANGLES`] to automatically
    /// remove degenerate triangles:
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::{TriMesh, TriMeshFlags};
    /// # use parry3d::math::Vector;
    /// # let vertices = vec![Vector::ZERO, Vector::new(1.0, 0.0, 0.0), Vector::new(0.0, 1.0, 0.0)];
    /// # let indices = vec![[0, 0, 1], [0, 1, 2]];
    /// let flags = TriMeshFlags::HALF_EDGE_TOPOLOGY
    ///     | TriMeshFlags::DELETE_BAD_TOPOLOGY_TRIANGLES;
    ///
    /// let mesh = TriMesh::with_flags(vertices, indices, flags)
    ///     .expect("Bad triangles removed");
    /// # }
    /// ```
    BadTriangle(u32),

    /// Two adjacent triangles have opposite orientations (inconsistent winding).
    ///
    /// For a manifold mesh with consistent normals, adjacent triangles must have
    /// compatible orientations. If two triangles share an edge, they must traverse
    /// that edge in opposite directions.
    ///
    /// This error reports:
    /// - `triangle1`, `triangle2`: The indices of the two conflicting triangles
    /// - `edge`: The shared edge as a pair of vertex indices `(v1, v2)`
    ///
    /// # Example of the Problem
    ///
    /// ```text
    /// CORRECT (opposite winding on shared edge):
    ///   Triangle 1: [a, b, c]  ->  edge (a,b)
    ///   Triangle 2: [b, a, d]  ->  edge (b,a)  ✓ opposite direction
    ///
    /// INCORRECT (same winding on shared edge):
    ///   Triangle 1: [a, b, c]  ->  edge (a,b)
    ///   Triangle 2: [a, b, d]  ->  edge (a,b)  ✗ same direction!
    /// ```
    ///
    //    /// TODO: figure out why this doc test fails?
    //    /// # How to Fix
    //    ///
    //    /// You need to reverse the winding order of one of the triangles:
    //    ///
    //    /// ```
    //    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    //    /// # use parry3d::shape::{TriMesh, TriMeshFlags};
    //    /// # use parry3d::math::Vector;
    //    /// let vertices = vec![
    //    ///     Vector::ZERO,  // vertex 0
    //    ///     Vector::new(1.0, 0.0, 0.0),  // vertex 1
    //    ///     Vector::new(0.5, 1.0, 0.0),  // vertex 2
    //    ///     Vector::new(0.5, -1.0, 0.0), // vertex 3
    //    /// ];
    //    ///
    //    /// // BAD: Both triangles traverse edge (0,1) in the same direction
    //    /// let bad_indices = vec![
    //    ///     [0, 1, 2],  // edge (0,1)
    //    ///     [0, 1, 3],  // edge (0,1) - same direction!
    //    /// ];
    //    ///
    //    /// let result = TriMesh::with_flags(
    //    ///     vertices.clone(),
    //    ///     bad_indices,
    //    ///     TriMeshFlags::HALF_EDGE_TOPOLOGY
    //    /// );
    //    /// assert!(result.is_err());
    //    ///
    //    /// // GOOD: Second triangle reversed to [1, 0, 3]
    //    /// let good_indices = vec![
    //    ///     [0, 1, 2],  // edge (0,1)
    //    ///     [1, 0, 3],  // edge (1,0) - opposite direction!
    //    /// ];
    //    ///
    //    /// let mesh = TriMesh::with_flags(
    //    ///     vertices,
    //    ///     good_indices,
    //    ///     TriMeshFlags::HALF_EDGE_TOPOLOGY
    //    /// ).expect("Valid mesh");
    //    /// # }
    //    /// ```
    ///
    /// # Common Causes
    ///
    /// - Mixing clockwise and counter-clockwise triangle definitions
    /// - Incorrectly constructed mesh from modeling software
    /// - Manual mesh construction with inconsistent winding
    /// - Merging separate meshes with different conventions
    BadAdjacentTrianglesOrientation {
        /// The first triangle, with an orientation opposite to the second triangle.
        triangle1: u32,
        /// The second triangle, with an orientation opposite to the first triangle.
        triangle2: u32,
        /// The edge shared between the two triangles.
        edge: (u32, u32),
    },
}

impl fmt::Display for TopologyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadTriangle(triangle) => {
                write!(f, "the triangle {triangle} has at least two identical vertices.")
            }
            Self::BadAdjacentTrianglesOrientation {
                triangle1,
                triangle2,
                edge,
            } => write!(
                f,
                "the triangles {triangle1} and {triangle2} sharing the edge {edge:?} have opposite orientations."
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for TopologyError {}

/// Errors that occur when creating a triangle mesh.
///
/// When constructing a [`TriMesh`] using [`TriMesh::new`] or [`TriMesh::with_flags`],
/// various validation checks are performed. This error type describes what went wrong.
///
/// # Common Usage
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// # use parry3d::shape::{TriMesh, TriMeshBuilderError, TopologyError};
/// # use parry3d::math::Vector;
/// let vertices = vec![Vector::ZERO];
/// let indices = vec![];  // Empty!
///
/// match TriMesh::new(vertices, indices) {
///     Err(TriMeshBuilderError::EmptyIndices) => {
///         println!("Cannot create a mesh with no triangles");
///     }
///     Err(TriMeshBuilderError::TopologyError(topo_err)) => {
///         println!("Mesh topology is invalid: {}", topo_err);
///     }
///     Ok(mesh) => {
///         println!("Mesh created successfully");
///     }
/// }
/// # }
/// ```
///
/// [`TriMesh`]: crate::shape::TriMesh
/// [`TriMesh::new`]: crate::shape::TriMesh::new
/// [`TriMesh::with_flags`]: crate::shape::TriMesh::with_flags
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TriMeshBuilderError {
    /// The index buffer is empty (no triangles provided).
    ///
    /// A triangle mesh must contain at least one triangle. An empty index buffer
    /// is not valid because there's nothing to render or use for collision detection.
    ///
    /// # How to Fix
    ///
    /// Ensure your index buffer has at least one triangle:
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::TriMesh;
    /// # use parry3d::math::Vector;
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    /// ];
    ///
    /// // BAD: No triangles
    /// let empty_indices = vec![];
    /// assert!(TriMesh::new(vertices.clone(), empty_indices).is_err());
    ///
    /// // GOOD: At least one triangle
    /// let indices = vec![[0, 1, 2]];
    /// assert!(TriMesh::new(vertices, indices).is_ok());
    /// # }
    /// ```
    EmptyIndices,

    /// The mesh topology is invalid.
    ///
    /// This wraps a [`TopologyError`] that provides details about the specific
    /// topology problem. This only occurs when creating a mesh with topology
    /// validation enabled (e.g., [`TriMeshFlags::HALF_EDGE_TOPOLOGY`]).
    ///
    /// See [`TopologyError`] for details on specific topology problems and how
    /// to fix them.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::{TriMesh, TriMeshFlags, TriMeshBuilderError};
    /// # use parry3d::math::Vector;
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    /// ];
    ///
    /// // Triangle with duplicate vertices
    /// let bad_indices = vec![[0, 0, 1]];
    ///
    /// match TriMesh::with_flags(vertices, bad_indices, TriMeshFlags::HALF_EDGE_TOPOLOGY) {
    ///     Err(TriMeshBuilderError::TopologyError(topo_err)) => {
    ///         println!("Topology error: {}", topo_err);
    ///         // Handle the specific topology issue
    ///     }
    ///     _ => {}
    /// }
    /// # }
    /// ```
    ///
    /// [`TriMeshFlags::HALF_EDGE_TOPOLOGY`]: crate::shape::TriMeshFlags::HALF_EDGE_TOPOLOGY
    TopologyError(TopologyError),
}

impl fmt::Display for TriMeshBuilderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyIndices => {
                f.write_str("A triangle mesh must contain at least one triangle.")
            }
            Self::TopologyError(err) => write!(f, "Topology Error: {err}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for TriMeshBuilderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::EmptyIndices => None,
            Self::TopologyError(err) => Some(err),
        }
    }
}

/// The set of pseudo-normals of a triangle mesh.
///
/// These pseudo-normals are used for the inside-outside test of a
/// point on the triangle, as described in the paper:
/// "Signed distance computation using the angle weighted pseudonormal", Baerentzen, et al.
/// DOI: 10.1109/TVCG.2005.49
#[derive(Default, Clone)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[repr(C)]
#[cfg(feature = "dim3")]
pub struct TriMeshPseudoNormals {
    /// The pseudo-normals of the vertices.
    pub vertices_pseudo_normal: Vec<Vector>,
    /// The pseudo-normals of the edges.
    pub edges_pseudo_normal: Vec<[Vector; 3]>,
}

/// The connected-components of a triangle mesh.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[repr(C)]
pub struct TriMeshConnectedComponents {
    /// The `face_colors[i]` gives the connected-component index
    /// of the i-th face.
    pub face_colors: Vec<u32>,
    /// The set of faces grouped by connected components.
    pub grouped_faces: Vec<u32>,
    /// The range of connected components. `self.grouped_faces[self.ranges[i]..self.ranges[i + 1]]`
    /// contains the indices of all the faces part of the i-th connected component.
    pub ranges: Vec<usize>,
}

impl TriMeshConnectedComponents {
    /// The total number of connected components.
    pub fn num_connected_components(&self) -> usize {
        self.ranges.len() - 1
    }

    /// Convert the connected-component description into actual meshes (returned as raw index and
    /// vertex buffers).
    ///
    /// The `mesh` must be the one used to generate `self`, otherwise it might panic or produce an
    /// unexpected result.
    pub fn to_mesh_buffers(&self, mesh: &TriMesh) -> Vec<(Vec<Vector>, Vec<[u32; 3]>)> {
        let mut result = vec![];
        let mut new_vtx_index: Vec<_> = vec![u32::MAX; mesh.vertices.len()];

        for ranges in self.ranges.windows(2) {
            let num_faces = ranges[1] - ranges[0];

            if num_faces == 0 {
                continue;
            }

            let mut vertices = Vec::with_capacity(num_faces);
            let mut indices = Vec::with_capacity(num_faces);

            for fid in ranges[0]..ranges[1] {
                let vids = mesh.indices[self.grouped_faces[fid] as usize];
                let new_vids = vids.map(|id| {
                    if new_vtx_index[id as usize] == u32::MAX {
                        vertices.push(mesh.vertices[id as usize]);
                        new_vtx_index[id as usize] = vertices.len() as u32 - 1;
                    }

                    new_vtx_index[id as usize]
                });
                indices.push(new_vids);
            }

            result.push((vertices, indices));
        }

        result
    }

    /// Convert the connected-component description into actual meshes.
    ///
    /// The `mesh` must be the one used to generate `self`, otherwise it might panic or produce an
    /// unexpected result.
    ///
    /// All the meshes are constructed with the given `flags`.
    pub fn to_meshes(
        &self,
        mesh: &TriMesh,
        flags: TriMeshFlags,
    ) -> Vec<Result<TriMesh, TriMeshBuilderError>> {
        self.to_mesh_buffers(mesh)
            .into_iter()
            .map(|(vtx, idx)| TriMesh::with_flags(vtx, idx, flags))
            .collect()
    }
}

/// A vertex of a triangle-mesh’s half-edge topology.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[repr(C)]
pub struct TopoVertex {
    /// One of the half-edge with this vertex as endpoint.
    pub half_edge: u32,
}

/// A face of a triangle-mesh’s half-edge topology.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[repr(C)]
pub struct TopoFace {
    /// The half-edge adjacent to this face, with a starting point equal
    /// to the first point of this face.
    pub half_edge: u32,
}

/// A half-edge of a triangle-mesh’s half-edge topology.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[repr(C)]
pub struct TopoHalfEdge {
    /// The next half-edge.
    pub next: u32,
    /// This half-edge twin on the adjacent triangle.
    ///
    /// This is `u32::MAX` if there is no twin.
    pub twin: u32,
    /// The first vertex of this edge.
    pub vertex: u32,
    /// The face associated to this half-edge.
    pub face: u32,
}

/// The half-edge topology information of a triangle mesh.
#[derive(Default, Clone)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[repr(C)]
pub struct TriMeshTopology {
    /// The vertices of this half-edge representation.
    pub vertices: Vec<TopoVertex>,
    /// The faces of this half-edge representation.
    pub faces: Vec<TopoFace>,
    /// The half-edges of this half-edge representation.
    pub half_edges: Vec<TopoHalfEdge>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
/// Controls how a [`TriMesh`] should be loaded.
pub struct TriMeshFlags(u16);

bitflags::bitflags! {
    impl TriMeshFlags: u16 {
        /// If set, the half-edge topology of the trimesh will be computed if possible.
        const HALF_EDGE_TOPOLOGY = 1;
        /// If set, the connected components of the trimesh will be computed.
        const CONNECTED_COMPONENTS = 1 << 1;
        /// If set, any triangle that results in a failing half-hedge topology computation will be deleted.
        const DELETE_BAD_TOPOLOGY_TRIANGLES = 1 << 2;
        /// If set, the trimesh will be assumed to be oriented (with outward normals).
        ///
        /// The pseudo-normals of its vertices and edges will be computed.
        const ORIENTED = 1 << 3;
        /// If set, the duplicate vertices of the trimesh will be merged.
        ///
        /// Two vertices with the exact same coordinates will share the same entry on the
        /// vertex buffer and the index buffer is adjusted accordingly.
        const MERGE_DUPLICATE_VERTICES = 1 << 4;
        /// If set, the triangles sharing two vertices with identical index values will be removed.
        ///
        /// Because of the way it is currently implemented, this methods implies that duplicate
        /// vertices will be merged. It will no longer be the case in the future once we decouple
        /// the computations.
        const DELETE_DEGENERATE_TRIANGLES = 1 << 5;
        /// If set, two triangles sharing three vertices with identical index values (in any order)
        /// will be removed.
        ///
        /// Because of the way it is currently implemented, this methods implies that duplicate
        /// vertices will be merged. It will no longer be the case in the future once we decouple
        /// the computations.
        const DELETE_DUPLICATE_TRIANGLES = 1 << 6;
        /// If set, a special treatment will be applied to contact manifold calculation to eliminate
        /// or fix contacts normals that could lead to incorrect bumps in physics simulation
        /// (especially on flat surfaces).
        ///
        /// This is achieved by taking into account adjacent triangle normals when computing contact
        /// points for a given triangle.
        const FIX_INTERNAL_EDGES = (1 << 7) | Self::MERGE_DUPLICATE_VERTICES.bits();
    }
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[repr(C)]
#[derive(Clone)]
/// A triangle mesh.
pub struct TriMesh {
    bvh: Bvh,
    vertices: Vec<Vector>,
    indices: Vec<[u32; 3]>,
    #[cfg(feature = "dim3")]
    pub(crate) pseudo_normals: Option<TriMeshPseudoNormals>,
    topology: Option<TriMeshTopology>,
    connected_components: Option<TriMeshConnectedComponents>,
    flags: TriMeshFlags,
}

impl fmt::Debug for TriMesh {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GenericTriMesh")
    }
}

impl TriMesh {
    /// Creates a new triangle mesh from a vertex buffer and an index buffer.
    ///
    /// This is the most common way to construct a `TriMesh`. The mesh is created with
    /// default settings (no topology computation, no pseudo-normals, etc.). For more
    /// control over these optional features, use [`TriMesh::with_flags`] instead.
    ///
    /// # Arguments
    ///
    /// * `vertices` - A vector of 3D points representing the mesh vertices
    /// * `indices` - A vector of triangles, where each triangle is represented by
    ///   three indices into the vertex buffer. Indices should be in counter-clockwise
    ///   order for outward-facing normals.
    ///
    /// # Returns
    ///
    /// * `Ok(TriMesh)` - If the mesh was successfully created
    /// * `Err(TriMeshBuilderError)` - If the mesh is invalid (e.g., empty index buffer)
    ///
    /// # Errors
    ///
    /// This function returns an error if:
    /// - The index buffer is empty (at least one triangle is required)
    ///
    /// # Performance
    ///
    /// This function builds a BVH (Bounding Volume Hierarchy) acceleration structure
    /// for the mesh, which takes O(n log n) time where n is the number of triangles.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::TriMesh;
    /// use parry3d::math::Vector;
    ///
    /// // Create a simple triangle mesh (a single triangle)
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    /// ];
    /// let indices = vec![[0, 1, 2]];
    ///
    /// let trimesh = TriMesh::new(vertices, indices).expect("Invalid mesh");
    /// assert_eq!(trimesh.num_triangles(), 1);
    /// # }
    /// ```
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::TriMesh;
    /// use parry2d::math::Vector;
    ///
    /// // Create a quad (two triangles) in 2D
    /// let vertices = vec![
    ///     Vector::ZERO,  // bottom-left
    ///     Vector::new(1.0, 0.0),  // bottom-right
    ///     Vector::new(1.0, 1.0),  // top-right
    ///     Vector::new(0.0, 1.0),  // top-left
    /// ];
    /// let indices = vec![
    ///     [0, 1, 2],  // first triangle
    ///     [0, 2, 3],  // second triangle
    /// ];
    ///
    /// let quad = TriMesh::new(vertices, indices).unwrap();
    /// assert_eq!(quad.num_triangles(), 2);
    /// assert_eq!(quad.vertices().len(), 4);
    /// # }
    /// ```
    pub fn new(vertices: Vec<Vector>, indices: Vec<[u32; 3]>) -> Result<Self, TriMeshBuilderError> {
        Self::with_flags(vertices, indices, TriMeshFlags::empty())
    }

    /// Creates a new triangle mesh from a vertex buffer and an index buffer, and flags controlling optional properties.
    ///
    /// This is the most flexible way to create a `TriMesh`, allowing you to specify exactly
    /// which optional features should be computed. Use this when you need:
    /// - Half-edge topology for adjacency information
    /// - Connected components analysis
    /// - Pseudo-normals for robust inside/outside tests
    /// - Automatic merging of duplicate vertices
    /// - Removal of degenerate or duplicate triangles
    ///
    /// # Arguments
    ///
    /// * `vertices` - A vector of 3D points representing the mesh vertices
    /// * `indices` - A vector of triangles, where each triangle is represented by
    ///   three indices into the vertex buffer
    /// * `flags` - A combination of [`TriMeshFlags`] controlling which optional features to compute
    ///
    /// # Returns
    ///
    /// * `Ok(TriMesh)` - If the mesh was successfully created
    /// * `Err(TriMeshBuilderError)` - If the mesh is invalid or topology computation failed
    ///
    /// # Errors
    ///
    /// This function returns an error if:
    /// - The index buffer is empty
    /// - [`TriMeshFlags::HALF_EDGE_TOPOLOGY`] is set but the topology is invalid
    ///   (e.g., non-manifold edges or inconsistent orientations)
    ///
    /// # Performance
    ///
    /// - Base construction: O(n log n) for BVH building
    /// - `HALF_EDGE_TOPOLOGY`: O(n) where n is the number of triangles
    /// - `CONNECTED_COMPONENTS`: O(n) with union-find
    /// - `ORIENTED` or `FIX_INTERNAL_EDGES`: O(n) for pseudo-normal computation
    /// - `MERGE_DUPLICATE_VERTICES`: O(n) with hash map lookups
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::{TriMesh, TriMeshFlags};
    /// use parry3d::math::Vector;
    ///
    /// // Create vertices for a simple mesh
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    ///     Vector::new(1.0, 1.0, 0.0),
    /// ];
    /// let indices = vec![[0, 1, 2], [1, 3, 2]];
    ///
    /// // Create a mesh with half-edge topology
    /// let flags = TriMeshFlags::HALF_EDGE_TOPOLOGY;
    /// let mesh = TriMesh::with_flags(vertices.clone(), indices.clone(), flags)
    ///     .expect("Failed to create mesh");
    ///
    /// // The topology information is now available
    /// assert!(mesh.topology().is_some());
    /// # }
    /// ```
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::{TriMesh, TriMeshFlags};
    /// use parry3d::math::Vector;
    ///
    /// # let vertices = vec![
    /// #     Vector::ZERO,
    /// #     Vector::new(1.0, 0.0, 0.0),
    /// #     Vector::new(0.0, 1.0, 0.0),
    /// # ];
    /// # let indices = vec![[0, 1, 2]];
    /// // Combine multiple flags for advanced features
    /// let flags = TriMeshFlags::HALF_EDGE_TOPOLOGY
    ///     | TriMeshFlags::MERGE_DUPLICATE_VERTICES
    ///     | TriMeshFlags::DELETE_DEGENERATE_TRIANGLES;
    ///
    /// let mesh = TriMesh::with_flags(vertices, indices, flags)
    ///     .expect("Failed to create mesh");
    /// # }
    /// ```
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::{TriMesh, TriMeshFlags};
    /// use parry3d::math::Vector;
    ///
    /// # let vertices = vec![
    /// #     Vector::ZERO,
    /// #     Vector::new(1.0, 0.0, 0.0),
    /// #     Vector::new(0.0, 1.0, 0.0),
    /// # ];
    /// # let indices = vec![[0, 1, 2]];
    /// // For robust point containment tests in 3D
    /// let flags = TriMeshFlags::ORIENTED;
    /// let mesh = TriMesh::with_flags(vertices, indices, flags)
    ///     .expect("Failed to create mesh");
    ///
    /// // Pseudo-normals are now computed for accurate inside/outside tests
    /// assert!(mesh.pseudo_normals().is_some());
    /// # }
    /// ```
    pub fn with_flags(
        vertices: Vec<Vector>,
        indices: Vec<[u32; 3]>,
        flags: TriMeshFlags,
    ) -> Result<Self, TriMeshBuilderError> {
        if indices.is_empty() {
            return Err(TriMeshBuilderError::EmptyIndices);
        }

        let mut result = Self {
            bvh: Bvh::new(),
            vertices,
            indices,
            #[cfg(feature = "dim3")]
            pseudo_normals: None,
            topology: None,
            connected_components: None,
            flags: TriMeshFlags::empty(),
        };

        let _ = result.set_flags(flags);

        if result.bvh.is_empty() {
            // The BVH hasn’t been computed by `.set_flags`.
            result.rebuild_bvh();
        }

        Ok(result)
    }

    /// Sets the flags of this triangle mesh, controlling its optional associated data.
    pub fn set_flags(&mut self, flags: TriMeshFlags) -> Result<(), TopologyError> {
        let mut result = Ok(());
        let prev_indices_len = self.indices.len();

        if !flags.contains(TriMeshFlags::HALF_EDGE_TOPOLOGY) {
            self.topology = None;
        }

        #[cfg(feature = "dim3")]
        if !flags.intersects(TriMeshFlags::ORIENTED | TriMeshFlags::FIX_INTERNAL_EDGES) {
            self.pseudo_normals = None;
        }

        if !flags.contains(TriMeshFlags::CONNECTED_COMPONENTS) {
            self.connected_components = None;
        }

        let difference = flags & !self.flags;

        if difference.intersects(
            TriMeshFlags::MERGE_DUPLICATE_VERTICES
                | TriMeshFlags::DELETE_DEGENERATE_TRIANGLES
                | TriMeshFlags::DELETE_DUPLICATE_TRIANGLES,
        ) {
            self.merge_duplicate_vertices(
                flags.contains(TriMeshFlags::DELETE_DEGENERATE_TRIANGLES),
                flags.contains(TriMeshFlags::DELETE_DUPLICATE_TRIANGLES),
            )
        }

        if difference.intersects(
            TriMeshFlags::HALF_EDGE_TOPOLOGY | TriMeshFlags::DELETE_BAD_TOPOLOGY_TRIANGLES,
        ) {
            result =
                self.compute_topology(flags.contains(TriMeshFlags::DELETE_BAD_TOPOLOGY_TRIANGLES));
        }

        #[cfg(feature = "std")]
        if difference.intersects(TriMeshFlags::CONNECTED_COMPONENTS) {
            self.compute_connected_components();
        }

        #[cfg(feature = "dim3")]
        if difference.intersects(TriMeshFlags::ORIENTED | TriMeshFlags::FIX_INTERNAL_EDGES) {
            self.compute_pseudo_normals();
        }

        if prev_indices_len != self.indices.len() {
            self.rebuild_bvh();
        }

        self.flags = flags;
        result
    }

    // TODO: support a crate like get_size2 (will require support on nalgebra too)?
    /// An approximation of the memory usage (in bytes) for this struct plus
    /// the memory it allocates dynamically.
    pub fn total_memory_size(&self) -> usize {
        size_of::<Self>() + self.heap_memory_size()
    }

    /// An approximation of the memory dynamically-allocated by this struct.
    pub fn heap_memory_size(&self) -> usize {
        // NOTE: if a new field is added to `Self`, adjust this function result.
        let Self {
            bvh,
            vertices,
            indices,
            topology,
            connected_components,
            flags: _,
            #[cfg(feature = "dim3")]
            pseudo_normals,
        } = self;
        let sz_bvh = bvh.heap_memory_size();
        let sz_vertices = vertices.capacity() * size_of::<Vector>();
        let sz_indices = indices.capacity() * size_of::<[u32; 3]>();
        #[cfg(feature = "dim3")]
        let sz_pseudo_normals = pseudo_normals
            .as_ref()
            .map(|pn| {
                pn.vertices_pseudo_normal.capacity() * size_of::<Vector>()
                    + pn.edges_pseudo_normal.capacity() * size_of::<[Vector; 3]>()
            })
            .unwrap_or(0);
        #[cfg(feature = "dim2")]
        let sz_pseudo_normals = 0;
        let sz_topology = topology
            .as_ref()
            .map(|t| {
                t.vertices.capacity() * size_of::<TopoVertex>()
                    + t.faces.capacity() * size_of::<TopoFace>()
                    + t.half_edges.capacity() * size_of::<TopoHalfEdge>()
            })
            .unwrap_or(0);
        let sz_connected_components = connected_components
            .as_ref()
            .map(|c| {
                c.face_colors.capacity() * size_of::<u32>()
                    + c.grouped_faces.capacity() * size_of::<f32>()
                    + c.ranges.capacity() * size_of::<usize>()
            })
            .unwrap_or(0);

        sz_bvh
            + sz_vertices
            + sz_indices
            + sz_pseudo_normals
            + sz_topology
            + sz_connected_components
    }

    /// Transforms in-place the vertices of this triangle mesh.
    ///
    /// Applies a rigid transformation (rotation and translation) to all vertices
    /// of the mesh. This is useful for positioning or orienting a mesh in world space.
    /// The transformation also updates the BVH and pseudo-normals (if present).
    ///
    /// # Arguments
    ///
    /// * `transform` - The isometry (rigid transformation) to apply to all vertices
    ///
    /// # Performance
    ///
    /// This operation is O(n) for transforming vertices, plus O(n log n) for
    /// rebuilding the BVH, where n is the number of triangles.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::TriMesh;
    /// use parry3d::math::{Vector, Pose};
    ///
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    /// ];
    /// let indices = vec![[0, 1, 2]];
    /// let mut mesh = TriMesh::new(vertices, indices).unwrap();
    ///
    /// // Translate the mesh by (10, 0, 0)
    /// let transform = Pose::translation(10.0, 0.0, 0.0);
    /// mesh.transform_vertices(&transform);
    ///
    /// // All vertices are now shifted
    /// assert_eq!(mesh.vertices()[0], Vector::new(10.0, 0.0, 0.0));
    /// assert_eq!(mesh.vertices()[1], Vector::new(11.0, 0.0, 0.0));
    /// assert_eq!(mesh.vertices()[2], Vector::new(10.0, 1.0, 0.0));
    /// # }
    /// ```
    pub fn transform_vertices(&mut self, transform: &Pose) {
        self.vertices
            .iter_mut()
            .for_each(|pt| *pt = transform * *pt);
        self.rebuild_bvh();

        // The pseudo-normals must be rotated too.
        #[cfg(feature = "dim3")]
        if let Some(pseudo_normals) = &mut self.pseudo_normals {
            pseudo_normals
                .vertices_pseudo_normal
                .iter_mut()
                .for_each(|n| *n = transform.rotation * *n);
            pseudo_normals.edges_pseudo_normal.iter_mut().for_each(|n| {
                n[0] = transform.rotation * n[0];
                n[1] = transform.rotation * n[1];
                n[2] = transform.rotation * n[2];
            });
        }
    }

    /// Returns a scaled version of this triangle mesh.
    ///
    /// Creates a new mesh with all vertices scaled by the given per-axis scale factors.
    /// Unlike rigid transformations, scaling can change the shape of the mesh (when
    /// scale factors differ between axes).
    ///
    /// The scaling also updates pseudo-normals (if present) to remain valid after
    /// the transformation, and efficiently scales the BVH structure.
    ///
    /// # Arguments
    ///
    /// * `scale` - The scale factors for each axis (x, y, z in 3D)
    ///
    /// # Returns
    ///
    /// A new `TriMesh` with scaled vertices
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::TriMesh;
    /// use parry3d::math::Vector;
    ///
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    /// ];
    /// let indices = vec![[0, 1, 2]];
    /// let mesh = TriMesh::new(vertices, indices).unwrap();
    ///
    /// // Uniform scaling: double all dimensions
    /// let scaled = mesh.clone().scaled(Vector::new(2.0, 2.0, 2.0));
    /// assert_eq!(scaled.vertices()[1], Vector::new(2.0, 0.0, 0.0));
    ///
    /// // Non-uniform scaling: stretch along X axis
    /// let stretched = mesh.scaled(Vector::new(3.0, 1.0, 1.0));
    /// assert_eq!(stretched.vertices()[1], Vector::new(3.0, 0.0, 0.0));
    /// assert_eq!(stretched.vertices()[2], Vector::new(0.0, 1.0, 0.0));
    /// # }
    /// ```
    pub fn scaled(mut self, scale: Vector) -> Self {
        self.vertices.iter_mut().for_each(|pt| *pt *= scale);

        #[cfg(feature = "dim3")]
        if let Some(pn) = &mut self.pseudo_normals {
            pn.vertices_pseudo_normal.iter_mut().for_each(|n| {
                *n *= scale;
                *n = n.normalize_or(*n);
            });
            pn.edges_pseudo_normal.iter_mut().for_each(|n| {
                n[0] *= scale;
                n[1] *= scale;
                n[2] *= scale;

                n[0] = n[0].normalize_or(n[0]);
                n[1] = n[1].normalize_or(n[1]);
                n[2] = n[2].normalize_or(n[2]);
            });
        }

        let mut bvh = self.bvh.clone();
        bvh.scale(scale);

        Self {
            bvh,
            vertices: self.vertices,
            indices: self.indices,
            #[cfg(feature = "dim3")]
            pseudo_normals: self.pseudo_normals,
            topology: self.topology,
            connected_components: self.connected_components,
            flags: self.flags,
        }
    }

    /// Appends a second triangle mesh to this triangle mesh.
    ///
    /// This combines two meshes into one by adding all vertices and triangles from
    /// the `rhs` mesh to this mesh. The vertex indices in the appended triangles
    /// are automatically adjusted to reference the correct vertices in the combined
    /// vertex buffer.
    ///
    /// After appending, all optional features (topology, pseudo-normals, etc.) are
    /// recomputed according to this mesh's current flags.
    ///
    /// # Arguments
    ///
    /// * `rhs` - The mesh to append to this mesh
    ///
    /// # Performance
    ///
    /// This operation rebuilds the entire mesh with all its features, which can be
    /// expensive for large meshes. Time complexity is O(n log n) where n is the
    /// total number of triangles after appending.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::TriMesh;
    /// use parry3d::math::Vector;
    ///
    /// // Create first mesh (a triangle)
    /// let vertices1 = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    /// ];
    /// let indices1 = vec![[0, 1, 2]];
    /// let mut mesh1 = TriMesh::new(vertices1, indices1).unwrap();
    ///
    /// // Create second mesh (another triangle, offset in space)
    /// let vertices2 = vec![
    ///     Vector::new(2.0, 0.0, 0.0),
    ///     Vector::new(3.0, 0.0, 0.0),
    ///     Vector::new(2.0, 1.0, 0.0),
    /// ];
    /// let indices2 = vec![[0, 1, 2]];
    /// let mesh2 = TriMesh::new(vertices2, indices2).unwrap();
    ///
    /// // Append second mesh to first
    /// mesh1.append(&mesh2);
    ///
    /// assert_eq!(mesh1.num_triangles(), 2);
    /// assert_eq!(mesh1.vertices().len(), 6);
    /// # }
    /// ```
    pub fn append(&mut self, rhs: &TriMesh) {
        let base_id = self.vertices.len() as u32;
        self.vertices.extend_from_slice(rhs.vertices());
        self.indices.extend(
            rhs.indices()
                .iter()
                .map(|idx| [idx[0] + base_id, idx[1] + base_id, idx[2] + base_id]),
        );

        let vertices = core::mem::take(&mut self.vertices);
        let indices = core::mem::take(&mut self.indices);
        *self = TriMesh::with_flags(vertices, indices, self.flags).unwrap();
    }

    /// Create a `TriMesh` from a set of points assumed to describe a counter-clockwise non-convex polygon.
    ///
    /// This function triangulates a 2D polygon using ear clipping algorithm. The polygon
    /// can be convex or concave, but must be simple (no self-intersections) and have
    /// counter-clockwise winding order.
    ///
    /// # Arguments
    ///
    /// * `vertices` - The vertices of the polygon in counter-clockwise order
    ///
    /// # Returns
    ///
    /// * `Some(TriMesh)` - If triangulation succeeded
    /// * `None` - If the polygon is invalid (self-intersecting, degenerate, etc.)
    ///
    /// # Requirements
    ///
    /// - Polygon must be simple (no self-intersections)
    /// - Vertices must be in counter-clockwise order
    /// - Polygon must have non-zero area
    /// - Only available in 2D (`dim2` feature)
    ///
    /// # Performance
    ///
    /// Ear clipping has O(n²) time complexity in the worst case, where n is the
    /// number of vertices.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::TriMesh;
    /// use parry2d::math::Vector;
    ///
    /// // Create a simple concave polygon (L-shape)
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(2.0, 0.0),
    ///     Vector::new(2.0, 1.0),
    ///     Vector::new(1.0, 1.0),
    ///     Vector::new(1.0, 2.0),
    ///     Vector::new(0.0, 2.0),
    /// ];
    ///
    /// let mesh = TriMesh::from_polygon(vertices)
    ///     .expect("Failed to triangulate polygon");
    ///
    /// // The polygon has been triangulated
    /// assert!(mesh.num_triangles() > 0);
    /// # }
    /// ```
    #[cfg(feature = "dim2")]
    pub fn from_polygon(vertices: Vec<Vector>) -> Option<Self> {
        triangulate_ear_clipping(&vertices).map(|indices| Self::new(vertices, indices).unwrap())
    }

    /// A flat view of the index buffer of this mesh.
    ///
    /// Returns the triangle indices as a flat array of `u32` values, where every
    /// three consecutive values form a triangle. This is useful for interfacing
    /// with graphics APIs that expect flat index buffers.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::TriMesh;
    /// use parry3d::math::Vector;
    ///
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    ///     Vector::new(1.0, 1.0, 0.0),
    /// ];
    /// let indices = vec![[0, 1, 2], [1, 3, 2]];
    /// let mesh = TriMesh::new(vertices, indices).unwrap();
    ///
    /// let flat = mesh.flat_indices();
    /// assert_eq!(flat, &[0, 1, 2, 1, 3, 2]);
    /// assert_eq!(flat.len(), mesh.num_triangles() * 3);
    /// # }
    /// ```
    pub fn flat_indices(&self) -> &[u32] {
        unsafe {
            let len = self.indices.len() * 3;
            let data = self.indices.as_ptr() as *const u32;
            core::slice::from_raw_parts(data, len)
        }
    }

    fn rebuild_bvh(&mut self) {
        let leaves = self.indices.iter().enumerate().map(|(i, idx)| {
            let aabb = Triangle::new(
                self.vertices[idx[0] as usize],
                self.vertices[idx[1] as usize],
                self.vertices[idx[2] as usize],
            )
            .local_aabb();
            (i, aabb)
        });

        self.bvh = Bvh::from_iter(BvhBuildStrategy::Binned, leaves)
    }

    /// Reverse the orientation of the triangle mesh.
    ///
    /// This flips the winding order of all triangles, effectively turning the mesh
    /// "inside-out". If triangles had counter-clockwise winding (outward normals),
    /// they will have clockwise winding (inward normals) after this operation.
    ///
    /// This is useful when:
    /// - A mesh was imported with incorrect orientation
    /// - You need to flip normals for rendering or physics
    /// - Creating the "back side" of a mesh
    ///
    /// # Performance
    ///
    /// This operation modifies triangles in-place (O(n)), but recomputes topology
    /// and pseudo-normals if they were present, which can be expensive for large meshes.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::TriMesh;
    /// use parry3d::math::Vector;
    ///
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    /// ];
    /// let indices = vec![[0, 1, 2]];
    ///
    /// let mut mesh = TriMesh::new(vertices, indices).unwrap();
    /// let original_triangle = mesh.triangle(0);
    ///
    /// // Reverse the mesh orientation
    /// mesh.reverse();
    ///
    /// let reversed_triangle = mesh.triangle(0);
    ///
    /// // The first two vertices are swapped
    /// assert_eq!(original_triangle.a, reversed_triangle.b);
    /// assert_eq!(original_triangle.b, reversed_triangle.a);
    /// assert_eq!(original_triangle.c, reversed_triangle.c);
    /// # }
    /// ```
    pub fn reverse(&mut self) {
        self.indices.iter_mut().for_each(|idx| idx.swap(0, 1));

        // NOTE: the BVH, and connected components are not changed by this operation.
        //       The pseudo-normals just have to be flipped.
        //       The topology must be recomputed.

        #[cfg(feature = "dim3")]
        if let Some(pseudo_normals) = &mut self.pseudo_normals {
            for n in &mut pseudo_normals.vertices_pseudo_normal {
                *n = -*n;
            }

            for n in pseudo_normals.edges_pseudo_normal.iter_mut() {
                n[0] = -n[0];
                n[1] = -n[1];
                n[2] = -n[2];
            }
        }

        if self.flags.contains(TriMeshFlags::HALF_EDGE_TOPOLOGY) {
            // TODO: this could be done more efficiently.
            let _ = self.compute_topology(false);
        }
    }

    /// Merge all duplicate vertices and adjust the index buffer accordingly.
    ///
    /// If `delete_degenerate_triangles` is set to true, any triangle with two
    /// identical vertices will be removed.
    ///
    /// This is typically used to recover a vertex buffer from which we can deduce
    /// adjacency information. between triangles by observing how the vertices are
    /// shared by triangles based on the index buffer.
    fn merge_duplicate_vertices(
        &mut self,
        delete_degenerate_triangles: bool,
        delete_duplicate_triangles: bool,
    ) {
        let mut vtx_to_id = HashMap::default();
        let mut new_vertices = Vec::with_capacity(self.vertices.len());
        let mut new_indices = Vec::with_capacity(self.indices.len());
        let mut triangle_set = HashSet::default();

        fn resolve_coord_id(
            coord: Vector,
            vtx_to_id: &mut HashMap<HashablePartialEq<Vector>, u32>,
            new_vertices: &mut Vec<Vector>,
        ) -> u32 {
            let key = HashablePartialEq::new(coord);
            let id = match vtx_to_id.entry(key) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => entry.insert(new_vertices.len() as u32),
            };

            if *id == new_vertices.len() as u32 {
                new_vertices.push(coord);
            }

            *id
        }

        for t in self.indices.iter() {
            let va = resolve_coord_id(
                self.vertices[t[0] as usize],
                &mut vtx_to_id,
                &mut new_vertices,
            );

            let vb = resolve_coord_id(
                self.vertices[t[1] as usize],
                &mut vtx_to_id,
                &mut new_vertices,
            );

            let vc = resolve_coord_id(
                self.vertices[t[2] as usize],
                &mut vtx_to_id,
                &mut new_vertices,
            );

            let is_degenerate = va == vb || va == vc || vb == vc;

            if !is_degenerate || !delete_degenerate_triangles {
                if delete_duplicate_triangles {
                    let (c, b, a) = crate::utils::sort3(&va, &vb, &vc);
                    if triangle_set.insert((*a, *b, *c)) {
                        new_indices.push([va, vb, vc])
                    }
                } else {
                    new_indices.push([va, vb, vc]);
                }
            }
        }

        new_vertices.shrink_to_fit();

        self.vertices = new_vertices;
        self.indices = new_indices;

        // Vertices and indices changed: the pseudo-normals are no longer valid.
        #[cfg(feature = "dim3")]
        if self.pseudo_normals.is_some() {
            self.compute_pseudo_normals();
        }

        // Vertices and indices changed: the topology no longer valid.
        #[cfg(feature = "dim3")]
        if self.topology.is_some() {
            let _ = self.compute_topology(false);
        }
    }

    #[cfg(feature = "dim3")]
    /// Computes the pseudo-normals used for solid point-projection.
    ///
    /// This computes the pseudo-normals needed by the point containment test described in
    /// "Signed distance computation using the angle weighted pseudonormal", Baerentzen, et al.
    /// DOI: 10.1109/TVCG.2005.49
    ///
    /// For the point-containment test to properly detect the inside of the trimesh (i.e. to return
    /// `proj.is_inside = true`), the trimesh must:
    /// - Be manifold (closed, no t-junctions, etc.)
    /// - Be oriented with outward normals.
    ///
    /// If the trimesh is correctly oriented, but is manifold everywhere except at its boundaries,
    /// then the computed pseudo-normals will provide correct point-containment test results except
    /// for points closest to the boundary of the mesh.
    ///
    /// It may be useful to call `self.remove_duplicate_vertices()` before this method, in order to fix the
    /// index buffer if some of the vertices of this trimesh are duplicated.
    fn compute_pseudo_normals(&mut self) {
        let mut vertices_pseudo_normal = vec![Vector::ZERO; self.vertices().len()];
        let mut edges_pseudo_normal = HashMap::default();
        let mut edges_multiplicity = HashMap::default();

        for idx in self.indices() {
            let vtx = self.vertices();
            let tri = Triangle::new(
                vtx[idx[0] as usize],
                vtx[idx[1] as usize],
                vtx[idx[2] as usize],
            );

            if let Some(n) = tri.normal() {
                let ang1 = (tri.b - tri.a).angle(tri.c - tri.a);
                let ang2 = (tri.a - tri.b).angle(tri.c - tri.b);
                let ang3 = (tri.b - tri.c).angle(tri.a - tri.c);

                vertices_pseudo_normal[idx[0] as usize] += n * ang1;
                vertices_pseudo_normal[idx[1] as usize] += n * ang2;
                vertices_pseudo_normal[idx[2] as usize] += n * ang3;

                let edges = [
                    SortedPair::new(idx[0], idx[1]),
                    SortedPair::new(idx[0], idx[2]),
                    SortedPair::new(idx[1], idx[2]),
                ];

                for edge in &edges {
                    let edge_n = edges_pseudo_normal.entry(*edge).or_insert(Vector::ZERO);
                    *edge_n += n; // NOTE: there is no need to multiply by the incident angle since it is always equal to PI for all the edges.
                    let edge_mult = edges_multiplicity.entry(*edge).or_insert(0);
                    *edge_mult += 1;
                }
            }
        }

        let edges_pseudo_normal = self
            .indices()
            .iter()
            .map(|idx| {
                let e0 = SortedPair::new(idx[0], idx[1]);
                let e1 = SortedPair::new(idx[1], idx[2]);
                let e2 = SortedPair::new(idx[2], idx[0]);
                let default = Vector::ZERO;
                [
                    edges_pseudo_normal.get(&e0).copied().unwrap_or(default),
                    edges_pseudo_normal.get(&e1).copied().unwrap_or(default),
                    edges_pseudo_normal.get(&e2).copied().unwrap_or(default),
                ]
            })
            .collect();

        self.pseudo_normals = Some(TriMeshPseudoNormals {
            vertices_pseudo_normal,
            edges_pseudo_normal,
        })
    }

    fn delete_bad_topology_triangles(&mut self) {
        let mut half_edge_set = HashSet::default();
        let mut deleted_any = false;

        // First, create three half-edges for each face.
        self.indices.retain(|idx| {
            if idx[0] == idx[1] || idx[0] == idx[2] || idx[1] == idx[2] {
                deleted_any = true;
                return false;
            }

            for k in 0..3 {
                let edge_key = (idx[k as usize], idx[(k as usize + 1) % 3]);
                if half_edge_set.contains(&edge_key) {
                    deleted_any = true;
                    return false;
                }
            }

            for k in 0..3 {
                let edge_key = (idx[k as usize], idx[(k as usize + 1) % 3]);
                let _ = half_edge_set.insert(edge_key);
            }

            true
        });
    }

    /// Computes half-edge topological information for this triangle mesh, based on its index buffer only.
    ///
    /// This computes the half-edge representation of this triangle mesh’s topology. This is useful for advanced
    /// geometric operations like trimesh-trimesh intersection geometry computation.
    ///
    /// It may be useful to call `self.merge_duplicate_vertices(true, true)` before this method, in order to fix the
    /// index buffer if some of the vertices of this trimesh are duplicated.
    ///
    /// # Return
    /// Returns `true` if the computation succeeded. Returns `false` if this mesh can’t have an half-edge representation
    /// because at least three faces share the same edge.
    fn compute_topology(&mut self, delete_bad_triangles: bool) -> Result<(), TopologyError> {
        if delete_bad_triangles {
            self.delete_bad_topology_triangles();
        }

        let mut topology = TriMeshTopology::default();
        let mut half_edge_map = HashMap::default();

        topology.vertices.resize(
            self.vertices.len(),
            TopoVertex {
                half_edge: u32::MAX,
            },
        );

        // First, create three half-edges for each face.
        for (fid, idx) in self.indices.iter().enumerate() {
            let half_edge_base_id = topology.half_edges.len() as u32;

            if idx[0] == idx[1] || idx[0] == idx[2] || idx[1] == idx[2] {
                return Err(TopologyError::BadTriangle(fid as u32));
            }

            for k in 0u32..3 {
                let half_edge = TopoHalfEdge {
                    next: half_edge_base_id + (k + 1) % 3,
                    // We don’t know which one it is yet.
                    // If the twin doesn’t exist, we use `u32::MAX` as
                    // it’s (invalid) index. This value can be relied on
                    // by other algorithms.
                    twin: u32::MAX,
                    vertex: idx[k as usize],
                    face: fid as u32,
                };
                topology.half_edges.push(half_edge);

                let edge_key = (idx[k as usize], idx[(k as usize + 1) % 3]);

                if let Some(existing) = half_edge_map.insert(edge_key, half_edge_base_id + k) {
                    // If the same edge already exists (with the same vertex order) then
                    // we have two triangles sharing the same but with opposite incompatible orientations.
                    return Err(TopologyError::BadAdjacentTrianglesOrientation {
                        edge: edge_key,
                        triangle1: topology.half_edges[existing as usize].face,
                        triangle2: fid as u32,
                    });
                }

                topology.vertices[idx[k as usize] as usize].half_edge = half_edge_base_id + k;
            }

            topology.faces.push(TopoFace {
                half_edge: half_edge_base_id,
            })
        }

        // Second, identify twins.
        for (key, he1) in &half_edge_map {
            if key.0 < key.1 {
                // Test, to avoid checking the same pair twice.
                if let Some(he2) = half_edge_map.get(&(key.1, key.0)) {
                    topology.half_edges[*he1 as usize].twin = *he2;
                    topology.half_edges[*he2 as usize].twin = *he1;
                }
            }
        }

        self.topology = Some(topology);

        Ok(())
    }

    // NOTE: this is private because that calculation is controlled by TriMeshFlags::CONNECTED_COMPONENTS
    // TODO: we should remove the CONNECTED_COMPONENTS flags and just have this be a free function.
    // TODO: this should be no_std compatible once ena is or once we have an alternative for it.
    #[cfg(feature = "std")]
    fn compute_connected_components(&mut self) {
        use ena::unify::{InPlaceUnificationTable, UnifyKey};

        #[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
        struct IntKey(u32);

        impl UnifyKey for IntKey {
            type Value = ();
            fn index(&self) -> u32 {
                self.0
            }
            fn from_index(u: u32) -> IntKey {
                IntKey(u)
            }
            fn tag() -> &'static str {
                "IntKey"
            }
        }

        let mut ufind: InPlaceUnificationTable<IntKey> = InPlaceUnificationTable::new();
        let mut face_colors = vec![u32::MAX; self.indices.len()];
        let mut ranges = vec![0];
        let mut vertex_to_range = vec![u32::MAX; self.vertices.len()];
        let mut grouped_faces = vec![u32::MAX; self.indices.len()];
        let mut vertex_to_key = vec![IntKey(u32::MAX); self.vertices.len()];

        let mut vertex_key = |id: u32, ufind: &mut InPlaceUnificationTable<IntKey>| {
            if vertex_to_key[id as usize].0 == u32::MAX {
                let new_key = ufind.new_key(());
                vertex_to_key[id as usize] = new_key;
                new_key
            } else {
                vertex_to_key[id as usize]
            }
        };

        for idx in self.indices() {
            let keys = idx.map(|i| vertex_key(i, &mut ufind));
            ufind.union(keys[0], keys[1]);
            ufind.union(keys[1], keys[2]);
            ufind.union(keys[2], keys[0]);
        }

        for (idx, face_color) in self.indices().iter().zip(face_colors.iter_mut()) {
            debug_assert_eq!(
                ufind.find(vertex_to_key[idx[0] as usize]),
                ufind.find(vertex_to_key[idx[1] as usize])
            );
            debug_assert_eq!(
                ufind.find(vertex_to_key[idx[0] as usize]),
                ufind.find(vertex_to_key[idx[2] as usize])
            );

            let group_index = ufind.find(vertex_to_key[idx[0] as usize]).0 as usize;

            if vertex_to_range[group_index] == u32::MAX {
                // Additional range
                ranges.push(0);
                vertex_to_range[group_index] = ranges.len() as u32 - 1;
            }

            let range_id = vertex_to_range[group_index];
            ranges[range_id as usize] += 1;
            // NOTE: the range_id points to the range upper bound. The face color is the range lower bound.
            *face_color = range_id - 1;
        }

        // Cumulated sum on range indices, to find the first index faces need to be inserted into
        // for each range.
        for i in 1..ranges.len() {
            ranges[i] += ranges[i - 1];
        }

        debug_assert_eq!(*ranges.last().unwrap(), self.indices().len());

        // Group faces.
        let mut insertion_in_range_index = ranges.clone();
        for (face_id, face_color) in face_colors.iter().enumerate() {
            let insertion_index = &mut insertion_in_range_index[*face_color as usize];
            grouped_faces[*insertion_index] = face_id as u32;
            *insertion_index += 1;
        }

        self.connected_components = Some(TriMeshConnectedComponents {
            face_colors,
            grouped_faces,
            ranges,
        })
    }

    #[allow(dead_code)] // Useful for testing.
    pub(crate) fn assert_half_edge_topology_is_valid(&self) {
        let topo = self
            .topology
            .as_ref()
            .expect("No topology information found.");
        assert_eq!(self.vertices.len(), topo.vertices.len());
        assert_eq!(self.indices.len(), topo.faces.len());

        for (face_id, (face, idx)) in topo.faces.iter().zip(self.indices.iter()).enumerate() {
            let he0 = topo.half_edges[face.half_edge as usize];
            assert_eq!(he0.face, face_id as u32);
            assert_eq!(he0.vertex, idx[0]);
            let he1 = topo.half_edges[he0.next as usize];
            assert_eq!(he1.face, face_id as u32);
            assert_eq!(he1.vertex, idx[1]);
            let he2 = topo.half_edges[he1.next as usize];
            assert_eq!(he2.face, face_id as u32);
            assert_eq!(he2.vertex, idx[2]);
            assert_eq!(he2.next, face.half_edge);
        }

        for he in &topo.half_edges {
            let idx = &self.indices[he.face as usize];
            assert!(he.vertex == idx[0] || he.vertex == idx[1] || he.vertex == idx[2]);
        }
    }

    /// An iterator through all the triangles of this mesh.
    ///
    /// Returns an iterator that yields [`Triangle`] shapes representing each
    /// triangle in the mesh. Each triangle contains the actual 3D coordinates
    /// of its three vertices (not indices).
    ///
    /// # Performance
    ///
    /// The iterator performs vertex lookups on-the-fly, so iterating through
    /// all triangles is O(n) where n is the number of triangles.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::TriMesh;
    /// use parry3d::math::Vector;
    ///
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    ///     Vector::new(1.0, 1.0, 0.0),
    /// ];
    /// let indices = vec![[0, 1, 2], [1, 3, 2]];
    /// let mesh = TriMesh::new(vertices, indices).unwrap();
    ///
    /// // Iterate through all triangles
    /// for triangle in mesh.triangles() {
    ///     println!("Triangle: {:?}, {:?}, {:?}", triangle.a, triangle.b, triangle.c);
    /// }
    ///
    /// // Count triangles with specific properties
    /// let count = mesh.triangles()
    ///     .filter(|tri| tri.area() > 0.1)
    ///     .count();
    /// # }
    /// ```
    pub fn triangles(&self) -> impl ExactSizeIterator<Item = Triangle> + '_ {
        self.indices.iter().map(move |ids| {
            Triangle::new(
                self.vertices[ids[0] as usize],
                self.vertices[ids[1] as usize],
                self.vertices[ids[2] as usize],
            )
        })
    }

    #[cfg(feature = "dim3")]
    /// Gets the normal of the triangle represented by `feature`.
    pub fn feature_normal(&self, feature: FeatureId) -> Option<Vector> {
        match feature {
            FeatureId::Face(i) => self
                .triangle(i % self.num_triangles() as u32)
                .feature_normal(FeatureId::Face(0)),
            _ => None,
        }
    }
}

impl TriMesh {
    /// The flags of this triangle mesh.
    ///
    /// Returns the [`TriMeshFlags`] that were used to construct this mesh,
    /// indicating which optional features are enabled.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::{TriMesh, TriMeshFlags};
    /// use parry3d::math::Vector;
    ///
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    /// ];
    /// let indices = vec![[0, 1, 2]];
    ///
    /// let flags = TriMeshFlags::HALF_EDGE_TOPOLOGY;
    /// let mesh = TriMesh::with_flags(vertices, indices, flags).unwrap();
    ///
    /// assert!(mesh.flags().contains(TriMeshFlags::HALF_EDGE_TOPOLOGY));
    /// # }
    /// ```
    pub fn flags(&self) -> TriMeshFlags {
        self.flags
    }

    /// Compute the axis-aligned bounding box of this triangle mesh.
    ///
    /// Returns the AABB that tightly bounds all triangles of the mesh after
    /// applying the given isometry transformation.
    ///
    /// # Arguments
    ///
    /// * `pos` - The position/orientation of the mesh in world space
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::TriMesh;
    /// use parry3d::math::{Vector, Pose};
    ///
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    /// ];
    /// let indices = vec![[0, 1, 2]];
    /// let mesh = TriMesh::new(vertices, indices).unwrap();
    ///
    /// let identity = Pose::identity();
    /// let aabb = mesh.aabb(&identity);
    ///
    /// // The AABB contains all vertices
    /// assert!(aabb.contains_local_point(Vector::new(0.5, 0.5, 0.0)));
    /// # }
    /// ```
    pub fn aabb(&self, pos: &Pose) -> Aabb {
        self.bvh.root_aabb().transform_by(pos)
    }

    /// Gets the local axis-aligned bounding box of this triangle mesh.
    ///
    /// Returns the AABB in the mesh's local coordinate system (without any
    /// transformation applied). This is faster than [`TriMesh::aabb`] when
    /// no transformation is needed.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::TriMesh;
    /// use parry3d::math::Vector;
    ///
    /// let vertices = vec![
    ///     Vector::new(-1.0, -1.0, 0.0),
    ///     Vector::new(1.0, -1.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    /// ];
    /// let indices = vec![[0, 1, 2]];
    /// let mesh = TriMesh::new(vertices, indices).unwrap();
    ///
    /// let aabb = mesh.local_aabb();
    /// assert_eq!(aabb.mins.x, -1.0);
    /// assert_eq!(aabb.maxs.x, 1.0);
    /// # }
    /// ```
    pub fn local_aabb(&self) -> Aabb {
        self.bvh.root_aabb()
    }

    /// The acceleration structure used by this triangle-mesh.
    ///
    /// Returns a reference to the BVH (Bounding Volume Hierarchy) that
    /// accelerates spatial queries on this mesh. The BVH is used internally
    /// for ray casting, collision detection, and other geometric queries.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::TriMesh;
    /// use parry3d::math::Vector;
    ///
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    /// ];
    /// let indices = vec![[0, 1, 2]];
    /// let mesh = TriMesh::new(vertices, indices).unwrap();
    ///
    /// let bvh = mesh.bvh();
    /// // The BVH can be used for advanced spatial queries
    /// assert!(!bvh.is_empty());
    /// # }
    /// ```
    pub fn bvh(&self) -> &Bvh {
        &self.bvh
    }

    /// The number of triangles forming this mesh.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::TriMesh;
    /// use parry3d::math::Vector;
    ///
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    ///     Vector::new(1.0, 1.0, 0.0),
    /// ];
    /// let indices = vec![[0, 1, 2], [1, 3, 2]];
    /// let mesh = TriMesh::new(vertices, indices).unwrap();
    ///
    /// assert_eq!(mesh.num_triangles(), 2);
    /// # }
    /// ```
    pub fn num_triangles(&self) -> usize {
        self.indices.len()
    }

    /// Does the given feature ID identify a backface of this trimesh?
    pub fn is_backface(&self, feature: FeatureId) -> bool {
        if let FeatureId::Face(i) = feature {
            i >= self.indices.len() as u32
        } else {
            false
        }
    }

    /// Get the `i`-th triangle of this mesh.
    ///
    /// Returns a [`Triangle`] shape with the actual vertex coordinates (not indices)
    /// of the requested triangle. The triangle index must be less than the number
    /// of triangles in the mesh.
    ///
    /// # Arguments
    ///
    /// * `i` - The index of the triangle to retrieve (0-based)
    ///
    /// # Panics
    ///
    /// Panics if `i >= self.num_triangles()`.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::TriMesh;
    /// use parry3d::math::Vector;
    ///
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    /// ];
    /// let indices = vec![[0, 1, 2]];
    /// let mesh = TriMesh::new(vertices, indices).unwrap();
    ///
    /// let triangle = mesh.triangle(0);
    /// assert_eq!(triangle.a, Vector::ZERO);
    /// assert_eq!(triangle.b, Vector::new(1.0, 0.0, 0.0));
    /// assert_eq!(triangle.c, Vector::new(0.0, 1.0, 0.0));
    /// # }
    /// ```
    pub fn triangle(&self, i: u32) -> Triangle {
        let idx = self.indices[i as usize];
        Triangle::new(
            self.vertices[idx[0] as usize],
            self.vertices[idx[1] as usize],
            self.vertices[idx[2] as usize],
        )
    }

    /// Returns the pseudo-normals of one of this mesh’s triangles, if it was computed.
    ///
    /// This returns `None` if the pseudo-normals of this triangle were not computed.
    /// To have its pseudo-normals computed, be sure to set the [`TriMeshFlags`] so that
    /// they contain the [`TriMeshFlags::FIX_INTERNAL_EDGES`] flag.
    #[cfg(feature = "dim3")]
    pub fn triangle_normal_constraints(&self, i: u32) -> Option<TrianglePseudoNormals> {
        if self.flags.contains(TriMeshFlags::FIX_INTERNAL_EDGES) {
            let triangle = self.triangle(i);
            let pseudo_normals = self.pseudo_normals.as_ref()?;
            let edges_pseudo_normals = pseudo_normals.edges_pseudo_normal[i as usize];

            // TODO: could the pseudo-normal be pre-normalized instead of having to renormalize
            //       every time we need them?
            Some(TrianglePseudoNormals {
                face: triangle.normal()?,
                edges: [
                    (edges_pseudo_normals[0]).try_normalize()?,
                    (edges_pseudo_normals[1]).try_normalize()?,
                    (edges_pseudo_normals[2]).try_normalize()?,
                ],
            })
        } else {
            None
        }
    }

    #[cfg(feature = "dim2")]
    #[doc(hidden)]
    pub fn triangle_normal_constraints(&self, _i: u32) -> Option<TrianglePseudoNormals> {
        None
    }

    /// The vertex buffer of this mesh.
    ///
    /// Returns a slice containing all vertex positions as 3D points.
    /// The vertices are stored in the order they were provided during construction.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::TriMesh;
    /// use parry3d::math::Vector;
    ///
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    /// ];
    /// let indices = vec![[0, 1, 2]];
    /// let mesh = TriMesh::new(vertices, indices).unwrap();
    ///
    /// assert_eq!(mesh.vertices().len(), 3);
    /// assert_eq!(mesh.vertices()[0], Vector::ZERO);
    /// # }
    /// ```
    pub fn vertices(&self) -> &[Vector] {
        &self.vertices
    }

    /// The index buffer of this mesh.
    ///
    /// Returns a slice of triangles, where each triangle is represented as an
    /// array of three vertex indices. The indices reference positions in the
    /// vertex buffer returned by [`TriMesh::vertices`].
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::TriMesh;
    /// use parry3d::math::Vector;
    ///
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    ///     Vector::new(1.0, 1.0, 0.0),
    /// ];
    /// let indices = vec![[0, 1, 2], [1, 3, 2]];
    /// let mesh = TriMesh::new(vertices, indices).unwrap();
    ///
    /// assert_eq!(mesh.indices().len(), 2);
    /// assert_eq!(mesh.indices()[0], [0, 1, 2]);
    /// assert_eq!(mesh.indices()[1], [1, 3, 2]);
    /// # }
    /// ```
    pub fn indices(&self) -> &[[u32; 3]] {
        &self.indices
    }

    /// Returns the topology information of this trimesh, if it has been computed.
    ///
    /// Topology information includes half-edge data structures that describe
    /// adjacency relationships between vertices, edges, and faces. This is
    /// computed when the [`TriMeshFlags::HALF_EDGE_TOPOLOGY`] flag is set.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::{TriMesh, TriMeshFlags};
    /// use parry3d::math::Vector;
    ///
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    /// ];
    /// let indices = vec![[0, 1, 2]];
    ///
    /// // Without topology flag
    /// let mesh = TriMesh::new(vertices.clone(), indices.clone()).unwrap();
    /// assert!(mesh.topology().is_none());
    ///
    /// // With topology flag
    /// let mesh = TriMesh::with_flags(
    ///     vertices,
    ///     indices,
    ///     TriMeshFlags::HALF_EDGE_TOPOLOGY
    /// ).unwrap();
    /// assert!(mesh.topology().is_some());
    /// # }
    /// ```
    pub fn topology(&self) -> Option<&TriMeshTopology> {
        self.topology.as_ref()
    }

    /// Returns the connected-component information of this trimesh, if it has been computed.
    ///
    /// Connected components represent separate, non-connected parts of the mesh.
    /// This is computed when the [`TriMeshFlags::CONNECTED_COMPONENTS`] flag is set
    /// (requires the `std` feature).
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # #[cfg(feature = "std")] {
    /// use parry3d::shape::{TriMesh, TriMeshFlags};
    /// use parry3d::math::Vector;
    ///
    /// // Create two separate triangles (not connected)
    /// let vertices = vec![
    ///     // First triangle
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    ///     // Second triangle (separate)
    ///     Vector::new(5.0, 0.0, 0.0),
    ///     Vector::new(6.0, 0.0, 0.0),
    ///     Vector::new(5.0, 1.0, 0.0),
    /// ];
    /// let indices = vec![[0, 1, 2], [3, 4, 5]];
    ///
    /// let mesh = TriMesh::with_flags(
    ///     vertices,
    ///     indices,
    ///     TriMeshFlags::CONNECTED_COMPONENTS
    /// ).unwrap();
    ///
    /// if let Some(cc) = mesh.connected_components() {
    ///     assert_eq!(cc.num_connected_components(), 2);
    /// }
    /// # }
    /// # }
    /// ```
    pub fn connected_components(&self) -> Option<&TriMeshConnectedComponents> {
        self.connected_components.as_ref()
    }

    /// Returns the connected-component of this mesh.
    ///
    /// The connected-components are returned as a set of `TriMesh` build with the given `flags`.
    pub fn connected_component_meshes(
        &self,
        flags: TriMeshFlags,
    ) -> Option<Vec<Result<TriMesh, TriMeshBuilderError>>> {
        self.connected_components()
            .map(|cc| cc.to_meshes(self, flags))
    }

    /// The pseudo-normals of this triangle mesh, if they have been computed.
    #[cfg(feature = "dim3")]
    pub fn pseudo_normals(&self) -> Option<&TriMeshPseudoNormals> {
        self.pseudo_normals.as_ref()
    }

    /// The pseudo-normals of this triangle mesh, if they have been computed **and** this mesh was
    /// marked as [`TriMeshFlags::ORIENTED`].
    #[cfg(feature = "dim3")]
    pub fn pseudo_normals_if_oriented(&self) -> Option<&TriMeshPseudoNormals> {
        if self.flags.intersects(TriMeshFlags::ORIENTED) {
            self.pseudo_normals.as_ref()
        } else {
            None
        }
    }
}

#[cfg(feature = "dim3")]
impl From<crate::shape::HeightField> for TriMesh {
    fn from(heightfield: crate::shape::HeightField) -> Self {
        let (vtx, idx) = heightfield.to_trimesh();
        TriMesh::new(vtx, idx).unwrap()
    }
}

#[cfg(feature = "dim3")]
impl From<Cuboid> for TriMesh {
    fn from(cuboid: Cuboid) -> Self {
        let (vtx, idx) = cuboid.to_trimesh();
        TriMesh::new(vtx, idx).unwrap()
    }
}

impl CompositeShape for TriMesh {
    fn map_part_at(
        &self,
        i: u32,
        f: &mut dyn FnMut(Option<&Pose>, &dyn Shape, Option<&dyn NormalConstraints>),
    ) {
        let tri = self.triangle(i);
        let normals = self.triangle_normal_constraints(i);
        f(
            None,
            &tri,
            normals.as_ref().map(|n| n as &dyn NormalConstraints),
        )
    }

    fn bvh(&self) -> &Bvh {
        &self.bvh
    }
}

impl TypedCompositeShape for TriMesh {
    type PartShape = Triangle;
    type PartNormalConstraints = TrianglePseudoNormals;

    #[inline(always)]
    fn map_typed_part_at<T>(
        &self,
        i: u32,
        mut f: impl FnMut(Option<&Pose>, &Self::PartShape, Option<&Self::PartNormalConstraints>) -> T,
    ) -> Option<T> {
        let tri = self.triangle(i);
        let pseudo_normals = self.triangle_normal_constraints(i);
        Some(f(None, &tri, pseudo_normals.as_ref()))
    }

    #[inline(always)]
    fn map_untyped_part_at<T>(
        &self,
        i: u32,
        mut f: impl FnMut(Option<&Pose>, &dyn Shape, Option<&dyn NormalConstraints>) -> T,
    ) -> Option<T> {
        let tri = self.triangle(i);
        let pseudo_normals = self.triangle_normal_constraints(i);
        Some(f(
            None,
            &tri,
            pseudo_normals.as_ref().map(|n| n as &dyn NormalConstraints),
        ))
    }
}

#[cfg(test)]
mod test {
    use crate::math::{Real, Vector};
    use crate::shape::{Cuboid, TriMesh, TriMeshFlags};

    #[test]
    fn trimesh_error_empty_indices() {
        assert!(
            TriMesh::with_flags(vec![], vec![], TriMeshFlags::empty()).is_err(),
            "A triangle mesh with no triangles is invalid."
        );
    }

    #[test]
    fn connected_components() {
        let (vtx, idx) = Cuboid::new(Vector::splat(0.5)).to_trimesh();

        // Push 10 copy of the mesh, each time pushed with an offset.
        let mut mesh = TriMesh::new(vtx.clone(), idx.clone()).unwrap();

        for i in 1..10 {
            let cc_vtx = vtx
                .iter()
                .map(|pt| *pt + Vector::splat(2.0 * i as Real))
                .collect();

            let to_append = TriMesh::new(cc_vtx, idx.clone()).unwrap();
            mesh.append(&to_append);
        }

        mesh.set_flags(TriMeshFlags::CONNECTED_COMPONENTS).unwrap();
        let connected_components = mesh.connected_components().unwrap();
        assert_eq!(connected_components.num_connected_components(), 10);

        let cc_meshes = connected_components.to_meshes(&mesh, TriMeshFlags::empty());

        for cc in cc_meshes {
            let cc = cc.unwrap();
            assert_eq!(cc.vertices.len(), vtx.len());
            assert_eq!(cc.indices.len(), idx.len());
        }
    }
}

use crate::shape::TriMeshBuilderError;
use core::fmt;

#[cfg(doc)]
use crate::shape::{TriMesh, TriMeshFlags};

/// Errors that can occur when computing the boolean intersection of two triangle meshes.
///
/// The [`intersect_meshes`] function computes the geometric intersection of two triangle meshes,
/// producing a new mesh that represents their overlapping volume. This operation requires both
/// input meshes to have certain properties and can fail for various reasons.
///
/// # Prerequisites for Mesh Intersection
///
/// Both input meshes must have:
/// 1. **Topology information**: Half-edge data structure for adjacency information
/// 2. **Pseudo-normals**: For robust inside/outside testing
///
/// These are enabled by setting the [`TriMeshFlags::ORIENTED`] flag when creating the mesh.
///
// /// TODO: figure out why this doc-test fails?
// /// # Common Usage Pattern
// ///
// /// ```
// /// # #[cfg(all(feature = "dim3", feature = "spade"))] {
// /// use parry3d::shape::{TriMesh, TriMeshFlags};
// /// use parry3d::transformation::{intersect_meshes, MeshIntersectionError};
// ///
// /// // Create two meshes with proper flags
// /// let vertices1 = vec![
// ///     Vector::new(-1.0, -1.0, -1.0),
// ///     Vector::new(1.0, -1.0, -1.0),
// ///     Vector::new(1.0, 1.0, -1.0),
// ///     Vector::new(-1.0, 1.0, -1.0),
// /// ];
// /// let indices1 = vec![[0, 1, 2], [0, 2, 3]];
// ///
// /// // IMPORTANT: Use ORIENTED flag to enable topology and pseudo-normals
// /// let mesh1 = TriMesh::with_flags(
// ///     vertices1,
// ///     indices1,
// ///     TriMeshFlags::ORIENTED
// /// ).expect("Failed to create mesh");
// ///
// /// let vertices2 = vec![
// ///     Vector::new(0.0, -1.0, -1.0),
// ///     Vector::new(2.0, -1.0, -1.0),
// ///     Vector::new(2.0, 1.0, -1.0),
// ///     Vector::new(0.0, 1.0, -1.0),
// /// ];
// /// let indices2 = vec![[0, 1, 2], [0, 2, 3]];
// /// let mesh2 = TriMesh::with_flags(
// ///     vertices2,
// ///     indices2,
// ///     TriMeshFlags::ORIENTED
// /// ).expect("Failed to create mesh");
// ///
// /// let pos1 = Pose::identity();
// /// let pos2 = Pose::identity();
// ///
// /// match intersect_meshes(&pos1, &mesh1, false, &pos2, &mesh2, false) {
// ///     Ok(intersection_mesh) => {
// ///         println!("Intersection computed successfully!");
// ///     }
// ///     Err(MeshIntersectionError::MissingTopology) => {
// ///         println!("One or both meshes missing topology - use TriMeshFlags::ORIENTED");
// ///     }
// ///     Err(MeshIntersectionError::MissingPseudoNormals) => {
// ///         println!("One or both meshes missing pseudo-normals - use TriMeshFlags::ORIENTED");
// ///     }
// ///     Err(err) => {
// ///         println!("Intersection failed: {}", err);
// ///     }
// /// }
// /// # }
// /// ```
///
/// [`intersect_meshes`]: crate::transformation::intersect_meshes
/// [`TriMeshFlags::ORIENTED`]: crate::shape::TriMeshFlags::ORIENTED
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum MeshIntersectionError {
    /// One or both meshes are missing topology information.
    ///
    /// Mesh intersection requires half-edge topology data to determine adjacency
    /// relationships between triangles. This information is automatically computed
    /// when you create a mesh with the [`TriMeshFlags::ORIENTED`] flag.
    ///
    /// # How to Fix
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::{TriMesh, TriMeshFlags};
    /// # use parry3d::math::Vector;
    /// # let vertices = vec![Vector::ZERO];
    /// # let indices = vec![[0, 0, 0]];
    /// // Instead of:
    /// // let mesh = TriMesh::new(vertices, indices)?;
    ///
    /// // Use ORIENTED flag:
    /// let mesh = TriMesh::with_flags(
    ///     vertices,
    ///     indices,
    ///     TriMeshFlags::ORIENTED
    /// ).unwrap();
    /// # }
    /// ```
    ///
    /// # Alternative
    ///
    /// You can also add topology to an existing mesh:
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::{TriMesh, TriMeshFlags};
    /// # use parry3d::math::Vector;
    /// # let vertices = vec![Vector::ZERO];
    /// # let indices = vec![[0, 0, 0]];
    /// let mut mesh = TriMesh::new(vertices, indices).unwrap();
    /// mesh.set_flags(TriMeshFlags::ORIENTED).unwrap();
    /// # }
    /// ```
    MissingTopology,

    /// One or both meshes are missing pseudo-normal information.
    ///
    /// Pseudo-normals are weighted normals used for robust point-in-mesh testing,
    /// which is essential for determining which parts of the meshes are inside or
    /// outside each other during intersection. They are computed when using the
    /// [`TriMeshFlags::ORIENTED`] flag.
    ///
    /// # Background
    ///
    /// Mesh intersection needs to determine which triangles from each mesh are inside,
    /// outside, or intersecting the other mesh. This requires reliable point containment
    /// tests, which use pseudo-normals as described in:
    ///
    /// "Signed distance computation using the angle weighted pseudonormal"
    /// by Baerentzen et al., DOI: 10.1109/TVCG.2005.49
    ///
    /// # How to Fix
    ///
    /// Use the same solution as [`MissingTopology`](Self::MissingTopology) - create your
    /// meshes with [`TriMeshFlags::ORIENTED`].
    MissingPseudoNormals,

    /// Internal failure while computing the intersection between two triangles.
    ///
    /// This error occurs when the triangle-triangle intersection algorithm encounters
    /// a case it cannot handle, typically due to:
    /// - Degenerate triangles (zero area, collinear vertices)
    /// - Numerical precision issues with nearly-parallel triangles
    /// - Edge cases in the intersection geometry
    ///
    /// # What to Try
    ///
    /// - Check your input meshes for degenerate triangles
    /// - Try using [`TriMeshFlags::DELETE_DEGENERATE_TRIANGLES`] when creating meshes
    /// - Ensure your mesh has reasonable numeric precision (not too small or too large coordinates)
    /// - Consider using [`intersect_meshes_with_tolerances`] with custom tolerances
    ///
    /// [`TriMeshFlags::DELETE_DEGENERATE_TRIANGLES`]: crate::shape::TriMeshFlags::DELETE_DEGENERATE_TRIANGLES
    /// [`intersect_meshes_with_tolerances`]: crate::transformation::intersect_meshes_with_tolerances
    TriTriError,

    /// Internal failure while merging vertices from triangle intersections.
    ///
    /// After computing triangle-triangle intersections, the algorithm merges nearby
    /// vertices to create a clean mesh. This error occurs when the merging process
    /// detects inconsistencies, usually caused by:
    /// - Numerical precision issues causing vertices to appear in wrong positions
    /// - Complex intersection patterns that create ambiguous vertex relationships
    ///
    /// # What to Try
    ///
    /// - Use [`intersect_meshes_with_tolerances`] with larger tolerances
    /// - Simplify your input meshes if they have very high triangle counts
    /// - Check for and remove nearly-degenerate triangles
    ///
    /// [`intersect_meshes_with_tolerances`]: crate::transformation::intersect_meshes_with_tolerances
    DuplicateVertices,

    /// Internal failure while triangulating an intersection face.
    ///
    /// The intersection of two meshes can create non-triangular polygonal faces
    /// that must be triangulated. This error occurs when the triangulation algorithm
    /// fails, typically due to:
    /// - Self-intersecting intersection polygons
    /// - Numerical issues creating invalid polygon geometry
    /// - Very complex intersection patterns
    ///
    /// This often happens with grazing intersections or when meshes have very
    /// different triangle sizes at the intersection boundary.
    TriangulationError,

    /// The resulting intersection mesh could not be constructed.
    ///
    /// After computing all triangle intersections and creating the intersection geometry,
    /// the final mesh construction failed. This wraps errors from [`TriMeshBuilderError`]
    /// and typically indicates:
    /// - The intersection resulted in invalid mesh topology
    /// - No triangles in the intersection (meshes don't overlap)
    /// - Topology violations in the computed intersection
    ///
    /// See [`TriMeshBuilderError`] for details on the specific failure.
    TriMeshBuilderError(TriMeshBuilderError),
}

impl fmt::Display for MeshIntersectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingTopology => f.write_str("at least one of the meshes is missing its topology information. Ensure that the `TriMeshFlags::ORIENTED` flag is enabled on both meshes."),
            Self::MissingPseudoNormals => f.write_str("at least one of the meshes is missing its pseudo-normals. Ensure that the `TriMeshFlags::ORIENTED` flag is enabled on both meshes."),
            Self::TriTriError => f.write_str("internal failure while intersecting two triangles"),
            Self::DuplicateVertices => f.write_str("internal failure while merging faces resulting from intersections"),
            Self::TriangulationError => f.write_str("internal failure while triangulating an intersection face"),
            Self::TriMeshBuilderError(err) => write!(f, "TriMeshBuilderError: {err}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for MeshIntersectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::TriMeshBuilderError(err) => Some(err),
            _ => None,
        }
    }
}

impl From<TriMeshBuilderError> for MeshIntersectionError {
    fn from(value: TriMeshBuilderError) -> Self {
        MeshIntersectionError::TriMeshBuilderError(value)
    }
}

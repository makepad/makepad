/// An identifier of a geometric feature (vertex, edge, or face) of a shape.
///
/// Feature IDs are used throughout Parry to identify specific geometric features on shapes
/// during collision detection, contact generation, and other geometric queries. They allow
/// algorithms to track which parts of shapes are interacting, which is essential for:
///
/// - **Contact manifold generation**: Tracking persistent contact points between frames
/// - **Collision response**: Determining which features are colliding
/// - **Debug visualization**: Highlighting specific geometric elements
/// - **Feature-based queries**: Retrieving geometric data for specific shape features
///
/// # Feature Types
///
/// - **Vertex**: A corner point of the shape (0-dimensional feature)
/// - **Edge**: A line segment connecting two vertices (1-dimensional feature, 3D only)
/// - **Face**: A flat surface bounded by edges (2-dimensional feature)
/// - **Unknown**: Used when the feature type cannot be determined or is not applicable
///
/// # Shape-Specific Identifiers
///
/// The numeric ID within each feature type is shape-dependent. For example:
/// - For a cuboid, vertex IDs might range from 0-7 (8 corners)
/// - For a triangle, face ID 0 typically refers to the triangle itself
/// - For composite shapes, IDs might encode both the sub-shape and the feature within it
///
/// The exact meaning of these IDs depends on the shape's internal representation, but they
/// are guaranteed to allow efficient retrieval of the feature's geometric information.
///
/// # Examples
///
/// Basic usage of feature IDs in 2D:
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::shape::FeatureId;
///
/// // Create a vertex feature identifier
/// let vertex_id = FeatureId::Vertex(5);
/// assert_eq!(vertex_id.unwrap_vertex(), 5);
///
/// // Create a face feature identifier (in 2D, faces are edges of the polygon)
/// let face_id = FeatureId::Face(2);
/// assert_eq!(face_id.unwrap_face(), 2);
///
/// // Unknown feature (used as default)
/// let unknown = FeatureId::Unknown;
/// assert_eq!(unknown, FeatureId::default());
/// # }
/// ```
///
/// Basic usage of feature IDs in 3D:
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::FeatureId;
///
/// // Create a vertex feature identifier
/// let vertex_id = FeatureId::Vertex(5);
/// assert_eq!(vertex_id.unwrap_vertex(), 5);
///
/// // Create an edge feature identifier (only available in 3D)
/// let edge_id = FeatureId::Edge(3);
/// assert_eq!(edge_id.unwrap_edge(), 3);
///
/// // Create a face feature identifier
/// let face_id = FeatureId::Face(2);
/// assert_eq!(face_id.unwrap_face(), 2);
///
/// // Unknown feature (used as default)
/// let unknown = FeatureId::Unknown;
/// assert_eq!(unknown, FeatureId::default());
/// # }
/// ```
///
/// Pattern matching on feature types in 3D:
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::FeatureId;
///
/// fn describe_feature(feature: FeatureId) -> String {
///     match feature {
///         FeatureId::Vertex(id) => format!("Vertex #{}", id),
///         FeatureId::Edge(id) => format!("Edge #{}", id),
///         FeatureId::Face(id) => format!("Face #{}", id),
///         FeatureId::Unknown => "Unknown feature".to_string(),
///     }
/// }
///
/// assert_eq!(describe_feature(FeatureId::Vertex(3)), "Vertex #3");
/// assert_eq!(describe_feature(FeatureId::Edge(5)), "Edge #5");
/// assert_eq!(describe_feature(FeatureId::Face(1)), "Face #1");
/// # }
/// ```
///
/// # 2D vs 3D
///
/// In 2D mode (`dim2` feature), the `Edge` variant is not available since edges in 2D
/// are effectively the same as faces (line segments). In 2D:
/// - Vertices represent corner points
/// - Faces represent edges of the polygon
///
/// In 3D mode (`dim3` feature), all three types are available:
/// - Vertices are 0D points
/// - Edges are 1D line segments
/// - Faces are 2D polygons
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, Default)]
pub enum FeatureId {
    /// Shape-dependent identifier of a vertex (0-dimensional corner point).
    ///
    /// The numeric ID is specific to each shape type and allows efficient lookup
    /// of the vertex's position and other geometric properties.
    Vertex(u32),
    #[cfg(feature = "dim3")]
    /// Shape-dependent identifier of an edge (1-dimensional line segment).
    ///
    /// Available only in 3D mode. The numeric ID is specific to each shape type
    /// and allows efficient lookup of the edge's endpoints and direction.
    Edge(u32),
    /// Shape-dependent identifier of a face (2-dimensional flat surface).
    ///
    /// In 2D, faces represent the edges of polygons (line segments).
    /// In 3D, faces represent polygonal surfaces. The numeric ID is specific
    /// to each shape type and allows efficient lookup of the face's vertices,
    /// normal vector, and other properties.
    Face(u32),
    // XXX: remove this variant.
    /// Unknown or unidentified feature.
    ///
    /// Used as a default value or when the specific feature cannot be determined.
    /// This variant should generally be avoided in production code.
    #[default]
    Unknown,
}

impl FeatureId {
    /// Retrieves the numeric ID if this is a vertex feature.
    ///
    /// # Panics
    ///
    /// Panics if the feature is not a vertex (i.e., if it's an edge, face, or unknown).
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::FeatureId;
    ///
    /// let vertex = FeatureId::Vertex(42);
    /// assert_eq!(vertex.unwrap_vertex(), 42);
    /// # }
    /// ```
    ///
    /// This will panic:
    ///
    /// ```no_run
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::FeatureId;
    ///
    /// let face = FeatureId::Face(5);
    /// face.unwrap_vertex(); // Panics!
    /// # }
    /// # #[cfg(all(feature = "dim2", feature = "f64"))] {
    /// use parry2d_f64::shape::FeatureId;
    ///
    /// let face = FeatureId::Face(5);
    /// face.unwrap_vertex(); // Panics!
    /// # }
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::FeatureId;
    ///
    /// let face = FeatureId::Face(5);
    /// face.unwrap_vertex(); // Panics!
    /// # }
    /// # #[cfg(all(feature = "dim3", feature = "f64"))] {
    /// use parry3d_f64::shape::FeatureId;
    ///
    /// let face = FeatureId::Face(5);
    /// face.unwrap_vertex(); // Panics!
    /// # }
    /// ```
    pub fn unwrap_vertex(self) -> u32 {
        match self {
            FeatureId::Vertex(id) => id,
            _ => panic!("The feature id does not identify a vertex."),
        }
    }

    /// Retrieves the numeric ID if this is an edge feature.
    ///
    /// Available only in 3D mode (`dim3` feature).
    ///
    /// # Panics
    ///
    /// Panics if the feature is not an edge (i.e., if it's a vertex, face, or unknown).
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::FeatureId;
    ///
    /// let edge = FeatureId::Edge(7);
    /// assert_eq!(edge.unwrap_edge(), 7);
    /// # }
    /// ```
    ///
    /// This will panic:
    ///
    /// ```no_run
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::FeatureId;
    ///
    /// let vertex = FeatureId::Vertex(3);
    /// vertex.unwrap_edge(); // Panics!
    /// # }
    /// ```
    #[cfg(feature = "dim3")]
    pub fn unwrap_edge(self) -> u32 {
        match self {
            FeatureId::Edge(id) => id,
            _ => panic!("The feature id does not identify an edge."),
        }
    }

    /// Retrieves the numeric ID if this is a face feature.
    ///
    /// # Panics
    ///
    /// Panics if the feature is not a face (i.e., if it's a vertex, edge, or unknown).
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::FeatureId;
    ///
    /// let face = FeatureId::Face(12);
    /// assert_eq!(face.unwrap_face(), 12);
    /// # }
    /// ```
    ///
    /// This will panic:
    ///
    /// ```should_panic
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::FeatureId;
    ///
    /// let vertex = FeatureId::Vertex(0);
    /// vertex.unwrap_face(); // Panics!
    /// # }
    /// # #[cfg(all(feature = "dim2", feature = "f64"))] {
    /// use parry2d_f64::shape::FeatureId;
    ///
    /// let vertex = FeatureId::Vertex(0);
    /// vertex.unwrap_face(); // Panics!
    /// # }
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::FeatureId;
    ///
    /// let vertex = FeatureId::Vertex(0);
    /// vertex.unwrap_face(); // Panics!
    /// # }
    /// # #[cfg(all(feature = "dim3", feature = "f64"))] {
    /// use parry3d_f64::shape::FeatureId;
    ///
    /// let vertex = FeatureId::Vertex(0);
    /// vertex.unwrap_face(); // Panics!
    /// # }
    /// ```
    pub fn unwrap_face(self) -> u32 {
        match self {
            FeatureId::Face(id) => id,
            _ => panic!("The feature id does not identify a face."),
        }
    }
}

/// A memory-efficient feature ID where the type and index are packed into a single `u32`.
///
/// `PackedFeatureId` is a space-optimized version of [`FeatureId`] that encodes both the
/// feature type (vertex, edge, or face) and its numeric identifier in a single 32-bit value.
/// This is particularly useful when storing large numbers of feature IDs, as it uses half
/// the memory of a standard enum representation.
///
/// # Memory Layout
///
/// The packing scheme uses the upper 2 bits to encode the feature type, leaving 30 bits
/// (0-1,073,741,823) for the feature index:
///
/// ```text
/// ┌──┬──┬────────────────────────────────┐
/// │31│30│29                              0│
/// ├──┴──┴────────────────────────────────┤
/// │Type │        Feature Index           │
/// │(2b) │           (30 bits)            │
/// └─────┴────────────────────────────────┘
///
/// Type encoding:
/// - 00: Unknown
/// - 01: Vertex
/// - 10: Edge (3D only)
/// - 11: Face
/// ```
///
/// # Use Cases
///
/// Use `PackedFeatureId` when:
/// - Storing feature IDs in large data structures (e.g., contact manifolds)
/// - Passing feature IDs across FFI boundaries where a fixed size is required
/// - Memory usage is a concern and you have many feature IDs
///
/// Use regular [`FeatureId`] when:
/// - Code clarity is more important than memory usage
/// - You need to pattern match on feature types frequently
/// - Working with small numbers of feature IDs
///
/// # Examples
///
/// Creating and unpacking feature IDs in 2D:
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::shape::{FeatureId, PackedFeatureId};
///
/// // Create a packed vertex ID
/// let packed_vertex = PackedFeatureId::vertex(10);
/// assert!(packed_vertex.is_vertex());
/// assert!(!packed_vertex.is_face());
///
/// // Create a packed face ID
/// let packed_face = PackedFeatureId::face(5);
/// assert!(packed_face.is_face());
///
/// // Unpack to get the full enum
/// let unpacked = packed_face.unpack();
/// assert_eq!(unpacked, FeatureId::Face(5));
/// # }
/// ```
///
/// Creating and unpacking feature IDs in 3D:
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::{FeatureId, PackedFeatureId};
///
/// // Create a packed vertex ID
/// let packed_vertex = PackedFeatureId::vertex(10);
/// assert!(packed_vertex.is_vertex());
/// assert!(!packed_vertex.is_face());
///
/// // Create a packed edge ID (3D only)
/// let packed_edge = PackedFeatureId::edge(7);
/// assert!(packed_edge.is_edge());
///
/// // Create a packed face ID
/// let packed_face = PackedFeatureId::face(5);
/// assert!(packed_face.is_face());
///
/// // Unpack to get the full enum
/// let unpacked = packed_face.unpack();
/// assert_eq!(unpacked, FeatureId::Face(5));
/// # }
/// ```
///
/// Converting between packed and unpacked forms:
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::shape::{FeatureId, PackedFeatureId};
///
/// // From FeatureId to PackedFeatureId
/// let feature = FeatureId::Vertex(42);
/// let packed: PackedFeatureId = feature.into();
/// assert!(packed.is_vertex());
///
/// // From PackedFeatureId back to FeatureId
/// let unpacked = packed.unpack();
/// assert_eq!(unpacked, FeatureId::Vertex(42));
/// # }
/// ```
///
/// Working with the unknown feature:
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::shape::PackedFeatureId;
///
/// let unknown = PackedFeatureId::UNKNOWN;
/// assert!(unknown.is_unknown());
/// assert!(!unknown.is_vertex());
/// assert!(!unknown.is_face());
/// # }
/// ```
///
/// Checking feature types efficiently in 3D:
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::PackedFeatureId;
///
/// let vertex = PackedFeatureId::vertex(100);
/// let edge = PackedFeatureId::edge(50);
/// let face = PackedFeatureId::face(25);
///
/// // Type checking is very fast (just bit masking)
/// assert!(vertex.is_vertex());
/// assert!(edge.is_edge());
/// assert!(face.is_face());
///
/// // Different types are not equal
/// assert_ne!(vertex, edge);
/// assert_ne!(edge, face);
/// # }
/// ```
///
/// # Performance
///
/// `PackedFeatureId` provides several performance benefits:
/// - **Memory**: Uses 4 bytes vs 8 bytes for `FeatureId` (on 64-bit systems)
/// - **Cache efficiency**: Better cache utilization when storing many IDs
/// - **Type checking**: Very fast (single bitwise AND operation)
/// - **Conversion**: Converting to/from `FeatureId` is essentially free
///
/// # Limitations
///
/// The packing scheme limits feature indices to 30 bits (max value: 1,073,741,823).
/// Attempting to create a packed feature ID with a larger index will panic in debug
/// mode due to the assertion checks in the constructor methods.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub struct PackedFeatureId(pub u32);

impl PackedFeatureId {
    /// Constant representing an unknown or unidentified feature.
    ///
    /// This is the default value and corresponds to `FeatureId::Unknown`.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::{PackedFeatureId, FeatureId};
    ///
    /// let unknown = PackedFeatureId::UNKNOWN;
    /// assert!(unknown.is_unknown());
    /// assert_eq!(unknown.unpack(), FeatureId::Unknown);
    /// # }
    /// ```
    pub const UNKNOWN: Self = Self(0);

    const CODE_MASK: u32 = 0x3fff_ffff;
    const HEADER_MASK: u32 = !Self::CODE_MASK;
    const HEADER_VERTEX: u32 = 0b01 << 30;
    #[cfg(feature = "dim3")]
    const HEADER_EDGE: u32 = 0b10 << 30;
    const HEADER_FACE: u32 = 0b11 << 30;

    /// Creates a packed feature ID for a vertex with the given index.
    ///
    /// # Panics
    ///
    /// Panics in debug mode if `code` uses any of the upper 2 bits (i.e., if `code >= 2^30`).
    /// The maximum valid value is 1,073,741,823 (0x3FFFFFFF).
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::{PackedFeatureId, FeatureId};
    ///
    /// let packed = PackedFeatureId::vertex(5);
    /// assert!(packed.is_vertex());
    /// assert_eq!(packed.unpack(), FeatureId::Vertex(5));
    /// # }
    /// ```
    pub fn vertex(code: u32) -> Self {
        assert_eq!(code & Self::HEADER_MASK, 0);
        Self(Self::HEADER_VERTEX | code)
    }

    /// Creates a packed feature ID for an edge with the given index.
    ///
    /// Available only in 3D mode (`dim3` feature).
    ///
    /// # Panics
    ///
    /// Panics in debug mode if `code` uses any of the upper 2 bits (i.e., if `code >= 2^30`).
    /// The maximum valid value is 1,073,741,823 (0x3FFFFFFF).
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::{PackedFeatureId, FeatureId};
    ///
    /// let packed = PackedFeatureId::edge(10);
    /// assert!(packed.is_edge());
    /// assert_eq!(packed.unpack(), FeatureId::Edge(10));
    /// # }
    /// ```
    #[cfg(feature = "dim3")]
    pub fn edge(code: u32) -> Self {
        assert_eq!(code & Self::HEADER_MASK, 0);
        Self(Self::HEADER_EDGE | code)
    }

    /// Creates a packed feature ID for a face with the given index.
    ///
    /// # Panics
    ///
    /// Panics in debug mode if `code` uses any of the upper 2 bits (i.e., if `code >= 2^30`).
    /// The maximum valid value is 1,073,741,823 (0x3FFFFFFF).
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::{PackedFeatureId, FeatureId};
    ///
    /// let packed = PackedFeatureId::face(15);
    /// assert!(packed.is_face());
    /// assert_eq!(packed.unpack(), FeatureId::Face(15));
    /// # }
    /// ```
    pub fn face(code: u32) -> Self {
        assert_eq!(code & Self::HEADER_MASK, 0);
        Self(Self::HEADER_FACE | code)
    }

    #[cfg(feature = "dim2")]
    /// Converts an array of vertex feature ids into an array of packed feature ids.
    pub(crate) fn vertices(code: [u32; 2]) -> [Self; 2] {
        [Self::vertex(code[0]), Self::vertex(code[1])]
    }

    #[cfg(feature = "dim3")]
    /// Converts an array of vertex feature ids into an array of packed feature ids.
    pub(crate) fn vertices(code: [u32; 4]) -> [Self; 4] {
        [
            Self::vertex(code[0]),
            Self::vertex(code[1]),
            Self::vertex(code[2]),
            Self::vertex(code[3]),
        ]
    }

    #[cfg(feature = "dim3")]
    /// Converts an array of edge feature ids into an array of packed feature ids.
    pub(crate) fn edges(code: [u32; 4]) -> [Self; 4] {
        [
            Self::edge(code[0]),
            Self::edge(code[1]),
            Self::edge(code[2]),
            Self::edge(code[3]),
        ]
    }

    /// Unpacks this feature ID into the full `FeatureId` enum.
    ///
    /// This converts the compact packed representation back into the explicit enum form,
    /// allowing you to pattern match on the feature type.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::{FeatureId, PackedFeatureId};
    ///
    /// let packed = PackedFeatureId::vertex(42);
    /// let unpacked = packed.unpack();
    ///
    /// match unpacked {
    ///     FeatureId::Vertex(id) => assert_eq!(id, 42),
    ///     _ => panic!("Expected a vertex!"),
    /// }
    /// # }
    /// ```
    ///
    /// Round-trip conversion:
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::{FeatureId, PackedFeatureId};
    ///
    /// let original = FeatureId::Face(100);
    /// let packed: PackedFeatureId = original.into();
    /// let unpacked = packed.unpack();
    ///
    /// assert_eq!(original, unpacked);
    /// # }
    /// ```
    pub fn unpack(self) -> FeatureId {
        let header = self.0 & Self::HEADER_MASK;
        let code = self.0 & Self::CODE_MASK;
        match header {
            Self::HEADER_VERTEX => FeatureId::Vertex(code),
            #[cfg(feature = "dim3")]
            Self::HEADER_EDGE => FeatureId::Edge(code),
            Self::HEADER_FACE => FeatureId::Face(code),
            _ => FeatureId::Unknown,
        }
    }

    /// Checks if this feature ID identifies a face.
    ///
    /// This is a very fast operation (single bitwise AND and comparison).
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::PackedFeatureId;
    ///
    /// let face = PackedFeatureId::face(5);
    /// let vertex = PackedFeatureId::vertex(5);
    ///
    /// assert!(face.is_face());
    /// assert!(!vertex.is_face());
    /// # }
    /// ```
    pub fn is_face(self) -> bool {
        self.0 & Self::HEADER_MASK == Self::HEADER_FACE
    }

    /// Checks if this feature ID identifies a vertex.
    ///
    /// This is a very fast operation (single bitwise AND and comparison).
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::PackedFeatureId;
    ///
    /// let vertex = PackedFeatureId::vertex(10);
    /// let face = PackedFeatureId::face(10);
    ///
    /// assert!(vertex.is_vertex());
    /// assert!(!face.is_vertex());
    /// # }
    /// ```
    pub fn is_vertex(self) -> bool {
        self.0 & Self::HEADER_MASK == Self::HEADER_VERTEX
    }

    /// Checks if this feature ID identifies an edge.
    ///
    /// Available only in 3D mode (`dim3` feature).
    ///
    /// This is a very fast operation (single bitwise AND and comparison).
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::PackedFeatureId;
    ///
    /// let edge = PackedFeatureId::edge(7);
    /// let vertex = PackedFeatureId::vertex(7);
    ///
    /// assert!(edge.is_edge());
    /// assert!(!vertex.is_edge());
    /// # }
    /// ```
    #[cfg(feature = "dim3")]
    pub fn is_edge(self) -> bool {
        self.0 & Self::HEADER_MASK == Self::HEADER_EDGE
    }

    /// Checks if this feature ID is unknown.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::PackedFeatureId;
    ///
    /// let unknown = PackedFeatureId::UNKNOWN;
    /// let vertex = PackedFeatureId::vertex(0);
    ///
    /// assert!(unknown.is_unknown());
    /// assert!(!vertex.is_unknown());
    /// # }
    /// ```
    pub fn is_unknown(self) -> bool {
        self == Self::UNKNOWN
    }
}

impl From<FeatureId> for PackedFeatureId {
    /// Converts a `FeatureId` into its packed representation.
    ///
    /// This is a lossless conversion that encodes the feature type and index
    /// into a single `u32` value.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::{FeatureId, PackedFeatureId};
    ///
    /// // Explicit conversion
    /// let feature = FeatureId::Vertex(123);
    /// let packed = PackedFeatureId::from(feature);
    /// assert!(packed.is_vertex());
    ///
    /// // Using Into trait
    /// let feature = FeatureId::Face(456);
    /// let packed: PackedFeatureId = feature.into();
    /// assert!(packed.is_face());
    ///
    /// // Round-trip conversion preserves the value
    /// let original = FeatureId::Vertex(789);
    /// let packed: PackedFeatureId = original.into();
    /// assert_eq!(packed.unpack(), original);
    /// # }
    /// ```
    fn from(value: FeatureId) -> Self {
        match value {
            FeatureId::Face(fid) => Self::face(fid),
            #[cfg(feature = "dim3")]
            FeatureId::Edge(fid) => Self::edge(fid),
            FeatureId::Vertex(fid) => Self::vertex(fid),
            FeatureId::Unknown => Self::UNKNOWN,
        }
    }
}

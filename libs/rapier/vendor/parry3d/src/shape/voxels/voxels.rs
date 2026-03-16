use super::{
    VoxelIndex, EMPTY_FACE_MASK, FACES_TO_FEATURE_MASKS, FACES_TO_OCTANT_MASKS,
    FACES_TO_VOXEL_TYPES, INTERIOR_FACE_MASK,
};
use crate::bounding_volume::{Aabb, BoundingVolume};
#[cfg(not(feature = "std"))]
use crate::math::ComplexField;
use crate::math::{ivect_to_vect, vect_to_ivect, IVector, Int, Vector};
use crate::partitioning::{Bvh, BvhBuildStrategy, BvhNode};
use crate::shape::voxels::voxels_chunk::{VoxelsChunk, VoxelsChunkHeader};
use crate::shape::VoxelsChunkRef;
use crate::utils::hashmap::HashMap;
use alloc::{vec, vec::Vec};

/// Categorization of a voxel based on its neighbors.
///
/// This enum describes the local topology of a filled voxel by examining which of its
/// immediate neighbors (along coordinate axes) are also filled. This information is crucial
/// for collision detection to avoid the "internal edges problem" where objects can get
/// caught on edges between adjacent voxels.
///
/// # Type Classification
///
/// - **Empty**: The voxel is not filled.
/// - **Interior**: All axis-aligned neighbors are filled (safest for collision).
/// - **Face**: Missing neighbors in one coordinate direction (e.g., top face exposed).
/// - **Edge** (3D only): Missing neighbors in two coordinate directions (e.g., corner edge exposed).
/// - **Vertex**: Missing neighbors in all coordinate directions (isolated corner).
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::{Voxels, VoxelType};
/// use parry3d::math::{Vector, IVector};
///
/// // Create a small 2x2x2 cube of voxels
/// let voxels = Voxels::new(
///     Vector::new(1.0, 1.0, 1.0),
///     &[
///         IVector::new(0, 0, 0),
///         IVector::new(1, 0, 0),
///         IVector::new(0, 1, 0),
///         IVector::new(1, 1, 0),
///         IVector::new(0, 0, 1),
///         IVector::new(1, 0, 1),
///         IVector::new(0, 1, 1),
///         IVector::new(1, 1, 1),
///     ],
/// );
///
/// // Check voxel type - interior voxels are fully surrounded
/// let state = voxels.voxel_state(IVector::new(0, 0, 0)).unwrap();
/// let voxel_type = state.voxel_type();
/// println!("Voxel type: {:?}", voxel_type);
/// # }
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VoxelType {
    /// The voxel is empty.
    Empty,
    /// The voxel is a vertex if all three coordinate axis directions have at
    /// least one empty neighbor.
    Vertex,
    /// The voxel is on an edge if it has non-empty neighbors in both directions of
    /// a single coordinate axis.
    #[cfg(feature = "dim3")]
    Edge,
    /// The voxel is on an edge if it has non-empty neighbors in both directions of
    /// two coordinate axes.
    Face,
    /// The voxel is on an edge if it has non-empty neighbors in both directions of
    /// all coordinate axes.
    Interior,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
/// The status of the cell of an heightfield.
pub struct AxisMask(u8);

bitflags::bitflags! {
    /// Flags for identifying signed directions along coordinate axes, or faces of a voxel.
    impl AxisMask: u8 {
        /// The direction or face along the `+x` coordinate axis.
        const X_POS = 1 << 0;
        /// The direction or face along the `-x` coordinate axis.
        const X_NEG = 1 << 1;
        /// The direction or face along the `+y` coordinate axis.
        const Y_POS = 1 << 2;
        /// The direction or face along the `-y` coordinate axis.
        const Y_NEG = 1 << 3;
        /// The direction or face along the `+z` coordinate axis.
        #[cfg(feature= "dim3")]
        const Z_POS = 1 << 4;
        /// The direction or face along the `-z` coordinate axis.
        #[cfg(feature= "dim3")]
        const Z_NEG = 1 << 5;
    }
}

/// Indicates the local shape of a voxel on each octant.
///
/// This provides geometric information of the shape’s exposed features on each octant.
// This is an alternative to `FACES_TO_FEATURE_MASKS` that can be more convenient for some
// collision-detection algorithms.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OctantPattern;

// NOTE: it is important that the max value of any OctantPattern variant
//       is 7 because we don’t allocate more than 3 bits to store it in
//      `FACES_TO_OCTANT_MASKS`.
/// Indicates the local shape of a voxel on each octant.
///
/// This provides geometric information of the shape’s exposed features on each octant.
// This is an alternative to `FACES_TO_FEATURE_MASKS` that can be more convenient for some
// collision-detection algorithms.
#[cfg(feature = "dim3")]
impl OctantPattern {
    /// The voxel doesn't have any exposed feature on the octant with this mask.
    pub const INTERIOR: u32 = 0;
    /// The voxel has an exposed vertex on the octant with this mask.
    pub const VERTEX: u32 = 1;
    /// The voxel has an exposed edges with direction X on the octant with this mask.
    pub const EDGE_X: u32 = 2;
    /// The voxel has an exposed edges with direction Y on the octant with this mask.
    pub const EDGE_Y: u32 = 3;
    /// The voxel has an exposed edges with direction Z on the octant with this mask.
    pub const EDGE_Z: u32 = 4;
    /// The voxel has an exposed face with normal X on the octant with this mask.
    pub const FACE_X: u32 = 5;
    /// The voxel has an exposed face with normal Y on the octant with this mask.
    pub const FACE_Y: u32 = 6;
    /// The voxel has an exposed face with normal Z on the octant with this mask.
    pub const FACE_Z: u32 = 7;
}

// NOTE: it is important that the max value of any OctantPattern variant
//       is 7 because we don’t allocate more than 3 bits to store it in
//      `FACES_TO_OCTANT_MASKS`.
/// Indicates the local shape of a voxel on each octant.
///
/// This provides geometric information of the shape’s exposed features on each octant.
/// This is an alternative to `FACES_TO_FEATURE_MASKS` that can be more convenient for some
/// collision-detection algorithms.
#[cfg(feature = "dim2")]
impl OctantPattern {
    /// The voxel doesn't have any exposed feature on the octant with this mask.
    pub const INTERIOR: u32 = 0;
    /// The voxel has an exposed vertex on the octant with this mask.
    pub const VERTEX: u32 = 1;
    /// The voxel has an exposed face with normal X on the octant with this mask.
    pub const FACE_X: u32 = 2;
    /// The voxel has an exposed face with normal Y on the octant with this mask.
    pub const FACE_Y: u32 = 3;
}

// The local neighborhood information is encoded in a 8-bits number in groups of two bits
// per coordinate axis: `0bwwzzyyxx`. In each group of two bits, e.g. `xx`, the rightmost (resp.
// leftmost) bit set to 1 means that the neighbor voxel with coordinate `+1` (resp `-1`) relative
// to the current voxel along the `x` axis is filled. If the bit is 0, then the corresponding
// neighbor is empty. See the `AxisMask` bitflags.
// For example, in 2D, the mask `0b00_00_10_01` matches the following configuration (assuming +y goes
// up, and +x goes right):
//
// ```txt
//  0 0 0
//  0 x 1
//  0 1 0
// ```
//
// The special value `0b01000000` indicates that the voxel is empty.
// And the value `0b00111111` (`0b00001111` in 2D) indicates that the voxel is an interior voxel (its whole neighborhood
// is filled).
/// A description of the local neighborhood of a voxel.
///
/// This compact representation stores which immediate neighbors (along coordinate axes) of a
/// voxel are filled. This information is essential for proper collision detection between voxels
/// and other shapes, as it helps avoid the "internal edges problem."
///
/// The state is encoded as a single byte where each pair of bits represents one coordinate axis,
/// indicating whether neighbors in the positive and negative directions are filled.
///
/// # Special States
///
/// - [`VoxelState::EMPTY`]: The voxel itself is empty (not part of the shape).
/// - [`VoxelState::INTERIOR`]: All neighbors are filled (completely surrounded voxel).
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::{Voxels, VoxelState, AxisMask};
/// use parry3d::math::{Vector, IVector};
///
/// // Create a simple voxel shape
/// let voxels = Voxels::new(
///     Vector::new(1.0, 1.0, 1.0),
///     &[IVector::new(0, 0, 0), IVector::new(1, 0, 0)],
/// );
///
/// // Query the state of a voxel
/// let state = voxels.voxel_state(IVector::new(0, 0, 0)).unwrap();
///
/// // Check if empty
/// assert!(!state.is_empty());
///
/// // Get which faces are exposed (not adjacent to other voxels)
/// let free_faces = state.free_faces();
/// if free_faces.contains(AxisMask::X_NEG) {
///     println!("The -X face is exposed");
/// }
///
/// // Get the voxel type based on neighborhood
/// println!("Voxel type: {:?}", state.voxel_type());
/// # }
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub struct VoxelState(pub(super) u8);

impl VoxelState {
    /// The value of empty voxels.
    pub const EMPTY: VoxelState = VoxelState(EMPTY_FACE_MASK);
    /// The value of a voxel with non-empty neighbors in all directions.
    pub const INTERIOR: VoxelState = VoxelState(INTERIOR_FACE_MASK);

    pub(crate) const fn new(state: u8) -> Self {
        Self(state)
    }

    /// Is this voxel empty?
    pub const fn is_empty(self) -> bool {
        self.0 == EMPTY_FACE_MASK
    }

    /// A bit mask indicating which faces of the voxel don’t have any
    /// adjacent non-empty voxel.
    pub const fn free_faces(self) -> AxisMask {
        if self.0 == INTERIOR_FACE_MASK || self.0 == EMPTY_FACE_MASK {
            AxisMask::empty()
        } else {
            AxisMask::from_bits_truncate((!self.0) & INTERIOR_FACE_MASK)
        }
    }

    /// The [`VoxelType`] of this voxel.
    pub const fn voxel_type(self) -> VoxelType {
        FACES_TO_VOXEL_TYPES[self.0 as usize]
    }

    // Bitmask indicating what vertices, edges, or faces of the voxel are "free".
    pub(crate) const fn feature_mask(self) -> u16 {
        FACES_TO_FEATURE_MASKS[self.0 as usize]
    }

    pub(crate) const fn octant_mask(self) -> u32 {
        FACES_TO_OCTANT_MASKS[self.0 as usize]
    }
}

/// Information associated to a voxel.
///
/// This structure provides complete information about a single voxel including its position
/// in both grid coordinates and world space, as well as its state (empty/filled and neighborhood).
///
/// # Note
///
/// The `linear_id` field is an internal implementation detail that can become invalidated when
/// the voxel structure is modified (e.g., via [`Voxels::set_voxel`] or [`Voxels::crop`]).
/// For stable references to voxels, always use `grid_coords`.
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Voxels;
/// use parry3d::math::{Vector, IVector};
///
/// let voxels = Voxels::new(
///     Vector::new(0.5, 0.5, 0.5),
///     &[IVector::new(0, 0, 0), IVector::new(1, 0, 0)],
/// );
///
/// // Iterate through all voxels
/// for voxel in voxels.voxels() {
///     if !voxel.state.is_empty() {
///         println!("Voxel at grid position {:?}", voxel.grid_coords);
///         println!("  World center: {:?}", voxel.center);
///         println!("  Type: {:?}", voxel.state.voxel_type());
///     }
/// }
/// # }
/// ```
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct VoxelData {
    /// The temporary index in the internal voxels' storage.
    ///
    /// This index can be invalidated after a call to [`Voxels::set_voxel`], or
    /// [`Voxels::crop`].
    pub linear_id: VoxelIndex,
    /// The voxel's integer grid coordinates.
    pub grid_coords: IVector,
    /// The voxel's center position in the local-space of the [`Voxels`] shape it is part of.
    pub center: Vector,
    /// The voxel's state, indicating if it's empty or full.
    pub state: VoxelState,
}

/// A shape made of axis-aligned, uniformly sized cubes (aka. voxels).
///
/// # What are Voxels?
///
/// Voxels (volumetric pixels) are 3D cubes (or 2D squares) arranged on a regular grid. Think of
/// them as 3D building blocks, like LEGO bricks or Minecraft blocks. Each voxel has:
/// - A position on an integer grid (e.g., `(0, 0, 0)`, `(1, 2, 3)`)
/// - A uniform size (e.g., 1.0 × 1.0 × 1.0 meters)
/// - A state: filled (solid) or empty (air)
///
/// # When to Use Voxels?
///
/// Voxels are ideal for:
/// - **Minecraft-style worlds**: Block-based terrain and structures
/// - **Destructible environments**: Easy to add/remove individual blocks
/// - **Procedural generation**: Grid-based algorithms for caves, terrain, dungeons
/// - **Volumetric data**: Medical imaging, scientific simulations
/// - **Retro aesthetics**: Pixel art style in 3D
///
/// Voxels may NOT be ideal for:
/// - Smooth organic shapes (use meshes instead)
/// - Very large sparse worlds (consider octrees or chunk-based systems)
/// - Scenes requiring fine geometric detail at all scales
///
/// # The Internal Edges Problem
///
/// When an object slides across a flat surface made of voxels, it can snag on the edges between
/// adjacent voxels, causing jerky motion. Parry's `Voxels` shape solves this by tracking neighbor
/// relationships: it knows which voxel faces are internal (adjacent to another voxel) vs external
/// (exposed to air), allowing smooth collision response.
///
/// # Memory Efficiency
///
/// The internal storage uses sparse chunks, storing only one byte per voxel for neighborhood
/// information. Empty regions consume minimal memory. This is much more efficient than storing
/// a triangle mesh representation of all voxel surfaces.
///
/// # Examples
///
/// ## Basic Usage: Creating a Voxel Shape
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Voxels;
/// use parry3d::math::{Vector, IVector};
///
/// // Create a simple 3×3×3 cube of voxels
/// let voxel_size = Vector::new(1.0, 1.0, 1.0);
/// let mut coords = Vec::new();
/// for x in 0..3 {
///     for y in 0..3 {
///         for z in 0..3 {
///             coords.push(IVector::new(x, y, z));
///         }
///     }
/// }
///
/// let voxels = Voxels::new(voxel_size, &coords);
/// println!("Created voxel shape with {} voxels", coords.len());
/// # }
/// ```
///
/// ## Creating Voxels from World-Space Vectors
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Voxels;
/// use parry3d::math::Vector;
///
/// // Sample points in world space (e.g., from a point cloud)
/// let points = vec![
///     Vector::new(0.1, 0.2, 0.3),
///     Vector::new(1.5, 2.1, 3.7),
///     Vector::new(0.8, 0.9, 1.2),
/// ];
///
/// // Create voxels with 0.5 unit size - nearby points merge into same voxel
/// let voxels = Voxels::from_points(Vector::new(0.5, 0.5, 0.5), &points);
/// println!("Created voxel shape from {} points", points.len());
/// # }
/// ```
///
/// ## Querying Voxel State
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Voxels;
/// use parry3d::math::{Vector, IVector};
///
/// let voxels = Voxels::new(
///     Vector::new(1.0, 1.0, 1.0),
///     &[IVector::new(0, 0, 0), IVector::new(1, 0, 0)],
/// );
///
/// // Check if a specific grid position is filled
/// if let Some(state) = voxels.voxel_state(IVector::new(0, 0, 0)) {
///     println!("Voxel is filled!");
///     println!("Type: {:?}", state.voxel_type());
///     println!("Free faces: {:?}", state.free_faces());
/// } else {
///     println!("Voxel is empty or outside the domain");
/// }
///
/// // Convert world-space point to grid coordinates
/// let world_point = Vector::new(1.3, 0.7, 0.2);
/// let grid_coord = voxels.voxel_at_point(world_point);
/// println!("Vector at {:?} is in voxel {:?}", world_point, grid_coord);
/// # }
/// ```
///
/// ## Iterating Through Voxels
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Voxels;
/// use parry3d::math::{Vector, IVector};
///
/// let voxels = Voxels::new(
///     Vector::new(0.5, 0.5, 0.5),
///     &[IVector::new(0, 0, 0), IVector::new(1, 0, 0), IVector::new(0, 1, 0)],
/// );
///
/// // Iterate through all non-empty voxels
/// for voxel in voxels.voxels() {
///     if !voxel.state.is_empty() {
///         println!("Voxel at grid {:?}, world center {:?}",
///                  voxel.grid_coords, voxel.center);
///     }
/// }
/// # }
/// ```
///
/// ## Modifying Voxels Dynamically
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Voxels;
/// use parry3d::math::{Vector, IVector};
///
/// let mut voxels = Voxels::new(
///     Vector::new(1.0, 1.0, 1.0),
///     &[IVector::new(0, 0, 0)],
/// );
///
/// // Add a new voxel
/// voxels.set_voxel(IVector::new(1, 0, 0), true);
///
/// // Remove a voxel
/// voxels.set_voxel(IVector::new(0, 0, 0), false);
///
/// // Check the result
/// assert!(voxels.voxel_state(IVector::new(0, 0, 0)).unwrap().is_empty());
/// assert!(!voxels.voxel_state(IVector::new(1, 0, 0)).unwrap().is_empty());
/// # }
/// ```
///
/// ## Spatial Queries
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Voxels;
/// use parry3d::bounding_volume::Aabb;
/// use parry3d::math::{Vector, IVector};
///
/// let voxels = Voxels::new(
///     Vector::new(1.0, 1.0, 1.0),
///     &[IVector::new(0, 0, 0), IVector::new(1, 0, 0), IVector::new(2, 0, 0)],
/// );
///
/// // Find voxels intersecting an AABB
/// let query_aabb = Aabb::new(Vector::new(-0.5, -0.5, -0.5), Vector::new(1.5, 1.5, 1.5));
/// let count = voxels.voxels_intersecting_local_aabb(&query_aabb)
///     .filter(|v| !v.state.is_empty())
///     .count();
/// println!("Found {} voxels in AABB", count);
///
/// // Get the overall domain bounds
/// let [mins, maxs] = voxels.domain();
/// println!("Voxel grid spans from {:?} to {:?}", mins, maxs);
/// # }
/// ```
///
/// # See Also
///
/// - [`VoxelState`]: Information about a voxel's neighbors
/// - [`VoxelType`]: Classification of voxels by their topology
/// - [`VoxelData`]: Complete information about a single voxel
/// - [`crate::transformation::voxelization`]: Convert meshes to voxels
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub struct Voxels {
    /// A BVH of chunk keys.
    ///
    /// The bounding boxes are the ones of the chunk’s voxels **keys**. This is equivalent to a bvh
    /// of the chunks with a uniform voxel size of 1.
    pub(super) chunk_bvh: Bvh,
    pub(super) chunk_headers: HashMap<IVector, VoxelsChunkHeader>,
    pub(super) chunk_keys: Vec<IVector>,
    pub(super) chunks: Vec<VoxelsChunk>,
    pub(super) free_chunks: Vec<usize>,
    pub(super) voxel_size: Vector,
}

impl Voxels {
    /// Initializes a voxel shape from grid coordinates.
    ///
    /// This is the primary constructor for creating a `Voxels` shape. You provide:
    /// - `voxel_size`: The physical dimensions of each voxel (e.g., `1.0 × 1.0 × 1.0` meters)
    /// - `grid_coordinates`: Integer grid positions for each filled voxel
    ///
    /// # Coordinate System
    ///
    /// Each voxel with grid coordinates `(x, y, z)` will be positioned such that:
    /// - Its minimum corner (bottom-left-back) is at `(x, y, z) * voxel_size`
    /// - Its center is at `((x, y, z) + 0.5) * voxel_size`
    /// - Its maximum corner is at `((x, y, z) + 1) * voxel_size`
    ///
    /// For example, with `voxel_size = 2.0` and grid coord `(1, 0, 0)`:
    /// - Minimum corner: `(2.0, 0.0, 0.0)`
    /// - Center: `(3.0, 1.0, 1.0)`
    /// - Maximum corner: `(4.0, 2.0, 2.0)`
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Voxels;
    /// use parry3d::math::{Vector, IVector};
    ///
    /// // Create a 2×2×2 cube of voxels with 1.0 unit size
    /// let voxels = Voxels::new(
    ///     Vector::new(1.0, 1.0, 1.0),
    ///     &[
    ///         IVector::new(0, 0, 0), IVector::new(1, 0, 0),
    ///         IVector::new(0, 1, 0), IVector::new(1, 1, 0),
    ///         IVector::new(0, 0, 1), IVector::new(1, 0, 1),
    ///         IVector::new(0, 1, 1), IVector::new(1, 1, 1),
    ///     ],
    /// );
    ///
    /// // Verify the first voxel's center position
    /// let center = voxels.voxel_center(IVector::new(0, 0, 0));
    /// assert_eq!(center, Vector::new(0.5, 0.5, 0.5));
    /// # }
    /// ```
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Voxels;
    /// use parry3d::math::{Vector, IVector};
    ///
    /// // Create a line of voxels along the X axis
    /// let voxels = Voxels::new(
    ///     Vector::new(0.5, 0.5, 0.5),
    ///     &[IVector::new(0, 0, 0), IVector::new(1, 0, 0), IVector::new(2, 0, 0)],
    /// );
    ///
    /// // Query the domain (bounding grid coordinates)
    /// // Note: domain is aligned to internal chunk boundaries for efficiency
    /// let [mins, maxs] = voxels.domain();
    /// assert_eq!(mins, IVector::new(0, 0, 0));
    /// // maxs will be chunk-aligned (chunks are 8x8x8), so it includes more space
    /// assert!(maxs.x >= 3 && maxs.y >= 1 && maxs.z >= 1);
    /// # }
    /// ```
    pub fn new(voxel_size: Vector, grid_coordinates: &[IVector]) -> Self {
        let mut result = Self {
            chunk_bvh: Bvh::new(),
            chunk_headers: HashMap::default(),
            chunk_keys: vec![],
            chunks: vec![],
            free_chunks: vec![],
            voxel_size,
        };

        for vox in grid_coordinates {
            let (chunk_key, id_in_chunk) = Self::chunk_key_and_id_in_chunk(*vox);
            let chunk_header = result.chunk_headers.entry(chunk_key).or_insert_with(|| {
                let id = result.chunks.len();
                result.chunks.push(VoxelsChunk::default());
                result.chunk_keys.push(chunk_key);
                VoxelsChunkHeader { id, len: 0 }
            });
            chunk_header.len += 1;
            result.chunks[chunk_header.id].states[id_in_chunk] = VoxelState::INTERIOR;
        }

        result.chunk_bvh = Bvh::from_iter(
            BvhBuildStrategy::Ploc,
            result.chunk_headers.iter().map(|(chunk_key, chunk_id)| {
                (chunk_id.id, VoxelsChunk::aabb(chunk_key, result.voxel_size))
            }),
        );

        result.recompute_all_voxels_states();
        result
    }

    /// Computes a voxel shape from a set of world-space points.
    ///
    /// This constructor converts continuous world-space coordinates into discrete grid coordinates
    /// by snapping each point to the voxel grid. Multiple points can map to the same voxel.
    ///
    /// # How it Works
    ///
    /// Each point is converted to grid coordinates by:
    /// 1. Dividing the point's coordinates by `voxel_size`
    /// 2. Taking the floor to get integer grid coordinates
    /// 3. Removing duplicates (multiple points in the same voxel become one voxel)
    ///
    /// For example, with `voxel_size = 1.0`:
    /// - Vector `(0.3, 0.7, 0.9)` → Grid `(0, 0, 0)`
    /// - Vector `(1.1, 0.2, 0.5)` → Grid `(1, 0, 0)`
    /// - Vector `(0.9, 0.1, 0.8)` → Grid `(0, 0, 0)` (merges with first)
    ///
    /// # Use Cases
    ///
    /// - Converting point clouds into voxel representations
    /// - Creating voxel shapes from scattered data
    /// - Simplifying complex point sets into uniform grids
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Voxels;
    /// use parry3d::math::Vector;
    ///
    /// // Sample points in world space
    /// let points = vec![
    ///     Vector::new(0.1, 0.2, 0.3),   // → Grid (0, 0, 0)
    ///     Vector::new(0.7, 0.8, 0.9),   // → Grid (0, 0, 0) - same voxel!
    ///     Vector::new(1.2, 0.3, 0.1),   // → Grid (1, 0, 0)
    ///     Vector::new(0.5, 1.5, 0.2),   // → Grid (0, 1, 0)
    /// ];
    ///
    /// // Create voxels with 1.0 unit size
    /// let voxels = Voxels::from_points(Vector::new(1.0, 1.0, 1.0), &points);
    ///
    /// // Only 3 unique voxels created (first two points merged)
    /// let filled_count = voxels.voxels()
    ///     .filter(|v| !v.state.is_empty())
    ///     .count();
    /// assert_eq!(filled_count, 3);
    /// # }
    /// ```
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Voxels;
    /// use parry3d::math::{Vector, IVector};
    ///
    /// // Higher resolution voxelization
    /// let points = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 1.0, 1.0),
    /// ];
    ///
    /// // Smaller voxels = finer detail
    /// let voxels = Voxels::from_points(Vector::new(0.5, 0.5, 0.5), &points);
    ///
    /// // First point at grid (0,0,0), second at grid (2,2,2) due to smaller voxel size
    /// assert!(voxels.voxel_state(IVector::new(0, 0, 0)).is_some());
    /// assert!(voxels.voxel_state(IVector::new(2, 2, 2)).is_some());
    /// # }
    /// ```
    pub fn from_points(voxel_size: Vector, points: &[Vector]) -> Self {
        let voxels: Vec<_> = points
            .iter()
            .map(|pt| vect_to_ivect((*pt / voxel_size).floor()))
            .collect();
        Self::new(voxel_size, &voxels)
    }

    pub(crate) fn chunk_bvh(&self) -> &Bvh {
        &self.chunk_bvh
    }

    /// The semi-open range of grid coordinates covered by this voxel shape.
    ///
    /// Returns `[mins, maxs]` where the domain is the semi-open interval `[mins, maxs)`,
    /// meaning `mins` is included but `maxs` is excluded. This provides conservative bounds
    /// on the range of voxel grid coordinates that might be filled.
    ///
    /// This is useful for:
    /// - Determining the spatial extent of the voxel shape
    /// - Pre-allocating storage for processing voxels
    /// - Clipping operations to valid regions
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Voxels;
    /// use parry3d::math::{Vector, IVector};
    ///
    /// let voxels = Voxels::new(
    ///     Vector::new(1.0, 1.0, 1.0),
    ///     &[IVector::new(0, 0, 0), IVector::new(2, 3, 1)],
    /// );
    ///
    /// let [mins, maxs] = voxels.domain();
    /// assert_eq!(mins, IVector::new(0, 0, 0));
    /// // Domain is conservative and chunk-aligned
    /// assert!(maxs.x > 2 && maxs.y > 3 && maxs.z > 1);
    ///
    /// // Iterate through filled voxels (more efficient than iterating domain)
    /// for voxel in voxels.voxels() {
    ///     if !voxel.state.is_empty() {
    ///         println!("Filled voxel at {:?}", voxel.grid_coords);
    ///     }
    /// }
    /// # }
    /// ```
    pub fn domain(&self) -> [IVector; 2] {
        let aabb = self.chunk_bvh.root_aabb();

        // NOTE that we shift the AABB's bounds so the endpoint matches a voxel center
        //      to avoid rounding errors.
        let half_sz = self.voxel_size() / 2.0;
        let mins = self.voxel_at_point(aabb.mins + half_sz);
        // + 1 because the range is semi-open.
        let maxs = self.voxel_at_point(aabb.maxs - half_sz);
        let one = IVector::splat(1);
        [mins, maxs + one]
    }

    // /// The number of voxels along each coordinate axis.
    // pub fn dimensions(&self) -> Vector<u32> {
    //     (self.domain_maxs - self.domain_mins).map(|e| e as u32)
    // }

    /// The size of each voxel part this [`Voxels`] shape.
    pub fn voxel_size(&self) -> Vector {
        self.voxel_size
    }

    /// Scale this shape.
    pub fn scaled(mut self, scale: Vector) -> Self {
        self.voxel_size *= scale;
        self
    }

    /// A reference to the chunk with id `chunk_id`.
    ///
    /// Panics if the chunk doesn’t exist.
    pub fn chunk_ref(&self, chunk_id: u32) -> VoxelsChunkRef<'_> {
        VoxelsChunkRef {
            my_id: chunk_id as usize,
            parent: self,
            states: &self.chunks[chunk_id as usize].states,
            key: &self.chunk_keys[chunk_id as usize],
        }
    }

    /// The AABB of the voxel with the given quantized `key`.
    pub fn voxel_aabb(&self, key: IVector) -> Aabb {
        let center = self.voxel_center(key);
        let hext = self.voxel_size / 2.0;
        Aabb::from_half_extents(center, hext)
    }

    /// Returns the state of the voxel at the given grid coordinates.
    ///
    /// Returns `None` if the voxel doesn't exist in this shape's internal storage,
    /// or `Some(VoxelState)` containing information about whether the voxel is filled
    /// and which of its neighbors are also filled.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Voxels;
    /// use parry3d::math::{Vector, IVector};
    ///
    /// let voxels = Voxels::new(
    ///     Vector::new(1.0, 1.0, 1.0),
    ///     &[IVector::new(0, 0, 0), IVector::new(1, 0, 0)],
    /// );
    ///
    /// // Query an existing voxel
    /// if let Some(state) = voxels.voxel_state(IVector::new(0, 0, 0)) {
    ///     assert!(!state.is_empty());
    ///     println!("Voxel type: {:?}", state.voxel_type());
    /// }
    ///
    /// // Query a non-existent voxel
    /// assert!(voxels.voxel_state(IVector::new(10, 10, 10)).is_none());
    /// # }
    /// ```
    pub fn voxel_state(&self, key: IVector) -> Option<VoxelState> {
        let vid = self.linear_index(key)?;
        Some(self.chunks[vid.chunk_id].states[vid.id_in_chunk])
    }

    /// Calculates the grid coordinates of the voxel containing the given world-space point.
    ///
    /// This conversion is independent of whether the voxel is actually filled or empty - it
    /// simply determines which grid cell the point falls into based on the voxel size.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Voxels;
    /// use parry3d::math::{Vector, IVector};
    ///
    /// let voxels = Voxels::new(
    ///     Vector::new(1.0, 1.0, 1.0),
    ///     &[IVector::new(0, 0, 0)],
    /// );
    ///
    /// // Vector in first voxel (center at 0.5, 0.5, 0.5)
    /// assert_eq!(voxels.voxel_at_point(Vector::new(0.3, 0.7, 0.2)), IVector::new(0, 0, 0));
    ///
    /// // Vector just inside second voxel boundary
    /// assert_eq!(voxels.voxel_at_point(Vector::new(1.0, 0.0, 0.0)), IVector::new(1, 0, 0));
    ///
    /// // Negative coordinates work too
    /// assert_eq!(voxels.voxel_at_point(Vector::new(-0.5, -0.5, -0.5)), IVector::new(-1, -1, -1));
    /// # }
    /// ```
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Voxels;
    /// use parry3d::math::{Vector, IVector};
    ///
    /// // With non-uniform voxel size
    /// let voxels = Voxels::new(
    ///     Vector::new(2.0, 0.5, 1.0),
    ///     &[],
    /// );
    ///
    /// // X coordinate divided by 2.0, Y by 0.5, Z by 1.0
    /// assert_eq!(voxels.voxel_at_point(Vector::new(3.0, 1.2, 0.8)), IVector::new(1, 2, 0));
    /// # }
    /// ```
    pub fn voxel_at_point(&self, point: Vector) -> IVector {
        vect_to_ivect((point / self.voxel_size).floor())
    }

    /// Gets the voxel at the given flat voxel index.
    pub fn voxel_at_flat_id(&self, id: u32) -> Option<IVector> {
        let vid = VoxelIndex::from_flat_id(id as usize);
        let chunk_key = self.chunk_keys.get(vid.chunk_id)?;
        if *chunk_key == VoxelsChunk::INVALID_CHUNK_KEY {
            return None;
        }

        Some(VoxelsChunk::voxel_key_at_id(
            *chunk_key,
            vid.id_in_chunk as u32,
        ))
    }

    /// The range of grid coordinates of voxels intersecting the given AABB.
    ///
    /// The returned range covers both empty and non-empty voxels, and is not limited to the
    /// bounds defined by [`Self::domain`].
    /// The range is semi, open, i.e., the range along each dimension `i` is understood as
    /// the semi-open interval: `range[0][i]..range[1][i]`.
    pub fn voxel_range_intersecting_local_aabb(&self, aabb: &Aabb) -> [IVector; 2] {
        let mins = vect_to_ivect((aabb.mins / self.voxel_size).floor());
        let maxs = vect_to_ivect((aabb.maxs / self.voxel_size).ceil());
        [mins, maxs]
    }

    /// The AABB of a given range of voxels.
    ///
    /// The AABB is computed independently of [`Self::domain`] and independently of whether
    /// the voxels contained within are empty or not.
    pub fn voxel_range_aabb(&self, mins: IVector, maxs: IVector) -> Aabb {
        Aabb {
            mins: ivect_to_vect(mins) * self.voxel_size,
            maxs: ivect_to_vect(maxs) * self.voxel_size,
        }
    }

    /// Aligns the given AABB with the voxelized grid.
    ///
    /// The aligned is calculated such that the returned AABB has corners lying at the grid
    /// intersections (i.e. matches voxel corners) and fully contains the input `aabb`.
    pub fn align_aabb_to_grid(&self, aabb: &Aabb) -> Aabb {
        let mins = (aabb.mins / self.voxel_size).floor() * self.voxel_size;
        let maxs = (aabb.maxs / self.voxel_size).ceil() * self.voxel_size;
        Aabb { mins, maxs }
    }

    /// Iterates through every voxel intersecting the given aabb.
    ///
    /// Returns the voxel’s linearized id, center, and state.
    pub fn voxels_intersecting_local_aabb(
        &self,
        aabb: &Aabb,
    ) -> impl Iterator<Item = VoxelData> + '_ {
        let [mins, maxs] = self.voxel_range_intersecting_local_aabb(aabb);
        self.voxels_in_range(mins, maxs)
    }

    /// The center point of all the voxels in this shape (including empty ones).
    ///
    /// The voxel data associated to each center is provided to determine what kind of voxel
    /// it is (and, in particular, if it is empty or full).
    pub fn voxels(&self) -> impl Iterator<Item = VoxelData> + '_ {
        let aabb = self.chunk_bvh.root_aabb();
        self.voxels_in_range(
            self.voxel_at_point(aabb.mins),
            self.voxel_at_point(aabb.maxs),
        )
    }

    /// Iterate through the data of all the voxels within the given (semi-open) voxel grid indices.
    ///
    /// Note that this yields both empty and non-empty voxels within the range. This does not
    /// include any voxel that falls outside [`Self::domain`].
    pub fn voxels_in_range(
        &self,
        mins: IVector,
        maxs: IVector,
    ) -> impl Iterator<Item = VoxelData> + '_ {
        let range_aabb = Aabb::new(self.voxel_center(mins), self.voxel_center(maxs));

        self.chunk_bvh
            .leaves(move |node: &BvhNode| node.aabb().intersects(&range_aabb))
            .flat_map(move |chunk_id| {
                let chunk = self.chunk_ref(chunk_id);
                chunk.voxels_in_range(mins, maxs)
            })
    }

    fn voxel_to_chunk_key(voxel_key: IVector) -> IVector {
        fn div_floor(a: Int, b: usize) -> Int {
            let sign = (a < 0) as Int;
            (a + sign) / b as Int - sign
        }

        #[cfg(feature = "dim2")]
        {
            IVector::new(
                div_floor(voxel_key.x, VoxelsChunk::VOXELS_PER_CHUNK_DIM),
                div_floor(voxel_key.y, VoxelsChunk::VOXELS_PER_CHUNK_DIM),
            )
        }
        #[cfg(feature = "dim3")]
        {
            IVector::new(
                div_floor(voxel_key.x, VoxelsChunk::VOXELS_PER_CHUNK_DIM),
                div_floor(voxel_key.y, VoxelsChunk::VOXELS_PER_CHUNK_DIM),
                div_floor(voxel_key.z, VoxelsChunk::VOXELS_PER_CHUNK_DIM),
            )
        }
    }

    /// Given a voxel key, returns the key of the voxel chunk that contains it, as well as the
    /// linear index of the voxel within that chunk.
    #[cfg(feature = "dim2")]
    pub(super) fn chunk_key_and_id_in_chunk(voxel_key: IVector) -> (IVector, usize) {
        let chunk_key = Self::voxel_to_chunk_key(voxel_key);
        // NOTE: always positive since we subtracted the smallest possible key on that chunk.
        let voxel_key_in_chunk = voxel_key - chunk_key * VoxelsChunk::VOXELS_PER_CHUNK_DIM as Int;
        let id_in_chunk = (voxel_key_in_chunk.x
            + voxel_key_in_chunk.y * VoxelsChunk::VOXELS_PER_CHUNK_DIM as Int)
            as usize;
        (chunk_key, id_in_chunk)
    }

    /// Given a voxel key, returns the key of the voxel chunk that contains it, as well as the
    /// linear index of the voxel within that chunk.
    #[cfg(feature = "dim3")]
    pub(super) fn chunk_key_and_id_in_chunk(voxel_key: IVector) -> (IVector, usize) {
        let chunk_key = Self::voxel_to_chunk_key(voxel_key);
        // NOTE: always positive since we subtracted the smallest possible key on that chunk.
        let voxel_key_in_chunk = voxel_key - chunk_key * VoxelsChunk::VOXELS_PER_CHUNK_DIM as Int;
        let id_in_chunk = (voxel_key_in_chunk.x
            + voxel_key_in_chunk.y * VoxelsChunk::VOXELS_PER_CHUNK_DIM as Int
            + voxel_key_in_chunk.z
                * VoxelsChunk::VOXELS_PER_CHUNK_DIM as Int
                * VoxelsChunk::VOXELS_PER_CHUNK_DIM as Int) as usize;
        (chunk_key, id_in_chunk)
    }

    /// The linearized index associated to the given voxel key.
    pub fn linear_index(&self, voxel_key: IVector) -> Option<VoxelIndex> {
        let (chunk_key, id_in_chunk) = Self::chunk_key_and_id_in_chunk(voxel_key);
        let chunk_id = self.chunk_headers.get(&chunk_key)?.id;
        Some(VoxelIndex {
            chunk_id,
            id_in_chunk,
        })
    }

    /// The world-space center position of the voxel with the given grid coordinates.
    ///
    /// Returns the center point regardless of whether the voxel is actually filled.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Voxels;
    /// use parry3d::math::{Vector, IVector};
    ///
    /// let voxels = Voxels::new(
    ///     Vector::new(1.0, 1.0, 1.0),
    ///     &[IVector::new(0, 0, 0)],
    /// );
    ///
    /// // Center of voxel at origin
    /// assert_eq!(voxels.voxel_center(IVector::new(0, 0, 0)), Vector::new(0.5, 0.5, 0.5));
    ///
    /// // Center of voxel at (1, 2, 3)
    /// assert_eq!(voxels.voxel_center(IVector::new(1, 2, 3)), Vector::new(1.5, 2.5, 3.5));
    /// # }
    /// ```
    pub fn voxel_center(&self, key: IVector) -> Vector {
        (ivect_to_vect(key) + Vector::splat(0.5)) * self.voxel_size
    }
}

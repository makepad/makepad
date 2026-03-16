use crate::bounding_volume::Aabb;
use crate::math::{ivect_to_vect, IVector, Int, Real, Vector};
use crate::shape::{VoxelData, VoxelState, Voxels};

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub(super) struct VoxelsChunkHeader {
    pub(super) id: usize,
    // The number of non-empty voxels in the chunk.
    // This is used for detecting when a chunk can be removed
    // if it becomes fully empty.
    pub(super) len: usize,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[repr(C)]
#[repr(align(64))]
pub(super) struct VoxelsChunk {
    #[cfg_attr(feature = "serde-serialize", serde(with = "serde_arrays"))]
    pub(super) states: [VoxelState; VoxelsChunk::VOXELS_PER_CHUNK],
}

impl Default for VoxelsChunk {
    fn default() -> Self {
        Self {
            states: [VoxelState::EMPTY; VoxelsChunk::VOXELS_PER_CHUNK],
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct VoxelIndex {
    pub(super) chunk_id: usize,
    pub(super) id_in_chunk: usize,
}

impl VoxelIndex {
    pub fn flat_id(&self) -> usize {
        self.chunk_id * VoxelsChunk::VOXELS_PER_CHUNK + self.id_in_chunk
    }

    pub fn from_flat_id(id: usize) -> Self {
        Self {
            chunk_id: id / VoxelsChunk::VOXELS_PER_CHUNK,
            id_in_chunk: id % VoxelsChunk::VOXELS_PER_CHUNK,
        }
    }
}

impl VoxelsChunk {
    // TODO: find the ideal number. We want a good balance between cache locality
    //       and number of BVH leaf nodes.
    #[cfg(feature = "dim2")]
    pub(super) const VOXELS_PER_CHUNK_DIM: usize = 16;
    #[cfg(feature = "dim3")]
    pub(super) const VOXELS_PER_CHUNK_DIM: usize = 8;
    #[cfg(feature = "dim2")]
    pub(super) const VOXELS_PER_CHUNK: usize =
        Self::VOXELS_PER_CHUNK_DIM * Self::VOXELS_PER_CHUNK_DIM;
    #[cfg(feature = "dim3")]
    pub(super) const VOXELS_PER_CHUNK: usize =
        Self::VOXELS_PER_CHUNK_DIM * Self::VOXELS_PER_CHUNK_DIM * Self::VOXELS_PER_CHUNK_DIM;

    #[cfg(feature = "dim2")]
    pub(super) const INVALID_CHUNK_KEY: IVector = IVector::new(Int::MAX, Int::MAX);
    #[cfg(feature = "dim3")]
    pub(super) const INVALID_CHUNK_KEY: IVector = IVector::new(Int::MAX, Int::MAX, Int::MAX);

    /// The key of the voxel at the given linearized index within this chunk.
    #[cfg(feature = "dim2")]
    pub(super) fn voxel_key_at_id(chunk_key: IVector, id_in_chunk: u32) -> IVector {
        let y = id_in_chunk as Int / Self::VOXELS_PER_CHUNK_DIM as Int;
        let x = id_in_chunk as Int % Self::VOXELS_PER_CHUNK_DIM as Int;
        chunk_key * (Self::VOXELS_PER_CHUNK_DIM as Int) + IVector::new(x, y)
    }

    /// The key of the voxel at the given linearized index.
    #[cfg(feature = "dim3")]
    pub(super) fn voxel_key_at_id(chunk_key: IVector, id_in_chunk: u32) -> IVector {
        let d0d1 = (Self::VOXELS_PER_CHUNK_DIM * Self::VOXELS_PER_CHUNK_DIM) as u32;
        let z = id_in_chunk / d0d1;
        let y = (id_in_chunk - z * d0d1) / Self::VOXELS_PER_CHUNK_DIM as u32;
        let x = id_in_chunk % Self::VOXELS_PER_CHUNK_DIM as u32;
        chunk_key * (Self::VOXELS_PER_CHUNK_DIM as Int) + IVector::new(x as Int, y as Int, z as Int)
    }

    /// The semi-open range of valid voxel keys for this chunk.
    pub(super) fn keys_bounds(chunk_key: &IVector) -> [IVector; 2] {
        let imins = chunk_key * Self::VOXELS_PER_CHUNK_DIM as Int;
        let imaxs = imins + IVector::splat(Self::VOXELS_PER_CHUNK_DIM as Int);
        [imins, imaxs]
    }

    pub(super) fn aabb(chunk_key: &IVector, voxel_size: Vector) -> Aabb {
        let [imins, imaxs] = Self::keys_bounds(chunk_key);
        let aabb = Aabb::new(ivect_to_vect(imins), ivect_to_vect(imaxs));
        Aabb::new(aabb.mins * voxel_size, aabb.maxs * voxel_size)
    }
}

/// A reference to a chunk of voxels within a [`Voxels`] shape.
///
/// # What is a Chunk?
///
/// To efficiently manage large voxel worlds, Parry internally divides the voxel grid into
/// fixed-size chunks. Each chunk contains a small region of voxels (e.g., 8×8×8 in 3D or
/// 16×16 in 2D). This chunking provides:
///
/// - **Spatial acceleration**: Quick queries using a BVH of chunks
/// - **Memory efficiency**: Empty chunks are not stored
/// - **Cache locality**: Nearby voxels are stored together
///
/// A `VoxelsChunkRef` provides read-only access to a single chunk's data, allowing you to
/// query voxels within that chunk without iterating through the entire voxel shape.
///
/// # When to Use
///
/// You typically don't create `VoxelsChunkRef` directly. Instead, you get them from:
/// - [`Voxels::chunk_ref`]: Get a specific chunk by ID
/// - BVH traversal for spatial queries on large voxel worlds
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
/// // Get a chunk reference (chunk IDs come from BVH traversal)
/// let chunk_ref = voxels.chunk_ref(0);
///
/// // Query voxels within this chunk
/// for voxel in chunk_ref.voxels() {
///     if !voxel.state.is_empty() {
///         println!("Voxel at {:?}", voxel.grid_coords);
///     }
/// }
///
/// // Get chunk's AABB
/// let aabb = chunk_ref.local_aabb();
/// println!("Chunk bounds: {:?}", aabb);
/// # }
/// ```
#[derive(Copy, Clone)]
pub struct VoxelsChunkRef<'a> {
    /// The linear index of this chunk within the `Voxels` shape.
    pub my_id: usize,
    /// The voxel shape this chunk is part of.
    pub parent: &'a Voxels,
    /// The fill status of each voxel from this chunk.
    pub states: &'a [VoxelState; VoxelsChunk::VOXELS_PER_CHUNK],
    /// The spatial index of this chunk.
    pub key: &'a IVector,
}

impl<'a> VoxelsChunkRef<'a> {
    /// The AABB of this chunk of voxels.
    ///
    /// Note that this return the AABB of the whole chunk, without taking into account the fact
    /// that some voxels are empty.
    pub fn local_aabb(&self) -> Aabb {
        VoxelsChunk::aabb(self.key, self.parent.voxel_size)
    }

    /// The domain of this chunk of voxels.
    pub fn domain(&self) -> [IVector; 2] {
        VoxelsChunk::keys_bounds(self.key)
    }

    /// Returns the spatial index of the voxel containing the given point.
    pub fn voxel_at_point_unchecked(&self, pt: Vector) -> IVector {
        self.parent.voxel_at_point(pt)
    }

    /// The state of the voxel with key `voxel_key`.
    pub fn voxel_state(&self, voxel_key: IVector) -> Option<VoxelState> {
        let (chunk_key, id_in_chunk) = Voxels::chunk_key_and_id_in_chunk(voxel_key);
        if &chunk_key != self.key {
            return None;
        }
        Some(self.states[id_in_chunk])
    }

    /// Clamps the `voxel_key` so it is within the bounds of `self`.
    pub fn clamp_voxel(&self, voxel_key: IVector) -> IVector {
        let [mins, maxs] = self.domain();
        voxel_key.clamp(mins, maxs)
    }

    /// The AABB of the voxel with this key.
    ///
    /// Returns a result even if the voxel doesn’t belong to this chunk.
    pub fn voxel_aabb_unchecked(&self, voxel_key: IVector) -> Aabb {
        self.parent.voxel_aabb(voxel_key)
    }

    /// Convert a voxel index (expressed relative to the main `Voxels` shape, not relative to the
    /// chunk alone) into a flat index within a voxel chunk.
    ///
    /// Returns `None` if the voxel isn’t part of this chunk.
    pub fn flat_id(&self, voxel_key: IVector) -> Option<u32> {
        let (chunk_key, id_in_chunk) = Voxels::chunk_key_and_id_in_chunk(voxel_key);
        if &chunk_key != self.key {
            return None;
        }

        Some(
            VoxelIndex {
                chunk_id: self.my_id,
                id_in_chunk,
            }
            .flat_id() as u32,
        )
    }

    /// Iterates through all the voxels in this chunk.
    ///
    /// Note that this only yields non-empty voxels within the range. This does not
    /// include any voxel that falls outside [`Self::domain`].
    pub fn voxels(&self) -> impl Iterator<Item = VoxelData> + '_ {
        let range = self.domain();
        self.voxels_in_range(range[0], range[1])
    }

    /// Iterate through the data of all the voxels within the given (semi-open) voxel grid indices.
    ///
    /// Note that this only yields non-empty voxels within the range. This does not
    /// include any voxel that falls outside [`Self::domain`].
    #[cfg(feature = "dim2")]
    pub fn voxels_in_range(
        self,
        mins: IVector,
        maxs: IVector,
    ) -> impl Iterator<Item = VoxelData> + use<'a> {
        let [chunk_mins, chunk_maxs] = VoxelsChunk::keys_bounds(self.key);
        let mins = mins.max(chunk_mins);
        let maxs = maxs.min(chunk_maxs);

        (mins[0]..maxs[0]).flat_map(move |ix| {
            (mins[1]..maxs[1]).flat_map(move |iy| {
                let id_in_chunk = (ix - chunk_mins[0]) as usize
                    + (iy - chunk_mins[1]) as usize * VoxelsChunk::VOXELS_PER_CHUNK_DIM;
                let state = self.states[id_in_chunk];

                if state.is_empty() {
                    return None;
                }

                let grid_coords = IVector::new(ix, iy);
                let center =
                    Vector::new(ix as Real + 0.5, iy as Real + 0.5) * (self.parent.voxel_size);
                Some(VoxelData {
                    linear_id: VoxelIndex {
                        chunk_id: self.my_id,
                        id_in_chunk,
                    },
                    grid_coords,
                    center,
                    state,
                })
            })
        })
    }

    /// Iterate through the data of all the voxels within the given (semi-open) voxel grid indices.
    ///
    /// Note that this yields both empty and non-empty voxels within the range. This does not
    /// include any voxel that falls outside [`Self::domain`].
    #[cfg(feature = "dim3")]
    pub fn voxels_in_range(
        self,
        mins: IVector,
        maxs: IVector,
    ) -> impl Iterator<Item = VoxelData> + use<'a> {
        let [chunk_mins, chunk_maxs] = VoxelsChunk::keys_bounds(self.key);
        let mins = mins.max(chunk_mins);
        let maxs = maxs.min(chunk_maxs);

        (mins[0]..maxs[0]).flat_map(move |ix| {
            (mins[1]..maxs[1]).flat_map(move |iy| {
                (mins[2]..maxs[2]).filter_map(move |iz| {
                    let id_in_chunk = (ix - chunk_mins[0]) as usize
                        + (iy - chunk_mins[1]) as usize * VoxelsChunk::VOXELS_PER_CHUNK_DIM
                        + (iz - chunk_mins[2]) as usize
                            * VoxelsChunk::VOXELS_PER_CHUNK_DIM
                            * VoxelsChunk::VOXELS_PER_CHUNK_DIM;
                    let state = self.states[id_in_chunk];

                    if state.is_empty() {
                        return None;
                    }

                    let grid_coords = IVector::new(ix, iy, iz);
                    let center = Vector::new(ix as Real + 0.5, iy as Real + 0.5, iz as Real + 0.5)
                        * (self.parent.voxel_size);
                    Some(VoxelData {
                        linear_id: VoxelIndex {
                            chunk_id: self.my_id,
                            id_in_chunk,
                        },
                        grid_coords,
                        center,
                        state,
                    })
                })
            })
        })
    }
}

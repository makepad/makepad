use crate::bounding_volume::Aabb;
use crate::math::{IVector, Vector, DIM};
use crate::shape::voxels::voxels_chunk::{VoxelsChunk, VoxelsChunkHeader};
use crate::shape::{VoxelState, Voxels};
use crate::utils::hashmap::Entry;
use alloc::vec;

impl Voxels {
    /// Sets the size of each voxel along each local coordinate axis.
    ///
    /// Since the internal spatial acceleration structure needs to be updated, this
    /// operation runs in `O(n)` time, where `n` is the number of voxels.
    pub fn set_voxel_size(&mut self, new_size: Vector) {
        let scale = new_size / self.voxel_size;
        self.chunk_bvh.scale(scale);
        self.voxel_size = new_size;
    }

    /// Adds or removes a voxel at the specified grid coordinates.
    ///
    /// This is the primary method for dynamically modifying a voxel shape. It can:
    /// - Add a new voxel by setting `is_filled = true`
    /// - Remove an existing voxel by setting `is_filled = false`
    ///
    /// The method automatically updates the neighborhood information for the affected voxel
    /// and all its neighbors to maintain correct collision detection behavior.
    ///
    /// # Returns
    ///
    /// The previous [`VoxelState`] of the voxel before modification. This allows you to
    /// detect whether the operation actually changed anything.
    ///
    /// # Examples
    ///
    /// ## Adding Voxels
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Voxels;
    /// use parry3d::math::{Vector, IVector};
    ///
    /// let mut voxels = Voxels::new(Vector::new(1.0, 1.0, 1.0), &[]);
    ///
    /// // Add a voxel at (0, 0, 0)
    /// let prev_state = voxels.set_voxel(IVector::new(0, 0, 0), true);
    /// assert!(prev_state.is_empty()); // Was empty before
    ///
    /// // Verify it was added
    /// let state = voxels.voxel_state(IVector::new(0, 0, 0)).unwrap();
    /// assert!(!state.is_empty());
    /// # }
    /// ```
    ///
    /// ## Removing Voxels
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Voxels;
    /// use parry3d::math::{Vector, IVector};
    ///
    /// let mut voxels = Voxels::new(
    ///     Vector::new(1.0, 1.0, 1.0),
    ///     &[IVector::new(0, 0, 0), IVector::new(1, 0, 0)],
    /// );
    ///
    /// // Remove the voxel at (0, 0, 0)
    /// voxels.set_voxel(IVector::new(0, 0, 0), false);
    ///
    /// // Verify it was removed
    /// let state = voxels.voxel_state(IVector::new(0, 0, 0)).unwrap();
    /// assert!(state.is_empty());
    /// # }
    /// ```
    ///
    /// ## Building Shapes Dynamically
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Voxels;
    /// use parry3d::math::{Vector, IVector};
    ///
    /// let mut voxels = Voxels::new(Vector::new(1.0, 1.0, 1.0), &[]);
    ///
    /// // Build a 3×3 floor
    /// for x in 0..3 {
    ///     for z in 0..3 {
    ///         voxels.set_voxel(IVector::new(x, 0, z), true);
    ///     }
    /// }
    ///
    /// // Count filled voxels
    /// let filled = voxels.voxels()
    ///     .filter(|v| !v.state.is_empty())
    ///     .count();
    /// assert_eq!(filled, 9);
    /// # }
    /// ```
    ///
    /// ## Detecting Changes
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
    /// // Try to add a voxel that already exists
    /// let prev = voxels.set_voxel(IVector::new(0, 0, 0), true);
    /// if !prev.is_empty() {
    ///     println!("Voxel was already filled!");
    /// }
    /// # }
    /// ```
    pub fn set_voxel(&mut self, key: IVector, is_filled: bool) -> VoxelState {
        let (chunk_key, id_in_chunk) = Self::chunk_key_and_id_in_chunk(key);
        let header_entry = self.chunk_headers.entry(chunk_key);

        if !is_filled && matches!(header_entry, Entry::Vacant(_)) {
            // The voxel is already empty (it doesn’t exist at all).
            // Nothing more to do.
            return VoxelState::EMPTY;
        }

        let chunk_header = header_entry.or_insert_with(|| {
            let id = self.free_chunks.pop().unwrap_or_else(|| {
                self.chunks.push(VoxelsChunk::default());
                self.chunk_keys.push(chunk_key);
                self.chunks.len() - 1
            });

            self.chunk_keys[id] = chunk_key;
            self.chunk_bvh
                .insert(VoxelsChunk::aabb(&chunk_key, self.voxel_size), id as u32);
            VoxelsChunkHeader { id, len: 0 }
        });
        let chunk_id = chunk_header.id;

        let prev = self.chunks[chunk_id].states[id_in_chunk];
        let new_is_empty = !is_filled;

        if prev.is_empty() ^ new_is_empty {
            let can_remove_chunk = if new_is_empty {
                chunk_header.len -= 1;
                chunk_header.len == 0
            } else {
                chunk_header.len += 1;
                false
            };

            self.chunks[chunk_id].states[id_in_chunk] =
                self.update_neighbors_state(key, new_is_empty);

            if can_remove_chunk {
                self.chunk_bvh.remove(chunk_id as u32);

                #[cfg(feature = "enhanced-determinism")]
                let _ = self.chunk_headers.swap_remove(&chunk_key);
                #[cfg(not(feature = "enhanced-determinism"))]
                let _ = self.chunk_headers.remove(&chunk_key);

                self.free_chunks.push(chunk_id);
                self.chunk_keys[chunk_id] = VoxelsChunk::INVALID_CHUNK_KEY;
            }
        }

        prev
    }

    /// Crops the voxel shape in-place to a rectangular region.
    ///
    /// Removes all voxels outside the specified grid coordinate bounds `[domain_mins, domain_maxs]`
    /// (inclusive on both ends). This is useful for:
    /// - Extracting a sub-region of a larger voxel world
    /// - Removing voxels outside a region of interest
    /// - Implementing chunk-based world management
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Voxels;
    /// use parry3d::math::{Vector, IVector};
    ///
    /// let mut voxels = Voxels::new(
    ///     Vector::new(1.0, 1.0, 1.0),
    ///     &[
    ///         IVector::new(0, 0, 0),
    ///         IVector::new(1, 0, 0),
    ///         IVector::new(2, 0, 0),
    ///         IVector::new(3, 0, 0),
    ///     ],
    /// );
    ///
    /// // Keep only voxels in the range [1, 2]
    /// voxels.crop(IVector::new(1, 0, 0), IVector::new(2, 0, 0));
    ///
    /// // Only two voxels remain
    /// let count = voxels.voxels()
    ///     .filter(|v| !v.state.is_empty())
    ///     .count();
    /// assert_eq!(count, 2);
    /// # }
    /// ```
    pub fn crop(&mut self, domain_mins: IVector, domain_maxs: IVector) {
        // TODO PERF: this could be done more efficiently.
        if let Some(new_shape) = self.cropped(domain_mins, domain_maxs) {
            *self = new_shape;
        }
    }

    /// Returns a cropped version of this voxel shape with a rectangular domain.
    ///
    /// This removes every voxels out of the `[domain_mins, domain_maxs]` bounds.
    pub fn cropped(&self, domain_mins: IVector, domain_maxs: IVector) -> Option<Self> {
        // TODO PERF: can be optimized significantly.
        let mut in_box = vec![];
        for vox in self.voxels() {
            if !vox.state.is_empty()
                && grid_aabb_contains_point(&domain_mins, &domain_maxs, &vox.grid_coords)
            {
                in_box.push(vox.grid_coords);
            }
        }

        if !in_box.is_empty() {
            Some(Voxels::new(self.voxel_size, &in_box))
        } else {
            None
        }
    }

    /// Splits this voxel shape into two separate shapes based on an AABB.
    ///
    /// Partitions the voxels into two groups:
    /// - **Inside**: Voxels whose centers fall inside the given `aabb`
    /// - **Outside**: All remaining voxels
    ///
    /// Returns `(Some(inside), Some(outside))`, or `None` for either if that partition is empty.
    ///
    /// # Use Cases
    ///
    /// - Spatial partitioning for physics simulation
    /// - Implementing destructible objects (remove the "inside" part on explosion)
    /// - Chunk-based world management
    /// - Level-of-detail systems
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Voxels;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::{Vector, IVector};
    ///
    /// let voxels = Voxels::new(
    ///     Vector::new(1.0, 1.0, 1.0),
    ///     &[
    ///         IVector::new(0, 0, 0),  // Center at (0.5, 0.5, 0.5)
    ///         IVector::new(2, 0, 0),  // Center at (2.5, 0.5, 0.5)
    ///         IVector::new(4, 0, 0),  // Center at (4.5, 0.5, 0.5)
    ///     ],
    /// );
    ///
    /// // Split at X = 3.0
    /// let split_box = Aabb::new(
    ///     Vector::new(-10.0, -10.0, -10.0),
    ///     Vector::new(3.0, 10.0, 10.0),
    /// );
    ///
    /// let (inside, outside) = voxels.split_with_box(&split_box);
    ///
    /// // First two voxels inside, last one outside
    /// assert!(inside.is_some());
    /// assert!(outside.is_some());
    /// # }
    /// ```
    pub fn split_with_box(&self, aabb: &Aabb) -> (Option<Self>, Option<Self>) {
        // TODO PERF: can be optimized significantly.
        let mut in_box = vec![];
        let mut rest = vec![];
        for vox in self.voxels() {
            if !vox.state.is_empty() {
                if aabb.contains_local_point(vox.center) {
                    in_box.push(vox.grid_coords);
                } else {
                    rest.push(vox.grid_coords);
                }
            }
        }

        let in_box = if !in_box.is_empty() {
            Some(Voxels::new(self.voxel_size, &in_box))
        } else {
            None
        };

        let rest = if !rest.is_empty() {
            Some(Voxels::new(self.voxel_size, &rest))
        } else {
            None
        };

        (in_box, rest)
    }
}

fn grid_aabb_contains_point(mins: &IVector, maxs: &IVector, point: &IVector) -> bool {
    for i in 0..DIM {
        if point[i] < mins[i] || point[i] > maxs[i] {
            return false;
        }
    }

    true
}

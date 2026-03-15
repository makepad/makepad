// Rust port, with modifications, of https://github.com/kmammou/v-hacd/blob/master/src/VHACD_Lib/src/vhacdVolume.cpp
// By Khaled Mamou
//
// # License of the original C++ code:
// > Copyright (c) 2011 Khaled Mamou (kmamou at gmail dot com)
// > All rights reserved.
// >
// >
// > Redistribution and use in source and binary forms, with or without modification, are permitted provided that the following conditions are met:
// >
// > 1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following disclaimer.
// >
// > 2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided with the distribution.
// >
// > 3. The names of the contributors may not be used to endorse or promote products derived from this software without specific prior written permission.
// >
// > THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use crate::bounding_volume::Aabb;
#[cfg(not(feature = "std"))]
use crate::math::ComplexField;
use crate::math::{IVector, Int, Real, Vector, DIM};
use crate::query;
use crate::transformation::voxelization::{Voxel, VoxelSet};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// Controls how voxelization determines which voxels are filled vs empty.
///
/// The fill mode is a critical parameter that determines the structure of the resulting voxel set.
/// Choose the appropriate mode based on whether you need just the surface boundary or a solid volume.
///
/// # Variants
///
/// ## `SurfaceOnly`
///
/// Only voxels that intersect the input surface boundary (triangle mesh in 3D, polyline in 2D)
/// are marked as filled. This creates a hollow shell representation.
///
/// **Use this when:**
/// - You only need the surface boundary
/// - Memory is limited (fewer voxels to store)
/// - You're approximating a thin shell or surface
///
/// **Example:**
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::voxelization::{FillMode, VoxelSet};
/// use parry3d::shape::Ball;
/// ///
/// let ball = Ball::new(1.0);
/// let (vertices, indices) = ball.to_trimesh(20, 20);
///
/// let surface_voxels = VoxelSet::voxelize(
///     &vertices,
///     &indices,
///     10,
///     FillMode::SurfaceOnly,  // Only the shell
///     false,
/// );
///
/// // All voxels are on the surface
/// assert!(surface_voxels.voxels().iter().all(|v| v.is_on_surface));
/// # }
/// ```
///
/// ## `FloodFill`
///
/// Marks surface voxels AND all interior voxels as filled, creating a solid volume. Uses a
/// flood-fill algorithm starting from outside the shape to determine inside vs outside.
///
/// **Fields:**
/// - `detect_cavities`: If `true`, properly detects and preserves internal cavities/holes.
///   If `false`, all enclosed spaces are filled (faster but may fill unintended regions).
///
/// - `detect_self_intersections` (2D only): If `true`, attempts to handle self-intersecting
///   polylines correctly. More expensive but more robust.
///
/// **Use this when:**
/// - You need volume information (mass properties, volume computation)
/// - You want a solid representation for collision detection
/// - You need to distinguish between interior and surface voxels
///
/// **Example:**
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::voxelization::{FillMode, VoxelSet};
/// use parry3d::shape::Ball;
/// ///
/// let ball = Ball::new(1.0);
/// let (vertices, indices) = ball.to_trimesh(20, 20);
///
/// let solid_voxels = VoxelSet::voxelize(
///     &vertices,
///     &indices,
///     15,
///     FillMode::FloodFill {
///         detect_cavities: false,  // No cavities in a sphere
///     },
///     false,
/// );
///
/// // Mix of surface and interior voxels
/// let surface_count = solid_voxels.voxels().iter().filter(|v| v.is_on_surface).count();
/// let interior_count = solid_voxels.voxels().iter().filter(|v| !v.is_on_surface).count();
/// assert!(surface_count > 0 && interior_count > 0);
/// # }
/// ```
///
/// # Performance Notes
///
/// - `SurfaceOnly` is faster and uses less memory than `FloodFill`
/// - `detect_cavities = true` adds computational overhead but gives more accurate results
/// - For simple convex shapes, `detect_cavities = false` is usually sufficient
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FillMode {
    /// Only consider full the voxels intersecting the surface of the
    /// shape being voxelized.
    SurfaceOnly,
    /// Use a flood-fill technique to consider fill the voxels intersecting
    /// the surface of the shape being voxelized, as well as all the voxels
    /// bounded of them.
    FloodFill {
        /// Detects holes inside of a solid contour.
        detect_cavities: bool,
        /// Attempts to properly handle self-intersections.
        #[cfg(feature = "dim2")]
        detect_self_intersections: bool,
    },
    // RaycastFill
}

impl Default for FillMode {
    fn default() -> Self {
        Self::FloodFill {
            detect_cavities: false,
            #[cfg(feature = "dim2")]
            detect_self_intersections: false,
        }
    }
}

impl FillMode {
    #[cfg(feature = "dim2")]
    pub(crate) fn detect_cavities(self) -> bool {
        match self {
            FillMode::FloodFill {
                detect_cavities, ..
            } => detect_cavities,
            _ => false,
        }
    }

    #[cfg(feature = "dim2")]
    pub(crate) fn detect_self_intersections(self) -> bool {
        match self {
            FillMode::FloodFill {
                detect_self_intersections,
                ..
            } => detect_self_intersections,
            _ => false,
        }
    }

    #[cfg(feature = "dim3")]
    pub(crate) fn detect_self_intersections(self) -> bool {
        false
    }
}

/// The state of a voxel during and after voxelization.
///
/// This enum represents the classification of each voxel in the grid. After voxelization completes,
/// only three values are meaningful for end-user code: `PrimitiveOutsideSurface`,
/// `PrimitiveInsideSurface`, and `PrimitiveOnSurface`. The other variants are intermediate
/// states used during the flood-fill algorithm.
///
/// # Usage
///
/// Most users will work with [`VoxelSet`] which filters out outside voxels and only stores
/// filled ones. However, if you're working directly with [`VoxelizedVolume`], you'll encounter
/// all these values.
///
/// # Final States (After Voxelization)
///
/// - **`PrimitiveOutsideSurface`**: Voxel is completely outside the shape
/// - **`PrimitiveInsideSurface`**: Voxel is fully inside the shape (only with [`FillMode::FloodFill`])
/// - **`PrimitiveOnSurface`**: Voxel intersects the boundary surface
///
/// # Intermediate States (During Voxelization)
///
/// The following are temporary states used during the flood-fill algorithm and should be
/// ignored by user code:
/// - `PrimitiveUndefined`
/// - `PrimitiveOutsideSurfaceToWalk`
/// - `PrimitiveInsideSurfaceToWalk`
/// - `PrimitiveOnSurfaceNoWalk`
/// - `PrimitiveOnSurfaceToWalk1`
/// - `PrimitiveOnSurfaceToWalk2`
///
/// # Example
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::voxelization::{FillMode, VoxelValue, VoxelizedVolume};
/// use parry3d::shape::Cuboid;
/// use parry3d::math::Vector;
///
/// let cuboid = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
/// let (vertices, indices) = cuboid.to_trimesh();
///
/// // Create a dense voxelized volume
/// let volume = VoxelizedVolume::voxelize(
///     &vertices,
///     &indices,
///     8,
///     FillMode::FloodFill { detect_cavities: false },
///     false,
/// );
///
/// // Query individual voxel values
/// let resolution = volume.resolution();
/// for i in 0..resolution[0] {
///     for j in 0..resolution[1] {
///         for k in 0..resolution[2] {
///             match volume.voxel(i, j, k) {
///                 VoxelValue::PrimitiveOnSurface => {
///                     // This voxel is on the boundary
///                 }
///                 VoxelValue::PrimitiveInsideSurface => {
///                     // This voxel is inside the shape
///                 }
///                 VoxelValue::PrimitiveOutsideSurface => {
///                     // This voxel is outside the shape
///                 }
///                 _ => {
///                     // Should not happen after voxelization completes
///                 }
///             }
///         }
///     }
/// }
/// # }
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VoxelValue {
    /// Intermediate value, should be ignored by end-user code.
    PrimitiveUndefined,
    /// Intermediate value, should be ignored by end-user code.
    PrimitiveOutsideSurfaceToWalk,
    /// Intermediate value, should be ignored by end-user code.
    PrimitiveInsideSurfaceToWalk,
    /// Intermediate value, should be ignored by end-user code.
    PrimitiveOnSurfaceNoWalk,
    /// Intermediate value, should be ignored by end-user code.
    PrimitiveOnSurfaceToWalk1,
    /// Intermediate value, should be ignored by end-user code.
    PrimitiveOnSurfaceToWalk2,
    /// A voxel that is outside of the voxelized shape.
    PrimitiveOutsideSurface,
    /// A voxel that is on the interior of the voxelized shape.
    PrimitiveInsideSurface,
    /// Voxel that intersects the surface of the voxelized shape.
    PrimitiveOnSurface,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct VoxelData {
    #[cfg(feature = "dim2")]
    multiplicity: u32,
    num_primitive_intersections: u32,
}

/// A dense voxel grid storing the state of every voxel in a cubic volume.
///
/// `VoxelizedVolume` is the intermediate representation used during the voxelization process.
/// Unlike [`VoxelSet`] which only stores filled voxels, `VoxelizedVolume` stores a complete
/// dense 3D array where every grid cell has an associated [`VoxelValue`].
///
/// # When to Use
///
/// Most users should use [`VoxelSet`] instead, which is converted from `VoxelizedVolume`
/// automatically and is much more memory-efficient. You might want to use `VoxelizedVolume`
/// directly if you need to:
///
/// - Query the state of ALL voxels, including empty ones
/// - Implement custom voxel processing algorithms
/// - Generate visualizations showing both filled and empty voxels
///
/// # Memory Usage
///
/// `VoxelizedVolume` stores **all** voxels in a dense 3D array:
/// - Memory usage: `O(resolution^3)` in 3D or `O(resolution^2)` in 2D
/// - A 100×100×100 grid requires 1 million voxel entries
///
/// For this reason, [`VoxelSet`] is usually preferred for storage.
///
/// # Conversion
///
/// `VoxelizedVolume` automatically converts to `VoxelSet`:
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::voxelization::{FillMode, VoxelSet, VoxelizedVolume};
/// use parry3d::shape::Ball;
/// ///
/// let ball = Ball::new(1.0);
/// let (vertices, indices) = ball.to_trimesh(10, 10);
///
/// // Create dense volume
/// let volume = VoxelizedVolume::voxelize(
///     &vertices,
///     &indices,
///     10,
///     FillMode::SurfaceOnly,
///     false,
/// );
///
/// // Convert to sparse set (automatic via Into trait)
/// let voxel_set: VoxelSet = volume.into();
/// # }
/// ```
///
/// # Example: Querying All Voxels
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::voxelization::{FillMode, VoxelValue, VoxelizedVolume};
/// use parry3d::shape::Cuboid;
/// use parry3d::math::Vector;
///
/// let cuboid = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
/// let (vertices, indices) = cuboid.to_trimesh();
///
/// let volume = VoxelizedVolume::voxelize(
///     &vertices,
///     &indices,
///     8,
///     FillMode::FloodFill { detect_cavities: false },
///     false,
/// );
///
/// // Count voxels by state
/// let resolution = volume.resolution();
/// let mut inside = 0;
/// let mut surface = 0;
/// let mut outside = 0;
///
/// for i in 0..resolution[0] {
///     for j in 0..resolution[1] {
///         for k in 0..resolution[2] {
///             match volume.voxel(i, j, k) {
///                 VoxelValue::PrimitiveInsideSurface => inside += 1,
///                 VoxelValue::PrimitiveOnSurface => surface += 1,
///                 VoxelValue::PrimitiveOutsideSurface => outside += 1,
///                 _ => {}
///             }
///         }
///     }
/// }
///
/// println!("Inside: {}, Surface: {}, Outside: {}", inside, surface, outside);
/// # }
/// ```
pub struct VoxelizedVolume {
    origin: Vector,
    scale: Real,
    resolution: [u32; DIM],
    values: Vec<VoxelValue>,
    data: Vec<VoxelData>,
    primitive_intersections: Vec<(u32, u32)>,
}

impl VoxelizedVolume {
    /// Voxelizes the given shape described by its boundary:
    /// a triangle mesh (in 3D) or polyline (in 2D).
    ///
    /// # Parameters
    /// * `points` - The vertex buffer of the boundary of the shape to voxelize.
    /// * `indices` - The index buffer of the boundary of the shape to voxelize.
    /// * `resolution` - Controls the number of subdivision done along each axis. This number
    ///   is the number of subdivisions along the axis where the input shape has the largest extent.
    ///   The other dimensions will have a different automatically-determined resolution (in order to
    ///   keep the voxels cubic).
    /// * `fill_mode` - Controls what is being voxelized.
    /// * `keep_voxel_to_primitives_map` - If set to `true` a map between the voxels
    ///   and the primitives (3D triangles or 2D segments) it intersects will be computed.
    pub fn with_voxel_size(
        points: &[Vector],
        indices: &[[u32; DIM]],
        voxel_size: Real,
        fill_mode: FillMode,
        keep_voxel_to_primitives_map: bool,
    ) -> Self {
        let mut result = VoxelizedVolume {
            resolution: [0; DIM],
            origin: Vector::ZERO,
            scale: voxel_size,
            values: Vec::new(),
            data: Vec::new(),
            primitive_intersections: Vec::new(),
        };

        if points.is_empty() {
            return result;
        }

        let aabb = crate::bounding_volume::details::local_point_cloud_aabb_ref(points);
        result.origin = aabb.mins;
        let extents = aabb.extents() / voxel_size;
        #[cfg(feature = "dim2")]
        {
            result.resolution = [
                (extents.x.ceil() as u32).max(2) + 1,
                (extents.y.ceil() as u32).max(2) + 1,
            ];
        }
        #[cfg(feature = "dim3")]
        {
            result.resolution = [
                (extents.x.ceil() as u32).max(2) + 1,
                (extents.y.ceil() as u32).max(2) + 1,
                (extents.z.ceil() as u32).max(2) + 1,
            ];
        }

        result.do_voxelize(points, indices, fill_mode, keep_voxel_to_primitives_map);
        result
    }

    /// Voxelizes the given shape described by its boundary:
    /// a triangle mesh (in 3D) or polyline (in 2D).
    ///
    /// # Parameters
    /// * `points` - The vertex buffer of the boundary of the shape to voxelize.
    /// * `indices` - The index buffer of the boundary of the shape to voxelize.
    /// * `resolution` - Controls the number of subdivision done along each axis. This number
    ///   is the number of subdivisions along the axis where the input shape has the largest extent.
    ///   The other dimensions will have a different automatically-determined resolution (in order to
    ///   keep the voxels cubic).
    /// * `fill_mode` - Controls what is being voxelized.
    /// * `keep_voxel_to_primitives_map` - If set to `true` a map between the voxels
    ///   and the primitives (3D triangles or 2D segments) it intersects will be computed.
    pub fn voxelize(
        points: &[Vector],
        indices: &[[u32; DIM]],
        resolution: u32,
        fill_mode: FillMode,
        keep_voxel_to_primitives_map: bool,
    ) -> Self {
        let mut result = VoxelizedVolume {
            resolution: [0; DIM],
            origin: Vector::ZERO,
            scale: 1.0,
            values: Vec::new(),
            data: Vec::new(),
            primitive_intersections: Vec::new(),
        };

        if points.is_empty() {
            return result;
        }

        let aabb = crate::bounding_volume::details::local_point_cloud_aabb_ref(points);
        result.origin = aabb.mins;

        let d = aabb.maxs - aabb.mins;
        let r;

        #[cfg(feature = "dim2")]
        if d[0] > d[1] {
            r = d[0];
            result.resolution[0] = resolution;
            result.resolution[1] = 2 + (resolution as Real * d[1] / d[0]) as u32;
        } else {
            r = d[1];
            result.resolution[1] = resolution;
            result.resolution[0] = 2 + (resolution as Real * d[0] / d[1]) as u32;
        }

        #[cfg(feature = "dim3")]
        if d[0] >= d[1] && d[0] >= d[2] {
            r = d[0];
            result.resolution[0] = resolution;
            result.resolution[1] = 2 + (resolution as Real * d[1] / d[0]) as u32;
            result.resolution[2] = 2 + (resolution as Real * d[2] / d[0]) as u32;
        } else if d[1] >= d[0] && d[1] >= d[2] {
            r = d[1];
            result.resolution[1] = resolution;
            result.resolution[0] = 2 + (resolution as Real * d[0] / d[1]) as u32;
            result.resolution[2] = 2 + (resolution as Real * d[2] / d[1]) as u32;
        } else {
            r = d[2];
            result.resolution[2] = resolution;
            result.resolution[0] = 2 + (resolution as Real * d[0] / d[2]) as u32;
            result.resolution[1] = 2 + (resolution as Real * d[1] / d[2]) as u32;
        }

        result.scale = r / (resolution as Real - 1.0);
        result.do_voxelize(points, indices, fill_mode, keep_voxel_to_primitives_map);
        result
    }

    fn do_voxelize(
        &mut self,
        points: &[Vector],
        indices: &[[u32; DIM]],
        fill_mode: FillMode,
        keep_voxel_to_primitives_map: bool,
    ) {
        let inv_scale = 1.0 / self.scale;
        self.allocate();

        let mut tri_pts = [Vector::ZERO; DIM];
        let box_half_size = Vector::splat(0.5);
        let mut ijk0 = IVector::ZERO;
        let mut ijk1 = IVector::ZERO;

        let detect_self_intersections = fill_mode.detect_self_intersections();
        #[cfg(feature = "dim2")]
        let lock_high_multiplicities =
            fill_mode.detect_cavities() && fill_mode.detect_self_intersections();

        for (tri_id, tri) in indices.iter().enumerate() {
            // Find the range of voxels potentially intersecting the triangle.
            for c in 0..DIM {
                let pt = points[tri[c] as usize];
                tri_pts[c] = (pt - self.origin) * inv_scale;

                let i = (tri_pts[c].x + 0.5) as Int;
                let j = (tri_pts[c].y + 0.5) as Int;
                #[cfg(feature = "dim3")]
                let k = (tri_pts[c].z + 0.5) as Int;

                assert!((i as u32) < self.resolution[0] && (j as u32) < self.resolution[1]);
                #[cfg(feature = "dim3")]
                assert!((k as u32) < self.resolution[2]);

                #[cfg(feature = "dim2")]
                let ijk = IVector::new(i, j);
                #[cfg(feature = "dim3")]
                let ijk = IVector::new(i, j, k);

                if c == 0 {
                    ijk0 = ijk;
                    ijk1 = ijk;
                } else {
                    ijk0 = ijk0.min(ijk);
                    ijk1 = ijk1.max(ijk);
                }
            }

            ijk0 = (ijk0 - IVector::ONE).max(IVector::ZERO);
            #[cfg(feature = "dim2")]
            let resolution_vec = IVector::new(self.resolution[0] as Int, self.resolution[1] as Int);
            #[cfg(feature = "dim3")]
            let resolution_vec = IVector::new(
                self.resolution[0] as Int,
                self.resolution[1] as Int,
                self.resolution[2] as Int,
            );
            ijk1 = (ijk1 + IVector::ONE).min(resolution_vec);

            #[cfg(feature = "dim2")]
            let range_k = 0..1;
            #[cfg(feature = "dim3")]
            let range_k = ijk0.z..ijk1.z;

            // Determine exactly what voxel intersect the triangle.
            for i in ijk0.x..ijk1.x {
                for j in ijk0.y..ijk1.y {
                    for k in range_k.clone() {
                        #[cfg(feature = "dim2")]
                        let pt = Vector::new(i as Real, j as Real);
                        #[cfg(feature = "dim3")]
                        let pt = Vector::new(i as Real, j as Real, k as Real);

                        let id = self.voxel_index(i as u32, j as u32, k as u32);
                        let value = &mut self.values[id as usize];
                        let data = &mut self.data[id as usize];

                        if detect_self_intersections
                            || keep_voxel_to_primitives_map
                            || *value == VoxelValue::PrimitiveUndefined
                        {
                            let aabb = Aabb::from_half_extents(pt, box_half_size);

                            #[cfg(feature = "dim2")]
                            if !detect_self_intersections {
                                let segment = crate::shape::Segment::from(tri_pts);
                                let intersect =
                                    query::details::intersection_test_aabb_segment(&aabb, &segment);

                                if intersect {
                                    if keep_voxel_to_primitives_map {
                                        data.num_primitive_intersections += 1;
                                        self.primitive_intersections.push((id, tri_id as u32));
                                    }

                                    *value = VoxelValue::PrimitiveOnSurface;
                                }
                            } else if let Some(params) =
                                aabb.clip_line_parameters(tri_pts[0], tri_pts[1] - tri_pts[0])
                            {
                                let eps = 0.0; // -1.0e-6;

                                assert!(params.0 <= params.1);
                                if params.0 > 1.0 + eps || params.1 < 0.0 - eps {
                                    continue;
                                }

                                if keep_voxel_to_primitives_map {
                                    data.num_primitive_intersections += 1;
                                    self.primitive_intersections.push((id, tri_id as u32));
                                }

                                if data.multiplicity > 4 && lock_high_multiplicities {
                                    *value = VoxelValue::PrimitiveOnSurfaceNoWalk;
                                } else {
                                    *value = VoxelValue::PrimitiveOnSurface;
                                }
                            };

                            #[cfg(feature = "dim3")]
                            {
                                let triangle = crate::shape::Triangle::from(tri_pts);
                                let intersect = query::details::intersection_test_aabb_triangle(
                                    &aabb, &triangle,
                                );

                                if intersect {
                                    *value = VoxelValue::PrimitiveOnSurface;

                                    if keep_voxel_to_primitives_map {
                                        data.num_primitive_intersections += 1;
                                        self.primitive_intersections.push((id, tri_id as u32));
                                    }
                                }
                            };
                        }
                    }
                }
            }
        }

        match fill_mode {
            FillMode::SurfaceOnly => {
                for value in &mut self.values {
                    if *value != VoxelValue::PrimitiveOnSurface {
                        *value = VoxelValue::PrimitiveOutsideSurface
                    }
                }
            }
            FillMode::FloodFill {
                detect_cavities, ..
            } => {
                #[cfg(feature = "dim2")]
                {
                    self.mark_outside_surface(0, 0, self.resolution[0], 1);
                    self.mark_outside_surface(
                        0,
                        self.resolution[1] - 1,
                        self.resolution[0],
                        self.resolution[1],
                    );
                    self.mark_outside_surface(0, 0, 1, self.resolution[1]);
                    self.mark_outside_surface(
                        self.resolution[0] - 1,
                        0,
                        self.resolution[0],
                        self.resolution[1],
                    );
                }

                #[cfg(feature = "dim3")]
                {
                    self.mark_outside_surface(0, 0, 0, self.resolution[0], self.resolution[1], 1);
                    self.mark_outside_surface(
                        0,
                        0,
                        self.resolution[2] - 1,
                        self.resolution[0],
                        self.resolution[1],
                        self.resolution[2],
                    );
                    self.mark_outside_surface(0, 0, 0, self.resolution[0], 1, self.resolution[2]);
                    self.mark_outside_surface(
                        0,
                        self.resolution[1] - 1,
                        0,
                        self.resolution[0],
                        self.resolution[1],
                        self.resolution[2],
                    );
                    self.mark_outside_surface(0, 0, 0, 1, self.resolution[1], self.resolution[2]);
                    self.mark_outside_surface(
                        self.resolution[0] - 1,
                        0,
                        0,
                        self.resolution[0],
                        self.resolution[1],
                        self.resolution[2],
                    );
                }

                if detect_cavities {
                    let _ = self.propagate_values(
                        VoxelValue::PrimitiveOutsideSurfaceToWalk,
                        VoxelValue::PrimitiveOutsideSurface,
                        None,
                        VoxelValue::PrimitiveOnSurfaceToWalk1,
                    );

                    loop {
                        if !self.propagate_values(
                            VoxelValue::PrimitiveInsideSurfaceToWalk,
                            VoxelValue::PrimitiveInsideSurface,
                            Some(VoxelValue::PrimitiveOnSurfaceToWalk1),
                            VoxelValue::PrimitiveOnSurfaceToWalk2,
                        ) {
                            break;
                        }

                        if !self.propagate_values(
                            VoxelValue::PrimitiveOutsideSurfaceToWalk,
                            VoxelValue::PrimitiveOutsideSurface,
                            Some(VoxelValue::PrimitiveOnSurfaceToWalk2),
                            VoxelValue::PrimitiveOnSurfaceToWalk1,
                        ) {
                            break;
                        }
                    }

                    for voxel in &mut self.values {
                        if *voxel == VoxelValue::PrimitiveOnSurfaceToWalk1
                            || *voxel == VoxelValue::PrimitiveOnSurfaceToWalk2
                            || *voxel == VoxelValue::PrimitiveOnSurfaceNoWalk
                        {
                            *voxel = VoxelValue::PrimitiveOnSurface;
                        }
                    }
                } else {
                    let _ = self.propagate_values(
                        VoxelValue::PrimitiveOutsideSurfaceToWalk,
                        VoxelValue::PrimitiveOutsideSurface,
                        None,
                        VoxelValue::PrimitiveOnSurface,
                    );

                    self.replace_value(
                        VoxelValue::PrimitiveUndefined,
                        VoxelValue::PrimitiveInsideSurface,
                    );
                }
            }
        }
    }

    /// The number of voxel subdivisions along each coordinate axis.
    pub fn resolution(&self) -> [u32; DIM] {
        self.resolution
    }

    /// The scale factor that needs to be applied to the voxels of `self`
    /// in order to give them the size matching the original model's size.
    pub fn scale(&self) -> Real {
        self.scale
    }

    fn allocate(&mut self) {
        #[cfg(feature = "dim2")]
        let len = self.resolution[0] * self.resolution[1];
        #[cfg(feature = "dim3")]
        let len = self.resolution[0] * self.resolution[1] * self.resolution[2];
        self.values
            .resize(len as usize, VoxelValue::PrimitiveUndefined);
        self.data.resize(
            len as usize,
            VoxelData {
                #[cfg(feature = "dim2")]
                multiplicity: 0,
                num_primitive_intersections: 0,
            },
        );
    }

    fn voxel_index(&self, i: u32, j: u32, _k: u32) -> u32 {
        #[cfg(feature = "dim2")]
        return i + j * self.resolution[0];
        #[cfg(feature = "dim3")]
        return i + j * self.resolution[0] + _k * self.resolution[0] * self.resolution[1];
    }

    fn voxel_mut(&mut self, i: u32, j: u32, k: u32) -> &mut VoxelValue {
        let idx = self.voxel_index(i, j, k);
        &mut self.values[idx as usize]
    }

    /// The value of the given voxel.
    ///
    /// In 2D, the `k` argument is ignored.
    pub fn voxel(&self, i: u32, j: u32, k: u32) -> VoxelValue {
        let idx = self.voxel_index(i, j, k);
        self.values[idx as usize]
    }

    /// Mark all the PrimitiveUndefined voxels within the given bounds as PrimitiveOutsideSurfaceToWalk.
    #[cfg(feature = "dim2")]
    fn mark_outside_surface(&mut self, i0: u32, j0: u32, i1: u32, j1: u32) {
        for i in i0..i1 {
            for j in j0..j1 {
                let v = self.voxel_mut(i, j, 0);

                if *v == VoxelValue::PrimitiveUndefined {
                    *v = VoxelValue::PrimitiveOutsideSurfaceToWalk;
                }
            }
        }
    }

    /// Mark all the PrimitiveUndefined voxels within the given bounds as PrimitiveOutsideSurfaceToWalk.
    #[cfg(feature = "dim3")]
    fn mark_outside_surface(&mut self, i0: u32, j0: u32, k0: u32, i1: u32, j1: u32, k1: u32) {
        for i in i0..i1 {
            for j in j0..j1 {
                for k in k0..k1 {
                    let v = self.voxel_mut(i, j, k);

                    if *v == VoxelValue::PrimitiveUndefined {
                        *v = VoxelValue::PrimitiveOutsideSurfaceToWalk;
                    }
                }
            }
        }
    }

    fn walk_forward(
        primitive_undefined_value_to_set: VoxelValue,
        on_surface_value_to_set: VoxelValue,
        start: isize,
        end: isize,
        mut ptr: isize,
        out: &mut [VoxelValue],
        stride: isize,
        max_distance: isize,
    ) {
        let mut i = start;
        let mut count = 0;

        while count < max_distance && i < end {
            if out[ptr as usize] == VoxelValue::PrimitiveUndefined {
                out[ptr as usize] = primitive_undefined_value_to_set;
            } else if out[ptr as usize] == VoxelValue::PrimitiveOnSurface {
                out[ptr as usize] = on_surface_value_to_set;
                break;
            } else {
                break;
            }

            i += 1;
            ptr += stride;
            count += 1;
        }
    }

    fn walk_backward(
        primitive_undefined_value_to_set: VoxelValue,
        on_surface_value_to_set: VoxelValue,
        start: isize,
        end: isize,
        mut ptr: isize,
        out: &mut [VoxelValue],
        stride: isize,
        max_distance: isize,
    ) {
        let mut i = start;
        let mut count = 0;

        while count < max_distance && i >= end {
            if out[ptr as usize] == VoxelValue::PrimitiveUndefined {
                out[ptr as usize] = primitive_undefined_value_to_set;
            } else if out[ptr as usize] == VoxelValue::PrimitiveOnSurface {
                out[ptr as usize] = on_surface_value_to_set;
                break;
            } else {
                break;
            }

            i -= 1;
            ptr -= stride;
            count += 1;
        }
    }

    fn propagate_values(
        &mut self,
        inside_surface_value_to_walk: VoxelValue,
        inside_surface_value_to_set: VoxelValue,
        on_surface_value_to_walk: Option<VoxelValue>,
        on_surface_value_to_set: VoxelValue,
    ) -> bool {
        let mut voxels_walked;
        let mut walked_at_least_once = false;
        let i0 = self.resolution[0];
        let j0 = self.resolution[1];
        #[cfg(feature = "dim2")]
        let k0 = 1;
        #[cfg(feature = "dim3")]
        let k0 = self.resolution[2];

        // Avoid striding too far in each direction to stay in L1 cache as much as possible.
        // The cache size required for the walk is roughly (4 * walk_distance * 64) since
        // the k direction doesn't count as it's walking byte per byte directly in a cache lines.
        // ~16k is required for a walk distance of 64 in each directions.
        let walk_distance = 64;

        // using the stride directly instead of calling get_voxel for each iterations saves
        // a lot of multiplications and pipeline stalls due to values dependencies on imul.
        let istride = self.voxel_index(1, 0, 0) as isize - self.voxel_index(0, 0, 0) as isize;
        let jstride = self.voxel_index(0, 1, 0) as isize - self.voxel_index(0, 0, 0) as isize;
        #[cfg(feature = "dim3")]
        let kstride = self.voxel_index(0, 0, 1) as isize - self.voxel_index(0, 0, 0) as isize;

        // It might seem counter intuitive to go over the whole voxel range multiple times
        // but since we do the run in memory order, it leaves us with far fewer cache misses
        // than a BFS algorithm and it has the additional benefit of not requiring us to
        // store and manipulate a fifo for recursion that might become huge when the number
        // of voxels is large.
        // This will outperform the BFS algorithm by several orders of magnitude in practice.
        loop {
            voxels_walked = 0;

            for i in 0..i0 {
                for j in 0..j0 {
                    for k in 0..k0 {
                        let idx = self.voxel_index(i, j, k) as isize;
                        let voxel = self.voxel_mut(i, j, k);

                        if *voxel == inside_surface_value_to_walk {
                            voxels_walked += 1;
                            walked_at_least_once = true;
                            *voxel = inside_surface_value_to_set;
                        } else if Some(*voxel) != on_surface_value_to_walk {
                            continue;
                        }

                        // walk in each direction to mark other voxel that should be walked.
                        // this will generate a 3d pattern that will help the overall
                        // algorithm converge faster while remaining cache friendly.
                        #[cfg(feature = "dim3")]
                        Self::walk_forward(
                            inside_surface_value_to_walk,
                            on_surface_value_to_set,
                            k as isize + 1,
                            k0 as isize,
                            idx + kstride,
                            &mut self.values,
                            kstride,
                            walk_distance,
                        );
                        #[cfg(feature = "dim3")]
                        Self::walk_backward(
                            inside_surface_value_to_walk,
                            on_surface_value_to_set,
                            k as isize - 1,
                            0,
                            idx - kstride,
                            &mut self.values,
                            kstride,
                            walk_distance,
                        );

                        Self::walk_forward(
                            inside_surface_value_to_walk,
                            on_surface_value_to_set,
                            j as isize + 1,
                            j0 as isize,
                            idx + jstride,
                            &mut self.values,
                            jstride,
                            walk_distance,
                        );
                        Self::walk_backward(
                            inside_surface_value_to_walk,
                            on_surface_value_to_set,
                            j as isize - 1,
                            0,
                            idx - jstride,
                            &mut self.values,
                            jstride,
                            walk_distance,
                        );

                        Self::walk_forward(
                            inside_surface_value_to_walk,
                            on_surface_value_to_set,
                            (i + 1) as isize,
                            i0 as isize,
                            idx + istride,
                            &mut self.values,
                            istride,
                            walk_distance,
                        );
                        Self::walk_backward(
                            inside_surface_value_to_walk,
                            on_surface_value_to_set,
                            i as isize - 1,
                            0,
                            idx - istride,
                            &mut self.values,
                            istride,
                            walk_distance,
                        );
                    }
                }
            }

            if voxels_walked == 0 {
                break;
            }
        }

        walked_at_least_once
    }

    fn replace_value(&mut self, current_value: VoxelValue, new_value: VoxelValue) {
        for voxel in &mut self.values {
            if *voxel == current_value {
                *voxel = new_value;
            }
        }
    }

    /// Naive conversion of all the voxels with the given `value` to a triangle-mesh.
    ///
    /// This conversion is extremely naive: it will simply collect all the 12 triangles forming
    /// the faces of each voxel. No actual boundary extraction is done.
    #[cfg(feature = "dim3")]
    pub fn to_trimesh(&self, value: VoxelValue) -> (Vec<Vector>, Vec<[u32; DIM]>) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        for i in 0..self.resolution[0] {
            for j in 0..self.resolution[1] {
                for k in 0..self.resolution[2] {
                    let voxel = self.voxel(i, j, k);

                    if voxel == value {
                        let ijk = Vector::new(i as Real, j as Real, k as Real);

                        let shifts = [
                            Vector::new(-0.5, -0.5, -0.5),
                            Vector::new(0.5, -0.5, -0.5),
                            Vector::new(0.5, 0.5, -0.5),
                            Vector::new(-0.5, 0.5, -0.5),
                            Vector::new(-0.5, -0.5, 0.5),
                            Vector::new(0.5, -0.5, 0.5),
                            Vector::new(0.5, 0.5, 0.5),
                            Vector::new(-0.5, 0.5, 0.5),
                        ];

                        for shift in &shifts {
                            vertices.push(self.origin + (ijk + shift) * self.scale);
                        }

                        let s = vertices.len() as u32;
                        indices.push([s, s + 2, s + 1]);
                        indices.push([s, s + 3, s + 2]);
                        indices.push([s + 4, s + 5, s + 6]);
                        indices.push([s + 4, s + 6, s + 7]);
                        indices.push([s + 7, s + 6, s + 2]);
                        indices.push([s + 7, s + 2, s + 3]);
                        indices.push([s + 4, s + 1, s + 5]);
                        indices.push([s + 4, s, s + 1]);
                        indices.push([s + 6, s + 5, s + 1]);
                        indices.push([s + 6, s + 1, s + 2]);
                        indices.push([s + 7, s, s + 4]);
                        indices.push([s + 7, s + 3, s]);
                    }
                }
            }
        }

        (vertices, indices)
    }
}

impl From<VoxelizedVolume> for VoxelSet {
    fn from(mut shape: VoxelizedVolume) -> Self {
        let mut curr_intersection_index = 0;
        let mut vset = VoxelSet::new();
        let mut vset_intersections = Vec::new();
        vset.origin = shape.origin;
        vset.scale = shape.scale;

        #[cfg(feature = "dim2")]
        let k1 = 1;
        #[cfg(feature = "dim3")]
        let k1 = shape.resolution[2];

        for i in 0..shape.resolution[0] {
            for j in 0..shape.resolution[1] {
                for k in 0..k1 {
                    let id = shape.voxel_index(i, j, k) as usize;
                    let value = shape.values[id];
                    #[cfg(feature = "dim2")]
                    let coords = IVector::new(i as Int, j as Int);
                    #[cfg(feature = "dim3")]
                    let coords = IVector::new(i as Int, j as Int, k as Int);

                    if value == VoxelValue::PrimitiveInsideSurface {
                        let voxel = Voxel {
                            coords,
                            is_on_surface: false,
                            intersections_range: (curr_intersection_index, curr_intersection_index),
                        };
                        vset.voxels.push(voxel);
                    } else if value == VoxelValue::PrimitiveOnSurface {
                        let mut voxel = Voxel {
                            coords,
                            is_on_surface: true,
                            intersections_range: (curr_intersection_index, curr_intersection_index),
                        };

                        if !shape.primitive_intersections.is_empty() {
                            let num_intersections =
                                shape.data[id].num_primitive_intersections as usize;
                            // We store the index where we should write the intersection on the
                            // vset into num_primitive_intersections. That way we can reuse it
                            // afterwards when copying the set of intersection into a single
                            // flat Vec.
                            shape.data[id].num_primitive_intersections =
                                curr_intersection_index as u32;
                            curr_intersection_index += num_intersections;
                            voxel.intersections_range.1 = curr_intersection_index;
                        }

                        vset.voxels.push(voxel);
                    }
                }
            }
        }

        if !shape.primitive_intersections.is_empty() {
            vset_intersections.resize(shape.primitive_intersections.len(), 0);
            for (voxel_id, prim_id) in shape.primitive_intersections {
                let num_inter = &mut shape.data[voxel_id as usize].num_primitive_intersections;
                vset_intersections[*num_inter as usize] = prim_id;
                *num_inter += 1;
            }
        }

        vset.intersections = Arc::new(vset_intersections);

        vset
    }
}

/*
fn traceRay(
    mesh: &RaycastMesh,
    start: Real,
    dir: Vector,
    inside_count: &mut u32,
    outside_count: &mut u32,
) {
    let out_t;
    let u;
    let v;
    let w;
    let face_sign;
    let face_index;
    let hit = raycast_mesh.raycast(start, dir, out_t, u, v, w, face_sign, face_index);

    if hit {
        if face_sign >= 0 {
            *inside_count += 1;
        } else {
            *outside_count += 1;
        }
    }
}


fn raycast_fill(volume: &Volume, raycast_mesh: &RaycastMesh) {
if !raycast_mesh {
    return;
}

let scale = volume.scale;
let bmin = volume.min_bb;

let i0 = volume.resolution[0];
let j0 = volume.resolution[1];
let k0 = volume.resolution[2];

for i in 0..i0 {
    for j in 0..j0 {
        for k in 0..k0 {
            let voxel = volume.get_voxel(i, j, k);

            if voxel != VoxelValue::PrimitiveOnSurface {
                let start = Vector::new(
                    i as Real * scale + bmin[0],
                    j as Real * scale + bmin[1],
                    k as Real * scale + bmin[2],
                );

                let mut inside_count = 0;
                let mut outside_count = 0;

                let directions = [
                    Vector::X,
                    -Vector::X,
                    Vector::Y,
                    -Vector::Y,
                    Vector::Z,
                    -Vector::Z,
                ];

                for r in 0..6 {
                    traceRay(
                        raycast_mesh,
                        start,
                        &directions[r * 3],
                        &mut inside_count,
                        &mut outside_count,
                    );

                    // Early out if we hit the outside of the mesh
                    if outside_count != 0 {
                        break;
                    }

                    // Early out if we accumulated 3 inside hits
                    if inside_count >= 3 {
                        break;
                    }
                }

                if outside_count == 0 && inside_count >= 3 {
                    volume.set_voxel(i, j, k, VoxelValue::PrimitiveInsideSurface);
                } else {
                    volume.set_voxel(i, j, k, VoxelValue::PrimitiveOutsideSurface);
                }
            }
        }
    }
}
}
 */

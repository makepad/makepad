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

use super::{FillMode, VoxelizedVolume};
use crate::bounding_volume::Aabb;
use crate::math::{ivect_to_vect, MatExt, VectorExt};
use crate::math::{IVector, Matrix, Real, Vector, DIM};
use crate::transformation::vhacd::CutPlane;
use alloc::sync::Arc;
use alloc::{vec, vec::Vec};

#[cfg(feature = "dim2")]
type ConvexHull = Vec<Vector>;
#[cfg(feature = "dim3")]
type ConvexHull = (Vec<Vector>, Vec<[u32; DIM]>);

/// A single voxel in a voxel grid.
///
/// A voxel represents a cubic (or square in 2D) cell in a discrete grid. Each voxel is identified
/// by its integer grid coordinates and contains metadata about its position relative to the
/// voxelized shape.
///
/// # Fields
///
/// - `coords`: The grid position `(i, j)` in 2D or `(i, j, k)` in 3D. These are **integer**
///   coordinates in the voxel grid, not world-space coordinates.
///
/// - `is_on_surface`: Whether this voxel intersects the surface boundary of the shape. If `false`,
///   the voxel is completely inside the shape (only possible when using [`FillMode::FloodFill`]).
///
/// # Example
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::voxelization::{FillMode, VoxelSet};
/// use parry3d::shape::Ball;
/// ///
/// let ball = Ball::new(1.0);
/// let (vertices, indices) = ball.to_trimesh(10, 10);
///
/// let voxels = VoxelSet::voxelize(
///     &vertices,
///     &indices,
///     8,
///     FillMode::FloodFill { detect_cavities: false },
///     false,
/// );
///
/// // Examine individual voxels
/// for voxel in voxels.voxels() {
///     // Grid coordinates (integer position in the voxel grid)
///     println!("Grid position: {:?}", voxel);
///
///     // World-space position (actual 3D coordinates)
///     let world_pos = voxels.get_voxel_point(voxel);
///     println!("World position: {:?}", world_pos);
///
///     // Surface vs interior
///     if voxel.is_on_surface {
///         println!("This voxel is on the surface boundary");
///     } else {
///         println!("This voxel is inside the shape");
///     }
/// }
/// # }
/// ```
#[derive(Copy, Clone, Debug)]
pub struct Voxel {
    /// The integer coordinates of the voxel as part of the voxel grid.
    pub coords: IVector,
    /// Is this voxel on the surface of the volume (i.e. not inside of it)?
    pub is_on_surface: bool,
    /// Range of indices (to be looked up into the `VoxelSet` primitive map)
    /// of the primitives intersected by this voxel.
    pub(crate) intersections_range: (usize, usize),
}

impl Default for Voxel {
    fn default() -> Self {
        Self {
            coords: IVector::ZERO,
            is_on_surface: false,
            intersections_range: (0, 0),
        }
    }
}

/// A sparse set of filled voxels resulting from voxelization.
///
/// `VoxelSet` is a memory-efficient storage format that only contains voxels marked as "filled"
/// during the voxelization process. This is much more efficient than storing a dense 3D array
/// for shapes that are mostly empty or have a hollow interior.
///
/// # Structure
///
/// Each `VoxelSet` contains:
/// - A list of filled voxels with their grid coordinates
/// - The origin point and scale factor for converting grid coordinates to world space
/// - Optional metadata tracking which primitives (triangles/segments) intersect each voxel
///
/// # Grid Coordinates vs World Coordinates
///
/// Voxels are stored with integer grid coordinates `(i, j, k)`. To convert to world-space
/// coordinates, use:
///
/// ```text
/// world_position = origin + (i, j, k) * scale
/// ```
///
/// The [`get_voxel_point()`](VoxelSet::get_voxel_point) method does this conversion for you.
///
/// # Memory Layout
///
/// Unlike [`VoxelizedVolume`] which stores a dense 3D array, `VoxelSet` uses sparse storage:
/// - Only filled voxels are stored (typically much fewer than total grid cells)
/// - Memory usage is `O(filled_voxels)` instead of `O(resolution^3)`
/// - Ideal for shapes with low surface-to-volume ratio
///
/// # Example: Basic Usage
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::voxelization::{FillMode, VoxelSet};
/// use parry3d::shape::Cuboid;
/// use parry3d::math::Vector;
///
/// // Create and voxelize a shape
/// let cuboid = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
/// let (vertices, indices) = cuboid.to_trimesh();
///
/// let voxels = VoxelSet::voxelize(
///     &vertices,
///     &indices,
///     10,                      // resolution
///     FillMode::SurfaceOnly,   // hollow surface only
///     false,                   // no primitive mapping
/// );
///
/// // Query the voxel set
/// println!("Number of voxels: {}", voxels.len());
/// println!("Voxel size: {}", voxels.scale);
/// println!("Total volume: {}", voxels.compute_volume());
///
/// // Access voxels
/// for voxel in voxels.voxels() {
///     let world_pos = voxels.get_voxel_point(voxel);
///     println!("Voxel at {:?}", world_pos);
/// }
/// # }
/// ```
///
/// # Example: Volume Computation
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::voxelization::{FillMode, VoxelSet};
/// use parry3d::shape::Ball;
/// ///
/// let ball = Ball::new(1.0);
/// let (vertices, indices) = ball.to_trimesh(20, 20);
///
/// // Voxelize with interior filling
/// let voxels = VoxelSet::voxelize(
///     &vertices,
///     &indices,
///     20,
///     FillMode::FloodFill { detect_cavities: false },
///     false,
/// );
///
/// // Compute approximate volume
/// let voxel_volume = voxels.compute_volume();
/// let expected_volume = 4.0 / 3.0 * std::f32::consts::PI;
/// println!("Voxel volume: {:.3}, Expected: {:.3}", voxel_volume, expected_volume);
/// # }
/// ```
pub struct VoxelSet {
    /// The 3D origin of this voxel-set.
    pub origin: Vector,
    /// The scale factor between the voxel integer coordinates and their
    /// actual float world-space coordinates.
    pub scale: Real,
    pub(crate) min_bb_voxels: IVector,
    pub(crate) max_bb_voxels: IVector,
    pub(crate) voxels: Vec<Voxel>,
    pub(crate) intersections: Arc<Vec<u32>>,
    pub(crate) primitive_classes: Arc<Vec<u32>>,
}

impl Default for VoxelSet {
    fn default() -> Self {
        Self::new()
    }
}

impl VoxelSet {
    /// Creates a new empty set of voxels.
    pub fn new() -> Self {
        Self {
            origin: Vector::ZERO,
            min_bb_voxels: IVector::ZERO,
            max_bb_voxels: IVector::ONE,
            scale: 1.0,
            voxels: Vec::new(),
            intersections: Arc::new(Vec::new()),
            primitive_classes: Arc::new(Vec::new()),
        }
    }

    /// The volume of a single voxel of this voxel set.
    #[cfg(feature = "dim2")]
    pub fn voxel_volume(&self) -> Real {
        self.scale * self.scale
    }

    /// The volume of a single voxel of this voxel set.
    #[cfg(feature = "dim3")]
    pub fn voxel_volume(&self) -> Real {
        self.scale * self.scale * self.scale
    }

    /// Voxelizes a shape by specifying the physical size of each voxel.
    ///
    /// This creates a voxelized representation of a shape defined by its boundary:
    /// - In 3D: A triangle mesh (vertices and triangle indices)
    /// - In 2D: A polyline (vertices and segment indices)
    ///
    /// The resolution is automatically determined based on the shape's bounding box
    /// and the requested voxel size.
    ///
    /// # Parameters
    ///
    /// * `points` - Vertex buffer defining the boundary of the shape. These are the
    ///   vertices of the triangle mesh (3D) or polyline (2D).
    ///
    /// * `indices` - Index buffer defining the boundary primitives:
    ///   - 3D: Each element is `[v0, v1, v2]` defining a triangle
    ///   - 2D: Each element is `[v0, v1]` defining a line segment
    ///
    /// * `voxel_size` - The physical size (edge length) of each cubic voxel. Smaller
    ///   values give higher resolution but use more memory.
    ///
    /// * `fill_mode` - Controls which voxels are marked as filled:
    ///   - [`FillMode::SurfaceOnly`]: Only voxels intersecting the boundary
    ///   - [`FillMode::FloodFill`]: Surface voxels plus interior voxels
    ///
    /// * `keep_voxel_to_primitives_map` - If `true`, stores which primitives (triangles
    ///   or segments) intersect each voxel. Required for [`compute_exact_convex_hull()`]
    ///   and [`compute_primitive_intersections()`], but uses additional memory.
    ///
    /// # Returns
    ///
    /// A sparse `VoxelSet` containing only the filled voxels.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::transformation::voxelization::{FillMode, VoxelSet};
    /// use parry3d::shape::Ball;
    ///
    /// let ball = Ball::new(2.0);  // radius = 2.0
    /// let (vertices, indices) = ball.to_trimesh(20, 20);
    ///
    /// // Create voxels with 0.1 unit size
    /// let voxels = VoxelSet::with_voxel_size(
    ///     &vertices,
    ///     &indices,
    ///     0.1,                    // each voxel is 0.1 x 0.1 x 0.1
    ///     FillMode::SurfaceOnly,
    ///     false,
    /// );
    ///
    /// println!("Created {} voxels", voxels.len());
    /// println!("Each voxel has volume {}", voxels.voxel_volume());
    /// # }
    /// ```
    ///
    /// [`compute_exact_convex_hull()`]: VoxelSet::compute_exact_convex_hull
    /// [`compute_primitive_intersections()`]: VoxelSet::compute_primitive_intersections
    pub fn with_voxel_size(
        points: &[Vector],
        indices: &[[u32; DIM]],
        voxel_size: Real,
        fill_mode: FillMode,
        keep_voxel_to_primitives_map: bool,
    ) -> Self {
        VoxelizedVolume::with_voxel_size(
            points,
            indices,
            voxel_size,
            fill_mode,
            keep_voxel_to_primitives_map,
        )
        .into()
    }

    /// Voxelizes a shape by specifying the grid resolution along the longest axis.
    ///
    /// This creates a voxelized representation of a shape defined by its boundary:
    /// - In 3D: A triangle mesh (vertices and triangle indices)
    /// - In 2D: A polyline (vertices and segment indices)
    ///
    /// The voxel size is automatically computed to fit the specified number of subdivisions
    /// along the shape's longest axis, while maintaining cubic (or square in 2D) voxels.
    /// Other axes will have proportionally determined resolutions.
    ///
    /// # Parameters
    ///
    /// * `points` - Vertex buffer defining the boundary of the shape. These are the
    ///   vertices of the triangle mesh (3D) or polyline (2D).
    ///
    /// * `indices` - Index buffer defining the boundary primitives:
    ///   - 3D: Each element is `[v0, v1, v2]` defining a triangle
    ///   - 2D: Each element is `[v0, v1]` defining a line segment
    ///
    /// * `resolution` - Number of voxel subdivisions along the longest axis of the shape's
    ///   bounding box. Higher values give more detail but use more memory.
    ///   For example, `resolution = 10` creates approximately 10 voxels along the longest dimension.
    ///
    /// * `fill_mode` - Controls which voxels are marked as filled:
    ///   - [`FillMode::SurfaceOnly`]: Only voxels intersecting the boundary
    ///   - [`FillMode::FloodFill`]: Surface voxels plus interior voxels
    ///
    /// * `keep_voxel_to_primitives_map` - If `true`, stores which primitives (triangles
    ///   or segments) intersect each voxel. Required for [`compute_exact_convex_hull()`]
    ///   and [`compute_primitive_intersections()`], but uses additional memory.
    ///
    /// # Returns
    ///
    /// A sparse `VoxelSet` containing only the filled voxels.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::transformation::voxelization::{FillMode, VoxelSet};
    /// use parry3d::shape::Cuboid;
    /// use parry3d::math::Vector;
    ///
    /// // Create a cuboid: 2 units wide (x), 1 unit tall (y), 0.5 units deep (z)
    /// let cuboid = Cuboid::new(Vector::new(1.0, 0.5, 0.25));
    /// let (vertices, indices) = cuboid.to_trimesh();
    ///
    /// // Voxelize with 20 subdivisions along the longest axis (x = 2.0)
    /// // Other axes will be proportionally subdivided to maintain cubic voxels
    /// let voxels = VoxelSet::voxelize(
    ///     &vertices,
    ///     &indices,
    ///     20,                           // 20 voxels along x-axis
    ///     FillMode::FloodFill {
    ///         detect_cavities: false,
    ///     },
    ///     false,
    /// );
    ///
    /// println!("Created {} voxels", voxels.len());
    /// println!("Voxel scale: {}", voxels.scale);  // automatically computed
    /// println!("Total volume: {}", voxels.compute_volume());
    /// # }
    /// ```
    ///
    /// # Choosing Resolution
    ///
    /// - **Low (5-10)**: Fast, coarse approximation, good for rough collision proxies
    /// - **Medium (10-30)**: Balanced detail and performance, suitable for most use cases
    /// - **High (50+)**: Fine detail, high memory usage, used for precise volume computation
    ///
    /// [`compute_exact_convex_hull()`]: VoxelSet::compute_exact_convex_hull
    /// [`compute_primitive_intersections()`]: VoxelSet::compute_primitive_intersections
    pub fn voxelize(
        points: &[Vector],
        indices: &[[u32; DIM]],
        resolution: u32,
        fill_mode: FillMode,
        keep_voxel_to_primitives_map: bool,
    ) -> Self {
        VoxelizedVolume::voxelize(
            points,
            indices,
            resolution,
            fill_mode,
            keep_voxel_to_primitives_map,
        )
        .into()
    }

    /// The minimal coordinates of the integer bounding-box of the voxels in this set.
    pub fn min_bb_voxels(&self) -> IVector {
        self.min_bb_voxels
    }

    /// The maximal coordinates of the integer bounding-box of the voxels in this set.
    pub fn max_bb_voxels(&self) -> IVector {
        self.max_bb_voxels
    }

    /// Computes the total volume occupied by all voxels in this set.
    ///
    /// This calculates the approximate volume by multiplying the number of filled voxels
    /// by the volume of each individual voxel. The result is an approximation of the
    /// volume of the original shape.
    ///
    /// # Accuracy
    ///
    /// The accuracy depends on the voxelization resolution:
    /// - Higher resolution (smaller voxels) → more accurate volume
    /// - Lower resolution (larger voxels) → faster but less accurate
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::transformation::voxelization::{FillMode, VoxelSet};
    /// use parry3d::shape::Ball;
    ///
    /// let ball = Ball::new(1.0);
    /// let (vertices, indices) = ball.to_trimesh(20, 20);
    ///
    /// let voxels = VoxelSet::voxelize(
    ///     &vertices,
    ///     &indices,
    ///     30,  // Higher resolution for better accuracy
    ///     FillMode::FloodFill { detect_cavities: false },
    ///     false,
    /// );
    ///
    /// let voxel_volume = voxels.compute_volume();
    /// let expected_volume = 4.0 / 3.0 * std::f32::consts::PI * 1.0_f32.powi(3);
    ///
    /// println!("Voxel volume: {:.3}", voxel_volume);
    /// println!("Expected volume: {:.3}", expected_volume);
    /// println!("Error: {:.1}%", ((voxel_volume - expected_volume).abs() / expected_volume * 100.0));
    /// # }
    /// ```
    pub fn compute_volume(&self) -> Real {
        self.voxel_volume() * self.voxels.len() as Real
    }

    /// Converts voxel grid coordinates to world-space coordinates.
    ///
    /// Given a voxel, this computes the world-space position of the voxel's center.
    /// The conversion formula is:
    ///
    /// ```text
    /// world_position = origin + (voxel + 0.5) * scale
    /// ```
    ///
    /// Note that we add 0.5 to get the center of the voxel rather than its corner.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::transformation::voxelization::{FillMode, VoxelSet};
    /// use parry3d::shape::Cuboid;
    /// use parry3d::math::Vector;
    ///
    /// let cuboid = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
    /// let (vertices, indices) = cuboid.to_trimesh();
    ///
    /// let voxels = VoxelSet::voxelize(
    ///     &vertices,
    ///     &indices,
    ///     10,
    ///     FillMode::SurfaceOnly,
    ///     false,
    /// );
    ///
    /// // Convert grid coordinates to world coordinates
    /// for voxel in voxels.voxels() {
    ///     let grid_coords = voxel;
    ///     let world_coords = voxels.get_voxel_point(voxel);
    ///
    ///     println!("Grid: {:?} -> World: {:?}", grid_coords, world_coords);
    /// }
    /// # }
    /// ```
    pub fn get_voxel_point(&self, voxel: &Voxel) -> Vector {
        #[cfg(feature = "dim2")]
        let coords = Vector::new(voxel.coords.x as Real, voxel.coords.y as Real);
        #[cfg(feature = "dim3")]
        let coords = Vector::new(
            voxel.coords.x as Real,
            voxel.coords.y as Real,
            voxel.coords.z as Real,
        );
        self.get_point(coords)
    }

    pub(crate) fn get_point(&self, voxel: Vector) -> Vector {
        self.origin + voxel * self.scale
    }

    /// Does this voxel not contain any element?
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The number of voxels in this set.
    pub fn len(&self) -> usize {
        self.voxels.len()
    }

    /// The set of voxels.
    pub fn voxels(&self) -> &[Voxel] {
        &self.voxels
    }

    /// Update the bounding box of this voxel set.
    pub fn compute_bb(&mut self) {
        let num_voxels = self.voxels.len();

        if num_voxels == 0 {
            return;
        }

        self.min_bb_voxels = self.voxels[0].coords;
        self.max_bb_voxels = self.voxels[0].coords;

        for p in 0..num_voxels {
            self.min_bb_voxels = self.min_bb_voxels.min(self.voxels[p].coords);
            self.max_bb_voxels = self.max_bb_voxels.max(self.voxels[p].coords);
        }
    }

    /// Computes a precise convex hull by clipping primitives against voxel boundaries.
    ///
    /// This method produces a more accurate convex hull than [`compute_convex_hull()`] by:
    /// 1. Finding which primitives (triangles/segments) intersect each voxel
    /// 2. Clipping those primitives to the voxel boundaries
    /// 3. Computing the convex hull from the clipped geometry
    ///
    /// This approach gives much tighter convex hulls, especially at lower resolutions, because
    /// it uses the actual intersection geometry rather than just voxel centers.
    ///
    /// # Requirements
    ///
    /// This method requires that the voxel set was created with `keep_voxel_to_primitives_map = true`.
    /// Otherwise, this method will panic.
    ///
    /// # Parameters
    ///
    /// * `points` - The same vertex buffer used during voxelization
    /// * `indices` - The same index buffer used during voxelization
    ///
    /// # Returns
    ///
    /// In 2D: A vector of points forming the convex hull polygon
    /// In 3D: A tuple of `(vertices, triangle_indices)` forming the convex hull mesh
    ///
    /// # Panics
    ///
    /// Panics if this `VoxelSet` was created with `keep_voxel_to_primitives_map = false`.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::transformation::voxelization::{FillMode, VoxelSet};
    /// use parry3d::shape::Cuboid;
    /// use parry3d::math::Vector;
    ///
    /// let cuboid = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
    /// let (vertices, indices) = cuboid.to_trimesh();
    ///
    /// // IMPORTANT: Set keep_voxel_to_primitives_map = true
    /// let voxels = VoxelSet::voxelize(
    ///     &vertices,
    ///     &indices,
    ///     10,
    ///     FillMode::SurfaceOnly,
    ///     true,  // Enable primitive mapping
    /// );
    ///
    /// // Compute exact convex hull using triangle clipping
    /// let (hull_vertices, hull_indices) = voxels.compute_exact_convex_hull(&vertices, &indices);
    ///
    /// println!("Exact convex hull: {} vertices, {} triangles",
    ///          hull_vertices.len(), hull_indices.len());
    /// # }
    /// ```
    ///
    /// # Comparison with `compute_convex_hull()`
    ///
    /// - `compute_exact_convex_hull()`: More accurate, requires primitive mapping, slower
    /// - `compute_convex_hull()`: Approximate, uses voxel centers, faster
    ///
    /// [`compute_convex_hull()`]: VoxelSet::compute_convex_hull
    #[cfg(feature = "dim2")]
    pub fn compute_exact_convex_hull(
        &self,
        points: &[Vector],
        indices: &[[u32; DIM]],
    ) -> Vec<Vector> {
        self.do_compute_exact_convex_hull(points, indices)
    }

    /// Computes a precise convex hull by clipping primitives against voxel boundaries.
    ///
    /// This method produces a more accurate convex hull than [`compute_convex_hull()`] by:
    /// 1. Finding which primitives (triangles/segments) intersect each voxel
    /// 2. Clipping those primitives to the voxel boundaries
    /// 3. Computing the convex hull from the clipped geometry
    ///
    /// This approach gives much tighter convex hulls, especially at lower resolutions, because
    /// it uses the actual intersection geometry rather than just voxel centers.
    ///
    /// # Requirements
    ///
    /// This method requires that the voxel set was created with `keep_voxel_to_primitives_map = true`.
    /// Otherwise, this method will panic.
    ///
    /// # Parameters
    ///
    /// * `points` - The same vertex buffer used during voxelization
    /// * `indices` - The same index buffer used during voxelization
    ///
    /// # Returns
    ///
    /// In 2D: A vector of points forming the convex hull polygon
    /// In 3D: A tuple of `(vertices, triangle_indices)` forming the convex hull mesh
    ///
    /// # Panics
    ///
    /// Panics if this `VoxelSet` was created with `keep_voxel_to_primitives_map = false`.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::transformation::voxelization::{FillMode, VoxelSet};
    /// use parry3d::shape::Cuboid;
    /// use parry3d::math::Vector;
    ///
    /// let cuboid = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
    /// let (vertices, indices) = cuboid.to_trimesh();
    ///
    /// // IMPORTANT: Set keep_voxel_to_primitives_map = true
    /// let voxels = VoxelSet::voxelize(
    ///     &vertices,
    ///     &indices,
    ///     10,
    ///     FillMode::SurfaceOnly,
    ///     true,  // Enable primitive mapping
    /// );
    ///
    /// // Compute exact convex hull using triangle clipping
    /// let (hull_vertices, hull_indices) = voxels.compute_exact_convex_hull(&vertices, &indices);
    ///
    /// println!("Exact convex hull: {} vertices, {} triangles",
    ///          hull_vertices.len(), hull_indices.len());
    /// # }
    /// ```
    ///
    /// # Comparison with `compute_convex_hull()`
    ///
    /// - `compute_exact_convex_hull()`: More accurate, requires primitive mapping, slower
    /// - `compute_convex_hull()`: Approximate, uses voxel centers, faster
    ///
    /// [`compute_convex_hull()`]: VoxelSet::compute_convex_hull
    #[cfg(feature = "dim3")]
    pub fn compute_exact_convex_hull(
        &self,
        points: &[Vector],
        indices: &[[u32; DIM]],
    ) -> (Vec<Vector>, Vec<[u32; DIM]>) {
        self.do_compute_exact_convex_hull(points, indices)
    }

    fn do_compute_exact_convex_hull(
        &self,
        points: &[Vector],
        indices: &[[u32; DIM]],
    ) -> ConvexHull {
        assert!(!self.intersections.is_empty(),
                "Cannot compute exact convex hull without voxel-to-primitives-map. Consider passing voxel_to_primitives_map = true to the voxelizer.");
        let mut surface_points = Vec::new();
        #[cfg(feature = "dim3")]
        let (mut polygon, mut workspace) = (Vec::new(), Vec::new());
        let mut pushed_points = vec![false; points.len()];

        // Grab all the points.
        for voxel in self.voxels.iter().filter(|v| v.is_on_surface) {
            let intersections =
                &self.intersections[voxel.intersections_range.0..voxel.intersections_range.1];
            for prim_id in intersections {
                let ia = indices[*prim_id as usize][0] as usize;
                let ib = indices[*prim_id as usize][1] as usize;
                #[cfg(feature = "dim3")]
                let ic = indices[*prim_id as usize][2] as usize;

                // If the primitives have been classified by VHACD, we know that:
                // - A class equal to Some(u32::MAX) means that the primitives intersects multiple
                //   convex parts, so we need to split it.
                // - A class equal to None means that we did not compute any classes (so we
                //   must assume that each triangle have to be split since it may intersect
                //   multiple parts.
                // - A class different from `None` and `Some(u32::MAX)` means that the triangle is
                //   included in only one convex part. So instead of cutting it, just push the whole
                //   triangle once.
                let prim_class = self.primitive_classes.get(*prim_id as usize).copied();
                if prim_class == Some(u32::MAX) || prim_class.is_none() {
                    let aabb_center = self.origin + ivect_to_vect(voxel.coords) * self.scale;
                    let aabb =
                        Aabb::from_half_extents(aabb_center, Vector::splat(self.scale / 2.0));

                    #[cfg(feature = "dim2")]
                    if let Some(seg) = aabb.clip_segment(points[ia], points[ib]) {
                        surface_points.push(seg.a);
                        surface_points.push(seg.b);
                    }

                    #[cfg(feature = "dim3")]
                    {
                        polygon.clear();
                        polygon.extend_from_slice(&[points[ia], points[ib], points[ic]]);
                        aabb.clip_polygon_with_workspace(&mut polygon, &mut workspace);
                        surface_points.append(&mut polygon);
                    }
                } else {
                    // We know this triangle is only contained by
                    // one voxel set, i.e., `self`. So we don't
                    // need to cut it.
                    //
                    // Because one triangle may intersect multiple voxels contained by
                    // the same convex part, we only push vertices we have not pushed
                    // so far in order to avoid some useless duplicate points (duplicate
                    // points are OK as far as convex hull computation is concerned, but
                    // they imply some redundant computations).
                    let mut push_pt = |i: usize| {
                        if !pushed_points[i] {
                            surface_points.push(points[i]);
                            pushed_points[i] = true;
                        }
                    };

                    push_pt(ia);
                    push_pt(ib);
                    #[cfg(feature = "dim3")]
                    push_pt(ic);
                }
            }

            if intersections.is_empty() {
                self.map_voxel_points(voxel, |p| surface_points.push(p));
            }
        }

        // Compute the convex-hull.
        convex_hull(&surface_points)
    }

    /// Computes the intersections between all the voxels of this voxel set,
    /// and all the primitives (triangle or segments) it intersected (as per
    /// the voxel-to-primitives-map computed during voxelization).
    ///
    /// Panics if the voxelization was performed without setting the parameter
    /// `voxel_to_primitives_map = true`.
    pub fn compute_primitive_intersections(
        &self,
        points: &[Vector],
        indices: &[[u32; DIM]],
    ) -> Vec<Vector> {
        assert!(!self.intersections.is_empty(),
                "Cannot compute primitive intersections voxel-to-primitives-map. Consider passing voxel_to_primitives_map = true to the voxelizer.");
        let mut surface_points = Vec::new();
        #[cfg(feature = "dim3")]
        let (mut polygon, mut workspace) = (Vec::new(), Vec::new());

        // Grab all the points.
        for voxel in self.voxels.iter().filter(|v| v.is_on_surface) {
            let intersections =
                &self.intersections[voxel.intersections_range.0..voxel.intersections_range.1];
            for prim_id in intersections {
                let aabb_center = self.origin + ivect_to_vect(voxel.coords) * self.scale;
                let aabb = Aabb::from_half_extents(aabb_center, Vector::splat(self.scale / 2.0));

                let pa = points[indices[*prim_id as usize][0] as usize];
                let pb = points[indices[*prim_id as usize][1] as usize];
                #[cfg(feature = "dim3")]
                let pc = points[indices[*prim_id as usize][2] as usize];

                #[cfg(feature = "dim2")]
                if let Some(seg) = aabb.clip_segment(pa, pb) {
                    surface_points.push(seg.a);
                    surface_points.push(seg.b);
                }

                #[cfg(feature = "dim3")]
                {
                    workspace.clear();
                    polygon.clear();
                    polygon.extend_from_slice(&[pa, pb, pc]);
                    aabb.clip_polygon_with_workspace(&mut polygon, &mut workspace);

                    if polygon.len() > 2 {
                        for i in 1..polygon.len() - 1 {
                            surface_points.push(polygon[0]);
                            surface_points.push(polygon[i]);
                            surface_points.push(polygon[i + 1]);
                        }
                    }
                }
            }
        }

        surface_points
    }

    /// Compute the convex-hull of the voxels in this set.
    ///
    /// # Parameters
    /// * `sampling` - The convex-hull computation will ignore `sampling` voxels at
    ///   regular intervals. Useful to save some computation times if an exact result isn't need.
    ///   Use `0` to make sure no voxel is being ignored.
    #[cfg(feature = "dim2")]
    pub fn compute_convex_hull(&self, sampling: u32) -> Vec<Vector> {
        let mut points = Vec::new();

        // Grab all the points.
        for voxel in self
            .voxels
            .iter()
            .filter(|v| v.is_on_surface)
            .step_by(sampling as usize)
        {
            self.map_voxel_points(voxel, |p| points.push(p));
        }

        // Compute the convex-hull.
        convex_hull(&points)
    }

    /// Compute the convex-hull of the voxels in this set.
    ///
    /// # Parameters
    /// * `sampling` - The convex-hull computation will ignore `sampling` voxels at
    ///   regular intervals. Useful to save some computation times if an exact result isn't need.
    ///   Use `0` to make sure no voxel is being ignored.
    #[cfg(feature = "dim3")]
    pub fn compute_convex_hull(&self, sampling: u32) -> (Vec<Vector>, Vec<[u32; DIM]>) {
        let mut points = Vec::new();

        // Grab all the points.
        for voxel in self
            .voxels
            .iter()
            .filter(|v| v.is_on_surface)
            .step_by(sampling as usize)
        {
            self.map_voxel_points(voxel, |p| points.push(p));
        }

        // Compute the convex-hull.
        convex_hull(&points)
    }

    /// Gets the vertices of the given voxel.
    fn map_voxel_points(&self, voxel: &Voxel, mut f: impl FnMut(Vector)) {
        let ijk = ivect_to_vect(voxel.coords);

        #[cfg(feature = "dim2")]
        let shifts = [
            Vector::new(-0.5, -0.5),
            Vector::new(0.5, -0.5),
            Vector::new(0.5, 0.5),
            Vector::new(-0.5, 0.5),
        ];

        #[cfg(feature = "dim3")]
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
            f(self.origin + (ijk + *shift) * self.scale)
        }
    }

    pub(crate) fn intersect(
        &self,
        plane: &CutPlane,
        positive_pts: &mut Vec<Vector>,
        negative_pts: &mut Vec<Vector>,
        sampling: u32,
    ) {
        let num_voxels = self.voxels.len();

        if num_voxels == 0 {
            return;
        }

        let d0 = self.scale;
        let mut sp = 0;
        let mut sn = 0;

        for v in 0..num_voxels {
            let voxel = self.voxels[v];
            let pt = self.get_voxel_point(&voxel);
            let d = plane.abc.dot(pt) + plane.d;

            // if      (d >= 0.0 && d <= d0) positive_pts.push(pt);
            // else if (d < 0.0 && -d <= d0) negative_pts.push(pt);

            if d >= 0.0 {
                if d <= d0 {
                    self.map_voxel_points(&voxel, |p| positive_pts.push(p));
                } else {
                    sp += 1;

                    if sp == sampling {
                        self.map_voxel_points(&voxel, |p| positive_pts.push(p));
                        sp = 0;
                    }
                }
            } else if -d <= d0 {
                self.map_voxel_points(&voxel, |p| negative_pts.push(p));
            } else {
                sn += 1;
                if sn == sampling {
                    self.map_voxel_points(&voxel, |p| negative_pts.push(p));
                    sn = 0;
                }
            }
        }
    }

    // Returns (negative_volume, positive_volume)
    pub(crate) fn compute_clipped_volumes(&self, plane: &CutPlane) -> (Real, Real) {
        if self.voxels.is_empty() {
            return (0.0, 0.0);
        }

        let mut num_positive_voxels = 0;

        for voxel in &self.voxels {
            let pt = self.get_voxel_point(voxel);
            let d = plane.abc.dot(pt) + plane.d;
            num_positive_voxels += (d >= 0.0) as usize;
        }

        let num_negative_voxels = self.voxels.len() - num_positive_voxels;
        let positive_volume = self.voxel_volume() * (num_positive_voxels as Real);
        let negative_volume = self.voxel_volume() * (num_negative_voxels as Real);

        (negative_volume, positive_volume)
    }

    // Set `on_surf` such that it contains only the voxel on surface contained by `self`.
    pub(crate) fn select_on_surface(&self, on_surf: &mut VoxelSet) {
        on_surf.origin = self.origin;
        on_surf.voxels.clear();
        on_surf.scale = self.scale;

        for voxel in &self.voxels {
            if voxel.is_on_surface {
                on_surf.voxels.push(*voxel);
            }
        }
    }

    /// Splits this voxel set into two parts, depending on where the voxel center lies wrt. the given plane.
    pub(crate) fn clip(
        &self,
        plane: &CutPlane,
        positive_part: &mut VoxelSet,
        negative_part: &mut VoxelSet,
    ) {
        let num_voxels = self.voxels.len();

        if num_voxels == 0 {
            return;
        }

        negative_part.origin = self.origin;
        negative_part.voxels.clear();
        negative_part.voxels.reserve(num_voxels);
        negative_part.scale = self.scale;

        positive_part.origin = self.origin;
        positive_part.voxels.clear();
        positive_part.voxels.reserve(num_voxels);
        positive_part.scale = self.scale;

        let d0 = self.scale;

        for v in 0..num_voxels {
            let mut voxel = self.voxels[v];
            let pt = self.get_voxel_point(&voxel);
            let d = plane.abc.dot(pt) + plane.d;

            if d >= 0.0 {
                if voxel.is_on_surface || d <= d0 {
                    voxel.is_on_surface = true;
                    positive_part.voxels.push(voxel);
                } else {
                    positive_part.voxels.push(voxel);
                }
            } else if voxel.is_on_surface || -d <= d0 {
                voxel.is_on_surface = true;
                negative_part.voxels.push(voxel);
            } else {
                negative_part.voxels.push(voxel);
            }
        }
    }

    /// Convert `self` into a mesh, including only the voxels on the surface or only the voxel
    /// inside of the volume.
    #[cfg(feature = "dim3")]
    pub fn to_trimesh(
        &self,
        base_index: u32,
        is_on_surface: bool,
    ) -> (Vec<Vector>, Vec<[u32; DIM]>) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        for voxel in &self.voxels {
            if voxel.is_on_surface == is_on_surface {
                self.map_voxel_points(voxel, |p| vertices.push(p));

                indices.push([base_index, base_index + 2, base_index + 1]);
                indices.push([base_index, base_index + 3, base_index + 2]);
                indices.push([base_index + 4, base_index + 5, base_index + 6]);
                indices.push([base_index + 4, base_index + 6, base_index + 7]);
                indices.push([base_index + 7, base_index + 6, base_index + 2]);
                indices.push([base_index + 7, base_index + 2, base_index + 3]);
                indices.push([base_index + 4, base_index + 1, base_index + 5]);
                indices.push([base_index + 4, base_index, base_index + 1]);
                indices.push([base_index + 6, base_index + 5, base_index + 1]);
                indices.push([base_index + 6, base_index + 1, base_index + 2]);
                indices.push([base_index + 7, base_index, base_index + 4]);
                indices.push([base_index + 7, base_index + 3, base_index]);
            }
        }

        (vertices, indices)
    }

    pub(crate) fn compute_principal_axes(&self) -> Vector {
        let num_voxels = self.voxels.len();
        if num_voxels == 0 {
            return Vector::ZERO;
        }

        // TODO: find a way to reuse crate::utils::cov?
        // The difficulty being that we need to iterate through the set of
        // points twice. So passing an iterator to crate::utils::cov
        // isn't really possible.
        let mut center = Vector::ZERO;
        let denom = 1.0 / (num_voxels as Real);

        for voxel in &self.voxels {
            center += ivect_to_vect(voxel.coords) * denom;
        }

        let mut cov_mat = Matrix::ZERO;
        for voxel in &self.voxels {
            let xyz = ivect_to_vect(voxel.coords) - center;
            cov_mat += xyz.kronecker(xyz * denom);
        }
        cov_mat.symmetric_eigenvalues()
    }

    pub(crate) fn compute_axes_aligned_clipping_planes(
        &self,
        downsampling: u32,
        planes: &mut Vec<CutPlane>,
    ) {
        let min_v = self.min_bb_voxels();
        let max_v = self.max_bb_voxels();

        for dim in 0..DIM {
            let i0 = min_v[dim];
            let i1 = max_v[dim];

            for i in (i0..=i1).step_by(downsampling as usize) {
                let plane = CutPlane {
                    abc: Vector::ith(dim, 1.0),
                    axis: dim as u8,
                    d: -(self.origin[dim] + (i as Real + 0.5) * self.scale),
                    index: i as u32,
                };

                planes.push(plane);
            }
        }
    }
}

#[cfg(feature = "dim2")]
fn convex_hull(vertices: &[Vector]) -> Vec<Vector> {
    if vertices.len() > 1 {
        crate::transformation::convex_hull(vertices)
    } else {
        Vec::new()
    }
}

#[cfg(feature = "dim3")]
fn convex_hull(vertices: &[Vector]) -> (Vec<Vector>, Vec<[u32; DIM]>) {
    if vertices.len() > 2 {
        crate::transformation::convex_hull(vertices)
    } else {
        (Vec::new(), Vec::new())
    }
}

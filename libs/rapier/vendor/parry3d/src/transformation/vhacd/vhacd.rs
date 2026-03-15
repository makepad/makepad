// Rust port, with modifications, of https://github.com/kmammou/v-hacd/blob/master/src/VHACD_Lib/src/VHACD.cpp
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

use crate::math::{Int, Real, Vector, VectorExt, DIM};
use crate::transformation::vhacd::VHACDParameters;
use crate::transformation::voxelization::{VoxelSet, VoxelizedVolume};
use alloc::sync::Arc;
use alloc::vec::Vec;

#[cfg(feature = "dim2")]
type ConvexHull = Vec<Vector>;
#[cfg(feature = "dim3")]
type ConvexHull = (Vec<Vector>, Vec<[u32; 3]>);

#[derive(Copy, Clone, Debug)]
pub(crate) struct CutPlane {
    pub abc: Vector,
    pub d: Real,
    pub axis: u8,
    pub index: u32,
}

/// Approximate convex decomposition using the VHACD algorithm.
///
/// This structure holds the result of the V-HACD (Volumetric Hierarchical Approximate
/// Convex Decomposition) algorithm, which decomposes a concave shape into multiple
/// approximately-convex parts.
///
/// # Overview
///
/// The `VHACD` struct stores the decomposition result as a collection of voxelized parts,
/// where each part is approximately convex. These parts can be converted to convex hulls
/// for use in collision detection and physics simulation.
///
/// # Basic Workflow
///
/// 1. **Decompose**: Create a `VHACD` instance using [`VHACD::decompose`] or [`VHACD::from_voxels`]
/// 2. **Access Parts**: Get the voxelized parts using [`voxel_parts`](VHACD::voxel_parts)
/// 3. **Generate Hulls**: Compute convex hulls with [`compute_convex_hulls`](VHACD::compute_convex_hulls)
///    or [`compute_exact_convex_hulls`](VHACD::compute_exact_convex_hulls)
///
/// # Examples
///
/// ## Basic Usage
///
/// ```no_run
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::math::Vector;
/// use parry3d::transformation::vhacd::{VHACD, VHACDParameters};
///
/// // Define a simple mesh (tetrahedron)
/// let vertices = vec![
///     Vector::new(0.0, 0.0, 0.0),
///     Vector::new(1.0, 0.0, 0.0),
///     Vector::new(0.5, 1.0, 0.0),
///     Vector::new(0.5, 0.5, 1.0),
/// ];
/// let indices = vec![
///     [0, 1, 2], [0, 2, 3], [0, 3, 1], [1, 3, 2],
/// ];
///
/// // Decompose with default parameters
/// let decomposition = VHACD::decompose(
///     &VHACDParameters::default(),
///     &vertices,
///     &indices,
///     false,
/// );
///
/// // Access the results
/// println!("Generated {} parts", decomposition.voxel_parts().len());
///
/// // Get convex hulls for collision detection
/// let hulls = decomposition.compute_convex_hulls(4);
/// # }
/// ```
///
/// ## With Custom Parameters
///
/// ```no_run
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::math::Vector;
/// use parry3d::transformation::vhacd::{VHACD, VHACDParameters};
///
/// # let vertices = vec![
/// #     Vector::new(0.0, 0.0, 0.0), Vector::new(1.0, 0.0, 0.0),
/// #     Vector::new(0.5, 1.0, 0.0), Vector::new(0.5, 0.5, 1.0),
/// # ];
/// # let indices = vec![[0, 1, 2], [0, 2, 3], [0, 3, 1], [1, 3, 2]];
/// #
/// // High-quality decomposition settings
/// let params = VHACDParameters {
///     resolution: 128,
///     concavity: 0.001,
///     max_convex_hulls: 32,
///     ..Default::default()
/// };
///
/// let decomposition = VHACD::decompose(&params, &vertices, &indices, false);
/// # }
/// ```
///
/// # See Also
///
/// - [`VHACDParameters`]: Configuration for the decomposition algorithm
/// - [`compute_convex_hulls`](VHACD::compute_convex_hulls): Generate convex hulls from voxels
/// - [`compute_exact_convex_hulls`](VHACD::compute_exact_convex_hulls): Generate hulls from original mesh
/// - Module documentation: [`crate::transformation::vhacd`]
pub struct VHACD {
    // raycast_mesh: Option<RaycastMesh>,
    voxel_parts: Vec<VoxelSet>,
    volume_ch0: Real,
    max_concavity: Real,
}

impl VHACD {
    /// Decompose the given polyline (in 2D) or triangle mesh (in 3D).
    ///
    /// This is the primary method for performing approximate convex decomposition. It takes
    /// a mesh defined by vertices and indices, voxelizes it, and decomposes it into
    /// approximately-convex parts using the V-HACD algorithm.
    ///
    /// # Parameters
    ///
    /// * `params` - Configuration parameters controlling the decomposition process.
    ///   See [`VHACDParameters`] for details on each parameter.
    ///
    /// * `points` - The vertex positions of your mesh (3D) or polyline (2D).
    ///   Each point represents a vertex in world space.
    ///
    /// * `indices` - The connectivity information:
    ///   - **3D**: Triangle indices `[u32; 3]` - each entry defines a triangle using 3 vertex indices
    ///   - **2D**: Segment indices `[u32; 2]` - each entry defines a line segment using 2 vertex indices
    ///
    /// * `keep_voxel_to_primitives_map` - Whether to maintain a mapping between voxels and
    ///   the original mesh primitives (triangles/segments) they intersect.
    ///   - **`true`**: Enables [`compute_exact_convex_hulls`](VHACD::compute_exact_convex_hulls)
    ///     which uses original mesh geometry for more accurate results. Uses more memory.
    ///   - **`false`**: Only voxel-based hulls available via [`compute_convex_hulls`](VHACD::compute_convex_hulls).
    ///     More memory efficient.
    ///
    /// # Returns
    ///
    /// A `VHACD` instance containing the decomposition results. Use [`voxel_parts`](VHACD::voxel_parts)
    /// to access the raw voxelized parts, or [`compute_convex_hulls`](VHACD::compute_convex_hulls)
    /// to generate convex hull geometry.
    ///
    /// # Performance
    ///
    /// Decomposition time depends primarily on:
    /// - **`resolution`**: Higher = slower (cubic scaling in 3D)
    /// - **`max_convex_hulls`**: More parts = more splits = longer time
    /// - **Mesh complexity**: More vertices/triangles = slower voxelization
    ///
    /// Typical times (on modern CPU):
    /// - Simple mesh (1K triangles, resolution 64): 100-500ms
    /// - Complex mesh (10K triangles, resolution 128): 1-5 seconds
    /// - Very complex (100K triangles, resolution 256): 10-60 seconds
    ///
    /// # Examples
    ///
    /// ## Basic Decomposition
    ///
    /// ```no_run
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::math::Vector;
    /// use parry3d::transformation::vhacd::{VHACD, VHACDParameters};
    ///
    /// // Simple L-shaped mesh
    /// let vertices = vec![
    ///     Vector::new(0.0, 0.0, 0.0), Vector::new(2.0, 0.0, 0.0),
    ///     Vector::new(2.0, 1.0, 0.0), Vector::new(1.0, 1.0, 0.0),
    /// ];
    /// let indices = vec![
    ///     [0, 1, 2], [0, 2, 3],
    /// ];
    ///
    /// let decomposition = VHACD::decompose(
    ///     &VHACDParameters::default(),
    ///     &vertices,
    ///     &indices,
    ///     false, // Don't need exact hulls
    /// );
    ///
    /// println!("Parts: {}", decomposition.voxel_parts().len());
    /// # }
    /// ```
    ///
    /// ## With Exact Hull Generation
    ///
    /// ```no_run
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::math::Vector;
    /// use parry3d::transformation::vhacd::{VHACD, VHACDParameters};
    ///
    /// # let vertices = vec![
    /// #     Vector::new(0.0, 0.0, 0.0), Vector::new(1.0, 0.0, 0.0),
    /// #     Vector::new(0.5, 1.0, 0.0), Vector::new(0.5, 0.5, 1.0),
    /// # ];
    /// # let indices = vec![[0, 1, 2], [0, 2, 3], [0, 3, 1], [1, 3, 2]];
    /// #
    /// // Enable voxel-to-primitive mapping for exact hulls
    /// let decomposition = VHACD::decompose(
    ///     &VHACDParameters::default(),
    ///     &vertices,
    ///     &indices,
    ///     true, // <-- Enable for exact hulls
    /// );
    ///
    /// // Now we can compute exact hulls using original mesh
    /// let exact_hulls = decomposition.compute_exact_convex_hulls(&vertices, &indices);
    /// # }
    /// ```
    ///
    /// ## Custom Parameters
    ///
    /// ```no_run
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::math::Vector;
    /// use parry3d::transformation::vhacd::{VHACD, VHACDParameters};
    ///
    /// # let vertices = vec![
    /// #     Vector::new(0.0, 0.0, 0.0), Vector::new(1.0, 0.0, 0.0),
    /// #     Vector::new(0.5, 1.0, 0.0), Vector::new(0.5, 0.5, 1.0),
    /// # ];
    /// # let indices = vec![[0, 1, 2], [0, 2, 3], [0, 3, 1], [1, 3, 2]];
    /// #
    /// // High-quality settings for important objects
    /// let params = VHACDParameters {
    ///     resolution: 128,        // High detail
    ///     concavity: 0.001,       // Tight fit
    ///     max_convex_hulls: 32,   // Allow many parts
    ///     ..Default::default()
    /// };
    ///
    /// let decomposition = VHACD::decompose(&params, &vertices, &indices, false);
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`VHACDParameters`]: Detailed parameter documentation
    /// - [`from_voxels`](VHACD::from_voxels): Decompose pre-voxelized data
    /// - [`compute_convex_hulls`](VHACD::compute_convex_hulls): Generate voxel-based hulls
    /// - [`compute_exact_convex_hulls`](VHACD::compute_exact_convex_hulls): Generate mesh-based hulls
    pub fn decompose(
        params: &VHACDParameters,
        points: &[Vector],
        indices: &[[u32; DIM]],
        keep_voxel_to_primitives_map: bool,
    ) -> Self {
        // if params.project_hull_vertices || params.fill_mode == FillMode::RAYCAST_FILL {
        //     self.raycast_mesh =
        //         RaycastMesh::create_raycast_mesh(num_points, points, num_triangles, triangles);
        // }

        let voxelized = VoxelizedVolume::voxelize(
            points,
            indices,
            params.resolution,
            params.fill_mode,
            keep_voxel_to_primitives_map,
            // &self.raycast_mesh,
        );

        let mut result = Self::from_voxels(params, voxelized.into());

        let primitive_classes = Arc::new(result.classify_primitives(indices.len()));
        for part in &mut result.voxel_parts {
            part.primitive_classes = primitive_classes.clone();
        }

        result
    }

    /// Perform an approximate convex decomposition of a pre-voxelized set of voxels.
    ///
    /// This method allows you to decompose a shape that has already been voxelized,
    /// bypassing the voxelization step. This is useful if you:
    /// - Already have voxelized data from another source
    /// - Want to decompose the same voxelization with different parameters
    /// - Need more control over the voxelization process
    ///
    /// # Parameters
    ///
    /// * `params` - Configuration parameters for the decomposition algorithm.
    ///   See [`VHACDParameters`] for details.
    ///
    /// * `voxels` - A pre-voxelized volume represented as a [`VoxelSet`].
    ///   You can create this using [`VoxelizedVolume::voxelize`] or other voxelization methods.
    ///
    /// # Returns
    ///
    /// A `VHACD` instance containing the decomposition results.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::math::Vector;
    /// use parry3d::transformation::vhacd::{VHACD, VHACDParameters};
    /// use parry3d::transformation::voxelization::{VoxelizedVolume, FillMode};
    ///
    /// # let vertices = vec![
    /// #     Vector::new(0.0, 0.0, 0.0), Vector::new(1.0, 0.0, 0.0),
    /// #     Vector::new(0.5, 1.0, 0.0), Vector::new(0.5, 0.5, 1.0),
    /// # ];
    /// # let indices = vec![[0, 1, 2], [0, 2, 3], [0, 3, 1], [1, 3, 2]];
    /// #
    /// // First, voxelize the mesh manually
    /// let voxelized = VoxelizedVolume::voxelize(
    ///     &vertices,
    ///     &indices,
    ///     64, // resolution
    ///     FillMode::FloodFill {
    ///         detect_cavities: false,
    ///     },
    ///     false, // don't keep primitive mapping
    /// );
    ///
    /// // Then decompose the voxels
    /// let decomposition = VHACD::from_voxels(
    ///     &VHACDParameters::default(),
    ///     voxelized.into(),
    /// );
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`decompose`](VHACD::decompose): Decompose directly from mesh (includes voxelization)
    /// - [`VoxelizedVolume`]: For manual voxelization
    /// - [`VoxelSet`]: The voxel data structure
    ///
    /// [`VoxelizedVolume::voxelize`]: crate::transformation::voxelization::VoxelizedVolume::voxelize
    /// [`VoxelSet`]: crate::transformation::voxelization::VoxelSet
    /// [`VoxelizedVolume`]: crate::transformation::voxelization::VoxelizedVolume
    pub fn from_voxels(params: &VHACDParameters, voxels: VoxelSet) -> Self {
        let mut result = Self {
            // raycast_mesh: None,
            voxel_parts: Vec::new(),
            volume_ch0: 0.0,
            max_concavity: -Real::MAX,
        };

        result.do_compute_acd(params, voxels);
        result
    }

    /// Returns the approximately-convex voxelized parts computed by the VHACD algorithm.
    ///
    /// Each part in the returned slice represents an approximately-convex region of the
    /// original shape, stored as a set of voxels. These voxelized parts are the direct
    /// result of the decomposition algorithm.
    ///
    /// # Returns
    ///
    /// A slice of [`VoxelSet`] structures, where each set represents one convex part.
    /// The number of parts depends on the shape's complexity and the parameters used
    /// (especially `concavity` and `max_convex_hulls`).
    ///
    /// # Usage
    ///
    /// You typically don't use the voxel parts directly for collision detection. Instead:
    /// - Use [`compute_convex_hulls`](VHACD::compute_convex_hulls) for voxel-based convex hulls
    /// - Use [`compute_exact_convex_hulls`](VHACD::compute_exact_convex_hulls) for mesh-based hulls
    ///
    /// However, accessing voxel parts directly is useful for:
    /// - Debugging and visualization of the decomposition
    /// - Custom processing of the voxelized representation
    /// - Understanding how the algorithm divided the shape
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::math::Vector;
    /// use parry3d::transformation::vhacd::{VHACD, VHACDParameters};
    ///
    /// # let vertices = vec![
    /// #     Vector::new(0.0, 0.0, 0.0), Vector::new(1.0, 0.0, 0.0),
    /// #     Vector::new(0.5, 1.0, 0.0), Vector::new(0.5, 0.5, 1.0),
    /// # ];
    /// # let indices = vec![[0, 1, 2], [0, 2, 3], [0, 3, 1], [1, 3, 2]];
    /// #
    /// let decomposition = VHACD::decompose(
    ///     &VHACDParameters::default(),
    ///     &vertices,
    ///     &indices,
    ///     false,
    /// );
    ///
    /// let parts = decomposition.voxel_parts();
    /// println!("Generated {} convex parts", parts.len());
    ///
    /// // Inspect individual parts
    /// for (i, part) in parts.iter().enumerate() {
    ///     println!("Part {}: {} voxels", i, part.voxels().len());
    /// }
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`VoxelSet`]: The voxel data structure
    /// - [`compute_convex_hulls`](VHACD::compute_convex_hulls): Convert to collision-ready convex hulls
    ///
    /// [`VoxelSet`]: crate::transformation::voxelization::VoxelSet
    pub fn voxel_parts(&self) -> &[VoxelSet] {
        &self.voxel_parts
    }

    #[cfg(feature = "dim2")]
    fn compute_preferred_cutting_direction(eigenvalues: Vector) -> (Vector, Real) {
        let vx = eigenvalues.y * eigenvalues.y;
        let vy = eigenvalues.x * eigenvalues.x;

        if vx < vy {
            let e = eigenvalues.y * eigenvalues.y;
            let dir = Vector::X;

            if e == 0.0 {
                (dir, 0.0)
            } else {
                (dir, 1.0 - vx / e)
            }
        } else {
            let e = eigenvalues.x * eigenvalues.x;
            let dir = Vector::Y;

            if e == 0.0 {
                (dir, 0.0)
            } else {
                (dir, 1.0 - vy / e)
            }
        }
    }

    #[cfg(feature = "dim3")]
    fn compute_preferred_cutting_direction(eigenvalues: Vector) -> (Vector, Real) {
        let vx = (eigenvalues.y - eigenvalues.z) * (eigenvalues.y - eigenvalues.z);
        let vy = (eigenvalues.x - eigenvalues.z) * (eigenvalues.x - eigenvalues.z);
        let vz = (eigenvalues.x - eigenvalues.y) * (eigenvalues.x - eigenvalues.y);

        if vx < vy && vx < vz {
            let e = eigenvalues.y * eigenvalues.y + eigenvalues.z * eigenvalues.z;
            let dir = Vector::X;

            if e == 0.0 {
                (dir, 0.0)
            } else {
                (dir, 1.0 - vx / e)
            }
        } else if vy < vx && vy < vz {
            let e = eigenvalues.x * eigenvalues.x + eigenvalues.z * eigenvalues.z;
            let dir = Vector::Y;

            if e == 0.0 {
                (dir, 0.0)
            } else {
                (dir, 1.0 - vy / e)
            }
        } else {
            let e = eigenvalues.x * eigenvalues.x + eigenvalues.y * eigenvalues.y;
            let dir = Vector::Z;

            if e == 0.0 {
                (dir, 0.0)
            } else {
                (dir, 1.0 - vz / e)
            }
        }
    }

    fn refine_axes_aligned_clipping_planes(
        vset: &VoxelSet,
        best_plane: &CutPlane,
        downsampling: u32,
        planes: &mut Vec<CutPlane>,
    ) {
        let min_v = vset.min_bb_voxels();
        let max_v = vset.max_bb_voxels();

        let best_id = best_plane.axis as usize;
        let i0 = min_v[best_id].max(best_plane.index.saturating_sub(downsampling) as Int);
        let i1 = max_v[best_id].min((best_plane.index + downsampling) as Int);

        for i in i0..=i1 {
            let plane = CutPlane {
                abc: Vector::ith(best_id, 1.0),
                axis: best_plane.axis,
                d: -(vset.origin[best_id] + (i as Real + 0.5) * vset.scale),
                index: i as u32,
            };
            planes.push(plane);
        }
    }

    // Returns the best plane, and the min concavity.
    fn compute_best_clipping_plane(
        &self,
        input_voxels: &VoxelSet,
        input_voxels_ch: &ConvexHull,
        planes: &[CutPlane],
        preferred_cutting_direction: Vector,
        w: Real,
        alpha: Real,
        beta: Real,
        convex_hull_downsampling: u32,
        params: &VHACDParameters,
    ) -> (CutPlane, Real) {
        let mut best_plane = planes[0];
        let mut min_concavity = Real::MAX;
        let mut i_best = -1;
        let mut min_total = Real::MAX;

        let mut left_ch;
        let mut right_ch;
        let mut left_ch_pts = Vec::new();
        let mut right_ch_pts = Vec::new();
        let mut left_voxels = VoxelSet::new();
        let mut right_voxels = VoxelSet::new();
        let mut on_surface_voxels = VoxelSet::new();

        input_voxels.select_on_surface(&mut on_surface_voxels);

        for (x, plane) in planes.iter().enumerate() {
            // Compute convex hulls.
            if params.convex_hull_approximation {
                right_ch_pts.clear();
                left_ch_pts.clear();

                on_surface_voxels.intersect(
                    plane,
                    &mut right_ch_pts,
                    &mut left_ch_pts,
                    convex_hull_downsampling * 32,
                );

                clip_mesh(
                    #[cfg(feature = "dim2")]
                    input_voxels_ch,
                    #[cfg(feature = "dim3")]
                    &input_voxels_ch.0,
                    plane,
                    &mut right_ch_pts,
                    &mut left_ch_pts,
                );
                right_ch = convex_hull(&right_ch_pts);
                left_ch = convex_hull(&left_ch_pts);
            } else {
                on_surface_voxels.clip(plane, &mut right_voxels, &mut left_voxels);
                right_ch = right_voxels.compute_convex_hull(convex_hull_downsampling);
                left_ch = left_voxels.compute_convex_hull(convex_hull_downsampling);
            }

            let volume_left_ch = compute_volume(&left_ch);
            let volume_right_ch = compute_volume(&right_ch);

            // compute clipped volumes
            let (volume_left, volume_right) = input_voxels.compute_clipped_volumes(plane);
            let concavity_left = compute_concavity(volume_left, volume_left_ch, self.volume_ch0);
            let concavity_right = compute_concavity(volume_right, volume_right_ch, self.volume_ch0);
            let concavity = concavity_left + concavity_right;

            // compute cost
            let balance = alpha * (volume_left - volume_right).abs() / self.volume_ch0;
            let d = w * plane.abc.dot(preferred_cutting_direction);
            let symmetry = beta * d;
            let total = concavity + balance + symmetry;

            if total < min_total || (total == min_total && (x as Int) < i_best) {
                min_concavity = concavity;
                best_plane = *plane;
                min_total = total;
                i_best = x as Int;
            }
        }

        (best_plane, min_concavity)
    }

    fn process_primitive_set(
        &mut self,
        params: &VHACDParameters,
        first_iteration: bool,
        parts: &mut Vec<VoxelSet>,
        temp: &mut Vec<VoxelSet>,
        mut voxels: VoxelSet,
    ) {
        let volume = voxels.compute_volume(); // Compute the volume for this primitive set
        voxels.compute_bb(); // Compute the bounding box for this primitive set.
        let voxels_convex_hull = voxels.compute_convex_hull(params.convex_hull_downsampling); // Generate the convex hull for this primitive set.

        // Compute the volume of the convex hull
        let volume_ch = compute_volume(&voxels_convex_hull);

        // If this is the first iteration, store the volume of the base
        if first_iteration {
            self.volume_ch0 = volume_ch;
        }

        // Compute the concavity of this volume
        let concavity = compute_concavity(volume, volume_ch, self.volume_ch0);

        // Compute the volume error.
        if concavity > params.concavity {
            let eigenvalues = voxels.compute_principal_axes();
            let (preferred_cutting_direction, w) =
                Self::compute_preferred_cutting_direction(eigenvalues);

            let mut planes = Vec::new();
            voxels.compute_axes_aligned_clipping_planes(params.plane_downsampling, &mut planes);

            let (mut best_plane, mut min_concavity) = self.compute_best_clipping_plane(
                &voxels,
                &voxels_convex_hull,
                &planes,
                preferred_cutting_direction,
                w,
                concavity * params.alpha,
                concavity * params.beta,
                params.convex_hull_downsampling,
                params,
            );

            if params.plane_downsampling > 1 || params.convex_hull_downsampling > 1 {
                let mut planes_ref = Vec::new();

                Self::refine_axes_aligned_clipping_planes(
                    &voxels,
                    &best_plane,
                    params.plane_downsampling,
                    &mut planes_ref,
                );

                let best = self.compute_best_clipping_plane(
                    &voxels,
                    &voxels_convex_hull,
                    &planes_ref,
                    preferred_cutting_direction,
                    w,
                    concavity * params.alpha,
                    concavity * params.beta,
                    1, // convex_hull_downsampling = 1
                    params,
                );

                best_plane = best.0;
                min_concavity = best.1;
            }

            if min_concavity > self.max_concavity {
                self.max_concavity = min_concavity;
            }

            let mut best_left = VoxelSet::new();
            let mut best_right = VoxelSet::new();

            voxels.clip(&best_plane, &mut best_right, &mut best_left);

            temp.push(best_left);
            temp.push(best_right);
        } else {
            parts.push(voxels);
        }
    }

    fn do_compute_acd(&mut self, params: &VHACDParameters, mut voxels: VoxelSet) {
        let intersections = voxels.intersections.clone();
        let mut input_parts = Vec::new();
        let mut parts = Vec::new();
        let mut temp = Vec::new();
        input_parts.push(core::mem::take(&mut voxels));

        let mut first_iteration = true;
        self.volume_ch0 = 1.0;

        // Compute the decomposition depth based on the number of convex hulls being requested.
        let mut hull_count = 2;
        let mut depth = 1;

        while params.max_convex_hulls > hull_count {
            depth += 1;
            hull_count *= 2;
        }

        // We must always increment the decomposition depth one higher than the maximum number of hulls requested.
        // The reason for this is as follows.
        // Say, for example, the user requests 32 convex hulls exactly.  This would be a decomposition depth of 5.
        // However, when we do that, we do *not* necessarily get 32 hulls as a result.  This is because, during
        // the recursive descent of the binary tree, one or more of the leaf nodes may have no concavity and
        // will not be split.  So, in this way, even with a decomposition depth of 5, you can produce fewer than
        // 32 hulls.  So, in this case, we would set the decomposition depth to 6 (producing up to as high as 64 convex
        // hulls). Then, the merge step which combines over-described hulls down to the user requested amount, we will end
        // up getting exactly 32 convex hulls as a result. We could just allow the artist to directly control the
        // decomposition depth directly, but this would be a bit too complex and the preference is simply to let them
        // specify how many hulls they want and derive the solution from that.
        depth += 1;

        for _ in 0..depth {
            if input_parts.is_empty() {
                break;
            }

            for input_part in input_parts.drain(..) {
                self.process_primitive_set(
                    params,
                    first_iteration,
                    &mut parts,
                    &mut temp,
                    input_part,
                );
                first_iteration = false;
            }

            core::mem::swap(&mut input_parts, &mut temp);
            // Note that temp is already clear because our previous for
            // loop used `drain`. However we call `clear` here explicitly
            // to make sure it still works if we remove the `drain` in the
            // future.
            temp.clear();
        }

        parts.append(&mut input_parts);
        self.voxel_parts = parts;

        for part in &mut self.voxel_parts {
            part.intersections = intersections.clone();
        }
    }

    // Returns a vector such that `result[i]` gives the index of the voxelized convex part that
    // intersects it.
    //
    // If multiple convex parts intersect the same primitive, then `result[i]` is set to `u32::MAX`.
    // This is used to avoid some useless triangle/segment cutting when computing the exact convex hull
    // of a voxelized convex part.
    fn classify_primitives(&self, num_primitives: usize) -> Vec<u32> {
        if num_primitives == 0 {
            return Vec::new();
        }

        const NO_CLASS: u32 = u32::MAX - 1;
        const MULTICLASS: u32 = u32::MAX;

        let mut primitive_classes = Vec::new();
        primitive_classes.resize(num_primitives, NO_CLASS);

        for (ipart, part) in self.voxel_parts.iter().enumerate() {
            for voxel in &part.voxels {
                let range = voxel.intersections_range.0..voxel.intersections_range.1;
                for inter in &part.intersections[range] {
                    let class = &mut primitive_classes[*inter as usize];
                    if *class == NO_CLASS {
                        *class = ipart as u32;
                    } else if *class != ipart as u32 {
                        *class = MULTICLASS;
                    }
                }
            }
        }

        primitive_classes
    }

    /// Compute the intersections between the voxelized convex part of this decomposition,
    /// and all the primitives from the original decomposed polyline/trimesh,
    ///
    /// This will panic if `keep_voxel_to_primitives_map` was set to `false` when initializing
    /// `self`.
    pub fn compute_primitive_intersections(
        &self,
        points: &[Vector],
        indices: &[[u32; DIM]],
    ) -> Vec<Vec<Vector>> {
        self.voxel_parts
            .iter()
            .map(|part| part.compute_primitive_intersections(points, indices))
            .collect()
    }

    /// Compute exact convex hulls using the original mesh geometry (2D version).
    ///
    /// This method generates convex hulls for each decomposed part by computing the convex
    /// hull of the intersection between the voxelized part and the **original polyline primitives**.
    /// This produces more accurate hulls than the voxel-based method, preserving the original
    /// geometry's detail.
    ///
    /// # Requirements
    ///
    /// This method requires that `keep_voxel_to_primitives_map` was set to `true` when calling
    /// [`VHACD::decompose`]. If it was `false`, this method will **panic**.
    ///
    /// # Parameters
    ///
    /// * `points` - The same vertex buffer used when calling [`VHACD::decompose`]
    /// * `indices` - The same index buffer (segment indices `[u32; 2]`) used when calling
    ///   [`VHACD::decompose`]
    ///
    /// # Returns
    ///
    /// A vector of convex polygons (one per decomposed part), where each polygon is
    /// represented as a `Vec<Vector>` containing the hull vertices in order.
    ///
    /// # Performance
    ///
    /// This is more expensive than [`compute_convex_hulls`](VHACD::compute_convex_hulls) because
    /// it needs to compute intersections with the original primitives and then compute convex hulls.
    /// However, it produces more accurate results.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::math::Vector;
    /// use parry2d::transformation::vhacd::{VHACD, VHACDParameters};
    ///
    /// // Define an L-shaped polyline
    /// let vertices = vec![
    ///     Vector::new(0.0, 0.0), Vector::new(2.0, 0.0),
    ///     Vector::new(2.0, 1.0), Vector::new(1.0, 1.0),
    ///     Vector::new(1.0, 2.0), Vector::new(0.0, 2.0),
    /// ];
    /// let indices = vec![
    ///     [0, 1], [1, 2], [2, 3], [3, 4], [4, 5], [5, 0],
    /// ];
    ///
    /// // IMPORTANT: Set keep_voxel_to_primitives_map to true
    /// let decomposition = VHACD::decompose(
    ///     &VHACDParameters::default(),
    ///     &vertices,
    ///     &indices,
    ///     true, // <-- Required for exact hulls
    /// );
    ///
    /// // Compute exact convex hulls using original geometry
    /// let exact_hulls = decomposition.compute_exact_convex_hulls(&vertices, &indices);
    ///
    /// for (i, hull) in exact_hulls.iter().enumerate() {
    ///     println!("Hull {}: {} vertices", i, hull.len());
    /// }
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if `keep_voxel_to_primitives_map` was `false` during decomposition.
    ///
    /// # See Also
    ///
    /// - [`compute_convex_hulls`](VHACD::compute_convex_hulls): Faster voxel-based hulls
    /// - [`compute_primitive_intersections`](VHACD::compute_primitive_intersections): Get raw intersection points
    #[cfg(feature = "dim2")]
    pub fn compute_exact_convex_hulls(
        &self,
        points: &[Vector],
        indices: &[[u32; DIM]],
    ) -> Vec<Vec<Vector>> {
        self.voxel_parts
            .iter()
            .map(|part| part.compute_exact_convex_hull(points, indices))
            .collect()
    }

    /// Compute exact convex hulls using the original mesh geometry (3D version).
    ///
    /// This method generates convex hulls for each decomposed part by computing the convex
    /// hull of the intersection between the voxelized part and the **original triangle mesh
    /// primitives**. This produces more accurate hulls than the voxel-based method, preserving
    /// the original geometry's detail.
    ///
    /// # Requirements
    ///
    /// This method requires that `keep_voxel_to_primitives_map` was set to `true` when calling
    /// [`VHACD::decompose`]. If it was `false`, this method will **panic**.
    ///
    /// # Parameters
    ///
    /// * `points` - The same vertex buffer used when calling [`VHACD::decompose`]
    /// * `indices` - The same index buffer (triangle indices `[u32; 3]`) used when calling
    ///   [`VHACD::decompose`]
    ///
    /// # Returns
    ///
    /// A vector of convex hulls (one per decomposed part), where each hull is represented as
    /// a tuple `(vertices, indices)`:
    /// - `vertices`: `Vec<Vector>` - The hull vertices
    /// - `indices`: `Vec<[u32; 3]>` - Triangle indices defining the hull surface
    ///
    /// # Performance
    ///
    /// This is more expensive than [`compute_convex_hulls`](VHACD::compute_convex_hulls) because
    /// it needs to compute intersections with the original primitives and then compute convex hulls.
    /// However, it produces more accurate results that better match the original mesh.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::math::Vector;
    /// use parry3d::transformation::vhacd::{VHACD, VHACDParameters};
    ///
    /// # let vertices = vec![
    /// #     Vector::new(0.0, 0.0, 0.0), Vector::new(1.0, 0.0, 0.0),
    /// #     Vector::new(0.5, 1.0, 0.0), Vector::new(0.5, 0.5, 1.0),
    /// # ];
    /// # let indices = vec![[0, 1, 2], [0, 2, 3], [0, 3, 1], [1, 3, 2]];
    /// #
    /// // IMPORTANT: Set keep_voxel_to_primitives_map to true
    /// let decomposition = VHACD::decompose(
    ///     &VHACDParameters::default(),
    ///     &vertices,
    ///     &indices,
    ///     true, // <-- Required for exact hulls
    /// );
    ///
    /// // Compute exact convex hulls using original geometry
    /// let exact_hulls = decomposition.compute_exact_convex_hulls(&vertices, &indices);
    ///
    /// for (i, (verts, tris)) in exact_hulls.iter().enumerate() {
    ///     println!("Hull {}: {} vertices, {} triangles", i, verts.len(), tris.len());
    /// }
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if `keep_voxel_to_primitives_map` was `false` during decomposition.
    ///
    /// # See Also
    ///
    /// - [`compute_convex_hulls`](VHACD::compute_convex_hulls): Faster voxel-based hulls
    /// - [`compute_primitive_intersections`](VHACD::compute_primitive_intersections): Get raw intersection points
    #[cfg(feature = "dim3")]
    pub fn compute_exact_convex_hulls(
        &self,
        points: &[Vector],
        indices: &[[u32; DIM]],
    ) -> Vec<(Vec<Vector>, Vec<[u32; DIM]>)> {
        self.voxel_parts
            .iter()
            .map(|part| part.compute_exact_convex_hull(points, indices))
            .collect()
    }

    /// Compute convex hulls from the voxelized parts (2D version).
    ///
    /// This method generates convex polygons for each decomposed part by computing the convex
    /// hull of the **voxel vertices**. This is faster than [`compute_exact_convex_hulls`](VHACD::compute_exact_convex_hulls)
    /// but the hulls are based on the voxelized representation rather than the original geometry.
    ///
    /// # Parameters
    ///
    /// * `downsampling` - Controls how many voxels to skip when generating the hull.
    ///   Higher values = fewer points = simpler hulls = faster computation.
    ///   - `1`: Use all voxel vertices (highest quality, slowest)
    ///   - `4`: Use every 4th voxel (good balance, recommended)
    ///   - `8+`: Use fewer voxels (fastest, simpler hulls)
    ///
    ///   Values less than 1 are clamped to 1.
    ///
    /// # Returns
    ///
    /// A vector of convex polygons (one per decomposed part), where each polygon is
    /// represented as a `Vec<Vector>` containing the hull vertices in order.
    ///
    /// # Performance
    ///
    /// This method is faster than [`compute_exact_convex_hulls`](VHACD::compute_exact_convex_hulls)
    /// because it works directly with voxel data without needing to intersect with original
    /// mesh primitives. However, the resulting hulls are slightly less accurate.
    ///
    /// # When to Use
    ///
    /// Use this method when:
    /// - You don't need the highest accuracy
    /// - Performance is important
    /// - You didn't enable `keep_voxel_to_primitives_map` during decomposition
    /// - The voxel resolution is high enough for your needs
    ///
    /// Use [`compute_exact_convex_hulls`](VHACD::compute_exact_convex_hulls) when:
    /// - You need the most accurate representation of the original geometry
    /// - You have enabled `keep_voxel_to_primitives_map`
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::math::Vector;
    /// use parry2d::transformation::vhacd::{VHACD, VHACDParameters};
    ///
    /// // Define an L-shaped polyline
    /// let vertices = vec![
    ///     Vector::new(0.0, 0.0), Vector::new(2.0, 0.0),
    ///     Vector::new(2.0, 1.0), Vector::new(1.0, 1.0),
    ///     Vector::new(1.0, 2.0), Vector::new(0.0, 2.0),
    /// ];
    /// let indices = vec![
    ///     [0, 1], [1, 2], [2, 3], [3, 4], [4, 5], [5, 0],
    /// ];
    ///
    /// let decomposition = VHACD::decompose(
    ///     &VHACDParameters::default(),
    ///     &vertices,
    ///     &indices,
    ///     false, // voxel-to-primitive mapping not needed
    /// );
    ///
    /// // Compute voxel-based convex hulls with moderate downsampling
    /// let hulls = decomposition.compute_convex_hulls(4);
    ///
    /// for (i, hull) in hulls.iter().enumerate() {
    ///     println!("Hull {}: {} vertices", i, hull.len());
    /// }
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`compute_exact_convex_hulls`](VHACD::compute_exact_convex_hulls): More accurate mesh-based hulls
    /// - [`voxel_parts`](VHACD::voxel_parts): Access the raw voxel data
    #[cfg(feature = "dim2")]
    pub fn compute_convex_hulls(&self, downsampling: u32) -> Vec<Vec<Vector>> {
        let downsampling = downsampling.max(1);
        self.voxel_parts
            .iter()
            .map(|part| part.compute_convex_hull(downsampling))
            .collect()
    }

    /// Compute convex hulls from the voxelized parts (3D version).
    ///
    /// This method generates convex meshes for each decomposed part by computing the convex
    /// hull of the **voxel vertices**. This is faster than [`compute_exact_convex_hulls`](VHACD::compute_exact_convex_hulls)
    /// but the hulls are based on the voxelized representation rather than the original geometry.
    ///
    /// # Parameters
    ///
    /// * `downsampling` - Controls how many voxels to skip when generating the hull.
    ///   Higher values = fewer points = simpler hulls = faster computation.
    ///   - `1`: Use all voxel vertices (highest quality, slowest)
    ///   - `4`: Use every 4th voxel (good balance, recommended)
    ///   - `8+`: Use fewer voxels (fastest, simpler hulls)
    ///
    ///   Values less than 1 are clamped to 1.
    ///
    /// # Returns
    ///
    /// A vector of convex hulls (one per decomposed part), where each hull is represented as
    /// a tuple `(vertices, indices)`:
    /// - `vertices`: `Vec<Vector>` - The hull vertices
    /// - `indices`: `Vec<[u32; 3]>` - Triangle indices defining the hull surface
    ///
    /// # Performance
    ///
    /// This method is faster than [`compute_exact_convex_hulls`](VHACD::compute_exact_convex_hulls)
    /// because it works directly with voxel data without needing to intersect with original
    /// mesh primitives. The performance scales with the number of voxels and the downsampling factor.
    ///
    /// # When to Use
    ///
    /// Use this method when:
    /// - You don't need the highest accuracy
    /// - Performance is important
    /// - You didn't enable `keep_voxel_to_primitives_map` during decomposition
    /// - The voxel resolution is high enough for your needs
    /// - You're using this for real-time collision detection
    ///
    /// Use [`compute_exact_convex_hulls`](VHACD::compute_exact_convex_hulls) when:
    /// - You need the most accurate representation of the original geometry
    /// - You have enabled `keep_voxel_to_primitives_map`
    /// - Quality is more important than speed
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::math::Vector;
    /// use parry3d::transformation::vhacd::{VHACD, VHACDParameters};
    ///
    /// # let vertices = vec![
    /// #     Vector::new(0.0, 0.0, 0.0), Vector::new(1.0, 0.0, 0.0),
    /// #     Vector::new(0.5, 1.0, 0.0), Vector::new(0.5, 0.5, 1.0),
    /// # ];
    /// # let indices = vec![[0, 1, 2], [0, 2, 3], [0, 3, 1], [1, 3, 2]];
    /// #
    /// let decomposition = VHACD::decompose(
    ///     &VHACDParameters::default(),
    ///     &vertices,
    ///     &indices,
    ///     false, // voxel-to-primitive mapping not needed
    /// );
    ///
    /// // Compute voxel-based convex hulls with moderate downsampling
    /// let hulls = decomposition.compute_convex_hulls(4);
    ///
    /// for (i, (verts, tris)) in hulls.iter().enumerate() {
    ///     println!("Hull {}: {} vertices, {} triangles", i, verts.len(), tris.len());
    /// }
    /// # }
    /// ```
    ///
    /// ## Creating a Compound Shape for Collision Detection
    ///
    /// ```no_run
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::math::Vector;
    /// use parry3d::shape::{SharedShape, Compound};
    /// use parry3d::transformation::vhacd::{VHACD, VHACDParameters};
    /// use parry3d::math::Pose;
    ///
    /// # let vertices = vec![
    /// #     Vector::new(0.0, 0.0, 0.0), Vector::new(1.0, 0.0, 0.0),
    /// #     Vector::new(0.5, 1.0, 0.0), Vector::new(0.5, 0.5, 1.0),
    /// # ];
    /// # let indices = vec![[0, 1, 2], [0, 2, 3], [0, 3, 1], [1, 3, 2]];
    /// #
    /// let decomposition = VHACD::decompose(
    ///     &VHACDParameters::default(),
    ///     &vertices,
    ///     &indices,
    ///     false,
    /// );
    ///
    /// let hulls = decomposition.compute_convex_hulls(4);
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`compute_exact_convex_hulls`](VHACD::compute_exact_convex_hulls): More accurate mesh-based hulls
    /// - [`voxel_parts`](VHACD::voxel_parts): Access the raw voxel data
    /// - [`SharedShape::convex_decomposition`](crate::shape::SharedShape::convex_decomposition): High-level API
    #[cfg(feature = "dim3")]
    pub fn compute_convex_hulls(&self, downsampling: u32) -> Vec<(Vec<Vector>, Vec<[u32; DIM]>)> {
        let downsampling = downsampling.max(1);
        self.voxel_parts
            .iter()
            .map(|part| part.compute_convex_hull(downsampling))
            .collect()
    }
}

fn compute_concavity(volume: Real, volume_ch: Real, volume0: Real) -> Real {
    (volume_ch - volume).abs() / volume0
}

fn clip_mesh(
    points: &[Vector],
    plane: &CutPlane,
    positive_part: &mut Vec<Vector>,
    negative_part: &mut Vec<Vector>,
) {
    for pt in points {
        let d = plane.abc.dot(*pt) + plane.d;

        if d > 0.0 {
            positive_part.push(*pt);
        } else if d < 0.0 {
            negative_part.push(*pt);
        } else {
            positive_part.push(*pt);
            negative_part.push(*pt);
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

#[cfg(feature = "dim2")]
fn compute_volume(polygon: &[Vector]) -> Real {
    if !polygon.is_empty() {
        crate::mass_properties::details::convex_polygon_area_and_center_of_mass(polygon).0
    } else {
        0.0
    }
}

#[cfg(feature = "dim3")]
fn compute_volume(mesh: &(Vec<Vector>, Vec<[u32; DIM]>)) -> Real {
    if !mesh.0.is_empty() {
        crate::mass_properties::details::trimesh_signed_volume_and_center_of_mass(&mesh.0, &mesh.1)
            .0
    } else {
        0.0
    }
}

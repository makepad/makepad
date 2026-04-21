use crate::bounding_volume::Aabb;
use crate::math::{Pose, Vector};
use crate::partitioning::{Bvh, BvhBuildStrategy};
use crate::query::{PointProjection, PointQueryWithLocation};
use crate::shape::composite_shape::CompositeShape;
use crate::shape::{FeatureId, Segment, SegmentPointLocation, Shape, TypedCompositeShape};
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::query::details::NormalConstraints;

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
/// A polyline shape formed by connected line segments.
///
/// A polyline is a sequence of line segments (edges) connecting vertices. It can be open
/// (not forming a closed loop) or closed (where the last vertex connects back to the first).
/// Polylines are commonly used for paths, boundaries, and 2D/3D curves.
///
/// # Structure
///
/// A polyline consists of:
/// - **Vertices**: Vectors in 2D or 3D space
/// - **Indices**: Pairs of vertex indices defining each segment
/// - **BVH**: Bounding Volume Hierarchy for fast spatial queries
///
/// # Properties
///
/// - **Composite shape**: Made up of multiple segments
/// - **1-dimensional**: Has length but no volume
/// - **Flexible topology**: Can be open or closed, branching or linear
/// - **Accelerated queries**: Uses BVH for efficient collision detection
///
/// # Use Cases
///
/// Polylines are ideal for:
/// - **Paths and roads**: Navigation paths, road networks
/// - **Terrain boundaries**: Cliff edges, coastlines, level boundaries
/// - **Outlines**: 2D shape outlines, contours
/// - **Wire frames**: Simplified representations of complex shapes
/// - **Motion paths**: Character movement paths, camera rails
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Polyline;
/// use parry3d::math::Vector;
///
/// // Create a simple L-shaped polyline
/// let vertices = vec![
///     Vector::ZERO,
///     Vector::new(1.0, 0.0, 0.0),
///     Vector::new(1.0, 1.0, 0.0),
/// ];
///
/// // Indices are automatically generated to connect consecutive vertices
/// let polyline = Polyline::new(vertices, None);
///
/// // The polyline has 2 segments: (0,1) and (1,2)
/// assert_eq!(polyline.num_segments(), 2);
/// assert_eq!(polyline.vertices().len(), 3);
/// # }
/// ```
///
/// # Custom Connectivity
///
/// You can provide custom indices to create non-sequential connections:
///
/// ```rust
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::shape::Polyline;
/// use parry2d::math::Vector;
///
/// // Create a triangle polyline (closed loop)
/// let vertices = vec![
///     Vector::ZERO,
///     Vector::new(1.0, 0.0),
///     Vector::new(0.5, 1.0),
/// ];
///
/// // Manually specify edges to create a closed triangle
/// let indices = vec![
///     [0, 1],  // Bottom edge
///     [1, 2],  // Right edge
///     [2, 0],  // Left edge (closes the loop)
/// ];
///
/// let polyline = Polyline::new(vertices, Some(indices));
/// assert_eq!(polyline.num_segments(), 3);
/// # }
/// ```
pub struct Polyline {
    bvh: Bvh,
    vertices: Vec<Vector>,
    indices: Vec<[u32; 2]>,
}

impl Polyline {
    /// Creates a new polyline from a vertex buffer and an optional index buffer.
    ///
    /// This is the main constructor for creating a polyline. If no indices are provided,
    /// the vertices will be automatically connected in sequence (vertex 0 to 1, 1 to 2, etc.).
    ///
    /// # Arguments
    ///
    /// * `vertices` - A vector of points defining the polyline vertices
    /// * `indices` - Optional vector of `[u32; 2]` pairs defining which vertices connect.
    ///   If `None`, vertices are connected sequentially.
    ///
    /// # Returns
    ///
    /// A new `Polyline` with an internal BVH for accelerated queries.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Polyline;
    /// use parry3d::math::Vector;
    ///
    /// // Create a zigzag path with automatic sequential connections
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 1.0, 0.0),
    ///     Vector::new(2.0, 0.0, 0.0),
    ///     Vector::new(3.0, 1.0, 0.0),
    /// ];
    /// let polyline = Polyline::new(vertices, None);
    /// assert_eq!(polyline.num_segments(), 3);
    /// # }
    /// ```
    ///
    /// # Custom Connectivity Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::Polyline;
    /// use parry2d::math::Vector;
    ///
    /// // Create a square with custom indices
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0),
    ///     Vector::new(1.0, 1.0),
    ///     Vector::new(0.0, 1.0),
    /// ];
    ///
    /// // Define edges to form a closed square
    /// let indices = vec![
    ///     [0, 1], [1, 2], [2, 3], [3, 0]
    /// ];
    ///
    /// let square = Polyline::new(vertices, Some(indices));
    /// assert_eq!(square.num_segments(), 4);
    ///
    /// // Each segment connects the correct vertices
    /// let first_segment = square.segment(0);
    /// assert_eq!(first_segment.a, Vector::ZERO);
    /// assert_eq!(first_segment.b, Vector::new(1.0, 0.0));
    /// # }
    /// ```
    pub fn new(vertices: Vec<Vector>, indices: Option<Vec<[u32; 2]>>) -> Self {
        let indices =
            indices.unwrap_or_else(|| (0..vertices.len() as u32 - 1).map(|i| [i, i + 1]).collect());
        let leaves = indices.iter().enumerate().map(|(i, idx)| {
            let aabb =
                Segment::new(vertices[idx[0] as usize], vertices[idx[1] as usize]).local_aabb();
            (i, aabb)
        });

        // NOTE: we apply no dilation factor because we won't
        // update this tree dynamically.
        let bvh = Bvh::from_iter(BvhBuildStrategy::Binned, leaves);

        Self {
            bvh,
            vertices,
            indices,
        }
    }

    /// Computes the axis-aligned bounding box of this polyline in world space.
    ///
    /// The AABB is the smallest box aligned with the world axes that fully contains
    /// the polyline after applying the given position/rotation transformation.
    ///
    /// # Arguments
    ///
    /// * `pos` - The position and orientation (isometry) of the polyline in world space
    ///
    /// # Returns
    ///
    /// An `Aabb` that bounds the transformed polyline
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Polyline;
    /// use parry3d::math::{Vector, Pose};
    ///
    /// // Create a polyline along the X axis
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(2.0, 0.0, 0.0),
    /// ];
    /// let polyline = Polyline::new(vertices, None);
    ///
    /// // Compute AABB at the origin
    /// let identity = Pose::identity();
    /// let aabb = polyline.aabb(&identity);
    /// assert_eq!(aabb.mins.x, 0.0);
    /// assert_eq!(aabb.maxs.x, 2.0);
    ///
    /// // Compute AABB after translating by (10, 5, 0)
    /// let translated = Pose::translation(10.0, 5.0, 0.0);
    /// let aabb_translated = polyline.aabb(&translated);
    /// assert_eq!(aabb_translated.mins.x, 10.0);
    /// assert_eq!(aabb_translated.maxs.x, 12.0);
    /// # }
    /// ```
    pub fn aabb(&self, pos: &Pose) -> Aabb {
        self.bvh.root_aabb().transform_by(pos)
    }

    /// Gets the local axis-aligned bounding box of this polyline.
    ///
    /// This returns the AABB in the polyline's local coordinate system (before any
    /// transformation is applied). It's more efficient than `aabb()` when you don't
    /// need to transform the polyline.
    ///
    /// # Returns
    ///
    /// An `Aabb` that bounds the polyline in local space
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::Polyline;
    /// use parry2d::math::Vector;
    ///
    /// // Create a rectangular polyline
    /// let vertices = vec![
    ///     Vector::new(-1.0, -2.0),
    ///     Vector::new(3.0, -2.0),
    ///     Vector::new(3.0, 4.0),
    ///     Vector::new(-1.0, 4.0),
    /// ];
    /// let polyline = Polyline::new(vertices, None);
    ///
    /// // Get the local AABB
    /// let aabb = polyline.local_aabb();
    ///
    /// // The AABB should contain all vertices
    /// assert_eq!(aabb.mins.x, -1.0);
    /// assert_eq!(aabb.mins.y, -2.0);
    /// assert_eq!(aabb.maxs.x, 3.0);
    /// assert_eq!(aabb.maxs.y, 4.0);
    /// # }
    /// ```
    pub fn local_aabb(&self) -> Aabb {
        self.bvh.root_aabb()
    }

    /// The BVH acceleration structure for this polyline.
    pub fn bvh(&self) -> &Bvh {
        &self.bvh
    }

    /// Returns the number of segments (edges) in this polyline.
    ///
    /// Each segment connects two vertices. For a polyline with `n` vertices and
    /// sequential connectivity, there are `n-1` segments. For custom connectivity,
    /// the number of segments equals the number of index pairs.
    ///
    /// # Returns
    ///
    /// The total number of line segments
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Polyline;
    /// use parry3d::math::Vector;
    ///
    /// // Sequential polyline: 5 vertices -> 4 segments
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(2.0, 0.0, 0.0),
    ///     Vector::new(3.0, 0.0, 0.0),
    ///     Vector::new(4.0, 0.0, 0.0),
    /// ];
    /// let polyline = Polyline::new(vertices.clone(), None);
    /// assert_eq!(polyline.num_segments(), 4);
    ///
    /// // Custom connectivity: can have different number of segments
    /// let indices = vec![[0, 4], [1, 3]]; // Only 2 segments
    /// let custom = Polyline::new(vertices, Some(indices));
    /// assert_eq!(custom.num_segments(), 2);
    /// # }
    /// ```
    pub fn num_segments(&self) -> usize {
        self.indices.len()
    }

    /// Returns an iterator over all segments in this polyline.
    ///
    /// Each segment is returned as a [`Segment`] object with two endpoints.
    /// The iterator yields exactly `num_segments()` items.
    ///
    /// # Returns
    ///
    /// An exact-size iterator that yields `Segment` instances
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::Polyline;
    /// use parry2d::math::Vector;
    ///
    /// // Create a triangle
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0),
    ///     Vector::new(0.5, 1.0),
    /// ];
    /// let polyline = Polyline::new(vertices, None);
    ///
    /// // Iterate over all segments
    /// let mut total_length = 0.0;
    /// for segment in polyline.segments() {
    ///     total_length += segment.length();
    /// }
    ///
    /// // Calculate expected perimeter (not closed, so 2 sides only)
    /// assert!(total_length > 2.0);
    ///
    /// // Collect all segments into a vector
    /// let segments: Vec<_> = polyline.segments().collect();
    /// assert_eq!(segments.len(), 2);
    /// # }
    /// ```
    pub fn segments(&self) -> impl ExactSizeIterator<Item = Segment> + '_ {
        self.indices.iter().map(move |ids| {
            Segment::new(
                self.vertices[ids[0] as usize],
                self.vertices[ids[1] as usize],
            )
        })
    }

    /// Returns the segment at the given index.
    ///
    /// This retrieves a specific segment by its index. Indices range from `0` to
    /// `num_segments() - 1`. If you need to access multiple segments, consider
    /// using the `segments()` iterator instead.
    ///
    /// # Arguments
    ///
    /// * `i` - The index of the segment to retrieve (0-based)
    ///
    /// # Returns
    ///
    /// A `Segment` representing the edge at index `i`
    ///
    /// # Panics
    ///
    /// Panics if `i >= num_segments()`
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Polyline;
    /// use parry3d::math::Vector;
    ///
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(2.0, 1.0, 0.0),
    /// ];
    /// let polyline = Polyline::new(vertices, None);
    ///
    /// // Get the first segment (connects vertex 0 to vertex 1)
    /// let seg0 = polyline.segment(0);
    /// assert_eq!(seg0.a, Vector::ZERO);
    /// assert_eq!(seg0.b, Vector::new(1.0, 0.0, 0.0));
    /// assert_eq!(seg0.length(), 1.0);
    ///
    /// // Get the second segment (connects vertex 1 to vertex 2)
    /// let seg1 = polyline.segment(1);
    /// assert_eq!(seg1.a, Vector::new(1.0, 0.0, 0.0));
    /// assert_eq!(seg1.b, Vector::new(2.0, 1.0, 0.0));
    /// # }
    /// ```
    pub fn segment(&self, i: u32) -> Segment {
        let idx = self.indices[i as usize];
        Segment::new(
            self.vertices[idx[0] as usize],
            self.vertices[idx[1] as usize],
        )
    }

    /// Transforms  the feature-id of a segment to the feature-id of this polyline.
    pub fn segment_feature_to_polyline_feature(
        &self,
        segment: u32,
        _feature: FeatureId,
    ) -> FeatureId {
        // TODO: return a vertex feature when it makes sense.
        #[cfg(feature = "dim2")]
        return FeatureId::Face(segment);
        #[cfg(feature = "dim3")]
        return FeatureId::Edge(segment);
    }

    /// Returns a slice containing all vertices of this polyline.
    ///
    /// Vertices are the points that define the polyline. Segments connect
    /// pairs of these vertices according to the index buffer.
    ///
    /// # Returns
    ///
    /// A slice of all vertex points
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::Polyline;
    /// use parry2d::math::Vector;
    ///
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0),
    ///     Vector::new(1.0, 1.0),
    /// ];
    /// let polyline = Polyline::new(vertices.clone(), None);
    ///
    /// // Access all vertices
    /// let verts = polyline.vertices();
    /// assert_eq!(verts.len(), 3);
    /// assert_eq!(verts[0], Vector::ZERO);
    /// assert_eq!(verts[1], Vector::new(1.0, 0.0));
    /// assert_eq!(verts[2], Vector::new(1.0, 1.0));
    ///
    /// // You can iterate over vertices
    /// for (i, vertex) in polyline.vertices().iter().enumerate() {
    ///     println!("Vertex {}: {:?}", i, vertex);
    /// }
    /// # }
    /// ```
    pub fn vertices(&self) -> &[Vector] {
        &self.vertices[..]
    }

    /// Returns a slice containing all segment indices.
    ///
    /// Each index is a pair `[u32; 2]` representing a segment connecting two vertices.
    /// The first element is the index of the segment's start vertex, and the second
    /// is the index of the end vertex.
    ///
    /// # Returns
    ///
    /// A slice of all segment index pairs
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Polyline;
    /// use parry3d::math::Vector;
    ///
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(2.0, 0.0, 0.0),
    /// ];
    ///
    /// // With automatic indices
    /// let polyline = Polyline::new(vertices.clone(), None);
    /// let indices = polyline.indices();
    /// assert_eq!(indices.len(), 2);
    /// assert_eq!(indices[0], [0, 1]); // First segment: vertex 0 -> 1
    /// assert_eq!(indices[1], [1, 2]); // Second segment: vertex 1 -> 2
    ///
    /// // With custom indices
    /// let custom_indices = vec![[0, 2], [1, 0]];
    /// let custom = Polyline::new(vertices, Some(custom_indices));
    /// assert_eq!(custom.indices()[0], [0, 2]);
    /// assert_eq!(custom.indices()[1], [1, 0]);
    /// # }
    /// ```
    pub fn indices(&self) -> &[[u32; 2]] {
        &self.indices
    }

    /// A flat view of the index buffer of this mesh.
    pub fn flat_indices(&self) -> &[u32] {
        unsafe {
            let len = self.indices.len() * 2;
            let data = self.indices.as_ptr() as *const u32;
            core::slice::from_raw_parts(data, len)
        }
    }

    /// Computes a scaled version of this polyline.
    ///
    /// This consumes the polyline and returns a new one with all vertices scaled
    /// component-wise by the given scale vector. The connectivity (indices) remains
    /// unchanged, but the BVH is rebuilt to reflect the new geometry.
    ///
    /// # Arguments
    ///
    /// * `scale` - The scaling factors for each axis
    ///
    /// # Returns
    ///
    /// A new polyline with scaled vertices
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::Polyline;
    /// use parry2d::math::Vector;
    ///
    /// let vertices = vec![
    ///     Vector::new(1.0, 2.0),
    ///     Vector::new(3.0, 4.0),
    /// ];
    /// let polyline = Polyline::new(vertices, None);
    ///
    /// // Scale by 2x in X and 3x in Y
    /// let scaled = polyline.scaled(Vector::new(2.0, 3.0));
    ///
    /// // Check scaled vertices
    /// assert_eq!(scaled.vertices()[0], Vector::new(2.0, 6.0));
    /// assert_eq!(scaled.vertices()[1], Vector::new(6.0, 12.0));
    /// # }
    /// ```
    ///
    /// # Note
    ///
    /// This method consumes `self`. If you need to keep the original polyline,
    /// clone it first:
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Polyline;
    /// use parry3d::math::Vector;
    ///
    /// let vertices = vec![Vector::ZERO, Vector::new(1.0, 0.0, 0.0)];
    /// let original = Polyline::new(vertices, None);
    ///
    /// // Clone before scaling if you need to keep the original
    /// let scaled = original.clone().scaled(Vector::new(2.0, 2.0, 2.0));
    ///
    /// // Both polylines still exist
    /// assert_eq!(original.vertices()[1].x, 1.0);
    /// assert_eq!(scaled.vertices()[1].x, 2.0);
    /// # }
    /// ```
    pub fn scaled(mut self, scale: Vector) -> Self {
        self.vertices.iter_mut().for_each(|pt| *pt *= scale);
        let mut bvh = self.bvh.clone();
        bvh.scale(scale);
        Self {
            bvh,
            vertices: self.vertices,
            indices: self.indices,
        }
    }

    /// Reverses the orientation of this polyline.
    ///
    /// This operation:
    /// 1. Swaps the start and end vertex of each segment (reversing edge direction)
    /// 2. Reverses the order of segments in the index buffer
    /// 3. Rebuilds the BVH to maintain correct acceleration structure
    ///
    /// After reversing, traversing the polyline segments in order will visit
    /// the same geometry but in the opposite direction.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::Polyline;
    /// use parry2d::math::Vector;
    ///
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0),
    ///     Vector::new(2.0, 0.0),
    /// ];
    /// let mut polyline = Polyline::new(vertices, None);
    ///
    /// // Original: segment 0 goes from vertex 0 to 1, segment 1 from 1 to 2
    /// assert_eq!(polyline.indices()[0], [0, 1]);
    /// assert_eq!(polyline.indices()[1], [1, 2]);
    ///
    /// // Reverse the polyline
    /// polyline.reverse();
    ///
    /// // After reversing: order is flipped and directions are swapped
    /// // The last segment becomes first, with swapped endpoints
    /// assert_eq!(polyline.indices()[0], [2, 1]);
    /// assert_eq!(polyline.indices()[1], [1, 0]);
    /// # }
    /// ```
    ///
    /// # Use Cases
    ///
    /// This is useful for:
    /// - Correcting winding order for 2D shapes
    /// - Reversing path direction for navigation
    /// - Ensuring consistent edge orientation in connected components
    pub fn reverse(&mut self) {
        for idx in &mut self.indices {
            idx.swap(0, 1);
        }

        self.indices.reverse();

        // Rebuild the bvh since the segment indices no longer map to the correct element.
        // TODO PERF: should the Bvh have a function for efficient leaf index remapping?
        //            Probably not worth it unless this function starts showing up as a
        //            bottleneck for someone.
        let leaves = self.segments().map(|seg| seg.local_aabb()).enumerate();
        let bvh = Bvh::from_iter(BvhBuildStrategy::Binned, leaves);
        self.bvh = bvh;
    }

    /// Extracts the connected components of this polyline, consuming `self`.
    ///
    /// This method is currently quite restrictive on the kind of allowed input. The polyline
    /// represented by `self` must already have an index buffer sorted such that:
    /// - Each connected component appears in the index buffer one after the other, i.e., a
    ///   connected component of this polyline must be a contiguous range of this polyline’s
    ///   index buffer.
    /// - Each connected component is closed, i.e., each range of this polyline index buffer
    ///   `self.indices[i_start..=i_end]` forming a complete connected component, we must have
    ///   `self.indices[i_start][0] == self.indices[i_end][1]`.
    /// - The indices for each component must already be in order, i.e., if the segments
    ///   `self.indices[i]` and `self.indices[i + 1]` are part of the same connected component then
    ///   we must have `self.indices[i][1] == self.indices[i + 1][0]`.
    ///
    /// # Output
    /// Returns the set of polylines. If the inputs fulfill the constraints mentioned above, each
    /// polyline will be a closed loop with consistent edge orientations, i.e., for all indices `i`,
    /// we have `polyline.indices[i][1] == polyline.indices[i + 1][0]`.
    ///
    /// The orientation of each closed loop (clockwise or counterclockwise) are identical to their
    /// original orientation in `self`.
    pub fn extract_connected_components(&self) -> Vec<Polyline> {
        let vertices = self.vertices();
        let indices = self.indices();

        if indices.is_empty() {
            // Polyline is empty, return empty Vec
            Vec::new()
        } else {
            let mut components = Vec::new();

            let mut start_i = 0; // Start position of component
            let mut start_node = indices[0][0]; // Start vertex index of component

            let mut component_vertices = Vec::new();
            let mut component_indices: Vec<[u32; 2]> = Vec::new();

            // Iterate over indices, building polylines as we go
            for (i, idx) in indices.iter().enumerate() {
                component_vertices.push(vertices[idx[0] as usize]);

                if idx[1] != start_node {
                    // Keep scanning and adding data
                    component_indices.push([(i - start_i) as u32, (i - start_i + 1) as u32]);
                } else {
                    // Start node reached: build polyline and start next component
                    component_indices.push([(i - start_i) as u32, 0]);
                    components.push(Polyline::new(
                        core::mem::take(&mut component_vertices),
                        Some(core::mem::take(&mut component_indices)),
                    ));

                    if i + 1 < indices.len() {
                        // More components to find
                        start_node = indices[i + 1][0];
                        start_i = i + 1;
                    }
                }
            }

            components
        }
    }

    /// Perform a point projection assuming a solid interior based on a counter-clock-wise orientation.
    ///
    /// This is similar to `self.project_local_point_and_get_location` except that the resulting
    /// `PointProjection::is_inside` will be set to true if the point is inside of the area delimited
    /// by this polyline, assuming that:
    /// - This polyline isn’t self-crossing.
    /// - This polyline is closed with `self.indices[i][1] == self.indices[(i + 1) % num_indices][0]` where
    ///   `num_indices == self.indices.len()`.
    /// - This polyline is oriented counter-clockwise.
    /// - In 3D, the polyline is assumed to be fully coplanar, on a plane with normal given by
    ///   `axis`.
    ///
    /// These properties are not checked.
    pub fn project_local_point_assuming_solid_interior_ccw(
        &self,
        point: Vector,
        #[cfg(feature = "dim3")] axis: u8,
    ) -> (PointProjection, (u32, SegmentPointLocation)) {
        let mut proj = self.project_local_point_and_get_location(point, false);
        let segment1 = self.segment((proj.1).0);

        #[cfg(feature = "dim2")]
        let normal1 = segment1.normal();
        #[cfg(feature = "dim3")]
        let normal1 = segment1.planar_normal(axis);

        if let Some(normal1) = normal1 {
            proj.0.is_inside = match proj.1 .1 {
                SegmentPointLocation::OnVertex(i) => {
                    let dir2 = if i == 0 {
                        let adj_seg = if proj.1 .0 == 0 {
                            self.indices().len() as u32 - 1
                        } else {
                            proj.1 .0 - 1
                        };

                        assert_eq!(segment1.a, self.segment(adj_seg).b);
                        -self.segment(adj_seg).scaled_direction()
                    } else {
                        assert_eq!(i, 1);
                        let adj_seg = (proj.1 .0 + 1) % self.indices().len() as u32;
                        assert_eq!(segment1.b, self.segment(adj_seg).a);

                        self.segment(adj_seg).scaled_direction()
                    };

                    let dot = normal1.dot(dir2);
                    // TODO: is this threshold too big? This corresponds to an angle equal to
                    //       abs(acos(1.0e-3)) = (90 - 0.057) degrees.
                    //       We did encounter some cases where this was needed, but perhaps the
                    //       actual problem was an issue with the SegmentPointLocation (which should
                    //       perhaps have been Edge instead of Vertex)?
                    let threshold = 1.0e-3 * dir2.length();
                    if dot.abs() > threshold {
                        // If the vertex is a reentrant vertex, then the point is
                        // inside. Otherwise, it is outside.
                        dot >= 0.0
                    } else {
                        // If the two edges are collinear, we can’t classify the vertex.
                        // So check against the edge’s normal instead.
                        (point - proj.0.point).dot(normal1) <= 0.0
                    }
                }
                SegmentPointLocation::OnEdge(_) => (point - proj.0.point).dot(normal1) <= 0.0,
            };
        }

        proj
    }
}

impl CompositeShape for Polyline {
    fn map_part_at(
        &self,
        i: u32,
        f: &mut dyn FnMut(Option<&Pose>, &dyn Shape, Option<&dyn NormalConstraints>),
    ) {
        let tri = self.segment(i);
        f(None, &tri, None)
    }

    fn bvh(&self) -> &Bvh {
        &self.bvh
    }
}

impl TypedCompositeShape for Polyline {
    type PartShape = Segment;
    type PartNormalConstraints = ();

    #[inline(always)]
    fn map_typed_part_at<T>(
        &self,
        i: u32,
        mut f: impl FnMut(Option<&Pose>, &Self::PartShape, Option<&Self::PartNormalConstraints>) -> T,
    ) -> Option<T> {
        let seg = self.segment(i);
        Some(f(None, &seg, None))
    }

    #[inline(always)]
    fn map_untyped_part_at<T>(
        &self,
        i: u32,
        mut f: impl FnMut(Option<&Pose>, &dyn Shape, Option<&dyn NormalConstraints>) -> T,
    ) -> Option<T> {
        let seg = self.segment(i);
        Some(f(None, &seg, None))
    }
}

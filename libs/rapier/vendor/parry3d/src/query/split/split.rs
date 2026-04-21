/// The result of a plane-splitting operation.
///
/// This enum represents the three possible outcomes when splitting a geometric shape with a plane.
/// It efficiently handles all cases without unnecessary allocations when the shape doesn't need
/// to be split.
///
/// # Type Parameter
///
/// The generic type `T` represents the shape being split. Common types include:
/// - [`Aabb`](crate::bounding_volume::Aabb) - Axis-aligned bounding box
/// - [`Segment`](crate::shape::Segment) - Line segment
/// - [`TriMesh`](crate::shape::TriMesh) - Triangle mesh
///
/// # Half-Space Definition
///
/// Given a plane defined by a normal vector `n` and bias `b`, a point `p` lies in:
/// - **Negative half-space** if `n · p < b` (behind the plane)
/// - **Positive half-space** if `n · p > b` (in front of the plane)
/// - **On the plane** if `n · p ≈ b` (within epsilon tolerance)
///
/// # Examples
///
/// ## Splitting an AABB
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::bounding_volume::Aabb;
/// use parry3d::math::Vector;
/// use parry3d::query::SplitResult;
///
/// let aabb = Aabb::new(Vector::new(0.0, 0.0, 0.0), Vector::new(10.0, 10.0, 10.0));
///
/// // Split along X-axis at x = 5.0
/// match aabb.canonical_split(0, 5.0, 1e-6) {
///     SplitResult::Pair(left, right) => {
///         println!("AABB split into two pieces");
///         println!("Left AABB: {:?}", left);
///         println!("Right AABB: {:?}", right);
///     }
///     SplitResult::Negative => {
///         println!("AABB is entirely on the negative side (x < 5.0)");
///     }
///     SplitResult::Positive => {
///         println!("AABB is entirely on the positive side (x > 5.0)");
///     }
/// }
/// # }
/// ```
///
/// ## Splitting a Segment
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Segment;
/// use parry3d::math::Vector;
/// use parry3d::query::SplitResult;
///
/// let segment = Segment::new(Vector::new(0.0, 0.0, 0.0), Vector::new(10.0, 0.0, 0.0));
///
/// // Split along X-axis at x = 3.0
/// match segment.canonical_split(0, 3.0, 1e-6) {
///     SplitResult::Pair(seg1, seg2) => {
///         println!("Segment split at x = 3.0");
///         println!("First segment: {:?} to {:?}", seg1.a, seg1.b);
///         println!("Second segment: {:?} to {:?}", seg2.a, seg2.b);
///     }
///     SplitResult::Negative => {
///         println!("Entire segment is on the negative side");
///     }
///     SplitResult::Positive => {
///         println!("Entire segment is on the positive side");
///     }
/// }
/// # }
/// ```
pub enum SplitResult<T> {
    /// The split operation yielded two results: one lying on the negative half-space of the plane
    /// and the second lying on the positive half-space of the plane.
    ///
    /// The first element is the shape piece on the **negative side** of the plane (where `n · p < b`).
    /// The second element is the shape piece on the **positive side** of the plane (where `n · p > b`).
    ///
    /// For closed meshes, both pieces are typically "capped" with new geometry on the cutting plane
    /// to ensure the result consists of valid closed shapes.
    Pair(T, T),

    /// The shape being split is fully contained in the negative half-space of the plane.
    ///
    /// This means all points of the shape satisfy `n · p < b` (or are within epsilon of the plane).
    /// No splitting occurred because the shape doesn't cross the plane.
    Negative,

    /// The shape being split is fully contained in the positive half-space of the plane.
    ///
    /// This means all points of the shape satisfy `n · p > b` (or are within epsilon of the plane).
    /// No splitting occurred because the shape doesn't cross the plane.
    Positive,
}

/// The result of a plane-intersection operation.
///
/// This enum represents the outcome when computing the intersection between a geometric shape
/// and a plane. Unlike [`SplitResult`] which produces pieces of the original shape, this produces
/// the geometry that lies exactly on the plane (within epsilon tolerance).
///
/// # Type Parameter
///
/// The generic type `T` represents the result of the intersection. Common types include:
/// - [`Polyline`](crate::shape::Polyline) - For mesh-plane intersections, representing the
///   cross-section outline
///
/// # Use Cases
///
/// Plane-intersection operations are useful for:
/// - **Cross-sectional analysis**: Computing 2D slices of 3D geometry
/// - **Contour generation**: Finding outlines at specific heights
/// - **Visualization**: Displaying cutting planes through complex geometry
/// - **CAD/CAM**: Generating toolpaths or analyzing part geometry
///
/// # Examples
///
/// ## Computing a Mesh Cross-Section
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32", feature = "spade"))] {
/// use parry3d::shape::TriMesh;
/// use parry3d::math::Vector;
/// use parry3d::query::IntersectResult;
///
/// // Create a simple tetrahedron mesh
/// let vertices = vec![
///     Vector::new(0.0, 0.0, 0.0),
///     Vector::new(1.0, 0.0, 0.0),
///     Vector::new(0.5, 1.0, 0.0),
///     Vector::new(0.5, 0.5, 1.0),
/// ];
/// let indices = vec![
///     [0u32, 1, 2],  // Bottom face
///     [0, 1, 3],     // Front face
///     [1, 2, 3],     // Right face
///     [2, 0, 3],     // Left face
/// ];
/// let mesh = TriMesh::new(vertices, indices).unwrap();
///
/// // Compute cross-section at z = 0.5
/// match mesh.canonical_intersection_with_plane(2, 0.5, 1e-6) {
///     IntersectResult::Intersect(polyline) => {
///         println!("Cross-section computed!");
///         println!("Number of vertices: {}", polyline.vertices().len());
///         // The polyline represents the outline of the mesh at z = 0.5
///         // It may consist of multiple disconnected loops if the mesh
///         // has multiple separate pieces at this height
///     }
///     IntersectResult::Negative => {
///         println!("Mesh is entirely below z = 0.5");
///     }
///     IntersectResult::Positive => {
///         println!("Mesh is entirely above z = 0.5");
///     }
/// }
/// # }
/// ```
///
/// ## Handling Multiple Connected Components
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32", feature = "spade"))] {
/// use parry3d::shape::TriMesh;
/// use parry3d::query::IntersectResult;
///
/// # let vertices = vec![
/// #     parry3d::math::Vector::ZERO,
/// #     parry3d::math::Vector::new(1.0, 0.0, 0.0),
/// #     parry3d::math::Vector::new(0.5, 1.0, 0.5),
/// # ];
/// # let indices = vec![[0u32, 1, 2]];
/// # let mesh = TriMesh::new(vertices, indices).unwrap();
/// // When intersecting a mesh with holes or multiple separate parts,
/// // the resulting polyline may have multiple connected components
/// match mesh.canonical_intersection_with_plane(2, 0.5, 1e-6) {
///     IntersectResult::Intersect(polyline) => {
///         // The polyline contains all intersection curves
///         // You can identify separate components by analyzing connectivity
///         // through the polyline's segment indices
///         let indices = polyline.indices();
///         println!("Number of edges in cross-section: {}", indices.len());
///     }
///     _ => println!("No intersection"),
/// }
/// # }
/// ```
pub enum IntersectResult<T> {
    /// The intersect operation yielded a result, lying on the plane (within epsilon tolerance).
    ///
    /// For triangle meshes, this is typically a [`Polyline`](crate::shape::Polyline) representing
    /// the outline where the mesh intersects the plane. The polyline may consist of multiple
    /// disconnected loops if the mesh has holes or multiple separate pieces at the intersection
    /// height.
    ///
    /// The intersection geometry lies on the plane, meaning all points `p` satisfy `n · p ≈ b`
    /// (within the specified epsilon tolerance).
    Intersect(T),

    /// The shape being intersected is fully contained in the negative half-space of the plane.
    ///
    /// This means all points of the shape satisfy `n · p < b` (the shape is entirely "behind"
    /// or "below" the plane). The plane doesn't intersect the shape at all.
    Negative,

    /// The shape being intersected is fully contained in the positive half-space of the plane.
    ///
    /// This means all points of the shape satisfy `n · p > b` (the shape is entirely "in front of"
    /// or "above" the plane). The plane doesn't intersect the shape at all.
    Positive,
}

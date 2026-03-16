use core::fmt;

/// Error indicating that a geometric query is not supported between certain shape combinations.
///
/// Many geometric queries in Parry (like distance calculation, contact detection, or time-of-impact)
/// are implemented using specialized algorithms for specific pairs of shapes. When you attempt a
/// query between two shapes for which no implementation exists, this error is returned.
///
/// # When This Error Occurs
///
/// This error typically occurs in two situations:
///
/// 1. **Missing Implementation**: The query has not been implemented for the specific pair of shapes.
///    For example, some advanced queries might not support all combinations of composite shapes.
///
/// 2. **Complex Shape Combinations**: Certain queries involving composite shapes (like [`Compound`],
///    [`TriMesh`], or [`HeightField`]) may not be fully supported, especially for less common operations.
///
/// # Common Scenarios
///
/// - Computing contact manifolds between two custom shapes that don't have a specialized implementation
/// - Using non-linear shape casting with certain composite shapes
/// - Querying distance between shapes that require more complex algorithms not yet implemented
///
/// # How to Handle This Error
///
/// When you encounter this error, you have several options:
///
/// ## 1. Use a Different Query Type
///
/// Try a more basic query that's more widely supported:
///
/// ```no_run
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// # use parry3d::query::{contact, distance};
/// # use parry3d::shape::{Ball, Cuboid};
/// # use parry3d::math::{Pose, Vector};
/// # let shape1 = Ball::new(1.0);
/// # let shape2 = Cuboid::new(Vector::splat(1.0));
/// # let pos1 = Pose::identity();
/// # let pos2 = Pose::identity();
/// // If contact manifolds are unsupported, try basic contact:
/// if let Ok(contact) = contact(&pos1, &shape1, &pos2, &shape2, 0.0) {
///     // Process the contact point
/// }
///
/// // Or try distance computation:
/// let dist = distance(&pos1, &shape1, &pos2, &shape2);
/// # }
/// ```
///
/// ## 2. Decompose Complex Shapes
///
/// Break down complex shapes into simpler components:
///
/// ```no_run
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// # use parry3d::shape::{TriMesh, Ball, Compound, SharedShape};
/// # use parry3d::query::distance;
/// # use parry3d::math::{Pose, Vector};
/// # let mesh = TriMesh::new(vec![], vec![]).unwrap();
/// # let ball = Ball::new(1.0);
/// # let pos1 = Pose::identity();
/// # let pos2 = Pose::identity();
/// // Instead of querying against a complex mesh directly,
/// // iterate through its triangles:
/// for triangle in mesh.triangles() {
///     let dist = distance(&pos1, &triangle, &pos2, &ball);
///     // Process each triangle-ball pair
/// }
/// # }
/// ```
///
/// ## 3. Use the BVH for Composite Shapes
///
/// For shapes with BVH acceleration structures (like [`TriMesh`]), use specialized traversal methods:
///
/// ```no_run
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// # use parry3d::shape::TriMesh;
/// # use parry3d::bounding_volume::Aabb;
/// # use parry3d::math::Vector;
/// # let mesh = TriMesh::new(vec![Vector::ZERO], vec![[0, 0, 0]]).unwrap();
/// # let query_aabb = Aabb::new(Vector::ZERO, Vector::ZERO);
/// // Use BVH queries instead of direct shape queries:
/// for leaf_id in mesh.bvh().intersect_aabb(&query_aabb) {
///     let triangle = mesh.triangle(leaf_id);
///     // Process the triangle
/// }
/// # }
/// ```
///
/// ## 4. Implement a Custom Query Dispatcher
///
/// For advanced use cases, implement the [`QueryDispatcher`] trait to add support for your specific
/// shape combinations:
///
/// ```no_run
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// # use parry3d::query::{QueryDispatcher, DefaultQueryDispatcher};
/// # use parry3d::shape::Shape;
/// # use parry3d::math::{Pose, Real};
/// struct MyQueryDispatcher {
///     default: DefaultQueryDispatcher,
/// }
///
/// // Implement QueryDispatcher and add your custom query implementations
/// # }
/// ```
///
/// # Example: Catching and Handling the Error
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// # use parry3d::query::{contact, Unsupported};
/// # use parry3d::shape::{Ball, Cuboid};
/// # use parry3d::math::{Pose, Vector};
/// let ball = Ball::new(1.0);
/// let cuboid = Cuboid::new(Vector::splat(1.0));
/// let pos1 = Pose::identity();
/// let pos2 = Pose::identity();
///
/// // Most queries return Result<T, Unsupported>
/// match contact(&pos1, &ball, &pos2, &cuboid, 0.0) {
///     Ok(Some(contact)) => {
///         // Query succeeded and found a contact
///         println!("Contact found at distance: {}", contact.dist);
///     }
///     Ok(None) => {
///         // Query succeeded but no contact found
///         println!("No contact");
///     }
///     Err(Unsupported) => {
///         // Query not supported for this shape combination
///         println!("This query is not supported between these shapes");
///         // Fall back to an alternative approach
///     }
/// }
/// # }
/// ```
///
/// [`Compound`]: crate::shape::Compound
/// [`TriMesh`]: crate::shape::TriMesh
/// [`HeightField`]: crate::shape::HeightField
/// [`QueryDispatcher`]: crate::query::QueryDispatcher
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct Unsupported;

impl fmt::Display for Unsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("query not supported between these shapes")
    }
}

#[cfg(feature = "alloc")]
impl core::error::Error for Unsupported {}

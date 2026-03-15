use crate::math::{Pose, Real};
use crate::query::{ClosestPoints, DefaultQueryDispatcher, QueryDispatcher, Unsupported};
use crate::shape::Shape;

/// Computes the pair of closest points between two shapes.
///
/// This query finds the two points on the surfaces of the shapes that are closest
/// to each other, up to a specified maximum distance. It's particularly useful for
/// proximity detection, AI systems, and optimized spatial queries where you only
/// care about nearby objects.
///
/// # Behavior
///
/// The function returns one of three possible results:
///
/// - **`Intersecting`**: Shapes are overlapping (penetrating or touching)
/// - **`WithinMargin`**: Shapes are separated but the distance between them is ≤ `max_dist`
/// - **`Disjoint`**: Shapes are separated by more than `max_dist`
///
/// When shapes are separated and within the margin, the returned points represent
/// the exact locations on each shape's surface that are closest to each other.
///
/// # Maximum Distance Parameter
///
/// The `max_dist` parameter controls how far to search for closest points:
///
/// - **`max_dist = 0.0`**: Only returns points if shapes are touching or intersecting
/// - **`max_dist > 0.0`**: Returns points if separation distance ≤ `max_dist`
/// - **`max_dist = f32::MAX`**: Always computes closest points (no distance limit)
///
/// Using a finite `max_dist` can significantly improve performance by allowing early
/// termination when shapes are far apart.
///
/// # Arguments
///
/// * `pos1` - Position and orientation of the first shape in world space
/// * `g1` - The first shape (can be any shape implementing the `Shape` trait)
/// * `pos2` - Position and orientation of the second shape in world space
/// * `g2` - The second shape (can be any shape implementing the `Shape` trait)
/// * `max_dist` - Maximum separation distance to search for closest points
///
/// # Returns
///
/// * `Ok(ClosestPoints::Intersecting)` - Shapes are overlapping
/// * `Ok(ClosestPoints::WithinMargin(pt1, pt2))` - Closest points in world-space
/// * `Ok(ClosestPoints::Disjoint)` - Shapes are further than `max_dist` apart
/// * `Err(Unsupported)` - This shape pair combination is not supported
///
/// # Performance
///
/// Performance depends on shape types and the `max_dist` parameter:
///
/// - **Ball-Ball**: Very fast (analytical solution)
/// - **Convex-Convex**: Moderate (GJK algorithm)
/// - **Concave shapes**: Slower (requires BVH traversal)
/// - **Finite `max_dist`**: Faster due to early termination when distance exceeds limit
///
/// # Example: Basic Usage
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::{closest_points, ClosestPoints};
/// use parry3d::shape::Ball;
/// use parry3d::math::Pose;
///
/// let ball1 = Ball::new(1.0);
/// let ball2 = Ball::new(1.0);
///
/// // Position balls 5 units apart
/// let pos1 = Pose::translation(0.0, 0.0, 0.0);
/// let pos2 = Pose::translation(5.0, 0.0, 0.0);
///
/// // Find closest points (unlimited distance)
/// let result = closest_points(&pos1, &ball1, &pos2, &ball2, f32::MAX).unwrap();
///
/// if let ClosestPoints::WithinMargin(pt1, pt2) = result {
///     // pt1 is at (1.0, 0.0, 0.0) - surface of ball1
///     // pt2 is at (4.0, 0.0, 0.0) - surface of ball2
///     let distance = (pt2 - pt1).length();
///     assert!((distance - 3.0).abs() < 1e-5); // 5.0 - 1.0 - 1.0 = 3.0
/// }
/// # }
/// ```
///
/// # Example: Limited Search Distance
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::{closest_points, ClosestPoints};
/// use parry3d::shape::Ball;
/// use parry3d::math::Pose;
///
/// let ball1 = Ball::new(1.0);
/// let ball2 = Ball::new(1.0);
///
/// let pos1 = Pose::translation(0.0, 0.0, 0.0);
/// let pos2 = Pose::translation(10.0, 0.0, 0.0);
///
/// // Only search within 5.0 units
/// let result = closest_points(&pos1, &ball1, &pos2, &ball2, 5.0).unwrap();
///
/// // Shapes are 8.0 units apart (10.0 - 1.0 - 1.0), which exceeds max_dist
/// assert_eq!(result, ClosestPoints::Disjoint);
///
/// // With larger search distance, we get the points
/// let result2 = closest_points(&pos1, &ball1, &pos2, &ball2, 10.0).unwrap();
/// assert!(matches!(result2, ClosestPoints::WithinMargin(_, _)));
/// # }
/// ```
///
/// # Example: Intersecting Shapes
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::{closest_points, ClosestPoints};
/// use parry3d::shape::Cuboid;
/// use parry3d::math::{Pose, Vector};
///
/// let box1 = Cuboid::new(Vector::new(2.0, 2.0, 2.0));
/// let box2 = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
///
/// // Position boxes so they overlap
/// let pos1 = Pose::translation(0.0, 0.0, 0.0);
/// let pos2 = Pose::translation(2.0, 0.0, 0.0);
///
/// let result = closest_points(&pos1, &box1, &pos2, &box2, 10.0).unwrap();
///
/// // When shapes intersect, closest points are undefined
/// assert_eq!(result, ClosestPoints::Intersecting);
/// # }
/// ```
///
/// # Example: AI Proximity Detection
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::{closest_points, ClosestPoints};
/// use parry3d::shape::Ball;
/// use parry3d::math::Pose;
///
/// // Enemy detection radius
/// let detection_radius = 15.0;
///
/// let player = Ball::new(0.5);
/// let enemy = Ball::new(0.5);
///
/// let player_pos = Pose::translation(0.0, 0.0, 0.0);
/// let enemy_pos = Pose::translation(12.0, 0.0, 0.0);
///
/// let result = closest_points(
///     &player_pos,
///     &player,
///     &enemy_pos,
///     &enemy,
///     detection_radius
/// ).unwrap();
///
/// match result {
///     ClosestPoints::WithinMargin(player_point, enemy_point) => {
///         // Enemy detected! Calculate approach vector
///         let approach_vector = player_point - enemy_point;
///         println!("Enemy approaching from: {:?}", approach_vector.normalize());
///     }
///     ClosestPoints::Disjoint => {
///         println!("Enemy out of detection range");
///     }
///     ClosestPoints::Intersecting => {
///         println!("Enemy contact!");
///     }
/// }
/// # }
/// ```
///
/// # Example: Different Shape Types
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::{closest_points, ClosestPoints};
/// use parry3d::shape::{Ball, Cuboid};
/// use parry3d::math::{Pose, Vector};
///
/// let ball = Ball::new(2.0);
/// let cuboid = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
///
/// let pos_ball = Pose::translation(5.0, 0.0, 0.0);
/// let pos_cuboid = Pose::translation(0.0, 0.0, 0.0);
///
/// let result = closest_points(&pos_ball, &ball, &pos_cuboid, &cuboid, 10.0).unwrap();
///
/// if let ClosestPoints::WithinMargin(pt_ball, pt_cuboid) = result {
///     // pt_ball is on the ball's surface
///     // pt_cuboid is on the cuboid's surface
///     println!("Closest point on ball: {:?}", pt_ball);
///     println!("Closest point on cuboid: {:?}", pt_cuboid);
///
///     // Verify distance
///     let separation = (pt_ball - pt_cuboid).length();
///     println!("Separation distance: {}", separation);
/// }
/// # }
/// ```
///
/// # Comparison with Other Queries
///
/// Choose the right query for your use case:
///
/// | Query | Returns | Use When |
/// |-------|---------|----------|
/// | `closest_points` | Vector locations | You need exact surface points |
/// | [`distance`](crate::query::distance::distance()) | Distance value | You only need the distance |
/// | [`contact`](crate::query::contact::contact()) | Contact info | Shapes are touching/penetrating |
/// | [`intersection_test`](crate::query::intersection_test::intersection_test()) | Boolean | You only need yes/no overlap |
///
/// # See Also
///
/// - [`ClosestPoints`] - The return type with detailed documentation
/// - [`distance`](crate::query::distance::distance()) - For just the distance value
/// - [`contact`](crate::query::contact::contact()) - For penetration depth and contact normals
/// - [`intersection_test`](crate::query::intersection_test::intersection_test()) - For boolean overlap test
pub fn closest_points(
    pos1: &Pose,
    g1: &dyn Shape,
    pos2: &Pose,
    g2: &dyn Shape,
    max_dist: Real,
) -> Result<ClosestPoints, Unsupported> {
    let pos12 = pos1.inv_mul(pos2);
    DefaultQueryDispatcher
        .closest_points(&pos12, g1, g2, max_dist)
        .map(|res| res.transform_by(pos1, pos2))
}

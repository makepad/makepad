use crate::math::Pose;
use crate::query::{DefaultQueryDispatcher, QueryDispatcher, Unsupported};
use crate::shape::Shape;

/// Tests whether two shapes are intersecting (overlapping).
///
/// This is the fastest collision query, returning only a boolean result without
/// computing contact points, normals, or penetration depth. Use this when you only
/// need to know **if** shapes collide, not **where** or **how much**.
///
/// # Behavior
///
/// Returns `true` if:
/// - Shapes are **touching** (surfaces just make contact)
/// - Shapes are **penetrating** (overlapping with any amount)
///
/// Returns `false` if:
/// - Shapes are **separated** (any distance apart, even very close)
///
/// # Performance
///
/// This is the fastest collision detection query:
/// - **Ball-Ball**: Extremely fast (just distance check)
/// - **AABB-AABB**: Very fast (6 comparisons in 3D)
/// - **Convex-Convex**: Fast (early-exit GJK algorithm)
/// - **Concave shapes**: Uses BVH for acceleration
///
/// Significantly faster than `contact()` or `distance()` because it can
/// terminate early once any intersection is found.
///
/// # Arguments
///
/// * `pos1` - Position and orientation of the first shape
/// * `g1` - The first shape
/// * `pos2` - Position and orientation of the second shape
/// * `g2` - The second shape
///
/// # Returns
///
/// * `Ok(true)` - Shapes are intersecting
/// * `Ok(false)` - Shapes are not intersecting
/// * `Err(Unsupported)` - This shape pair combination is not supported
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::intersection_test;
/// use parry3d::shape::Ball;
/// use parry3d::math::Pose;
///
/// let ball1 = Ball::new(1.0);
/// let ball2 = Ball::new(1.0);
///
/// // Overlapping balls
/// let pos1 = Pose::translation(0.0, 0.0, 0.0);
/// let pos2 = Pose::translation(1.5, 0.0, 0.0);
///
/// let intersecting = intersection_test(&pos1, &ball1, &pos2, &ball2).unwrap();
/// assert!(intersecting); // Distance 1.5 < combined radii 2.0
///
/// // Separated balls
/// let pos3 = Pose::translation(5.0, 0.0, 0.0);
/// let not_intersecting = intersection_test(&pos1, &ball1, &pos3, &ball2).unwrap();
/// assert!(!not_intersecting); // Distance 5.0 > combined radii 2.0
/// # }
/// ```
///
/// # Use Cases
///
/// - **Trigger volumes**: Detect when player enters an area
/// - **Broad-phase**: Quickly filter out distant object pairs
/// - **Game logic**: Simple overlap detection (pickup items, damage zones)
/// - **Optimization**: Pre-check before expensive narrow-phase queries
///
/// # When to Use Other Queries
///
/// - Need contact points/normal? → Use [`contact`](crate::query::contact())
/// - Need penetration depth? → Use [`contact`](crate::query::contact())
/// - Need separation distance? → Use [`distance`](crate::query::distance())
/// - Need closest points? → Use [`closest_points`](crate::query::closest_points())
///
/// # See Also
///
/// - [`contact`](crate::query::contact()) - Get contact information if intersecting
/// - [`distance`](crate::query::distance()) - Get separation distance
/// - [`closest_points`](crate::query::closest_points()) - Get closest point locations
pub fn intersection_test(
    pos1: &Pose,
    g1: &dyn Shape,
    pos2: &Pose,
    g2: &dyn Shape,
) -> Result<bool, Unsupported> {
    let pos12 = pos1.inv_mul(pos2);
    DefaultQueryDispatcher.intersection_test(&pos12, g1, g2)
}

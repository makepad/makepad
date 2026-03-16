use crate::math::{Pose, Real};
use crate::query::{Contact, DefaultQueryDispatcher, QueryDispatcher, Unsupported};
use crate::shape::Shape;

/// Computes contact information between two shapes.
///
/// This function finds a single contact point (or pair of points) where two shapes
/// touch or penetrate. It's one of the most important queries for physics simulation
/// and collision response.
///
/// # Behavior
///
/// The function returns contact data in the following cases:
///
/// - **Penetrating**: Shapes overlap → Returns contact with `dist < 0.0`
/// - **Touching**: Shapes just make contact → Returns contact with `dist ≈ 0.0`
/// - **Nearly touching**: Distance ≤ `prediction` → Returns contact with `dist > 0.0`
/// - **Separated**: Distance > `prediction` → Returns `None`
///
/// # Prediction Distance
///
/// The `prediction` parameter allows detecting contacts before shapes actually touch.
/// This is useful for:
///
/// - **Continuous collision detection**: Predict contacts that will occur soon
/// - **Speculative contacts**: Pre-compute contacts for upcoming frames
/// - **Efficiency**: Avoid re-computing contacts every frame for slow-moving objects
///
/// Use `0.0` for exact contact detection only.
///
/// # Arguments
///
/// * `pos1` - Position and orientation of the first shape in world space
/// * `g1` - The first shape
/// * `pos2` - Position and orientation of the second shape in world space
/// * `g2` - The second shape
/// * `prediction` - Maximum separation distance for contact detection (typically `0.0`)
///
/// # Returns
///
/// * `Ok(Some(contact))` - Contact found (touching, penetrating, or within prediction distance)
/// * `Ok(None)` - No contact (shapes separated by more than `prediction`)
/// * `Err(Unsupported)` - This shape pair combination is not supported
///
/// # Contact Data
///
/// The returned [`Contact`] contains:
/// - `point1`, `point2`: Contact points on each shape's surface (world space)
/// - `normal1`, `normal2`: Surface normals at contact points (world space)
/// - `dist`: Signed distance (negative = penetration depth)
///
/// # Performance
///
/// Performance depends on shape types:
/// - **Convex-Convex**: Fast (GJK/EPA algorithms)
/// - **Concave shapes**: Slower (requires BVH traversal)
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::contact;
/// use parry3d::shape::{Ball, Cuboid};
/// use parry3d::math::{Pose, Vector};
///
/// let ball = Ball::new(1.0);
/// let cuboid = Cuboid::new(Vector::new(2.0, 2.0, 2.0));
///
/// // Position shapes so they're penetrating
/// let pos_ball = Pose::translation(2.5, 0.0, 0.0);
/// let pos_cuboid = Pose::identity();
///
/// // Compute contact (no prediction distance)
/// if let Ok(Some(contact)) = contact(&pos_ball, &ball, &pos_cuboid, &cuboid, 0.0) {
///     // Penetration depth (negative distance)
///     let penetration = -contact.dist;
///     println!("Penetrating by: {} units", penetration);
///
///     // Normal points from ball toward cuboid
///     println!("Separation direction: {:?}", contact.normal1);
///
///     // Use contact to resolve collision:
///     // - Apply impulse along normal1 to separate shapes
///     // - Move shapes apart by penetration depth
/// }
/// # }
/// ```
///
/// # Example with Prediction
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::contact;
/// use parry3d::shape::Ball;
/// use parry3d::math::Pose;
///
/// let ball1 = Ball::new(1.0);
/// let ball2 = Ball::new(1.0);
///
/// // Balls separated by 2.2 units (just outside contact range)
/// let pos1 = Pose::translation(0.0, 0.0, 0.0);
/// let pos2 = Pose::translation(4.2, 0.0, 0.0); // radii sum = 2.0, gap = 2.2
///
/// // Without prediction: no contact
/// assert!(contact(&pos1, &ball1, &pos2, &ball2, 0.0).unwrap().is_none());
///
/// // With prediction of 0.5: still no contact (gap 2.2 > 0.5)
/// assert!(contact(&pos1, &ball1, &pos2, &ball2, 0.5).unwrap().is_none());
///
/// // With prediction of 3.0: contact detected
/// if let Ok(Some(c)) = contact(&pos1, &ball1, &pos2, &ball2, 3.0) {
///     assert!(c.dist > 0.0); // Positive = separated but within prediction
///     assert!(c.dist <= 3.0); // Within prediction distance
/// }
/// # }
/// ```
///
/// # See Also
///
/// - [`distance`](crate::query::distance::distance()) - For just the distance value
/// - [`closest_points`](crate::query::closest_points::closest_points()) - For closest point locations
/// - [`intersection_test`](crate::query::intersection_test::intersection_test()) - For boolean overlap test
pub fn contact(
    pos1: &Pose,
    g1: &dyn Shape,
    pos2: &Pose,
    g2: &dyn Shape,
    prediction: Real,
) -> Result<Option<Contact>, Unsupported> {
    let pos12 = pos1.inv_mul(pos2);
    let mut result = DefaultQueryDispatcher.contact(&pos12, g1, g2, prediction);

    if let Ok(Some(contact)) = &mut result {
        contact.transform_by_mut(pos1, pos2);
    }

    result
}

use crate::math::{Pose, Real, Vector};
use crate::shape::SupportMap;

/// Computes the separation distance between two convex shapes along a given direction.
///
/// This is the most general SAT separation computation function in Parry. It works with any
/// two convex shapes that implement the [`SupportMap`] trait, which includes spheres, capsules,
/// cones, convex polyhedra, and more.
///
/// # What is a Support Map?
///
/// A support map is a function that, given a direction vector, returns the furthest point on
/// the shape in that direction. This is a fundamental operation in collision detection algorithms
/// like GJK, EPA, and SAT.
///
/// # How This Function Works
///
/// 1. Finds the furthest point on `sm1` in direction `dir1` (the "support point")
/// 2. Finds the furthest point on `sm2` in direction `-dir1` (opposite direction)
/// 3. Transforms `sm2`'s support point to `sm1`'s coordinate space
/// 4. Computes the signed distance between these points along `dir1`
///
/// # Parameters
///
/// - `sm1`: The first convex shape
/// - `sm2`: The second convex shape
/// - `pos12`: The position of `sm2` relative to `sm1`
/// - `dir1`: The unit direction vector (in `sm1`'s local space) along which to compute separation
///
/// # Returns
///
/// The separation distance as a `Real`:
/// - **Positive**: The shapes are separated by at least this distance along `dir1`
/// - **Negative**: The shapes are overlapping (the absolute value is the penetration depth)
/// - **Zero**: The shapes are exactly touching along this axis
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::{Ball, Cuboid};
/// use parry3d::query::sat::support_map_support_map_compute_separation;
/// use parry3d::math::{Pose, Vector};
///
/// let sphere = Ball::new(1.0);
/// let cube = Cuboid::new(Vector::splat(1.0));
///
/// // Position cube to the right of the sphere
/// let pos12 = Pose::translation(3.0, 0.0, 0.0);
///
/// // Test separation along the X axis
/// let dir = Vector::X.normalize();
/// let separation = support_map_support_map_compute_separation(
///     &sphere,
///     &cube,
///     &pos12,
///     dir
/// );
///
/// // They should be separated (sphere radius 1.0 + cube extent 1.0 = 2.0, distance 3.0)
/// assert!(separation > 0.0);
/// # }
/// ```
///
/// # Use Cases
///
/// This function is typically used as a building block in SAT implementations. You would:
/// 1. Generate candidate separating axes (face normals, edge cross products, etc.)
/// 2. Call this function for each candidate axis
/// 3. Track the axis with maximum separation
/// 4. If the maximum separation is positive, the shapes don't collide
///
/// # Performance Note
///
/// This function is generic and works with any support map shapes, but specialized implementations
/// (like [`cuboid_cuboid_find_local_separating_normal_oneway`](super::cuboid_cuboid_find_local_separating_normal_oneway))
/// may be more efficient for specific shape pairs.
#[allow(dead_code)]
pub fn support_map_support_map_compute_separation(
    sm1: &impl SupportMap,
    sm2: &impl SupportMap,
    pos12: &Pose,
    dir1: Vector,
) -> Real {
    let p1 = sm1.local_support_point_toward(dir1);
    let p2 = sm2.support_point_toward(pos12, -dir1);
    (p2 - p1).dot(dir1)
}

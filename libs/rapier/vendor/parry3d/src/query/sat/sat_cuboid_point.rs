use crate::math::{Pose, Real, Vector};
use crate::shape::{Cuboid, SupportMap};

/// Computes the separation distance between a point and a cuboid along a specified normal direction.
///
/// This function is used in SAT (Separating Axis Theorem) implementations for shapes that can be
/// treated as having a single representative point with an associated normal vector. Examples include:
/// - Segments (in 2D, using one endpoint and the segment's normal)
/// - Triangles (in 3D, using one vertex and the triangle's face normal)
///
/// # Why This Works
///
/// For a cuboid centered at the origin with symmetry, we only need to test one direction of the
/// normal (not both +normal and -normal) because the cuboid looks the same from both directions.
/// This optimization makes the function more efficient than the general support map approach.
///
/// # Parameters
///
/// - `point1`: A point in the first shape's local coordinate space
/// - `normal1`: Optional unit normal vector associated with the point (e.g., triangle face normal)
///   - If `None`, the function returns maximum negative separation (indicating overlap)
/// - `shape2`: The cuboid to test against
/// - `pos12`: The position of `shape2` (cuboid) relative to `point1`'s coordinate frame
///
/// # Returns
///
/// A tuple containing:
/// - `Real`: The separation distance along the normal direction
///   - **Positive**: The point and cuboid are separated
///   - **Negative**: The point penetrates the cuboid (or normal is None)
///   - **Zero**: The point exactly touches the cuboid surface
/// - `Vector`: The oriented normal direction used for the test (pointing from point toward cuboid)
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::shape::Cuboid;
/// use parry2d::query::sat::point_cuboid_find_local_separating_normal_oneway;
/// use parry2d::math::{Vector, Pose};
///
/// let point = Vector::ZERO;
/// let normal = Some((Vector::X.normalize()));
/// let cuboid = Cuboid::new(Vector::new(1.0, 1.0));
///
/// // Position cuboid 3 units to the right
/// let pos12 = Pose::translation(3.0, 0.0);
///
/// let (separation, _dir) = point_cuboid_find_local_separating_normal_oneway(
///     point,
///     normal,
///     &cuboid,
///     &pos12
/// );
///
/// // Should be separated by 1.0 (distance 3.0 - cuboid extent 1.0 - point distance 0.0)
/// assert!(separation > 0.0);
/// # }
/// ```
///
/// # Implementation Note
///
/// This function only works correctly when the **cuboid is on the right-hand side** (as shape2)
/// because it exploits the cuboid's symmetry around the origin. The cuboid must be centered at
/// its local origin for this optimization to be valid.
pub fn point_cuboid_find_local_separating_normal_oneway(
    point1: Vector,
    normal1: Option<Vector>,
    shape2: &Cuboid,
    pos12: &Pose,
) -> (Real, Vector) {
    let mut best_separation = -Real::MAX;
    let mut best_dir = Vector::ZERO;

    if let Some(normal1) = normal1 {
        let axis1 = if (pos12.translation - point1).dot(normal1) >= 0.0 {
            normal1
        } else {
            -normal1
        };

        let pt2 = shape2.support_point_toward(pos12, -axis1);
        let separation = (pt2 - point1).dot(axis1);

        if separation > best_separation {
            best_separation = separation;
            best_dir = axis1;
        }
    }

    (best_separation, best_dir)
}

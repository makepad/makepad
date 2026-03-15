use crate::math::{Pose, Real, Vector, VectorExt, DIM};
use crate::shape::{Cuboid, SupportMap};

/// Computes the separation distance between two cuboids along a given axis.
///
/// This function is part of the Separating Axis Theorem (SAT) implementation for cuboid-cuboid
/// collision detection. It projects both cuboids onto the specified axis and computes how far
/// apart they are (positive = separated, negative = overlapping).
///
/// # How It Works
///
/// 1. Orients the axis to point from cuboid1 toward cuboid2 (using the translation vector)
/// 2. Finds the support points (furthest points) on each cuboid in that direction
/// 3. Computes the signed distance between these support points along the axis
///
/// # Parameters
///
/// - `cuboid1`: The first cuboid (in its local coordinate frame)
/// - `cuboid2`: The second cuboid
/// - `pos12`: The position of cuboid2 relative to cuboid1 (transforms from cuboid2's space to cuboid1's space)
/// - `axis1`: The axis direction in cuboid1's local space to test for separation
///
/// # Returns
///
/// A tuple containing:
/// - `Real`: The separation distance along the axis
///   - **Positive**: Shapes are separated by this distance
///   - **Negative**: Shapes are overlapping (penetration depth is the absolute value)
///   - **Zero**: Shapes are exactly touching
/// - `Vector`: The oriented axis direction (pointing from cuboid1 toward cuboid2)
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Cuboid;
/// use parry3d::query::sat::cuboid_cuboid_compute_separation_wrt_local_line;
/// use parry3d::math::{Pose, Vector};
///
/// let cube1 = Cuboid::new(Vector::splat(1.0));
/// let cube2 = Cuboid::new(Vector::splat(1.0));
///
/// // Position cube2 at (3, 0, 0) relative to cube1
/// let pos12 = Pose::translation(3.0, 0.0, 0.0);
///
/// // Test separation along the X axis
/// let (separation, _axis) = cuboid_cuboid_compute_separation_wrt_local_line(
///     &cube1,
///     &cube2,
///     &pos12,
///     Vector::X
/// );
///
/// // Should be separated by 1.0 (distance 3.0 - half_extents 1.0 - 1.0)
/// assert!(separation > 0.0);
/// # }
/// ```
#[cfg(feature = "dim3")]
pub fn cuboid_cuboid_compute_separation_wrt_local_line(
    cuboid1: &Cuboid,
    cuboid2: &Cuboid,
    pos12: &Pose,
    axis1: Vector,
) -> (Real, Vector) {
    #[expect(clippy::unnecessary_cast)]
    let signum = (1.0 as Real).copysign(pos12.translation.dot(axis1));
    let axis1 = axis1 * signum;
    let axis2 = pos12.rotation.inverse() * -axis1;
    let local_pt1 = cuboid1.local_support_point(axis1);
    let local_pt2 = cuboid2.local_support_point(axis2);
    let pt2 = pos12 * local_pt2;
    let separation = (pt2 - local_pt1).dot(axis1);
    (separation, axis1)
}

/// Finds the best separating axis by testing all edge-edge combinations between two cuboids.
///
/// In 3D, edge-edge contact is common when two boxes collide. This function tests all possible
/// axes formed by the cross product of edges from each cuboid to find the axis with maximum
/// separation (or minimum penetration).
///
/// # Why Test Edge Cross Products?
///
/// When two 3D convex polyhedra collide, the separating axis (if one exists) must be either:
/// 1. A face normal from one shape
/// 2. A face normal from the other shape
/// 3. **Perpendicular to an edge from each shape** (the cross product of the edges)
///
/// This function handles case 3. For two cuboids, there are 3 edges per cuboid (aligned with X, Y, Z),
/// giving 3 × 3 = 9 possible edge pair combinations to test.
///
/// # Parameters
///
/// - `cuboid1`: The first cuboid (in its local coordinate frame)
/// - `cuboid2`: The second cuboid
/// - `pos12`: The position of cuboid2 relative to cuboid1
///
/// # Returns
///
/// A tuple containing:
/// - `Real`: The best (maximum) separation found across all edge-edge axes
///   - **Positive**: Shapes are separated
///   - **Negative**: Shapes are overlapping
/// - `Vector`: The axis direction that gives this separation
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Cuboid;
/// use parry3d::query::sat::cuboid_cuboid_find_local_separating_edge_twoway;
/// use parry3d::math::{Pose, Vector};
///
/// let cube1 = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
/// let cube2 = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
///
/// // Rotate and position cube2 so edge-edge contact is likely
/// let pos12 = Pose::translation(2.0, 2.0, 0.0);
///
/// let (separation, _axis) = cuboid_cuboid_find_local_separating_edge_twoway(
///     &cube1,
///     &cube2,
///     &pos12
/// );
///
/// if separation > 0.0 {
///     println!("Separated by {} along an edge-edge axis", separation);
/// }
/// # }
/// ```
///
/// # Note
///
/// This function only tests edge-edge axes. For a complete SAT test, you must also test
/// face normals using [`cuboid_cuboid_find_local_separating_normal_oneway`].
#[cfg(feature = "dim3")]
pub fn cuboid_cuboid_find_local_separating_edge_twoway(
    cuboid1: &Cuboid,
    cuboid2: &Cuboid,
    pos12: &Pose,
) -> (Real, Vector) {
    use approx::AbsDiffEq;
    let mut best_separation = -Real::MAX;
    let mut best_dir = Vector::ZERO;

    let x2 = pos12.rotation * Vector::X;
    let y2 = pos12.rotation * Vector::Y;
    let z2 = pos12.rotation * Vector::Z;

    // We have 3 * 3 = 9 axes to test.
    let axes = [
        // Vector::{x, y ,z}().cross(y2)
        Vector::new(0.0, -x2.z, x2.y),
        Vector::new(x2.z, 0.0, -x2.x),
        Vector::new(-x2.y, x2.x, 0.0),
        // Vector::{x, y ,z}().cross(y2)
        Vector::new(0.0, -y2.z, y2.y),
        Vector::new(y2.z, 0.0, -y2.x),
        Vector::new(-y2.y, y2.x, 0.0),
        // Vector::{x, y ,z}().cross(y2)
        Vector::new(0.0, -z2.z, z2.y),
        Vector::new(z2.z, 0.0, -z2.x),
        Vector::new(-z2.y, z2.x, 0.0),
    ];

    for axis1 in &axes {
        let norm1 = axis1.length();
        if norm1 > Real::default_epsilon() {
            let (separation, axis1) = cuboid_cuboid_compute_separation_wrt_local_line(
                cuboid1,
                cuboid2,
                pos12,
                axis1 / norm1,
            );

            if separation > best_separation {
                best_separation = separation;
                best_dir = axis1;
            }
        }
    }

    (best_separation, best_dir)
}

/// Finds the best separating axis by testing the face normals of the first cuboid.
///
/// This function tests the face normals (X, Y, Z axes in 2D/3D) of `cuboid1` to find which
/// direction gives the maximum separation between the two cuboids. This is the "one-way" test
/// that only considers faces from one cuboid.
///
/// # Why "One-Way"?
///
/// For a complete SAT test between two cuboids, you need to test:
/// 1. Face normals from cuboid1 (this function)
/// 2. Face normals from cuboid2 (call this function again with swapped arguments)
/// 3. Edge-edge cross products in 3D (`cuboid_cuboid_find_local_separating_edge_twoway`)
///
/// By testing only one shape's normals at a time, the implementation can be more efficient
/// and reusable.
///
/// # Parameters
///
/// - `cuboid1`: The cuboid whose face normals will be tested
/// - `cuboid2`: The other cuboid
/// - `pos12`: The position of cuboid2 relative to cuboid1
///
/// # Returns
///
/// A tuple containing:
/// - `Real`: The maximum separation found among cuboid1's face normals
///   - **Positive**: Shapes are separated by at least this distance
///   - **Negative**: Shapes are overlapping (penetration)
/// - `Vector`: The face normal direction that gives this separation
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::shape::Cuboid;
/// use parry2d::query::sat::cuboid_cuboid_find_local_separating_normal_oneway;
/// use parry2d::math::{Pose, Vector};
///
/// let rect1 = Cuboid::new(Vector::new(1.0, 1.0));
/// let rect2 = Cuboid::new(Vector::new(0.5, 0.5));
///
/// // Position rect2 to the right of rect1
/// let pos12 = Pose::translation(2.5, 0.0);
///
/// // Test rect1's face normals (X and Y axes)
/// let (sep1, normal1) = cuboid_cuboid_find_local_separating_normal_oneway(
///     &rect1, &rect2, &pos12
/// );
///
/// // Test rect2's face normals by swapping arguments and inverting the transform
/// let (sep2, normal2) = cuboid_cuboid_find_local_separating_normal_oneway(
///     &rect2, &rect1, &pos12.inverse()
/// );
///
/// // The maximum separation indicates if shapes collide
/// let max_separation = sep1.max(sep2);
/// if max_separation > 0.0 {
///     println!("Shapes are separated!");
/// }
/// # }
/// ```
///
/// # Algorithm Details
///
/// For each axis (X, Y, and Z in 3D):
/// 1. Computes the support point on cuboid2 in the negative axis direction
/// 2. Transforms it to cuboid1's space
/// 3. Measures the signed distance from cuboid1's boundary
/// 4. Tracks the axis with maximum separation
pub fn cuboid_cuboid_find_local_separating_normal_oneway(
    cuboid1: &Cuboid,
    cuboid2: &Cuboid,
    pos12: &Pose,
) -> (Real, Vector) {
    let mut best_separation = -Real::MAX;
    let mut best_dir = Vector::ZERO;

    for i in 0..DIM {
        #[expect(clippy::unnecessary_cast)]
        let sign = (1.0 as Real).copysign(pos12.translation[i]);
        let axis1 = Vector::ith(i, sign);
        let axis2 = pos12.rotation.inverse() * -axis1;
        let local_pt2 = cuboid2.local_support_point(axis2);
        let pt2 = pos12 * local_pt2;
        let separation = pt2[i] * sign - cuboid1.half_extents[i];

        if separation > best_separation {
            best_separation = separation;
            best_dir = axis1;
        }
    }

    (best_separation, best_dir)
}

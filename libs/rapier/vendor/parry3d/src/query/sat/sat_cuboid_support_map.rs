use crate::math::{Pose, Real, Vector, VectorExt, DIM};
use crate::shape::{Cuboid, SupportMap};

/// Computes the separation distance between a cuboid and a convex support map shape along a given axis.
///
/// This function tests both the positive and negative directions of the axis to find which
/// orientation gives the actual separation between the shapes. This is necessary because we don't
/// know in advance which direction will show the true separation.
///
/// # Parameters
///
/// - `cube1`: The cuboid
/// - `shape2`: Any convex shape implementing [`SupportMap`] (sphere, capsule, convex mesh, etc.)
/// - `pos12`: The position of `shape2` relative to `cube1`
/// - `axis1`: The unit direction vector (in `cube1`'s local space) to test
///
/// # Returns
///
/// A tuple containing:
/// - `Real`: The separation distance along the better of the two axis directions
///   - **Positive**: Shapes are separated
///   - **Negative**: Shapes are overlapping
/// - `Vector`: The axis direction (either `axis1` or `-axis1`) that gives this separation
///
/// # Why Test Both Directions?
///
/// When testing separation along an axis, we need to check both `axis1` and `-axis1` because:
/// - The shapes might be oriented such that one direction shows separation while the other shows overlap
/// - We want to find the direction that gives the maximum (least negative) separation
///
/// For symmetric shapes like sphere vs sphere, both directions give the same result, but for
/// asymmetric configurations, testing both is necessary for correctness.
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::{Cuboid, Ball};
/// use parry3d::query::sat::cuboid_support_map_compute_separation_wrt_local_line;
/// use parry3d::math::{Pose, Vector};
///
/// let cube = Cuboid::new(Vector::splat(1.0));
/// let sphere = Ball::new(0.5);
///
/// // Position sphere near the cube
/// let pos12 = Pose::translation(2.0, 0.0, 0.0);
///
/// // Test separation along an arbitrary axis
/// let axis = Vector::new(1.0, 1.0, 0.0).normalize();
/// let (separation, chosen_axis) = cuboid_support_map_compute_separation_wrt_local_line(
///     &cube,
///     &sphere,
///     &pos12,
///     axis
/// );
///
/// println!("Separation: {} along axis: {:?}", separation, chosen_axis);
/// # }
/// ```
///
/// # Performance Note
///
/// This is a relatively expensive operation as it computes support points in both directions.
/// For specific shape pairs (like cuboid-cuboid), there are optimized versions that avoid this
/// double computation.
#[cfg(feature = "dim3")]
pub fn cuboid_support_map_compute_separation_wrt_local_line(
    cube1: &Cuboid,
    shape2: &impl SupportMap,
    pos12: &Pose,
    axis1: Vector,
) -> (Real, Vector) {
    let axis1_2 = pos12.rotation.inverse() * axis1;
    let separation1 = {
        let axis2 = -axis1_2;
        let local_pt1 = cube1.local_support_point_toward(axis1);
        let local_pt2 = shape2.local_support_point_toward(axis2);
        let pt2 = pos12 * local_pt2;
        (pt2 - local_pt1).dot(axis1)
    };

    let separation2 = {
        let axis2 = axis1_2;
        let local_pt1 = cube1.local_support_point_toward(-axis1);
        let local_pt2 = shape2.local_support_point_toward(axis2);
        let pt2 = pos12 * local_pt2;
        (pt2 - local_pt1).dot(-axis1)
    };

    if separation1 > separation2 {
        (separation1, axis1)
    } else {
        (separation2, -axis1)
    }
}

/// Finds the best separating axis by testing edge-edge combinations between a cuboid and a support map shape.
///
/// This function is used in 3D SAT implementations where edge-edge contact is possible. It tests
/// a precomputed set of axes (typically cross products of edges from both shapes) to find the
/// axis with maximum separation.
///
/// # Parameters
///
/// - `cube1`: The cuboid
/// - `shape2`: Any convex shape implementing [`SupportMap`]
/// - `axes`: A slice of axis directions to test (not necessarily unit length)
/// - `pos12`: The position of `shape2` relative to `cube1`
///
/// # Returns
///
/// A tuple containing:
/// - `Real`: The maximum separation found across all tested axes
///   - **Positive**: Shapes are separated
///   - **Negative**: Shapes are overlapping (minimum penetration)
/// - `Vector`: The axis direction that gives this separation (normalized)
///
/// # Why Precomputed Axes?
///
/// The caller typically computes the candidate axes as cross products of edges:
/// - Cuboid has 3 edge directions (X, Y, Z)
/// - The other shape's edges depend on its geometry
/// - Cross products of these edges give potential separating axes
///
/// This function handles the actual separation testing for those precomputed axes.
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::{Cuboid, Capsule};
/// use parry3d::query::sat::cuboid_support_map_find_local_separating_edge_twoway;
/// use parry3d::math::{Pose, Vector};
///
/// let cube = Cuboid::new(Vector::splat(1.0));
/// let capsule = Capsule::new(Vector::new(0.0, -1.0, 0.0), Vector::new(0.0, 1.0, 0.0), 0.5);
///
/// // Position capsule near the cube
/// let pos12 = Pose::translation(2.0, 0.0, 0.0);
///
/// // Compute edge cross products
/// let capsule_dir = Vector::Y; // capsule's axis direction
/// let axes = [
///     Vector::X.cross(capsule_dir), // cube X × capsule axis
///     Vector::Y.cross(capsule_dir), // cube Y × capsule axis
///     Vector::Z.cross(capsule_dir), // cube Z × capsule axis
/// ];
///
/// let (separation, axis) = cuboid_support_map_find_local_separating_edge_twoway(
///     &cube,
///     &capsule,
///     &axes,
///     &pos12
/// );
///
/// println!("Best edge-edge separation: {} along {:?}", separation, axis);
/// # }
/// ```
///
/// # Implementation Details
///
/// - Axes with near-zero length are skipped (they represent parallel or degenerate edges)
/// - Each axis is normalized before computing separation
/// - The function tests both positive and negative directions of each axis using
///   `cuboid_support_map_compute_separation_wrt_local_line`
#[cfg(feature = "dim3")]
pub fn cuboid_support_map_find_local_separating_edge_twoway(
    cube1: &Cuboid,
    shape2: &impl SupportMap,
    axes: &[Vector],
    pos12: &Pose,
) -> (Real, Vector) {
    let mut best_separation = -Real::MAX;
    let mut best_dir = Vector::ZERO;

    for axis1 in axes {
        if let Some(axis1) = (*axis1).try_normalize() {
            let (separation, axis1) =
                cuboid_support_map_compute_separation_wrt_local_line(cube1, shape2, pos12, axis1);

            if separation > best_separation {
                best_separation = separation;
                best_dir = axis1;
            }
        }
    }

    (best_separation, best_dir)
}

/// Finds the best separating axis by testing the face normals of a cuboid against a support map shape.
///
/// This function tests all face normals (X, Y, and Z axes in both positive and negative directions)
/// of the cuboid to find which direction gives the maximum separation from the support map shape.
///
/// # Parameters
///
/// - `cube1`: The cuboid whose face normals will be tested
/// - `shape2`: Any convex shape implementing [`SupportMap`]
/// - `pos12`: The position of `shape2` relative to `cube1`
///
/// # Returns
///
/// A tuple containing:
/// - `Real`: The maximum separation found among the cuboid's face normals
///   - **Positive**: Shapes are separated
///   - **Negative**: Shapes are overlapping
/// - `Vector`: The face normal direction that gives this separation
///
/// # Usage in Complete SAT
///
/// This function tests only the face normals of the cuboid. For a complete SAT collision test
/// between a cuboid and another shape, you typically need to:
///
/// 1. Test cuboid's face normals (this function)
/// 2. Test the other shape's face normals (if applicable)
/// 3. Test edge-edge cross products in 3D (if both shapes have edges)
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::shape::{Cuboid, Ball};
/// use parry2d::query::sat::cuboid_support_map_find_local_separating_normal_oneway;
/// use parry2d::math::{Pose, Vector};
///
/// let cube = Cuboid::new(Vector::new(1.0, 1.0));
/// let sphere = Ball::new(0.5);
///
/// // Position sphere to the right of the cube
/// let pos12 = Pose::translation(2.0, 0.0);
///
/// let (separation, normal) = cuboid_support_map_find_local_separating_normal_oneway(
///     &cube,
///     &sphere,
///     &pos12
/// );
///
/// if separation > 0.0 {
///     println!("Shapes separated by {} along normal {}", separation, normal);
/// } else {
///     println!("Shapes overlapping by {} along normal {}", -separation, normal);
/// }
/// # }
/// ```
///
/// # Performance
///
/// This function is more efficient than `cuboid_support_map_compute_separation_wrt_local_line`
/// for face normals because it can directly compute the separation without testing both directions.
/// It tests 2×DIM axes (where DIM is 2 or 3).
pub fn cuboid_support_map_find_local_separating_normal_oneway<S: SupportMap>(
    cube1: &Cuboid,
    shape2: &S,
    pos12: &Pose,
) -> (Real, Vector) {
    let mut best_separation = -Real::MAX;
    let mut best_dir = Vector::ZERO;

    for i in 0..DIM {
        for sign in &[-1.0, 1.0] {
            let axis1 = Vector::ith(i, *sign);
            let pt2 = shape2.support_point_toward(pos12, -axis1);
            let separation = pt2[i] * *sign - cube1.half_extents[i];

            if separation > best_separation {
                best_separation = separation;
                best_dir = axis1;
            }
        }
    }

    (best_separation, best_dir)
}

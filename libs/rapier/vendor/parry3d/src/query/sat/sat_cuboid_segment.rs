use crate::math::{Pose, Real, Vector};
use crate::query::sat;
use crate::shape::{Cuboid, Segment};

/// Finds the best separating axis by testing edge-edge combinations between a cuboid and a segment (3D only).
///
/// In 3D, when a box collides with a line segment, the contact might occur along an axis
/// perpendicular to both a cuboid edge and the segment itself. This function tests all such
/// axes (cross products) to find the one with maximum separation.
///
/// # Parameters
///
/// - `cube1`: The cuboid
/// - `segment2`: The line segment
/// - `pos12`: The position of the segment relative to the cuboid
///
/// # Returns
///
/// A tuple containing:
/// - `Real`: The maximum separation found across all edge-edge axes
///   - **Positive**: Shapes are separated
///   - **Negative**: Shapes are overlapping
/// - `Vector`: The axis direction that gives this separation
///
/// # The 3 Axes Tested
///
/// A segment is a single edge, so we test cross products between:
/// - The segment's direction (B - A)
/// - Each of the 3 cuboid edge directions (X, Y, Z)
///
/// This gives 3 potential separating axes. The function delegates to
/// [`cuboid_support_map_find_local_separating_edge_twoway`](super::cuboid_support_map_find_local_separating_edge_twoway)
/// to perform the actual separation tests.
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::{Cuboid, Segment};
/// use parry3d::query::sat::cuboid_segment_find_local_separating_edge_twoway;
/// use parry3d::math::{Vector, Pose};
///
/// let cube = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
/// let segment = Segment::new(
///     Vector::ZERO,
///     Vector::new(0.0, 2.0, 0.0)
/// );
///
/// // Position segment to the right of the cube
/// let pos12 = Pose::translation(2.5, 0.0, 0.0);
///
/// let (separation, axis) = cuboid_segment_find_local_separating_edge_twoway(
///     &cube,
///     &segment,
///     &pos12
/// );
///
/// println!("Edge-edge separation: {}", separation);
/// # }
/// ```
///
/// # Usage in Complete SAT
///
/// For a complete cuboid-segment collision test, you must also test:
/// 1. Cuboid face normals (X, Y, Z axes)
/// 2. Segment normal (in 2D) or face normal (for degenerate 3D cases)
/// 3. Edge-edge axes (this function, 3D only)
#[cfg(feature = "dim3")]
pub fn cuboid_segment_find_local_separating_edge_twoway(
    cube1: &Cuboid,
    segment2: &Segment,
    pos12: &Pose,
) -> (Real, Vector) {
    let x2 = pos12.rotation * (segment2.b - segment2.a);

    let axes = [
        // Vector::{x, y ,z}().cross(y2)
        Vector::new(0.0, -x2.z, x2.y),
        Vector::new(x2.z, 0.0, -x2.x),
        Vector::new(-x2.y, x2.x, 0.0),
    ];

    sat::cuboid_support_map_find_local_separating_edge_twoway(cube1, segment2, &axes, pos12)
}

/// Finds the best separating axis by testing a segment's normal against a cuboid (2D only).
///
/// In 2D, a segment (line segment) has an associated normal vector perpendicular to the segment.
/// This function tests both directions of this normal to find the maximum separation from the cuboid.
///
/// # How It Works
///
/// The function treats the segment as a point-with-normal using one of its endpoints (point A)
/// and delegates to [`point_cuboid_find_local_separating_normal_oneway`](super::point_cuboid_find_local_separating_normal_oneway).
///
/// # Parameters
///
/// - `segment1`: The line segment whose normal will be tested
/// - `shape2`: The cuboid
/// - `pos12`: The position of the cuboid relative to the segment
///
/// # Returns
///
/// A tuple containing:
/// - `Real`: The separation distance along the segment's normal
///   - **Positive**: Shapes are separated
///   - **Negative**: Shapes are overlapping
/// - `Vector`: The normal direction that gives this separation
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::shape::{Segment, Cuboid};
/// use parry2d::query::sat::segment_cuboid_find_local_separating_normal_oneway;
/// use parry2d::math::{Vector, Pose};
///
/// // Horizontal segment
/// let segment = Segment::new(
///     Vector::ZERO,
///     Vector::new(2.0, 0.0)
/// );
/// let cuboid = Cuboid::new(Vector::new(1.0, 1.0));
///
/// // Position cuboid above the segment
/// let pos12 = Pose::translation(1.0, 2.5);
///
/// let (separation, normal) = segment_cuboid_find_local_separating_normal_oneway(
///     &segment,
///     &cuboid,
///     &pos12
/// );
///
/// println!("Separation along segment normal: {}", separation);
/// # }
/// ```
///
/// # 2D Only
///
/// This function is only available in 2D. In 3D, segments don't have a unique normal direction
/// (there are infinitely many perpendicular directions), so edge-edge cross products are used
/// instead (see `cuboid_segment_find_local_separating_edge_twoway`).
#[cfg(feature = "dim2")]
pub fn segment_cuboid_find_local_separating_normal_oneway(
    segment1: &Segment,
    shape2: &Cuboid,
    pos12: &Pose,
) -> (Real, Vector) {
    sat::point_cuboid_find_local_separating_normal_oneway(
        segment1.a,
        segment1.normal(),
        shape2,
        pos12,
    )
}

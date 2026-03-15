use crate::math::{Pose, Real, Vector};
use crate::query::sat;
use crate::shape::{Segment, SupportMap, Triangle};

/// Finds the best separating axis by testing a triangle's face normal against a segment (3D only).
///
/// In 3D, a triangle has a face normal (perpendicular to its plane). This function tests both
/// directions of this normal (+normal and -normal) to find the maximum separation from the segment.
///
/// # How It Works
///
/// The function computes support points on the segment in both the positive and negative normal
/// directions, then measures which direction gives greater separation from the triangle's surface.
///
/// # Parameters
///
/// - `triangle1`: The triangle whose face normal will be tested
/// - `segment2`: The line segment
/// - `pos12`: The position of the segment relative to the triangle
///
/// # Returns
///
/// A tuple containing:
/// - `Real`: The separation distance along the triangle's face normal
///   - **Positive**: Shapes are separated
///   - **Negative**: Shapes are overlapping
///   - **Very negative** if the triangle has no normal (degenerate triangle)
/// - `Vector`: The face normal direction (or its negation) that gives this separation
///
/// # Degenerate Triangles
///
/// If the triangle is degenerate (all three points are collinear), it has no valid normal.
/// In this case, the function returns `-Real::MAX` for separation and a zero vector for the normal.
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::{Triangle, Segment};
/// use parry3d::query::sat::triangle_segment_find_local_separating_normal_oneway;
/// use parry3d::math::{Vector, Pose};
///
/// // Triangle in the XY plane
/// let triangle = Triangle::new(
///     Vector::ZERO,
///     Vector::new(2.0, 0.0, 0.0),
///     Vector::new(1.0, 2.0, 0.0)
/// );
///
/// // Vertical segment above the triangle
/// let segment = Segment::new(
///     Vector::new(1.0, 1.0, 1.0),
///     Vector::new(1.0, 1.0, 3.0)
/// );
///
/// let pos12 = Pose::identity();
///
/// let (separation, normal) = triangle_segment_find_local_separating_normal_oneway(
///     &triangle,
///     &segment,
///     &pos12
/// );
///
/// if separation > 0.0 {
///     println!("Separated by {} along triangle normal", separation);
/// }
/// # }
/// ```
///
/// # Usage in Complete SAT
///
/// For a complete triangle-segment collision test, you must also test:
/// 1. Triangle face normal (this function)
/// 2. Segment normal (in 2D) or edge-edge axes (in 3D)
/// 3. Edge-edge cross products (see [`segment_triangle_find_local_separating_edge_twoway`])
pub fn triangle_segment_find_local_separating_normal_oneway(
    triangle1: &Triangle,
    segment2: &Segment,
    pos12: &Pose,
) -> (Real, Vector) {
    if let Some(dir) = triangle1.normal() {
        let p2a = segment2.support_point_toward(pos12, -dir);
        let p2b = segment2.support_point_toward(pos12, dir);
        let sep_a = (p2a - triangle1.a).dot(dir);
        let sep_b = -(p2b - triangle1.a).dot(dir);

        if sep_a >= sep_b {
            (sep_a, dir)
        } else {
            (sep_b, -dir)
        }
    } else {
        (-Real::MAX, Vector::ZERO)
    }
}

/// Finds the best separating axis by testing edge-edge combinations between a segment and a triangle (3D only).
///
/// In 3D, when a line segment and triangle collide, the contact might occur along an axis
/// perpendicular to both the segment and one of the triangle's edges. This function tests all
/// such axes (cross products) to find the one with maximum separation.
///
/// # Parameters
///
/// - `segment1`: The line segment
/// - `triangle2`: The triangle
/// - `pos12`: The position of the triangle relative to the segment
///
/// # Returns
///
/// A tuple containing:
/// - `Real`: The maximum separation found across all edge-edge axes
///   - **Positive**: Shapes are separated
///   - **Negative**: Shapes are overlapping
/// - `Vector`: The axis direction that gives this separation
///
/// # The Axes Tested
///
/// The function computes cross products between:
/// - The segment's direction (B - A)
/// - Each of the 3 triangle edges (AB, BC, CA)
///
/// This gives 3 base axes. The function tests both each axis and its negation (6 total),
/// finding which gives the maximum separation.
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::{Segment, Triangle};
/// use parry3d::query::sat::segment_triangle_find_local_separating_edge_twoway;
/// use parry3d::math::{Vector, Pose};
///
/// let segment = Segment::new(
///     Vector::ZERO,
///     Vector::new(0.0, 0.0, 2.0)
/// );
///
/// let triangle = Triangle::new(
///     Vector::new(1.0, 0.0, 1.0),
///     Vector::new(3.0, 0.0, 1.0),
///     Vector::new(2.0, 2.0, 1.0)
/// );
///
/// let pos12 = Pose::identity();
///
/// let (separation, axis) = segment_triangle_find_local_separating_edge_twoway(
///     &segment,
///     &triangle,
///     &pos12
/// );
///
/// if separation > 0.0 {
///     println!("Separated by {} along edge-edge axis", separation);
/// }
/// # }
/// ```
///
/// # Implementation Details
///
/// - Axes with near-zero length (parallel edges) are skipped
/// - The function uses [`support_map_support_map_compute_separation`](super::support_map_support_map_compute_separation)
///   to compute the actual separation along each axis
/// - Both positive and negative directions are tested for each cross product
///
/// # Usage in Complete SAT
///
/// For a complete segment-triangle collision test, you must also test:
/// 1. Triangle face normal ([`triangle_segment_find_local_separating_normal_oneway`])
/// 2. Segment-specific axes (depends on whether the segment has associated normals)
/// 3. Edge-edge cross products (this function)
pub fn segment_triangle_find_local_separating_edge_twoway(
    segment1: &Segment,
    triangle2: &Triangle,
    pos12: &Pose,
) -> (Real, Vector) {
    let x2 = pos12.rotation * (triangle2.b - triangle2.a);
    let y2 = pos12.rotation * (triangle2.c - triangle2.b);
    let z2 = pos12.rotation * (triangle2.a - triangle2.c);
    let dir1 = segment1.scaled_direction();

    let crosses1 = [dir1.cross(x2), dir1.cross(y2), dir1.cross(z2)];
    let axes1 = [
        crosses1[0],
        crosses1[1],
        crosses1[2],
        -crosses1[0],
        -crosses1[1],
        -crosses1[2],
    ];
    let mut max_separation = -Real::MAX;
    let mut sep_dir = axes1[0];

    for axis1 in &axes1 {
        if let Some(axis1) = (*axis1).try_normalize() {
            let sep =
                sat::support_map_support_map_compute_separation(segment1, triangle2, pos12, axis1);

            if sep > max_separation {
                max_separation = sep;
                sep_dir = axis1;
            }
        }
    }

    (max_separation, sep_dir)
}

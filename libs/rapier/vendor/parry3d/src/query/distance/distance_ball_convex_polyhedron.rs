use crate::math::{Pose, Real};
use crate::shape::{Ball, Shape};

/// Distance between a ball and a convex polyhedron.
///
/// This function panics if the input shape does not implement
/// both the ConvexPolyhedron and PointQuery traits.
#[inline]
pub fn distance_ball_convex_polyhedron(
    pos12: &Pose,
    ball1: &Ball,
    shape2: &(impl Shape + ?Sized),
) -> Real {
    distance_convex_polyhedron_ball(&pos12.inverse(), shape2, ball1)
}

/// Distance between a convex polyhedron and a ball.
///
/// This function panics if the input shape does not implement
/// both the ConvexPolyhedron and PointQuery traits.
#[inline]
pub fn distance_convex_polyhedron_ball(
    pos12: &Pose,
    shape1: &(impl Shape + ?Sized),
    ball2: &Ball,
) -> Real {
    let center2_1 = pos12.translation;
    let proj = shape1.project_local_point(center2_1, true);
    ((proj.point - center2_1).length() - ball2.radius).max(0.0)
}

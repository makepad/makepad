use crate::math::{Pose, Real};
use crate::query::ClosestPoints;
use crate::shape::{Ball, Shape};

/// ClosestPoints between a ball and a convex polyhedron.
///
/// This function panics if the input shape does not implement
/// both the ConvexPolyhedron and PointQuery traits.
#[inline]
pub fn closest_points_ball_convex_polyhedron(
    pos12: &Pose,
    ball1: &Ball,
    shape2: &(impl Shape + ?Sized),
    prediction: Real,
) -> ClosestPoints {
    match crate::query::details::contact_ball_convex_polyhedron(pos12, ball1, shape2, prediction) {
        Some(contact) => {
            if contact.dist <= 0.0 {
                ClosestPoints::Intersecting
            } else {
                ClosestPoints::WithinMargin(contact.point1, contact.point2)
            }
        }
        None => ClosestPoints::Disjoint,
    }
}

/// ClosestPoints between a convex polyhedron and a ball.
///
/// This function panics if the input shape does not implement
/// both the ConvexPolyhedron and PointQuery traits.
#[inline]
pub fn closest_points_convex_polyhedron_ball(
    pos12: &Pose,
    shape1: &(impl Shape + ?Sized),
    ball2: &Ball,
    prediction: Real,
) -> ClosestPoints {
    match crate::query::details::contact_convex_polyhedron_ball(pos12, shape1, ball2, prediction) {
        Some(contact) => {
            if contact.dist <= 0.0 {
                ClosestPoints::Intersecting
            } else {
                ClosestPoints::WithinMargin(contact.point1, contact.point2)
            }
        }
        None => ClosestPoints::Disjoint,
    }
}

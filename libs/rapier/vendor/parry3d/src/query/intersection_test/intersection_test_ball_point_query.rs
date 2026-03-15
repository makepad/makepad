use crate::math::Pose;
use crate::query::PointQuery;
use crate::shape::Ball;

/// Intersection test between a ball and a shape implementing the `PointQuery` trait.
pub fn intersection_test_ball_point_query<P: ?Sized + PointQuery>(
    pos12: &Pose,
    ball1: &Ball,
    point_query2: &P,
) -> bool {
    intersection_test_point_query_ball(&pos12.inverse(), point_query2, ball1)
}

/// Intersection test between a shape implementing the `PointQuery` trait and a ball.
pub fn intersection_test_point_query_ball<P: ?Sized + PointQuery>(
    pos12: &Pose,
    point_query1: &P,
    ball2: &Ball,
) -> bool {
    let local_p2_1 = pos12.translation;
    let proj = point_query1.project_local_point(local_p2_1, true);
    proj.is_inside || (local_p2_1 - proj.point).length_squared() <= ball2.radius * ball2.radius
}

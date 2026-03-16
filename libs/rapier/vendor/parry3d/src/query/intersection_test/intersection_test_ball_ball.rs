use crate::math::Vector;
use crate::shape::Ball;

/// Intersection test between balls.
#[inline]
pub fn intersection_test_ball_ball(center12: Vector, b1: &Ball, b2: &Ball) -> bool {
    let r1 = b1.radius;
    let r2 = b2.radius;
    let distance_squared = center12.length_squared();
    let sum_radius = r1 + r2;
    distance_squared <= sum_radius * sum_radius
}

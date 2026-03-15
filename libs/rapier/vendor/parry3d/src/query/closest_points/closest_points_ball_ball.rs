use crate::math::{Pose, Real};
use crate::query::ClosestPoints;
use crate::shape::Ball;

/// Closest points between balls.
///
/// Each returned point is expressed on the local-space of the corresponding shape.
#[inline]
pub fn closest_points_ball_ball(pos12: &Pose, b1: &Ball, b2: &Ball, margin: Real) -> ClosestPoints {
    assert!(
        margin >= 0.0,
        "The proximity margin must be positive or null."
    );

    let r1 = b1.radius;
    let r2 = b2.radius;
    let delta_pos = pos12.translation;
    let distance = delta_pos.length();
    let sum_radius = r1 + r2;

    if distance - margin <= sum_radius {
        if distance <= sum_radius {
            ClosestPoints::Intersecting
        } else {
            let normal = delta_pos.normalize();
            let p1 = normal * r1;
            let p2 = pos12.rotation.inverse() * normal * -r2;
            ClosestPoints::WithinMargin(p1, p2)
        }
    } else {
        ClosestPoints::Disjoint
    }
}

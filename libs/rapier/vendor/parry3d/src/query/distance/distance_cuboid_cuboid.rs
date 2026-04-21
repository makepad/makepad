use crate::math::{Pose, Real};
use crate::query::ClosestPoints;
use crate::shape::Cuboid;

/// Distance between two cuboids.
#[inline]
pub fn distance_cuboid_cuboid(pos12: &Pose, cuboid1: &Cuboid, cuboid2: &Cuboid) -> Real {
    match crate::query::details::closest_points_cuboid_cuboid(pos12, cuboid1, cuboid2, Real::MAX) {
        ClosestPoints::WithinMargin(p1, p2) => (p1 - (pos12 * p2)).length(),
        _ => 0.0,
    }
}

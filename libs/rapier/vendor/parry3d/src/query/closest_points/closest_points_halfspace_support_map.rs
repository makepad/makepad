use crate::math::{Pose, Real};
use crate::query::ClosestPoints;
use crate::shape::HalfSpace;
use crate::shape::SupportMap;

/// Closest points between a halfspace and a support-mapped shape (Cuboid, ConvexHull, etc.)
pub fn closest_points_halfspace_support_map<G: ?Sized + SupportMap>(
    pos12: &Pose,
    halfspace: &HalfSpace,
    other: &G,
    margin: Real,
) -> ClosestPoints {
    assert!(
        margin >= 0.0,
        "The proximity margin must be positive or null."
    );

    let deepest = other.support_point(pos12, -halfspace.normal);
    let distance = halfspace.normal.dot(-deepest);

    if distance >= -margin {
        if distance >= 0.0 {
            ClosestPoints::Intersecting
        } else {
            let p1 = deepest + halfspace.normal * distance;
            let p2 = pos12.inverse_transform_point(deepest);
            ClosestPoints::WithinMargin(p1, p2)
        }
    } else {
        ClosestPoints::Disjoint
    }
}

/// Closest points between a support-mapped shape (Cuboid, ConvexHull, etc.) and a halfspace.
pub fn closest_points_support_map_halfspace<G: ?Sized + SupportMap>(
    pos12: &Pose,
    other: &G,
    halfspace: &HalfSpace,
    margin: Real,
) -> ClosestPoints {
    closest_points_halfspace_support_map(&pos12.inverse(), halfspace, other, margin).flipped()
}

use crate::math::Pose;
use crate::shape::HalfSpace;
use crate::shape::SupportMap;

/// Intersection test between a halfspace and a support-mapped shape (Cuboid, ConvexHull, etc.)
pub fn intersection_test_halfspace_support_map<G: ?Sized + SupportMap>(
    pos12: &Pose,
    halfspace: &HalfSpace,
    other: &G,
) -> bool {
    let deepest = other.support_point_toward(pos12, -halfspace.normal);
    halfspace.normal.dot(deepest) <= 0.0
}

/// Intersection test between a support-mapped shape (Cuboid, ConvexHull, etc.) and a halfspace.
pub fn intersection_test_support_map_halfspace<G: ?Sized + SupportMap>(
    pos12: &Pose,
    other: &G,
    halfspace: &HalfSpace,
) -> bool {
    intersection_test_halfspace_support_map(&pos12.inverse(), halfspace, other)
}

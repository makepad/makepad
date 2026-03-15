use crate::math::{Pose, Real};
use crate::shape::HalfSpace;
use crate::shape::SupportMap;

/// Distance between a halfspace and a support-mapped shape.
pub fn distance_halfspace_support_map<G: ?Sized + SupportMap>(
    pos12: &Pose,
    halfspace: &HalfSpace,
    other: &G,
) -> Real {
    let deepest = other.support_point_toward(pos12, -halfspace.normal);
    halfspace.normal.dot(deepest).max(0.0)
}

/// Distance between a support-mapped shape and a halfspace.
pub fn distance_support_map_halfspace<G: ?Sized + SupportMap>(
    pos12: &Pose,
    other: &G,
    halfspace: &HalfSpace,
) -> Real {
    distance_halfspace_support_map(&pos12.inverse(), halfspace, other)
}

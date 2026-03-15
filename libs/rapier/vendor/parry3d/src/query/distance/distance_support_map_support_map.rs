use crate::math::{Pose, Real, Vector};
use crate::query::gjk::{self, CsoPoint, GJKResult, VoronoiSimplex};
use crate::shape::SupportMap;
use num::Bounded;

/// Distance between support-mapped shapes.
pub fn distance_support_map_support_map<G1, G2>(pos12: &Pose, g1: &G1, g2: &G2) -> Real
where
    G1: ?Sized + SupportMap,
    G2: ?Sized + SupportMap,
{
    distance_support_map_support_map_with_params(pos12, g1, g2, &mut VoronoiSimplex::new(), None)
}

/// Distance between support-mapped shapes.
///
/// This allows a more fine grained control other the underlying GJK algorigtm.
pub fn distance_support_map_support_map_with_params<G1, G2>(
    pos12: &Pose,
    g1: &G1,
    g2: &G2,
    simplex: &mut VoronoiSimplex,
    init_dir: Option<Vector>,
) -> Real
where
    G1: ?Sized + SupportMap,
    G2: ?Sized + SupportMap,
{
    // TODO: or m2.translation - m1.translation ?
    let dir = init_dir.unwrap_or_else(|| -pos12.translation);

    let normalized_dir =
        if dir.length_squared() > crate::math::DEFAULT_EPSILON * crate::math::DEFAULT_EPSILON {
            dir.normalize()
        } else {
            Vector::X
        };

    simplex.reset(CsoPoint::from_shapes(pos12, g1, g2, normalized_dir));

    match gjk::closest_points(pos12, g1, g2, Real::max_value(), true, simplex) {
        GJKResult::Intersection => 0.0,
        GJKResult::ClosestPoints(p1, p2, _) => (p1 - p2).length(),
        GJKResult::Proximity(_) => unreachable!(),
        GJKResult::NoIntersection(_) => 0.0, // TODO: GJK did not converge.
    }
}

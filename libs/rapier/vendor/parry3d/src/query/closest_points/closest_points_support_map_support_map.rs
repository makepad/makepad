use crate::math::{Pose, Real, Vector};
use crate::query::gjk::{self, CsoPoint, GJKResult, VoronoiSimplex};
use crate::query::ClosestPoints;
use crate::shape::SupportMap;

/// Closest points between support-mapped shapes (`Cuboid`, `ConvexHull`, etc.)
pub fn closest_points_support_map_support_map<G1, G2>(
    pos12: &Pose,
    g1: &G1,
    g2: &G2,
    prediction: Real,
) -> ClosestPoints
where
    G1: ?Sized + SupportMap,
    G2: ?Sized + SupportMap,
{
    match closest_points_support_map_support_map_with_params(
        pos12,
        g1,
        g2,
        prediction,
        &mut VoronoiSimplex::new(),
        None,
    ) {
        GJKResult::ClosestPoints(pt1, pt2, _) => {
            ClosestPoints::WithinMargin(pt1, pos12.inverse_transform_point(pt2))
        }
        GJKResult::NoIntersection(_) => ClosestPoints::Disjoint,
        GJKResult::Intersection => ClosestPoints::Intersecting,
        GJKResult::Proximity(_) => unreachable!(),
    }
}

/// Closest points between support-mapped shapes (`Cuboid`, `ConvexHull`, etc.)
///
/// This allows a more fine grained control other the underlying GJK algorigtm.
pub fn closest_points_support_map_support_map_with_params<G1, G2>(
    pos12: &Pose,
    g1: &G1,
    g2: &G2,
    prediction: Real,
    simplex: &mut VoronoiSimplex,
    init_dir: Option<Vector>,
) -> GJKResult
where
    G1: ?Sized + SupportMap,
    G2: ?Sized + SupportMap,
{
    let dir = match init_dir {
        // TODO: or pos12.translation (without the minus sign) ?
        None => -pos12.translation,
        Some(dir) => dir,
    };

    if let Some(dir) = (dir).try_normalize() {
        simplex.reset(CsoPoint::from_shapes(pos12, g1, g2, dir));
    } else {
        simplex.reset(CsoPoint::from_shapes(pos12, g1, g2, Vector::X));
    }

    gjk::closest_points(pos12, g1, g2, prediction, true, simplex)
}

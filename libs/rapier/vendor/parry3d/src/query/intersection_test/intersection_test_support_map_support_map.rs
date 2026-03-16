use crate::math::{Pose, Vector};
use crate::query::gjk::{self, CsoPoint, GJKResult, VoronoiSimplex};
use crate::shape::SupportMap;

/// Intersection test between support-mapped shapes (`Cuboid`, `ConvexHull`, etc.)
pub fn intersection_test_support_map_support_map<G1, G2>(pos12: &Pose, g1: &G1, g2: &G2) -> bool
where
    G1: ?Sized + SupportMap,
    G2: ?Sized + SupportMap,
{
    intersection_test_support_map_support_map_with_params(
        pos12,
        g1,
        g2,
        &mut VoronoiSimplex::new(),
        None,
    )
    .0
}

/// Intersection test between support-mapped shapes (`Cuboid`, `ConvexHull`, etc.)
///
/// This allows a more fine grained control other the underlying GJK algorithm.
pub fn intersection_test_support_map_support_map_with_params<G1, G2>(
    pos12: &Pose,
    g1: &G1,
    g2: &G2,
    simplex: &mut VoronoiSimplex,
    init_dir: Option<Vector>,
) -> (bool, Vector)
where
    G1: ?Sized + SupportMap,
    G2: ?Sized + SupportMap,
{
    let dir = if let Some(init_dir) = init_dir {
        init_dir
    } else if let Some(init_dir) = (pos12.translation).try_normalize() {
        init_dir
    } else {
        Vector::X
    };

    simplex.reset(CsoPoint::from_shapes(pos12, g1, g2, dir));

    match gjk::closest_points(pos12, g1, g2, 0.0, false, simplex) {
        GJKResult::Intersection => (true, dir),
        GJKResult::Proximity(dir) => (false, dir),
        GJKResult::NoIntersection(dir) => (false, dir),
        GJKResult::ClosestPoints(..) => unreachable!(),
    }
}

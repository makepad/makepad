use crate::math::Pose;
use crate::query::sat;
use crate::shape::Cuboid;

/// Intersection test between cuboids.
#[inline]
pub fn intersection_test_cuboid_cuboid(pos12: &Pose, cuboid1: &Cuboid, cuboid2: &Cuboid) -> bool {
    let sep1 = sat::cuboid_cuboid_find_local_separating_normal_oneway(cuboid1, cuboid2, pos12).0;

    if sep1 > 0.0 {
        return false;
    }

    let pos21 = pos12.inverse();
    let sep2 = sat::cuboid_cuboid_find_local_separating_normal_oneway(cuboid2, cuboid1, &pos21).0;
    if sep2 > 0.0 {
        return false;
    }

    #[cfg(feature = "dim2")]
    return true; // This case does not exist in 2D.
    #[cfg(feature = "dim3")]
    {
        let sep3 = sat::cuboid_cuboid_find_local_separating_edge_twoway(cuboid1, cuboid2, pos12).0;
        sep3 <= 0.0
    }
}

use crate::bounding_volume::Aabb;
use crate::math::{Pose, Rotation};
use crate::query::sat;
use crate::shape::{Cuboid, Segment};

/// Test if a segment intersects an Aabb.
pub fn intersection_test_aabb_segment(aabb1: &Aabb, segment2: &Segment) -> bool {
    let cuboid1 = Cuboid::new(aabb1.half_extents());
    let pos12 = Pose::from_parts(-aabb1.center(), Rotation::IDENTITY);
    intersection_test_cuboid_segment(&pos12, &cuboid1, segment2)
}

/// Test if a segment intersects a cuboid.
#[inline]
pub fn intersection_test_segment_cuboid(
    pos12: &Pose,
    segment1: &Segment,
    cuboid2: &Cuboid,
) -> bool {
    intersection_test_cuboid_segment(&pos12.inverse(), cuboid2, segment1)
}

/// Test if a segment intersects a cuboid.
#[inline]
pub fn intersection_test_cuboid_segment(pos12: &Pose, cube1: &Cuboid, segment2: &Segment) -> bool {
    let sep1 =
        sat::cuboid_support_map_find_local_separating_normal_oneway(cube1, segment2, pos12).0;
    if sep1 > 0.0 {
        return false;
    }

    // This case does not exist in 3D.
    #[cfg(feature = "dim2")]
    {
        let sep2 = sat::segment_cuboid_find_local_separating_normal_oneway(
            segment2,
            cube1,
            &pos12.inverse(),
        )
        .0;
        if sep2 > 0.0 {
            return false;
        }
    }

    #[cfg(feature = "dim2")]
    return true; // This case does not exist in 2D.
    #[cfg(feature = "dim3")]
    {
        let sep3 = sat::cuboid_segment_find_local_separating_edge_twoway(cube1, segment2, pos12).0;
        sep3 <= 0.0
    }
}

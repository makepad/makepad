use crate::bounding_volume::Aabb;
use crate::math::{Pose, Rotation};
use crate::query::sat;
use crate::shape::{Cuboid, Triangle};

/// Tests if a triangle intersects an Aabb.
pub fn intersection_test_aabb_triangle(aabb1: &Aabb, triangle2: &Triangle) -> bool {
    let cuboid1 = Cuboid::new(aabb1.half_extents());
    let pos12 = Pose::from_parts(-aabb1.center(), Rotation::IDENTITY);
    intersection_test_cuboid_triangle(&pos12, &cuboid1, triangle2)
}

/// Tests if a triangle intersects a cuboid.
#[inline]
pub fn intersection_test_triangle_cuboid(
    pos12: &Pose,
    triangle1: &Triangle,
    cuboid2: &Cuboid,
) -> bool {
    intersection_test_cuboid_triangle(&pos12.inverse(), cuboid2, triangle1)
}

/// Tests if a triangle intersects an cuboid.
#[inline]
pub fn intersection_test_cuboid_triangle(
    pos12: &Pose,
    cube1: &Cuboid,
    triangle2: &Triangle,
) -> bool {
    let sep1 =
        sat::cuboid_support_map_find_local_separating_normal_oneway(cube1, triangle2, pos12).0;
    if sep1 > 0.0 {
        return false;
    }

    let pos21 = pos12.inverse();
    let sep2 = sat::triangle_cuboid_find_local_separating_normal_oneway(triangle2, cube1, &pos21).0;
    if sep2 > 0.0 {
        return false;
    }

    #[cfg(feature = "dim2")]
    return true; // This case does not exist in 2D.
    #[cfg(feature = "dim3")]
    {
        let sep3 =
            sat::cuboid_triangle_find_local_separating_edge_twoway(cube1, triangle2, pos12).0;
        sep3 <= 0.0
    }
}

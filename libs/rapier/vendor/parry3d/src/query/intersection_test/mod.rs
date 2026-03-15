//! Implementation details of the `intersection_test` function.

pub use self::intersection_test::intersection_test;
pub use self::intersection_test_ball_ball::intersection_test_ball_ball;
pub use self::intersection_test_ball_point_query::{
    intersection_test_ball_point_query, intersection_test_point_query_ball,
};
pub use self::intersection_test_cuboid_cuboid::intersection_test_cuboid_cuboid;
pub use self::intersection_test_cuboid_segment::{
    intersection_test_aabb_segment, intersection_test_cuboid_segment,
    intersection_test_segment_cuboid,
};
pub use self::intersection_test_cuboid_triangle::{
    intersection_test_aabb_triangle, intersection_test_cuboid_triangle,
    intersection_test_triangle_cuboid,
};
pub use self::intersection_test_halfspace_support_map::{
    intersection_test_halfspace_support_map, intersection_test_support_map_halfspace,
};
pub use self::intersection_test_support_map_support_map::intersection_test_support_map_support_map;
pub use self::intersection_test_support_map_support_map::intersection_test_support_map_support_map_with_params;
#[cfg(feature = "alloc")]
pub use self::{
    intersection_test_composite_shape_shape::{
        intersection_test_composite_shape_shape, intersection_test_shape_composite_shape,
    },
    intersection_test_voxels_shape::{
        intersection_test_shape_voxels, intersection_test_voxels_shape,
        intersection_test_voxels_shape_shapes,
    },
};

mod intersection_test;
mod intersection_test_ball_ball;
mod intersection_test_ball_point_query;
#[cfg(feature = "alloc")]
mod intersection_test_composite_shape_shape;
mod intersection_test_cuboid_cuboid;
mod intersection_test_cuboid_segment;
mod intersection_test_cuboid_triangle;
mod intersection_test_halfspace_support_map;
mod intersection_test_support_map_support_map;

#[cfg(feature = "alloc")]
mod intersection_test_voxels_shape;

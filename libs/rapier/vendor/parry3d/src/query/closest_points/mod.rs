//! Implementation details of the `closest_points` function.

pub use self::closest_points::ClosestPoints;
pub use self::closest_points_ball_ball::closest_points_ball_ball;
pub use self::closest_points_ball_convex_polyhedron::{
    closest_points_ball_convex_polyhedron, closest_points_convex_polyhedron_ball,
};
#[cfg(feature = "alloc")]
pub use self::closest_points_composite_shape_shape::{
    closest_points_composite_shape_shape, closest_points_shape_composite_shape,
};
pub use self::closest_points_cuboid_cuboid::closest_points_cuboid_cuboid;
pub use self::closest_points_cuboid_triangle::{
    closest_points_cuboid_triangle, closest_points_triangle_cuboid,
};
pub use self::closest_points_halfspace_support_map::{
    closest_points_halfspace_support_map, closest_points_support_map_halfspace,
};
pub use self::closest_points_line_line::{
    closest_points_line_line, closest_points_line_line_parameters,
    closest_points_line_line_parameters_eps,
};
pub use self::closest_points_segment_segment::{
    closest_points_segment_segment, closest_points_segment_segment_2d,
    closest_points_segment_segment_with_locations,
    closest_points_segment_segment_with_locations_nD,
};
pub use self::closest_points_shape_shape::closest_points;
pub use self::closest_points_support_map_support_map::closest_points_support_map_support_map;
pub use self::closest_points_support_map_support_map::closest_points_support_map_support_map_with_params;

mod closest_points;
mod closest_points_ball_ball;
mod closest_points_ball_convex_polyhedron;
#[cfg(feature = "alloc")]
mod closest_points_composite_shape_shape;
mod closest_points_cuboid_cuboid;
mod closest_points_cuboid_triangle;
mod closest_points_halfspace_support_map;
mod closest_points_line_line;
mod closest_points_segment_segment;
mod closest_points_shape_shape;
mod closest_points_support_map_support_map;

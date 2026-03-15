//! Implementation details of the `distance` function.

pub use self::distance::distance;
pub use self::distance_ball_ball::distance_ball_ball;
pub use self::distance_ball_convex_polyhedron::{
    distance_ball_convex_polyhedron, distance_convex_polyhedron_ball,
};
#[cfg(feature = "alloc")]
pub use self::distance_composite_shape_shape::{
    distance_composite_shape_shape, distance_shape_composite_shape,
};
pub use self::distance_cuboid_cuboid::distance_cuboid_cuboid;
pub use self::distance_halfspace_support_map::{
    distance_halfspace_support_map, distance_support_map_halfspace,
};
pub use self::distance_segment_segment::distance_segment_segment;
pub use self::distance_support_map_support_map::{
    distance_support_map_support_map, distance_support_map_support_map_with_params,
};

mod distance;
mod distance_ball_ball;
mod distance_ball_convex_polyhedron;
#[cfg(feature = "alloc")]
mod distance_composite_shape_shape;
mod distance_cuboid_cuboid;
mod distance_halfspace_support_map;
mod distance_segment_segment;
mod distance_support_map_support_map;

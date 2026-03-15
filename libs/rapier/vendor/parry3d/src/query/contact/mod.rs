//! Implementation details of the `contact` and `contacts` functions.

pub use self::contact::Contact;
pub use self::contact_ball_ball::contact_ball_ball;
pub use self::contact_ball_convex_polyhedron::{
    contact_ball_convex_polyhedron, contact_convex_polyhedron_ball,
};
#[cfg(feature = "alloc")]
pub use self::contact_composite_shape_shape::{
    contact_composite_shape_shape, contact_shape_composite_shape,
};
pub use self::contact_cuboid_cuboid::contact_cuboid_cuboid;
pub use self::contact_halfspace_support_map::{
    contact_halfspace_support_map, contact_support_map_halfspace,
};
pub use self::contact_shape_shape::contact;
#[cfg(feature = "alloc")]
pub use self::contact_support_map_support_map::{
    contact_support_map_support_map, contact_support_map_support_map_with_params,
};

mod contact;
mod contact_ball_ball;
mod contact_ball_convex_polyhedron;
#[cfg(feature = "alloc")]
mod contact_composite_shape_shape;
mod contact_cuboid_cuboid;
mod contact_halfspace_support_map;
mod contact_shape_shape;
#[cfg(feature = "alloc")]
mod contact_support_map_support_map;

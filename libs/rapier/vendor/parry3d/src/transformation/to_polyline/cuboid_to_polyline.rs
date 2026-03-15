use crate::math::Vector2;
use crate::shape::Cuboid;
use crate::transformation::utils;
use alloc::{vec, vec::Vec};

impl Cuboid {
    /// Discretize the boundary of this cuboid as a polygonal line.
    pub fn to_polyline(&self) -> Vec<Vector2> {
        utils::scaled(unit_rectangle(), self.half_extents * 2.0)
    }
}

/// The contour of a unit cuboid lying on the x-y plane.
fn unit_rectangle() -> Vec<Vector2> {
    vec![
        Vector2::new(-0.5, -0.5),
        Vector2::new(0.5, -0.5),
        Vector2::new(0.5, 0.5),
        Vector2::new(-0.5, 0.5),
    ]
}

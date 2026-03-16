pub use self::mesh_intersection::{
    intersect_meshes, intersect_meshes_with_tolerances, MeshIntersectionTolerances,
};
pub use self::mesh_intersection_error::MeshIntersectionError;
use triangle_triangle_intersection::*;

use crate::math::Real;

mod mesh_intersection;
mod mesh_intersection_error;
mod triangle_triangle_intersection;

const EPS: Real = 1.0e-6;

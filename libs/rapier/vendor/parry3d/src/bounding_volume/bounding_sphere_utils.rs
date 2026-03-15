use crate::math::{ComplexField, Real, Vector};
use crate::utils;

use super::BoundingSphere;

/// Computes the bounding sphere of a set of point, given its center.
#[inline]
pub fn point_cloud_bounding_sphere_with_center(pts: &[Vector], center: Vector) -> BoundingSphere {
    let mut sqradius = 0.0;

    for pt in pts.iter() {
        let dist_sq = pt.distance_squared(center);

        if dist_sq > sqradius {
            sqradius = dist_sq
        }
    }
    BoundingSphere::new(center, <Real as ComplexField>::sqrt(sqradius))
}

/// Computes a bounding sphere of the specified set of point.
#[inline]
pub fn point_cloud_bounding_sphere(pts: &[Vector]) -> BoundingSphere {
    point_cloud_bounding_sphere_with_center(pts, utils::center(pts))
}

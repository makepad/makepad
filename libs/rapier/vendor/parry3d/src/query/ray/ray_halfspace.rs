use crate::math::{Real, Vector};
use crate::query::{Ray, RayCast, RayIntersection};
use crate::shape::{FeatureId, HalfSpace};

/// Computes the time_of_impact of an unbounded line with a halfspace described by its center and normal.
#[inline]
pub fn line_toi_with_halfspace(
    halfspace_center: Vector,
    halfspace_normal: Vector,
    line_origin: Vector,
    line_dir: Vector,
) -> Option<Real> {
    let dpos = halfspace_center - line_origin;
    let denom = halfspace_normal.dot(line_dir);

    if relative_eq!(denom, 0.0) {
        None
    } else {
        Some(halfspace_normal.dot(dpos) / denom)
    }
}

/// Computes the time_of_impact of a ray with a halfspace described by its center and normal.
#[inline]
pub fn ray_toi_with_halfspace(center: Vector, normal: Vector, ray: &Ray) -> Option<Real> {
    if let Some(t) = line_toi_with_halfspace(center, normal, ray.origin, ray.dir) {
        if t >= 0.0 {
            return Some(t);
        }
    }

    None
}

impl RayCast for HalfSpace {
    #[inline]
    fn cast_local_ray_and_get_normal(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        solid: bool,
    ) -> Option<RayIntersection> {
        let dpos = -ray.origin;

        let dot_normal_dpos = self.normal.dot(dpos);

        if solid && dot_normal_dpos > 0.0 {
            // The ray is inside of the solid half-space.
            return Some(RayIntersection::new(0.0, Vector::ZERO, FeatureId::Face(0)));
        }

        let t = dot_normal_dpos / self.normal.dot(ray.dir);

        if t >= 0.0 && t <= max_time_of_impact {
            let n = if dot_normal_dpos > 0.0 {
                -self.normal
            } else {
                self.normal
            };

            Some(RayIntersection::new(t, n, FeatureId::Face(0)))
        } else {
            None
        }
    }
}

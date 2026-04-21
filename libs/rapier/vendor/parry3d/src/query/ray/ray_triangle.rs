use crate::math::Real;
#[cfg(feature = "dim2")]
use crate::math::Vector;
#[cfg(feature = "dim3")]
use crate::math::Vector;
use crate::query::{Ray, RayCast, RayIntersection};
use crate::shape::{FeatureId, Triangle};

impl RayCast for Triangle {
    #[inline]
    #[cfg(feature = "dim2")]
    fn cast_local_ray_and_get_normal(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        solid: bool,
    ) -> Option<RayIntersection> {
        let edges = self.edges();

        if solid {
            // Check if ray starts in triangle
            let perp_sign1 = edges[0]
                .scaled_direction()
                .perp_dot(ray.origin - edges[0].a)
                > 0.0;
            let perp_sign2 = edges[1]
                .scaled_direction()
                .perp_dot(ray.origin - edges[1].a)
                > 0.0;
            let perp_sign3 = edges[2]
                .scaled_direction()
                .perp_dot(ray.origin - edges[2].a)
                > 0.0;

            if perp_sign1 == perp_sign2 && perp_sign1 == perp_sign3 {
                return Some(RayIntersection::new(0.0, Vector::Y, FeatureId::Face(0)));
            }
        }

        let mut best = None;
        let mut smallest_toi = Real::MAX;

        for edge in &edges {
            if let Some(inter) = edge.cast_local_ray_and_get_normal(ray, max_time_of_impact, solid)
            {
                if inter.time_of_impact < smallest_toi {
                    smallest_toi = inter.time_of_impact;
                    best = Some(inter);
                }
            }
        }

        best
    }

    #[inline]
    #[cfg(feature = "dim3")]
    fn cast_local_ray_and_get_normal(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        _: bool,
    ) -> Option<RayIntersection> {
        let inter = local_ray_intersection_with_triangle(self.a, self.b, self.c, ray)?.0;

        if inter.time_of_impact <= max_time_of_impact {
            Some(inter)
        } else {
            None
        }
    }
}

/// Computes the intersection between a triangle and a ray.
///
/// If an intersection is found, the time of impact, the normal and the barycentric coordinates of
/// the intersection point are returned.
#[cfg(feature = "dim3")]
pub fn local_ray_intersection_with_triangle(
    a: Vector,
    b: Vector,
    c: Vector,
    ray: &Ray,
) -> Option<(RayIntersection, Vector)> {
    let ab = b - a;
    let ac = c - a;

    // normal
    let n = ab.cross(ac);
    let d = n.dot(ray.dir);

    // the normal and the ray direction are parallel
    if d == 0.0 {
        return None;
    }

    let ap = ray.origin - a;
    let t = ap.dot(n);

    // the ray does not intersect the halfspace defined by the triangle
    if (t < 0.0 && d < 0.0) || (t > 0.0 && d > 0.0) {
        return None;
    }

    let fid = if d < 0.0 { 0 } else { 1 };

    let d = d.abs();

    //
    // intersection: compute barycentric coordinates
    //
    let e = -ray.dir.cross(ap);

    let mut v;
    let mut w;
    let time_of_impact;
    let normal;

    if t < 0.0 {
        v = -ac.dot(e);

        if v < 0.0 || v > d {
            return None;
        }

        w = ab.dot(e);

        if w < 0.0 || v + w > d {
            return None;
        }

        let invd = 1.0 / d;
        time_of_impact = -t * invd;
        normal = -n.normalize();
        v *= invd;
        w *= invd;
    } else {
        v = ac.dot(e);

        if v < 0.0 || v > d {
            return None;
        }

        w = -ab.dot(e);

        if w < 0.0 || v + w > d {
            return None;
        }

        let invd = 1.0 / d;
        time_of_impact = t * invd;
        normal = n.normalize();
        v *= invd;
        w *= invd;
    }

    Some((
        RayIntersection::new(time_of_impact, normal, FeatureId::Face(fid)),
        Vector::new(-v - w + 1.0, v, w),
    ))
}

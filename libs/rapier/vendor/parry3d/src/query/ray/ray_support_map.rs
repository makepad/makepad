use crate::math::Real;
#[cfg(feature = "dim2")]
use crate::query;
use crate::query::gjk::{self, CsoPoint, VoronoiSimplex};
use crate::query::{Ray, RayCast, RayIntersection};
#[cfg(all(feature = "alloc", feature = "dim2"))]
use crate::shape::ConvexPolygon;
#[cfg(all(feature = "alloc", feature = "dim3"))]
use crate::shape::ConvexPolyhedron;
use crate::shape::{Capsule, FeatureId, Segment, SupportMap};
#[cfg(feature = "dim3")]
use crate::shape::{Cone, Cylinder};

use num::Zero;

/// Cast a ray on a shape using the GJK algorithm.
pub fn local_ray_intersection_with_support_map_with_params<G: ?Sized + SupportMap>(
    shape: &G,
    simplex: &mut VoronoiSimplex,
    ray: &Ray,
    max_time_of_impact: Real,
    solid: bool,
) -> Option<RayIntersection> {
    let supp = shape.local_support_point(-ray.dir);
    simplex.reset(CsoPoint::single_point(supp - ray.origin));

    let inter = gjk::cast_local_ray(shape, simplex, ray, max_time_of_impact);

    if !solid {
        inter.and_then(|(time_of_impact, normal)| {
            if time_of_impact.is_zero() {
                // the ray is inside of the shape.
                let ndir = ray.dir.normalize();
                let supp = shape.local_support_point(ndir);
                let eps: Real = 0.001;
                let shift = (supp - ray.origin).dot(ndir) + eps;
                let new_ray = Ray::new(ray.origin + ndir * shift, -ray.dir);

                // TODO: replace by? : simplex.translate_by(&(ray.origin - new_ray.origin));
                simplex.reset(CsoPoint::single_point(supp - new_ray.origin));

                gjk::cast_local_ray(shape, simplex, &new_ray, shift + eps).and_then(
                    |(time_of_impact, outward_normal)| {
                        let time_of_impact = shift - time_of_impact;
                        if time_of_impact <= max_time_of_impact {
                            Some(RayIntersection::new(
                                time_of_impact,
                                -outward_normal,
                                FeatureId::Unknown,
                            ))
                        } else {
                            None
                        }
                    },
                )
            } else {
                Some(RayIntersection::new(
                    time_of_impact,
                    normal,
                    FeatureId::Unknown,
                ))
            }
        })
    } else {
        inter.map(|(time_of_impact, normal)| {
            RayIntersection::new(time_of_impact, normal, FeatureId::Unknown)
        })
    }
}

#[cfg(feature = "dim3")]
impl RayCast for Cylinder {
    fn cast_local_ray_and_get_normal(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        solid: bool,
    ) -> Option<RayIntersection> {
        local_ray_intersection_with_support_map_with_params(
            self,
            &mut VoronoiSimplex::new(),
            ray,
            max_time_of_impact,
            solid,
        )
    }
}

#[cfg(feature = "dim3")]
impl RayCast for Cone {
    fn cast_local_ray_and_get_normal(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        solid: bool,
    ) -> Option<RayIntersection> {
        local_ray_intersection_with_support_map_with_params(
            self,
            &mut VoronoiSimplex::new(),
            ray,
            max_time_of_impact,
            solid,
        )
    }
}

impl RayCast for Capsule {
    fn cast_local_ray_and_get_normal(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        solid: bool,
    ) -> Option<RayIntersection> {
        local_ray_intersection_with_support_map_with_params(
            self,
            &mut VoronoiSimplex::new(),
            ray,
            max_time_of_impact,
            solid,
        )
    }
}

#[cfg(feature = "dim3")]
#[cfg(feature = "alloc")]
impl RayCast for ConvexPolyhedron {
    fn cast_local_ray_and_get_normal(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        solid: bool,
    ) -> Option<RayIntersection> {
        local_ray_intersection_with_support_map_with_params(
            self,
            &mut VoronoiSimplex::new(),
            ray,
            max_time_of_impact,
            solid,
        )
    }
}

#[cfg(feature = "dim2")]
#[cfg(feature = "alloc")]
impl RayCast for ConvexPolygon {
    fn cast_local_ray_and_get_normal(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        solid: bool,
    ) -> Option<RayIntersection> {
        local_ray_intersection_with_support_map_with_params(
            self,
            &mut VoronoiSimplex::new(),
            ray,
            max_time_of_impact,
            solid,
        )
    }
}

#[allow(unused_variables)]
impl RayCast for Segment {
    fn cast_local_ray_and_get_normal(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        solid: bool,
    ) -> Option<RayIntersection> {
        #[cfg(feature = "dim2")]
        {
            use crate::math::Vector;

            let seg_dir = self.scaled_direction();
            let (s, t, parallel) = query::details::closest_points_line_line_parameters_eps(
                ray.origin,
                ray.dir,
                self.a,
                seg_dir,
                crate::math::DEFAULT_EPSILON,
            );

            if parallel {
                // The lines are parallel, we have to distinguish
                // the case where there is no intersection at all
                // from the case where the line are collinear.
                let dpos = self.a - ray.origin;
                let normal = self.normal().unwrap_or(Vector::ZERO);

                if dpos.dot(normal).abs() < crate::math::DEFAULT_EPSILON {
                    // The rays and the segment are collinear.
                    let dist1 = dpos.dot(ray.dir);
                    let dist2 = dist1 + seg_dir.dot(ray.dir);

                    match (dist1 >= 0.0, dist2 >= 0.0) {
                        (true, true) => {
                            let time_of_impact = dist1.min(dist2) / ray.dir.length_squared();
                            if time_of_impact > max_time_of_impact {
                                None
                            } else if dist1 <= dist2 {
                                Some(RayIntersection::new(
                                    time_of_impact,
                                    normal,
                                    FeatureId::Vertex(0),
                                ))
                            } else {
                                Some(RayIntersection::new(
                                    dist2 / ray.dir.length_squared(),
                                    normal,
                                    FeatureId::Vertex(1),
                                ))
                            }
                        }
                        (true, false) | (false, true) => {
                            // The ray origin lies on the segment.
                            Some(RayIntersection::new(0.0, normal, FeatureId::Face(0)))
                        }
                        (false, false) => {
                            // The segment is behind the ray.
                            None
                        }
                    }
                } else {
                    // The rays never intersect.
                    None
                }
            } else if s >= 0.0 && s <= max_time_of_impact && t >= 0.0 && t <= 1.0 {
                let normal = self.normal().unwrap_or(Vector::ZERO);

                let dot = normal.dot(ray.dir);
                if dot > 0.0 {
                    Some(RayIntersection::new(s, -normal, FeatureId::Face(1)))
                } else if dot < 0.0 {
                    Some(RayIntersection::new(s, normal, FeatureId::Face(0)))
                } else {
                    // dot == 0 happens when lines are parallel, which is normally handled before,
                    // but this may happen if segment is zero length, as the ray is not considered parallel.
                    None
                }
            } else {
                // The closest points are outside of
                // the ray or segment bounds.
                None
            }
        }
        #[cfg(feature = "dim3")]
        {
            // XXX: implement an analytic solution for 3D too.
            local_ray_intersection_with_support_map_with_params(
                self,
                &mut VoronoiSimplex::new(),
                ray,
                max_time_of_impact,
                solid,
            )
        }
    }
}

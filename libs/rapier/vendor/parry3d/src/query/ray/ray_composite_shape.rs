use crate::math::Real;
use crate::partitioning::BvhNode;
use crate::query::{Ray, RayCast, RayIntersection};
use crate::shape::{CompositeShapeRef, Compound, Polyline, TypedCompositeShape};

impl<S: TypedCompositeShape> CompositeShapeRef<'_, S> {
    /// Casts a ray on this composite shape.
    ///
    /// The ray is effectively limited to a segment that starts at `Ray::origin` and ends at
    /// `Ray::origin + Ray::direction * max_time_of_impact`. Set `max_time_of_impact` to `Real::MAX`
    /// for an unbounded ray.
    ///
    /// If `solid` is `false`, then the sub-shapes of `self` are seen as hollow and the ray won’t
    /// immediately stop until it reaches a boundary even if it started inside a shape.
    ///
    /// Returns the ray’s time of impact and the index of the sub-shape of `self` that was hit.
    /// The hit point can be retrieved with `ray.point_at(t)` where `t` is the value returned
    /// by this function.
    #[inline]
    pub fn cast_local_ray(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        solid: bool,
    ) -> Option<(u32, Real)> {
        let hit = self
            .0
            .bvh()
            .cast_ray(ray, max_time_of_impact, |primitive, best_so_far| {
                self.0.map_typed_part_at(primitive, |pose, part, _| {
                    if let Some(pose) = pose {
                        part.cast_ray(pose, ray, best_so_far, solid)
                    } else {
                        part.cast_local_ray(ray, best_so_far, solid)
                    }
                })?
            })?;
        (hit.1 < max_time_of_impact).then_some(hit)
    }

    /// Same as [`Self::cast_local_ray`] but also computes the normal at the hit location.
    #[inline]
    pub fn cast_local_ray_and_get_normal(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        solid: bool,
    ) -> Option<(u32, RayIntersection)> {
        self.0.bvh().find_best(
            max_time_of_impact,
            |node: &BvhNode, best_so_far| node.cast_ray(ray, best_so_far),
            |primitive, best_so_far| {
                self.0.map_typed_part_at(primitive, |pose, part, _| {
                    if let Some(pose) = pose {
                        part.cast_ray_and_get_normal(pose, ray, best_so_far, solid)
                    } else {
                        part.cast_local_ray_and_get_normal(ray, best_so_far, solid)
                    }
                })?
            },
        )
    }
}

impl RayCast for Polyline {
    #[inline]
    fn cast_local_ray(&self, ray: &Ray, max_time_of_impact: Real, solid: bool) -> Option<Real> {
        CompositeShapeRef(self)
            .cast_local_ray(ray, max_time_of_impact, solid)
            .map(|hit| hit.1)
    }

    #[inline]
    fn cast_local_ray_and_get_normal(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        solid: bool,
    ) -> Option<RayIntersection> {
        CompositeShapeRef(self)
            .cast_local_ray_and_get_normal(ray, max_time_of_impact, solid)
            .map(|hit| hit.1)
    }
}

impl RayCast for Compound {
    #[inline]
    fn cast_local_ray(&self, ray: &Ray, max_time_of_impact: Real, solid: bool) -> Option<Real> {
        CompositeShapeRef(self)
            .cast_local_ray(ray, max_time_of_impact, solid)
            .map(|hit| hit.1)
    }

    #[inline]
    fn cast_local_ray_and_get_normal(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        solid: bool,
    ) -> Option<RayIntersection> {
        CompositeShapeRef(self)
            .cast_local_ray_and_get_normal(ray, max_time_of_impact, solid)
            .map(|hit| hit.1)
    }
}

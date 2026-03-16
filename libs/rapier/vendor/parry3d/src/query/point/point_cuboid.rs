use crate::bounding_volume::Aabb;
use crate::math::{Real, Vector};
use crate::query::{PointProjection, PointQuery};
use crate::shape::{Cuboid, FeatureId};

impl PointQuery for Cuboid {
    #[inline]
    fn project_local_point(&self, pt: Vector, solid: bool) -> PointProjection {
        let dl = -self.half_extents;
        let ur = self.half_extents;
        Aabb::new(dl, ur).project_local_point(pt, solid)
    }

    #[inline]
    fn project_local_point_and_get_feature(&self, pt: Vector) -> (PointProjection, FeatureId) {
        let dl = -self.half_extents;
        let ur = self.half_extents;
        Aabb::new(dl, ur).project_local_point_and_get_feature(pt)
    }

    #[inline]
    fn distance_to_local_point(&self, pt: Vector, solid: bool) -> Real {
        let dl = -self.half_extents;
        let ur = self.half_extents;
        Aabb::new(dl, ur).distance_to_local_point(pt, solid)
    }

    #[inline]
    fn contains_local_point(&self, pt: Vector) -> bool {
        let dl = -self.half_extents;
        let ur = self.half_extents;
        Aabb::new(dl, ur).contains_local_point(pt)
    }
}

use crate::bounding_volume::Aabb;
use crate::math::Pose;
use crate::shape::ConvexPolygon;

impl ConvexPolygon {
    /// Computes the world-space [`Aabb`] of this convex polygon, transformed by `pos`.
    #[inline]
    pub fn aabb(&self, pos: &Pose) -> Aabb {
        super::details::point_cloud_aabb_ref(pos, self.points())
    }

    /// Computes the local-space [`Aabb`] of this convex polygon.
    #[inline]
    pub fn local_aabb(&self) -> Aabb {
        super::details::local_point_cloud_aabb_ref(self.points())
    }
}

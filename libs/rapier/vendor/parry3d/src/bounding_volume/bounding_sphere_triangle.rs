use crate::bounding_volume;
use crate::bounding_volume::BoundingSphere;
use crate::math::Pose;
use crate::shape::Triangle;

impl Triangle {
    /// Computes the world-space bounding sphere of this triangle, transformed by `pos`.
    #[inline]
    pub fn bounding_sphere(&self, pos: &Pose) -> BoundingSphere {
        let bv: BoundingSphere = self.local_bounding_sphere();
        bv.transform_by(pos)
    }

    /// Computes the local-space bounding sphere of this triangle.
    #[inline]
    pub fn local_bounding_sphere(&self) -> BoundingSphere {
        let pts = [self.a, self.b, self.c];
        bounding_volume::details::point_cloud_bounding_sphere(&pts[..])
    }
}

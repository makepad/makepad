use crate::bounding_volume::BoundingSphere;
use crate::math::{Pose, Vector};
use crate::shape::Ball;

impl Ball {
    /// Computes the world-space bounding sphere of this ball, transformed by `pos`.
    #[inline]
    pub fn bounding_sphere(&self, pos: &Pose) -> BoundingSphere {
        let bv: BoundingSphere = self.local_bounding_sphere();
        bv.transform_by(pos)
    }

    /// Computes the local-space Aabb of this ball.
    #[inline]
    pub fn local_bounding_sphere(&self) -> BoundingSphere {
        BoundingSphere::new(Vector::ZERO, self.radius)
    }
}

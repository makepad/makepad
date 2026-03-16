use crate::bounding_volume::BoundingSphere;
use crate::math::{Pose, Real, Vector};
use crate::shape::HalfSpace;

use num::Bounded;

impl HalfSpace {
    /// Computes the world-space bounding sphere of this half-space, transformed by `pos`.
    #[inline]
    pub fn bounding_sphere(&self, pos: &Pose) -> BoundingSphere {
        let bv: BoundingSphere = self.local_bounding_sphere();
        bv.transform_by(pos)
    }

    /// Computes the local-space bounding sphere of this half-space.
    #[inline]
    pub fn local_bounding_sphere(&self) -> BoundingSphere {
        let radius = Real::max_value();

        BoundingSphere::new(Vector::ZERO, radius)
    }
}

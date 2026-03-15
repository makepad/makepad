use crate::bounding_volume::BoundingSphere;
use crate::math::{ComplexField, Pose, Real, Vector};
use crate::shape::Cylinder;

impl Cylinder {
    /// Computes the world-space bounding sphere of this cylinder, transformed by `pos`.
    #[inline]
    pub fn bounding_sphere(&self, pos: &Pose) -> BoundingSphere {
        let bv: BoundingSphere = self.local_bounding_sphere();
        bv.transform_by(pos)
    }

    /// Computes the local-space bounding sphere of this cylinder.
    #[inline]
    pub fn local_bounding_sphere(&self) -> BoundingSphere {
        let radius = <Real as ComplexField>::sqrt(
            self.radius * self.radius + self.half_height * self.half_height,
        );

        BoundingSphere::new(Vector::ZERO, radius)
    }
}

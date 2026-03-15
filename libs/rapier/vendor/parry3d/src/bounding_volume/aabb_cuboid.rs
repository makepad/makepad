use crate::bounding_volume::Aabb;
use crate::math::Pose;
use crate::shape::Cuboid;
use crate::utils::PoseOps;

impl Cuboid {
    /// Computes the world-space [`Aabb`] of this cuboid, transformed by `pos`.
    #[inline]
    pub fn aabb(&self, pos: &Pose) -> Aabb {
        let center = pos.translation;
        let ws_half_extents = pos.absolute_transform_vector(self.half_extents);

        Aabb::from_half_extents(center, ws_half_extents)
    }

    /// Computes the local-space [`Aabb`] of this cuboid.
    #[inline]
    pub fn local_aabb(&self) -> Aabb {
        Aabb::new(-self.half_extents, self.half_extents)
    }
}

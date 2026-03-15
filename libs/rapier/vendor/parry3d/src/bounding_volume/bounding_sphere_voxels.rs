use crate::bounding_volume::BoundingSphere;
use crate::math::Pose;
use crate::shape::Voxels;

impl Voxels {
    /// Computes the world-space bounding sphere of this set of voxels, transformed by `pos`.
    #[inline]
    pub fn bounding_sphere(&self, pos: &Pose) -> BoundingSphere {
        self.local_aabb().bounding_sphere().transform_by(pos)
    }

    /// Computes the local-space bounding sphere of this set of voxels.
    #[inline]
    pub fn local_bounding_sphere(&self) -> BoundingSphere {
        self.local_aabb().bounding_sphere()
    }
}

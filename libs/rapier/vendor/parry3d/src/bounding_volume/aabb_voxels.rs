use crate::bounding_volume::Aabb;
use crate::math::Pose;
use crate::shape::Voxels;

impl Voxels {
    /// Computes the world-space Aabb of this set of voxels, transformed by `pos`.
    #[inline]
    pub fn aabb(&self, pos: &Pose) -> Aabb {
        self.chunk_bvh().root_aabb().transform_by(pos)
    }

    /// Computes the local-space Aabb of this set of voxels.
    #[inline]
    pub fn local_aabb(&self) -> Aabb {
        self.chunk_bvh().root_aabb()
    }
}

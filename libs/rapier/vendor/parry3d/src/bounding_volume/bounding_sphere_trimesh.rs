use crate::bounding_volume::BoundingSphere;
use crate::math::Pose;
use crate::shape::TriMesh;

impl TriMesh {
    /// Computes the world-space bounding sphere of this triangle mesh, transformed by `pos`.
    #[inline]
    pub fn bounding_sphere(&self, pos: &Pose) -> BoundingSphere {
        self.local_aabb().bounding_sphere().transform_by(pos)
    }

    /// Computes the local-space bounding sphere of this triangle mesh.
    #[inline]
    pub fn local_bounding_sphere(&self) -> BoundingSphere {
        self.local_aabb().bounding_sphere()
    }
}

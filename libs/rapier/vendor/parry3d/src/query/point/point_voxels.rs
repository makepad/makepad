use crate::math::{Real, Vector};
use crate::query::{PointProjection, PointQuery};
use crate::shape::{Cuboid, FeatureId, Voxels, VoxelsChunkRef};

impl PointQuery for Voxels {
    #[inline]
    fn project_local_point(&self, pt: Vector, solid: bool) -> PointProjection {
        self.chunk_bvh()
            .project_point(pt, Real::MAX, |chunk_id, _| {
                let chunk = self.chunk_ref(chunk_id);
                chunk
                    .project_local_point_and_get_vox_id(pt, solid)
                    .map(|(proj, _)| proj)
            })
            .map(|res| res.1 .1)
            .unwrap_or(PointProjection::new(false, Vector::splat(Real::MAX)))
    }

    #[inline]
    fn project_local_point_and_get_feature(&self, pt: Vector) -> (PointProjection, FeatureId) {
        self.chunk_bvh()
            .project_point_and_get_feature(pt, Real::MAX, |chunk_id, _| {
                let chunk = self.chunk_ref(chunk_id);
                // TODO: we need a way to return both the voxel id, and the feature on the voxel.
                chunk
                    .project_local_point_and_get_vox_id(pt, false)
                    .map(|(proj, vox)| (proj, FeatureId::Face(vox)))
            })
            .map(|res| res.1 .1)
            .unwrap_or((
                PointProjection::new(false, Vector::splat(Real::MAX)),
                FeatureId::Unknown,
            ))
    }
}

impl<'a> VoxelsChunkRef<'a> {
    #[inline]
    fn project_local_point_and_get_vox_id(
        &self,
        pt: Vector,
        solid: bool,
    ) -> Option<(PointProjection, u32)> {
        // TODO: optimize this naive implementation that just iterates on all the voxels
        //       from this chunk.
        let base_cuboid = Cuboid::new(self.parent.voxel_size() / 2.0);
        let mut smallest_dist = Real::MAX;
        let mut result = PointProjection::new(false, pt);
        let mut result_vox_id = 0;

        for vox in self.voxels() {
            let mut candidate = base_cuboid.project_local_point(pt - vox.center, solid);
            candidate.point += vox.center;

            let candidate_dist = (candidate.point - pt).length();
            if candidate_dist < smallest_dist {
                result = candidate;
                result_vox_id = vox.linear_id.flat_id();
                smallest_dist = candidate_dist;
            }
        }

        (smallest_dist < Real::MAX).then_some((result, result_vox_id as u32))
    }
}

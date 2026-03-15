use crate::bounding_volume::Aabb;
use crate::math::Vector;
use crate::shape::Voxels;
use alloc::{vec, vec::Vec};

impl Voxels {
    /// Computes an unoptimized mesh representation of this shape.
    ///
    /// Each free face of each voxel will result in two triangles. No effort is made to merge
    /// adjacent triangles on large flat areas.
    pub fn to_trimesh(&self) -> (Vec<Vector>, Vec<[u32; 3]>) {
        let aabb = Aabb::from_half_extents(Vector::ZERO, self.voxel_size() / 2.0);
        let aabb_vtx = aabb.vertices();

        let mut vtx = vec![];
        let mut idx = vec![];
        for vox in self.voxels() {
            let mask = vox.state.free_faces();
            for i in 0..6 {
                if mask.bits() & (1 << i) != 0 {
                    let fvid = Aabb::FACES_VERTEX_IDS[i];
                    let base_id = vtx.len() as u32;
                    vtx.push(vox.center + aabb_vtx[fvid.0]);
                    vtx.push(vox.center + aabb_vtx[fvid.1]);
                    vtx.push(vox.center + aabb_vtx[fvid.2]);
                    vtx.push(vox.center + aabb_vtx[fvid.3]);

                    if i % 2 == 0 {
                        idx.push([base_id, base_id + 1, base_id + 2]);
                        idx.push([base_id, base_id + 2, base_id + 3]);
                    } else {
                        idx.push([base_id, base_id + 2, base_id + 1]);
                        idx.push([base_id, base_id + 3, base_id + 2]);
                    }
                }
            }
        }

        (vtx, idx)
    }
}

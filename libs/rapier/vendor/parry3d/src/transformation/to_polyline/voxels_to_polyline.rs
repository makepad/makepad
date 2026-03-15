use crate::bounding_volume::Aabb;
use crate::math::{Vector, Vector2};
use crate::shape::{VoxelType, Voxels};
use alloc::{vec, vec::Vec};

impl Voxels {
    /// Converts the outlines of this set of voxels as a polyline.
    pub fn to_polyline(&self) -> (Vec<Vector2>, Vec<[u32; 2]>) {
        let mut points = vec![];
        self.iter_outline(|a, b| {
            points.push(a);
            points.push(b);
        });
        let indices = (0..points.len() as u32 / 2)
            .map(|i| [i * 2, i * 2 + 1])
            .collect();
        (points, indices)
    }

    /// Outlines this voxels shape with segments.
    pub fn iter_outline(&self, mut f: impl FnMut(Vector, Vector)) {
        // TODO: move this as a new method: Voxels::to_outline?
        let radius = self.voxel_size() / 2.0;
        let aabb = Aabb::from_half_extents(Vector::ZERO, radius);
        let vtx = aabb.vertices();

        for vox in self.voxels() {
            match vox.state.voxel_type() {
                VoxelType::Vertex => {
                    let mask = vox.state.feature_mask();

                    for edge in Aabb::FACES_VERTEX_IDS {
                        if mask & (1 << edge.0) != 0 || mask & (1 << edge.1) != 0 {
                            f(vox.center + vtx[edge.0], vox.center + vtx[edge.1]);
                        }
                    }
                }
                VoxelType::Face => {
                    let vtx = aabb.vertices();
                    let mask = vox.state.feature_mask();

                    for (i, edge) in Aabb::FACES_VERTEX_IDS.iter().enumerate() {
                        if mask & (1 << i) != 0 {
                            f(vox.center + vtx[edge.0], vox.center + vtx[edge.1]);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

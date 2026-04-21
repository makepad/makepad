use crate::bounding_volume::Aabb;
use crate::math::Vector;
use crate::shape::RoundCuboid;
use crate::transformation::utils;
use alloc::{vec, vec::Vec};

impl RoundCuboid {
    /// Outlines this round cuboid’s surface with polylines.
    pub fn to_outline(&self, nsubdivs: u32) -> (Vec<Vector>, Vec<[u32; 2]>) {
        let aabb = Aabb::from_half_extents(Vector::ZERO, self.inner_shape.half_extents);
        let aabb_vtx = aabb.vertices();
        let fidx = Aabb::FACES_VERTEX_IDS;
        let x = Vector::X * self.border_radius;
        let y = Vector::Y * self.border_radius;
        let z = Vector::Z * self.border_radius;

        #[rustfmt::skip]
        let mut vtx = vec![
            aabb_vtx[fidx[0].0] + x, aabb_vtx[fidx[0].1] + x, aabb_vtx[fidx[0].2] + x, aabb_vtx[fidx[0].3] + x,
            aabb_vtx[fidx[1].0] - x, aabb_vtx[fidx[1].1] - x, aabb_vtx[fidx[1].2] - x, aabb_vtx[fidx[1].3] - x,
            aabb_vtx[fidx[2].0] + y, aabb_vtx[fidx[2].1] + y, aabb_vtx[fidx[2].2] + y, aabb_vtx[fidx[2].3] + y,
            aabb_vtx[fidx[3].0] - y, aabb_vtx[fidx[3].1] - y, aabb_vtx[fidx[3].2] - y, aabb_vtx[fidx[3].3] - y,
            aabb_vtx[fidx[4].0] + z, aabb_vtx[fidx[4].1] + z, aabb_vtx[fidx[4].2] + z, aabb_vtx[fidx[4].3] + z,
            aabb_vtx[fidx[5].0] - z, aabb_vtx[fidx[5].1] - z, aabb_vtx[fidx[5].2] - z, aabb_vtx[fidx[5].3] - z,
        ];

        let mut idx = vec![];
        for i in 0..6 {
            idx.push([i * 4, i * 4 + 1]);
            idx.push([i * 4 + 1, i * 4 + 2]);
            idx.push([i * 4 + 2, i * 4 + 3]);
            idx.push([i * 4 + 3, i * 4]);
        }

        let arcs = [
            [4, 13, 20],
            [0, 12, 21],
            [1, 8, 22],
            [5, 9, 23],
            [7, 14, 16],
            [3, 15, 17],
            [2, 11, 18],
            [6, 10, 19],
        ];

        for (center, aidx) in arcs.iter().enumerate() {
            for ia in 0..3 {
                let ib = (ia + 1) % 3;
                utils::push_arc_and_idx(
                    aabb_vtx[center],
                    aidx[ia],
                    aidx[ib],
                    nsubdivs,
                    &mut vtx,
                    &mut idx,
                );
            }
        }

        (vtx, idx)
    }
}

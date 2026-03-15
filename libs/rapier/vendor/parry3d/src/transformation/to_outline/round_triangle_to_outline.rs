use crate::bounding_volume::Aabb;
use crate::math::{Vector, Real, Vector};
use crate::shape::RoundTriangle;
use crate::transformation::utils;

impl RoundTriangle {
    /// Outlines this round triangle’s surface with polylines.
    pub fn to_outline(&self, nsubdivs: u32) -> (Vec<Vector>, Vec<[u32; 2]>) {
        let tri = &self.inner_shape;
        let n = tri
            .normal()
            .map(|n| n.into_inner())
            .unwrap_or_else(|| Vector::ZERO);
        let mut out_vtx = vec![
            tri.a + n * self.border_radius,
            tri.b + n * self.border_radius,
            tri.c + n * self.border_radius,
            tri.a - n * self.border_radius,
            tri.b - n * self.border_radius,
            tri.c - n * self.border_radius,
        ];
        let mut out_idx = vec![[0, 1], [1, 2], [2, 0], [3, 4], [4, 5], [5, 3]];
        let ab = tri.b - tri.a;
        let ac = tri.c - tri.a;
        let bc = tri.b - tri.c;

        utils::push_arc_and_idx(tri.a, 0, 3, nsubdivs, &mut out_vtx, &mut out_idx);
        utils::push_arc_and_idx(tri.b, 1, 4, nsubdivs, &mut out_vtx, &mut out_idx);
        utils::push_arc_and_idx(tri.c, 2, 5, nsubdivs, &mut out_vtx, &mut out_idx);

        (out_vtx, out_idx)
    }
}

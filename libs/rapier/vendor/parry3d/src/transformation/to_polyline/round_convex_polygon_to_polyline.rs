use crate::math::Vector2;
use crate::shape::RoundConvexPolygon;
use crate::transformation::utils;
use alloc::{vec, vec::Vec};

impl RoundConvexPolygon {
    /// Discretize the boundary of this round convex polygon as a polygonal line.
    pub fn to_polyline(&self, border_subdivs: u32) -> Vec<Vector2> {
        let mut out_vtx = vec![];
        let pts = self.inner_shape.points();
        let ns = self.inner_shape.normals();
        let br = self.border_radius;

        out_vtx.push(pts[0] + *ns.last().unwrap() * br);

        for ia in 0..pts.len() - 1 {
            let ib = ia + 1;

            let arc_start = *out_vtx.last().unwrap();
            let arc_end = pts[ia] + ns[ia] * br;
            utils::push_arc(pts[ia], arc_start, arc_end, border_subdivs, &mut out_vtx);
            out_vtx.push(arc_end);
            out_vtx.push(pts[ib] + ns[ia] * br);
        }

        let arc_center = *pts.last().unwrap();
        let arc_start = *out_vtx.last().unwrap();
        let arc_end = arc_center + *ns.last().unwrap() * br;
        utils::push_arc(arc_center, arc_start, arc_end, border_subdivs, &mut out_vtx);
        out_vtx.push(arc_end);

        out_vtx
    }
}

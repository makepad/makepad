use crate::math::Vector;
use crate::shape::RoundCylinder;
use crate::transformation::utils;
use alloc::{vec, vec::Vec};

impl RoundCylinder {
    /// Outlines this round cylinder’s shape using polylines.
    pub fn to_outline(&self, nsubdiv: u32, border_nsubdiv: u32) -> (Vec<Vector>, Vec<[u32; 2]>) {
        let r = self.inner_shape.radius;
        let br = self.border_radius;
        let he = self.inner_shape.half_height;

        let mut out_vtx = vec![];
        let mut out_idx = vec![];

        // Compute the profile.
        let center_ab = Vector::new(-r, -he, 0.0);
        let center_cd = Vector::new(-r, he, 0.0);
        let a = Vector::new(-r, -he - br, 0.0);
        let b = Vector::new(-r - br, -he, 0.0);
        let c = Vector::new(-r - br, he, 0.0);
        let d = Vector::new(-r, he + br, 0.0);

        out_vtx.push(a);
        utils::push_arc(center_ab, a, b, border_nsubdiv, &mut out_vtx);
        out_vtx.push(b);
        out_vtx.push(c);
        utils::push_arc(center_cd, c, d, border_nsubdiv, &mut out_vtx);
        out_vtx.push(d);

        let circles = [
            0..1,
            border_nsubdiv..border_nsubdiv + 2,
            border_nsubdiv * 2 + 1..border_nsubdiv * 2 + 2,
        ];
        utils::apply_revolution(false, false, &circles, nsubdiv, &mut out_vtx, &mut out_idx);
        (out_vtx, out_idx)
    }
}

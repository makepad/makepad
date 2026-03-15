use crate::math::Vector;
use crate::shape::Cylinder;
use crate::transformation::utils;
use alloc::{vec, vec::Vec};

impl Cylinder {
    /// Outlines this cylinder’s shape using polylines.
    pub fn to_outline(&self, nsubdiv: u32) -> (Vec<Vector>, Vec<[u32; 2]>) {
        let diameter = self.radius * 2.0;
        let height = self.half_height * 2.0;
        let scale = Vector::new(diameter, height, diameter);
        let (vtx, idx) = unit_cylinder_outline(nsubdiv);
        (utils::scaled(vtx, scale), idx)
    }
}

/// Generates a cylinder with unit height and diameter.
fn unit_cylinder_outline(nsubdiv: u32) -> (Vec<Vector>, Vec<[u32; 2]>) {
    let mut out_vtx = vec![Vector::new(-0.5, -0.5, 0.0), Vector::new(-0.5, 0.5, 0.0)];
    let mut out_idx = vec![];
    #[allow(clippy::single_range_in_vec_init)] // The single range is on purpose.
    utils::apply_revolution(false, false, &[0..2], nsubdiv, &mut out_vtx, &mut out_idx);
    (out_vtx, out_idx)
}

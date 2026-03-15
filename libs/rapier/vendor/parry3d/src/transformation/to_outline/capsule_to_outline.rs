use crate::math::{Real, Vector};
use crate::shape::{Capsule, Cylinder};
use crate::transformation::utils;
use alloc::vec::Vec;

impl Capsule {
    /// Outlines this capsule’s shape using polylines.
    pub fn to_outline(&self, nsubdiv: u32) -> (Vec<Vector>, Vec<[u32; 2]>) {
        let (vtx, idx) = canonical_capsule_outline(self.radius, self.half_height(), nsubdiv);
        (utils::transformed(vtx, self.canonical_transform()), idx)
    }
}

/// Generates a capsule.
pub(crate) fn canonical_capsule_outline(
    caps_radius: Real,
    cylinder_half_height: Real,
    nsubdiv: u32,
) -> (Vec<Vector>, Vec<[u32; 2]>) {
    let (mut vtx, mut idx) = Cylinder::new(cylinder_half_height, caps_radius).to_outline(nsubdiv);
    let shift = vtx.len() as u32;

    // Generate the hemispheres.
    super::ball_to_outline::push_unit_hemisphere_outline(nsubdiv / 2, &mut vtx, &mut idx);
    super::ball_to_outline::push_unit_hemisphere_outline(nsubdiv / 2, &mut vtx, &mut idx);

    let ncap_pts = (nsubdiv / 2 + 1) * 2;
    vtx[shift as usize..(shift + ncap_pts) as usize]
        .iter_mut()
        .for_each(|pt| {
            *pt *= caps_radius * 2.0;
            pt.y += cylinder_half_height
        });

    vtx[(shift + ncap_pts) as usize..]
        .iter_mut()
        .for_each(|pt| {
            *pt *= caps_radius * 2.0;
            pt.y = -pt.y - cylinder_half_height
        });

    (vtx, idx)
}

use crate::math::{Real, Vector3};
use crate::shape::Capsule;
use crate::transformation::utils;
use alloc::vec::Vec;

impl Capsule {
    /// Discretize the boundary of this capsule as a triangle-mesh.
    pub fn to_trimesh(
        &self,
        ntheta_subdiv: u32,
        nphi_subdiv: u32,
    ) -> (Vec<Vector3>, Vec<[u32; 3]>) {
        let diameter = self.radius * 2.0;
        let height = self.half_height() * 2.0;
        let (vtx, idx) = canonical_capsule(diameter, height, ntheta_subdiv, nphi_subdiv);
        (utils::transformed(vtx, self.canonical_transform()), idx)
    }
}

/// Generates a capsule.
pub(crate) fn canonical_capsule(
    caps_diameter: Real,
    cylinder_height: Real,
    ntheta_subdiv: u32,
    nphi_subdiv: u32,
) -> (Vec<Vector3>, Vec<[u32; 3]>) {
    let (coords, indices) = super::ball_to_trimesh::unit_hemisphere(ntheta_subdiv, nphi_subdiv / 2);
    let mut bottom_coords = coords.clone();
    let mut bottom_indices = indices.clone();
    utils::reverse_clockwising(&mut bottom_indices[..]);

    let mut top_coords = coords;
    let mut top_indices = indices;

    let half_height = cylinder_height * 0.5;

    // shift the top
    for coord in top_coords.iter_mut() {
        coord.x *= caps_diameter;
        coord.y = coord.y * caps_diameter + half_height;
        coord.z *= caps_diameter;
    }

    // flip + shift the bottom
    for coord in bottom_coords.iter_mut() {
        coord.x *= caps_diameter;
        coord.y = -(coord.y * caps_diameter) - half_height;
        coord.z *= caps_diameter;
    }

    // shift the top index buffer
    let base_top_coords = bottom_coords.len() as u32;

    for idx in top_indices.iter_mut() {
        idx[0] += base_top_coords;
        idx[1] += base_top_coords;
        idx[2] += base_top_coords;
    }

    // merge all buffers
    bottom_coords.extend(top_coords);
    bottom_indices.extend(top_indices);

    // attach the two caps
    utils::push_ring_indices(0, base_top_coords, ntheta_subdiv, &mut bottom_indices);

    (bottom_coords, bottom_indices)
}

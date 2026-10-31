//! The depth query planes as debug quads. The room field's mesher itself is
//! `makepad_tsdf::mesh` (with its worker, `makepad_tsdf::mesh_worker`).
use crate::algorithms::tsdf_query::{depth_query_plane_quad, DepthQuerySupportPlane};
use makepad_widgets::makepad_platform::xr_tsdf::mesh::push_debug_depth_quad;

pub(crate) fn push_debug_depth_plane(indices: &mut Vec<u32>, vertices: &mut Vec<f32>, plane: DepthQuerySupportPlane) {
    push_debug_depth_quad(indices, vertices, depth_query_plane_quad(plane));
}

#[cfg(test)]
include!("../tests/util/depth_debug_mesh.rs");

//! Meshing a field over a box at a cell size (see [`crate::surface_mesh`]).

use crate::sdf::Sdf3;
use crate::surface_mesh::mesh_surface;
use makepad_csg_math::Vec3d;
use makepad_csg_mesh::mesh::TriMesh;
use std::sync::Arc;

/// The largest side of a box.
fn extent(min: Vec3d, max: Vec3d) -> f64 {
    let d = max - min;
    d.x.max(d.y).max(d.z)
}

/// The default cell for a box: 80 cells over its largest side, never finer than
/// `finest`.
pub fn default_cell(min: Vec3d, max: Vec3d, finest: f64) -> f64 {
    (extent(min, max) / 80.0).max(finest)
}

/// Mesh `field` inside `min..max` with cubic cells of `cell`: closed,
/// edge-manifold, wound outwards.
pub fn mesh_field(field: Arc<dyn Sdf3 + Send + Sync>, min: Vec3d, max: Vec3d, cell: f64) -> TriMesh {
    mesh_surface(field, min, max, cell)
}

/// Mesh `sdf` inside `min..max` with `2^depth` cells along the box's largest side.
pub fn sdf_to_mesh(sdf: impl Sdf3 + Send + Sync + 'static, min: Vec3d, max: Vec3d, depth: usize) -> TriMesh {
    let cell = extent(min, max) / (1u64 << depth.min(20)) as f64;
    mesh_surface(Arc::new(sdf), min, max, cell)
}

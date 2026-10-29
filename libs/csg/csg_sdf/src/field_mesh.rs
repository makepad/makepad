//! Meshing a field over a box at a cell size: the octree depth for the cell, a
//! cubic grid with a margin round the shape, dual contouring, welded vertices
//! and degenerate triangles dropped.

use crate::octree::sdf_to_mesh;
use crate::sdf::Sdf3;
use makepad_csg_math::Vec3d;
use makepad_csg_mesh::mesh::TriMesh;
use std::sync::Arc;

/// Shallowest and deepest octree a field is meshed with.
pub const MIN_DEPTH: usize = 4;
pub const MAX_DEPTH: usize = 8;

/// The largest side of a box.
fn extent(min: Vec3d, max: Vec3d) -> f64 {
    let d = max - min;
    d.x.max(d.y).max(d.z)
}

/// The octree depth that meshes a box of `min..max` at `cell` per cell, with a
/// margin of two cells each side.
pub fn depth_for(min: Vec3d, max: Vec3d, cell: f64) -> usize {
    let extent = extent(min, max).max(1e-6);
    let cells = (extent / cell.max(1e-6) + 4.0).max(1.0);
    (cells.log2().ceil() as usize).clamp(MIN_DEPTH, MAX_DEPTH)
}

/// The default cell for a box: 80 cells over its largest side, never finer than
/// `finest`.
pub fn default_cell(min: Vec3d, max: Vec3d, finest: f64) -> f64 {
    (extent(min, max) / 80.0).max(finest)
}

/// Mesh `field` inside `min..max` with cubic cells of `cell` (larger only when the
/// box needs more than the deepest octree's cells). The grid is a cube grown to a
/// whole number of cells round the box with two cells of margin, so the surface
/// never touches its boundary. Triangles are wound outwards.
pub fn mesh_field(field: Arc<dyn Sdf3 + Send + Sync>, min: Vec3d, max: Vec3d, cell: f64) -> TriMesh {
    let depth = depth_for(min, max, cell);
    let extent = extent(min, max);
    let cells = (1u64 << depth) as f64;
    let side = if cell * cells >= extent + cell * 4.0 { cell * cells } else { extent * cells / (cells - 4.0) };
    let center = (min + max) * 0.5;
    let half = Vec3d::new(side * 0.5, side * 0.5, side * 0.5);
    let mut tri = sdf_to_mesh(Shared(field), center - half, center + half, depth);
    tri.weld_vertices(side * 1e-9);
    tri.triangles.retain(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]);
    tri
}

struct Shared(Arc<dyn Sdf3 + Send + Sync>);

impl Sdf3 for Shared {
    fn distance(&self, p: Vec3d) -> f64 {
        self.0.distance(p)
    }
}

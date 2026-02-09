// makepad-csg: Top-level CSG library
//
// Provides an OpenSCAD-like API for constructive solid geometry.
// Build solids from primitives, combine with boolean operations,
// transform, and export to STL/OBJ for 3D printing.

pub mod solid;

// Re-export the main types
pub use solid::{difference_all, intersection_all, union_all, Solid};

// Re-export sub-crate types that users commonly need
pub use makepad_csg_math::{dvec3, BBox3d, Mat4d, Vec3d};
pub use makepad_csg_mesh::mesh::TriMesh;
pub use makepad_csg_mesh::validate::MeshReport;

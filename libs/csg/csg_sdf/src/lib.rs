// makepad-csg-sdf: Signed Distance Field meshing via dual contouring
//
// Provides an SDF trait, primitive shapes, boolean combinators (including
// smooth/blobby union with a "gloopiness" parameter), and a dual-contouring
// mesher that converts any SDF into a TriMesh usable with the rest of the
// CSG stack.

pub mod bounds;
mod combinators;
mod field_mesh;
mod grid;
mod octree;
mod primitives;
mod sdf;
mod shape;
mod splash_field;

pub use bounds::{changed, cull, node_bounds, same, Bounds};
pub use combinators::*;
pub use field_mesh::{default_cell, depth_for, mesh_field, MAX_DEPTH, MIN_DEPTH};
pub use grid::SdfGrid3;
pub use octree::{sdf_to_mesh, sdf_to_mesh_ref};
pub use primitives::*;
pub use sdf::Sdf3;
pub use shape::{distance, SdfKernel, SdfNode, SdfOp, SdfShape};
pub use splash_field::{SdfSplashExpr, SplashPoint};

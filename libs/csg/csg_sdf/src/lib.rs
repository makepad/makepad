// makepad-csg-sdf: Signed Distance Field meshing via dual contouring
//
// Provides an SDF trait, primitive shapes, boolean combinators (including
// smooth/blobby union with a "gloopiness" parameter), and a sparse,
// surface-following dual-contouring mesher that converts any SDF into a
// closed TriMesh usable with the rest of the CSG stack.

pub mod bounds;
mod combinators;
mod field_mesh;
pub mod mesh_field;
mod primitives;
mod sdf;
mod shape;
mod splash_field;
pub mod surface_mesh;

pub use bounds::{changed, cull, deform_bounds, node_bounds, same, Bounds};
pub use combinators::*;
pub use field_mesh::{default_cell, mesh_field, sdf_to_mesh};
pub use primitives::*;
pub use sdf::Sdf3;
pub use shape::{distance, distance_slot, part_id, quat_from_degrees, quat_rotate, SdfDeform, SdfKernel, SdfMaterial, SdfNode, SdfOp, SdfPlace, SdfShape, SdfSplat};
pub use splash_field::{SdfSplashExpr, SplashPoint};

// makepad-csg-sdf: Signed Distance Field meshing via dual contouring
//
// Provides an SDF trait, primitive shapes, boolean combinators (including
// smooth/blobby union with a "gloopiness" parameter), and a dual-contouring
// mesher that converts any SDF into a TriMesh usable with the rest of the
// CSG stack.

mod combinators;
mod grid;
mod octree;
mod primitives;
mod sdf;

pub use combinators::*;
pub use grid::SdfGrid3;
pub use octree::sdf_to_mesh;
pub use primitives::*;
pub use sdf::Sdf3;

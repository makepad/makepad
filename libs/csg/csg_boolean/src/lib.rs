// makepad-csg-boolean: Corefinement-based boolean operations.
//
// Production-quality boolean operations that preserve original triangles
// away from intersections and produce high-quality mesh output.

pub mod aabb_tree;
pub mod boolean;
pub mod cdt;
pub mod classify;
pub mod corefine;
pub mod thread_pool;
pub mod tri_tri;

pub use aabb_tree::AabbTree;
pub use boolean::{difference, intersection, mesh_boolean, mesh_boolean_in, union, BoolOp};
pub use cdt::CDT;
pub use classify::{classify_triangles, classify_triangles_in, point_inside_mesh, MeshAccel, TriLocation};
pub use corefine::{corefine, corefine_in, CorefinementResult};
pub use tri_tri::{tri_tri_intersection, TriTriResult};

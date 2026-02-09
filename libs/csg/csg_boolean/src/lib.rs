// makepad-csg-boolean: Corefinement-based boolean operations.
//
// Production-quality boolean operations that preserve original triangles
// away from intersections and produce high-quality mesh output.

pub mod aabb_tree;
pub mod boolean;
pub mod cdt;
pub mod classify;
pub mod corefine;
pub mod tri_tri;

pub use aabb_tree::AabbTree;
pub use boolean::{difference, intersection, mesh_boolean, union, BoolOp};
pub use cdt::CDT;
pub use classify::{classify_triangles, point_inside_mesh, TriLocation};
pub use corefine::{corefine, CorefinementResult};
pub use tri_tri::{tri_tri_intersection, TriTriResult};

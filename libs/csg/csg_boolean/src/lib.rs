// makepad-csg-boolean: Corefinement-based boolean operations.
//
// Production-quality boolean operations that preserve original triangles
// away from intersections and produce high-quality mesh output.

pub mod aabb_tree;
pub mod boolean;
pub mod cdt;
pub mod classify;
pub mod corefine;
mod contact;
pub mod thread_pool;
pub mod tri_tri;
mod virtual_points;
mod classes;
mod keys;
mod sites;
mod cut;
mod cells;
mod weld;

pub use aabb_tree::AabbTree;
pub use boolean::{difference, intersection, mesh_boolean, mesh_boolean_in, mesh_boolean_report, union, BoolOp, BooleanReport};
pub use cdt::CDT;
pub use classify::{classify_triangles, classify_triangles_in, point_inside_mesh, MeshAccel, TriLocation};
pub use corefine::{corefine, corefine_in, CorefineReport, CorefinementResult};
pub use tri_tri::{tri_tri_intersection, TriTriResult};

#[cfg(test)]
mod star_prism_tests;

#[cfg(test)]
mod twisted_contact_tests;

#[cfg(test)]
mod two_shot_tests;

#[cfg(test)]
mod coplanar_tests;

#[cfg(test)]
mod rounding_tests;

#[cfg(test)]
mod zero_contact_tests;

#[cfg(test)]
mod tag_tests;

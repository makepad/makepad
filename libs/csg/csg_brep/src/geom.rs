//! Core geometric value types and tolerance helpers: 2D/3D vectors, extents
//! (bounding boxes and intervals), pseudo boxes, transforms and the `SmTol`
//! tolerance policy.
//!
//! Upstream: `source/SMLib/{inc,src}/SmVector2d`, `SmVector3d`, `SmExtent*`,
//! `SmPseudoBox`, `SmTol`, `SmTransform`, `SmMatrix` and related `geom.*` units of
//! `plan/modules.toml`.

pub mod a_object;
pub mod assert_array;
pub mod attribute;
pub mod axis2_placement;
pub mod context;
pub mod extent1d;
pub mod extent2d;
pub mod extent3d;
pub mod geom_utility;
pub mod global_solver;
pub mod grid;
pub mod hash_table;
pub mod local_solve1d;
pub mod local_solve_nd;
pub mod matrix;
pub mod object;
pub mod periodic_extent1d;
pub mod point_set;
pub mod polar_box;
pub mod polynomial;
pub mod pseudo_box;
pub mod solution_array;
pub mod tol;
pub mod tools;
pub mod transform;
pub mod tree;
pub mod vector2d;
pub mod vector3d;

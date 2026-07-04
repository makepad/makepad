// Rust port of Box3D (https://github.com/erincatto/box3d) by Erin Catto.
// See PORTING.md for the conventions used by this port.
//
// The module layout mirrors the C source files one to one.

#![allow(dead_code)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::needless_range_loop)]

// Foundation (include/box3d + core)
pub mod constants;
pub mod core;
pub mod id;
pub mod math_functions;
pub mod math_internal;
pub mod types;

// Test utilities (shared/utils.h)
pub mod test_utils;

// Infrastructure
pub mod arena_allocator;
pub mod bitset;
pub mod container;
pub mod ctz;
pub mod id_pool;
pub mod table;
pub mod timer;

// Geometry / collision
pub mod aabb;
pub mod capsule;
pub mod compound;
pub mod distance;
pub mod dynamic_tree;
pub mod height_field;
pub mod hull;
pub mod mesh;
pub mod sphere;

// Manifolds / narrow phase
pub mod convex_manifold;
pub mod manifold;
pub mod mesh_contact;
pub mod triangle_manifold;

// Dynamics
pub mod body;
pub mod broad_phase;
pub mod constraint_graph;
pub mod contact;
pub mod contact_solver;
pub mod island;
pub mod joint;
pub mod parallel_for;
pub mod physics_world;
pub mod sensor;
pub mod shape;
pub mod simd;
pub mod solver;
pub mod solver_set;

// Snapshots (recording substrate subset)
pub mod recording;
pub mod recording_replay;
pub mod world_snapshot;

// Joints
pub mod distance_joint;
pub mod motor_joint;
pub mod mover;
pub mod parallel_joint;
pub mod prismatic_joint;
pub mod revolute_joint;
pub mod spherical_joint;
pub mod weld_joint;
pub mod wheel_joint;

pub use crate::constants::*;
pub use crate::core::*;
pub use crate::id::*;
pub use crate::math_functions::*;
pub use crate::types::*;

pub use crate::aabb::*;
pub use crate::capsule::*;
pub use crate::distance::*;
pub use crate::dynamic_tree::*;
pub use crate::hull::*;
pub use crate::sphere::*;

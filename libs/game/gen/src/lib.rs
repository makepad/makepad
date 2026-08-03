//! Procedural geometry and texture generation for Makepad Arcade.
//!
//! Everything here is seed-deterministic and baked: generation runs once
//! and produces a finished mesh in the packed vertex format, so runtime
//! cost is an ordinary draw. Two devices given the same seed produce
//! byte-identical geometry, which is what lets a forest replicate as
//! (preset, seed, position) tuples instead of vertex data.
//!
//! Generation never touches the world rng ([`rng::GenRng`] is a separate
//! stream) — the same discipline the bake, particle and audio systems follow.

pub mod cache;
pub mod implicit;
pub mod interior;
pub mod kit;
pub mod levelgen;
pub mod lsystem;
pub mod mesh;
pub mod rng;
pub mod scatter;
pub mod spline;
pub mod terrain;
pub mod texgen;
pub mod tree;

pub use cache::*;
pub use implicit::*;
pub use interior::*;
pub use kit::*;
pub use levelgen::*;
pub use lsystem::*;
pub use mesh::*;
pub use rng::*;
pub use scatter::*;
pub use spline::*;
pub use texgen::*;
pub use tree::*;

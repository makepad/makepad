//! Worker-owned polygon authoring. Persistent data consists of flat arrays and
//! edge attributes; adjacency is derived on demand and contains no pointer cycles.
//! All mutating operations install a candidate only after successful validation.
//! Geometry uses local f64 coordinates, metres and Y-up conventions.

mod codec;
mod context;
mod geometry;
mod mesh;
mod modifiers;
mod topology;
mod construction;
mod bevel;
mod deform;
mod shading;
mod sculpt;
mod uv;
mod unwrap;
mod decimate;
mod transfer;
mod intersections;
pub use intersections::*;
mod ops;
mod types;
mod validate;

pub use context::*;
pub use mesh::*;
pub use types::*;
pub use validate::*;

#[cfg(test)]
mod tests;

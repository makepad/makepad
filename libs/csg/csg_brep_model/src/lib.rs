//! makepad-csg-brep-model: what a modelling language needs from the exact B-rep kernel.
//!
//! Units are millimetres, the frame is Y up, like the solid graph. A [`Body`] is a
//! solid bound to exact surfaces. Every operation returns a [`BrepError`] instead of
//! panicking, and runs through [`guard`], which turns a panic or runaway recursion in
//! the kernel into an error. [`tess`] turns a body into a watertight triangle mesh to a
//! chord tolerance; [`query`] answers exact questions (mass, edges, faces, distances).

pub mod engine;
pub mod guard;
pub mod ops;
pub mod profile;
pub mod query;
pub mod tess;

pub use engine::engine;
pub use guard::{guarded, stack_size, BrepError, CancelToken};
pub use ops::{new_kernel, Body};
pub use tess::{tessellate, TessMesh, TessOptions};

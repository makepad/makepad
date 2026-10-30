//! makepad-csg-sculpt: the CPU reference of Fab's organic modelling.
//!
//! - [`ops`]: organic SDF edits (smooth booleans with a blend radius,
//!   metaballs, tubes with varying radius along splines, twist, bend, taper,
//!   mirror, shell, offset, bounded noise displacement), each with a world
//!   box outside which it leaves the field alone and a Lipschitz bound.
//! - [`stroke`]: sculpt strokes (add, carve, smooth, inflate, flatten,
//!   pinch, grab) as compact records that are the source of a sculpt layer.
//! - [`kernel`]: the stroke bake as a GPU-shaped kernel (per-sample body,
//!   parameter block); the layer bakes through it.
//! - [`layer`]: a sculpt layer baked into sparse 8^3 distance bricks, with
//!   bounded re-bakes on stroke edits and live strokes streamed from a
//!   checkpoint.
//! - [`material`]: the material slot table (colour, metal, rough, emissive,
//!   colorize, fade, unlit) the per-sample u8 slots index; sliders edit the
//!   table, never the bricks.
//! - [`record`]: brush input to a quantised, decimated stroke path.
//! - [`brush`]: shared desktop and XR brush state, live strokes and haptics.
//! - [`rig`]: bone groups (rigid parts joined by smooth union) and instances
//!   (rigid transform, uniform scale) of a sculpted object.
//!
//! The brick engine (GPU bake and raymarch) and the direct slicer evaluate
//! the same edits; this crate defines the numbers they must reproduce.

pub mod brush;
pub mod kernel;
pub mod layer;
pub mod material;
pub mod math;
pub mod ops;
pub mod record;
pub mod rig;
pub mod stroke;

#[cfg(test)]
mod tests;

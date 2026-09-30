//! `makepad-render-lines`: fat lines, points and sprites (KERNELS.md P4).
//!
//! - [`batch`]: [`LineBatch`] (capsule segments with per-end width, colour,
//!   distance and birth) and [`PointBatch`] (sprites), plain data that eval
//!   code, kernels ([`layouts`]) and physics fill;
//! - [`ink`]: widths in logical pixels, coverage in physical pixels, and the
//!   0.7 px fade floor: a line keeps its ink at every output scale;
//! - [`draw`]: [`LineRenderer`] records batches into the current pass,
//!   linear, premultiplied and unclamped (HDR), with over or additive
//!   blending, dashes, trim windows, comet tails, distance fade and birth
//!   reveal, in world space or in logical screen pixels.
//!
//! The crate owns no pass, target or post: the host (the render graph's
//! sub-frame loop, a 2D compositor) calls it inside its own pass.

pub mod batch;
pub mod draw;
pub mod ink;
pub mod layouts;

pub use batch::*;
pub use draw::{DrawLinePoint, DrawLineSegment, LineRenderer, LineView};
pub use layouts::layouts;

/// Register the line and point shaders (after `makepad_draw::script_mod`).
pub fn script_mod(vm: &mut makepad_draw::ScriptVm) {
    draw::script_mod(vm);
}

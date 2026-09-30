//! The engine side of compute kernels (KERNELS.md §3.5.4a, §3.5.5): where a
//! kernel's output lives between the job that writes it and the frames that
//! draw it, and where kernel jobs run.
//!
//! - [`ring`]: a kernel output is a ring of slots. A slot is `Free`,
//!   `Writing` (one job owns it), `Published` (the one the draw uses, with
//!   its count and generation) or `InFlight` (replaced, but a submitted frame
//!   still reads it). A slot returns to `Free` only once the GPU fence of the
//!   last frame that read it has signalled. Buffers are sized to a
//!   high-water mark. A layout change (a shader edit) swaps at a publish:
//!   the old slots keep drawing until the new layout's first publish, and a
//!   job that started under the old layout cannot publish.
//! - [`topology`]: what the buffer is drawn as, and the index validation
//!   every publish runs (an index at or past the vertex count keeps the
//!   previous slot and reports the error).
//! - [`fences`]: frame serials (submitted, completed) from the platform, and
//!   a fence-delay test flag that holds completion back by whole frames.
//! - [`layout`]: a draw shader's reflected record (`Cx::layout_of`) as the
//!   layout a kernel writes in place (`output(Layout)`, `emit_buffer(Layout, n)`).
//! - [`pipeline`]: passes chained by buffer name, with ping-pong pairs, a
//!   barrier between passes, repeats, and emitted records compacted in
//!   element order.
//! - [`tools`]: the AI's `kernel_guide` and `kernel_check` (definitions in
//!   `makepad-kernel-tools`), the same in every app.
//! - [`anim`] (document key tracks as kernel input, AK1), [`precompute`]
//!   (steppers run ahead into a history sampled by t, AK5), [`feed`]
//!   (reduce results and records as pass and material uniforms, AK11).
//! - [`engine`]: the process's kernel scheduler, on the platform `TaskPool`
//!   (Heavy lane) once an app installs it, on a headless pool before; and a
//!   compile cache.

pub mod anim;
pub mod engine;
pub mod feed;
pub mod fences;
pub mod layout;
pub mod pipeline;
pub mod precompute;
pub mod ring;
pub mod tools;
pub mod topology;

pub use engine::{compile, engine, install, KernelEngine};
pub use fences::{CxFences, FrameFences, ManualFences};
pub use layout::{kernel_layout, LayoutMapError};
pub use makepad_script_compute as compute;
pub use pipeline::{Pass, Pipeline, PipelineError};
pub use ring::{DrawView, OutputRing, PublishError, RingBusy, RingStats, SlotState, WriteLease};
pub use topology::{Topology, TopologyError};

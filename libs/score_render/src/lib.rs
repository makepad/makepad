//! Retained, backend-neutral score paint lists and Makepad rendering support.
//!
//! The layout engine emits one immutable [`PaintList`] per page. Rendering is
//! deliberately split into deterministic planning and backend replay, so the
//! GPU and `MAKEPAD=headless` consume exactly the same culled, batched geometry.

mod batch;
mod cache;
mod geometry;
mod gpu;
mod lod;
mod paint;
mod spatial;
mod style;

pub use batch::*;
pub use cache::*;
pub use geometry::*;
pub use gpu::*;
pub use lod::*;
pub use paint::*;
pub use spatial::*;
pub use style::*;

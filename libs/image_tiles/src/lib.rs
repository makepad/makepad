//! makepad-image-tiles: a pannable, zoomable wall of downloaded pictures.
//!
//! Three pieces, one on-disk format:
//!
//! - [`tape`] — the engine: NV12 planes, the 128→8 px slot pyramid, shard
//!   atlas geometry, and one hardware-HEVC intra frame per file. This is
//!   the same engine the Source Library picture wall runs on; the tapes are
//!   byte-compatible.
//! - The **baker** ([`bake`], and the `image-tiles-bake` binary) — reads a
//!   manifest of image URLs, downloads and decodes them in RAM, and bakes a
//!   library directory: atlas tapes, per-picture full frames and zoom
//!   pyramids, and a small SQLite index ([`db`]). No JPEGs or PNGs are kept
//!   on disk. Point it (or an AI, or your own script) at any list of URLs.
//! - The **viewer** ([`grid::TileGrid`]) — a widget that opens a baked
//!   library and draws every picture on one pannable, zoomable grid:
//!   instanced tiles batched per atlas page, camera glides on uniforms,
//!   continuous LOD with crossfades, and full-resolution promotion under
//!   byte budgets.
//!
//! Quickest start:
//! ```text
//! cargo run -p makepad-image-tiles --release --bin image-tiles-bake -- \
//!     --root local/image-tiles examples/image_tiles/manifest.tsv
//! cargo run -p makepad-example-image-tiles --release
//! ```
//!
//! Note: baking requires the platform's single-intra-frame hardware encoder
//! (VideoToolbox — macOS today); viewing requires hardware decode.

pub use makepad_widgets;

pub mod bake;
pub mod db;
pub mod grid;
pub mod pack;
pub mod library;
pub mod store;
pub mod tape;

pub use grid::{TileGrid, TileGridAction};
pub use library::Library;

use makepad_widgets::ScriptVm;

/// Register the TileGrid widget. A host calls this once, after
/// `makepad_widgets::script_mod`, before its own UI module.
pub fn script_mod(vm: &mut ScriptVm) {
    crate::grid::script_mod(vm);
}

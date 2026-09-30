//! `makepad-text-mesh`: shaped text as geometry (KERNELS.md P4).
//!
//! - [`text_mesh`]: the glyph extruder (outlines, counters, bevels, the
//!   verified triangulation, layouts in space) writing the fx vertex stream;
//! - [`letters`]: 3D type with one mesh per letter, word and line centres,
//!   caps / walls / bevel by part, ink grids, centre lines and tubes;
//! - [`typo`]: variable axes, layout metrics, outlines, fitting, stroke
//!   fonts, karaoke states, text on 3D paths;
//! - [`host`]: `text.outline` and `text.layout` for kernels.

pub mod host;
pub mod letters;
pub mod text_mesh;
pub mod typo;

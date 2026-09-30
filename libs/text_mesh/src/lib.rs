//! `makepad-text-mesh`: shaped text as geometry (KERNELS.md P4).
//!
//! - [`text_mesh`]: the glyph extruder (outlines, counters, bevels, the
//!   verified triangulation, layouts in space) writing the fx vertex stream;
//! - [`letters`]: 3D type with one mesh per letter, word and line centres,
//!   caps / walls / bevel by part, ink grids, centre lines and tubes.
//!
//! Typography that is not geometry (layout metrics, outlines, stroke fonts,
//! fitting, karaoke, the kernel host components) is `makepad-typography`.

pub mod letters;
pub mod text_mesh;

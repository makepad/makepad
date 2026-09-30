//! Typography as engine primitives (PDOOM-PARITY §4, P4): fonts read at any
//! variable-axis values ([`font`], F1), shaped layout metrics ([`layout`],
//! F2), glyph outlines as paths ([`outline`], F3), fitting ([`fit`], F4),
//! single-stroke fonts with optical kerning and write-on ([`stroke`], F5),
//! karaoke states ([`karaoke`], F6) and text on a projected 3D path
//! ([`path`]). Pure CPU code, `Send`, no `Cx`: documents, kernels (through
//! [`crate::host`]) and renderers share it.

pub mod fit;
pub mod font;
pub mod karaoke;
pub mod layout;
pub mod outline;
pub mod path;
pub mod stroke;

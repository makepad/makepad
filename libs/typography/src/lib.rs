//! `makepad-typography`: typography as engine primitives (PDOOM-PARITY §4, P4): fonts read at any
//! variable-axis values ([`font`], F1), shaped layout metrics ([`layout`],
//! F2), glyph outlines as paths ([`outline`], F3), fitting ([`fit`], F4),
//! single-stroke fonts with optical kerning and write-on ([`stroke`], F5),
//! karaoke states ([`karaoke`], F6) and text on a projected 3D path
//! ([`path`]). Pure CPU code, `Send`, no `Cx`: documents, kernels (through
//! [`crate::host`]) and renderers share it. Everything but [`stroke`] and
//! [`karaoke`] needs the default `shaping` feature; without it the crate is
//! std only.

#[cfg(feature = "shaping")]
pub mod fit;
#[cfg(feature = "shaping")]
pub mod host;
#[cfg(feature = "shaping")]
pub mod font;
pub mod karaoke;
#[cfg(feature = "shaping")]
pub mod layout;
#[cfg(feature = "shaping")]
pub mod outline;
#[cfg(feature = "shaping")]
pub mod path;
#[cfg(feature = "shaping")]
pub mod record;
pub mod stroke;

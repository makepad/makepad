//! Texture compression: the block codecs and the container every texture
//! travels in.
//!
//! No dependencies and no `unsafe`. A texture arrives from outside the
//! process — a converted model, a published asset, a file someone dropped in
//! — so every length and offset in here is untrusted input and is checked
//! before it is used.

pub mod bc1;
pub mod ktx2;

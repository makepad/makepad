//! Cross-family internals shared by the AI model family crates that are
//! about to be split out of `libs/diffusion` (lane T6a, /aiarch.md §1).
//!
//! Only genuinely shared code lives here: the `DiffusionError` type (every
//! family's error type, kept as-is so the rename is a later lane's problem)
//! and the plain sharded-safetensors-directory reader. Family-private
//! internals (quantized weight sources, name canonicalization, ...) stay in
//! their family crates.

pub mod error;
pub mod sharded;

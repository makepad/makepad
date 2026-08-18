//! Backend-neutral model-weight disk layer. One home for every weight
//! format parser in the tree (safetensors, GGUF, torch zip/pickle, npy),
//! the mmap machinery, and the WeightSet API models load through.
//! No GPU dependencies — the cuda/metal stores consume views produced here.
//! Plan of record: /aiarch.md
//!
//! Populated by the restructure lanes: formats/ (safetensors, gguf, torch,
//! npy), mmap.rs, weight_set.rs.

pub mod formats;
pub mod mmap;
pub mod quant;

// The safetensors slice is the only format lane T2 re-points consumers to;
// re-export its types at the crate root so `use makepad_ai_loader::X`
// mirrors the old `use makepad_mlx::X` line exactly.
pub use formats::safetensors::{MlxDType, MlxRtError, MlxSafetensorsHeader, MlxTensorEntry};
/// Forward-looking alias for the canonical error name in /aiarch.md §1
/// (`loader/` -> `LoaderError`). `MlxRtError` stays the primary export
/// until lane T3 renames call sites; both paths name the same type.
pub use formats::safetensors::MlxRtError as LoaderError;

pub use mmap::MappedRegion;

//! Moved to makepad-ai-loader (lane T2, /aiarch.md §1): GGUF parsing is
//! backend-neutral disk layer, not llama-specific. Re-exported here so
//! `crate::gguf::*` keeps working at every existing call site. Errors
//! convert via `impl From<GgufError> for LlamaError` in error.rs.

pub use makepad_ai_loader::formats::gguf::*;

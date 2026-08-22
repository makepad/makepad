//! Moved to makepad-ai-loader (lane T2, /aiarch.md §1): read-only mmap is
//! backend-neutral disk layer, not ggml-specific. Re-exported here so
//! `crate::MappedRegion` / `crate::mmap::MappedRegion` keep working.

pub use makepad_ai_loader::mmap::MappedRegion;

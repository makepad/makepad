// Moved to makepad-ai-common (lane T6a, /aiarch.md §1): the DiffusionError
// type + Result alias are genuinely cross-family, so they now live in
// libs/ai/models/common/src/error.rs. This shim keeps every
// `crate::error::X` / `crate::{DiffusionError, Result}` path in this crate's
// 100+ modules compiling unchanged.
pub use makepad_ai_common::error::*;

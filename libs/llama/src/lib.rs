//! Facade over `makepad-ai-llm` so existing `makepad_llama::` paths keep
//! resolving while consumers re-point (aiarch.md §1).

pub use makepad_ai_llm::*;

//! Makepad AI - Multi-backend AI integration library
//!
//! Supports:
//! - Claude (Anthropic) - API key or OAuth token for Pro/Max subscriptions
//! - OpenAI - GPT-4, o3-mini, etc.
//! - Google Gemini

pub mod backend;
pub mod backends;
pub mod types;

pub use backend::*;
pub use backends::*;
pub use types::*;

// Re-export dependencies for convenience
pub use makepad_live_id;
pub use makepad_micro_serde;
pub use makepad_widgets2;

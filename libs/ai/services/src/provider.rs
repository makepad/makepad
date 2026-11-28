//! Provider names shared by the chat panel and its launchers.

/// The provider slug of the F10 panel's Claude Desktop row.
pub const CLAUDE_DESKTOP: &str = "claude-desktop";
/// The environment variable that picks the F10 panel's provider. `none` is no
/// model at all: the AI hub's providers read it too
/// (`makepad_ai_hub::providers::no_model`) and find nothing.
pub const PROVIDER_ENV: &str = "MAKEPAD_AI_PROVIDER";

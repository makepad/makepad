pub mod claude;
pub mod claude_api;
pub mod cli;
pub mod codex_cli;
pub mod fleet_http;
pub mod grok;
pub mod grok_cli;
pub mod openai;
pub mod provider;
pub mod qwen;
pub mod responses;

/// The environment variable a launch names its AI provider with: the chat
/// panel's pick (`makepad_ai_services::provider::PROVIDER_ENV`, the same
/// name), and `none` for no model at all.
pub const PROVIDER_ENV: &str = "MAKEPAD_AI_PROVIDER";

/// What a provider says when the launch has no model.
pub const NO_MODEL: &str = "this launch uses no model (MAKEPAD_AI_PROVIDER=none)";

/// Why nothing answers here, when the launch said no model at all
/// (`MAKEPAD_AI_PROVIDER=none`, as a test or a person sets it). Every
/// provider asks this before it looks for anything: a model command line
/// ([`cli::find_cli`]), an API key, the LAN's hubs ([`qwen`], the chat
/// election). So no caller finds or starts a model, whichever one asks.
pub fn no_model() -> Option<&'static str> {
    no_model_in(std::env::var_os(PROVIDER_ENV).as_deref())
}

/// [`no_model`] for the variable's value.
pub(crate) fn no_model_in(provider: Option<&std::ffi::OsStr>) -> Option<&'static str> {
    provider.and_then(std::ffi::OsStr::to_str).is_some_and(|p| p.trim() == "none").then_some(NO_MODEL)
}

/// The reason a provider that found nothing gives: [`NO_MODEL`] when the
/// launch said so, `missing` (what to install or set) otherwise.
pub fn unavailable_reason(missing: &str) -> String {
    no_model().unwrap_or(missing).to_string()
}

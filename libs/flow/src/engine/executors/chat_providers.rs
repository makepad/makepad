//! Explicit provider adapters for Flow's chat executor.
use makepad_ai_hub::chat_wire::{ChatMessage, ChatRole, ProviderAvailability};
use makepad_ai_hub::providers::provider::{ChatProvider, ProviderEvent, TurnInput};

pub const PROVIDER_SLUGS: [&str; 6] = ["fleet-qwen", "openai", "grok", "claude-cli", "codex-cli", "grok-cli"];

pub(crate) trait ProviderSession {
    fn begin(&mut self, system: &str, prompt: &str) -> Result<(), String>;
    fn poll(&mut self) -> Vec<ProviderEvent>;
    fn cancel(&mut self);
    fn availability(&mut self) -> ProviderAvailability;
}

pub enum ProviderAdapter {
    Fleet(makepad_ai_hub::providers::qwen::FleetQwenChatProvider<makepad_ai_hub::providers::qwen::HttpFleetTransport>),
    OpenAi(makepad_ai_hub::providers::openai::OpenAiChatProvider),
    Grok(makepad_ai_hub::providers::grok::GrokChatProvider),
    Claude(makepad_ai_hub::providers::claude::ClaudeCodeChatProvider),
    Codex(makepad_ai_hub::providers::codex_cli::CodexCliChatProvider),
    GrokCli(makepad_ai_hub::providers::grok_cli::GrokCliChatProvider),
}

impl ProviderAdapter {
    pub fn new(slug: &str, max_tokens: Option<u32>) -> Result<Self, String> {
        let p = match slug {
            "fleet-qwen" => {
                let bases = makepad_ai_hub::discovery::start_listener().nodes().into_iter().map(|n| n.base_url).collect();
                Self::Fleet(makepad_ai_hub::providers::qwen::FleetQwenChatProvider::new(
                    makepad_ai_hub::providers::qwen::HttpFleetTransport, bases,
                ).with_max_tokens(max_tokens).with_thinking(Some(false)))
            }
            "openai" => {
                let mut config = makepad_ai_hub::providers::responses::ResponsesConfig::openai_from_env();
                if let Some(n) = max_tokens { config = config.with_max_output_tokens(n); }
                Self::OpenAi(makepad_ai_hub::providers::responses::ResponsesChatProvider::new(config, makepad_ai_hub::providers::responses::BlockingTransport, None))
            }
            "grok" => {
                let mut config = makepad_ai_hub::providers::responses::ResponsesConfig::grok_from_env();
                if let Some(n) = max_tokens { config = config.with_max_output_tokens(n); }
                Self::Grok(makepad_ai_hub::providers::responses::ResponsesChatProvider::new(config, makepad_ai_hub::providers::responses::BlockingTransport, None))
            }
            "claude-cli" => Self::Claude(makepad_ai_hub::providers::claude::ClaudeCodeChatProvider::new(None)),
            "codex-cli" => Self::Codex(makepad_ai_hub::providers::codex_cli::CodexCliChatProvider::new(None)),
            "grok-cli" => Self::GrokCli(makepad_ai_hub::providers::grok_cli::GrokCliChatProvider::new(None)),
            _ => return Err(format!("unknown chat provider: {slug}")),
        };
        Ok(p)
    }
    fn provider(&mut self) -> &mut dyn ChatProvider {
        match self { Self::Fleet(p) => p, Self::OpenAi(p) => p, Self::Grok(p) => p, Self::Claude(p) => p, Self::Codex(p) => p, Self::GrokCli(p) => p }
    }
    pub fn availability(&mut self) -> ProviderAvailability { self.provider().availability() }
    pub fn begin(&mut self, system: &str, prompt: &str) -> Result<(), String> {
        self.provider().begin_turn(&TurnInput { system: system.into(), messages: vec![ChatMessage { role: ChatRole::User, text: prompt.into() }], tools_enabled: false, dynamic_context: String::new() })
    }
    pub fn poll(&mut self) -> Vec<ProviderEvent> { self.provider().poll() }
    pub fn cancel(&mut self) { self.provider().cancel() }
}

impl ProviderSession for ProviderAdapter {
    fn begin(&mut self, system: &str, prompt: &str) -> Result<(), String> { self.begin(system, prompt) }
    fn poll(&mut self) -> Vec<ProviderEvent> { self.poll() }
    fn cancel(&mut self) { self.cancel() }
    fn availability(&mut self) -> ProviderAvailability { self.availability() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_provider_slugs_are_exactly_the_six_sandbox_choices() {
        assert_eq!(PROVIDER_SLUGS, ["fleet-qwen", "openai", "grok", "claude-cli", "codex-cli", "grok-cli"]);
    }

    #[test]
    fn unknown_provider_is_explicitly_rejected() {
        assert!(ProviderAdapter::new("not-a-provider", None).is_err());
    }
}

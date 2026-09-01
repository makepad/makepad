//! OpenAI Responses chat provider (`https://api.openai.com/v1/responses`).
//!
//! Credentials come from `OPENAI_API_KEY`. The model defaults to
//! `gpt-5.6` and can be overridden with `MAKEPAD_CONTENT_CHAT_OPENAI_MODEL`.
//! Production constructors pin this origin. Tests inject a transport; they
//! cannot retarget this alias with a Grok config.

use crate::chat_wire::ProviderKind;
use crate::providers::responses::{
    ApiKey, BlockingTransport, ResponsesChatProvider, ResponsesConfig, ResponsesTransport,
};
use makepad_strict_json::Value;

pub const OPENAI_RESPONSES_URL: &str = "https://api.openai.com/v1/responses";
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-5.6";
pub const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";
pub const OPENAI_MODEL_ENV: &str = "MAKEPAD_CONTENT_CHAT_OPENAI_MODEL";

pub type OpenAiChatProvider<T = BlockingTransport> = ResponsesChatProvider<T>;

pub fn from_env(native_tools: Option<Value>) -> OpenAiChatProvider<BlockingTransport> {
    ResponsesChatProvider::new(ResponsesConfig::openai_from_env(), BlockingTransport, native_tools)
}

/// Production constructor. Origin is always [`OPENAI_RESPONSES_URL`].
pub fn new(
    api_key: ApiKey,
    model: impl Into<String>,
    native_tools: Option<Value>,
) -> OpenAiChatProvider<BlockingTransport> {
    ResponsesChatProvider::new(
        ResponsesConfig::openai(api_key, model),
        BlockingTransport,
        native_tools,
    )
}

/// Explicit transport constructor. Refuses a non-OpenAI config.
pub fn with_transport<T: ResponsesTransport>(
    config: ResponsesConfig,
    transport: T,
    native_tools: Option<Value>,
) -> Result<OpenAiChatProvider<T>, String> {
    if config.kind() != ProviderKind::OpenAi {
        return Err("openai provider cannot accept a different provider config".to_string());
    }
    Ok(ResponsesChatProvider::new(config, transport, native_tools))
}

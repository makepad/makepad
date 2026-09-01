//! xAI Grok Responses chat provider (`https://api.x.ai/v1/responses`).
//!
//! Credentials come from `XAI_API_KEY`. The model defaults to `grok-4.5`
//! and can be overridden with `MAKEPAD_CONTENT_CHAT_GROK_MODEL`. Production
//! constructors pin this origin. Tests inject a transport; they cannot
//! retarget this alias with an OpenAI config.

use crate::chat_wire::ProviderKind;
use crate::providers::responses::{
    ApiKey, BlockingTransport, ResponsesChatProvider, ResponsesConfig, ResponsesTransport,
};
use makepad_strict_json::Value;

pub const GROK_RESPONSES_URL: &str = "https://api.x.ai/v1/responses";
pub const DEFAULT_GROK_MODEL: &str = "grok-4.5";
pub const GROK_API_KEY_ENV: &str = "XAI_API_KEY";
pub const GROK_MODEL_ENV: &str = "MAKEPAD_CONTENT_CHAT_GROK_MODEL";

pub type GrokChatProvider<T = BlockingTransport> = ResponsesChatProvider<T>;

pub fn from_env(native_tools: Option<Value>) -> GrokChatProvider<BlockingTransport> {
    ResponsesChatProvider::new(ResponsesConfig::grok_from_env(), BlockingTransport, native_tools)
}

/// Production constructor. Origin is always [`GROK_RESPONSES_URL`].
pub fn new(
    api_key: ApiKey,
    model: impl Into<String>,
    native_tools: Option<Value>,
) -> GrokChatProvider<BlockingTransport> {
    ResponsesChatProvider::new(
        ResponsesConfig::grok(api_key, model),
        BlockingTransport,
        native_tools,
    )
}

/// Explicit transport constructor. Refuses a non-Grok config.
pub fn with_transport<T: ResponsesTransport>(
    config: ResponsesConfig,
    transport: T,
    native_tools: Option<Value>,
) -> Result<GrokChatProvider<T>, String> {
    if config.kind() != ProviderKind::Grok {
        return Err("grok provider cannot accept a different provider config".to_string());
    }
    Ok(ResponsesChatProvider::new(config, transport, native_tools))
}

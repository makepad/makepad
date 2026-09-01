//! Route's Claude escalation/dispatcher agent over the hub's Messages-API
//! provider — the Cx-driven `makepad_ai::ClaudeBackend`'s replacement.
//!
//! The provider (`makepad_ai_hub::providers::claude_api`) runs each turn on
//! its own thread and queues events; this adapter is stateful where the
//! provider is per-turn: it owns the conversation history, injects it into
//! every `TurnInput`, and maps provider events onto the seam's
//! [`AgentEvent`]s. Route's frame loop pumps `handle_event` continuously
//! (it is a map renderer), so poll-driven streaming reads as streaming.

use makepad_converse::agent_seam::{
    Agent, AgentEvent, Message, MessageRole, PromptId, SessionConfig, SessionId,
    StopReason, ToolDefinition,
};
use makepad_ai_hub::chat_wire::{ChatMessage, ChatRole};
use makepad_ai_hub::providers::claude_api::{ClaudeApiChatProvider, ClaudeApiConfig, ClaudeAuth};
use makepad_ai_hub::providers::provider::{ChatProvider, ProviderEvent, TurnInput};
use makepad_strict_json as json;
use makepad_widgets::*;

/// Anthropic tool schema array from the seam's tool definitions:
/// `[{name, description, input_schema}, …]`, parameters passed through as
/// the already-JSON schema each tool declares.
fn native_tools_payload(tools: &[ToolDefinition]) -> Option<json::Value> {
    if tools.is_empty() {
        return None;
    }
    let mut arr = Vec::new();
    for tool in tools {
        let schema = json::parse(tool.parameters.as_bytes())
            .unwrap_or(json::Value::Obj(Vec::new()));
        arr.push(json::Value::Obj(vec![
            ("name".to_string(), json::Value::Str(tool.name.clone())),
            (
                "description".to_string(),
                json::Value::Str(tool.description.clone()),
            ),
            ("input_schema".to_string(), schema),
        ]));
    }
    Some(json::Value::Arr(arr))
}

pub struct ClaudeAgent {
    model: String,
    api_key: String,
    provider: Option<ClaudeApiChatProvider<
        makepad_ai_hub::providers::claude_api::BlockingClaudeTransport,
    >>,
    session_id: Option<SessionId>,
    system_prompt: String,
    history: Vec<ChatMessage>,
    current_prompt: Option<PromptId>,
    /// Streamed text of the running turn, so history gets the full reply.
    turn_text: String,
    /// Events minted on the UI side (session ready, submit errors) that the
    /// next handle_event delivers alongside the provider's.
    queued: Vec<AgentEvent>,
}

impl ClaudeAgent {
    pub fn new(model: String, api_key: String) -> Self {
        Self {
            model,
            api_key,
            provider: None,
            session_id: None,
            system_prompt: String::new(),
            history: Vec::new(),
            current_prompt: None,
            turn_text: String::new(),
            queued: Vec::new(),
        }
    }
}

impl Agent for ClaudeAgent {
    fn create_session(&mut self, _cx: &mut Cx, config: SessionConfig) -> SessionId {
        let session_id = SessionId::new();
        self.system_prompt = config.system_prompt.unwrap_or_default();
        // Route holds the key already (read_secret at init); build the auth
        // directly rather than re-reading env. ClaudeCli is the nearest kind
        // label — kind is presentational on this wire.
        let api = match ClaudeAuth::api_key(self.api_key.clone()) {
            Ok(auth) => ClaudeApiConfig::new(
                makepad_ai_hub::chat_wire::ProviderKind::ClaudeCli,
                auth,
                self.model.clone(),
            ),
            Err(reason) => {
                self.queued.push(AgentEvent::SessionError {
                    session_id,
                    error: reason,
                });
                self.session_id = Some(session_id);
                return session_id;
            }
        };
        let tools = native_tools_payload(&config.tools);
        self.provider = Some(ClaudeApiChatProvider::new(api, tools));
        self.session_id = Some(session_id);
        self.queued.push(AgentEvent::SessionReady { session_id });
        session_id
    }

    fn send_prompt(&mut self, _cx: &mut Cx, _session_id: SessionId, text: &str) -> PromptId {
        let prompt_id = PromptId::new();
        self.history.push(ChatMessage::new(ChatRole::User, text));
        self.turn_text.clear();
        let input = TurnInput::new(self.system_prompt.clone(), self.history.clone());
        if let Some(provider) = &mut self.provider {
            if let Err(error) = provider.begin_turn(&input) {
                self.queued.push(AgentEvent::PromptError { prompt_id, error });
                return prompt_id;
            }
        } else {
            self.queued.push(AgentEvent::PromptError {
                prompt_id,
                error: "no session".to_string(),
            });
            return prompt_id;
        }
        self.current_prompt = Some(prompt_id);
        prompt_id
    }

    fn send_tool_result(
        &mut self,
        _cx: &mut Cx,
        _session_id: SessionId,
        tool_use_id: &str,
        result: &str,
        is_error: bool,
    ) {
        let Some(provider) = &mut self.provider else {
            return;
        };
        let output = if is_error {
            format!("ERROR: {result}")
        } else {
            result.to_string()
        };
        if let Err(error) = provider.continue_function(tool_use_id, &output) {
            if let Some(prompt_id) = self.current_prompt {
                self.queued.push(AgentEvent::PromptError { prompt_id, error });
            }
        }
    }

    fn cancel_prompt(&mut self, _cx: &mut Cx, _prompt_id: PromptId) {
        if let Some(provider) = &mut self.provider {
            provider.cancel();
        }
        self.current_prompt = None;
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event) -> Vec<AgentEvent> {
        let mut out = std::mem::take(&mut self.queued);
        let Some(provider) = &mut self.provider else {
            return out;
        };
        let Some(prompt_id) = self.current_prompt else {
            // No turn in flight: drain silently so a late Done never leaks
            // into the next prompt.
            let _ = provider.poll();
            return out;
        };
        for event in provider.poll() {
            match event {
                ProviderEvent::Delta(text) => {
                    self.turn_text.push_str(&text);
                    out.push(AgentEvent::TextDelta { prompt_id, text });
                }
                ProviderEvent::Status { .. } | ProviderEvent::Serving(_) => {}
                ProviderEvent::FunctionCall { call_id, name, arguments } => {
                    out.push(AgentEvent::ToolRequest {
                        prompt_id,
                        tool_use_id: call_id,
                        tool_name: name,
                        tool_input: arguments,
                    });
                }
                ProviderEvent::Done { text } => {
                    let full = if text.trim().is_empty() {
                        self.turn_text.clone()
                    } else {
                        text
                    };
                    if !full.trim().is_empty() {
                        self.history
                            .push(ChatMessage::new(ChatRole::Assistant, full));
                    }
                    self.current_prompt = None;
                    out.push(AgentEvent::TurnComplete {
                        prompt_id,
                        stop_reason: StopReason::EndTurn,
                    });
                }
                ProviderEvent::Error(error) => {
                    self.current_prompt = None;
                    out.push(AgentEvent::PromptError { prompt_id, error });
                }
            }
        }
        out
    }

    fn is_session_ready(&self, session_id: SessionId) -> bool {
        self.session_id == Some(session_id)
    }

    fn is_stateless(&self) -> bool {
        true
    }

    fn inject_history(&mut self, _session_id: SessionId, messages: Vec<Message>) {
        self.history = messages
            .iter()
            .map(|message| {
                let role = match message.role {
                    MessageRole::Assistant => ChatRole::Assistant,
                    _ => ChatRole::User,
                };
                ChatMessage::new(role, message.text())
            })
            .filter(|m| !m.text.trim().is_empty())
            .collect();
    }
}

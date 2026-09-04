//! Route's thin `agent_seam Agent` adapter to the shared local chat engine.

use makepad_converse::agent_seam::*;
use makepad_ai_hub::{
    hub::{AiHub, ChatConfig},
    hub_chat::HubChatSession,
    local_llm::{ChatEvent, LocalLlmConfig, ToolSpec},
};
use makepad_widgets::*;

use std::path::PathBuf;
use std::sync::Arc;

pub const DEFAULT_LOCAL_MODEL: &str = "local/models/Qwen3.5-9B-UD-Q4_K_XL.gguf";
const MAX_CONTEXT: u32 = 32768;
const MAX_NEW_TOKENS: usize = 768;
const MIN_REMAINING_CONTEXT: usize = 256;

pub struct LocalAgent {
    model_path: PathBuf,
    session: Option<HubChatSession>,
    session_id: Option<SessionId>,
    prompt_id: Option<PromptId>,
    ready: bool,
    /// Tool results the app still owes us for the current round, in order.
    awaiting_tools: usize,
    tool_results: Vec<(String, String, bool)>,
    next_tool_id: u64,
    timing: ToUISender<String>,
}

impl LocalAgent {
    pub fn new(model_path: String, timing: ToUISender<String>) -> Self {
        Self {
            model_path: model_path.into(),
            session: None,
            session_id: None,
            prompt_id: None,
            ready: false,
            awaiting_tools: 0,
            tool_results: Vec::new(),
            next_tool_id: 0,
            timing,
        }
    }

    fn set_timing(&self, text: String) {
        let _ = self.timing.send(text);
    }
}

impl Agent for LocalAgent {
    fn create_session(&mut self, _cx: &mut Cx, config: SessionConfig) -> SessionId {
        let session_id = SessionId::new();
        let mut llm = LocalLlmConfig::new(self.model_path.clone());
        llm.max_context = MAX_CONTEXT;
        llm.max_new_tokens = MAX_NEW_TOKENS;
        llm.min_remaining_context = MIN_REMAINING_CONTEXT;
        let tools = config
            .tools
            .into_iter()
            .map(|tool| ToolSpec::new(tool.name, tool.description, tool.parameters))
            .collect();
        self.session = Some(AiHub::in_process().start_local_chat(ChatConfig {
            llm,
            system_prompt: config.system_prompt.unwrap_or_default(),
            tools,
            wake: Some(Arc::new(SignalToUI::set_ui_signal)),
        }));
        self.session_id = Some(session_id);
        self.prompt_id = None;
        self.ready = false;
        self.awaiting_tools = 0;
        self.tool_results.clear();
        session_id
    }

    fn send_prompt(&mut self, _cx: &mut Cx, _session_id: SessionId, text: &str) -> PromptId {
        let prompt_id = PromptId::new();
        self.prompt_id = Some(prompt_id);
        self.awaiting_tools = 0;
        self.tool_results.clear();
        if let Some(session) = &self.session {
            session.send_user_turn(text.to_string());
        }
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
        if self.awaiting_tools == 0 {
            return;
        }
        self.tool_results
            .push((tool_use_id.to_string(), result.to_string(), is_error));
        if self.tool_results.len() >= self.awaiting_tools {
            self.awaiting_tools = 0;
            let results = self
                .tool_results
                .drain(..)
                .map(|(_, result, is_error)| (result, is_error))
                .collect();
            if let Some(session) = &self.session {
                session.send_tool_results(results);
            }
        }
    }

    fn cancel_prompt(&mut self, _cx: &mut Cx, _prompt_id: PromptId) {
        if let Some(session) = &self.session {
            session.cancel();
        }
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        let Some(session_id) = self.session_id else {
            return out;
        };
        let events = match &self.session {
            Some(session) => session.poll(),
            None => return out,
        };
        let prompt_id = self.prompt_id.unwrap_or_else(PromptId::new);
        for event in events {
            match event {
                ChatEvent::Loading { phase, fraction } => {
                    self.set_timing(format!(
                        "local model loading: {phase} {:.0}%",
                        fraction * 100.0
                    ));
                }
                ChatEvent::Ready {
                    prefill_tokens,
                    secs,
                } => {
                    self.ready = true;
                    self.set_timing(format!(
                        "local model ready: {prefill_tokens} tok prefix in {secs:.1}s"
                    ));
                    out.push(AgentEvent::SessionReady { session_id });
                }
                ChatEvent::Failed(error) => {
                    out.push(AgentEvent::SessionError { session_id, error });
                }
                ChatEvent::Delta(text) => {
                    out.push(AgentEvent::TextDelta { prompt_id, text });
                }
                ChatEvent::ToolCall { name, args } => {
                    self.next_tool_id += 1;
                    out.push(AgentEvent::ToolRequest {
                        prompt_id,
                        tool_use_id: format!("local_{}", self.next_tool_id),
                        tool_name: name,
                        tool_input: tool_args_json(args),
                    });
                }
                ChatEvent::TurnDone {
                    tool_calls,
                    tokens,
                    secs,
                    context_used,
                    context_max,
                } => {
                    self.awaiting_tools = tool_calls;
                    self.set_timing(format!(
                        "local: gen {tokens} tok {secs:.1}s ({:.1} tok/s) · ctx {context_used}/{context_max}",
                        tokens as f64 / secs.max(0.001),
                    ));
                    if tool_calls == 0 {
                        out.push(AgentEvent::TurnComplete {
                            prompt_id,
                            stop_reason: StopReason::EndTurn,
                        });
                    }
                }
                ChatEvent::ContextFull => {
                    out.push(AgentEvent::PromptError {
                        prompt_id,
                        error: "local context window is full — restart the app to reset the conversation"
                            .into(),
                    });
                }
            }
        }
        out
    }

    fn is_session_ready(&self, _session_id: SessionId) -> bool {
        self.ready
    }
}

/// The shared engine exposes parsed key/value pairs; `agent_seam Agent`
/// carries one JSON object. Preserve Route's numeric/bool/object coercion so
/// its typed tool broker sees the same inputs as before.
fn tool_args_json(args: Vec<(String, String)>) -> String {
    let mut out = String::from("{");
    for (index, (key, value)) in args.into_iter().enumerate() {
        if index != 0 {
            out.push(',');
        }
        out.push_str(&serde_json::to_string(&key).unwrap_or_else(|_| "\"\"".into()));
        out.push(':');
        let encoded = match serde_json::from_str::<serde_json::Value>(&value) {
            Ok(parsed) if !parsed.is_string() => parsed.to_string(),
            _ => serde_json::to_string(&value).unwrap_or_else(|_| "\"\"".into()),
        };
        out.push_str(&encoded);
    }
    out.push('}');
    out
}

#[cfg(test)]
mod tests {
    use super::tool_args_json;

    #[test]
    fn hub_tool_args_become_typed_agent_json() {
        let args = vec![
            ("to".into(), "utrecht".into()),
            ("zoom".into(), "14".into()),
            ("on".into(), "true".into()),
        ];
        assert_eq!(
            tool_args_json(args),
            r#"{"to":"utrecht","zoom":14,"on":true}"#
        );
    }
}

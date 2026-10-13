//! The real models behind the [`Model`] seam.
//!
//! - [`NoModel`] — no backend at all: the panel, the services and the tool
//!   console all work, and the status line says plainly that nothing
//!   answers. The web demo ships with this until a WebGPU model exists.
//! - [`CliModel`] — the person's own Claude Code / Codex CLIs.
//! - `LocalModel` (`local_model.rs`, feature `localai`) — the machine's
//!   own model through the hub's chat session.
//! - [`ClaudeModel`] — the Anthropic Messages API provider from the hub.
//!   The provider is per-turn and this adapter owns the history, so a
//!   tool change just rebuilds the provider with the new native tool
//!   array; the conversation carries on.
//!
//! Every adapter turns provider-specific tool-call shapes into one JSON
//! object as text, which is what the engine core routes on.

use crate::engine::cli_model::{CliKind, CliModel};
use crate::engine::{Model, ModelEvent, ToolDefinition};
use crate::provider::CLAUDE_DESKTOP;
use crate::state::{ProviderChoice, ProviderRow};
use makepad_ai_hub::chat_wire::{ChatMessage, ChatRole, ProviderKind};
use makepad_ai_hub::providers::claude_api::{BlockingClaudeTransport, ClaudeApiChatProvider, ClaudeApiConfig, ClaudeAuth};
use makepad_ai_hub::providers::provider::{ChatProvider, ProviderEvent, TurnInput};
use makepad_strict_json as json;

pub use super::local_model::{hub_generation, local_model_path, local_model_path_in, LocalModel, DEFAULT_LOCAL_MODEL, LOCAL_MODEL_ENV};

/// The weights this build can run in-process: the machine's own model with
/// the `localai` runtime, none without (the hub chat then only routes to a
/// node on this machine or the LAN).
fn local_weights() -> Option<std::path::PathBuf> {
    #[cfg(feature = "localai")]
    {
        local_model_path()
    }
    #[cfg(not(feature = "localai"))]
    {
        None
    }
}

/// The hub row alone, for a panel refreshing its menu when discovery lands.
pub fn hub_row() -> ProviderRow {
    super::local_model::local_row(local_weights().as_deref(), cfg!(feature = "localai"))
}

/// The Anthropic model the cloud adapter asks for.
pub const DEFAULT_CLAUDE_MODEL: &str = "claude-sonnet-5";

/// The rows the provider chip offers, with honest availability.
pub fn provider_rows(local_only: bool) -> Vec<ProviderRow> {
    let mut rows = Vec::new();
    // The AI hub is always listed: the hub's election finds the model where
    // it is resident — a node on this machine, the fleet's chat box, or
    // (with local AI built in) the weights here — and the row says honestly
    // when none of those answers.
    rows.push(hub_row());
    let claude_unavailable = if local_only {
        Some("local AI only".into())
    } else if claude_key().is_none() {
        Some("no API key".into())
    } else {
        None
    };
    rows.push(ProviderRow {
        choice: ProviderChoice::Cloud("claude-api".into()),
        label: format!("Claude API · {DEFAULT_CLAUDE_MODEL}"),
        unavailable: claude_unavailable,
    });
    // The person's own logged-in CLIs on this machine.
    for kind in [CliKind::ClaudeCode, CliKind::Codex] {
        let unavailable = if local_only { Some("local AI only".into()) } else { kind.unavailable() };
        rows.push(ProviderRow { choice: ProviderChoice::Cloud(kind.slug().into()), label: kind.label().into(), unavailable });
    }
    // The desktop assistant drives this app's tools through makepad-remote;
    // the conversation is typed there.
    rows.push(ProviderRow {
        choice: ProviderChoice::Cloud(CLAUDE_DESKTOP.into()),
        label: "Claude Desktop (MCP)".into(),
        unavailable: if local_only {
            Some("local AI only".into())
        } else if !crate::mcp::mcpb::claude_desktop_installed() {
            Some("not installed".into())
        } else {
            None
        },
    });
    rows.push(ProviderRow { choice: ProviderChoice::Cloud("none".into()), label: "Tools only".into(), unavailable: None });
    rows
}

fn claude_key() -> Option<String> {
    std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.trim().is_empty())
        .or_else(|| std::env::var("CLAUDE_CODE_OAUTH_TOKEN").ok().filter(|k| !k.trim().is_empty()))
}

/// What a panel starts on instead of the default Local: in a build without
/// `localai` the hub is only a LAN node that discovery may not have found
/// yet, so the start is the first provider that can answer here — Claude
/// Code, then Codex, when installed; else Claude Desktop; else the Claude
/// API when a key is set; else no model. The AI hub stays a menu pick.
/// `None` when the choice stands. The caller leaves the saved choice as it
/// is, so a `localai` build still starts on Local.
pub fn fallback_choice(choice: &ProviderChoice) -> Option<ProviderChoice> {
    #[cfg(feature = "localai")]
    {
        let _ = choice;
        None
    }
    #[cfg(not(feature = "localai"))]
    {
        if !choice.is_local() {
            return None;
        }
        let slug = [CliKind::ClaudeCode, CliKind::Codex]
            .into_iter()
            .find(|kind| kind.unavailable().is_none())
            .map(|kind| kind.slug())
            .or_else(|| crate::mcp::mcpb::claude_desktop_installed().then_some(CLAUDE_DESKTOP))
            .or_else(|| claude_key().map(|_| "claude-api"))
            .unwrap_or("none");
        Some(ProviderChoice::Cloud(slug.into()))
    }
}

/// Build the model for a choice, honestly: `Err` names why it cannot be.
pub fn build_model(choice: &ProviderChoice, local_only: bool) -> Result<Box<dyn Model>, String> {
    match choice {
        // With or without weights on this machine: the election decides
        // where the model runs, and the first line says so if nowhere.
        ProviderChoice::Local => Ok(Box::new(LocalModel::new(local_weights()))),
        ProviderChoice::Cloud(slug) if slug == "none" => Ok(Box::new(NoModel)),
        ProviderChoice::Cloud(_) if local_only => Err("local AI only".into()),
        ProviderChoice::Cloud(slug) if slug == CLAUDE_DESKTOP => {
            if crate::mcp::mcpb::claude_desktop_installed() {
                Ok(Box::new(ClaudeDesktopModel::new()))
            } else {
                Err("not installed".into())
            }
        }
        ProviderChoice::Cloud(slug) if CliKind::from_slug(slug).is_some() => {
            let kind = CliKind::from_slug(slug).unwrap();
            match kind.unavailable() {
                Some(reason) => Err(reason),
                None => Ok(Box::new(CliModel::new(kind))),
            }
        }
        ProviderChoice::Cloud(slug) if slug == "claude-api" => {
            let key = claude_key().ok_or_else(|| "no API key".to_string())?;
            Ok(Box::new(ClaudeModel::new(DEFAULT_CLAUDE_MODEL, key)))
        }
        ProviderChoice::Cloud(slug) => Err(format!("provider '{slug}' is not wired yet")),
    }
}

// The no-answer models live in `no_model.rs`, outside this feature, so a
// build without a runtime still has one to hand the core.
pub use super::no_model::{ClaudeDesktopModel, NoModel, NoModelWithReason};

// -------------------------------------------------------------- ClaudeModel

/// Anthropic's Messages API through the hub's provider. Stateless per
/// turn: this adapter owns the history and resends it.
pub struct ClaudeModel {
    model: String,
    api_key: String,
    provider: Option<ClaudeApiChatProvider<BlockingClaudeTransport>>,
    system: String,
    history: Vec<ChatMessage>,
    in_turn: bool,
    turn_text: String,
    queued: Vec<ModelEvent>,
}

impl ClaudeModel {
    pub fn new(model: &str, api_key: String) -> Self {
        ClaudeModel {
            model: model.to_string(),
            api_key,
            provider: None,
            system: String::new(),
            history: Vec::new(),
            in_turn: false,
            turn_text: String::new(),
            queued: Vec::new(),
        }
    }

    /// Anthropic's tool array: `[{name, description, input_schema}, …]`.
    /// Dots are not allowed in Anthropic tool names, so the api form is
    /// sent and mapped back on the way in.
    fn native_tools(tools: &[ToolDefinition]) -> Option<json::Value> {
        if tools.is_empty() {
            return None;
        }
        let arr = tools
            .iter()
            .map(|t| {
                let schema = json::parse(t.parameters.as_bytes()).unwrap_or(json::Value::Obj(Vec::new()));
                json::Value::Obj(vec![
                    ("name".to_string(), json::Value::Str(t.name.replace('.', "__"))),
                    ("description".to_string(), json::Value::Str(t.description.clone())),
                    ("input_schema".to_string(), schema),
                ])
            })
            .collect();
        Some(json::Value::Arr(arr))
    }
}

impl Model for ClaudeModel {
    fn label(&self) -> String {
        format!("Claude API · {}", self.model)
    }

    fn configure(&mut self, system: &str, tools: &[ToolDefinition]) -> Result<(), String> {
        self.system = system.to_string();
        let auth = ClaudeAuth::api_key(self.api_key.clone())?;
        let api = ClaudeApiConfig::new(ProviderKind::ClaudeCli, auth, self.model.clone());
        self.provider = Some(ClaudeApiChatProvider::new(api, Self::native_tools(tools)));
        Ok(())
    }

    fn send_user(&mut self, text: &str, dynamic_context: &str) {
        self.history.push(ChatMessage::new(ChatRole::User, text));
        self.turn_text.clear();
        let mut input = TurnInput::new(self.system.clone(), self.history.clone());
        input.dynamic_context = dynamic_context.to_string();
        let Some(provider) = &mut self.provider else {
            self.queued.push(ModelEvent::Error("the cloud API is not configured".into()));
            return;
        };
        match provider.begin_turn(&input) {
            Ok(()) => self.in_turn = true,
            Err(e) => self.queued.push(ModelEvent::Error(e)),
        }
    }

    fn send_tool_result(&mut self, call_id: &str, text: &str, is_error: bool) {
        let Some(provider) = &mut self.provider else { return };
        let output = if is_error { format!("ERROR: {text}") } else { text.to_string() };
        if let Err(e) = provider.continue_function(call_id, &output) {
            self.queued.push(ModelEvent::Error(e));
        }
    }

    fn cancel(&mut self) {
        if let Some(provider) = &mut self.provider {
            provider.cancel();
        }
        self.in_turn = false;
    }

    fn reset(&mut self) {
        self.cancel();
        if let Some(provider) = &mut self.provider {
            provider.reset_conversation();
        }
        self.history.clear();
        self.turn_text.clear();
    }

    fn poll(&mut self) -> Vec<ModelEvent> {
        let mut out = std::mem::take(&mut self.queued);
        let Some(provider) = &mut self.provider else { return out };
        if !self.in_turn {
            let _ = provider.poll();
            return out;
        }
        for event in provider.poll() {
            match event {
                ProviderEvent::Delta(text) => {
                    self.turn_text.push_str(&text);
                    out.push(ModelEvent::Delta(text));
                }
                ProviderEvent::Status { .. } | ProviderEvent::Serving(_) => {}
                ProviderEvent::FunctionCallPartial { call_id, name, partial_json } => {
                    out.push(ModelEvent::ToolCallPartial { call_id, name: name.replace("__", "."), partial_json });
                }
                ProviderEvent::FunctionCall { call_id, name, arguments } => {
                    out.push(ModelEvent::ToolCall { call_id, name: name.replace("__", "."), args: arguments });
                }
                ProviderEvent::Done { text } => {
                    let full = if text.trim().is_empty() { self.turn_text.clone() } else { text };
                    if !full.trim().is_empty() {
                        self.history.push(ChatMessage::new(ChatRole::Assistant, full));
                    }
                    self.in_turn = false;
                    out.push(ModelEvent::TurnDone { tool_calls: 0 });
                }
                ProviderEvent::Error(e) => {
                    self.in_turn = false;
                    out.push(ModelEvent::Error(e));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_tool_names_lose_their_dots_on_the_wire() {
        let tools = vec![ToolDefinition { name: "route.plan".into(), description: "Plan.".into(), parameters: r#"{"type":"object"}"#.into() }];
        let payload = ClaudeModel::native_tools(&tools).unwrap().to_json();
        assert!(payload.contains("route__plan") && !payload.contains("route.plan"));
    }
}

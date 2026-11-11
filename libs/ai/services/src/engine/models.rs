//! The real models behind the [`Model`] seam.
//!
//! - [`NoModel`] — no backend at all: the panel, the services and the tool
//!   console all work, and the status line says plainly that nothing
//!   answers. The web demo ships with this until a WebGPU model exists.
//! - [`CliModel`] — the person's own Claude Code / Codex CLIs.
//! - `LocalModel` (`local_model.rs`, feature `localai`) — the machine's
//!   own model through the hub's chat session.
//! - [`ClaudeModel`] — the Anthropic Messages API provider from the hub.
//!   The provider retains the signed transcript. This adapter owns visible
//!   history, so a tool change rebuilds the provider with the new native tool
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

/// The hub rows alone (the election, then each other model the hubs
/// serve), for a panel refreshing its menu when discovery lands.
pub fn hub_rows(local_only: bool) -> Vec<ProviderRow> {
    super::local_model::hub_rows(local_weights().as_deref(), cfg!(feature = "localai"), local_only)
}

/// The shared Claude model list: the hub owns it, since its Claude refuses any model not on it.
pub use makepad_ai_hub::claude_cli::CLAUDE_MODELS;
pub const DEFAULT_CLAUDE_MODEL: &str = CLAUDE_MODELS[2].0;
/// A chat turn's output room: these models always think, and the thinking
/// counts against it. All three stream up to 128K.
const CHAT_MAX_TOKENS: u32 = 64_000;

pub(super) fn claude_model_label(model: &str) -> &str {
    CLAUDE_MODELS.iter().find(|(id, _)| *id == model).map(|(_, label)| *label).unwrap_or(model)
}

/// A cloud choice's provider and model: `claude-cli:<id>` and
/// `claude-api:<id>` name one of [`CLAUDE_MODELS`]; a bare slug is the
/// provider's default (the CLI's own, the API's [`DEFAULT_CLAUDE_MODEL`]).
pub(super) fn cloud_selection(slug: &str) -> (&str, Option<&str>) {
    let (base, model) = slug.split_once(':').map_or((slug, None), |(base, model)| (base, Some(model)));
    let model = model.filter(|model| {
        (base == "claude-api" || CliKind::from_slug(base) == Some(CliKind::ClaudeCode))
            && CLAUDE_MODELS.iter().any(|(id, _)| id == model)
    });
    (base, model)
}

/// The rows the provider chip offers, with honest availability.
pub fn provider_rows(local_only: bool) -> Vec<ProviderRow> {
    let mut rows = Vec::new();
    // The AI hub is always listed: the hub's election finds the model where
    // it is resident — a node on this machine, the fleet's chat box, or
    // (with local AI built in) the weights here — and the row says honestly
    // when none of those answers. Every other model the hubs serve (the
    // hub's own Claude Code among them) is a row of its own.
    rows.extend(hub_rows(local_only));
    // The Claude API has no menu rows; `claude-api[:<id>]` still resolves as a
    // saved pick or a launch override (`build_model`).
    // The person's own logged-in CLIs, run on this machine by this app.
    for kind in [CliKind::ClaudeCode, CliKind::Codex] {
        let unavailable = if local_only { Some("local AI only".into()) } else { kind.unavailable() };
        rows.push(ProviderRow { choice: ProviderChoice::Cloud(kind.slug().into()), label: kind.label().into(), unavailable: unavailable.clone() });
        if kind == CliKind::ClaudeCode {
            for &(id, label) in CLAUDE_MODELS {
                rows.push(ProviderRow {
                    choice: ProviderChoice::Cloud(format!("{}:{id}", kind.slug())),
                    label: format!("{} · {label}", kind.label()),
                    unavailable: unavailable.clone(),
                });
            }
        }
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

fn claude_auth_from_values(api_key: Option<&str>, oauth_token: Option<&str>) -> Option<ClaudeAuth> {
    if let Some(key) = api_key.filter(|value| !value.trim().is_empty()) {
        ClaudeAuth::api_key(key).ok()
    } else {
        oauth_token.filter(|value| !value.trim().is_empty())
            .and_then(|token| ClaudeAuth::oauth_token(token).ok())
    }
}

fn claude_key() -> Option<ClaudeAuth> {
    claude_auth_from_values(
        std::env::var("ANTHROPIC_API_KEY").ok().as_deref(),
        std::env::var("CLAUDE_CODE_OAUTH_TOKEN").ok().as_deref(),
    )
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
        // A named hub model stands: it is a menu pick of a hub that answered.
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
        // The hub's Claude runs at Anthropic: refused under the lock.
        ProviderChoice::Hub(_) if local_only && choice.is_cloud() => Err("local AI only".into()),
        ProviderChoice::Hub(model) => Ok(Box::new(LocalModel::for_hub_model(model))),
        ProviderChoice::Cloud(slug) if slug == "none" => Ok(Box::new(NoModel::default())),
        ProviderChoice::Cloud(_) if local_only => Err("local AI only".into()),
        ProviderChoice::Cloud(slug) if slug == CLAUDE_DESKTOP => {
            if crate::mcp::mcpb::claude_desktop_installed() {
                Ok(Box::new(ClaudeDesktopModel::new()))
            } else {
                Err("not installed".into())
            }
        }
        ProviderChoice::Cloud(slug) => {
            let (base, model) = cloud_selection(slug);
            if let (None, Some((_, ignored))) = (model, slug.split_once(':')) {
                eprintln!("[ai] ignoring model suffix {ignored:?} for provider {base:?}; using the provider default");
            }
            if let Some(kind) = CliKind::from_slug(base) {
                match kind.unavailable() {
                    Some(reason) => Err(reason),
                    None => Ok(Box::new(CliModel::with_model(kind, model))),
                }
            } else if base == "claude-api" {
                let key = claude_key().ok_or_else(|| "no API key".to_string())?;
                Ok(Box::new(ClaudeModel::new(model.unwrap_or(DEFAULT_CLAUDE_MODEL), key)))
            } else {
                Err(format!("provider '{slug}' is not wired yet"))
            }
        }
    }
}

// The no-answer models live in `no_model.rs`, outside this feature, so a
// build without a runtime still has one to hand the core.
pub use super::no_model::{ClaudeDesktopModel, NoModel, NoModelWithReason};

// -------------------------------------------------------------- ClaudeModel

/// Anthropic's Messages API through the hub's provider. The adapter keeps
/// visible history; the provider retains the full signed transcript.
pub struct ClaudeModel {
    model: String,
    auth: ClaudeAuth,
    provider: Option<ClaudeApiChatProvider<BlockingClaudeTransport>>,
    system: String,
    history: Vec<ChatMessage>,
    in_turn: bool,
    turn_text: String,
    queued: Vec<ModelEvent>,
}

impl ClaudeModel {
    pub fn new(model: &str, auth: ClaudeAuth) -> Self {
        ClaudeModel {
            model: model.to_string(),
            auth,
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
        format!("Claude API · {}", claude_model_label(&self.model))
    }

    fn configure(&mut self, system: &str, tools: &[ToolDefinition]) -> Result<(), String> {
        self.system = system.to_string();
        let auth = self.auth.clone();
        let api = ClaudeApiConfig::new(ProviderKind::ClaudeCli, auth, self.model.clone()).with_max_tokens(CHAT_MAX_TOKENS);
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
    fn saved_model_choices_resolve_and_keep_their_menu_rows() {
        assert_eq!(DEFAULT_CLAUDE_MODEL, makepad_ai_hub::providers::claude_api::DEFAULT_CLAUDE_MODEL);
        let rows = provider_rows(true);
        assert!(rows.iter().all(|r| !r.choice.slug().starts_with("claude-api")), "the menu offers no Claude API rows");
        let cli: Vec<_> = rows.iter().filter(|r| r.choice.slug().starts_with("claude-cli")).collect();
        assert_eq!(cli.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), [
            "Claude Code (this machine)", "Claude Code (this machine) · Fable 5.2",
            "Claude Code (this machine) · Opus 5.5", "Claude Code (this machine) · Sonnet 5.5",
        ]);
        assert!(cli.iter().all(|r| r.unavailable.as_deref() == Some("local AI only")));
        for &(id, label) in CLAUDE_MODELS {
            for base in ["claude-cli", "claude-api"] {
                let slug = if base == "claude-api" && id == DEFAULT_CLAUDE_MODEL { base.to_string() } else { format!("{base}:{id}") };
                let choice = ProviderChoice::Cloud(slug.clone());
                assert_eq!(ProviderChoice::from_slug(&choice.slug()), choice);
                let (resolved_base, model) = cloud_selection(&slug);
                assert_eq!((resolved_base, model.unwrap_or(DEFAULT_CLAUDE_MODEL)), (base, id));
                if base == "claude-cli" {
                    let row = rows.iter().find(|r| r.choice == choice).expect("the saved slug has its menu row");
                    assert!(row.label.ends_with(label));
                    assert_eq!(row.unavailable.as_deref(), Some("local AI only"));
                }
                assert!(build_model(&choice, true).is_err(), "suffix cannot bypass local-only lock");
                if base == "claude-cli" {
                    let kind = CliKind::from_slug(resolved_base).unwrap();
                    assert_eq!(kind, CliKind::ClaudeCode);
                    let mut cli = CliModel::with_model(kind, model);
                    cli.reset();
                    assert_eq!(cli.label(), format!("Claude Code (this machine) · {label}"));
                    // The same provider factory CliModel uses, without launching a CLI.
                    let args = CliModel::claude_provider(model).arguments("");
                    assert!(args.windows(2).any(|w| w[0] == "--model" && w[1] == id));
                } else {
                    let mut api = ClaudeModel::new(model.unwrap_or(DEFAULT_CLAUDE_MODEL), ClaudeAuth::api_key("opaque-test-key").unwrap());
                    api.configure("", &[]).unwrap();
                    assert_eq!(api.provider.as_mut().unwrap().availability(),
                        makepad_ai_hub::chat_wire::ProviderAvailability::Available {
                            model: id.into(), detail: "api.anthropic.com".into(),
                        });
                    assert_eq!(api.label(), format!("Claude API · {label}"));
                }
            }
        }
        for base in ["claude-cli", "claude-api", "codex-cli"] {
            let choice = ProviderChoice::Cloud(base.into());
            assert_eq!(ProviderChoice::from_slug(&choice.slug()), choice);
            assert_eq!(cloud_selection(base), (base, None));
            assert_eq!(cloud_selection(&format!("{base}:unknown")), (base, None));
        }
        assert_eq!(cloud_selection("codex-cli:claude-fable-5-2"), ("codex-cli", None));
        assert!(!CliModel::claude_provider(None).arguments("").iter().any(|arg| arg == "--model"));
    }

    #[test]
    fn claude_env_values_preserve_auth_kind_and_key_precedence() {
        for (key, token, oauth) in [
            (None, None, None), (Some(""), Some("  "), None),
            (Some("key"), None, Some(false)), (None, Some("token"), Some(true)),
            (Some("key"), Some("token"), Some(false)), (Some("  "), Some("token"), Some(true)),
            (Some("invalid\nkey"), Some("token"), None), (None, Some("invalid\ntoken"), None),
        ] {
            let auth = claude_auth_from_values(key, token);
            assert_eq!(auth.as_ref().map(ClaudeAuth::is_oauth), oauth);
            if let Some(auth) = auth {
                let mut model = ClaudeModel::new(DEFAULT_CLAUDE_MODEL, auth);
                model.configure("system", &[]).unwrap();
                assert_eq!(model.auth.is_oauth(), oauth.unwrap());
            }
        }
    }

    #[test]
    fn claude_tool_names_lose_their_dots_on_the_wire() {
        let tools = vec![ToolDefinition { name: "route.plan".into(), description: "Plan.".into(), parameters: r#"{"type":"object"}"#.into() }];
        let payload = ClaudeModel::native_tools(&tools).unwrap().to_json();
        assert!(payload.contains("route__plan") && !payload.contains("route.plan"));
    }
}

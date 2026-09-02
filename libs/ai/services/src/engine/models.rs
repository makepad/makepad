//! The real models behind the [`Model`] seam.
//!
//! - [`NoModel`] — no backend at all: the panel, the services and the tool
//!   console all work, and the status line says plainly that nothing
//!   answers. The web demo ships with this until a WebGPU model exists.
//! - [`LocalModel`] — the machine's own model through the hub's chat
//!   session (the residency election, then the in-process engine). Its
//!   tool table is prefilled into an append-only context, so a change of
//!   tools does not restart the session: the next turn carries a tool
//!   update block the model reads like any other message. A changed
//!   system prompt travels the same way.
//! - [`ClaudeModel`] — the Anthropic Messages API provider from the hub.
//!   The provider is per-turn and this adapter owns the history, so a
//!   tool change just rebuilds the provider with the new native tool
//!   array; the conversation carries on.
//!
//! Every adapter turns provider-specific tool-call shapes into one JSON
//! object as text, which is what the engine core routes on.

use crate::engine::{Model, ModelEvent, ToolDefinition};
use crate::state::{ProviderChoice, ProviderRow};
use makepad_ai_hub::chat_wire::{ChatMessage, ChatRole, ProviderKind};
use makepad_ai_hub::hub::{AiHub, ChatConfig};
use makepad_ai_hub::hub_chat::HubChatSession;
use makepad_ai_hub::local_llm::{ChatEvent, LocalLlmConfig, ToolSpec};
use makepad_ai_hub::providers::claude_api::{BlockingClaudeTransport, ClaudeApiChatProvider, ClaudeApiConfig, ClaudeAuth};
use makepad_ai_hub::providers::provider::{ChatProvider, ProviderEvent, TurnInput};
use makepad_platform::thread::SignalToUI;
use makepad_strict_json as json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Where the local weights live, relative to a checkout or the exe.
pub const DEFAULT_LOCAL_MODEL: &str = "local/models/Qwen3.5-9B-UD-Q4_K_XL.gguf";
/// The environment override for the local model file.
pub const LOCAL_MODEL_ENV: &str = "MAKEPAD_AI_CHAT_MODEL";
const MAX_CONTEXT: u32 = 32768;
const MAX_NEW_TOKENS: usize = 768;
const MIN_REMAINING_CONTEXT: usize = 256;
/// The Anthropic model the cloud adapter asks for.
pub const DEFAULT_CLAUDE_MODEL: &str = "claude-sonnet-5";

/// Where the local weights are, or `None` when this machine has none.
/// Independent of the working directory — an app launched from a copied
/// binary in some scratch folder must find the same weights as one run
/// from the checkout: the env override, then the makepad home's weights
/// directory (where the hub's registry installs models), then the
/// checkout the executable lives in, then the working directory last.
pub fn local_model_path() -> Option<PathBuf> {
    local_model_path_in(
        std::env::var_os(LOCAL_MODEL_ENV).map(PathBuf::from),
        &makepad_ai_hub::home::weights_dir(),
        std::env::current_exe().ok().as_deref(),
        std::env::current_dir().ok().as_deref(),
    )
}

/// [`local_model_path`] over explicit inputs, so the order is testable.
pub fn local_model_path_in(
    from_env: Option<PathBuf>,
    weights_dir: &Path,
    exe: Option<&Path>,
    cwd: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(path) = from_env {
        return path.is_file().then_some(path);
    }
    if let Some(found) = chat_gguf_under(weights_dir, 0) {
        return Some(found);
    }
    let relative = Path::new(DEFAULT_LOCAL_MODEL);
    if let Some(root) = exe.and_then(repo_root_of) {
        let candidate = root.join(relative);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    if let Some(cwd) = cwd {
        let candidate = cwd.join(relative);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// The checkout an executable runs out of (`target/<profile>/<bin>` under
/// a root with a `Cargo.toml` and a `local/` beside it), if any — the
/// same rule the window manager uses to decide it runs from a checkout.
fn repo_root_of(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .skip(1)
        .take(6)
        .find(|dir| dir.join("Cargo.toml").is_file() && dir.join("local").is_dir())
        .map(Path::to_path_buf)
}

/// How deep the weights directory is searched: the registry installs a
/// model as `<repo>/<file>`, so two levels reach every installed GGUF.
const WEIGHTS_SEARCH_DEPTH: usize = 2;

/// The best chat GGUF under `dir`: a Qwen chat model over any other, the
/// larger file over the smaller. Projector and OCR files are not chat
/// models and are never picked.
fn chat_gguf_under(dir: &Path, depth: usize) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(bool, u64, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < WEIGHTS_SEARCH_DEPTH {
                if let Some(found) = chat_gguf_under(&path, depth + 1) {
                    let size = std::fs::metadata(&found).map(|m| m.len()).unwrap_or(0);
                    consider(&mut best, found, size);
                }
            }
            continue;
        }
        if !is_chat_gguf(&path) {
            continue;
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        consider(&mut best, path, size);
    }
    best.map(|(_, _, path)| path)
}

fn consider(best: &mut Option<(bool, u64, PathBuf)>, path: PathBuf, size: u64) {
    let qwen = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.to_ascii_lowercase().contains("qwen"))
        .unwrap_or(false);
    let better = match best {
        None => true,
        Some((best_qwen, best_size, _)) => (qwen, size) > (*best_qwen, *best_size),
    };
    if better {
        *best = Some((qwen, size, path));
    }
}

fn is_chat_gguf(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else { return false };
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".gguf") && !lower.contains("mmproj") && !lower.contains("ocr")
}

/// The rows the provider chip offers, with honest availability.
pub fn provider_rows(local_only: bool) -> Vec<ProviderRow> {
    let mut rows = Vec::new();
    // Local is always a choice: the hub's election finds the model where
    // it is resident — a node on this machine, the fleet's chat box, or the
    // weights here — and says honestly when none of those answers.
    rows.push(ProviderRow {
        choice: ProviderChoice::Local,
        label: local_label(local_model_path().as_deref()),
        unavailable: None,
    });
    let claude_unavailable = if local_only {
        Some("Local AI only is on".into())
    } else if claude_key().is_none() {
        Some("no ANTHROPIC_API_KEY or CLAUDE_CODE_OAUTH_TOKEN".into())
    } else {
        None
    };
    rows.push(ProviderRow { choice: ProviderChoice::Cloud("claude-api".into()), label: "Claude (API)".into(), unavailable: claude_unavailable });
    rows.push(ProviderRow { choice: ProviderChoice::Cloud("none".into()), label: "No model (tools only)".into(), unavailable: None });
    rows
}

/// The provider chip's base label: the local weights when there are any.
fn local_label(path: Option<&Path>) -> String {
    match path {
        Some(p) => format!("Local · {}", model_short_name(p)),
        None => "Local".to_string(),
    }
}

fn model_short_name(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("model");
    // "Qwen3.5-9B-UD-Q4_K_XL" → "Qwen3.5 9B"
    let mut parts = stem.split('-');
    let family = parts.next().unwrap_or(stem);
    match parts.next() {
        Some(size) => format!("{family} {size}"),
        None => family.to_string(),
    }
}

fn claude_key() -> Option<String> {
    std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.trim().is_empty())
        .or_else(|| std::env::var("CLAUDE_CODE_OAUTH_TOKEN").ok().filter(|k| !k.trim().is_empty()))
}

/// Build the model for a choice, honestly: `Err` names why it cannot be.
pub fn build_model(choice: &ProviderChoice, local_only: bool) -> Result<Box<dyn Model>, String> {
    match choice {
        // With or without weights on this machine: the election decides
        // where the model runs, and the first line says so if nowhere.
        ProviderChoice::Local => Ok(Box::new(LocalModel::new(local_model_path()))),
        ProviderChoice::Cloud(slug) if slug == "none" => Ok(Box::new(NoModel)),
        ProviderChoice::Cloud(_) if local_only => Err("Local AI only is on".into()),
        ProviderChoice::Cloud(slug) if slug == "claude-api" || slug == "claude-cli" => {
            let key = claude_key().ok_or_else(|| "no ANTHROPIC_API_KEY or CLAUDE_CODE_OAUTH_TOKEN".to_string())?;
            Ok(Box::new(ClaudeModel::new(DEFAULT_CLAUDE_MODEL, key)))
        }
        ProviderChoice::Cloud(slug) => Err(format!("provider '{slug}' is not wired yet")),
    }
}

// The no-answer models live in `no_model.rs`, outside this feature, so a
// build without a runtime still has one to hand the core.
pub use super::no_model::{NoModel, NoModelWithReason};

// --------------------------------------------------------------- LocalModel

/// The machine's own model through the hub chat session.
pub struct LocalModel {
    /// The weights on this machine, when it has any. `None` is fine: the
    /// election still reaches a node here or on the fleet.
    model_path: Option<PathBuf>,
    session: Option<HubChatSession>,
    system: String,
    tools: Vec<ToolDefinition>,
    /// Tools/system changed after the session started: rendered into the
    /// next user turn as an update block.
    pending_update: Option<String>,
    /// Results the engine still owes for the current round, in call order.
    awaiting: usize,
    results: Vec<(String, bool)>,
    next_call: u64,
    label: String,
    queued: Vec<ModelEvent>,
}

impl LocalModel {
    pub fn new(model_path: Option<PathBuf>) -> Self {
        let label = local_label(model_path.as_deref());
        LocalModel {
            model_path,
            session: None,
            system: String::new(),
            tools: Vec::new(),
            pending_update: None,
            awaiting: 0,
            results: Vec::new(),
            next_call: 0,
            label,
            queued: Vec::new(),
        }
    }

    fn start_session(&mut self) {
        let mut llm = LocalLlmConfig::new(self.model_path.clone().unwrap_or_default());
        llm.max_context = MAX_CONTEXT;
        llm.max_new_tokens = MAX_NEW_TOKENS;
        llm.min_remaining_context = MIN_REMAINING_CONTEXT;
        let tools = self
            .tools
            .iter()
            .map(|t| ToolSpec::new(t.name.clone(), t.description.clone(), t.parameters.clone()))
            .collect();
        self.session = Some(AiHub::in_process().start_local_chat(ChatConfig {
            llm,
            system_prompt: self.system.clone(),
            tools,
            wake: Some(Arc::new(SignalToUI::set_ui_signal)),
        }));
        self.awaiting = 0;
        self.results.clear();
        self.pending_update = None;
    }

    /// The update block a running session reads when the table changed:
    /// the same one-JSON-line-per-tool shape its prefix used.
    fn render_update(system: &str, tools: &[ToolDefinition]) -> String {
        let mut out = String::from("[The application set changed. This replaces the earlier application list and tool table.]\n");
        out.push_str(system.trim());
        out.push_str("\n<tools>\n");
        for t in tools {
            out.push_str(&json::obj(vec![
                ("name", json::s(t.name.clone())),
                ("description", json::s(t.description.clone())),
                ("parameters", json::parse(t.parameters.as_bytes()).unwrap_or(json::Value::Obj(Vec::new()))),
            ])
            .to_json());
            out.push('\n');
        }
        out.push_str("</tools>\n");
        out
    }
}

impl Model for LocalModel {
    /// `Local · Qwen3.5 9B`, and once the election settled where the model
    /// runs: `Local · qwen3.8-27b on 10.0.0.165`, `Local · Qwen3.5 9B ·
    /// in-process`.
    fn label(&self) -> String {
        match self.session.as_ref().and_then(|s| s.route()) {
            Some(route) => format!("{} · {}", self.label, route.label()),
            None => self.label.clone(),
        }
    }

    fn configure(&mut self, system: &str, tools: &[ToolDefinition]) -> Result<(), String> {
        let changed = system != self.system || tools != self.tools.as_slice();
        self.system = system.to_string();
        self.tools = tools.to_vec();
        // No session yet: nothing to do — the weights load on the FIRST
        // user line (`send_user`), never on a registration. An app joining
        // the bus must not cost the machine a model load the person has
        // not asked for.
        match &self.session {
            None => {}
            Some(_) if changed => self.pending_update = Some(Self::render_update(system, tools)),
            Some(_) => {}
        }
        Ok(())
    }

    fn send_user(&mut self, text: &str, dynamic_context: &str) {
        if self.session.is_none() {
            // The first line of the conversation is what loads the model.
            self.start_session();
        }
        let Some(session) = &self.session else {
            self.queued.push(ModelEvent::Error("the local model is not loaded".into()));
            return;
        };
        let mut turn = String::new();
        if let Some(update) = self.pending_update.take() {
            turn.push_str(&update);
            turn.push('\n');
        }
        if !dynamic_context.trim().is_empty() {
            turn.push_str("[Current state of the applications]\n");
            turn.push_str(dynamic_context.trim());
            turn.push_str("\n\n");
        }
        turn.push_str(text);
        self.awaiting = 0;
        self.results.clear();
        session.send_user_turn(turn);
    }

    fn send_tool_result(&mut self, _call_id: &str, text: &str, is_error: bool) {
        if self.awaiting == 0 {
            return;
        }
        self.results.push((text.to_string(), is_error));
        if self.results.len() >= self.awaiting {
            self.awaiting = 0;
            if let Some(session) = &self.session {
                session.send_tool_results(self.results.drain(..).collect());
            }
        }
    }

    fn cancel(&mut self) {
        if let Some(session) = &self.session {
            session.cancel();
        }
        self.awaiting = 0;
        self.results.clear();
    }

    fn reset(&mut self) {
        // The context is append-only; a fresh conversation is a fresh
        // session (the hub election keeps the weights resident where it can).
        // A model never started stays unstarted: the next line starts it.
        if self.session.is_some() {
            self.start_session();
        }
    }

    fn poll(&mut self) -> Vec<ModelEvent> {
        let mut out = std::mem::take(&mut self.queued);
        let Some(session) = &self.session else { return out };
        for event in session.poll() {
            match event {
                ChatEvent::Loading { phase, fraction } => out.push(ModelEvent::Loading { phase, fraction }),
                ChatEvent::Ready { .. } => out.push(ModelEvent::Ready),
                ChatEvent::Failed(e) => out.push(ModelEvent::Error(e)),
                ChatEvent::Delta(text) => out.push(ModelEvent::Delta(text)),
                ChatEvent::ToolCall { name, args } => {
                    self.next_call += 1;
                    out.push(ModelEvent::ToolCall { call_id: format!("l{}", self.next_call), name, args: tool_args_json(args) });
                }
                ChatEvent::TurnDone { tool_calls, tokens, secs, .. } => {
                    self.awaiting = tool_calls;
                    if secs > 0.0 && tokens > 0 {
                        out.push(ModelEvent::Rate(tokens as f32 / secs as f32));
                    }
                    out.push(ModelEvent::TurnDone { tool_calls });
                }
                ChatEvent::ContextFull => out.push(ModelEvent::Error("the local context window is full — Clear starts a new conversation".into())),
            }
        }
        out
    }
}

/// The local engine hands back string key/value pairs; the core wants one
/// JSON object. Values that parse as JSON non-strings (numbers, bools,
/// objects) stay typed; everything else is a string.
fn tool_args_json(args: Vec<(String, String)>) -> String {
    let mut fields = Vec::with_capacity(args.len());
    for (key, value) in args {
        let v = match json::parse(value.as_bytes()) {
            Ok(json::Value::Str(_)) | Err(_) => json::Value::Str(value),
            Ok(parsed) => parsed,
        };
        fields.push((key, v));
    }
    json::Value::Obj(fields).to_json()
}

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
        "Claude".into()
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
            self.queued.push(ModelEvent::Error("Claude is not configured".into()));
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
    fn local_tool_args_become_typed_json() {
        let args = vec![("to".into(), "utrecht".into()), ("zoom".into(), "14".into()), ("on".into(), "true".into())];
        assert_eq!(tool_args_json(args), r#"{"to":"utrecht","zoom":14,"on":true}"#);
    }

    #[test]
    fn the_update_block_lists_every_tool_once() {
        let tools = vec![ToolDefinition { name: "files.stat".into(), description: "Stat.".into(), parameters: r#"{"type":"object"}"#.into() }];
        let block = LocalModel::render_update("doctrine", &tools);
        assert!(block.contains("<tools>") && block.contains("files.stat") && block.contains("doctrine"));
    }

    #[test]
    fn claude_tool_names_lose_their_dots_on_the_wire() {
        let tools = vec![ToolDefinition { name: "route.plan".into(), description: "Plan.".into(), parameters: r#"{"type":"object"}"#.into() }];
        let payload = ClaudeModel::native_tools(&tools).unwrap().to_json();
        assert!(payload.contains("route__plan") && !payload.contains("route.plan"));
    }

    #[test]
    fn model_names_read_well() {
        assert_eq!(model_short_name(Path::new("/x/Qwen3.5-9B-UD-Q4_K_XL.gguf")), "Qwen3.5 9B");
        assert_eq!(local_label(Some(Path::new("/x/Qwen3.5-9B-UD-Q4_K_XL.gguf"))), "Local · Qwen3.5 9B");
        assert_eq!(local_label(None), "Local");
    }

    /// A scratch tree: a fake makepad home with weights, a fake checkout
    /// with `local/models`, and a working directory that is neither.
    struct Tree {
        root: PathBuf,
    }

    impl Tree {
        fn new(name: &str) -> Tree {
            let root = std::env::temp_dir().join(format!("aichat-models-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Tree { root }
        }
        fn file(&self, rel: &str, bytes: usize) -> PathBuf {
            let path = self.root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, vec![0u8; bytes]).unwrap();
            path
        }
        fn dir(&self, rel: &str) -> PathBuf {
            let path = self.root.join(rel);
            std::fs::create_dir_all(&path).unwrap();
            path
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn the_weights_are_found_without_the_working_directory() {
        let t = Tree::new("lookup");
        let weights = t.dir("home/weights");
        let elsewhere = t.dir("elsewhere");
        // Nothing anywhere: none, whatever the cwd.
        assert_eq!(local_model_path_in(None, &weights, None, Some(&elsewhere)), None);
        // The registry's layout under the home: found from any cwd; the
        // projector beside it is never the answer, a Qwen beats a larger
        // other model, the larger Qwen beats the smaller.
        t.file("home/weights/some/Other-13B.gguf", 3000);
        t.file("home/weights/unsloth/Qwen3.5-9B-mmproj-F16.gguf", 4000);
        let small = t.file("home/weights/unsloth/Qwen3.5-4B-Q5.gguf", 1000);
        assert_eq!(local_model_path_in(None, &weights, None, Some(&elsewhere)), Some(small.clone()));
        let big = t.file("home/weights/unsloth/Qwen3.5-9B-UD-Q4_K_XL.gguf", 2000);
        assert_eq!(local_model_path_in(None, &weights, None, Some(&elsewhere)), Some(big));
        // A checkout found from the executable's place, no home weights.
        let t2 = Tree::new("checkout");
        let empty_weights = t2.dir("home/weights");
        t2.file("repo/Cargo.toml", 1);
        t2.dir("repo/local");
        let checkout_model = t2.file(&format!("repo/{DEFAULT_LOCAL_MODEL}"), 10);
        let exe = t2.file("repo/target/release/aichat", 1);
        assert_eq!(local_model_path_in(None, &empty_weights, Some(&exe), Some(&elsewhere)), Some(checkout_model.clone()));
        // A copied binary outside any checkout: the cwd is the last resort.
        let copied = t2.file("bin/aichat", 1);
        assert_eq!(local_model_path_in(None, &empty_weights, Some(&copied), Some(&elsewhere)), None);
        let repo = t2.root.join("repo");
        assert_eq!(local_model_path_in(None, &empty_weights, Some(&copied), Some(&repo)), Some(checkout_model));
        // The override wins when it points at a file, and answers none
        // (never a fallback) when it does not.
        let pinned = t2.file("pinned/m.gguf", 5);
        assert_eq!(local_model_path_in(Some(pinned.clone()), &weights, None, Some(&repo)), Some(pinned));
        assert_eq!(local_model_path_in(Some(t2.root.join("missing.gguf")), &weights, None, Some(&repo)), None);
    }
}

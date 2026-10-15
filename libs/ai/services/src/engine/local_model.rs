//! The AI hub through the hub chat session.
//!
//! [`LocalModel`] runs the residency election, then (only in a build with
//! the `localai` feature, which links the hub's in-process runtime) the
//! machine's own engine. Without `localai` the same session still reaches a
//! hub node on this machine or the fleet on the LAN: chatting through a
//! remote hub needs no model runtime.
//! Its tool table is prefilled into an append-only context, so a change of
//! tools does not restart the session: the next turn carries a tool update
//! block the model reads like any other message. A changed system prompt
//! travels the same way.

use crate::engine::{Model, ModelEvent, ModelImage, ToolDefinition};
use crate::state::{ProviderChoice, ProviderRow};
use makepad_ai_hub::hub::{AiHub, ChatConfig};
use makepad_ai_hub::hub_chat::{ChatRoute, HubChatSession};
use makepad_ai_hub::local_llm::{ChatEvent, LocalLlmConfig, ToolSpec};
use makepad_ai_hub::providers::provider::ToolImage;
use makepad_platform::thread::SignalToUI;
use makepad_strict_json as json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where the local weights live, relative to a checkout or the exe.
pub const DEFAULT_LOCAL_MODEL: &str = "local/models/Qwen3.5-9B-UD-Q4_K_XL.gguf";
/// The environment override for the local model file.
pub const LOCAL_MODEL_ENV: &str = "MAKEPAD_AI_CHAT_MODEL";
const MAX_CONTEXT: u32 = 32768;
const MAX_NEW_TOKENS: usize = 768;
const MIN_REMAINING_CONTEXT: usize = 256;

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

/// Folders of the weights directory that hold other domains' models (the
/// hub registry's `cache_as` roots): a GGUF in them is a text encoder,
/// projector or music/video part, never a chat model.
const NON_CHAT_DIRS: &[&str] = &[
    "video", "music", "stems", "stt", "tts", "depth", "loras", "checkpoints", "artifacts", "fleet",
    "triposplat", "trellis2", "paint21", "flux2-dev", "flux2-dev-q4-24g", "flux2-klein-4b",
];

/// The best chat GGUF under `dir`: a Qwen chat model over any other, the
/// larger file over the smaller. Projector and OCR files are not chat
/// models and are never picked.
fn chat_gguf_under(dir: &Path, depth: usize) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(bool, u64, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let skip = depth == 0 && path.file_name().and_then(|n| n.to_str()).is_some_and(|n| NON_CHAT_DIRS.contains(&n));
            if depth < WEIGHTS_SEARCH_DEPTH && !skip {
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

/// What discovery knows about a hub with a chat model on the LAN.
#[derive(Clone, Debug, PartialEq, Eq)]
enum HubFind {
    Idle,
    Searching,
    /// `AI hub · 149 (Qwen3.8 27B)`.
    Found(String),
    Missing { at: Instant },
}

static HUB_FIND: Mutex<HubFind> = Mutex::new(HubFind::Idle);
/// Bumped whenever the discovery answer changes, so a panel redraws its
/// menu only then.
static HUB_GENERATION: AtomicU64 = AtomicU64::new(0);
/// A miss is looked at again after this long, so a hub started later shows up.
const HUB_RESCAN: Duration = Duration::from_secs(15);

/// Moves when the hub row's answer does.
pub fn hub_generation() -> u64 {
    HUB_GENERATION.load(Ordering::Relaxed)
}

fn set_hub_find(find: HubFind) {
    let mut slot = HUB_FIND.lock().unwrap_or_else(|e| e.into_inner());
    if *slot != find {
        *slot = find;
        HUB_GENERATION.fetch_add(1, Ordering::Relaxed);
    }
}

/// The current discovery answer. The first call (and one per [`HUB_RESCAN`]
/// while nothing is found) starts a probe on its own thread: the same
/// discovery the chat election runs, which loads no model and sends no
/// prompt. The caller is never blocked; the UI is signalled when the answer
/// lands.
fn hub_find() -> HubFind {
    let mut slot = HUB_FIND.lock().unwrap_or_else(|e| e.into_inner());
    let start = match &*slot {
        HubFind::Idle => {
            *slot = HubFind::Searching;
            HUB_GENERATION.fetch_add(1, Ordering::Relaxed);
            true
        }
        HubFind::Missing { at } if at.elapsed() >= HUB_RESCAN => {
            // Keep reporting the miss while it looks again; no flapping.
            *slot = HubFind::Missing { at: Instant::now() };
            true
        }
        _ => false,
    };
    let now = slot.clone();
    drop(slot);
    if start {
        let spawned = std::thread::Builder::new().name("ai-hub-find".into()).spawn(|| {
            set_hub_find(match makepad_ai_hub::hub_chat::probe_fleet_chat() {
                Ok(route) => HubFind::Found(route_label(&route)),
                Err(_) => HubFind::Missing { at: Instant::now() },
            });
            SignalToUI::set_ui_signal();
        });
        if spawned.is_err() {
            set_hub_find(HubFind::Missing { at: Instant::now() });
        }
    }
    now
}

/// `AI hub · 149 (Qwen3.8 27B)` for an elected route.
fn route_label(route: &ChatRoute) -> String {
    let this_machine = if cfg!(target_os = "macos") { "this Mac" } else { "this machine" };
    match route {
        ChatRoute::Fleet { base, model } => {
            format!("AI hub · {} ({})", node_short_name(base), model_short_name(Path::new(model)))
        }
        ChatRoute::InProcess { model } => format!("AI hub · {this_machine} ({})", model_short_name(Path::new(model))),
        ChatRoute::MachineNode { .. } => format!("AI hub · {this_machine}"),
    }
}

/// The provider menu's hub row, listed in every build. A hub found on the
/// LAN names its node and model; otherwise it reads "searching…" while
/// discovery runs and "no hub found" after, unselectable, unless this build
/// has the in-process runtime (`weights` is then the machine's own model,
/// which can always answer).
pub(super) fn local_row(weights: Option<&Path>, has_runtime: bool) -> ProviderRow {
    let (label, unavailable) = match hub_find() {
        HubFind::Found(label) => (label, None),
        _ if has_runtime => (local_label(weights), None),
        HubFind::Missing { .. } => ("AI hub".to_string(), Some("no hub found".to_string())),
        HubFind::Idle | HubFind::Searching => ("AI hub".to_string(), Some("searching…".to_string())),
    };
    ProviderRow { choice: ProviderChoice::Local, label, unavailable }
}

/// The provider chip's base label: the local weights when there are any.
fn local_label(path: Option<&Path>) -> String {
    match path {
        Some(p) => format!("AI hub · {}", model_short_name(p)),
        None => "AI hub · auto".to_string(),
    }
}

fn model_short_name(path: &Path) -> String {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("model");
    // A registry id such as "qwen3.8-27b" has a dot but no file extension.
    let stem = match name.rsplit_once('.') {
        Some((stem, extension)) if extension.eq_ignore_ascii_case("gguf") => stem,
        _ => name,
    };
    // "Qwen3.5-9B-UD-Q4_K_XL" → "Qwen3.5 9B"
    let mut parts = stem.split('-');
    let family = parts.next().unwrap_or(stem);
    let family = match family.strip_prefix("qwen") {
        Some(version) => format!("Qwen{version}"),
        None => family.to_string(),
    };
    match parts.next() {
        Some(size) => format!("{family} {}", size.to_ascii_uppercase()),
        None => family,
    }
}

fn node_short_name(base: &str) -> String {
    let authority = base.split_once("://").map_or(base, |(_, rest)| rest)
        .split('/').next().unwrap_or(base);
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        authority.split(':').next().unwrap_or(authority)
    };
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => ip.octets()[3].to_string(),
        Err(_) => host.to_string(),
    }
}

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

/// A tool result with the pending tool-table update in front of it, so
/// the model reads the new table before the result it applies to.
fn with_update(update: Option<String>, text: &str) -> String {
    match update {
        Some(u) => format!("{}\n{}", u.trim_end(), text),
        None => text.to_string(),
    }
}

impl Model for LocalModel {
    /// Name the elected node and its model, which may differ from the
    /// weights on this machine: `AI hub · 149 (Qwen3.8 27B)`.
    fn label(&self) -> String {
        let this_machine = if cfg!(target_os = "macos") { "this Mac" } else { "this machine" };
        match self.session.as_ref().and_then(|s| s.route()) {
            Some(ChatRoute::Fleet { base, model }) => {
                format!("AI hub · {} ({})", node_short_name(&base), model_short_name(Path::new(&model)))
            }
            Some(ChatRoute::InProcess { model }) => {
                format!("AI hub · {this_machine} ({})", model_short_name(Path::new(&model)))
            }
            Some(ChatRoute::MachineNode { .. }) => {
                let model = self.model_path.as_deref().map(model_short_name).unwrap_or_else(|| "auto".into());
                format!("AI hub · {this_machine} ({model})")
            }
            None => self.label.clone(),
        }
    }

    fn configure(&mut self, system: &str, tools: &[ToolDefinition]) -> Result<(), String> {
        let changed = system != self.system || tools != self.tools.as_slice();
        self.system = system.to_string();
        self.tools = tools.to_vec();
        // No session yet: nothing to do — the weights load on the first
        // user input (text or pictures), never on a registration. An app joining
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
            // Without pictures, the first user line starts the session.
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

    fn can_rebind_mid_turn(&self) -> bool {
        // The tool table travels as an update block in the next message,
        // so a rebind between a call and its result costs nothing.
        true
    }

    fn attach_images(&mut self, images: Vec<ModelImage>) -> Result<(), String> {
        if images.is_empty() { return Ok(()); }
        if self.session.is_none() { self.start_session(); }
        self.session.as_ref().ok_or("the local model is not loaded")?.attach_images(
            images.into_iter().map(|image| ToolImage { label: image.label, png: image.png }).collect(),
        )
    }

    fn send_tool_result(&mut self, _call_id: &str, text: &str, is_error: bool) {
        if self.awaiting == 0 {
            return;
        }
        // A table that changed while the call ran rides in with its result
        // (the same block a user turn would carry).
        let text = with_update(self.pending_update.take(), text);
        self.results.push((text, is_error));
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
    fn model_names_read_well() {
        assert_eq!(model_short_name(Path::new("/x/Qwen3.5-9B-UD-Q4_K_XL.gguf")), "Qwen3.5 9B");
        assert_eq!(local_label(Some(Path::new("/x/Qwen3.5-9B-UD-Q4_K_XL.gguf"))), "AI hub · Qwen3.5 9B");
        assert_eq!(local_label(None), "AI hub · auto");
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
        // A bigger Qwen-named text encoder under another domain's folder is no chat model.
        t.file("home/weights/video/MiniMax-H3-tiers/qwen3vl_32b_minimax_h3-Q4_K_M.gguf", 9000);
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

    #[test]
    fn route_labels_name_node_and_model() {
        let route = ChatRoute::Fleet { base: "http://10.0.0.149:8785".into(), model: "qwen3.8-27b".into() };
        assert_eq!(route_label(&route), "AI hub · 149 (Qwen3.8 27B)");
    }

    #[test]
    fn hub_row_without_runtime_is_unselectable_until_found() {
        set_hub_find(HubFind::Searching);
        let row = local_row(None, false);
        assert_eq!((row.label.as_str(), row.unavailable.as_deref()), ("AI hub", Some("searching…")));
        set_hub_find(HubFind::Missing { at: Instant::now() });
        assert_eq!(local_row(None, false).unavailable.as_deref(), Some("no hub found"));
        assert_eq!(local_row(None, true).unavailable, None);
        set_hub_find(HubFind::Found("AI hub · 149 (Qwen3.8 27B)".into()));
        let row = local_row(None, false);
        assert_eq!((row.label.as_str(), row.unavailable), ("AI hub · 149 (Qwen3.8 27B)", None));
    }

    /// Against the real LAN: `cargo test -- --ignored hub_on_the_lan`.
    #[test]
    #[ignore]
    fn hub_on_the_lan() {
        let route = makepad_ai_hub::hub_chat::probe_fleet_chat().expect("a hub with a chat model on the LAN");
        println!("LAN hub row: {}", route_label(&route));
    }
}

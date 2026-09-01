//! One chat session, run IN THE APP, pumped by a worker thread on a channel.
//!
//! The routing law (aicore, 2026-08-31, superseding the 2026-08-20 broker
//! law): the AI-HUB is the backbone and the asset server only stores. The
//! session engine (`makepad_asset_chat::session`) runs on this worker; the
//! model is reached through the hub's providers directly (fleet qwen over
//! LAN discovery, or a cloud provider from env); catalog tools execute over
//! the app's own asset-client; and the tools the session's profile parks on
//! the app (the game's `world.*`, the asset UI's `*.generate`) execute
//! through [`ClientTools`] exactly as before — the answer now lands by a
//! function call instead of a tool-result route.
//!
//! The UI thread never blocks: every provider round happens here, and the
//! only thing the app touches is [`ChatFeed`]'s channel and the transcript
//! global in [`crate::transcript`].

use crate::transcript::{ChatData, ChatRole};
use makepad_asset_chat::context::ClientProfile;
use makepad_asset_creator::tools::CreatorTools;
use makepad_asset_chat::session::{Session, SessionId, ToolExecutor};
use makepad_asset_chat::toolcall;
use makepad_asset_chat::tools::{ContentToolCall, ToolDef};
use makepad_asset_chat::wire::{
    AttachmentBinding, ChatEventBody, ProviderAvailability, ToolOutcome,
};
use makepad_ai_hub::providers::provider::ChatProvider;
use makepad_ai_hub::providers::qwen::{FleetQwenChatProvider, HttpFleetTransport};
use makepad_ai_hub::discovery;
use makepad_asset_client::dto::{ChatProviderKind, ChatToolOutcomeDto};
use makepad_asset_client::json::Value;
use makepad_asset_client::{ApiEndpoints, ChatAttachment};
use makepad_widgets::log;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// What the app can ask the worker to do. Everything else it learns from
/// the transcript.
enum Cmd {
    Send { text: String, attachments: Vec<ChatAttachment> },
    Cancel,
    Clear,
    Shutdown,
}

/// Which session this feed opens, and who it says it is. THIS is the
/// personality seam: the namespace and the declared client profile select
/// the taught context and the tool surface server-side, so two apps share
/// this whole file and still get their own chat.
#[derive(Clone)]
pub struct FeedConfig {
    pub endpoints: ApiEndpoints,
    pub token: Option<String>,
    /// Verified-cache directory for this feed's client.
    pub cache: PathBuf,
    /// Asset namespace the session works in ("sandbox", "gen").
    pub namespace: String,
    /// Declared client profile slug ("game", "gen", "vj"); the broker
    /// refuses unknown ones rather than guessing.
    pub client: String,
    pub provider: ChatProviderKind,
    /// How the provider is named in status lines ("Qwen").
    pub provider_label: String,
    /// Test seam: a factory overriding [`make_provider`] (scripted turns).
    pub provider_factory:
        Option<Arc<dyn Fn() -> Box<dyn ChatProvider> + Send + Sync>>,
}

impl FeedConfig {
    pub fn new(
        endpoints: ApiEndpoints,
        token: Option<String>,
        cache: PathBuf,
        namespace: impl Into<String>,
        client: impl Into<String>,
    ) -> FeedConfig {
        FeedConfig {
            endpoints,
            token,
            cache,
            namespace: namespace.into(),
            client: client.into(),
            provider: ChatProviderKind::FleetQwen,
            provider_label: "Qwen".to_string(),
            provider_factory: None,
        }
    }
}

/// The tools this app executes for its own sessions.
///
/// The broker parks a call it will not run itself and waits for the answer
/// on the tool-result route; the worker calls [`ClientTools::execute`] and
/// posts what it returns. The two title hooks are the app's vocabulary for
/// the chip — the mechanics are shared, the words are not.
pub trait ClientTools: Send {
    fn execute(&mut self, name: &str, args: &Value) -> ToolOutcome;

    /// Chip title while the call runs.
    fn call_title(&mut self, name: &str, args: &Value) -> String {
        default_call_title(name, args)
    }

    /// Chip title + appended detail once the outcome is known.
    fn outcome_summary(&mut self, name: &str, outcome: &ChatToolOutcomeDto) -> (String, String) {
        default_outcome_summary(name, outcome)
    }

    /// A fresh session opened (first turn, or after Clear).
    fn session_opened(&mut self) {}
}

/// For a session whose profile parks nothing on the client.
pub struct NoClientTools;

impl ClientTools for NoClientTools {
    fn execute(&mut self, name: &str, _args: &Value) -> ToolOutcome {
        ToolOutcome::Failed { message: format!("'{name}' is not a tool this app executes") }
    }
}

pub struct ChatFeed {
    tx: Sender<Cmd>,
    dirty: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
}

impl ChatFeed {
    pub fn start(cfg: FeedConfig, tools: Box<dyn ClientTools>) -> ChatFeed {
        let (tx, rx) = mpsc::channel();
        let dirty = Arc::new(AtomicBool::new(false));
        let connected = Arc::new(AtomicBool::new(false));
        let worker_dirty = dirty.clone();
        let worker_connected = connected.clone();
        thread::Builder::new()
            .name("asset-chat-feed".into())
            .spawn(move || worker(cfg, tools, rx, worker_dirty, worker_connected))
            .ok();
        ChatFeed { tx, dirty, connected }
    }

    /// Send the user's turn.
    ///
    /// The user's BUBBLE is not pushed here. The app owns what the user
    /// said — it is the only half that knows which lane the message went
    /// down (a host with a device-local agent beside this one still shows
    /// one bubble) — and this half owns everything that comes back. When
    /// both pushed, the message appeared twice.
    pub fn send(&self, text: String, attachments: Vec<ChatAttachment>) {
        ChatData::begin_stream();
        ChatData::set_activity("sending…");
        self.dirty.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Send { text, attachments });
    }

    /// Escape / Stop: ask the broker to end the turn in flight.
    pub fn cancel(&self) {
        let _ = self.tx.send(Cmd::Cancel);
    }

    /// Clear: wipe the conversation and open a fresh session on the next
    /// turn (the old one is retired server-side).
    pub fn clear(&self) {
        ChatData::clear();
        self.dirty.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Clear);
    }

    /// True once a session is open on the broker.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Has anything changed since the host last drew?
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }
}

impl Drop for ChatFeed {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
    }
}

/// Per-turn presentation state: the raw text of the current assistant
/// segment (thinking + tool lines included) and the names of pending tool
/// calls so their result events can title the chips.
#[derive(Default)]
struct TurnView {
    raw: String,
    call_names: HashMap<String, String>,
    /// Client-tool call ids this app has already executed and answered.
    /// Guards against re-executing a call if an event page is ever seen
    /// twice — a stale re-post is what the broker 409s.
    answered: HashSet<String>,
}

/// What one turn's poll loop ended with.
enum TurnEnd {
    /// The turn finished (Done / Cancelled).
    Done,
    /// The turn failed; the message is shown as a system line.
    Failed(String),
    /// The app asked to clear mid-turn: the session is retired after the
    /// turn's cancel lands.
    Cleared,
}

/// The provider this feed's configured kind opens — the broker's own
/// factory, replicated: fleet qwen over LAN discovery; cloud and CLI
/// providers from this host's env. Constructed fresh per session.
fn make_provider(kind: ChatProviderKind) -> Box<dyn ChatProvider> {
    match kind {
        ChatProviderKind::FleetQwen => {
            let bases: Vec<String> = discovery::start_listener()
                .nodes()
                .into_iter()
                .map(|n| n.base_url)
                .collect();
            Box::new(FleetQwenChatProvider::new(HttpFleetTransport, bases))
        }
        ChatProviderKind::OpenAi => Box::new(makepad_asset_chat::openai::from_env()),
        ChatProviderKind::Grok => Box::new(makepad_asset_chat::grok::from_env()),
        ChatProviderKind::ClaudeCli => {
            Box::new(makepad_asset_chat::claude::ClaudeCodeChatProvider::new(None))
        }
        ChatProviderKind::CodexCli => {
            Box::new(makepad_asset_chat::codex_cli::CodexCliChatProvider::new(None))
        }
        ChatProviderKind::GrokCli => {
            Box::new(makepad_asset_chat::grok_cli::GrokCliChatProvider::new(None))
        }
    }
}

/// The in-app tool executor: catalog and operation tools over the app's own
/// asset-client (the same hardened surface the broker drove), park decisions
/// from the session's client profile, everything else typed-Unavailable.
struct AppExec {
    inner: CreatorTools,
    profile: ClientProfile,
}

impl ToolExecutor for AppExec {
    fn capability_doc(&mut self) -> String {
        // The profile brief (context::assemble) was orphaned when the
        // session moved in-process (aicore P8) — the broker used to
        // prepend it, so no in-app executor did. It rides in front of the
        // live capability text, profile-matched.
        let mut doc = makepad_asset_chat::context::assemble(self.profile, "");
        doc.push('\n');
        doc.push_str(&self.inner.capability_doc());
        doc
    }

    fn tool_definitions(&mut self) -> Vec<ToolDef> {
        self.inner.tool_definitions()
    }

    fn client_executes(&mut self, call: &ContentToolCall) -> bool {
        self.profile.client_executes(call)
    }

    fn execute(
        &mut self,
        call: &ContentToolCall,
        ctx: &makepad_asset_chat::session::ExecCtx,
        progress: &mut dyn FnMut(u16, &str),
        cancel: &makepad_asset_chat::session::CancelFlag,
    ) -> ToolOutcome {
        self.inner.execute(call, ctx, progress, cancel)
    }
}

/// Wire → DTO outcome, for the app-facing summary hooks (same five shapes).
fn outcome_dto(outcome: &ToolOutcome) -> ChatToolOutcomeDto {
    match outcome {
        ToolOutcome::Ok { value } => ChatToolOutcomeDto::Ok { value: value.clone() },
        ToolOutcome::Failed { message } => {
            ChatToolOutcomeDto::Failed { message: message.clone() }
        }
        ToolOutcome::Refused { what } => ChatToolOutcomeDto::Refused { what: what.clone() },
        ToolOutcome::Denied { what } => ChatToolOutcomeDto::Denied { what: what.clone() },
        ToolOutcome::Unavailable { reason } => {
            ChatToolOutcomeDto::Unavailable { reason: reason.clone() }
        }
    }
}

fn worker(
    cfg: FeedConfig,
    mut tools: Box<dyn ClientTools>,
    rx: Receiver<Cmd>,
    dirty: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
) {
    let profile =
        ClientProfile::from_slug(&cfg.client).unwrap_or(ClientProfile::General);
    let mut exec = AppExec {
        inner: CreatorTools::connect(cfg.endpoints, cfg.token.clone(), cfg.namespace.clone()),
        profile,
    };
    let mut session: Option<Session> = None;
    let mut view = TurnView::default();
    let mut queued: VecDeque<(String, Vec<ChatAttachment>)> = VecDeque::new();
    loop {
        let cmd = match queued.pop_front() {
            Some((text, attachments)) => Cmd::Send { text, attachments },
            None => match rx.recv() {
                Ok(cmd) => cmd,
                Err(_) => break,
            },
        };
        match cmd {
            Cmd::Send { text, attachments } => {
                match run_turn(
                    &cfg,
                    &mut session,
                    &mut view,
                    &mut exec,
                    tools.as_mut(),
                    &text,
                    &attachments,
                    &rx,
                    &mut queued,
                    &dirty,
                    &connected,
                ) {
                    TurnEnd::Done => {}
                    TurnEnd::Failed(error) => {
                        ChatData::end_stream();
                        ChatData::set_activity("");
                        ChatData::push(ChatRole::System, error);
                    }
                    TurnEnd::Cleared => {
                        retire(&mut session, &mut exec);
                        ChatData::clear();
                    }
                }
                dirty.store(true, Ordering::Relaxed);
            }
            Cmd::Cancel => {
                if ChatData::is_streaming() {
                    if let Some(session) = &session {
                        session.cancel_flag().cancel();
                    }
                    ChatData::end_stream();
                    ChatData::set_activity("");
                    dirty.store(true, Ordering::Relaxed);
                }
            }
            Cmd::Clear => {
                retire(&mut session, &mut exec);
                ChatData::clear();
                dirty.store(true, Ordering::Relaxed);
            }
            Cmd::Shutdown => break,
        }
    }
    retire(&mut session, &mut exec);
}

/// Drop the session (and let the executor forget its operations) so a Clear
/// really does start over.
fn retire(session: &mut Option<Session>, exec: &mut AppExec) {
    if let Some(session) = session.take() {
        session.cancel_flag().cancel();
        let _ = exec;
    }
}

#[allow(clippy::too_many_arguments)]
fn run_turn(
    cfg: &FeedConfig,
    session: &mut Option<Session>,
    view: &mut TurnView,
    exec: &mut AppExec,
    tools: &mut dyn ClientTools,
    text: &str,
    attachments: &[ChatAttachment],
    rx: &Receiver<Cmd>,
    queued: &mut VecDeque<(String, Vec<ChatAttachment>)>,
    dirty: &AtomicBool,
    connected: &AtomicBool,
) -> TurnEnd {
    if session.is_none() {
        ChatData::set_activity(&format!("probing the {} provider…", cfg.provider_label));
        let mut provider = match &cfg.provider_factory {
            Some(factory) => factory(),
            None => make_provider(cfg.provider),
        };
        match provider.availability() {
            ProviderAvailability::Available { model, .. } => {
                ChatData::set_status(format!("{} ready · {model}", cfg.provider_label));
                ChatData::set_activity(&format!("{} ready · {model}", cfg.provider_label));
            }
            ProviderAvailability::Unavailable { reason } => {
                connected.store(false, Ordering::Relaxed);
                let line = format!(
                    "The {} provider is unavailable: {reason}",
                    cfg.provider_label
                );
                ChatData::set_status(&line);
                return TurnEnd::Failed(line);
            }
        }
        *session = Some(Session::new(cfg.client.clone(), provider));
        view.answered.clear();
        connected.store(true, Ordering::Relaxed);
        tools.session_opened();
    }
    let live = session.as_mut().expect("just ensured");
    ChatData::set_activity("sending…");
    let bindings: Vec<AttachmentBinding> = attachments
        .iter()
        .map(|a| AttachmentBinding { revision: a.revision, role: a.role.clone() })
        .collect();
    if let Err(refusal) = live.send(text, &bindings, exec) {
        let error = format!("send: {refusal:?}");
        *session = None;
        connected.store(false, Ordering::Relaxed);
        return TurnEnd::Failed(error);
    }
    ChatData::begin_stream();
    view.raw.clear();
    view.call_names.clear();
    ChatData::set_activity("thinking…");
    dirty.store(true, Ordering::Relaxed);
    let mut cleared = false;
    loop {
        // Commands are serviced INSIDE the turn: Escape has to land while
        // the reply is still streaming.
        loop {
            match rx.try_recv() {
                Ok(Cmd::Cancel) => {
                    live.cancel_flag().cancel();
                    ChatData::set_activity("stopping…");
                }
                Ok(Cmd::Clear) => {
                    live.cancel_flag().cancel();
                    cleared = true;
                    ChatData::set_activity("clearing…");
                }
                Ok(Cmd::Send { text, attachments }) => queued.push_back((text, attachments)),
                Ok(Cmd::Shutdown) | Err(TryRecvError::Disconnected) => {
                    live.cancel_flag().cancel();
                    return TurnEnd::Done;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        live.pump(exec);
        let session_id = live.id().clone();
        for event in live.drain_events() {
            if let Some(end) =
                handle_event(live, exec, &session_id, view, tools, event.body)
            {
                dirty.store(true, Ordering::Relaxed);
                return if cleared { TurnEnd::Cleared } else { end };
            }
        }
        dirty.store(true, Ordering::Relaxed);
        thread::sleep(Duration::from_millis(40));
    }
}

/// Returns `Some(end)` when the turn ended.
fn handle_event(
    session: &mut Session,
    exec: &mut AppExec,
    _id: &SessionId,
    view: &mut TurnView,
    tools: &mut dyn ClientTools,
    body: ChatEventBody,
) -> Option<TurnEnd> {
    match body {
        ChatEventBody::Delta { text, serving } => {
            ChatData::note_delta(
                text.len(),
                serving.map(|s| s.gen_tokens),
                serving.and_then(|s| Some((s.lanes_active?, s.slots_total?))),
                serving.and_then(|s| s.think_tokens),
                serving.and_then(|s| s.visible_tokens),
            );
            view.raw.push_str(&text);
            ChatData::set_thinking_text(&toolcall::split_thinking(&view.raw).thinking);
            let visible = toolcall::strip_marker(&view.raw);
            if visible.trim().is_empty() {
                match serving.and_then(|s| s.think_tokens) {
                    Some(n) if n > 0 => ChatData::set_activity(&format!("thinking · {n} tok")),
                    _ => ChatData::set_activity("thinking…"),
                }
            } else {
                ChatData::set_activity("");
                ChatData::set_stream_text(&visible);
            }
            None
        }
        ChatEventBody::ToolCall { id: call_id, name, args } => {
            view.call_names.insert(call_id.clone(), name.clone());
            ChatData::set_stream_text(&toolcall::strip_marker(&view.raw));
            view.raw.clear();
            let detail = format!("args: {}\n", args.to_json());
            ChatData::push_tool(&call_id, tools.call_title(&name, &args), detail);
            ChatData::set_activity(&format!("running {name}…"));
            // The session executes its own tools inside pump(); the calls
            // its profile parks on this app land here — execute and answer
            // by function call, no wire in between.
            let parked = ContentToolCall::parse(&name, &args)
                .map(|call| exec.profile.client_executes(&call))
                .unwrap_or(false);
            if parked && !view.answered.contains(&call_id) {
                view.answered.insert(call_id.clone());
                let outcome = tools.execute(&name, &args).clamped();
                if let Err(error) = session.provide_client_outcome(&call_id, outcome, exec) {
                    log!("chat-feed: client outcome for {call_id} refused: {error}");
                }
            }
            None
        }
        ChatEventBody::ToolProgress { note, .. } => {
            ChatData::set_activity(&note);
            None
        }
        ChatEventBody::ToolResult { id: call_id, outcome } => {
            let name = view.call_names.remove(&call_id).unwrap_or_else(|| "tool".into());
            let dto = outcome_dto(&outcome);
            let (title, detail) = tools.outcome_summary(&name, &dto);
            ChatData::finish_tool(&call_id, title, &detail);
            ChatData::set_activity("thinking about the result…");
            None
        }
        ChatEventBody::Done => {
            ChatData::set_stream_text(&toolcall::strip_marker(&view.raw));
            view.raw.clear();
            ChatData::end_stream();
            ChatData::set_activity("");
            Some(TurnEnd::Done)
        }
        ChatEventBody::Cancelled => {
            view.raw.clear();
            ChatData::end_stream();
            ChatData::set_activity("");
            Some(TurnEnd::Done)
        }
        ChatEventBody::Error { message, .. } => {
            view.raw.clear();
            ChatData::end_stream();
            ChatData::set_activity("");
            Some(TurnEnd::Failed(message))
        }
    }
}

// -------------------------------------------------------------- vocabulary

/// The chip title for tools every profile shares. An app overrides
/// [`ClientTools::call_title`] for its own verbs.
pub fn default_call_title(name: &str, args: &Value) -> String {
    match name {
        "assets.query" => {
            let sql = args.get("sql").and_then(Value::as_str).unwrap_or("");
            format!(
                "queried: {}",
                ellipsis(&sql.split_whitespace().collect::<Vec<_>>().join(" "), 72)
            )
        }
        "assets.schema" => "read the catalog schema".to_string(),
        "asset.search" => {
            let q = args.get("query").and_then(Value::as_str).unwrap_or("");
            format!("searched: {}", ellipsis(q, 60))
        }
        "asset.inspect" => "inspected an asset".to_string(),
        other => other.to_string(),
    }
}

/// The completed chip for tools every profile shares.
pub fn default_outcome_summary(name: &str, outcome: &ChatToolOutcomeDto) -> (String, String) {
    let base = match name {
        "assets.query" => "queried",
        "assets.schema" => "read the catalog schema",
        "asset.search" => "searched",
        "asset.inspect" => "inspected",
        other => other,
    };
    match outcome {
        ChatToolOutcomeDto::Ok { value } => {
            let note = value
                .get("rows")
                .and_then(Value::as_i64)
                .map(|n| format!(" → {n} rows"))
                .unwrap_or_default();
            let body = value
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| value.to_json());
            (format!("{base}{note}"), format!("\n{body}"))
        }
        ChatToolOutcomeDto::Failed { message } => {
            (format!("{base} — failed"), format!("\nfailed: {message}"))
        }
        ChatToolOutcomeDto::Refused { what } => {
            (format!("{base} — refused"), format!("\nrefused: {what}"))
        }
        ChatToolOutcomeDto::Denied { what } => {
            (format!("{base} — denied"), format!("\ndenied: {what}"))
        }
        ChatToolOutcomeDto::Unavailable { reason } => {
            (format!("{base} — unavailable"), format!("\nunavailable: {reason}"))
        }
    }
}

pub fn ellipsis(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_asset_client::json;

    #[test]
    fn shared_call_titles_read_like_activity() {
        let args = json::obj(vec![("sql", json::s("SELECT  canon_alias FROM search_annotations"))]);
        let title = default_call_title("assets.query", &args);
        assert!(title.starts_with("queried: SELECT canon_alias"), "{title}");
        let search = json::obj(vec![("query", json::s("fence"))]);
        assert_eq!(default_call_title("asset.search", &search), "searched: fence");
    }

    #[test]
    fn shared_outcomes_count_rows_and_report_failures() {
        let ok = ChatToolOutcomeDto::Ok {
            value: json::obj(vec![("rows", Value::Int(12)), ("text", json::s("canon_alias\n…"))]),
        };
        let (title, detail) = default_outcome_summary("assets.query", &ok);
        assert_eq!(title, "queried → 12 rows");
        assert!(detail.contains("canon_alias"));
        let (title, detail) = default_outcome_summary(
            "asset.search",
            &ChatToolOutcomeDto::Failed { message: "catalog offline".into() },
        );
        assert_eq!(title, "searched — failed");
        assert!(detail.contains("catalog offline"));
    }

    /// A profile that parks nothing still answers honestly rather than
    /// leaving the broker's park to time out in silence.
    #[test]
    fn a_feed_without_client_tools_refuses_by_name() {
        let outcome = NoClientTools.execute("world.place", &json::obj(vec![]));
        match outcome {
            ToolOutcome::Failed { message } => assert!(message.contains("world.place"), "{message}"),
            other => panic!("expected a failure, got {other:?}"),
        }
    }
}

//! One chat session on the Asset Server's broker, pumped by a worker
//! thread on a channel.
//!
//! The routing law (user, 2026-08-20): the asset server is the ONE routing
//! point for all AI. An app never talks to a fleet box or an external
//! provider — it opens a broker-owned chat session on its Asset Server,
//! sends user turns, and renders the event stream. The model runs on a
//! fleet box the SERVER picked; the model's tool calls execute on the
//! server (catalog SQL, search, typed operations) — except the tools the
//! session's declared profile parks BACK on this app (the game's `world.*`,
//! the asset UI's `*.generate` / `defaults.*` / `fleet.introspect`), which
//! this worker executes through [`ClientTools`] and posts to the
//! tool-result route.
//!
//! The UI thread never blocks: every HTTP call happens here, and the only
//! thing the app touches is [`ChatFeed`]'s channel and the transcript
//! global in [`crate::transcript`].

use crate::transcript::{ChatData, ChatRole};
use makepad_asset_chat::toolcall;
use makepad_asset_chat::wire::ToolOutcome;
use makepad_asset_client::dto::{
    ChatEventBodyDto, ChatProviderKind, ChatProviderStateDto, ChatSessionId, ChatToolOutcomeDto,
};
use makepad_asset_client::json::Value;
use makepad_asset_client::{
    ApiEndpoints, AssetClient, ChatAttachment, ChatCreateRequest, ChatSendRequest, ClientConfig,
};
use makepad_widgets::log;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

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
#[derive(Clone, Debug)]
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

    /// Push the user's turn. The bubble lands immediately so the UI never
    /// looks like it dropped the message.
    pub fn send(&self, text: String, attachments: Vec<ChatAttachment>) {
        ChatData::push(ChatRole::User, &text);
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

fn worker(
    cfg: FeedConfig,
    mut tools: Box<dyn ClientTools>,
    rx: Receiver<Cmd>,
    dirty: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
) {
    let _ = std::fs::create_dir_all(&cfg.cache);
    let mut client_cfg = ClientConfig::new(cfg.cache.clone());
    client_cfg.token = cfg.token.clone();
    let client = match AssetClient::connect(client_cfg, cfg.endpoints, None) {
        Ok(c) => c,
        Err(error) => {
            let line = format!("chat could not reach the asset server: {error}");
            ChatData::set_status(&line);
            ChatData::push(ChatRole::System, line);
            dirty.store(true, Ordering::Relaxed);
            return;
        }
    };
    // The session AND its event cursor live across turns: the broker's
    // event log is one monotonic stream per session, and polling a new turn
    // from 0 REPLAYS every earlier turn — re-executing old client tools and
    // posting stale results the broker answers with 409 no_client_tool.
    let mut session: Option<(ChatSessionId, u64)> = None;
    let mut view = TurnView::default();
    // Turns the user sent while another was still running.
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
                    &client,
                    &cfg,
                    &mut session,
                    &mut view,
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
                        retire(&client, &mut session);
                        ChatData::clear();
                    }
                }
                dirty.store(true, Ordering::Relaxed);
            }
            Cmd::Cancel => {
                // Nothing is streaming: a cancel outside a turn is a no-op
                // rather than a request the broker has to reason about.
                if ChatData::is_streaming() {
                    if let Some((id, _)) = &session {
                        let _ = client.chat_cancel(id);
                    }
                    ChatData::end_stream();
                    ChatData::set_activity("");
                    dirty.store(true, Ordering::Relaxed);
                }
            }
            Cmd::Clear => {
                retire(&client, &mut session);
                ChatData::clear();
                dirty.store(true, Ordering::Relaxed);
            }
            Cmd::Shutdown => break,
        }
    }
    retire(&client, &mut session);
}

/// Retire the session server-side so a Clear really does start over (and a
/// closing app does not leave a session against the broker's per-owner cap).
fn retire(client: &AssetClient, session: &mut Option<(ChatSessionId, u64)>) {
    if let Some((id, _)) = session.take() {
        let _ = client.chat_cancel(&id);
        let _ = client.chat_retire(&id);
    }
}

#[allow(clippy::too_many_arguments)]
fn run_turn(
    client: &AssetClient,
    cfg: &FeedConfig,
    session: &mut Option<(ChatSessionId, u64)>,
    view: &mut TurnView,
    tools: &mut dyn ClientTools,
    text: &str,
    attachments: &[ChatAttachment],
    rx: &Receiver<Cmd>,
    queued: &mut VecDeque<(String, Vec<ChatAttachment>)>,
    dirty: &AtomicBool,
    connected: &AtomicBool,
) -> TurnEnd {
    if session.is_none() {
        ChatData::set_activity("checking the server's chat providers…");
        let providers = match client.chat_providers() {
            Ok(p) => p,
            Err(e) => {
                connected.store(false, Ordering::Relaxed);
                return TurnEnd::Failed(format!("the asset server did not answer: {e}"));
            }
        };
        let picked = providers.iter().find(|p| p.kind == cfg.provider);
        match picked.map(|p| &p.state) {
            Some(ChatProviderStateDto::Available { model }) => {
                ChatData::set_status(format!("{} ready · {model}", cfg.provider_label));
                ChatData::set_activity(&format!("{} ready · {model}", cfg.provider_label));
            }
            Some(ChatProviderStateDto::Unavailable { reason }) => {
                connected.store(false, Ordering::Relaxed);
                let line = format!("The server's {} provider is unavailable: {reason}", cfg.provider_label);
                ChatData::set_status(&line);
                return TurnEnd::Failed(line);
            }
            None => {
                connected.store(false, Ordering::Relaxed);
                let line = format!(
                    "this asset server has no {} provider",
                    cfg.provider.as_str()
                );
                ChatData::set_status(&line);
                return TurnEnd::Failed(line);
            }
        }
        ChatData::set_activity("opening a chat session…");
        let request = ChatCreateRequest::new(cfg.namespace.clone(), cfg.provider)
            .with_client(cfg.client.clone());
        let created = match client.chat_create(&request) {
            Ok(c) => c,
            Err(e) => {
                connected.store(false, Ordering::Relaxed);
                return TurnEnd::Failed(format!("open chat session: {e}"));
            }
        };
        // Cursor 0 is correct exactly once: a fresh session has no history.
        *session = Some((created.session, 0));
        view.answered.clear();
        connected.store(true, Ordering::Relaxed);
        tools.session_opened();
    }
    let (id, mut cursor) = {
        let (id, cursor) = session.as_ref().expect("just ensured");
        (id.clone(), *cursor)
    };
    ChatData::set_activity("sending…");
    let request = ChatSendRequest {
        text: text.to_string(),
        attachments: attachments.to_vec(),
    };
    if let Err(e) = client.chat_send(&id, &request) {
        // A sealed/expired session starts over cleanly on the next message.
        *session = None;
        connected.store(false, Ordering::Relaxed);
        return TurnEnd::Failed(format!("send: {e}"));
    }
    ChatData::begin_stream();
    view.raw.clear();
    view.call_names.clear();
    ChatData::set_activity("thinking…");
    dirty.store(true, Ordering::Relaxed);
    // One failed poll must not kill a live turn: the broker's actor can be
    // busy past the route's own timeout during a long provider round, and a
    // parked client tool gives us a window to come back for it. Only a
    // sustained outage ends the turn.
    let mut poll_failures = 0u32;
    let mut cleared = false;
    loop {
        // Commands are serviced INSIDE the turn: Escape has to reach the
        // broker while the reply is still streaming, not after it lands.
        loop {
            match rx.try_recv() {
                Ok(Cmd::Cancel) => {
                    let _ = client.chat_cancel(&id);
                    ChatData::set_activity("stopping…");
                }
                Ok(Cmd::Clear) => {
                    let _ = client.chat_cancel(&id);
                    cleared = true;
                    ChatData::set_activity("clearing…");
                }
                // A turn at a time is the session engine's law; the next
                // one runs as soon as this one lands.
                Ok(Cmd::Send { text, attachments }) => queued.push_back((text, attachments)),
                Ok(Cmd::Shutdown) | Err(TryRecvError::Disconnected) => {
                    let _ = client.chat_cancel(&id);
                    return TurnEnd::Done;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        let page = match client.chat_events(&id, cursor, 8_000, 64) {
            Ok(page) => {
                poll_failures = 0;
                page
            }
            Err(e) => {
                poll_failures += 1;
                // The broker actor can be busy for MINUTES when another
                // session's provider round is queued behind fleet jobs —
                // and a parked tool's timeout is starved by exactly as
                // much, so patience here never loses to it. ~2 minutes.
                if poll_failures >= 60 {
                    return TurnEnd::Failed(format!("events: {e}"));
                }
                ChatData::set_activity("server busy — still listening…");
                thread::sleep(Duration::from_millis(2000));
                continue;
            }
        };
        cursor = page.cursor;
        // Persist the cursor as it advances: the NEXT turn must resume
        // after this one's events, never replay them.
        if let Some((_, saved)) = session.as_mut() {
            *saved = cursor;
        }
        for event in page.events {
            if let Some(end) = handle_event(client, &id, view, tools, event.body) {
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
    client: &AssetClient,
    id: &ChatSessionId,
    view: &mut TurnView,
    tools: &mut dyn ClientTools,
    body: ChatEventBodyDto,
) -> Option<TurnEnd> {
    match body {
        ChatEventBodyDto::Delta { text, serving } => {
            // The rate meter reads the RAW delta (thinking and tool lines
            // included — the box generated all of it) and prefers the
            // service's own token count over a guess from bytes.
            ChatData::note_delta(
                text.len(),
                serving.map(|s| s.gen_tokens),
                serving.and_then(|s| Some((s.lanes_active?, s.slots_total?))),
                serving.and_then(|s| s.think_tokens),
                serving.and_then(|s| s.visible_tokens),
            );
            view.raw.push_str(&text);
            // The porthole reads the think block as it arrives.
            ChatData::set_thinking_text(&toolcall::split_thinking(&view.raw).thinking);
            let visible = toolcall::strip_marker(&view.raw);
            if visible.trim().is_empty() {
                // The box streams its hidden-reasoning progress before any
                // visible text; show the work, not a generic wait.
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
        ChatEventBodyDto::ToolCall { id: call_id, name, args } => {
            view.call_names.insert(call_id.clone(), name.clone());
            ChatData::set_stream_text(&toolcall::strip_marker(&view.raw));
            view.raw.clear();
            let detail = format!("args: {}\n", args.to_json());
            ChatData::push_tool(&call_id, tools.call_title(&name, &args), detail);
            ChatData::set_activity(&format!("running {name}…"));
            // The broker executes its own tools; the ones this session's
            // profile parks on the app land here — execute and answer.
            if !view.answered.contains(&call_id) {
                deliver_client_tool(client, id, view, tools, &call_id, &name, &args);
            }
            None
        }
        ChatEventBodyDto::ToolProgress { note, .. } => {
            ChatData::set_activity(&note);
            None
        }
        ChatEventBodyDto::ToolResult { id: call_id, outcome } => {
            let name = view.call_names.remove(&call_id).unwrap_or_else(|| "tool".into());
            let (title, detail) = tools.outcome_summary(&name, &outcome);
            ChatData::finish_tool(&call_id, title, &detail);
            ChatData::set_activity("thinking about the result…");
            None
        }
        ChatEventBodyDto::Done => {
            ChatData::set_stream_text(&toolcall::strip_marker(&view.raw));
            view.raw.clear();
            ChatData::end_stream();
            ChatData::set_activity("");
            Some(TurnEnd::Done)
        }
        ChatEventBodyDto::Cancelled => {
            view.raw.clear();
            ChatData::end_stream();
            ChatData::set_activity("");
            Some(TurnEnd::Done)
        }
        ChatEventBodyDto::Error { message, .. } => {
            view.raw.clear();
            ChatData::end_stream();
            ChatData::set_activity("");
            Some(TurnEnd::Failed(message))
        }
    }
}

/// Execute one parked tool and PERSISTENTLY deliver its outcome.
///
/// The broker actor is single-threaded; a long provider round for ANOTHER
/// session can starve this route for minutes — and, crucially, it starves
/// the park-timeout check by exactly as much, so a post that keeps retrying
/// WINS: the real result lands before any timeout can fire. Give up only on
/// a definitive non-busy answer or after ~90 s of sustained failure. Every
/// attempt is logged so this leg is never dark.
fn deliver_client_tool(
    client: &AssetClient,
    id: &ChatSessionId,
    view: &mut TurnView,
    tools: &mut dyn ClientTools,
    call_id: &str,
    name: &str,
    args: &Value,
) {
    view.answered.insert(call_id.to_string());
    // Clamp to the wire bounds BEFORE posting: encode does not validate,
    // and an over-long honest refusal is 400'd wholesale by the broker's
    // parser while the model hears "the app did not answer".
    let outcome = tools.execute(name, args).clamped();
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut wait = Duration::from_millis(1000);
    let posted = loop {
        match client.chat_tool_result(id, call_id, &outcome.encode()) {
            Ok(()) => {
                log!("chat-feed: tool result {call_id} ({name}) accepted");
                break Ok(());
            }
            Err(e) => {
                let text = e.to_string();
                // 4xx-class protocol refusals are final; the busy/
                // unavailable class retries.
                let busy = text.contains("503")
                    || text.contains("timed out")
                    || text.contains("timeout")
                    || text.contains("connect")
                    || text.contains("unavailable");
                log!(
                    "chat-feed: tool result {call_id} ({name}) post failed: {text}{}",
                    if busy && Instant::now() < deadline { " — retrying" } else { " — giving up" }
                );
                if !busy || Instant::now() >= deadline {
                    break Err(e);
                }
                ChatData::set_activity("server busy — delivering the result…");
                thread::sleep(wait);
                wait = (wait * 2).min(Duration::from_secs(4));
            }
        }
    };
    if let Err(e) = posted {
        // NOT fatal: the turn is alive server-side and its fate arrives
        // through this same event stream. The chip keeps the truth without
        // alarming language — and says WHICH failure this was: a server
        // that REJECTED the answer (protocol skew — report it) is a
        // different animal from one that stayed busy.
        let text = e.to_string();
        let rejected = text.contains("400") || text.contains("malformed");
        let title_note = if rejected {
            "result rejected by the server (protocol)"
        } else {
            "result delivery failed"
        };
        let title = format!("{} — done ({title_note})", tools.call_title(name, args));
        ChatData::finish_tool(
            call_id,
            title,
            &format!("{e}\nthe app applied it locally; the model continues with the broker's own outcome"),
        );
        ChatData::set_activity("");
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

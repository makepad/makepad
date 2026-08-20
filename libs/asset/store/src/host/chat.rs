//! Broker-owned chat session actor.
//!
//! The control plane only authenticates and forwards; this thread owns every
//! [`Session`], constructs providers from server-side config (never from the
//! client wire), and executes tools. Local fleet Qwen and external OpenAI /
//! Grok are explicit peers. A local session may `llm.consult` the external
//! provider for text-only code/level/design drafts; that consult cannot run
//! tools or nest another session.

use super::api::principal_str;
use super::config::{ChatConfig, ChatScript, ScriptedLane, ScriptedTurn};
use super::util::log;
use makepad_asset_chat::catalog_sql::CatalogReader;
use makepad_asset_chat::context::{self, ClientProfile};
use makepad_asset_chat::dispatch::AssetServerTools;
use makepad_asset_chat::provider::{ChatProvider, ProviderEvent, ThreadedProvider, TurnInput};
use makepad_asset_chat::qwen::{FleetQwenChatProvider, HttpFleetTransport};
use makepad_asset_chat::session::{
    CancelFlag, ExecCtx, SendRefusal, Session, SessionId, ToolExecutor,
};
use makepad_asset_chat::tools::{self, ConsultTask, ContentToolCall, ToolDef};
use makepad_asset_chat::wire::{
    sanitize_public_error, AttachmentBinding, ChatEvent, ChatMessage, ChatRole,
    ProviderAvailability, ProviderKind, ToolOutcome, MAX_MESSAGE_BYTES,
};
use makepad_asset_client::ApiEndpoints;
use crate::PrincipalId;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const PUMP_SLICE: Duration = Duration::from_millis(50);
const CONSULT_POLL: Duration = Duration::from_millis(20);
const CONSULT_TIMEOUT: Duration = Duration::from_secs(60);
const ACTOR_IDLE: Duration = Duration::from_millis(20);
/// How long a parked client-executed tool may wait for the app's answer
/// before the broker times the round out with a `Failed` outcome the model
/// can react to.
// With the provider on its own thread the actor pumps freely, so this
// timeout actually fires on time now — give the client real headroom (a
// first game.map stream can take over 30 s) instead of the old value that
// was only survivable because the actor starved it as much as the client.
const CLIENT_TOOL_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct ChatHandle {
    tx: Sender<ChatCmd>,
}

pub struct ProviderStatus {
    pub kind: ProviderKind,
    pub available: bool,
    pub model: Option<String>,
    pub reason: Option<String>,
}

pub struct SessionView {
    pub id: SessionId,
    pub namespace: String,
    pub provider: ProviderKind,
    pub owner: PrincipalId,
    pub state: &'static str,
    pub turn: u64,
    pub idle: bool,
}

pub enum ChatFail {
    Down,
    NotFound,
    Busy,
    Sealed { reason: String },
    ProviderUnavailable { reason: String },
    ProviderError { message: String },
    TooLarge { what: &'static str },
    TooMany { what: &'static str },
    HistoryFull,
    InvalidAttachment { what: String },
    OverBudget { what: &'static str },
    ToolsConnect { message: String },
    /// A tool-result was posted but no client tool is parked (or the call
    /// id does not match).
    NoClientTool { what: &'static str },
}

enum ChatCmd {
    ListProviders { reply: Sender<Vec<ProviderStatus>> },
    Create(CreateReq),
    Get { owner: PrincipalId, id: SessionId, reply: Sender<Result<SessionView, ChatFail>> },
    /// Every live session this owner may see — the fold-back loop's way to
    /// find a play session's transcript without knowing its id.
    List { owner: PrincipalId, reply: Sender<Vec<SessionView>> },
    Send(SendReq),
    Events {
        owner: PrincipalId,
        id: SessionId,
        after: u64,
        limit: u32,
        reply: Sender<Result<(Vec<ChatEvent>, u64), ChatFail>>,
    },
    Cancel { owner: PrincipalId, id: SessionId, reply: Sender<Result<SessionView, ChatFail>> },
    Retire { owner: PrincipalId, id: SessionId, reply: Sender<Result<bool, ChatFail>> },
    /// The client's answer to a parked client-executed tool call.
    ToolResult {
        owner: PrincipalId,
        id: SessionId,
        call_id: String,
        outcome: ToolOutcome,
        reply: Sender<Result<(), ChatFail>>,
    },
    Shutdown,
}

struct CreateReq {
    owner: PrincipalId,
    namespace: String,
    token: String,
    provider: ProviderKind,
    profile: ClientProfile,
    reply: Sender<Result<SessionView, ChatFail>>,
}

struct SendReq {
    owner: PrincipalId,
    id: SessionId,
    text: String,
    attachments: Vec<AttachmentBinding>,
    reply: Sender<Result<u64, ChatFail>>,
}

pub fn spawn_broker(
    endpoints: ApiEndpoints,
    cfg: ChatConfig,
    catalog_db: Option<PathBuf>,
    log_enabled: bool,
    stop: Arc<AtomicBool>,
) -> Result<(ChatHandle, JoinHandle<()>), crate::ServerError> {
    let (tx, rx) = mpsc::channel();
    let factory: Box<dyn ProviderFactory> = match cfg.script.clone() {
        Some(script) => Box::new(ScriptedFactory::new(script)),
        None => Box::new(EnvFactory {
            fleet_bases: cfg.fleet_bases.clone(),
            fleet: cfg.fleet.clone(),
            availability: Vec::new(),
        }),
    };
    let join = std::thread::Builder::new()
        .name("asset-server-chat".into())
        .spawn(move || {
            let mut actor = Actor {
                endpoints,
                cfg,
                factory,
                sessions: HashMap::new(),
                // The broker shares a process (and root) with the catalog:
                // `assets.query` reads the SAME file the store serves,
                // through makepad-sqlite's read-only WAL snapshot reads.
                catalog: catalog_db.map(CatalogReader::new),
                log_enabled,
                stop,
            };
            loop {
                if actor.stop.load(Ordering::Relaxed) {
                    actor.shutdown_all();
                    break;
                }
                match rx.recv_timeout(ACTOR_IDLE) {
                    Ok(ChatCmd::Shutdown) => {
                        actor.shutdown_all();
                        break;
                    }
                    Ok(cmd) => actor.handle(cmd),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        actor.shutdown_all();
                        break;
                    }
                }
                actor.pump_all();
            }
        })
        .map_err(|e| crate::ServerError::Io {
            op: "spawn chat broker",
            kind: e.kind(),
        })?;
    Ok((ChatHandle { tx }, join))
}

impl ChatHandle {
    pub fn list_providers(&self) -> Result<Vec<ProviderStatus>, ChatFail> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(ChatCmd::ListProviders { reply: tx }).map_err(|_| ChatFail::Down)?;
        rx.recv_timeout(Duration::from_secs(5)).map_err(|_| ChatFail::Down)
    }

    pub fn create(
        &self,
        owner: PrincipalId,
        namespace: String,
        token: String,
        provider: ProviderKind,
        profile: ClientProfile,
    ) -> Result<SessionView, ChatFail> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(ChatCmd::Create(CreateReq {
                owner,
                namespace,
                token,
                provider,
                profile,
                reply: tx,
            }))
            .map_err(|_| ChatFail::Down)?;
        rx.recv_timeout(Duration::from_secs(15)).map_err(|_| ChatFail::Down)?
    }

    /// Deliver the client's outcome for a parked client-executed tool.
    pub fn tool_result(
        &self,
        owner: PrincipalId,
        id: SessionId,
        call_id: String,
        outcome: ToolOutcome,
    ) -> Result<(), ChatFail> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(ChatCmd::ToolResult { owner, id, call_id, outcome, reply: tx })
            .map_err(|_| ChatFail::Down)?;
        rx.recv_timeout(Duration::from_secs(15)).map_err(|_| ChatFail::Down)?
    }

    pub fn get(&self, owner: PrincipalId, id: SessionId) -> Result<SessionView, ChatFail> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(ChatCmd::Get { owner, id, reply: tx }).map_err(|_| ChatFail::Down)?;
        rx.recv_timeout(Duration::from_secs(5)).map_err(|_| ChatFail::Down)?
    }

    /// Every live session the owner may see (id-sorted).
    pub fn list(&self, owner: PrincipalId) -> Result<Vec<SessionView>, ChatFail> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(ChatCmd::List { owner, reply: tx }).map_err(|_| ChatFail::Down)?;
        rx.recv_timeout(Duration::from_secs(5)).map_err(|_| ChatFail::Down)
    }

    pub fn send(
        &self,
        owner: PrincipalId,
        id: SessionId,
        text: String,
        attachments: Vec<AttachmentBinding>,
    ) -> Result<u64, ChatFail> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(ChatCmd::Send(SendReq { owner, id, text, attachments, reply: tx }))
            .map_err(|_| ChatFail::Down)?;
        rx.recv_timeout(Duration::from_secs(15)).map_err(|_| ChatFail::Down)?
    }

    pub fn events(
        &self,
        owner: PrincipalId,
        id: SessionId,
        after: u64,
        limit: u32,
    ) -> Result<(Vec<ChatEvent>, u64), ChatFail> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(ChatCmd::Events { owner, id, after, limit, reply: tx })
            .map_err(|_| ChatFail::Down)?;
        rx.recv_timeout(Duration::from_secs(5)).map_err(|_| ChatFail::Down)?
    }

    pub fn cancel(&self, owner: PrincipalId, id: SessionId) -> Result<SessionView, ChatFail> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(ChatCmd::Cancel { owner, id, reply: tx }).map_err(|_| ChatFail::Down)?;
        rx.recv_timeout(Duration::from_secs(5)).map_err(|_| ChatFail::Down)?
    }

    pub fn retire(&self, owner: PrincipalId, id: SessionId) -> Result<bool, ChatFail> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(ChatCmd::Retire { owner, id, reply: tx }).map_err(|_| ChatFail::Down)?;
        rx.recv_timeout(Duration::from_secs(5)).map_err(|_| ChatFail::Down)?
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(ChatCmd::Shutdown);
    }
}

struct Actor {
    endpoints: ApiEndpoints,
    cfg: ChatConfig,
    factory: Box<dyn ProviderFactory>,
    sessions: HashMap<SessionId, Live>,
    catalog: Option<CatalogReader>,
    log_enabled: bool,
    stop: Arc<AtomicBool>,
}

struct Live {
    owner: PrincipalId,
    namespace: String,
    session: Session,
    tools: Option<AssetServerTools>,
    events: Vec<ChatEvent>,
    consult_depth: u32,
    /// Context/tool-surface flavor the connecting app declared at create.
    profile: ClientProfile,
    /// When a client-executed tool parked the turn: since when. The broker
    /// times the wait out with a Failed outcome after
    /// [`CLIENT_TOOL_TIMEOUT`].
    client_wait: Option<Instant>,
    /// Call ids of client tools already resolved (by the app or by the
    /// timeout). A LATE or duplicate tool-result post for one of these is
    /// acknowledged idempotently instead of 409ing — the turn has already
    /// moved on, and punishing the app for a slow answer turned a hiccup
    /// into a user-visible dead turn (play-session-1 entry 13). Bounded.
    resolved_tool_calls: Vec<String>,
}

impl Actor {
    fn handle(&mut self, cmd: ChatCmd) {
        match cmd {
            ChatCmd::ListProviders { reply } => {
                let _ = reply.send(self.list_providers());
            }
            ChatCmd::Create(req) => {
                let _ = req.reply.send(self.create(
                    req.owner,
                    req.namespace,
                    req.token,
                    req.provider,
                    req.profile,
                ));
            }
            ChatCmd::Get { owner, id, reply } => {
                let _ = reply.send(self.view_of(&owner, &id));
            }
            ChatCmd::List { owner, reply } => {
                let mut views: Vec<SessionView> = self
                    .sessions
                    .values_mut()
                    .filter(|live| live.owner == owner)
                    .map(|live| view_from(live))
                    .collect();
                views.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
                let _ = reply.send(views);
            }
            ChatCmd::Send(req) => {
                let _ = req.reply.send(self.send(req.owner, req.id, req.text, req.attachments));
            }
            ChatCmd::Events { owner, id, after, limit, reply } => {
                let _ = reply.send(self.events_after(&owner, &id, after, limit));
            }
            ChatCmd::Cancel { owner, id, reply } => {
                let _ = reply.send(self.cancel(&owner, &id));
            }
            ChatCmd::Retire { owner, id, reply } => {
                let _ = reply.send(self.retire(&owner, &id));
            }
            ChatCmd::ToolResult { owner, id, call_id, outcome, reply } => {
                let _ = reply.send(self.client_tool_result(&owner, &id, &call_id, outcome));
            }
            ChatCmd::Shutdown => self.shutdown_all(),
        }
    }

    fn list_providers(&mut self) -> Vec<ProviderStatus> {
        [ProviderKind::FleetQwen, ProviderKind::OpenAi, ProviderKind::Grok]
            .into_iter()
            .map(|kind| public_status(kind, self.factory.probe(kind)))
            .collect()
    }

    fn create(
        &mut self,
        owner: PrincipalId,
        namespace: String,
        token: String,
        provider: ProviderKind,
        profile: ClientProfile,
    ) -> Result<SessionView, ChatFail> {
        if self.sessions.len() >= self.cfg.max_sessions {
            return Err(ChatFail::OverBudget { what: "chat sessions" });
        }
        let owned = self.sessions.values().filter(|s| s.owner == owner).count();
        if owned >= self.cfg.max_sessions_per_owner {
            return Err(ChatFail::OverBudget { what: "chat sessions per owner" });
        }
        match self.factory.probe(provider) {
            ProviderAvailability::Unavailable { reason } => {
                return Err(ChatFail::ProviderUnavailable { reason: public_reason(&reason) });
            }
            ProviderAvailability::Available { .. } => {}
        }
        let boxed = self.factory.open(provider).map_err(|e| ChatFail::ProviderError {
            message: sanitize_public_error(&e),
        })?;
        let tools = match AssetServerTools::connect(self.endpoints, Some(token), namespace.clone()) {
            Ok(t) => Some(t),
            Err(e) => {
                return Err(ChatFail::ToolsConnect {
                    message: sanitize_public_error(&e.to_string()),
                });
            }
        };
        let session = Session::new(principal_str(&owner), boxed);
        let id = session.id().clone();
        let live = Live {
            owner,
            namespace,
            session,
            tools,
            events: Vec::new(),
            consult_depth: 0,
            profile,
            client_wait: None,
            resolved_tool_calls: Vec::new(),
        };
        let view = view_from(&live);
        self.sessions.insert(id, live);
        Ok(view)
    }

    fn live_mut(&mut self, owner: &PrincipalId, id: &SessionId) -> Result<&mut Live, ChatFail> {
        match self.sessions.get_mut(id) {
            Some(live) if live.owner == *owner => Ok(live),
            Some(_) | None => Err(ChatFail::NotFound),
        }
    }

    fn view_of(&mut self, owner: &PrincipalId, id: &SessionId) -> Result<SessionView, ChatFail> {
        self.live_mut(owner, id).map(|live| view_from(live))
    }

    fn send(
        &mut self,
        owner: PrincipalId,
        id: SessionId,
        text: String,
        attachments: Vec<AttachmentBinding>,
    ) -> Result<u64, ChatFail> {
        let endpoints = self.endpoints;
        let cap = self.cfg.event_cap;
        let live = self.sessions.get_mut(&id).ok_or(ChatFail::NotFound)?;
        if live.owner != owner {
            return Err(ChatFail::NotFound);
        }
        let primary = live.session.provider_kind();
        let result = {
            let mut tools = SessionTools {
                inner: &mut live.tools,
                factory: &mut *self.factory,
                endpoints,
                primary,
                consult_depth: &mut live.consult_depth,
                profile: live.profile,
                catalog: self.catalog.as_mut(),
            };
            live.session.send(&text, &attachments, &mut tools)
        };
        match result {
            Ok(turn) => {
                drain_into_logged(live, cap, self.log_enabled);
                Ok(turn)
            }
            Err(e) => Err(map_send(e)),
        }
    }

    /// Resume a parked client-executed tool with the client's outcome.
    fn client_tool_result(
        &mut self,
        owner: &PrincipalId,
        id: &SessionId,
        call_id: &str,
        outcome: ToolOutcome,
    ) -> Result<(), ChatFail> {
        let endpoints = self.endpoints;
        let cap = self.cfg.event_cap;
        let live = match self.sessions.get_mut(id) {
            Some(live) if live.owner == *owner => live,
            Some(_) | None => return Err(ChatFail::NotFound),
        };
        let primary = live.session.provider_kind();
        let result = {
            let mut tools = SessionTools {
                inner: &mut live.tools,
                factory: &mut *self.factory,
                endpoints,
                primary,
                consult_depth: &mut live.consult_depth,
                profile: live.profile,
                catalog: self.catalog.as_mut(),
            };
            live.session.provide_client_outcome(call_id, outcome, &mut tools)
        };
        match result {
            Ok(()) => {
                live.client_wait = None;
                remember_resolved(live, call_id);
                drain_into_logged(live, cap, self.log_enabled);
                Ok(())
            }
            // A result for a call that already resolved (the app answered
            // twice, or answered after the timeout fed a Failed outcome) is
            // acknowledged idempotently: the turn moved on and the late
            // bytes change nothing. Only a call this session never parked
            // on is a real protocol error.
            Err(_) if live.resolved_tool_calls.iter().any(|c| c == call_id) => Ok(()),
            Err(what) => Err(ChatFail::NoClientTool { what }),
        }
    }

    fn events_after(
        &mut self,
        owner: &PrincipalId,
        id: &SessionId,
        after: u64,
        limit: u32,
    ) -> Result<(Vec<ChatEvent>, u64), ChatFail> {
        let live = self.live_mut(owner, id)?;
        let mut out = Vec::new();
        // `after` is exclusive except for the initial `0`, which means
        // "from the start" and must include seq 0.
        for ev in &live.events {
            let include = if after == 0 { true } else { ev.seq > after };
            if include {
                out.push(ev.clone());
                if out.len() as u32 >= limit {
                    break;
                }
            }
        }
        let cursor = out.last().map(|e| e.seq).unwrap_or(after);
        Ok((out, cursor))
    }

    fn cancel(&mut self, owner: &PrincipalId, id: &SessionId) -> Result<SessionView, ChatFail> {
        let cap = self.cfg.event_cap;
        let live = self.live_mut(owner, id)?;
        live.session.cancel();
        drain_into(live, cap);
        Ok(view_from(live))
    }

    fn retire(&mut self, owner: &PrincipalId, id: &SessionId) -> Result<bool, ChatFail> {
        match self.sessions.get(id) {
            Some(live) if live.owner == *owner => {}
            _ => return Err(ChatFail::NotFound),
        }
        if let Some(mut live) = self.sessions.remove(id) {
            live.session.cancel();
            if let Some(tools) = &mut live.tools {
                let _ = tools.retire_session(live.session.id());
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn pump_all(&mut self) {
        let ids: Vec<SessionId> = self.sessions.keys().cloned().collect();
        for id in ids {
            self.pump_one(&id);
        }
    }

    fn pump_one(&mut self, id: &SessionId) {
        let endpoints = self.endpoints;
        let cap = self.cfg.event_cap;
        let Some(live) = self.sessions.get_mut(id) else {
            return;
        };
        if live.session.is_idle() {
            live.client_wait = None;
            return;
        }
        // A turn parked on a client-executed tool does not pump; it waits
        // for the tool-result route — bounded by CLIENT_TOOL_TIMEOUT.
        if live.session.awaiting_client_tool().is_some() {
            let since = *live.client_wait.get_or_insert_with(Instant::now);
            if since.elapsed() < CLIENT_TOOL_TIMEOUT {
                return;
            }
            let call_id = live
                .session
                .awaiting_client_tool()
                .expect("checked above")
                .to_string();
            let primary = live.session.provider_kind();
            let mut tools = SessionTools {
                inner: &mut live.tools,
                factory: &mut *self.factory,
                endpoints,
                primary,
                consult_depth: &mut live.consult_depth,
                profile: live.profile,
                catalog: self.catalog.as_mut(),
            };
            let _ = live.session.provide_client_outcome(
                &call_id,
                ToolOutcome::Failed {
                    message: "the connected app did not answer this tool call".to_string(),
                },
                &mut tools,
            );
            live.client_wait = None;
            remember_resolved(live, &call_id);
            drain_into_logged(live, cap, self.log_enabled);
            return;
        }
        live.client_wait = None;
        let primary = live.session.provider_kind();
        {
            let mut tools = SessionTools {
                inner: &mut live.tools,
                factory: &mut *self.factory,
                endpoints,
                primary,
                consult_depth: &mut live.consult_depth,
                profile: live.profile,
                catalog: self.catalog.as_mut(),
            };
            let start = Instant::now();
            while !live.session.is_idle() && start.elapsed() < PUMP_SLICE {
                live.session.pump(&mut tools);
            }
        }
        drain_into_logged(live, cap, self.log_enabled);
    }

    fn shutdown_all(&mut self) {
        for (_, mut live) in self.sessions.drain() {
            live.session.cancel();
            if let Some(tools) = &mut live.tools {
                let _ = tools.retire_session(live.session.id());
            }
        }
        log(self.log_enabled, "chat broker stopped");
    }
}

fn drain_into(live: &mut Live, cap: usize) {
    drain_into_logged(live, cap, false)
}

/// The broker's chat observability: with server logging on, every turn is
/// followable from stdout — tool calls with bounded args, bounded results,
/// the finished text, errors. The primary iteration instrument; grabs of
/// the UI are only for judging visuals.
/// Remember a resolved client-tool call id for the idempotent late-post
/// acknowledgement, keeping only the most recent few.
fn remember_resolved(live: &mut Live, call_id: &str) {
    if live.resolved_tool_calls.iter().any(|c| c == call_id) {
        return;
    }
    live.resolved_tool_calls.push(call_id.to_string());
    if live.resolved_tool_calls.len() > 8 {
        live.resolved_tool_calls.remove(0);
    }
}

fn drain_into_logged(live: &mut Live, cap: usize, log_enabled: bool) {
    use makepad_asset_chat::wire::ChatEventBody as B;
    for ev in live.session.drain_events() {
        if log_enabled {
            let sid = live.session.id().as_str();
            match &ev.body {
                B::ToolCall { name, args, .. } => {
                    log(true, &format!("chat[{sid}] call {name} {}", clip(&args.to_json(), 400)));
                }
                B::ToolResult { outcome, .. } => {
                    log(true, &format!("chat[{sid}] result {}", clip(&outcome.encode().to_json(), 400)));
                }
                B::Done => log(true, &format!("chat[{sid}] done")),
                B::Cancelled => log(true, &format!("chat[{sid}] cancelled")),
                B::Error { code, message } => {
                    log(true, &format!("chat[{sid}] ERROR {code}: {message}"));
                }
                B::Delta { .. } | B::ToolProgress { .. } => {}
            }
        }
        live.events.push(ev);
    }
    if live.events.len() > cap {
        let drop_n = live.events.len() - cap;
        live.events.drain(..drop_n);
    }
}

fn clip(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn view_from(live: &Live) -> SessionView {
    let state = if live.session.is_sealed() {
        "sealed"
    } else if live.session.is_idle() {
        "idle"
    } else {
        "streaming"
    };
    SessionView {
        id: live.session.id().clone(),
        namespace: live.namespace.clone(),
        provider: live.session.provider_kind(),
        owner: live.owner,
        state,
        turn: live.session.turn(),
        idle: live.session.is_idle(),
    }
}

fn map_send(e: SendRefusal) -> ChatFail {
    match e {
        SendRefusal::ProviderUnavailable { reason } => {
            ChatFail::ProviderUnavailable { reason: public_reason(&reason) }
        }
        SendRefusal::Busy => ChatFail::Busy,
        SendRefusal::TooLarge { what } => ChatFail::TooLarge { what },
        SendRefusal::TooMany { what } => ChatFail::TooMany { what },
        SendRefusal::HistoryFull => ChatFail::HistoryFull,
        SendRefusal::InvalidAttachment { what } => ChatFail::InvalidAttachment { what },
        SendRefusal::ProviderError { message } => {
            ChatFail::ProviderError { message: sanitize_public_error(&message) }
        }
        SendRefusal::Sealed { reason } => ChatFail::Sealed { reason: public_reason(&reason) },
    }
}

fn public_status(kind: ProviderKind, av: ProviderAvailability) -> ProviderStatus {
    match av {
        ProviderAvailability::Available { model, detail: _ } => ProviderStatus {
            kind,
            available: true,
            model: Some(sanitize_public_error(&model)),
            reason: None,
        },
        ProviderAvailability::Unavailable { reason } => ProviderStatus {
            kind,
            available: false,
            model: None,
            reason: Some(public_reason(&reason)),
        },
    }
}

fn public_reason(reason: &str) -> String {
    let cleaned = sanitize_public_error(reason);
    if looks_like_url(&cleaned) {
        "provider unavailable".to_string()
    } else {
        cleaned
    }
}

fn looks_like_url(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("http://") || lower.contains("https://") || lower.contains("://")
}

// ---------------------------------------------------------------------------
// tool wrapper: asset tools + llm.consult
// ---------------------------------------------------------------------------

struct SessionTools<'a> {
    inner: &'a mut Option<AssetServerTools>,
    factory: &'a mut dyn ProviderFactory,
    endpoints: ApiEndpoints,
    primary: ProviderKind,
    consult_depth: &'a mut u32,
    profile: ClientProfile,
    catalog: Option<&'a mut CatalogReader>,
}

impl SessionTools<'_> {
    /// The dynamic context layer: live catalog headline plus (for general
    /// sessions) the registered-operation capabilities and consult status.
    fn dynamic_context(&mut self) -> String {
        let mut out = String::new();
        if let Some(catalog) = self.catalog.as_mut() {
            match catalog.query(
                "SELECT kind, COUNT(*) AS n FROM search_annotations WHERE live=1 \
                 GROUP BY kind ORDER BY n DESC",
            ) {
                Ok(counts) => {
                    out.push_str("LIVE STORE right now (kind count): ");
                    for (i, row) in counts.rows.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        if let [kind, n] = row.as_slice() {
                            out.push_str(kind);
                            out.push(' ');
                            out.push_str(n);
                        }
                    }
                    out.push('\n');
                }
                Err(e) => out.push_str(&format!("Catalog SQL currently unavailable: {e}\n")),
            }
            if self.profile == ClientProfile::Game {
                // The kit inventory up front: without it the model spends
                // its whole tool budget paging the catalog to learn what
                // exists (iteration 5).
                match catalog.kit_summary() {
                    Ok(kits) => out.push_str(&kits),
                    Err(e) => out.push_str(&format!("(kit summary unavailable: {e})\n")),
                }
            }
        } else {
            out.push_str("Catalog SQL is not configured on this server.\n");
        }
        if self.profile == ClientProfile::Game {
            // Phase 1: existing assets only — no operation/consult teaching,
            // which also keeps the game context small.
            return out;
        }
        out.push('\n');
        match self.inner {
            Some(tools) => out.push_str(&tools.capability_doc()),
            None => out.push_str("Asset tools are not connected.\n"),
        }
        out.push_str("\nExternal generative consult:\n");
        if matches!(self.primary, ProviderKind::OpenAi | ProviderKind::Grok) {
            out.push_str(
                "llm.consult is unavailable: this session is already on an external provider.\n",
            );
            return out;
        }
        let mut any = false;
        for kind in [ProviderKind::OpenAi, ProviderKind::Grok] {
            match self.factory.probe(kind) {
                ProviderAvailability::Available { model, .. } => {
                    out.push_str("- ");
                    out.push_str(kind.slug());
                    out.push_str(" available (");
                    out.push_str(&sanitize_public_error(&model));
                    out.push_str(")\n");
                    any = true;
                }
                ProviderAvailability::Unavailable { reason } => {
                    out.push_str("- ");
                    out.push_str(kind.slug());
                    out.push_str(" unavailable (");
                    out.push_str(&public_reason(&reason));
                    out.push_str(")\n");
                }
            }
        }
        if any {
            out.push_str(
                "Use llm.consult from this local session to draft code, a level, or a design. \
                 The consult cannot run tools.\n",
            );
        }
        out
    }
}

impl ToolExecutor for SessionTools<'_> {
    fn capability_doc(&mut self) -> String {
        let dynamic = self.dynamic_context();
        let _ = self.endpoints;
        context::assemble(self.profile, &dynamic)
    }

    fn tool_definitions(&mut self) -> Vec<ToolDef> {
        match self.profile {
            ClientProfile::Game => tools::game_definitions(),
            ClientProfile::General | ClientProfile::Vj => tools::definitions(),
        }
    }

    /// Game sessions' world tools are executed by the connected app: the
    /// session parks and the outcome arrives over the tool-result route.
    fn client_executes(&mut self, call: &ContentToolCall) -> bool {
        self.profile.client_world_tools()
            && matches!(
                call,
                ContentToolCall::WorldPlace { .. }
                    | ContentToolCall::WorldRemove { .. }
                    | ContentToolCall::WorldMove { .. }
                    | ContentToolCall::WorldList
                    | ContentToolCall::WorldGetSource
                    | ContentToolCall::WorldSetSource { .. }
                    | ContentToolCall::WorldSetPlayerModel { .. }
                    | ContentToolCall::WorldSpawn { .. }
                    | ContentToolCall::WorldTune { .. }
            )
    }

    fn execute(
        &mut self,
        call: &ContentToolCall,
        ctx: &ExecCtx,
        progress: &mut dyn FnMut(u16, &str),
        cancel: &CancelFlag,
    ) -> ToolOutcome {
        match call {
            ContentToolCall::LlmConsult { task, prompt, provider } => {
                self.consult(*task, prompt, *provider, progress, cancel)
            }
            ContentToolCall::AssetsQuery { sql } => match self.catalog.as_mut() {
                Some(catalog) => run_catalog_query(catalog, sql),
                None => ToolOutcome::Unavailable {
                    reason: "catalog SQL is not configured on this server".into(),
                },
            },
            ContentToolCall::AssetsSchema => match self.catalog.as_mut() {
                Some(catalog) => match catalog.schema_text() {
                    Ok(text) => ToolOutcome::Ok {
                        value: json_obj(vec![("text", json_s(text))]),
                    },
                    Err(message) => ToolOutcome::Failed { message },
                },
                None => ToolOutcome::Unavailable {
                    reason: "catalog SQL is not configured on this server".into(),
                },
            },
            other => match self.inner {
                Some(tools) => tools.execute(other, ctx, progress, cancel),
                None => ToolOutcome::Failed { message: "asset tools are not connected".into() },
            },
        }
    }
}

use makepad_asset_client::json::{obj as json_obj_pairs, s as json_s, Value as JsonValue};

fn json_obj(pairs: Vec<(&str, JsonValue)>) -> JsonValue {
    json_obj_pairs(pairs)
}

/// Rendered-table budget per query result. Well under the 16 KiB outcome
/// cap on purpose: results live in the model's HISTORY for the rest of the
/// session, and the serving tier is a local model with a bounded context —
/// a narrower query beats a bigger table (the truncation footer says so).
const MAX_TABLE_BYTES: usize = 6_000;

fn run_catalog_query(catalog: &mut CatalogReader, sql: &str) -> ToolOutcome {
    match catalog.query(sql) {
        Ok(out) if out.rows.is_empty() => ToolOutcome::Ok {
            // A bare "0 rows" teaches nothing and invites retry loops;
            // carry the vocabulary hint in the result itself.
            value: json_obj(vec![
                ("rows", JsonValue::Int(0)),
                (
                    "text",
                    json_s(
                        "(0 rows) Check the vocabulary: SELECT kind, COUNT(*) FROM \
                         search_annotations WHERE live=1 GROUP BY kind — models are \
                         kind 'mesh'; browse one kit with canon_alias LIKE \
                         'kenney/<kit>/%'.",
                    ),
                ),
            ]),
        },
        Ok(out) => {
            let mut shown = out.rows.len();
            let text = loop {
                let view = makepad_asset_chat::catalog_sql::QueryOutput {
                    columns: out.columns.clone(),
                    rows: out.rows[..shown].to_vec(),
                    truncated: out.truncated || shown < out.rows.len(),
                    elapsed_ms: out.elapsed_ms,
                };
                let text = view.to_text();
                if text.len() <= MAX_TABLE_BYTES || shown == 0 {
                    break text;
                }
                shown /= 2;
            };
            ToolOutcome::Ok {
                value: json_obj(vec![
                    ("rows", JsonValue::Int(shown as i64)),
                    ("text", json_s(text)),
                ]),
            }
        }
        Err(message) if message.starts_with("refused") => ToolOutcome::Refused { what: message },
        Err(message) => ToolOutcome::Failed { message },
    }
}

impl SessionTools<'_> {
    fn consult(
        &mut self,
        task: ConsultTask,
        prompt: &str,
        requested: Option<ProviderKind>,
        progress: &mut dyn FnMut(u16, &str),
        cancel: &CancelFlag,
    ) -> ToolOutcome {
        if *self.consult_depth > 0 {
            return ToolOutcome::Refused { what: "nested llm.consult is not allowed".into() };
        }
        if matches!(self.primary, ProviderKind::OpenAi | ProviderKind::Grok) {
            return ToolOutcome::Unavailable {
                reason: "session is already on the external provider".into(),
            };
        }
        if prompt.is_empty() || prompt.len() > MAX_MESSAGE_BYTES {
            return ToolOutcome::Refused { what: "consult prompt empty or too large".into() };
        }
        let kind = match self.pick_consult(requested) {
            Ok(k) => k,
            Err(outcome) => return outcome,
        };
        let mut provider = match self.factory.open(kind) {
            Ok(p) => p,
            Err(e) => {
                return ToolOutcome::Unavailable { reason: public_reason(&e) };
            }
        };
        let model = match provider.availability() {
            ProviderAvailability::Available { model, .. } => sanitize_public_error(&model),
            ProviderAvailability::Unavailable { reason } => {
                return ToolOutcome::Unavailable { reason: public_reason(&reason) };
            }
        };
        *self.consult_depth += 1;
        progress(10, "consulting external model");
        let system = consult_system(task);
        let input = TurnInput {
            system,
            messages: vec![ChatMessage::new(ChatRole::User, prompt.to_string())],
            tools_enabled: false,
        };
        if let Err(e) = provider.begin_turn(&input) {
            *self.consult_depth -= 1;
            return ToolOutcome::Failed { message: sanitize_public_error(&e) };
        }
        let deadline = Instant::now() + CONSULT_TIMEOUT;
        let mut collected = String::new();
        let outcome = loop {
            if cancel.is_cancelled() {
                provider.cancel();
                break ToolOutcome::Failed { message: "consult cancelled".into() };
            }
            if Instant::now() >= deadline {
                provider.cancel();
                break ToolOutcome::Failed { message: "consult timed out".into() };
            }
            let events = provider.poll();
            if events.is_empty() {
                std::thread::sleep(CONSULT_POLL);
                continue;
            }
            let mut done: Option<String> = None;
            let mut failed: Option<String> = None;
            for ev in events {
                match ev {
                    ProviderEvent::Status { note, permille } => {
                        progress(permille.min(999), &note);
                    }
                    ProviderEvent::Delta(text) => {
                        if collected.len().saturating_add(text.len()) > MAX_MESSAGE_BYTES {
                            provider.cancel();
                            failed = Some("consult reply too large".into());
                            break;
                        }
                        collected.push_str(&text);
                    }
                    ProviderEvent::FunctionCall { .. } => {
                        provider.cancel();
                        failed = Some("external consult attempted a tool call".into());
                        break;
                    }
                    ProviderEvent::Error(message) => {
                        failed = Some(sanitize_public_error(&message));
                        break;
                    }
                    ProviderEvent::Done { text } => {
                        done = Some(if text.is_empty() { collected.clone() } else { text });
                    }
                }
            }
            if let Some(message) = failed {
                break ToolOutcome::Failed { message };
            }
            if let Some(text) = done {
                let text = sanitize_consult_text(&text);
                progress(1000, "consult complete");
                break ToolOutcome::Ok {
                    value: makepad_asset_client::json::obj(vec![
                        ("provider", makepad_asset_client::json::s(kind.slug())),
                        ("model", makepad_asset_client::json::s(model)),
                        ("task", makepad_asset_client::json::s(task.slug())),
                        ("text", makepad_asset_client::json::s(text)),
                    ]),
                };
            }
        };
        *self.consult_depth -= 1;
        outcome
    }

    fn pick_consult(&mut self, requested: Option<ProviderKind>) -> Result<ProviderKind, ToolOutcome> {
        let candidates = match requested {
            Some(ProviderKind::OpenAi) => vec![ProviderKind::OpenAi],
            Some(ProviderKind::Grok) => vec![ProviderKind::Grok],
            Some(ProviderKind::FleetQwen) => {
                return Err(ToolOutcome::Refused {
                    what: "llm.consult cannot target the local provider".into(),
                });
            }
            None => vec![ProviderKind::Grok, ProviderKind::OpenAi],
        };
        let mut reasons = Vec::new();
        for kind in candidates {
            match self.factory.probe(kind) {
                ProviderAvailability::Available { .. } => return Ok(kind),
                ProviderAvailability::Unavailable { reason } => {
                    reasons.push(format!("{}: {}", kind.slug(), public_reason(&reason)));
                }
            }
        }
        Err(ToolOutcome::Unavailable {
            reason: if reasons.is_empty() {
                "no external provider is configured".into()
            } else {
                reasons.join("; ")
            },
        })
    }
}

fn consult_system(task: ConsultTask) -> String {
    match task {
        ConsultTask::Code => {
            "You draft game code. Reply with the code only. Do not call tools.".into()
        }
        ConsultTask::Level => {
            "You draft a game level or scene. Reply with the level description or script only. \
             Do not call tools."
                .into()
        }
        ConsultTask::Design => {
            "You draft a game-design note. Reply with the design only. Do not call tools.".into()
        }
    }
}

fn sanitize_consult_text(text: &str) -> String {
    let cleaned = sanitize_public_error(text);
    if cleaned.len() <= MAX_MESSAGE_BYTES {
        if cleaned.is_empty() {
            "(empty consult)".into()
        } else {
            cleaned
        }
    } else {
        let mut end = MAX_MESSAGE_BYTES;
        while end > 0 && !cleaned.is_char_boundary(end) {
            end -= 1;
        }
        cleaned[..end].to_string()
    }
}

// ---------------------------------------------------------------------------
// providers
// ---------------------------------------------------------------------------

trait ProviderFactory: Send {
    fn probe(&mut self, kind: ProviderKind) -> ProviderAvailability;
    fn open(&mut self, kind: ProviderKind) -> Result<Box<dyn ChatProvider>, String>;
}

struct EnvFactory {
    fleet_bases: Vec<String>,
    fleet: String,
    /// Availability rows are a fleet SCAN each; a create used to pay two
    /// (probe + wrapper seed) and a bursty dead window turned both into a
    /// user-visible 503. Cache per kind for a short TTL.
    availability: Vec<(ProviderKind, ProviderAvailability, Instant)>,
}

const AVAILABILITY_TTL: Duration = Duration::from_secs(20);

impl EnvFactory {
    fn bases(&self) -> Vec<String> {
        makepad_asset_chat::fleet_discovery::set_wanted_fleet(&self.fleet);
        makepad_asset_chat::fleet_discovery::seed_bases(self.fleet_bases.clone());
        makepad_asset_chat::fleet_discovery::live_bases()
    }

    /// The unwrapped (synchronous) provider — status probes only.
    fn open_raw(&mut self, kind: ProviderKind) -> Result<Box<dyn ChatProvider>, String> {
        match kind {
            ProviderKind::FleetQwen => Ok(Box::new(FleetQwenChatProvider::new(
                HttpFleetTransport,
                self.bases(),
            ))),
            ProviderKind::OpenAi => Ok(Box::new(makepad_asset_chat::openai::from_env())),
            ProviderKind::Grok => Ok(Box::new(makepad_asset_chat::grok::from_env())),
        }
    }
}

impl ProviderFactory for EnvFactory {
    fn probe(&mut self, kind: ProviderKind) -> ProviderAvailability {
        if let Some((_, av, at)) =
            self.availability.iter().find(|(k, _, _)| *k == kind)
        {
            if at.elapsed() < AVAILABILITY_TTL {
                return av.clone();
            }
        }
        let av = match self.open_raw(kind) {
            Ok(mut p) => strip_location(p.availability()),
            Err(reason) => ProviderAvailability::Unavailable { reason },
        };
        self.availability.retain(|(k, _, _)| *k != kind);
        self.availability.push((kind, av.clone(), Instant::now()));
        av
    }

    fn open(&mut self, kind: ProviderKind) -> Result<Box<dyn ChatProvider>, String> {
        // Sessions get the provider behind ThreadedProvider so the broker
        // actor NEVER blocks on provider HTTP: the fleet begin_turn's ~18 s
        // flaky-LAN backoff ran inline on the actor and starved every other
        // session's events/tool-result routes (entry 13, act three). The
        // seed availability comes from the cached probe (create just did
        // one) instead of a second fleet scan.
        let seed = self.probe(kind);
        match kind {
            ProviderKind::FleetQwen => {
                let inner = FleetQwenChatProvider::new(HttpFleetTransport, self.bases());
                Ok(Box::new(ThreadedProvider::spawn(inner, seed)))
            }
            ProviderKind::OpenAi => {
                let inner = makepad_asset_chat::openai::from_env();
                Ok(Box::new(ThreadedProvider::spawn(inner, seed)))
            }
            ProviderKind::Grok => {
                let inner = makepad_asset_chat::grok::from_env();
                Ok(Box::new(ThreadedProvider::spawn(inner, seed)))
            }
        }
    }
}

fn strip_location(av: ProviderAvailability) -> ProviderAvailability {
    match av {
        ProviderAvailability::Available { model, detail: _ } => {
            ProviderAvailability::Available { model, detail: String::new() }
        }
        other => other,
    }
}

struct ScriptedFactory {
    fleet_qwen: ScriptedLane,
    openai: ScriptedLane,
    grok: ScriptedLane,
}

impl ScriptedFactory {
    fn new(script: ChatScript) -> ScriptedFactory {
        ScriptedFactory {
            fleet_qwen: script.fleet_qwen,
            openai: script.openai,
            grok: script.grok,
        }
    }

    fn lane(&self, kind: ProviderKind) -> &ScriptedLane {
        match kind {
            ProviderKind::FleetQwen => &self.fleet_qwen,
            ProviderKind::OpenAi => &self.openai,
            ProviderKind::Grok => &self.grok,
        }
    }
}

impl ProviderFactory for ScriptedFactory {
    fn probe(&mut self, kind: ProviderKind) -> ProviderAvailability {
        let lane = self.lane(kind);
        if lane.available {
            ProviderAvailability::Available {
                model: if lane.model.is_empty() {
                    "scripted".into()
                } else {
                    lane.model.clone()
                },
                detail: String::new(),
            }
        } else {
            ProviderAvailability::Unavailable {
                reason: format!("{} is not configured in this test", kind.slug()),
            }
        }
    }

    fn open(&mut self, kind: ProviderKind) -> Result<Box<dyn ChatProvider>, String> {
        match self.probe(kind) {
            ProviderAvailability::Unavailable { reason } => Err(reason),
            ProviderAvailability::Available { model, .. } => {
                Ok(Box::new(ScriptedProvider {
                    kind,
                    model,
                    turns: self.lane(kind).turns.clone(),
                    pending: Vec::new(),
                }))
            }
        }
    }
}

struct ScriptedProvider {
    kind: ProviderKind,
    model: String,
    turns: Vec<ScriptedTurn>,
    pending: Vec<ProviderEvent>,
}

impl ChatProvider for ScriptedProvider {
    fn kind(&self) -> ProviderKind {
        self.kind
    }

    fn availability(&mut self) -> ProviderAvailability {
        ProviderAvailability::Available { model: self.model.clone(), detail: String::new() }
    }

    fn begin_turn(&mut self, _input: &TurnInput) -> Result<(), String> {
        if !self.pending.is_empty() {
            return Err("a turn is already in flight".into());
        }
        if self.turns.is_empty() {
            return Err("script exhausted".into());
        }
        let turn = self.turns.remove(0);
        let text = match turn {
            ScriptedTurn::Text(text) => text,
            ScriptedTurn::Consult { task, prompt, provider, visible } => {
                let args = format!(
                    r#"{{"name":"llm.consult","args":{{"task":"{task}","prompt":{prompt},"provider":"{provider}"}}}}"#,
                    prompt = makepad_asset_client::json::s(prompt).to_json(),
                );
                format!("{visible}\n<<tool>>{args}")
            }
        };
        self.pending = vec![ProviderEvent::Delta(text.clone()), ProviderEvent::Done { text }];
        Ok(())
    }

    fn poll(&mut self) -> Vec<ProviderEvent> {
        std::mem::take(&mut self.pending)
    }

    fn cancel(&mut self) {
        self.pending.clear();
    }
}

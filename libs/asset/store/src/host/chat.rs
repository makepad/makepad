//! Broker-owned chat sessions: a registry plus ONE WORKER THREAD PER
//! SESSION.
//!
//! The control plane only authenticates and forwards; this module owns every
//! [`Session`], constructs providers from server-side config (never from the
//! client wire), and executes tools. Local fleet Qwen and external OpenAI /
//! Grok are explicit peers. A local session may `llm.consult` the external
//! provider for text-only code/level/design drafts; that consult cannot run
//! tools or nest another session.
//!
//! ## Why per-session threads
//!
//! This used to be one actor thread that owned every session and served one
//! command per pump round. Everything blocking ran on it: tool execution
//! (`operation.wait` is bounded by the MODEL's `timeout_ms`, up to two
//! minutes), `llm.consult`'s 60 s inline loop, the create path's fleet probe
//! and its `AssetServerTools::connect` HTTP call. One session doing any of
//! those froze the whole broker — which is exactly how a `create` timed out
//! at 15 s and answered `503 state unavailable` while the suite hammered it.
//! N sessions also never ran concurrently in any real sense: their turns
//! advanced round-robin, 50 ms at a time, from one thread that BUSY-SPUN
//! through each slice.
//!
//! Now:
//! - Each session owns a worker thread that pumps only itself. Tools,
//!   consults, and provider rounds are that session's own cost.
//! - The registry is a mutex-protected map. Route lookups take it for
//!   microseconds.
//! - `/events` and every session view read shared mutexes the worker
//!   publishes into — they can never queue behind a provider round or a
//!   two-minute tool.
//! - `create` runs on the caller's HTTP thread: it reserves a slot under the
//!   registry lock, then its own worker does the probe/connect. A busy
//!   broker cannot 503 it.
//! - Availability is probed ONCE per kind per TTL for the whole server
//!   (shared probe cells), and every fleet provider shares one node pick.
//!
//! One turn at a time per session is unchanged — that is the session
//! engine's law, not a threading artifact.

use super::api::principal_str;
use super::config::{ChatConfig, ChatScript, ScriptedLane, ScriptedTurn};
use super::util::log;
use makepad_asset_chat::catalog_sql::CatalogReader;
use makepad_asset_chat::context::{self, ClientProfile};
use makepad_asset_chat::dispatch::AssetServerTools;
use makepad_asset_chat::provider::{ChatProvider, ProviderEvent, ThreadedProvider, TurnInput};
use makepad_asset_chat::qwen::{FleetPickCache, FleetQwenChatProvider, HttpFleetTransport};
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
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const CONSULT_POLL: Duration = Duration::from_millis(20);
const CONSULT_TIMEOUT: Duration = Duration::from_secs(60);
/// How often a worker with a LIVE turn drains its provider. The provider
/// runs on its own thread and batches, so this is latency, not throughput —
/// and it replaces a 50 ms busy-spin that burned a core per 20 sessions.
const STREAM_TICK: Duration = Duration::from_millis(10);
/// How often an idle worker wakes just to notice shutdown.
const IDLE_TICK: Duration = Duration::from_millis(250);
/// Supervisor wakeup (reaps retired workers, watches the stop flag).
const SUPERVISOR_TICK: Duration = Duration::from_millis(200);
/// How long a client-executed tool may wait for the app's answer before the
/// broker times the round out with a `Failed` outcome the model can react
/// to. The worker sleeps on its command channel meanwhile — a first
/// game.map stream can take over 30 s.
const CLIENT_TOOL_TIMEOUT: Duration = Duration::from_secs(120);
/// Bound on a route's wait for a worker that must MUTATE the session
/// (create/send/tool-result). Reads never wait at all.
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// Cancel is best effort: the flag is raised synchronously, and this is only
/// how long we wait for the worker to publish the post-cancel view.
const CANCEL_ACK: Duration = Duration::from_secs(2);
/// Resolved client-tool call ids remembered per session for the idempotent
/// late-post acknowledgement.
const RESOLVED_MEMORY: usize = 8;

#[derive(Clone)]
pub struct ChatHandle {
    reg: Arc<Registry>,
    stop_tx: Sender<()>,
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

// ---------------------------------------------------------------------------
// shared per-session state
// ---------------------------------------------------------------------------

/// What the session's worker publishes and every route reads without
/// touching the worker.
struct Status {
    state: &'static str,
    turn: u64,
    idle: bool,
    /// Call id of a parked client-executed tool, published BEFORE the
    /// matching ToolCall event so a tool-result can never race ahead of the
    /// state that accepts it.
    awaiting: Option<String>,
    /// Call ids already resolved (by the app or by the timeout). A LATE or
    /// duplicate tool-result for one of these is acknowledged idempotently
    /// instead of 409ing — the turn has already moved on, and punishing the
    /// app for a slow answer turned a hiccup into a user-visible dead turn
    /// (play-session-1 entry 13). Bounded.
    resolved: Vec<String>,
}

struct Shared {
    id: SessionId,
    owner: PrincipalId,
    namespace: String,
    provider: ProviderKind,
    tx: Sender<SessionCmd>,
    /// Raised from ANY thread to interrupt a long tool immediately; the
    /// Cancel command that follows does the state transition.
    cancel: CancelFlag,
    status: Mutex<Status>,
    events: Mutex<Vec<ChatEvent>>,
}

impl Shared {
    fn status(&self) -> MutexGuard<'_, Status> {
        self.status.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn events(&self) -> MutexGuard<'_, Vec<ChatEvent>> {
        self.events.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn view(&self) -> SessionView {
        let status = self.status();
        SessionView {
            id: self.id.clone(),
            namespace: self.namespace.clone(),
            provider: self.provider,
            owner: self.owner,
            state: status.state,
            turn: status.turn,
            idle: status.idle,
        }
    }
}

enum SessionCmd {
    Send {
        text: String,
        attachments: Vec<AttachmentBinding>,
        reply: Sender<Result<u64, ChatFail>>,
    },
    ToolResult {
        call_id: String,
        outcome: ToolOutcome,
        reply: Sender<Result<(), ChatFail>>,
    },
    Cancel {
        reply: Sender<()>,
    },
    Shutdown,
}

// ---------------------------------------------------------------------------
// registry
// ---------------------------------------------------------------------------

struct Slot {
    shared: Arc<Shared>,
    join: JoinHandle<()>,
}

#[derive(Default)]
struct Inner {
    sessions: HashMap<SessionId, Slot>,
    /// Owners of creates that reserved a slot but have not landed yet, so
    /// concurrent creates cannot both slip past the cap.
    creating: Vec<PrincipalId>,
    /// Workers of retired sessions, joined by the supervisor.
    graveyard: Vec<JoinHandle<()>>,
    closed: bool,
}

impl Inner {
    /// Drop one create-in-flight reservation.
    fn release(&mut self, owner: &PrincipalId) {
        if let Some(i) = self.creating.iter().position(|o| o == owner) {
            self.creating.remove(i);
        }
    }
}

struct Registry {
    endpoints: ApiEndpoints,
    cfg: ChatConfig,
    factory: Arc<dyn ProviderFactory>,
    catalog_db: Option<PathBuf>,
    log_enabled: bool,
    stop: Arc<AtomicBool>,
    inner: Mutex<Inner>,
}

impl Registry {
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lookup(&self, owner: &PrincipalId, id: &SessionId) -> Result<Arc<Shared>, ChatFail> {
        let inner = self.inner();
        match inner.sessions.get(id) {
            Some(slot) if slot.shared.owner == *owner => Ok(slot.shared.clone()),
            Some(_) | None => Err(ChatFail::NotFound),
        }
    }

    /// Reserve one session slot against both caps, atomically with every
    /// other create in flight.
    fn reserve(&self, owner: PrincipalId) -> Result<(), ChatFail> {
        let mut inner = self.inner();
        if inner.closed {
            return Err(ChatFail::Down);
        }
        let live = inner.sessions.len() + inner.creating.len();
        if live >= self.cfg.max_sessions {
            return Err(ChatFail::OverBudget { what: "chat sessions" });
        }
        let owned = inner.sessions.values().filter(|s| s.shared.owner == owner).count()
            + inner.creating.iter().filter(|o| **o == owner).count();
        if owned >= self.cfg.max_sessions_per_owner {
            return Err(ChatFail::OverBudget { what: "chat sessions per owner" });
        }
        inner.creating.push(owner);
        Ok(())
    }

    /// Join every worker that already finished; called from the supervisor.
    fn reap(&self) {
        let mut inner = self.inner();
        let mut still = Vec::new();
        for join in inner.graveyard.drain(..) {
            if join.is_finished() {
                let _ = join.join();
            } else {
                still.push(join);
            }
        }
        inner.graveyard = still;
    }

    fn shutdown_all(&self) {
        let (slots, graveyard) = {
            let mut inner = self.inner();
            inner.closed = true;
            let slots: Vec<Slot> = inner.sessions.drain().map(|(_, slot)| slot).collect();
            let graveyard: Vec<JoinHandle<()>> = inner.graveyard.drain(..).collect();
            (slots, graveyard)
        };
        // Raise every cancel flag FIRST so workers inside a long tool start
        // unwinding while we are still asking the rest to stop.
        for slot in &slots {
            slot.shared.cancel.cancel();
            let _ = slot.shared.tx.send(SessionCmd::Shutdown);
        }
        for slot in slots {
            let _ = slot.join.join();
        }
        for join in graveyard {
            let _ = join.join();
        }
        log(self.log_enabled, "chat broker stopped");
    }
}

pub fn spawn_broker(
    endpoints: ApiEndpoints,
    cfg: ChatConfig,
    catalog_db: Option<PathBuf>,
    log_enabled: bool,
    stop: Arc<AtomicBool>,
) -> Result<(ChatHandle, JoinHandle<()>), crate::ServerError> {
    let factory: Arc<dyn ProviderFactory> = match cfg.script.clone() {
        Some(script) => Arc::new(ScriptedFactory::new(script)),
        None => Arc::new(EnvFactory::new(cfg.fleet_bases.clone(), cfg.fleet.clone())),
    };
    let reg = Arc::new(Registry {
        endpoints,
        cfg,
        factory,
        // The broker shares a process (and root) with the catalog:
        // `assets.query` reads the SAME file the store serves, through
        // makepad-sqlite's read-only WAL snapshot reads.
        catalog_db,
        log_enabled,
        stop,
        inner: Mutex::new(Inner::default()),
    });
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let supervisor = reg.clone();
    let join = std::thread::Builder::new()
        .name("asset-server-chat".into())
        .spawn(move || loop {
            match stop_rx.recv_timeout(SUPERVISOR_TICK) {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                    supervisor.shutdown_all();
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {
                    if supervisor.stop.load(Ordering::Relaxed) {
                        supervisor.shutdown_all();
                        break;
                    }
                    supervisor.reap();
                }
            }
        })
        .map_err(|e| crate::ServerError::Io {
            op: "spawn chat broker",
            kind: e.kind(),
        })?;
    Ok((ChatHandle { reg, stop_tx }, join))
}

impl ChatHandle {
    pub fn list_providers(&self) -> Result<Vec<ProviderStatus>, ChatFail> {
        Ok([ProviderKind::FleetQwen, ProviderKind::OpenAi, ProviderKind::Grok]
            .into_iter()
            .map(|kind| public_status(kind, self.reg.factory.probe(kind)))
            .collect())
    }

    /// Build one session. Runs on the CALLER's thread: the only shared thing
    /// it touches is the registry lock (microseconds) and the shared probe
    /// cell. The probe, the tools connect and `Session::new` happen on the
    /// new session's own worker, so no other session can delay this and this
    /// can delay no other session.
    pub fn create(
        &self,
        owner: PrincipalId,
        namespace: String,
        token: String,
        provider: ProviderKind,
        profile: ClientProfile,
    ) -> Result<SessionView, ChatFail> {
        self.reg.reserve(owner)?;
        // Every exit of spawn_session releases the reservation, together
        // with whatever it does to the session map.
        self.spawn_session(owner, namespace, token, provider, profile)
    }

    fn spawn_session(
        &self,
        owner: PrincipalId,
        namespace: String,
        token: String,
        provider: ProviderKind,
        profile: ClientProfile,
    ) -> Result<SessionView, ChatFail> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<SessionCmd>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<Arc<Shared>, ChatFail>>();
        let reg = self.reg.clone();
        let init = SessionInit {
            owner,
            namespace,
            token,
            provider,
            profile,
            tx: cmd_tx,
            rx: cmd_rx,
            ready: ready_tx,
        };
        let join = std::thread::Builder::new()
            .name("chat-session".into())
            .spawn(move || run_session(reg, init))
            .map_err(|_| ChatFail::Down)?;
        match ready_rx.recv_timeout(CALL_TIMEOUT) {
            Ok(Ok(shared)) => {
                let view = shared.view();
                // Land the session and drop the reservation in ONE lock
                // acquisition: a gap between them would count this owner
                // twice and could refuse a concurrent create that fits.
                let mut inner = self.reg.inner();
                inner.release(&owner);
                if inner.closed {
                    drop(inner);
                    let _ = shared.tx.send(SessionCmd::Shutdown);
                    let _ = join.join();
                    return Err(ChatFail::Down);
                }
                inner.sessions.insert(shared.id.clone(), Slot { shared, join });
                Ok(view)
            }
            Ok(Err(fail)) => {
                self.reg.inner().release(&owner);
                let _ = join.join();
                Err(fail)
            }
            Err(_) => {
                // The worker is wedged in its own connect; let the
                // supervisor reap it rather than blocking this request.
                let mut inner = self.reg.inner();
                inner.release(&owner);
                inner.graveyard.push(join);
                Err(ChatFail::Down)
            }
        }
    }

    /// Deliver the client's outcome for a parked client-executed tool.
    pub fn tool_result(
        &self,
        owner: PrincipalId,
        id: SessionId,
        call_id: String,
        outcome: ToolOutcome,
    ) -> Result<(), ChatFail> {
        let shared = self.reg.lookup(&owner, &id)?;
        // Answer from the published status wherever we can, so a late or
        // duplicate post never queues behind the session's own work.
        {
            let status = shared.status();
            match &status.awaiting {
                Some(awaiting) if awaiting == &call_id => {}
                other => {
                    if status.resolved.iter().any(|c| c == &call_id) {
                        return Ok(());
                    }
                    return Err(ChatFail::NoClientTool {
                        what: match other {
                            Some(_) => "tool call id mismatch",
                            None => "no client tool is awaiting a result",
                        },
                    });
                }
            }
        }
        let (tx, rx) = mpsc::channel();
        shared
            .tx
            .send(SessionCmd::ToolResult { call_id, outcome, reply: tx })
            .map_err(|_| ChatFail::Down)?;
        rx.recv_timeout(CALL_TIMEOUT).map_err(|_| ChatFail::Down)?
    }

    pub fn get(&self, owner: PrincipalId, id: SessionId) -> Result<SessionView, ChatFail> {
        Ok(self.reg.lookup(&owner, &id)?.view())
    }

    /// Every live session the owner may see (id-sorted).
    pub fn list(&self, owner: PrincipalId) -> Result<Vec<SessionView>, ChatFail> {
        let shared: Vec<Arc<Shared>> = {
            let inner = self.reg.inner();
            inner
                .sessions
                .values()
                .filter(|slot| slot.shared.owner == owner)
                .map(|slot| slot.shared.clone())
                .collect()
        };
        let mut views: Vec<SessionView> = shared.iter().map(|s| s.view()).collect();
        views.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
        Ok(views)
    }

    pub fn send(
        &self,
        owner: PrincipalId,
        id: SessionId,
        text: String,
        attachments: Vec<AttachmentBinding>,
    ) -> Result<u64, ChatFail> {
        let shared = self.reg.lookup(&owner, &id)?;
        // One turn at a time is the session engine's law; refusing here
        // keeps a busy session's second send off the worker's queue (where
        // it would otherwise wait behind the very turn it collides with).
        if !shared.status().idle {
            return Err(ChatFail::Busy);
        }
        let (tx, rx) = mpsc::channel();
        shared
            .tx
            .send(SessionCmd::Send { text, attachments, reply: tx })
            .map_err(|_| ChatFail::Down)?;
        rx.recv_timeout(CALL_TIMEOUT).map_err(|_| ChatFail::Down)?
    }

    pub fn events(
        &self,
        owner: PrincipalId,
        id: SessionId,
        after: u64,
        limit: u32,
    ) -> Result<(Vec<ChatEvent>, u64), ChatFail> {
        let shared = self.reg.lookup(&owner, &id)?;
        let mut out = Vec::new();
        // `after` is exclusive except for the initial `0`, which means
        // "from the start" and must include seq 0.
        for ev in shared.events().iter() {
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

    pub fn cancel(&self, owner: PrincipalId, id: SessionId) -> Result<SessionView, ChatFail> {
        let shared = self.reg.lookup(&owner, &id)?;
        // Raise the flag first: a tool already running (operation.wait runs
        // up to two minutes) observes it immediately, long before the
        // command below reaches the worker's queue.
        shared.cancel.cancel();
        let (tx, rx) = mpsc::channel();
        if shared.tx.send(SessionCmd::Cancel { reply: tx }).is_ok() {
            // Best effort: a worker mid-tool answers late, and the caller
            // still gets an honest current view rather than a 503.
            let _ = rx.recv_timeout(CANCEL_ACK);
        }
        Ok(shared.view())
    }

    pub fn retire(&self, owner: PrincipalId, id: SessionId) -> Result<bool, ChatFail> {
        let slot = {
            let mut inner = self.reg.inner();
            match inner.sessions.get(&id) {
                Some(slot) if slot.shared.owner == owner => {}
                Some(_) | None => return Err(ChatFail::NotFound),
            }
            inner.sessions.remove(&id)
        };
        let Some(slot) = slot else {
            return Ok(false);
        };
        slot.shared.cancel.cancel();
        let _ = slot.shared.tx.send(SessionCmd::Shutdown);
        // The worker retires its own tools on the way out. Joining here
        // would make DELETE wait on whatever tool is in flight.
        self.reg.inner().graveyard.push(slot.join);
        Ok(true)
    }

    pub fn shutdown(&self) {
        let _ = self.stop_tx.send(());
    }
}

// ---------------------------------------------------------------------------
// the session worker
// ---------------------------------------------------------------------------

struct SessionInit {
    owner: PrincipalId,
    namespace: String,
    token: String,
    provider: ProviderKind,
    profile: ClientProfile,
    tx: Sender<SessionCmd>,
    rx: mpsc::Receiver<SessionCmd>,
    ready: Sender<Result<Arc<Shared>, ChatFail>>,
}

/// Build the session (probe, provider, tools) and then own it for life.
/// Every blocking step here is this session's own cost.
fn run_session(reg: Arc<Registry>, init: SessionInit) {
    let SessionInit { owner, namespace, token, provider, profile, tx, rx, ready } = init;
    match reg.factory.probe(provider) {
        ProviderAvailability::Unavailable { reason } => {
            let _ = ready.send(Err(ChatFail::ProviderUnavailable {
                reason: public_reason(&reason),
            }));
            return;
        }
        ProviderAvailability::Available { .. } => {}
    }
    let boxed = match reg.factory.open(provider) {
        Ok(p) => p,
        Err(e) => {
            let _ = ready.send(Err(ChatFail::ProviderError {
                message: sanitize_public_error(&e),
            }));
            return;
        }
    };
    let tools = match AssetServerTools::connect(reg.endpoints, Some(token), namespace.clone()) {
        Ok(t) => t,
        Err(e) => {
            let _ = ready.send(Err(ChatFail::ToolsConnect {
                message: sanitize_public_error(&e.to_string()),
            }));
            return;
        }
    };
    let session = Session::new(principal_str(&owner), boxed);
    let shared = Arc::new(Shared {
        id: session.id().clone(),
        owner,
        namespace,
        provider,
        tx,
        cancel: session.cancel_flag(),
        status: Mutex::new(Status {
            state: "idle",
            turn: 0,
            idle: true,
            awaiting: None,
            resolved: Vec::new(),
        }),
        events: Mutex::new(Vec::new()),
    });
    let mut worker = Worker {
        shared: shared.clone(),
        session,
        tools: Some(tools),
        catalog: reg.catalog_db.clone().map(CatalogReader::new),
        factory: reg.factory.clone(),
        endpoints: reg.endpoints,
        profile,
        consult_depth: 0,
        event_cap: reg.cfg.event_cap,
        log_enabled: reg.log_enabled,
        client_wait: None,
    };
    if ready.send(Ok(shared)).is_err() {
        // The creator gave up (or the broker closed) before we landed.
        worker.teardown();
        return;
    }
    worker.run(&rx, &reg.stop);
    worker.teardown();
}

/// Borrow the worker's tool surface WITHOUT borrowing the whole worker, so
/// the session stays mutably available to the caller (disjoint fields).
macro_rules! executor {
    ($w:ident) => {{
        let primary = $w.session.provider_kind();
        SessionTools {
            inner: &mut $w.tools,
            factory: &*$w.factory,
            endpoints: $w.endpoints,
            primary,
            consult_depth: &mut $w.consult_depth,
            profile: $w.profile,
            catalog: $w.catalog.as_mut(),
        }
    }};
}

struct Worker {
    shared: Arc<Shared>,
    session: Session,
    tools: Option<AssetServerTools>,
    catalog: Option<CatalogReader>,
    factory: Arc<dyn ProviderFactory>,
    endpoints: ApiEndpoints,
    profile: ClientProfile,
    consult_depth: u32,
    event_cap: usize,
    log_enabled: bool,
    /// When a client-executed tool parked the turn: since when.
    client_wait: Option<Instant>,
}

impl Worker {
    fn run(&mut self, rx: &mpsc::Receiver<SessionCmd>, stop: &AtomicBool) {
        loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            match rx.recv_timeout(self.tick()) {
                Ok(SessionCmd::Shutdown) => return,
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.pump();
        }
    }

    /// How long this worker may sleep before it must act on its own.
    fn tick(&self) -> Duration {
        if self.session.is_idle() {
            return IDLE_TICK;
        }
        if self.session.awaiting_client_tool().is_some() {
            // Sleep until the client tool's deadline; the answer normally
            // arrives as a command and wakes us far sooner.
            let since = self.client_wait.unwrap_or_else(Instant::now);
            return CLIENT_TOOL_TIMEOUT.saturating_sub(since.elapsed()).max(STREAM_TICK);
        }
        STREAM_TICK
    }

    fn handle(&mut self, cmd: SessionCmd) {
        match cmd {
            SessionCmd::Send { text, attachments, reply } => {
                let result = {
                    let mut exec = executor!(self);
                    self.session.send(&text, &attachments, &mut exec)
                };
                self.publish();
                let _ = reply.send(result.map_err(map_send));
            }
            SessionCmd::ToolResult { call_id, outcome, reply } => {
                let result = {
                    let mut exec = executor!(self);
                    self.session.provide_client_outcome(&call_id, outcome, &mut exec)
                };
                let answer = match result {
                    Ok(()) => {
                        self.client_wait = None;
                        self.remember_resolved(&call_id);
                        Ok(())
                    }
                    // A result for a call that already resolved (the app
                    // answered twice, or answered after the timeout fed a
                    // Failed outcome) is acknowledged idempotently: the turn
                    // moved on and the late bytes change nothing.
                    Err(_) if self.was_resolved(&call_id) => Ok(()),
                    Err(what) => Err(ChatFail::NoClientTool { what }),
                };
                self.publish();
                let _ = reply.send(answer);
            }
            SessionCmd::Cancel { reply } => {
                self.session.cancel();
                self.client_wait = None;
                self.publish();
                let _ = reply.send(());
            }
            SessionCmd::Shutdown => {}
        }
    }

    fn pump(&mut self) {
        if self.session.is_idle() {
            self.client_wait = None;
            return;
        }
        // A turn parked on a client-executed tool does not pump; it waits
        // for the tool-result route — bounded by CLIENT_TOOL_TIMEOUT.
        if let Some(call_id) = self.session.awaiting_client_tool().map(str::to_string) {
            let since = *self.client_wait.get_or_insert_with(Instant::now);
            if since.elapsed() < CLIENT_TOOL_TIMEOUT {
                return;
            }
            {
                let mut exec = executor!(self);
                let _ = self.session.provide_client_outcome(
                    &call_id,
                    ToolOutcome::Failed {
                        message: "the connected app did not answer this tool call".to_string(),
                    },
                    &mut exec,
                );
            }
            self.client_wait = None;
            self.remember_resolved(&call_id);
            self.publish();
            return;
        }
        self.client_wait = None;
        {
            let mut exec = executor!(self);
            self.session.pump(&mut exec);
        }
        self.publish();
    }

    /// Publish state, then events. STATUS FIRST: a client can only learn a
    /// parked call id from the events, so a tool-result can never arrive
    /// before the status that accepts it.
    fn publish(&mut self) {
        {
            let mut status = self.shared.status();
            status.idle = self.session.is_idle();
            status.turn = self.session.turn();
            status.state = if self.session.is_sealed() {
                "sealed"
            } else if status.idle {
                "idle"
            } else {
                "streaming"
            };
            status.awaiting = self.session.awaiting_client_tool().map(str::to_string);
        }
        let drained = self.session.drain_events();
        if drained.is_empty() {
            return;
        }
        if self.log_enabled {
            self.log_events(&drained);
        }
        let mut events = self.shared.events();
        events.extend(drained);
        if events.len() > self.event_cap {
            let drop_n = events.len() - self.event_cap;
            events.drain(..drop_n);
        }
    }

    /// The broker's chat observability: with server logging on, every turn
    /// is followable from stdout — tool calls with bounded args, bounded
    /// results, the finished text, errors. The primary iteration
    /// instrument; grabs of the UI are only for judging visuals.
    fn log_events(&self, events: &[ChatEvent]) {
        use makepad_asset_chat::wire::ChatEventBody as B;
        let sid = self.session.id().as_str();
        for ev in events {
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
    }

    fn remember_resolved(&mut self, call_id: &str) {
        let mut status = self.shared.status();
        if status.resolved.iter().any(|c| c == call_id) {
            return;
        }
        status.resolved.push(call_id.to_string());
        if status.resolved.len() > RESOLVED_MEMORY {
            status.resolved.remove(0);
        }
    }

    fn was_resolved(&self, call_id: &str) -> bool {
        self.shared.status().resolved.iter().any(|c| c == call_id)
    }

    fn teardown(&mut self) {
        self.session.cancel();
        if let Some(tools) = &mut self.tools {
            let _ = tools.retire_session(self.session.id());
        }
        self.publish();
    }
}

fn clip(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
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
    factory: &'a dyn ProviderFactory,
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
                    | ContentToolCall::WorldAddAddon { .. }
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

/// Shared by every session worker and every create in flight, so it answers
/// on `&self` and does its own locking.
trait ProviderFactory: Send + Sync {
    fn probe(&self, kind: ProviderKind) -> ProviderAvailability;
    fn open(&self, kind: ProviderKind) -> Result<Box<dyn ChatProvider>, String>;
}

/// One kind's availability row plus the gate that keeps its probe SINGLE.
///
/// An availability row is a fleet SCAN. Before this, a create paid one and
/// the session's provider paid another, and N sessions multiplied both — so
/// a bursty dead window turned into a user-visible 503. Now: a fresh value
/// answers from memory; a stale value answers immediately while ONE caller
/// refreshes behind it; and only the very first probe (nothing known yet)
/// makes callers share one wait.
#[derive(Default)]
struct ProbeCell {
    value: Mutex<Option<(ProviderAvailability, Instant)>>,
    gate: Mutex<()>,
}

const AVAILABILITY_TTL: Duration = Duration::from_secs(20);

impl ProbeCell {
    fn fresh(&self) -> Option<ProviderAvailability> {
        let held = self.value.lock().unwrap_or_else(|e| e.into_inner());
        match &*held {
            Some((av, at)) if at.elapsed() < AVAILABILITY_TTL => Some(av.clone()),
            _ => None,
        }
    }

    fn last(&self) -> Option<ProviderAvailability> {
        let held = self.value.lock().unwrap_or_else(|e| e.into_inner());
        held.as_ref().map(|(av, _)| av.clone())
    }

    fn store(&self, av: ProviderAvailability) {
        let mut held = self.value.lock().unwrap_or_else(|e| e.into_inner());
        *held = Some((av, Instant::now()));
    }

    /// Run `probe` under the single-flight gate, or answer from what is
    /// already known rather than wait.
    fn refresh(&self, probe: impl FnOnce() -> ProviderAvailability) -> ProviderAvailability {
        let guard = match self.gate.try_lock() {
            Ok(g) => Some(g),
            Err(std::sync::TryLockError::Poisoned(g)) => Some(g.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        };
        match guard {
            Some(_held) => {
                // Someone may have refreshed while we took the gate.
                if let Some(av) = self.fresh() {
                    return av;
                }
                let av = probe();
                self.store(av.clone());
                av
            }
            None => {
                if let Some(av) = self.last() {
                    // Stale is fine and instant; the in-flight probe will
                    // publish the new row for whoever asks next.
                    return av;
                }
                // Nothing is known at all: share the one probe in flight.
                let _held = self.gate.lock().unwrap_or_else(|e| e.into_inner());
                self.last().unwrap_or(ProviderAvailability::Unavailable {
                    reason: "provider probe failed".to_string(),
                })
            }
        }
    }
}

struct EnvFactory {
    fleet_bases: Vec<String>,
    fleet: String,
    /// One node/model pick shared by every fleet provider this factory
    /// opens: N sessions probe the box once, not N times.
    picks: Arc<FleetPickCache>,
    cells: Mutex<Vec<(ProviderKind, Arc<ProbeCell>)>>,
}

impl EnvFactory {
    fn new(fleet_bases: Vec<String>, fleet: String) -> EnvFactory {
        EnvFactory {
            fleet_bases,
            fleet,
            picks: Arc::new(FleetPickCache::new()),
            cells: Mutex::new(Vec::new()),
        }
    }

    fn cell(&self, kind: ProviderKind) -> Arc<ProbeCell> {
        let mut cells = self.cells.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, cell)) = cells.iter().find(|(k, _)| *k == kind) {
            return cell.clone();
        }
        let cell = Arc::new(ProbeCell::default());
        cells.push((kind, cell.clone()));
        cell
    }

    fn bases(&self) -> Vec<String> {
        makepad_asset_chat::fleet_discovery::set_wanted_fleet(&self.fleet);
        makepad_asset_chat::fleet_discovery::seed_bases(self.fleet_bases.clone());
        makepad_asset_chat::fleet_discovery::live_bases()
    }

    /// The unwrapped (synchronous) provider — status probes only.
    fn open_raw(&self, kind: ProviderKind) -> Box<dyn ChatProvider> {
        match kind {
            ProviderKind::FleetQwen => Box::new(FleetQwenChatProvider::with_pick_cache(
                HttpFleetTransport,
                self.bases(),
                self.picks.clone(),
            )),
            ProviderKind::OpenAi => Box::new(makepad_asset_chat::openai::from_env()),
            ProviderKind::Grok => Box::new(makepad_asset_chat::grok::from_env()),
        }
    }
}

impl ProviderFactory for EnvFactory {
    fn probe(&self, kind: ProviderKind) -> ProviderAvailability {
        let cell = self.cell(kind);
        if let Some(av) = cell.fresh() {
            return av;
        }
        cell.refresh(|| strip_location(self.open_raw(kind).availability()))
    }

    fn open(&self, kind: ProviderKind) -> Result<Box<dyn ChatProvider>, String> {
        // Sessions get the provider behind ThreadedProvider so the session
        // worker NEVER blocks on provider HTTP: the fleet begin_turn's ~18 s
        // flaky-LAN backoff would otherwise hold the turn's own thread
        // through every poll's connect timeout. The seed availability comes
        // from the shared probe cell instead of a second fleet scan.
        let seed = self.probe(kind);
        match kind {
            ProviderKind::FleetQwen => {
                let inner = FleetQwenChatProvider::with_pick_cache(
                    HttpFleetTransport,
                    self.bases(),
                    self.picks.clone(),
                );
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
    /// Fixture stand-in for a serving tier's parallel capacity: at most this
    /// many scripted turns run at once across ALL sessions (0 = unlimited).
    gate: Arc<TurnGate>,
}

impl ScriptedFactory {
    fn new(script: ChatScript) -> ScriptedFactory {
        ScriptedFactory {
            fleet_qwen: script.fleet_qwen,
            openai: script.openai,
            grok: script.grok,
            gate: Arc::new(TurnGate::new(script.max_concurrent_turns)),
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
    fn probe(&self, kind: ProviderKind) -> ProviderAvailability {
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

    fn open(&self, kind: ProviderKind) -> Result<Box<dyn ChatProvider>, String> {
        match self.probe(kind) {
            ProviderAvailability::Unavailable { reason } => Err(reason),
            ProviderAvailability::Available { model, .. } => {
                let lane = self.lane(kind);
                Ok(Box::new(ScriptedProvider {
                    kind,
                    model,
                    turns: lane.turns.clone(),
                    delay: Duration::from_millis(lane.turn_delay_ms),
                    gate: self.gate.clone(),
                    pending: Vec::new(),
                }))
            }
        }
    }
}

/// Bounded parallelism for the scripted lane. Real serving tiers admit a
/// finite number of generates at once; a fixture that ignores that cannot
/// show fairness. Zero means unbounded.
///
/// Admission is FIFO by ticket, not "whoever the condvar wakes": a fairness
/// assertion that depends on wakeup luck is a flaky test, and a fixture that
/// can starve a session cannot prove the broker does not.
struct TurnGate {
    capacity: usize,
    state: Mutex<GateState>,
    room: std::sync::Condvar,
}

#[derive(Default)]
struct GateState {
    next_ticket: u64,
    queue: std::collections::VecDeque<u64>,
}

impl TurnGate {
    fn new(capacity: usize) -> TurnGate {
        TurnGate {
            capacity,
            state: Mutex::new(GateState::default()),
            room: std::sync::Condvar::new(),
        }
    }

    fn enter(&self) -> u64 {
        if self.capacity == 0 {
            return 0;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let ticket = state.next_ticket;
        state.next_ticket += 1;
        state.queue.push_back(ticket);
        loop {
            let position = state.queue.iter().position(|t| *t == ticket).unwrap_or(0);
            if position < self.capacity {
                return ticket;
            }
            state = self.room.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn leave(&self, ticket: u64) {
        if self.capacity == 0 {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.queue.retain(|t| *t != ticket);
        drop(state);
        self.room.notify_all();
    }
}

struct ScriptedProvider {
    kind: ProviderKind,
    model: String,
    turns: Vec<ScriptedTurn>,
    /// Wall-clock a scripted `begin_turn` costs, so a suite can watch N
    /// sessions overlap instead of measuring an instant fixture.
    delay: Duration,
    gate: Arc<TurnGate>,
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
        // Deliberately BLOCKING, like a synchronous provider: this is the
        // shape that used to freeze the whole broker.
        if !self.delay.is_zero() || self.gate.capacity > 0 {
            let ticket = self.gate.enter();
            if !self.delay.is_zero() {
                std::thread::sleep(self.delay);
            }
            self.gate.leave(ticket);
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

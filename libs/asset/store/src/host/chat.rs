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
use makepad_asset_chat::dispatch::AssetServerTools;
use makepad_asset_chat::provider::{ChatProvider, ProviderEvent, TurnInput};
use makepad_asset_chat::qwen::{FleetQwenChatProvider, HttpFleetTransport};
use makepad_asset_chat::session::{
    CancelFlag, ExecCtx, SendRefusal, Session, SessionId, ToolExecutor,
};
use makepad_asset_chat::tools::{ConsultTask, ContentToolCall};
use makepad_asset_chat::wire::{
    sanitize_public_error, AttachmentBinding, ChatEvent, ChatMessage, ChatRole,
    ProviderAvailability, ProviderKind, ToolOutcome, MAX_MESSAGE_BYTES,
};
use makepad_asset_client::ApiEndpoints;
use crate::PrincipalId;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const PUMP_SLICE: Duration = Duration::from_millis(50);
const CONSULT_POLL: Duration = Duration::from_millis(20);
const CONSULT_TIMEOUT: Duration = Duration::from_secs(60);
const ACTOR_IDLE: Duration = Duration::from_millis(20);

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
}

enum ChatCmd {
    ListProviders { reply: Sender<Vec<ProviderStatus>> },
    Create(CreateReq),
    Get { owner: PrincipalId, id: SessionId, reply: Sender<Result<SessionView, ChatFail>> },
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
    Shutdown,
}

struct CreateReq {
    owner: PrincipalId,
    namespace: String,
    token: String,
    provider: ProviderKind,
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
    log_enabled: bool,
    stop: Arc<AtomicBool>,
) -> Result<(ChatHandle, JoinHandle<()>), crate::ServerError> {
    let (tx, rx) = mpsc::channel();
    let factory: Box<dyn ProviderFactory> = match cfg.script.clone() {
        Some(script) => Box::new(ScriptedFactory::new(script)),
        None => Box::new(EnvFactory {
            fleet_bases: cfg.fleet_bases.clone(),
            fleet: cfg.fleet.clone(),
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
    ) -> Result<SessionView, ChatFail> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(ChatCmd::Create(CreateReq {
                owner,
                namespace,
                token,
                provider,
                reply: tx,
            }))
            .map_err(|_| ChatFail::Down)?;
        rx.recv_timeout(Duration::from_secs(15)).map_err(|_| ChatFail::Down)?
    }

    pub fn get(&self, owner: PrincipalId, id: SessionId) -> Result<SessionView, ChatFail> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(ChatCmd::Get { owner, id, reply: tx }).map_err(|_| ChatFail::Down)?;
        rx.recv_timeout(Duration::from_secs(5)).map_err(|_| ChatFail::Down)?
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
}

impl Actor {
    fn handle(&mut self, cmd: ChatCmd) {
        match cmd {
            ChatCmd::ListProviders { reply } => {
                let _ = reply.send(self.list_providers());
            }
            ChatCmd::Create(req) => {
                let _ = req.reply.send(self.create(req.owner, req.namespace, req.token, req.provider));
            }
            ChatCmd::Get { owner, id, reply } => {
                let _ = reply.send(self.view_of(&owner, &id));
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
            };
            live.session.send(&text, &attachments, &mut tools)
        };
        match result {
            Ok(turn) => {
                drain_into(live, cap);
                Ok(turn)
            }
            Err(e) => Err(map_send(e)),
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
            return;
        }
        let primary = live.session.provider_kind();
        {
            let mut tools = SessionTools {
                inner: &mut live.tools,
                factory: &mut *self.factory,
                endpoints,
                primary,
                consult_depth: &mut live.consult_depth,
            };
            let start = Instant::now();
            while !live.session.is_idle() && start.elapsed() < PUMP_SLICE {
                live.session.pump(&mut tools);
            }
        }
        drain_into(live, cap);
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
    for ev in live.session.drain_events() {
        live.events.push(ev);
    }
    if live.events.len() > cap {
        let drop_n = live.events.len() - cap;
        live.events.drain(..drop_n);
    }
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
}

impl ToolExecutor for SessionTools<'_> {
    fn capability_doc(&mut self) -> String {
        let mut out = match self.inner {
            Some(tools) => tools.capability_doc(),
            None => "Asset tools are not connected.\n".to_string(),
        };
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
        let _ = self.endpoints;
        out
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
            other => match self.inner {
                Some(tools) => tools.execute(other, ctx, progress, cancel),
                None => ToolOutcome::Failed { message: "asset tools are not connected".into() },
            },
        }
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
}

impl EnvFactory {
    fn bases(&self) -> Vec<String> {
        makepad_asset_chat::fleet_discovery::set_wanted_fleet(&self.fleet);
        makepad_asset_chat::fleet_discovery::seed_bases(self.fleet_bases.clone());
        makepad_asset_chat::fleet_discovery::live_bases()
    }
}

impl ProviderFactory for EnvFactory {
    fn probe(&mut self, kind: ProviderKind) -> ProviderAvailability {
        match self.open(kind) {
            Ok(mut p) => strip_location(p.availability()),
            Err(reason) => ProviderAvailability::Unavailable { reason },
        }
    }

    fn open(&mut self, kind: ProviderKind) -> Result<Box<dyn ChatProvider>, String> {
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

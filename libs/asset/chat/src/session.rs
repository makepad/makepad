//! The session engine: one conversation, one explicit provider, a local
//! [`Origin`] handle, a bounded event stream, and the turn/tool state machine.
//!
//! State machine per session:
//!
//! ```text
//! Idle --send--> Streaming
//!   FleetQwen: Done(text with <<tool>> line) -->
//!     ToolCall -> execute (ToolProgress*) -> ToolResult
//!     --textual follow-up turn--> Streaming            (bounded rounds)
//!   OpenAI/Grok: FunctionCall -->
//!     ToolCall -> execute (ToolProgress*) -> ToolResult
//!     --native continue_function--> Streaming          (bounded rounds)
//! Streaming --provider Done(plain text)--> Done event --> Idle
//! any active --cancel--> Cancelled event --> Idle
//! ```
//!
//! Send refuses — typed, never silently rerouted — when the bound provider
//! is unavailable, the session is mid-turn, or a bound is exceeded. Tool
//! execution is synchronous inside [`Session::pump`]: this crate is the
//! headless dependency slice, and its callers (broker loop, tests, later a
//! service thread) own their own threading.

use crate::provider::{ChatProvider, ProviderEvent, TurnInput};
use crate::toolcall::{self, Extract};
use crate::tools::{self, ContentToolCall};
use crate::wire::{
    sanitize_public_error, split_delta_text, AttachmentBinding, ChatEvent, ChatEventBody,
    ChatMessage, ChatRole, ProviderAvailability, ProviderKind, ToolOutcome, MAX_ATTACHMENTS,
    MAX_MESSAGES, MAX_MESSAGE_BYTES, MAX_PROGRESS_EVENTS, MAX_TOOL_CALL_ID, MAX_TOOL_JSON_BYTES,
    MAX_TOOL_ROUNDS, MAX_TURN_TEXT_BYTES,
};
use makepad_asset_client::json::Value;
use makepad_asset_data::AssetRevisionId;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// `chat_<16 hex>` — unique per process run, opaque on the wire.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(String);

impl SessionId {
    pub fn generate() -> SessionId {
        use std::sync::atomic::AtomicU64;
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        // Uniqueness, not secrecy: session authz is the transport layer's
        // bearer token, never knowledge of this id.
        let mix = t ^ (std::process::id() as u64).rotate_left(32) ^ n.rotate_left(17);
        SessionId(format!("chat_{mix:016x}"))
    }

    pub fn parse(s: &str) -> Option<SessionId> {
        let hex = s.strip_prefix("chat_")?;
        if hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            Some(SessionId(s.to_string()))
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Local handle for one chat session. `principal` is caller-supplied and
/// retained here only; this foundation does not transmit it to the Asset
/// Server (`OperationCreateRequest` has no audit/session field).
/// [`SessionId`] scopes in-process operation ownership and idempotency
/// hashing. Authorization is the dispatcher's bearer token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub principal: String,
    pub session: SessionId,
}

/// Cooperative cancellation shared between the session and a running tool.
#[derive(Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
    fn reset(&self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

/// What a tool execution may read: the local [`Origin`] (session id only
/// is used by dispatch) and which exact revisions this session has
/// legitimately seen (bound attachments plus revisions returned by earlier
/// tool results). Transform inputs must come from this set — the model
/// cannot conjure ids into the dispatcher.
pub struct ExecCtx<'a> {
    pub origin: &'a Origin,
    pub known: &'a HashSet<AssetRevisionId>,
}

/// Server-side tool execution. The one real implementation is
/// [`crate::dispatch::AssetServerTools`]; tests provide deterministic ones.
pub trait ToolExecutor {
    /// Live, honest capability text rendered into the system prompt
    /// (advertised profiles, or why none are available).
    fn capability_doc(&mut self) -> String;

    /// The tool vocabulary this executor supports, rendered into the system
    /// prompt at every send. The default is the broker's content base list;
    /// a game-client executor appends [`tools::sandbox_definitions`]. The
    /// typed parser accepts the FULL vocabulary regardless — a call outside
    /// this executor's list is answered with a typed `Unavailable`, never
    /// executed by accident.
    fn tool_definitions(&mut self) -> Vec<crate::tools::ToolDef> {
        tools::definitions()
    }

    /// True when `call` must be executed by the session's CLIENT — the app
    /// that owns the state the tool touches (the sandbox owns the game
    /// world). The session then emits the ToolCall event as usual but PARKS
    /// the turn instead of calling [`ToolExecutor::execute`]; the owner
    /// feeds the client's answer back through
    /// [`Session::provide_client_outcome`] (or times the wait out with a
    /// `Failed` outcome). Default: nothing is client-executed.
    fn client_executes(&mut self, call: &ContentToolCall) -> bool {
        let _ = call;
        false
    }

    /// Execute one typed call. `progress` may be called with
    /// `(permille, note)` any number of times; `cancel` should be observed
    /// by long-running tools.
    fn execute(
        &mut self,
        call: &ContentToolCall,
        ctx: &ExecCtx,
        progress: &mut dyn FnMut(u16, &str),
        cancel: &CancelFlag,
    ) -> ToolOutcome;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendRefusal {
    /// The bound provider is honestly unavailable. There is no fallback;
    /// the caller surfaces `reason` and the user may switch providers
    /// explicitly (a NEW session).
    ProviderUnavailable { reason: String },
    /// A turn is already in flight.
    Busy,
    TooLarge { what: &'static str },
    TooMany { what: &'static str },
    /// The bounded history is full; start a new session.
    HistoryFull,
    InvalidAttachment { what: String },
    /// The provider refused to start the turn.
    ProviderError { message: String },
    /// The session ended after an ambiguous post-tool continuation.
    /// Start a new session; this one will not retry a mutation.
    Sealed { reason: String },
}

enum Phase {
    Idle,
    /// A provider turn is streaming; `collected` accumulates deltas as a
    /// fallback for providers that do not repeat full text in `Done`.
    Streaming { collected: String },
    /// A client-executed tool call was emitted; the turn is parked until
    /// the owner provides the client's outcome (or a timeout outcome).
    AwaitingClientTool { call_id: String },
}

/// One conversation bound to one provider and one local [`Origin`].
pub struct Session {
    id: SessionId,
    origin: Origin,
    provider: Box<dyn ChatProvider>,
    history: Vec<ChatMessage>,
    known: HashSet<AssetRevisionId>,
    phase: Phase,
    system: String,
    events: VecDeque<ChatEvent>,
    seq: u64,
    turn: u64,
    tool_rounds: u32,
    cancel: CancelFlag,
    sealed: Option<String>,
    tool_executed_this_turn: bool,
    executed_mutation: bool,
    /// Visible assistant text preceding a PARKED client tool call, kept for
    /// the tool round once the client's outcome arrives.
    pending_clean: String,
    /// The tool-round budget was reached: the model got one FINAL
    /// completion round ("answer with what you have") and any further tool
    /// line it emits is not executed — the turn ends with its text.
    budget_final: bool,
    /// The current round's call rendered in the model's trained template,
    /// recorded into the assistant history entry by `tool_round` (textual
    /// lane only).
    last_call_trained: Option<String>,
    /// Serving facts the provider reported, waiting for the next delta to
    /// ride out on. Presentation only — dropping it changes nothing but a
    /// readout.
    pending_serving: Option<crate::wire::ServingFacts>,
}

impl Session {
    /// Bind a session to its provider. The binding is permanent: switching
    /// providers is explicitly a new session, so a transcript can never
    /// contain silently mixed provenance. `principal` is kept on
    /// [`Origin`] for the caller; it is not stamped onto Asset Server ops.
    pub fn new(principal: impl Into<String>, provider: Box<dyn ChatProvider>) -> Session {
        let id = SessionId::generate();
        Session {
            origin: Origin { principal: principal.into(), session: id.clone() },
            id,
            provider,
            history: Vec::new(),
            known: HashSet::new(),
            phase: Phase::Idle,
            system: String::new(),
            events: VecDeque::new(),
            seq: 0,
            turn: 0,
            tool_rounds: 0,
            cancel: CancelFlag::default(),
            sealed: None,
            tool_executed_this_turn: false,
            executed_mutation: false,
            pending_clean: String::new(),
            budget_final: false,
            last_call_trained: None,
            pending_serving: None,
        }
    }

    pub fn id(&self) -> &SessionId {
        &self.id
    }

    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    pub fn provider_kind(&self) -> ProviderKind {
        self.provider.kind()
    }

    pub fn availability(&mut self) -> ProviderAvailability {
        self.provider.availability()
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.phase, Phase::Idle)
    }

    /// The cooperative cancel flag this session hands to running tools.
    ///
    /// An owner that drives the session from a worker thread may raise this
    /// from ANOTHER thread to interrupt a long tool (`operation.wait` runs
    /// up to two minutes) immediately, instead of waiting for its `cancel`
    /// command to reach the worker's queue. `send` resets the flag at the
    /// start of every turn, so a raise on an idle session is harmless.
    pub fn cancel_flag(&self) -> CancelFlag {
        self.cancel.clone()
    }

    pub fn is_sealed(&self) -> bool {
        self.sealed.is_some()
    }

    pub fn turn(&self) -> u64 {
        self.turn
    }

    /// Revisions this session may legitimately name as transform inputs.
    pub fn known_revisions(&self) -> &HashSet<AssetRevisionId> {
        &self.known
    }

    pub fn drain_events(&mut self) -> Vec<ChatEvent> {
        self.events.drain(..).collect()
    }

    /// Start a user turn. Attachment revisions join the known-revision set
    /// only after the provider turn starts; a refused send leaves `known`
    /// unchanged so a failed attach cannot become a transform input.
    pub fn send(
        &mut self,
        text: &str,
        attachments: &[AttachmentBinding],
        tools_exec: &mut dyn ToolExecutor,
    ) -> Result<u64, SendRefusal> {
        if let Some(reason) = &self.sealed {
            return Err(SendRefusal::Sealed { reason: reason.clone() });
        }
        if !matches!(self.phase, Phase::Idle) {
            return Err(SendRefusal::Busy);
        }
        if text.is_empty() || text.len() > MAX_MESSAGE_BYTES {
            return Err(SendRefusal::TooLarge { what: "message" });
        }
        if attachments.len() > MAX_ATTACHMENTS {
            return Err(SendRefusal::TooMany { what: "attachments" });
        }
        for a in attachments {
            a.validate()
                .map_err(|e| SendRefusal::InvalidAttachment { what: e.to_string() })?;
        }
        if self.history.len() >= MAX_MESSAGES || self.history.len() + 2 > MAX_MESSAGES {
            return Err(SendRefusal::HistoryFull);
        }
        // Honest gate, no fallback: an unavailable provider refuses the
        // send with its live reason. Nothing here constructs another
        // provider — that is a user decision made elsewhere, explicitly.
        if let ProviderAvailability::Unavailable { reason } = self.provider.availability() {
            return Err(SendRefusal::ProviderUnavailable { reason });
        }

        let mut user_text = String::new();
        let att = toolcall::render_attachments(attachments);
        if !att.is_empty() {
            user_text.push_str(&att);
            user_text.push('\n');
        }
        user_text.push_str(text);
        if user_text.len() > MAX_MESSAGE_BYTES {
            return Err(SendRefusal::TooLarge { what: "message" });
        }
        self.push_history(ChatRole::User, user_text)
            .map_err(|_| SendRefusal::TooLarge { what: "message" })?;

        let defs = tools_exec.tool_definitions();
        let cap = tools_exec.capability_doc();
        self.system = if self.provider.kind().uses_native_tools() {
            tools::render_native_system(&defs, &cap)
        } else {
            toolcall::render_system(&defs, &cap)
        };
        self.turn += 1;
        self.tool_rounds = 0;
        self.tool_executed_this_turn = false;
        self.executed_mutation = false;
        self.budget_final = false;
        self.cancel.reset();
        if let Err(message) = self.begin_provider_turn() {
            self.history.pop();
            self.turn = self.turn.saturating_sub(1);
            return Err(SendRefusal::ProviderError { message: sanitize_public_error(&message) });
        }
        for a in attachments {
            self.known.insert(a.revision);
        }
        Ok(self.turn)
    }

    /// Abort the in-flight turn and any running tool. Idle cancel is a
    /// no-op and retains the provider conversation chain.
    pub fn cancel(&mut self) {
        if matches!(self.phase, Phase::Idle) {
            return;
        }
        self.cancel.cancel();
        self.provider.cancel();
        if self.should_seal_after_tool() {
            self.seal("cancelled after a tool executed");
        }
        self.pending_clean.clear();
        self.last_call_trained = None;
        self.phase = Phase::Idle;
        self.emit(ChatEventBody::Cancelled);
    }

    /// Drive the turn forward: drain provider events, run at most one tool
    /// round. Call from the owner's loop until `is_idle`.
    pub fn pump(&mut self, tools_exec: &mut dyn ToolExecutor) {
        if matches!(self.phase, Phase::Idle) {
            return;
        }
        // A parked turn waits for provide_client_outcome (or the owner's
        // timeout); the provider has nothing meaningful to say meanwhile.
        if matches!(self.phase, Phase::AwaitingClientTool { .. }) {
            return;
        }
        for ev in self.provider.poll() {
            match ev {
                ProviderEvent::Status { note, permille } => {
                    self.emit(ChatEventBody::ToolProgress {
                        id: "qwen".to_string(),
                        permille,
                        note,
                    });
                }
                ProviderEvent::Serving(facts) => {
                    // Held for the delta it describes: the wire carries
                    // these facts ON a delta, so a round that produces no
                    // more text simply never reports them.
                    self.pending_serving = Some(facts);
                }
                ProviderEvent::Delta(text) => {
                    if let Phase::Streaming { collected } = &mut self.phase {
                        collected.push_str(&text);
                        if collected.len() > MAX_TURN_TEXT_BYTES {
                            self.provider.cancel();
                            self.phase = Phase::Idle;
                            self.emit(ChatEventBody::Error {
                                code: "turn_too_large".to_string(),
                                message: "assistant turn exceeded the text budget".to_string(),
                            });
                            return;
                        }
                    }
                    // The facts describe the END of this text, so only the
                    // last chunk of a split carries them.
                    let chunks = split_delta_text(&text);
                    let last = chunks.len().saturating_sub(1);
                    for (i, chunk) in chunks.into_iter().enumerate() {
                        let serving =
                            if i == last { self.pending_serving.take() } else { None };
                        self.emit(ChatEventBody::Delta { text: chunk, serving });
                    }
                }
                ProviderEvent::FunctionCall { call_id, name, arguments } => {
                    let visible = match &self.phase {
                        Phase::Streaming { collected } => collected.clone(),
                        _ => String::new(),
                    };
                    self.finish_native_function(visible, call_id, name, arguments, tools_exec);
                    return;
                }
                ProviderEvent::Error(message) => {
                    self.phase = Phase::Idle;
                    if self.should_seal_after_tool() {
                        self.seal("provider error after a tool executed");
                    }
                    self.emit(ChatEventBody::Error {
                        code: "provider".to_string(),
                        message: sanitize_public_error(&message),
                    });
                    return;
                }
                ProviderEvent::Done { text } => {
                    let full = if text.is_empty() {
                        match &self.phase {
                            Phase::Streaming { collected } => collected.clone(),
                            _ => String::new(),
                        }
                    } else {
                        text
                    };
                    self.finish_provider_turn(full, tools_exec);
                    return;
                }
            }
        }
    }

    // ------------------------------------------------------------ internals

    fn begin_provider_turn(&mut self) -> Result<(), String> {
        if self.history.len() > MAX_MESSAGES {
            return Err("too many messages".to_string());
        }
        for m in &self.history {
            m.validate().map_err(|_| "message empty or too large".to_string())?;
        }
        let input = TurnInput::new(self.system.clone(), self.history.clone());
        self.provider.begin_turn(&input)?;
        self.phase = Phase::Streaming { collected: String::new() };
        Ok(())
    }

    fn finish_provider_turn(&mut self, full: String, tools_exec: &mut dyn ToolExecutor) {
        // Textual <<tool>> markers are FleetQwen-only. Native providers
        // resume through FunctionCall / continue_function.
        if self.provider.kind() != ProviderKind::FleetQwen {
            if let Err(body) = self.finish_assistant_text(full) {
                self.phase = Phase::Idle;
                self.emit(body);
                return;
            }
            self.phase = Phase::Idle;
            self.emit(ChatEventBody::Done);
            return;
        }
        // The post-budget completion round: whatever the model says IS the
        // answer; a tool line in it is cut off, not executed.
        if self.budget_final {
            let visible = match toolcall::extract(&full) {
                Extract::None => toolcall::split_thinking(&full).visible,
                Extract::Call { clean, .. } | Extract::Malformed { clean, .. } => clean,
            };
            if let Err(body) = self.finish_assistant_text(visible) {
                self.phase = Phase::Idle;
                self.emit(body);
                return;
            }
            self.phase = Phase::Idle;
            self.emit(ChatEventBody::Done);
            return;
        }
        match toolcall::extract(&full) {
            Extract::None => {
                let visible = toolcall::split_thinking(&full).visible;
                if let Err(body) = self.finish_assistant_text(visible) {
                    self.phase = Phase::Idle;
                    self.emit(body);
                    return;
                }
                self.phase = Phase::Idle;
                self.emit(ChatEventBody::Done);
            }
            Extract::Malformed { clean, reason } => {
                self.tool_round(clean, None, ToolOutcome::Refused { what: reason }, tools_exec);
            }
            Extract::Call { clean, name, args } => {
                let call_id = format!("tc_{}_{}", self.turn, self.tool_rounds + 1);
                let emit_args = if matches!(args, Value::Obj(_))
                    && args.to_json().len() <= MAX_TOOL_JSON_BYTES
                {
                    args.clone()
                } else {
                    Value::Obj(Vec::new())
                };
                self.emit(ChatEventBody::ToolCall {
                    id: call_id.clone(),
                    name: name.clone(),
                    args: emit_args,
                });
                self.last_call_trained = Some(render_trained_call(&name, &args));
                let outcome = match ContentToolCall::parse(&name, &args) {
                    Err(reason) => ToolOutcome::Refused { what: reason },
                    Ok(call) => {
                        if tools_exec.client_executes(&call) {
                            self.park_for_client(clean, call_id, &call);
                            return;
                        }
                        self.run_tool(&call_id, &call, tools_exec)
                    }
                };
                self.tool_round(clean, Some(call_id), outcome, tools_exec);
            }
        }
    }

    /// Emit nothing further and wait for the session owner to feed the
    /// CLIENT's outcome back. The ToolCall event was already emitted, so
    /// the client sees exactly what to execute.
    fn park_for_client(&mut self, clean: String, call_id: String, call: &ContentToolCall) {
        self.tool_executed_this_turn = true;
        if is_mutating(call) {
            self.executed_mutation = true;
        }
        self.pending_clean = clean;
        self.phase = Phase::AwaitingClientTool { call_id };
    }

    /// The call id of the client-executed tool this session is parked on.
    pub fn awaiting_client_tool(&self) -> Option<&str> {
        match &self.phase {
            Phase::AwaitingClientTool { call_id } => Some(call_id),
            _ => None,
        }
    }

    /// Feed a parked client tool its outcome (from the wire, or a timeout
    /// `Failed` from the owner) and resume the turn.
    pub fn provide_client_outcome(
        &mut self,
        call_id: &str,
        outcome: ToolOutcome,
        tools_exec: &mut dyn ToolExecutor,
    ) -> Result<(), &'static str> {
        match &self.phase {
            Phase::AwaitingClientTool { call_id: waiting } if waiting == call_id => {}
            Phase::AwaitingClientTool { .. } => return Err("tool call id mismatch"),
            _ => return Err("no client tool is awaiting a result"),
        }
        if let ToolOutcome::Ok { value } = &outcome {
            harvest_revisions(value, &mut self.known);
        }
        let clean = std::mem::take(&mut self.pending_clean);
        self.tool_round(clean, Some(call_id.to_string()), outcome, tools_exec);
        Ok(())
    }

    fn finish_native_function(
        &mut self,
        visible: String,
        call_id: String,
        api_name: String,
        arguments: String,
        tools_exec: &mut dyn ToolExecutor,
    ) {
        if call_id.is_empty() || call_id.len() > MAX_TOOL_CALL_ID {
            self.phase = Phase::Idle;
            self.emit(ChatEventBody::Error {
                code: "provider".to_string(),
                message: "function call missing call id".to_string(),
            });
            return;
        }
        if arguments.len() > MAX_TOOL_JSON_BYTES {
            self.emit(ChatEventBody::ToolCall {
                id: call_id.clone(),
                name: bounded(&api_name, 32).to_string(),
                args: Value::Obj(Vec::new()),
            });
            self.tool_round(
                visible,
                Some(call_id),
                ToolOutcome::Refused { what: "function arguments too large".to_string() },
                tools_exec,
            );
            return;
        }
        let refused = |what: String| ToolOutcome::Refused { what };
        let (canonical, args, outcome) = match tools::canonical_from_api_name(&api_name) {
            None => {
                let name = bounded(&api_name, 32).to_string();
                (
                    name,
                    Value::Obj(Vec::new()),
                    Some(refused(format!(
                        "unknown tool '{}'",
                        bounded(&api_name, 32)
                    ))),
                )
            }
            Some(canonical) => match makepad_asset_client::json::parse(arguments.as_bytes()) {
                Ok(v @ Value::Obj(_)) => (canonical.to_string(), v, None),
                Ok(_) => (
                    canonical.to_string(),
                    Value::Obj(Vec::new()),
                    Some(refused("function arguments must be an object".to_string())),
                ),
                Err(_) => (
                    canonical.to_string(),
                    Value::Obj(Vec::new()),
                    Some(refused("function arguments are not valid JSON".to_string())),
                ),
            },
        };
        let emit_args = if args.to_json().len() > MAX_TOOL_JSON_BYTES {
            Value::Obj(Vec::new())
        } else {
            args.clone()
        };
        self.emit(ChatEventBody::ToolCall {
            id: call_id.clone(),
            name: canonical.clone(),
            args: emit_args,
        });
        let outcome = match outcome {
            Some(o) => o,
            None => match ContentToolCall::parse(&canonical, &args) {
                Err(reason) => ToolOutcome::Refused { what: reason },
                Ok(call) => {
                    if tools_exec.client_executes(&call) {
                        self.park_for_client(visible, call_id, &call);
                        return;
                    }
                    self.run_tool(&call_id, &call, tools_exec)
                }
            },
        };
        self.tool_round(visible, Some(call_id), outcome, tools_exec);
    }

    fn run_tool(
        &mut self,
        call_id: &str,
        call: &ContentToolCall,
        tools_exec: &mut dyn ToolExecutor,
    ) -> ToolOutcome {
        let ctx = ExecCtx { origin: &self.origin, known: &self.known };
        let mut progress_events: Vec<(u16, String)> = Vec::new();
        self.tool_executed_this_turn = true;
        if is_mutating(call) {
            self.executed_mutation = true;
        }
        let outcome = {
            let mut progress = |permille: u16, note: &str| {
                if progress_events.len() < MAX_PROGRESS_EVENTS {
                    progress_events.push((permille.min(1000), bounded_note(note)));
                }
            };
            tools_exec.execute(call, &ctx, &mut progress, &self.cancel)
        };
        for (permille, note) in progress_events {
            self.emit(ChatEventBody::ToolProgress { id: call_id.to_string(), permille, note });
        }
        if let ToolOutcome::Ok { value } = &outcome {
            harvest_revisions(value, &mut self.known);
        }
        outcome
    }

    /// Record one tool round, feed the result back, and start the follow-up
    /// provider turn (or end the turn on cancel/budget/overflow).
    fn tool_round(
        &mut self,
        clean_text: String,
        call_id: Option<String>,
        outcome: ToolOutcome,
        _tools_exec: &mut dyn ToolExecutor,
    ) {
        let outcome = bounded_outcome(outcome);
        if let Some(id) = &call_id {
            self.emit(ChatEventBody::ToolResult { id: id.clone(), outcome: outcome.clone() });
        }
        if self.history.len() + 2 > MAX_MESSAGES {
            self.fail_closed_tool_round("history_full", "session history budget exhausted");
            return;
        }
        // The textual lane records the CALL in the assistant turn, in the
        // model's TRAINED spelling. Without it the in-context history shows
        // assistant turns that never call tools (only paired results), and
        // the model imitates that — observed live as turns ending on prose
        // like "Now the car-kit models." mid-build. In-context examples
        // beat instructions; make the history look exactly like what we
        // want more of.
        let assistant = if self.provider.kind() == ProviderKind::FleetQwen {
            match &self.last_call_trained {
                Some(call) if clean_text.len() + call.len() + 1 < MAX_MESSAGE_BYTES => {
                    let mut text = clean_text;
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(call);
                    text
                }
                _ => nonempty(clean_text),
            }
        } else {
            nonempty(clean_text)
        };
        self.last_call_trained = None;
        if self.push_history(ChatRole::Assistant, assistant).is_err() {
            self.fail_closed_tool_round(
                "turn_too_large",
                "assistant turn exceeded the history text budget",
            );
            return;
        }
        let tool_json = outcome.encode().to_json();
        if self.push_history(ChatRole::Tool, tool_json.clone()).is_err() {
            self.fail_closed_tool_round("turn_too_large", "tool result exceeded the history budget");
            return;
        }

        if self.cancel.is_cancelled() {
            if self.should_seal_after_tool() {
                self.seal("cancelled after a tool executed");
            }
            self.phase = Phase::Idle;
            self.emit(ChatEventBody::Cancelled);
            return;
        }
        self.tool_rounds += 1;
        if self.tool_rounds >= MAX_TOOL_ROUNDS && !self.budget_final {
            // Native-tool providers keep the fail-closed shape (their
            // sessions seal after tools anyway). The textual lane degrades
            // GRACEFULLY: a turn that spent its budget exploring still
            // ends in an answer or a build, never a dead session — the
            // model gets ONE final completion round with a nudge, and any
            // further tool line it emits is not executed.
            if self.provider.kind().uses_native_tools() {
                self.fail_closed_tool_round(
                    "tool_budget",
                    &format!("tool round budget ({MAX_TOOL_ROUNDS}) exhausted"),
                );
                return;
            }
            self.budget_final = true;
            let nudge = "{\"note\":\"tool budget reached — no more tool calls this \
                         turn; answer or build with what you already have\"}";
            if self.push_history(ChatRole::Tool, nudge.to_string()).is_err() {
                self.fail_closed_tool_round("history_full", "session history budget exhausted");
                return;
            }
        }
        if self.provider.kind().uses_native_tools() {
            let Some(id) = call_id else {
                self.fail_closed_tool_round("provider", "native tool round missing call id");
                return;
            };
            match self.provider.continue_function(&id, &tool_json) {
                Ok(()) => {
                    self.phase = Phase::Streaming { collected: String::new() };
                }
                Err(message) => {
                    self.seal("unresolved tool continuation");
                    self.phase = Phase::Idle;
                    self.emit(ChatEventBody::Error { code: "provider".to_string(), message });
                }
            }
        } else if let Err(message) = self.begin_provider_turn() {
            self.phase = Phase::Idle;
            self.emit(ChatEventBody::Error { code: "provider".to_string(), message });
        }
    }

    fn finish_assistant_text(&mut self, full: String) -> Result<(), ChatEventBody> {
        let text = nonempty(full);
        if text.len() > MAX_MESSAGE_BYTES {
            return Err(ChatEventBody::Error {
                code: "turn_too_large".to_string(),
                message: "assistant turn exceeded the history text budget".to_string(),
            });
        }
        self.push_history(ChatRole::Assistant, text).map_err(|_| ChatEventBody::Error {
            code: "history_full".to_string(),
            message: "session history budget exhausted".to_string(),
        })
    }

    fn push_history(&mut self, role: ChatRole, text: String) -> Result<(), ()> {
        if self.history.len() >= MAX_MESSAGES {
            return Err(());
        }
        let msg = ChatMessage::new(role, text);
        msg.validate().map_err(|_| ())?;
        self.history.push(msg);
        Ok(())
    }

    fn fail_closed_tool_round(&mut self, code: &str, message: &str) {
        self.provider.cancel();
        if self.should_seal_after_tool() {
            self.seal(message);
        }
        self.phase = Phase::Idle;
        self.emit(ChatEventBody::Error {
            code: code.to_string(),
            message: message.to_string(),
        });
    }

    fn should_seal_after_tool(&self) -> bool {
        self.provider.kind().uses_native_tools()
            && (self.tool_executed_this_turn || self.executed_mutation || self.tool_rounds > 0)
    }

    fn seal(&mut self, reason: &str) {
        if self.sealed.is_none() {
            self.sealed = Some(reason.to_string());
        }
    }

    fn emit(&mut self, body: ChatEventBody) {
        let seq = self.seq;
        self.seq += 1;
        let body = match body {
            ChatEventBody::Error { code, message } => ChatEventBody::Error {
                code,
                message: sanitize_public_error(&message),
            },
            other => other,
        };
        let ev = ChatEvent { seq, body };
        match &ev.body {
            ChatEventBody::ToolCall { args, .. } => {
                if !matches!(args, Value::Obj(_)) || args.to_json().len() > MAX_TOOL_JSON_BYTES {
                    self.events.push_back(ChatEvent {
                        seq,
                        body: ChatEventBody::Error {
                            code: "tool_args".to_string(),
                            message: "tool arguments exceeded the wire budget".to_string(),
                        },
                    });
                    return;
                }
            }
            ChatEventBody::ToolResult { outcome, .. } => {
                if outcome.validate().is_err() {
                    self.events.push_back(ChatEvent {
                        seq,
                        body: ChatEventBody::Error {
                            code: "tool_result".to_string(),
                            message: "tool result exceeded the wire budget".to_string(),
                        },
                    });
                    return;
                }
            }
            _ => {}
        }
        self.events.push_back(ev);
    }
}

/// One call in Qwen's trained tool template, for the assistant history of
/// the textual lane. String parameter values go in raw; everything else is
/// JSON — the symmetric inverse of `toolcall`'s native extractor.
fn render_trained_call(name: &str, args: &Value) -> String {
    let api = tools::api_from_canonical(name).unwrap_or_else(|| name.replace('.', "_"));
    let mut out = String::from("<tool_call>\n<function=");
    out.push_str(&api);
    out.push_str(">\n");
    if let Value::Obj(pairs) = args {
        for (key, value) in pairs {
            out.push_str("<parameter=");
            out.push_str(key);
            out.push_str(">\n");
            match value {
                Value::Str(s) => out.push_str(s),
                other => out.push_str(&other.to_json()),
            }
            out.push_str("\n</parameter>\n");
        }
    }
    out.push_str("</function>\n</tool_call>");
    out
}

/// History entries must be non-empty for the wire bound; a tool-only reply
/// still records a placeholder so the transcript stays coherent.
fn bounded(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn nonempty(s: String) -> String {
    if s.is_empty() {
        "(tool call)".to_string()
    } else {
        s
    }
}

fn is_mutating(call: &ContentToolCall) -> bool {
    matches!(
        call,
        ContentToolCall::ImageGenerate { .. }
            | ContentToolCall::VideoGenerate { .. }
            | ContentToolCall::AudioGenerate { .. }
            | ContentToolCall::SpeechGenerate { .. }
            | ContentToolCall::MusicGenerate { .. }
            | ContentToolCall::MeshGenerate { .. }
            | ContentToolCall::WorldGenerate { .. }
            | ContentToolCall::CharacterGenerate { .. }
            | ContentToolCall::DefaultsSet { .. }
            | ContentToolCall::OperationCreate { .. }
            | ContentToolCall::OperationCancel { .. }
            | ContentToolCall::OperationRetry { .. }
            | ContentToolCall::WorldPlace { .. }
            | ContentToolCall::WorldRemove { .. }
            | ContentToolCall::WorldMove { .. }
            | ContentToolCall::WorldSetSource { .. }
            | ContentToolCall::WorldSetPlayerModel { .. }
            | ContentToolCall::WorldSpawn { .. }
            | ContentToolCall::WorldTune { .. }
    )
}

fn bounded_outcome(outcome: ToolOutcome) -> ToolOutcome {
    if outcome.validate().is_ok() {
        outcome
    } else {
        ToolOutcome::Failed { message: "tool result too large".to_string() }
    }
}

fn bounded_note(note: &str) -> String {
    let mut end = note.len().min(crate::wire::MAX_NOTE_BYTES);
    while !note.is_char_boundary(end) {
        end -= 1;
    }
    note[..end].to_string()
}

/// Collect every `arev_…` spelled anywhere in a tool result value into the
/// session's known set, so returned revisions are chainable as inputs.
fn harvest_revisions(v: &Value, known: &mut HashSet<AssetRevisionId>) {
    match v {
        Value::Str(s) => {
            if let Ok(rev) = AssetRevisionId::from_str(s) {
                known.insert(rev);
            }
        }
        Value::Arr(items) => {
            for i in items {
                harvest_revisions(i, known);
            }
        }
        Value::Obj(pairs) => {
            for (_, val) in pairs {
                harvest_revisions(val, known);
            }
        }
        _ => {}
    }
}

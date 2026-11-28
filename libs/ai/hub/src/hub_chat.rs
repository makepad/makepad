//! The hub chat session: the election wrapped around wherever the model
//! is resident (aicore §3), so an app never chooses a computer.
//!
//! `HubChatSession::start` is what apps consume. Its worker thread runs the
//! election before any weights move, in this order:
//!
//! - a live co-located holder serving this model (a machine node / the hub
//!   service) → route the conversation there over loopback instead of
//!   loading a second copy;
//! - a holder still loading → wait on its published progress (one load, N
//!   clients) up to a bounded patience, then fall through;
//! - **a fleet chat node on the LAN** (a box whose role allows `chat`,
//!   heard by discovery and honestly advertising a chat model) → route the
//!   conversation there. This is where the 27B on the GPU box answers a
//!   laptop that holds no weights at all;
//! - the weights on this machine → claim the election, load in-process, and
//!   publish loading/ready INTO the lock record so later apps wait instead
//!   of stampeding. The guard lives exactly as long as the worker: process
//!   death reopens the election at the OS level;
//! - none of those → say exactly that, and where a model may be put. No
//!   download starts on its own.
//!
//! Tool packs travel on every route. In-process, the Qwen template's
//! `<tools>` block is prefilled and `<tool_call>` tokens are caught as they
//! decode; over a proxy the SAME system text goes to the node as its
//! `chat_system`, the same `<tool_call>` markup is split out of the streamed
//! text, and tool results go back as the `<tool_response>` turns the model
//! was trained on. One rendering, one parser, wherever the model runs.
//!
//! A proxied node that dies mid-conversation ends THAT turn honestly (the
//! node is named); the person's next line re-runs the election — the same
//! node if it is back, another fleet node, or the local weights. Nothing
//! silently loads gigabytes in the middle of a conversation.
//!
//! The surface extends [`crate::local_llm`]'s with image attachments and the
//! same [`ChatEvent`]s, so a consumer sees which route it landed on through
//! its `Loading` phases and [`HubChatSession::route`].

use crate::chat_wire::{ChatMessage, ChatRole, ProviderAvailability};
use crate::client::RequestPolicy;
use crate::discovery::Discovery;
#[cfg(feature = "llm")]
use crate::local_llm::build_prefix;
use crate::local_llm::{read_calls, system_text, ChatEvent, HiddenWork, LocalLlmConfig, MarkupSplitter, TaggedChatEvent, ToolSpec, WakeHook};
#[cfg(feature = "llm")]
use crate::machine::Claim;
use crate::machine::{self, ResidencyState};
use crate::providers::provider::{validate_tool_images, ChatProvider, ProviderEvent, ToolImage, TurnInput};
use crate::providers::qwen::{chat_offers_for, FleetQwenChatProvider, FleetTranscriptMark, FleetTransport};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a session waits on another process's published `Loading` before
/// giving up and loading its own copy. Weights stream for tens of seconds;
/// two minutes covers a cold 9GB load without stranding anyone forever.
const HOLDER_PATIENCE: Duration = Duration::from_secs(120);
/// Poll cadence against the lock record / the proxy provider.
const POLL: Duration = Duration::from_millis(150);
/// How long a session listens for fleet beacons before deciding the LAN
/// has no chat node. Nodes beat every two seconds; a listener that just
/// bound needs one full interval, and a second one covers a lost packet.
const FLEET_PATIENCE: Duration = Duration::from_millis(4500);
const FLEET_POLL: Duration = Duration::from_millis(250);
/// Set once the app shuts down: a fleet probe stops listening at its next poll.
static FLEET_PROBES_STOPPED: AtomicBool = AtomicBool::new(false);

/// Stops every running and future [`probe_fleet_inventory`] (the app is shutting down: its
/// threads must end so a relaunch does not wait for them).
pub fn stop_fleet_probes() {
    FLEET_PROBES_STOPPED.store(true, Ordering::Relaxed);
}

/// Where the conversation actually runs, once the election settled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatRoute {
    /// A co-located node on this machine serves the model over loopback.
    MachineNode { port: u16 },
    /// A fleet chat box on the LAN.
    Fleet { base: String, model: String },
    /// The weights were loaded into this process.
    InProcess { model: String },
}

impl ChatRoute {
    /// One phrase for a status chip: `qwen3.8-27b on 192.0.2.76`.
    pub fn label(&self) -> String {
        match self {
            ChatRoute::MachineNode { port } => format!("this machine's node :{port}"),
            ChatRoute::Fleet { base, model } => format!("{model} on {}", host_of(base)),
            ChatRoute::InProcess { .. } => "in-process".to_string(),
        }
    }

    /// The terminal failure of a node request. Keep the node's diagnostic in the log.
    pub fn failure(&self, error: &str, log: impl FnOnce(&str)) -> ChatEvent {
        if crate::talk::overflow_counts(error).is_some() {
            log(error);
            ChatEvent::Failed("This line doesn't fit in the room the model has left in this chat. Try it with fewer pictures, or start a new chat.".into())
        } else {
            ChatEvent::Failed(format!("{}: {error}", self.label()))
        }
    }
}

/// `http://192.0.2.76:8123` → `192.0.2.76`.
fn host_of(base: &str) -> &str {
    let rest = base.trim_start_matches("http://").trim_start_matches("https://");
    rest.split([':', '/']).next().unwrap_or(rest)
}

/// A stopped token stays stopped, including while a worker is polling a node.
#[derive(Default)]
pub(crate) struct TurnStops(Mutex<Arc<AtomicBool>>);

impl TurnStops {
    pub(crate) fn for_line(&self) -> Arc<AtomicBool> {
        let mut current = self.0.lock().unwrap();
        if current.load(Ordering::Relaxed) {
            *current = Arc::default();
        }
        current.clone()
    }

    fn current(&self) -> Arc<AtomicBool> {
        self.0.lock().unwrap().clone()
    }

    pub(crate) fn stop(&self) {
        self.0.lock().unwrap().store(true, Ordering::Relaxed);
    }
}

const ORPHAN_TOOL_RESULTS: &str = "tool results arrived after the active turn ended or was abandoned";

enum WorkerMsg {
    UserTurn(String, Arc<AtomicBool>),
    Stop,
    Reset(u64),
    ToolResults(Vec<(String, bool)>, Arc<AtomicBool>),
    AttachImages(Vec<ToolImage>),
}

/// Token counts from a terminal context-overflow refusal on a proxy route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextOverflow {
    pub need: usize,
    pub max: usize,
}

fn overflow_of(error: &str) -> Option<ContextOverflow> {
    crate::talk::overflow_counts(error).map(|(need, max)| ContextOverflow { need, max })
}

/// A running hub chat. Same shape as [`crate::local_llm::LocalLlmSession`].
pub struct HubChatSession {
    /// The consumer is owed a terminal event, including if its worker exits.
    awaiting_reply: std::cell::Cell<bool>,
    generation: std::cell::Cell<u64>,
    wake: Option<WakeHook>,
    to_worker: Sender<WorkerMsg>,
    from_worker: Receiver<TaggedChatEvent>,
    cancel: Arc<TurnStops>,
    route: Arc<Mutex<Option<ChatRoute>>>,
    overflow: Arc<Mutex<Option<ContextOverflow>>>,
}

/// Where a session may look for a text model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChatReach {
    /// In-process weights or a local model runtime on this machine over HTTP loopback.
    Machine,
    /// Also allow pinned and discovered hubs and their advertised backends.
    #[default]
    Lan,
}

// Catalogue backends that run text generation with local weights. New backends must opt in.
const LOCAL_TEXT_BACKENDS: &[&str] = &["llm"];
pub(crate) fn no_machine_model(wanted: Option<&str>) -> String {
    let reason = "AI Hub on this computer serves no local text model";
    match wanted {
        Some(model) => format!("{reason} named {model}"),
        None => reason.into(),
    }
}

impl ChatReach {
    pub(crate) fn allows_backend(self, backend: &str) -> bool {
        self == Self::Lan || LOCAL_TEXT_BACKENDS.contains(&backend)
    }

    fn request_policy(self) -> RequestPolicy {
        match self {
            Self::Machine => RequestPolicy::on_device(),
            Self::Lan => RequestPolicy::default(),
        }
    }

    fn discovery(self) -> Option<Discovery> {
        (self == Self::Lan).then(crate::discovery::start_listener)
    }
}

pub struct HubChatConfig {
    pub reach: ChatReach,
    /// Engine limits and the local weights. The path may be empty or point
    /// at nothing: a machine with no weights still gets the fleet.
    pub llm: LocalLlmConfig,
    /// Exact fleet model id to prefer. `None` lets the hub elect normally.
    pub preferred_model: Option<String>,
    /// Fleet generation cap. `None` omits the field and lets the node use
    /// its provider default.
    pub max_tokens: Option<u32>,
    /// Fleet thinking control. `None` preserves the provider default.
    pub thinking: Option<bool>,
    /// The Claude model a turn on a hub's Claude (`claude-cli`) runs: one of
    /// [`crate::claude_cli::CLAUDE_MODELS`]. The hub refuses such a turn without one.
    pub claude_model: Option<String>,
    pub system_prompt: String,
    pub tools: Vec<ToolSpec>,
    pub wake: Option<WakeHook>,
}

impl HubChatSession {
    pub fn start(config: HubChatConfig) -> Self {
        let (event_tx, from_worker) = channel();
        let (to_worker, msg_rx) = channel();
        let cancel = Arc::new(TurnStops::default());
        let initial_stop = cancel.current();
        let worker_cancel = cancel.clone();
        let route = Arc::new(Mutex::new(None));
        let worker_route = route.clone();
        let overflow = Arc::new(Mutex::new(None));
        let worker_overflow = overflow.clone();
        let wake = config.wake.clone();
        std::thread::Builder::new()
            .name("ai-hub-chat".into())
            .spawn(move || elect_and_run(config, msg_rx, event_tx, worker_cancel, initial_stop, worker_route, worker_overflow, chat_bases))
            .expect("spawn hub chat worker");
        Self {
            awaiting_reply: Default::default(), generation: Default::default(), wake,
            to_worker,
            from_worker,
            cancel,
            route,
            overflow,
        }
    }

    pub fn send_user_turn(&self, text: String) {
        self.awaiting_reply.set(true);
        self.send_input(WorkerMsg::UserTurn(text, self.cancel.for_line()));
    }

    pub fn send_tool_results(&self, results: Vec<(String, bool)>) {
        let stop = self.cancel.current();
        if !stop.load(Ordering::Relaxed) {
            self.awaiting_reply.set(true);
        }
        self.send_input(WorkerMsg::ToolResults(results, stop));
    }

    fn send_input(&self, input: WorkerMsg) {
        *self.overflow.lock().unwrap_or_else(|e| e.into_inner()) = None;
        if self.to_worker.send(input).is_err() {
            if let Some(wake) = &self.wake { wake(); }
        }
    }

    /// Queue pictures on the same channel so the next input cannot overtake them.
    pub fn attach_images(&self, images: Vec<ToolImage>) -> Result<(), String> {
        validate_tool_images(&images)?;
        self.to_worker.send(WorkerMsg::AttachImages(images))
            .map_err(|_| "the hub chat worker stopped".to_string())
    }

    pub fn cancel(&self) {
        self.awaiting_reply.set(false);
        self.cancel.stop();
        let _ = self.to_worker.send(WorkerMsg::Stop);
    }

    /// Start fresh on the elected route. The worker cancels any active job and
    /// clears its conversation before serving the next input.
    pub fn reset_history(&self) {
        self.awaiting_reply.set(false);
        let next = self.generation.get().wrapping_add(1);
        self.generation.set(next);
        self.cancel.stop();
        *self.overflow.lock().unwrap_or_else(|e| e.into_inner()) = None;
        while self.from_worker.try_recv().is_ok() {}
        let _ = self.to_worker.send(WorkerMsg::Reset(next));
    }

    pub fn poll(&self) -> Vec<ChatEvent> {
        let current = self.generation.get();
        let mut events = Vec::new();
        loop {
            match self.from_worker.try_recv() {
                Ok(envelope) => {
                    if envelope.generation != current { continue; }
                    let event = envelope.event;
                    if matches!(event, ChatEvent::Failed(_) | ChatEvent::ContextFull | ChatEvent::TurnDone { tool_calls: 0, .. }) {
                        self.awaiting_reply.set(false);
                    }
                    events.push(event);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.awaiting_reply.replace(false) {
                        events.push(ChatEvent::Failed("The model stopped before it could answer. Start a new chat to try again.".into()));
                    }
                    break;
                }
            }
        }
        events
    }

    /// Where the conversation runs, once the election settled; `None`
    /// while it is still deciding (or when nothing answers).
    pub fn route(&self) -> Option<ChatRoute> {
        self.route.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Counts from the last terminal context-overflow refusal on a proxy route, if any.
    pub fn take_overflow(&self) -> Option<ContextOverflow> {
        self.overflow.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

impl Drop for HubChatSession {
    fn drop(&mut self) {
        // Covers election/probe and pre-admission waits before the worker
        // next observes its disconnected command receiver.
        self.cancel.stop();
    }
}

/// The election key for a model file: its lowercase basename — the same key
/// the service claims for a registry model's primary weights file, so an app
/// and a machine node loading one GGUF meet in one election.
fn election_key(config: &LocalLlmConfig) -> String {
    config
        .model
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| "unknown-model".to_string())
}

/// Why a proxied conversation stopped being one.
enum ProxyExit {
    /// The consumer is gone; the worker ends.
    Ended,
    /// The next live user line is at the front of the pending commands.
    Reelect,
}

/// The model a person is told to pick when theirs cannot call tools.
const PREFERRED_TOOL_MODEL: &str = "qwen3.8-27b";
/// A tool call for the check fits well inside this.
const TOOL_CHECK_TOKENS: u32 = 128;

/// Whether the elected models can call tools, found by asking each one once:
/// the served model itself is the only thing that knows (its weights, its
/// template, its runtime), and a model that cannot writes plausible chat.
#[derive(Default)]
struct ToolCheck {
    passed: Vec<String>,
    failed: Vec<String>,
    warned: Vec<String>,
}

impl ToolCheck {
    /// `None`: no verdict (the node failed, or the person stopped): nothing is
    /// held against the model.
    fn judge(&mut self, provider: impl FnOnce() -> FleetQwenChatProvider<RequestPolicy>, model: &str, thinking: Option<bool>, cancel: &Arc<AtomicBool>, send: &impl Fn(ChatEvent), mut wait: impl FnMut(Duration) -> bool) -> Option<bool> {
        if model == crate::claude_cli::MODEL_ID || self.passed.iter().any(|m| m == model) {
            return Some(true);
        }
        if self.failed.iter().any(|m| m == model) {
            return Some(false);
        }
        send(ChatEvent::Loading { phase: format!("checking that {model} can call tools"), fraction: 0.0 });
        let mut provider = provider()
            .with_preferred_model(Some(model.to_string()))
            .with_max_tokens(Some(TOOL_CHECK_TOKENS))
            .with_thinking(thinking)
            .with_cancel_signal(cancel.clone());
        let tool = ToolSpec::new("add", "Add two numbers.", r#"{"type":"object","properties":{"a":{"type":"number"},"b":{"type":"number"}},"required":["a","b"]}"#);
        let system = system_text("Answer by calling the tool.", &[tool]);
        let input = TurnInput::new(system, vec![ChatMessage::new(ChatRole::User, "What is 17 + 25?")]);
        provider.begin_turn(&input).ok()?;
        let mut reply = String::new();
        let verdict = loop {
            if cancel.load(Ordering::Relaxed) {
                provider.cancel();
                return None;
            }
            for event in provider.poll() {
                match event {
                    ProviderEvent::Delta(text) => reply.push_str(&text),
                    ProviderEvent::Done { .. } => return Some(self.record(model, calls_the_check_tool(&reply))),
                    ProviderEvent::Error(_) => return None,
                    _ => {}
                }
            }
            if reply.len() > TOOL_CHECK_TOKENS as usize * 32 {
                break calls_the_check_tool(&reply);
            }
            if !wait(POLL) {
                provider.cancel();
                return None;
            }
        };
        provider.cancel();
        Some(self.record(model, verdict))
    }

    fn record(&mut self, model: &str, passed: bool) -> bool {
        if passed { &mut self.passed } else { &mut self.failed }.push(model.to_string());
        passed
    }
}

/// Whether a reply is a well-formed call of the check's `add` tool.
fn calls_the_check_tool(reply: &str) -> bool {
    let mut splitter = MarkupSplitter::default();
    splitter.push(reply);
    let (_, bodies) = splitter.finish();
    read_calls(bodies).0.iter().any(|(name, args)| name == "add" && args.len() == 2)
}

fn elect_and_run(
    config: HubChatConfig,
    msg_rx: Receiver<WorkerMsg>,
    event_tx: Sender<TaggedChatEvent>,
    cancel: Arc<TurnStops>,
    initial_stop: Arc<AtomicBool>,
    route_slot: Arc<Mutex<Option<ChatRoute>>>,
    overflow_slot: Arc<Mutex<Option<ContextOverflow>>>,
    bases: impl Fn(Option<&Discovery>) -> Vec<String>,
) {
    let wake = config.wake.clone();
    let events = event_tx.clone();
    let worker_gen = Arc::new(AtomicU64::new(0));
    let event_gen = worker_gen.clone();
    let send = move |event: ChatEvent| {
        let _ = events.send(TaggedChatEvent { generation: event_gen.load(Ordering::Relaxed), event });
        if let Some(wake) = &wake {
            wake();
        }
    };
    let set_route = |route: Option<ChatRoute>| {
        *route_slot.lock().unwrap_or_else(|e| e.into_inner()) = route;
    };
    let mut pending = VecDeque::new();
    let mut images = Vec::new();
    // The conversation outlives a route: a node that failed a turn is
    // re-elected (or replaced) with everything said so far.
    let mut history: Vec<ChatMessage> = Vec::new();
    let mut tool_check = ToolCheck::default();
    let mut nothing_passed = false;
    loop {
        set_route(None);
        *overflow_slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let reach = config.reach;
        let policy = reach.request_policy();

        // 0. A launch with no model (MAKEPAD_AI_PROVIDER=none) elects
        // nothing: no hub is asked and no weights load. It says so for each
        // line the person sends.
        if let Some(reason) = crate::providers::no_model() {
            if !after_election_failure(&initial_stop, &mut pending, &msg_rx, &mut images, &mut history,
                &worker_gen, &overflow_slot, &send, reason.to_string()) { return; }
            continue;
        }

        // 1. A co-located holder serving this model over loopback.
        // A named model is strict: only a node serving exactly that id answers;
        // this machine's own weights are never loaded in its place.
        let strict = config.preferred_model.is_some();
        let has_local_file = config.llm.model.is_file() && !strict;
        let disconnected = std::cell::Cell::new(false);
        let mut wait = |duration| {
            let connected = election_wait(duration, &msg_rx, &mut pending, &mut images, &mut history, &worker_gen);
            disconnected.set(!connected);
            connected
        };
        if has_local_file {
            let key = election_key(&config.llm);
            let port = machine_holder_port(&key, &send, &mut wait);
            if disconnected.get() { return; }
            if let Some(port) = port {
                let base = format!("http://127.0.0.1:{port}");
                let route = ChatRoute::MachineNode { port };
                let mut provider = FleetQwenChatProvider::for_reach(policy.clone(), vec![base], reach)
                    .with_preferred_model(config.preferred_model.clone())
                    .with_max_tokens(config.max_tokens)
                    .with_thinking(config.thinking)
                    .with_claude_model(config.claude_model.clone());
                if reach == ChatReach::Lan || matches!(provider.availability(), ProviderAvailability::Available { .. }) {
                    if !drain_election_msgs(&msg_rx, &mut pending, &mut images, &mut history, &worker_gen) { return; }
                    set_route(Some(route.clone()));
                    match run_proxy(provider, &config, &mut pending, &msg_rx, &mut images, &mut history, &worker_gen, &send, route, &overflow_slot, POLL) {
                        ProxyExit::Ended => return,
                        ProxyExit::Reelect => continue,
                    }
                }
            }
        }

        // 2. A fleet chat node on the LAN: one whose model can call tools, when
        // this chat has tools and any can.
        let excluded = if nothing_passed { Vec::new() } else { tool_check.failed.clone() };
        let discovery = reach.discovery();
        let fleet = fleet_provider_with(
            reach,
            &send,
            config.preferred_model.as_deref(),
            config.max_tokens,
            config.thinking,
            excluded,
            || bases(discovery.as_ref()),
            || policy.clone(),
            fleet_patience(discovery.as_ref()),
            FLEET_POLL,
            wait,
        );
        if disconnected.get() { return; }
        let fleet_reason = match policy.check().and(fleet) {
            Ok((provider, base, model)) => {
                let provider = provider.with_claude_model(config.claude_model.clone());
                if !config.tools.is_empty() {
                    let check = || FleetQwenChatProvider::for_reach(policy.clone(), vec![base.clone()], reach);
                    let verdict = tool_check.judge(check, &model, config.thinking, &cancel.current(), &send, |duration| {
                        // Return to the check on each command so it sees a tripped stop immediately.
                        match msg_rx.recv_timeout(duration) {
                            Ok(msg) => apply_election_msg(msg, &mut pending, &mut images, &mut history, &worker_gen),
                            Err(RecvTimeoutError::Timeout) => {}
                            Err(RecvTimeoutError::Disconnected) => { disconnected.set(true); return false; }
                        }
                        true
                    });
                    if disconnected.get() { return; }
                    if let Err(reason) = policy.check() {
                        send(ChatEvent::Failed(reason));
                        return;
                    }
                    if verdict == Some(false) {
                        if config.preferred_model.is_none() && !nothing_passed {
                            continue;
                        }
                        // Named, or the only one there is: it answers, and the
                        // person is told what it cannot do and what to pick.
                        if !tool_check.warned.contains(&model) {
                            tool_check.warned.push(model.clone());
                            let choice = match reach {
                                ChatReach::Lan => format!("{PREFERRED_TOOL_MODEL} or Claude Code"),
                                ChatReach::Machine => "a local model that supports tools".into(),
                            };
                            send(ChatEvent::Delta(format!(
                                "{model} did not pass the tool check: it can talk, but it cannot use this app's tools. \
                                 Choose {choice} in the chat's model menu.\n\n"
                            )));
                        }
                    }
                }
                let route = match reach {
                    ChatReach::Machine => ChatRoute::MachineNode {
                        port: crate::http_client::parse_url(&base).map_or(0, |url| url.port),
                    },
                    ChatReach::Lan => ChatRoute::Fleet { base, model },
                };
                if !drain_election_msgs(&msg_rx, &mut pending, &mut images, &mut history, &worker_gen) { return; }
                set_route(Some(route.clone()));
                match run_proxy(provider, &config, &mut pending, &msg_rx, &mut images, &mut history, &worker_gen, &send, route, &overflow_slot, POLL) {
                    ProxyExit::Ended => return,
                    ProxyExit::Reelect => continue,
                }
            }
            // Every model failed: the best of them answers, with the warning.
            Err(_) if policy.check().is_ok() && !tool_check.failed.is_empty() && !nothing_passed => {
                nothing_passed = true;
                continue;
            }
            Err(reason) => reason,
        };

        // 3. The weights on this machine (a build with the `llm` runtime).
        #[cfg(feature = "llm")]
        if has_local_file && policy.check().is_ok() {
            eprintln!("[hub-chat] no fleet chat node ({fleet_reason}); loading {}", config.llm.model.display());
            let key = election_key(&config.llm);
            let guard = match machine::claim(&key) {
                Ok(Claim::Won(mut guard)) => {
                    let _ = guard.publish(ResidencyState::Loading { fraction: 0.0 });
                    Some(guard)
                }
                _ => None,
            };
            set_route(Some(ChatRoute::InProcess { model: key }));
            let prefix = build_prefix(&config.system_prompt, &config.tools);
            if !drain_election_msgs(&msg_rx, &mut pending, &mut images, &mut history, &worker_gen) { return; }
            let initial_gen = worker_gen.load(Ordering::Relaxed);
            let local_rx = local_commands(pending, msg_rx, images, initial_gen, event_tx.clone(), config.wake.clone());
            crate::local_llm::worker_main(
                config.llm,
                prefix,
                None,
                local_rx,
                initial_gen,
                event_tx,
                config.wake,
                guard,
            );
            return;
        }

        // 4. Nothing answers. Say so once, and try again on the person's
        // next line — a fleet node may have come up by then. The line that
        // just failed is dropped: handing it back would retry it forever.
        eprintln!("[hub-chat] no model: {fleet_reason}; local {}", config.llm.model.display());
        let reason = match config.preferred_model.as_deref() {
            Some(wanted) => format!(
                "The required AI model {wanted} is not available: {fleet_reason}. No other model is used (MAKEPAD_AI_MODEL)."
            ),
            None if reach == ChatReach::Machine => fleet_reason,
            None => no_model_message(&config.llm.model),
        };
        if !after_election_failure(&initial_stop, &mut pending, &msg_rx, &mut images, &mut history,
            &worker_gen, &overflow_slot, &send, reason) { return; }
    }
}

/// The port of a co-located node serving this model, waiting out one
/// that is still loading. `None` when the election is open, held by a
/// non-serving app (the documented duplicate-copy soft failure), or the
/// holder failed.
fn machine_holder_port(key: &str, send: &impl Fn(ChatEvent), mut wait: impl FnMut(Duration) -> bool) -> Option<u16> {
    let deadline = Instant::now() + HOLDER_PATIENCE;
    loop {
        match machine::read_holder(key) {
            Ok(Some(record)) => match record.state {
                ResidencyState::Ready { port } if port > 0 => return Some(port),
                ResidencyState::Ready { .. } => {
                    eprintln!(
                        "[hub-chat] {key}: held by pid {} without a serving port — looking elsewhere",
                        record.pid
                    );
                    return None;
                }
                ResidencyState::Loading { fraction } => {
                    send(ChatEvent::Loading {
                        phase: format!("waiting on pid {}", record.pid),
                        fraction,
                    });
                    if Instant::now() > deadline {
                        eprintln!("[hub-chat] {key}: holder still loading after patience — looking elsewhere");
                        return None;
                    }
                    if !wait(POLL * 4) { return None; }
                }
                ResidencyState::Failed { reason, .. } => {
                    eprintln!("[hub-chat] {key}: holder reported failure ({reason}); looking elsewhere");
                    return None;
                }
            },
            _ => return None,
        }
    }
}

/// The fleet chat nodes among what discovery heard: every node whose role
/// allows `chat`, sorted, each once.
pub fn fleet_chat_bases(nodes: &[crate::discovery::DiscoveredNode]) -> Vec<String> {
    let mut bases: Vec<String> = nodes
        .iter()
        .filter(|n| crate::fleet::role_allows(&n.base_url, "chat"))
        .map(|n| n.base_url.clone())
        .collect();
    bases.sort();
    bases.dedup();
    bases
}

fn fleet_patience(discovery: Option<&Discovery>) -> Duration {
    if matches!(discovery, Some(Discovery::Available(_))) { FLEET_PATIENCE } else { Duration::ZERO }
}

fn machine_chat_bases() -> Vec<String> {
    // Entries may be stale. The provider probes the port before using it.
    let mut bases: Vec<_> = machine::read_node_entries().into_iter().filter_map(
        |(_, entry)| (entry.port != 0).then(|| format!("http://127.0.0.1:{}", entry.port)),
    ).collect();
    bases.sort();
    bases.dedup();
    bases
}

/// With no discovery, only the local runtime directory supplies candidates.
fn chat_bases(discovery: Option<&Discovery>) -> Vec<String> {
    let mut bases = machine_chat_bases();
    let Some(discovery) = discovery else { return bases; };
    let heard = fleet_chat_bases(&discovery.nodes());
    bases.extend(heard.iter().cloned());
    // A pinned hub (MAKEPAD_AI_HUB, the client's `pinned` file) is
    // reached without its LAN beacon; a pin without a port takes its beacon's.
    bases.extend(crate::hub_order::pinned_bases_beside(&heard));
    crate::hub_order::order(&mut bases);
    dedup_in_order(bases)
}

/// One chat model a hub offers, at the best-ranked hub that offers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubOffer {
    pub base: String,
    /// The id a chat names to get exactly it ([`HubChatConfig::preferred_model`]).
    pub model: String,
    /// What serves it: `claude-cli` is the hub's own Claude Code.
    pub backend: String,
}

/// The hubs' chat inventory: every chat model the reachable hubs serve, each
/// once (at the best-ranked hub offering it), in hub order — the same
/// discovery and `/models` probe the election runs; nothing loaded, no
/// prompt. `update` hears each growing answer while the fleet patience runs
/// (a LAN beacon may land after this machine's hub answered). Blocks up to
/// the patience: call it from a worker thread. `Err` when no hub offers any.
pub fn probe_fleet_inventory(update: impl FnMut(&[HubOffer])) -> Result<Vec<HubOffer>, String> {
    probe_inventory(ChatReach::Lan, update)
}

/// Probe this machine's node entries once for local text runtimes, respecting
/// `MAKEPAD_AI_MODEL`. Performs blocking loopback I/O. Call from a worker.
pub fn probe_machine_inventory() -> Result<Vec<HubOffer>, String> {
    probe_inventory(ChatReach::Machine, |_| {})
}

fn probe_inventory(reach: ChatReach, mut update: impl FnMut(&[HubOffer])) -> Result<Vec<HubOffer>, String> {
    // `MAKEPAD_AI_MODEL` names the one model every app chat here must use:
    // only it is offered.
    let required = std::env::var(crate::hub::REQUIRED_MODEL_ENV).ok().map(|m| m.trim().to_string()).filter(|m| !m.is_empty());
    if FLEET_PROBES_STOPPED.load(Ordering::Relaxed) {
        return Err("the app is shutting down".into());
    }
    let discovery = reach.discovery();
    let mut policy = reach.request_policy();
    let started = Instant::now();
    let mut offers: Vec<HubOffer> = Vec::new();
    let mut probed: Vec<String> = Vec::new();
    let mut last_reason = None;
    loop {
        if FLEET_PROBES_STOPPED.load(Ordering::Relaxed) {
            break;
        }
        let fresh: Vec<String> = chat_bases(discovery.as_ref()).into_iter().filter(|b| !probed.contains(b)).collect();
        let mut grew = false;
        for base in &fresh {
            probed.push(base.clone());
            match chat_offers_for(&mut policy, base, reach) {
                Ok(found) => {
                    for offer in found {
                        if required.as_deref().is_some_and(|r| r != offer.model) {
                            continue;
                        }
                        if !offers.iter().any(|o| o.model == offer.model) {
                            offers.push(HubOffer { base: base.clone(), model: offer.model, backend: offer.backend });
                            grew = true;
                        }
                    }
                }
                Err(reason) => {
                    policy.check()?;
                    last_reason = Some(reason);
                }
            }
        }
        if grew {
            update(&offers);
        }
        if started.elapsed() >= fleet_patience(discovery.as_ref()) {
            break;
        }
        std::thread::sleep(FLEET_POLL);
    }
    if offers.is_empty() {
        return Err(last_reason.unwrap_or_else(|| match reach {
            ChatReach::Machine => no_machine_model(required.as_deref()),
            ChatReach::Lan => format!("no fleet chat node heard on the LAN in {:.1} s", FLEET_PATIENCE.as_secs_f64()),
        }));
    }
    Ok(offers)
}

/// The first of each base, in the order given.
fn dedup_in_order(bases: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    bases.into_iter().filter(|b| seen.insert(b.clone())).collect()
}

fn fleet_provider_with<T: FleetTransport>(
    reach: ChatReach,
    send: &impl Fn(ChatEvent),
    preferred_model: Option<&str>,
    max_tokens: Option<u32>,
    thinking: Option<bool>,
    excluded: Vec<String>,
    mut discovered_bases: impl FnMut() -> Vec<String>,
    mut transport: impl FnMut() -> T,
    patience: Duration,
    poll: Duration,
    mut wait: impl FnMut(Duration) -> bool,
) -> Result<(FleetQwenChatProvider<T>, String, String), String> {
    let started = Instant::now();
    let mut fallback = None;
    let mut last_reason = None;
    let mut final_pass = false;
    loop {
        // Keep the ranking `discovered_bases` made (this machine's hub,
        // then a pinned one, then the LAN): sorting here put a LAN node
        // ahead of 127.0.0.1 ("10." < "127").
        let bases = dedup_in_order(discovered_bases());
        if !bases.is_empty() {
            let mut provider = FleetQwenChatProvider::for_reach(transport(), bases.clone(), reach)
                .with_preferred_model(preferred_model.map(str::to_string))
                .with_max_tokens(max_tokens)
                .with_thinking(thinking)
                .with_excluded_models(excluded.clone());
            match provider.availability() {
                ProviderAvailability::Available { model, detail } => {
                    if preferred_model.is_none_or(|preferred| preferred.trim() == model) {
                        return Ok((provider, detail, model));
                    }
                    // A healthy ordinary pick is remembered, but a preference
                    // gets the full discovery window to announce before this
                    // fallback is accepted.
                    fallback = Some((provider, detail, model));
                }
                ProviderAvailability::Unavailable { reason } => last_reason = Some(reason),
            }
        }
        if reach == ChatReach::Machine {
            return Err(last_reason.unwrap_or_else(|| no_machine_model(preferred_model)));
        }
        if started.elapsed() >= patience {
            // A stale/unresponsive loopback entry may have consumed the whole
            // patience window inside its HTTP timeout. Take one final roster
            // snapshot so a LAN node announced during that probe is still
            // considered instead of being hidden by the stale file.
            if !final_pass {
                let latest = dedup_in_order(discovered_bases());
                if latest != bases {
                    final_pass = true;
                    continue;
                }
            }
            if let Some(fallback) = fallback.filter(|_| preferred_model.is_none()) {
                return Ok(fallback);
            }
            return Err(last_reason.unwrap_or_else(|| {
                format!(
                    "no fleet chat node heard on the LAN in {:.1} s (fleet '{}')",
                    patience.as_secs_f64(),
                    crate::discovery::wanted_fleet()
                )
            }));
        }
        send(ChatEvent::Loading {
            phase: "listening for the fleet".to_string(),
            fraction: (started.elapsed().as_secs_f64() / patience.as_secs_f64()).min(0.99),
        });
        if !wait(poll) { return Err("chat disconnected".into()); }
    }
}

/// What the person reads when nothing answers: plainly, and where a model
/// may be put. The routes that were tried go to the log. No download starts
/// on its own.
pub fn no_model_message(model: &Path) -> String {
    if model.as_os_str().is_empty() {
        format!(
            "No local AI model is installed yet. Put a model file (.gguf) in {}, or pick another AI above.",
            crate::home::weights_dir().display()
        )
    } else {
        format!(
            "The local AI model {} is missing. Put it back, or pick another AI above.",
            model.display()
        )
    }
}

/// Pictures attached before the next input add up (a tool's render and a
/// look at the view both reach the model), the newest kept within the
/// providers' per-turn limit.
fn queue_images(images: &mut Vec<ToolImage>, attached: Vec<ToolImage>) {
    const MAX_TURN_IMAGES: usize = 4;
    images.extend(attached);
    if images.len() > MAX_TURN_IMAGES {
        images.drain(..images.len() - MAX_TURN_IMAGES);
    }
}

fn pop_or_try_recv(pending: &mut VecDeque<WorkerMsg>, msg_rx: &Receiver<WorkerMsg>) -> Result<WorkerMsg, TryRecvError> {
    pending.pop_front().map_or_else(|| msg_rx.try_recv(), Ok)
}

fn pop_or_recv_timeout(
    pending: &mut VecDeque<WorkerMsg>, msg_rx: &Receiver<WorkerMsg>, timeout: Duration,
) -> Result<WorkerMsg, RecvTimeoutError> {
    pending.pop_front().map_or_else(|| msg_rx.recv_timeout(timeout), Ok)
}

fn apply_election_msg(
    msg: WorkerMsg, pending: &mut VecDeque<WorkerMsg>, images: &mut Vec<ToolImage>,
    history: &mut Vec<ChatMessage>, generation: &AtomicU64,
) {
    match msg {
        WorkerMsg::Reset(next) => {
            history.clear();
            images.clear();
            pending.clear();
            generation.store(next, Ordering::Relaxed);
        }
        WorkerMsg::Stop => {
            images.clear();
            pending.clear();
        }
        other => pending.push_back(other),
    }
}

fn drain_election_msgs(
    msg_rx: &Receiver<WorkerMsg>, pending: &mut VecDeque<WorkerMsg>, images: &mut Vec<ToolImage>,
    history: &mut Vec<ChatMessage>, generation: &AtomicU64,
) -> bool {
    loop {
        match msg_rx.try_recv() {
            Ok(msg) => apply_election_msg(msg, pending, images, history, generation),
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
}

fn election_wait(
    duration: Duration, msg_rx: &Receiver<WorkerMsg>, pending: &mut VecDeque<WorkerMsg>,
    images: &mut Vec<ToolImage>, history: &mut Vec<ChatMessage>, generation: &AtomicU64,
) -> bool {
    if duration.is_zero() { return drain_election_msgs(msg_rx, pending, images, history, generation); }
    let deadline = Instant::now() + duration;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match msg_rx.recv_timeout(remaining) {
            Ok(msg) => apply_election_msg(msg, pending, images, history, generation),
            Err(RecvTimeoutError::Timeout) => return true,
            Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
    true
}

/// A startup failure is observable even before a line arrives. Invalidate its
/// input token before publishing it: a line still in the queue on that token
/// belongs to the failed election and must never be replayed on a new node.
fn after_election_failure(
    initial_stop: &Arc<AtomicBool>, pending: &mut VecDeque<WorkerMsg>, msg_rx: &Receiver<WorkerMsg>,
    images: &mut Vec<ToolImage>, history: &mut Vec<ChatMessage>, generation: &AtomicU64,
    overflow: &Mutex<Option<ContextOverflow>>, send: &impl Fn(ChatEvent), reason: String,
) -> bool {
    let connected = drain_election_msgs(msg_rx, pending, images, history, generation);
    let stop = loop {
        match pending.pop_front() {
            Some(WorkerMsg::UserTurn(_, stop)) if !stop.load(Ordering::Relaxed) => break stop,
            Some(WorkerMsg::ToolResults(_, stop)) if !stop.load(Ordering::Relaxed) => {
                send(ChatEvent::Failed(ORPHAN_TOOL_RESULTS.into()));
            }
            Some(_) => {}
            None => break initial_stop.clone(),
        }
    };
    if !stop.swap(true, Ordering::Relaxed) {
        send(ChatEvent::Failed(reason));
    }
    images.clear();
    connected && wait_next_user(pending, msg_rx, images, history, generation, overflow, send)
}

/// Wait for the next user line and report tool results left without a turn.
fn wait_next_user(
    pending: &mut VecDeque<WorkerMsg>, msg_rx: &Receiver<WorkerMsg>, images: &mut Vec<ToolImage>,
    history: &mut Vec<ChatMessage>, generation: &AtomicU64, overflow: &Mutex<Option<ContextOverflow>>,
    send: &impl Fn(ChatEvent),
) -> bool {
    loop {
        match pending.pop_front().map_or_else(|| msg_rx.recv(), Ok) {
            Ok(WorkerMsg::UserTurn(text, stop)) => {
                if !stop.load(Ordering::Relaxed) {
                    pending.push_front(WorkerMsg::UserTurn(text, stop));
                    return true;
                }
                images.clear();
            }
            Ok(WorkerMsg::Reset(next)) => {
                *overflow.lock().unwrap_or_else(|e| e.into_inner()) = None;
                history.clear();
                images.clear();
                generation.store(next, Ordering::Relaxed);
            }
            Ok(WorkerMsg::ToolResults(_, stop)) => {
                images.clear();
                if !stop.load(Ordering::Relaxed) {
                    send(ChatEvent::Failed(ORPHAN_TOOL_RESULTS.into()));
                }
            }
            Ok(WorkerMsg::Stop) => images.clear(),
            Ok(WorkerMsg::AttachImages(attached)) => queue_images(images, attached),
            Err(_) => return false,
        }
    }
}

// The in-process worker only accepts text; reject image turns before forwarding.
#[cfg(feature = "llm")]
fn local_commands(
    pending: VecDeque<WorkerMsg>,
    msg_rx: Receiver<WorkerMsg>,
    images: Vec<ToolImage>,
    initial_gen: u64,
    event_tx: Sender<TaggedChatEvent>,
    wake: Option<WakeHook>,
) -> Receiver<crate::local_llm::WorkerMsg> {
    let (to_local, local_rx) = channel();
    let mut has_images = !images.is_empty();
    std::thread::Builder::new()
        .name("ai-hub-chat-input".into())
        .spawn(move || {
            let mut generation = initial_gen;
            let mut active = None;
            for msg in pending.into_iter().chain(msg_rx) {
                let msg = match msg {
                    WorkerMsg::AttachImages(images) => {
                        has_images = !images.is_empty();
                        continue;
                    }
                    WorkerMsg::Stop => {
                        has_images = false;
                        continue;
                    }
                    WorkerMsg::Reset(next) => {
                        generation = next;
                        has_images = false;
                        active = None;
                        crate::local_llm::WorkerMsg::Reset(next)
                    }
                    WorkerMsg::UserTurn(text, stop) => {
                        if stop.load(Ordering::Relaxed) { has_images = false; continue; }
                        active = Some(stop.clone());
                        crate::local_llm::WorkerMsg::UserTurn(text, stop)
                    }
                    WorkerMsg::ToolResults(results, stop) => {
                        if stop.load(Ordering::Relaxed) { has_images = false; continue; }
                        crate::local_llm::WorkerMsg::ToolResults(results)
                    }
                };
                if std::mem::take(&mut has_images) {
                    // A refused tool continuation belongs to the original user line.
                    if active.as_ref().is_some_and(|stop| stop.swap(true, Ordering::Relaxed)) { continue; }
                    let event = ChatEvent::Failed("the in-process model cannot look at pictures".into());
                    let _ = event_tx.send(TaggedChatEvent { generation, event });
                    if let Some(wake) = &wake { wake(); }
                    continue;
                }
                if to_local.send(msg).is_err() {
                    drop(event_tx);
                    if let Some(wake) = &wake { wake(); }
                    break;
                }
            }
        })
        .expect("spawn hub chat input worker");
    local_rx
}

struct ProxyTurn {
    start: usize,
    provider: FleetTranscriptMark,
    elided: Vec<(usize, String)>,
    stop: Arc<AtomicBool>,
}

impl ProxyTurn {
    fn restore<T: FleetTransport>(self, provider: &mut FleetQwenChatProvider<T>, history: &mut Vec<ChatMessage>) {
        provider.restore(self.provider);
        history.truncate(self.start);
        for (index, text) in self.elided.into_iter().rev() {
            history[index].text = text;
        }
    }

    fn shorten(&mut self, history: &mut [ChatMessage], need: usize, max: usize) -> bool {
        crate::chat_shorten::shorten_history(history, need, max, |index, text| {
            if index < self.start && !self.elided.iter().any(|(i, _)| *i == index) {
                self.elided.push((index, text));
            }
        })
    }
}

/// Pictures can arrive mid-turn. A replacement line or reset belongs to
/// the outer loop and must stay ahead of any later commands.
fn interrupts_proxy_turn(msg: WorkerMsg, pending: &mut VecDeque<WorkerMsg>, images: &mut Vec<ToolImage>) -> bool {
    match msg {
        WorkerMsg::AttachImages(attached) => { queue_images(images, attached); false }
        msg @ (WorkerMsg::UserTurn(..) | WorkerMsg::Reset(_)) => { pending.push_front(msg); true }
        WorkerMsg::Stop | WorkerMsg::ToolResults(..) => { images.clear(); true }
    }
}

/// The proxy branch: the conversation runs on a node; this worker holds
/// the transcript, hands the node the same tool-bearing system text the
/// in-process prefix carries, and splits the node's `<think>` and
/// `<tool_call>` markup out of the streamed text with the one parser.
fn run_proxy<T: FleetTransport>(
    mut provider: FleetQwenChatProvider<T>,
    config: &HubChatConfig,
    pending: &mut VecDeque<WorkerMsg>,
    msg_rx: &Receiver<WorkerMsg>,
    images: &mut Vec<ToolImage>,
    history: &mut Vec<ChatMessage>,
    generation: &AtomicU64,
    send: &impl Fn(ChatEvent),
    route: ChatRoute,
    overflow: &Mutex<Option<ContextOverflow>>,
    poll: Duration,
) -> ProxyExit {
    let mut turn: Option<ProxyTurn> = None;
    let system = system_text(&config.system_prompt, &config.tools);
    send(ChatEvent::Ready {
        prefill_tokens: 0,
        secs: 0.0,
    });
    loop {
        let msg = match pending.pop_front() {
            Some(msg) => msg,
            None => match msg_rx.recv() {
                Ok(msg) => msg,
                Err(_) => return ProxyExit::Ended,
            },
        };
        match msg {
            WorkerMsg::Reset(next) => {
                turn = None;
                provider.reset_conversation();
                history.clear();
                images.clear();
                *overflow.lock().unwrap_or_else(|e| e.into_inner()) = None;
                generation.store(next, Ordering::Relaxed);
                continue;
            }
            WorkerMsg::AttachImages(attached) => {
                queue_images(images, attached);
                continue;
            }
            WorkerMsg::Stop => {
                if let Some(mark) = turn.take() {
                    mark.restore(&mut provider, history);
                }
                images.clear();
                continue;
            }
            WorkerMsg::UserTurn(text, stop) => {
                *overflow.lock().unwrap_or_else(|e| e.into_inner()) = None;
                // A new line can also abandon a turn while its tool is running.
                if let Some(mark) = turn.take() {
                    mark.restore(&mut provider, history);
                }
                if stop.load(Ordering::Relaxed) { images.clear(); continue; }
                turn = Some(ProxyTurn { start: history.len(), provider: provider.checkpoint(), elided: Vec::new(), stop: stop.clone() });
                provider = provider.with_cancel_signal(stop);
                history.push(ChatMessage::new(ChatRole::User, text));
            }
            WorkerMsg::ToolResults(results, stop) => {
                *overflow.lock().unwrap_or_else(|e| e.into_inner()) = None;
                if stop.load(Ordering::Relaxed) {
                    images.clear();
                    continue;
                }
                if turn.is_none() {
                    images.clear();
                    send(ChatEvent::Failed(ORPHAN_TOOL_RESULTS.into()));
                    continue;
                }
                // One `<tool_response>` turn per result, in call order — the
                // provider wraps each in the trained tags.
                for (text, is_error) in results {
                    let text = if is_error { format!("ERROR: {text}") } else { text };
                    history.push(ChatMessage::new(ChatRole::Tool, text));
                }
            }
        }
        let cancel = turn.as_ref().unwrap().stop.clone();
        // A turn the node refuses for its context (the conversation outgrew
        // it) can be retried with shortened history before any prose reaches the user.
        let mut compacted = false;
        // Replies whose tool calls could not be read, sent back to the model.
        let mut rewrites = 0usize;
        loop {
        let mut retry = false;
        let mut abandoned = false;
        // Keep the prompt inside the node's context before sending it: a
        // refused prompt costs a round trip (and has knocked a node's model
        // over). About 3 bytes a token for code and JSON, room to answer.
        if let Some(max) = provider.context_tokens() {
            let estimate = (system.len() + history.iter().map(|m| m.text.len() + 16).sum::<usize>()) / 3 + ANSWER_ROOM;
            if estimate > max && turn.as_mut().unwrap().shorten(history, estimate, max) {
                provider.history_pruned(0);
                send(ChatEvent::Loading { phase: "shortening the conversation".into(), fraction: 0.0 });
            }
        }
        let input = TurnInput::new(system.clone(), history.clone());
        'attempt: {
            if cancel.load(Ordering::Relaxed) {
                break 'attempt;
            }
            let result = provider.attach_tool_images(std::mem::take(images))
                .and_then(|()| provider.begin_turn(&input));
            if cancel.load(Ordering::Relaxed) { break 'attempt; }
            if let Err(error) = result {
                turn.take().unwrap().restore(&mut provider, history);
                *overflow.lock().unwrap_or_else(|e| e.into_inner()) = overflow_of(&error);
                send(route.failure(&error, |detail| eprintln!("[hub-chat] {detail}")));
                return if wait_next_user(pending, msg_rx, images, history, generation, overflow, send) {
                    ProxyExit::Reelect
                } else { ProxyExit::Ended };
            }
            let started = Instant::now();
            let mut splitter = MarkupSplitter::default();
            let mut gen_tokens = 0u32;
            let mut pending_visible = String::new();
            let mut has_visible_text = false;
            let mut hidden = HiddenWork::default();
            let mut last_progress = Instant::now();
            let mut last_note = String::from("submitted");
            'turn: loop {
            if cancel.load(Ordering::Relaxed) {
                break 'turn;
            }
            // A line typed mid-answer means the person moved on: the turn
            // is abandoned and that line is served next.
            match pop_or_try_recv(pending, msg_rx) {
                Err(TryRecvError::Empty) => {}
                Ok(msg) => {
                    if interrupts_proxy_turn(msg, pending, images) {
                        abandoned = true;
                        break 'turn;
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    provider.cancel();
                    return ProxyExit::Ended;
                }
            }
            let mut done = false;
            let events = provider.poll();
            if cancel.load(Ordering::Relaxed) { break 'turn; }
            if !events.is_empty() || provider.waiting_for_machine() {
                last_progress = Instant::now();
            } else if last_progress.elapsed() >= STALL && !last_note.contains("queued") {
                // Nothing came from the hub for a while: say so (with the last
                // thing it said) and give the line back, instead of a bar that
                // never moves. The hub's own job is cancelled; nothing is retried.
                turn.take().unwrap().restore(&mut provider, history);
                *overflow.lock().unwrap_or_else(|e| e.into_inner()) = None;
                send(ChatEvent::Failed(format!(
                    "{}: no progress from the AI hub for {} s (last: {last_note}); send the message again to retry",
                    route.label(),
                    STALL.as_secs()
                )));
                return if wait_next_user(pending, msg_rx, images, history, generation, overflow, send) {
                    ProxyExit::Reelect
                } else { ProxyExit::Ended };
            }
            for event in events {
                if cancel.load(Ordering::Relaxed) { break 'turn; }
                match event {
                    ProviderEvent::Delta(text) => {
                        let visible = splitter.push(&text);
                        if let Some(in_call) = splitter.hidden() {
                            if let Some(note) = hidden.wrote(in_call, text.len()) {
                                send(ChatEvent::Working(note));
                            }
                        }
                        if !visible.is_empty() {
                            if has_visible_text {
                                send(ChatEvent::Delta(visible));
                            } else {
                                pending_visible.push_str(&visible);
                                if !pending_visible.trim().is_empty() {
                                    has_visible_text = true;
                                    send(ChatEvent::Delta(std::mem::take(&mut pending_visible)));
                                }
                            }
                        }
                    }
                    ProviderEvent::Serving(facts) => gen_tokens = facts.gen_tokens,
                    ProviderEvent::Status { note, permille } => {
                        last_note = note.clone();
                        send(ChatEvent::Loading { phase: note, fraction: permille as f64 / 1000.0 })
                    }
                    ProviderEvent::Done { text } => {
                        let cut_call = splitter.unfinished_call();
                        let (visible, bodies) = splitter.finish();
                        if !visible.is_empty() {
                            if has_visible_text {
                                send(ChatEvent::Delta(visible));
                            } else {
                                pending_visible.push_str(&visible);
                                if !pending_visible.trim().is_empty() {
                                    has_visible_text = true;
                                    send(ChatEvent::Delta(std::mem::take(&mut pending_visible)));
                                }
                            }
                        }
                        if let Some(body) = cut_call {
                            // The reply stopped inside a call: say so instead of ending silently.
                            let tail: String = body.chars().rev().take(80).collect::<Vec<_>>().into_iter().rev().collect();
                            send(ChatEvent::Delta(format!("[bad tool call: the reply ended inside a tool call, after ...{tail:?}]")));
                            has_visible_text = true;
                        }
                        if !has_visible_text && bodies.is_empty() {
                            // An accepted job is never a reason to replay
                            // the prompt, even when it produced only thinking.
                            turn.take().unwrap().restore(&mut provider, history);
                            *overflow.lock().unwrap_or_else(|e| e.into_inner()) = None;
                            send(ChatEvent::Failed("empty completion".to_string()));
                            break 'attempt;
                        }
                        // The FULL reply is the history's: the provider's
                        // wire mirror is what the node's KV extends, and
                        // the session history only has to keep the roles
                        // in step with it.
                        if !text.trim().is_empty() {
                            history.push(ChatMessage::new(ChatRole::Assistant, text));
                        }
                        let (calls, unreadable) = read_calls(bodies);
                        let tool_calls = calls.len();
                        for (name, args) in calls {
                            send(ChatEvent::ToolCall { name, args });
                        }
                        // Nothing ran: tell the model why and let it write the call
                        // again (twice at most) instead of ending the turn silently.
                        if tool_calls == 0 && !unreadable.is_empty() && rewrites < 2 {
                            rewrites += 1;
                            let (error, head) = &unreadable[0];
                            history.push(ChatMessage::new(ChatRole::Tool, format!(
                                "ERROR: your tool call could not be read ({error}); it began {head:?}. Nothing ran. Write the call again: one valid JSON object {{\"name\": \"TOOL\", \"arguments\": {{...}}}} inside <tool_call> tags (strings quoted, no comments, no trailing commas)."
                            )));
                            send(ChatEvent::Loading { phase: "the model is rewriting a tool call".into(), fraction: 0.0 });
                            retry = true;
                            break 'attempt;
                        }
                        for (error, head) in unreadable {
                            send(ChatEvent::Delta(format!("[bad tool call: {error}; it began {head:?}]")));
                        }
                        if tool_calls == 0 { turn = None; }
                        send(ChatEvent::TurnDone {
                            tool_calls,
                            tokens: gen_tokens as usize,
                            secs: started.elapsed().as_secs_f64(),
                            context_used: 0,
                            context_max: provider.context_tokens().unwrap_or(0),
                        });
                        done = true;
                    }
                    ProviderEvent::Error(error) => {
                        // The desktop needs the GPU right now. The turn is not
                        // lost: it waits, saying so, until the model fits or
                        // the person stops it.
                        if is_ui_priority_refusal(&error) {
                            send(ChatEvent::Loading { phase: format!("waiting for GPU memory ({error})"), fraction: 0.0 });
                            let mut waited = Duration::ZERO;
                            while waited < UI_PRIORITY_RETRY && !cancel.load(Ordering::Relaxed) {
                                match pop_or_recv_timeout(pending, msg_rx, poll) {
                                    Err(RecvTimeoutError::Timeout) => waited = waited.saturating_add(poll),
                                    Ok(msg) => {
                                        if interrupts_proxy_turn(msg, pending, images) {
                                            abandoned = true;
                                            break 'attempt;
                                        }
                                    }
                                    Err(RecvTimeoutError::Disconnected) => {
                                        provider.cancel();
                                        return ProxyExit::Ended;
                                    }
                                }
                            }
                            retry = true;
                            break 'attempt;
                        }
                        if !has_visible_text && !compacted {
                            // Both refusal texts: the node's own ("the prompt is N
                            // tokens ... context is M") and a session's ("need N").
                            if let Some((need, max)) = crate::talk::overflow_counts(&error) {
                                compacted = true;
                                if turn.as_mut().unwrap().shorten(history, need, max) {
                                    provider.history_pruned(0);
                                    send(ChatEvent::Loading { phase: "shortening the conversation".into(), fraction: 0.0 });
                                    retry = true;
                                    break 'attempt;
                                }
                            }
                        }
                        turn.take().unwrap().restore(&mut provider, history);
                        *overflow.lock().unwrap_or_else(|e| e.into_inner()) = overflow_of(&error);
                        send(route.failure(&error, |detail| eprintln!("[hub-chat] {detail}")));
                        return if wait_next_user(pending, msg_rx, images, history, generation, overflow, send) {
                            ProxyExit::Reelect
                        } else { ProxyExit::Ended };
                    }
                    ProviderEvent::FunctionCall { .. } | ProviderEvent::FunctionCallPartial { .. } => {}
                }
            }
            if done {
                break 'turn;
            }
            match pop_or_recv_timeout(pending, msg_rx, poll) {
                Err(RecvTimeoutError::Timeout) => {}
                Ok(msg) => {
                    if interrupts_proxy_turn(msg, pending, images) {
                        abandoned = true;
                        break 'turn;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    provider.cancel();
                    return ProxyExit::Ended;
                }
            }
            }
        }
        if cancel.load(Ordering::Relaxed) || abandoned {
            if let Some(mark) = turn.take() {
                if !matches!(pending.front(), Some(WorkerMsg::Reset(_))) {
                    mark.restore(&mut provider, history);
                }
            }
            if pending.is_empty() { images.clear(); }
            break;
        }
        if !retry {
            break;
        }
        }
    }
}

/// How long a turn may go without any event from the hub (a status change,
/// text, the end) before the person is told and can send again. Not a limit
/// on work: a loading model and a long prefill report progress all along, and
/// a turn waiting in the hub's queue (behind a long image or music job) is
/// not stalled.
const STALL: Duration = Duration::from_secs(60);

/// How often a turn the desktop's GPU use refused asks again.
const UI_PRIORITY_RETRY: Duration = Duration::from_secs(5);

/// A node's "not enough GPU memory ..., UI has priority" (`gpu_yield`).
fn is_ui_priority_refusal(error: &str) -> bool {
    error.contains("UI has priority")
}

/// Tokens kept free for the answer when a conversation is fitted to a node.
const ANSWER_ROOM: usize = 4096;

/// `need` and `max_context` of a node's context-overflow refusal
/// ("session context overflow: need 37934 tokens, max_context is 32768").
pub(crate) fn context_overflow(error: &str) -> Option<(usize, usize)> {
    let rest = &error[error.find("context overflow")?..];
    let number_after = |key: &str| -> Option<usize> {
        let tail = &rest[rest.find(key)? + key.len()..];
        tail.trim_start().split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
    };
    Some((number_after("need")?, number_after("max_context is")?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_strict_json::{self as json, Value};
    use crate::providers::qwen::FleetError;

    const NEVER_POLL: Duration = Duration::from_secs(3600);

    #[test]
    fn the_tool_check_wants_a_well_formed_call() {
        assert!(calls_the_check_tool("<tool_call>{\"name\": \"add\", \"arguments\": {\"a\": 17, \"b\": 25}}</tool_call>"));
        assert!(calls_the_check_tool("<think>\nadd them\n</think>\n<tool_call>\n<function=add>\n<parameter=a>\n17\n</parameter>\n<parameter=b>\n25\n</parameter>\n</function>\n</tool_call>"));
        // What the broken 9B wrote.
        assert!(!calls_the_check_tool("I'll make a new scorecard for each part of the next step"));
        assert!(!calls_the_check_tool("17 + 25 is 42."));
        assert!(!calls_the_check_tool("<tool_call>{\"name\": \"other\", \"arguments\": {}}</tool_call>"));
    }

    #[test]
    fn a_gpu_refusal_is_told_apart_from_a_failure() {
        assert!(is_ui_priority_refusal("qwen3.8-27b on 192.0.2.39: model unavailable: not enough GPU memory for qwen3.8-27b, UI has priority: it needs 17994 MB, 19198 MB are free and the UI keeps 2048 MB"));
        assert!(!is_ui_priority_refusal("empty completion"));
    }

    #[test]
    fn a_context_overflow_elides_the_oldest_tool_results_but_the_newest_two() {
        let error = "qwen3.8-27b: backend error: llm: prefill: Format(\"session context overflow: need 37934 tokens, max_context is 32768\")";
        assert_eq!(context_overflow(error), Some((37934, 32768)));
        assert_eq!(context_overflow("timeout"), None);
        let big = "x".repeat(30_000);
        let mut history = vec![ChatMessage::new(ChatRole::User, "change it")];
        for _ in 0..4 {
            history.push(ChatMessage::new(ChatRole::Assistant, "<tool_call>…</tool_call>"));
            history.push(ChatMessage::new(ChatRole::Tool, big.clone()));
        }
        assert!(crate::chat_shorten::shorten_history(&mut history, 37934, 32768, |_, _| {}));
        let elided: Vec<bool> = history.iter().filter(|m| m.role == ChatRole::Tool).map(|m| m.text.starts_with("[an earlier tool result")).collect();
        // 11 712 tokens to free: one 10 000-token result is not enough, two are.
        assert_eq!(elided, vec![true, true, false, false]);
        assert_eq!(history[0].text, "change it");
        // Only the newest two results left: nothing to elide.
        let mut short = vec![ChatMessage::new(ChatRole::Tool, big.clone()), ChatMessage::new(ChatRole::Tool, big)];
        assert!(!crate::chat_shorten::shorten_history(&mut short, 40000, 32768, |_, _| {}));
    }

    #[test]
    fn the_nodes_own_overflow_refusal_shortens_the_conversation_and_retries() {
        // What a node answers today when a prompt outgrows its window: refused
        // before prefill, in its own words rather than a session's "need N".
        let refusal = "bad request: context overflow: the prompt is 32677 tokens and the answer needs 128 more, but this node's context is 32768 tokens (shorten the conversation, or POST /llm/context to raise it)";
        let big = "x".repeat(30_000);
        let mut history = Vec::new();
        for _ in 0..4 {
            history.push(ChatMessage::new(ChatRole::Assistant, "<tool_call>…</tool_call>"));
            history.push(ChatMessage::new(ChatRole::Tool, big.clone()));
        }
        let node = Scripted::default();
        node.job_errors.lock().unwrap().push_back(refusal.into());
        let (events, generates, _) = run_scripted_lines(node, &["fits now\n</think>\n\nDone."], history, &["hello"]);
        assert!(!events.iter().any(|e| matches!(e, ChatEvent::Failed(_))), "the refusal must not end the turn: {events:?}");
        assert!(
            events.iter().any(|e| matches!(e, ChatEvent::Loading { phase, .. } if phase == "shortening the conversation")),
            "the person must see the conversation being shortened"
        );
        assert_eq!(generates.len(), 2, "the refused line must be sent once more, shortened");
        let elided = |i: usize| generates[i].to_json().contains("[an earlier tool result");
        assert!(!elided(0) && elided(1), "the retry must carry the oldest tool results elided");
    }

    #[test]
    fn dropping_a_session_signals_cancellation_before_worker_disconnect() {
        let (to_worker, msg_rx) = channel();
        let (_event_tx, from_worker) = channel();
        let cancel = Arc::new(TurnStops::default());
        let session = HubChatSession {
            awaiting_reply: Default::default(), generation: Default::default(), wake: None, to_worker, from_worker, cancel: cancel.clone(),
            route: Arc::new(Mutex::new(None)), overflow: Arc::default(),
        };
        drop(session);
        assert!(cancel.current().load(Ordering::Relaxed));
        assert!(matches!(msg_rx.try_recv(), Err(TryRecvError::Disconnected)));
    }

    #[test]
    fn a_retry_after_worker_exit_receives_one_failure() {
        let (to_worker, commands) = channel();
        let (events, from_worker) = channel();
        let session = HubChatSession { awaiting_reply: Default::default(), generation: Default::default(), wake: None, to_worker, from_worker, cancel: Arc::default(), route: Arc::default(), overflow: Arc::default() };
        session.send_user_turn("first".into());
        events.send(TaggedChatEvent { generation: 0, event: ChatEvent::Failed("load failed".into()) }).unwrap();
        drop(events);
        drop(commands);
        assert!(matches!(session.poll().as_slice(), [ChatEvent::Failed(why)] if why == "load failed"),
            "a worker's last diagnostic must reach the chat exactly once");
        session.send_user_turn("retry".into());
        assert!(matches!(session.poll().as_slice(), [ChatEvent::Failed(_)]),
            "sending to the exited worker must return this line instead of leaving it waiting");
        assert!(session.poll().is_empty(), "an idle poll must not return the failed line twice");
    }

    #[test]
    fn the_election_key_is_the_file_basename() {
        let mut config = LocalLlmConfig::new("local/models/Qwen3.5-9B-UD-Q4_K_XL.gguf".into());
        assert_eq!(election_key(&config), "qwen3.5-9b-ud-q4_k_xl.gguf");
        config.model = "".into();
        assert_eq!(election_key(&config), "unknown-model");
    }

    #[test]
    fn routes_read_as_one_phrase() {
        assert_eq!(
            ChatRoute::Fleet { base: "http://192.0.2.76:8123".into(), model: "qwen3.8-27b".into() }.label(),
            "qwen3.8-27b on 192.0.2.76"
        );
        assert_eq!(ChatRoute::MachineNode { port: 8123 }.label(), "this machine's node :8123");
        assert_eq!(ChatRoute::InProcess { model: "x.gguf".into() }.label(), "in-process");
    }

    #[test]
    fn fleet_bases_are_the_chat_roles_sorted_once() {
        use crate::discovery::DiscoveredNode;
        let node = |base: &str| DiscoveredNode { base_url: base.into(), node_id: 1, fleet: "gen".into() };
        let bases = fleet_chat_bases(&[
            node("http://192.0.2.9:8123"),
            node("http://192.0.2.76:8123"),
            node("http://192.0.2.76:8123"),
        ]);
        // The default roles restrict .76 TO chat and leave others open.
        assert_eq!(bases, vec!["http://192.0.2.76:8123", "http://192.0.2.9:8123"]);
    }

    #[test]
    fn the_no_model_message_says_where_a_model_goes() {
        let text = no_model_message(Path::new(""));
        assert!(text.contains("No local AI model is installed"), "{text}");
        assert!(text.contains(&crate::home::weights_dir().display().to_string()), "{text}");
        assert!(!text.contains("fleet"), "{text}");
        let text = no_model_message(Path::new("/nowhere/m.gguf"));
        assert!(text.contains("/nowhere/m.gguf is missing"), "{text}");
    }

    /// A scripted node: records every `/generate` body, answers job polls
    /// with the next scripted raw reply. Shared across threads.
    #[derive(Clone, Default)]
    struct Scripted {
        clock: Option<Arc<Mutex<Instant>>>,
        submit_errors: Arc<Mutex<VecDeque<FleetError>>>,
        poll_errors: Arc<Mutex<VecDeque<FleetError>>>,
        job_error: Option<&'static str>,
        /// Job errors answered before the scripted replies, one per poll.
        job_errors: Arc<Mutex<VecDeque<String>>>,
        models: Option<Value>,
        gets: Arc<Mutex<Vec<String>>>,
        generates: Arc<Mutex<Vec<Value>>>,
        cancels: Arc<Mutex<Vec<String>>>,
        post_order: Arc<Mutex<Vec<String>>>,
        replies: Arc<Mutex<VecDeque<String>>>,
        windows: Arc<Mutex<VecDeque<i64>>>,
        partials: Arc<Mutex<VecDeque<(String, String)>>>,
        think_open: Arc<Mutex<Option<bool>>>,
    }

    impl FleetTransport for Scripted {
        fn now(&self) -> Instant {
            self.clock.as_ref().map_or_else(Instant::now, |clock| *clock.lock().unwrap())
        }
        fn post_json(&mut self, url: &str, body: &Value) -> Result<Value, String> {
            self.post_json_detailed(url, body).map_err(|e| e.to_string())
        }
        fn post_json_detailed(&mut self, url: &str, body: &Value) -> Result<Value, FleetError> {
            if let Some(job_url) = url.strip_suffix("/cancel") {
                let job = job_url.rsplit('/').next().unwrap();
                self.cancels.lock().unwrap().push(url.into());
                self.post_order.lock().unwrap().push(format!("cancel:{job}"));
                return Ok(Value::Obj(Vec::new()));
            }
            assert!(url.ends_with("/generate"), "{url}");
            let mut g = self.generates.lock().unwrap();
            g.push(body.clone());
            self.post_order.lock().unwrap().push(format!("generate:j{}", g.len()));
            if let Some(error) = self.submit_errors.lock().unwrap().pop_front() { return Err(error); }
            let mut fields = vec![("job_id", json::s(format!("j{}", g.len())))];
            if let Some(think_open) = *self.think_open.lock().unwrap() {
                fields.push(("think_open", Value::Bool(think_open)));
            }
            Ok(json::obj(fields))
        }
        fn get_json(&mut self, url: &str) -> Result<Value, String> {
            self.get_json_detailed(url).map_err(|e| e.to_string())
        }
        fn get_json_detailed(&mut self, url: &str) -> Result<Value, FleetError> {
            self.gets.lock().unwrap().push(url.into());
            if url.ends_with("/health") {
                return Ok(json::obj(vec![("capabilities", Value::Arr(vec![json::s("chat")]))]));
            }
            if url.ends_with("/models") {
                if let Some(models) = &self.models { return Ok(models.clone()); }
                return Ok(json::obj(vec![(
                    "models",
                    Value::Arr(vec![json::obj(vec![
                        ("id", json::s("qwen3.8-27b")),
                        ("domain", json::s("chat")),
                        ("available", Value::Bool(true)),
                        ("state", json::s("loaded")),
                    ])]),
                )]));
            }
            if let Some(error) = self.poll_errors.lock().unwrap().pop_front() { return Err(error); }
            let mut partials = self.partials.lock().unwrap();
            if partials.front().is_some_and(|(path, _)| url.ends_with(path)) {
                let (_, text) = partials.pop_front().unwrap();
                return Ok(json::obj(vec![("state", json::s("running")), ("partial_text", json::s(text))]));
            }
            if let Some(error) = self.job_errors.lock().unwrap().pop_front() {
                return Ok(json::obj(vec![("state", json::s("error")), ("error", json::s(error))]));
            }
            let raw = self.replies.lock().unwrap().pop_front();
            match raw {
                Some(raw) => {
                    let mut fields = vec![("state", json::s("done")), ("partial_text", json::s(raw)), ("text", json::s(""))];
                    if let Some(window) = self.windows.lock().unwrap().pop_front() {
                        fields.push(("serving", json::obj(vec![("measure", json::obj(vec![("window", Value::Int(window))]))])));
                    }
                    Ok(json::obj(fields))
                }
                None => Ok(json::obj(vec![("state", json::s("error")), ("error", json::s(self.job_error.unwrap_or("node went away")))])),
            }
        }
    }

    fn tools() -> Vec<ToolSpec> {
        vec![ToolSpec::new(
            "os.launch",
            "Start an app.",
            r#"{"type":"object","properties":{"app":{"type":"string"}},"required":["app"]}"#,
        )]
    }

    fn collect_until_turn_done(rx: &Receiver<TaggedChatEvent>) -> Vec<ChatEvent> {
        let mut out = Vec::new();
        for TaggedChatEvent { event: ev, .. } in rx {
            let done = matches!(ev, ChatEvent::TurnDone { .. } | ChatEvent::Failed(_));
            out.push(ev);
            if done { return out; }
        }
        panic!("proxy disconnected before a terminal event: {out:?}");
    }

    #[test]
    fn a_proxied_conversation_carries_the_tools_and_the_results() {
        let node = Scripted::default();
        node.replies.lock().unwrap().push_back(
            "<think>which app?</think>\n\n<tool_call>\n<function=os.launch>\n<parameter=app>\nterminal\n</parameter>\n</function>\n</tool_call>".into(),
        );
        node.replies.lock().unwrap().push_back("<think>done</think>\n\nOpened the terminal.".into());
        let provider = FleetQwenChatProvider::new(node.clone(), vec!["http://192.0.2.76:8123".into()]);
        let config = HubChatConfig {
            reach: ChatReach::Lan,
            llm: LocalLlmConfig::new("".into()),
            preferred_model: None,
            claude_model: None,
            max_tokens: Some(u32::MAX),
            thinking: None,
            system_prompt: "You run the desktop.".into(),
            tools: tools(),
            wake: None,
        };
        let (to_worker, msg_rx) = channel();
        let (event_tx, events) = channel();
        let next_node = node.clone();
        let worker = std::thread::spawn(move || {
            let generation = AtomicU64::new(0);
            let send = |event| {
                let _ = event_tx.send(TaggedChatEvent { generation: generation.load(Ordering::Relaxed), event });
            };
            let route = ChatRoute::Fleet { base: "http://192.0.2.76:8123".into(), model: "qwen3.8-27b".into() };
            let mut history = Vec::new();
            let mut images = Vec::new();
            let mut pending = VecDeque::new();
            let ProxyExit::Reelect = run_proxy(provider, &config, &mut pending, &msg_rx, &mut images, &mut history, &generation, &send, route.clone(), &Mutex::default(), POLL)
                else { panic!("a refused line must allow a new election"); };
            let provider = FleetQwenChatProvider::new(next_node, vec!["http://192.0.2.76:8123".into()]);
            run_proxy(provider, &config, &mut pending, &msg_rx, &mut images, &mut history, &generation, &send, route, &Mutex::default(), POLL)
        });
        to_worker.send(WorkerMsg::UserTurn("open the terminal".into(), Arc::default())).unwrap();
        let first = collect_until_turn_done(&events);
        assert!(first.iter().any(|e| matches!(e, ChatEvent::Ready { .. })));
        let call = first.iter().find_map(|e| match e {
            ChatEvent::ToolCall { name, args } => Some((name.clone(), args.clone())),
            _ => None,
        });
        assert_eq!(call, Some(("os.launch".to_string(), vec![("app".to_string(), "terminal".to_string())])));
        assert!(matches!(first.last(), Some(ChatEvent::TurnDone { tool_calls: 1, .. })), "{first:?}");
        let visible: String = first
            .iter()
            .filter_map(|e| match e {
                ChatEvent::Delta(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert!(!visible.contains("which app") && !visible.contains("tool_call"), "{visible:?}");

        // The result goes back; the model answers in prose.
        to_worker.send(WorkerMsg::ToolResults(vec![("launched terminal".into(), false)], Arc::default())).unwrap();
        let second = collect_until_turn_done(&events);
        let visible: String = second
            .iter()
            .filter_map(|e| match e {
                ChatEvent::Delta(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert!(visible.contains("Opened the terminal."), "{visible:?}");
        assert!(matches!(second.last(), Some(ChatEvent::TurnDone { tool_calls: 0, .. })));

        // What the node saw: the tool pack in the system text, the result
        // as a `<tool_response>` user turn.
        let generates = node.generates.lock().unwrap();
        assert_eq!(generates.len(), 2);
        let system = generates[0].get("chat_system").and_then(Value::as_str).unwrap();
        assert!(system.contains("<tools>") && system.contains("\"os.launch\"") && system.contains("You run the desktop."), "{system}");
        assert!(!system.contains("<|im_start|>"), "the node wraps the template itself");
        let messages = generates[1].get("chat_messages").and_then(Value::as_arr).unwrap();
        let tool_turn = messages
            .iter()
            .find(|m| m.get("text").and_then(Value::as_str).map(|t| t.contains("launched terminal")).unwrap_or(false))
            .expect("the tool result reached the node");
        assert_eq!(tool_turn.get("role").and_then(Value::as_str), Some("user"));
        assert!(tool_turn.get("text").and_then(Value::as_str).unwrap().starts_with("<tool_response>"));
        drop(generates);

        // The node dies on the next turn: an honest failure naming it, then
        // the person's next line re-elects.
        node.partials.lock().unwrap().push_back(("/job/j3".into(), "<think>checking</think>partial answer".into()));
        to_worker.send(WorkerMsg::UserTurn("and now?".into(), Arc::default())).unwrap();
        let third = collect_until_turn_done(&events);
        match third.last() {
            Some(ChatEvent::Failed(text)) => assert!(text.contains("qwen3.8-27b on 192.0.2.76"), "{text}"),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(third.iter().any(|e| matches!(e, ChatEvent::Delta(s) if s.contains("partial answer"))),
            "the refused turn must have streamed before its node failed");
        node.replies.lock().unwrap().push_back("<think>done</think>back online".into());
        to_worker.send(WorkerMsg::UserTurn("hello again".into(), Arc::default())).unwrap();
        assert!(matches!(collect_until_turn_done(&events).last(), Some(ChatEvent::TurnDone { .. })),
            "the re-elected node must answer the next line");
        drop(to_worker);
        assert!(matches!(worker.join().unwrap(), ProxyExit::Ended), "the proxy must finish on disconnect");
        let requests = node.generates.lock().unwrap();
        let next = requests.last().unwrap().get("chat_messages").unwrap().to_json();
        assert!(next.contains("open the terminal") && next.contains("launched terminal") && next.contains("hello again"),
            "the re-elected node must receive the completed tool round and the new line");
        assert!(!next.contains("and now?") && !next.contains("partial answer"),
            "the refused line and its partial reply must be absent from the actual next request");
    }

    #[test]
    fn stopped_and_abandoned_turns_leave_the_nodes_next_request() {
        let node = Scripted::default();
        let call = r#"<tool_call>{"name":"os.launch","arguments":{"app":"viewer"}}</tool_call>"#;
        node.replies.lock().unwrap().extend([
            call.into(),
            "first answer".into(),
            call.into(),
            "last answer".into(),
        ]);
        node.partials.lock().unwrap().extend([
            ("/job/j4".into(), "partial tool continuation".into()),
            ("/job/j5".into(), "partial retyped answer".into()),
        ]);
        let base = "http://192.0.2.76:8123";
        let provider = FleetQwenChatProvider::new(node.clone(), vec![base.into()]).with_thinking(Some(false));
        let config = HubChatConfig {
            reach: ChatReach::Lan, llm: LocalLlmConfig::new("".into()), preferred_model: None,
            claude_model: None, max_tokens: None, thinking: Some(false),
            system_prompt: "Look at pictures.".into(), tools: tools(), wake: None,
        };
        let (to_worker, msg_rx) = channel();
        let (event_tx, from_worker) = channel();
        let chat = HubChatSession { awaiting_reply: Default::default(), generation: Default::default(), wake: None, to_worker, from_worker, cancel: Arc::default(), route: Arc::default(), overflow: Arc::default() };
        let (release, gate) = channel();
        let worker = std::thread::spawn(move || {
            let generation = AtomicU64::new(0);
            let send = |event| {
                let hold = matches!(&event, ChatEvent::Delta(text) if text.starts_with("partial "));
                let _ = event_tx.send(TaggedChatEvent { generation: generation.load(Ordering::Relaxed), event });
                if hold { let _ = gate.recv(); }
            };
            let route = ChatRoute::Fleet { base: base.into(), model: "qwen3.8-27b".into() };
            run_proxy(provider, &config, &mut VecDeque::new(), &msg_rx, &mut Vec::new(), &mut Vec::new(), &generation, &send, route, &Mutex::default(), POLL)
        });
        let picture = |red: u8| {
            let png = crate::testpattern::encode_png_rgb8(&[red, 0, 0], 1, 1).unwrap();
            ToolImage { label: format!("picture {red}"), png: png.into() }
        };
        // Waits for the expected point and returns any other answer text that reached the chat first.
        let wait = |expected: &str| {
            let mut other = Vec::new();
            loop {
                match chat.from_worker.recv().expect("the proxy must reach the scripted event").event {
                    ChatEvent::Delta(text) if text.starts_with("partial ") => {
                        assert_eq!(text, expected, "the worker must hold the expected open reply");
                        return other;
                    }
                    ChatEvent::Delta(text) => other.push(text),
                    ChatEvent::TurnDone { tool_calls, .. } => {
                        assert_eq!(if tool_calls == 0 { "done" } else { "tool" }, expected, "the worker must complete the expected round");
                        return other;
                    }
                    ChatEvent::Failed(error) => panic!("unexpected proxy failure: {error}"),
                    _ => {}
                }
            }
        };
        chat.attach_images((0..4).map(picture).collect()).unwrap();
        chat.send_user_turn("keep this".into());
        wait("tool");
        chat.attach_images((4..8).map(picture).collect()).unwrap();
        chat.send_tool_results(vec![("completed result".into(), false)]);
        wait("done");
        chat.attach_images(vec![picture(8)]).unwrap();
        chat.send_user_turn("describe this".into());
        wait("tool");
        chat.send_tool_results(vec![("stopped result".into(), false)]);
        wait("partial tool continuation");
        let old_signal = chat.cancel.current();
        chat.cancel();
        chat.send_user_turn("describe this".into());
        assert!(old_signal.load(Ordering::Relaxed), "an immediate retype must leave the running worker's Stop observable");
        release.send(()).unwrap();
        let after_stop = wait("partial retyped answer");
        assert!(after_stop.is_empty(), "the stopped turn's answer must never reach the chat: {after_stop:?}");
        chat.send_user_turn("final question".into());
        release.send(()).unwrap();
        wait("done");
        drop(chat);
        worker.join().unwrap();

        let sent = node.generates.lock().unwrap();
        assert_eq!(sent.len(), 6, "each scripted round must reach the node once");
        let rows = |i: usize| sent[i].get("chat_messages").unwrap().as_arr().unwrap();
        let first = rows(1);
        let retyped = rows(4);
        assert!(first[0].get("images").is_some(), "the completed picture must have reached the node");
        assert!(rows(2)[0].get("images").is_none(), "the ninth picture must evict the first row during the stopped turn");
        assert!(rows(2).last().unwrap().get("images").is_some(), "the stopped picture must have reached the node before Stop");
        assert_eq!(&retyped[..3], first, "all completed pictures, including the evicted row, must be restored byte for byte");
        assert_eq!(retyped.len(), 5, "the stopped user, assistant tool call and tool result must leave the next request");
        assert_eq!(retyped[4].get("text").and_then(Value::as_str), Some("describe this"), "the retyped line must carry only its own text");
        assert!(retyped[4].get("images").is_none(), "a retype must not inherit the stopped picture");
        let last = rows(5);
        assert_eq!(&last[..4], &retyped[..4], "abandoning a turn must preserve the completed wire prefix");
        assert_eq!(last.len(), 5, "the abandoned line must leave the node's next request");
        assert_eq!(last[4].get("text").and_then(Value::as_str), Some("final question"), "the newest line must reach the node");
    }

    fn run_scripted_turn(node: Scripted, replies: &[&str]) -> (Vec<ChatEvent>, Vec<Value>) {
        let (events, generates, _) = run_scripted_lines(node, replies, Vec::new(), &["hello"]);
        (events, generates)
    }

    fn scripted_config() -> HubChatConfig {
        HubChatConfig {
            reach: ChatReach::Lan, llm: LocalLlmConfig::new("".into()), preferred_model: None,
            claude_model: None, max_tokens: Some(u32::MAX), thinking: None,
            system_prompt: "Answer directly.".into(), tools: Vec::new(), wake: None,
        }
    }

    struct ScriptedRun {
        pending: VecDeque<WorkerMsg>,
        events: Vec<ChatEvent>,
        generates: Vec<Value>,
        history: Vec<ChatMessage>,
        overflow: Option<ContextOverflow>,
        exit: ProxyExit,
    }

    fn scripted_session(route: Option<ChatRoute>) -> (HubChatSession, Receiver<WorkerMsg>, Sender<TaggedChatEvent>) {
        let (to_worker, msg_rx) = channel();
        let (event_tx, from_worker) = channel();
        let chat = HubChatSession {
            awaiting_reply: Default::default(), generation: Default::default(), wake: None, to_worker, from_worker,
            cancel: Arc::default(), route: Arc::new(Mutex::new(route)), overflow: Arc::default(),
        };
        (chat, msg_rx, event_tx)
    }

    fn scripted_provider(node: Scripted, config: &HubChatConfig) -> (FleetQwenChatProvider<Scripted>, ChatRoute) {
        let (base, route) = match config.reach {
            ChatReach::Machine => ("http://127.0.0.1:8123", ChatRoute::MachineNode { port: 8123 }),
            ChatReach::Lan => {
                let base = "http://192.0.2.1:8123";
                (base, ChatRoute::Fleet { base: base.into(), model: "qwen3.8-27b".into() })
            }
        };
        let provider = FleetQwenChatProvider::for_reach(node, vec![base.into()], config.reach)
            .with_max_tokens(config.max_tokens).with_thinking(config.thinking);
        (provider, route)
    }

    fn scripted_proxy(
        node: Scripted, config: HubChatConfig, history: Vec<ChatMessage>,
        interact: impl FnOnce(&HubChatSession) -> Vec<ChatEvent>,
    ) -> ScriptedRun {
        scripted_proxy_with_poll(node, config, history, POLL, interact)
    }

    fn scripted_proxy_with_poll(
        node: Scripted, config: HubChatConfig, mut history: Vec<ChatMessage>, poll: Duration,
        interact: impl FnOnce(&HubChatSession) -> Vec<ChatEvent>,
    ) -> ScriptedRun {
        let (provider, route) = scripted_provider(node.clone(), &config);
        let (chat, msg_rx, event_tx) = scripted_session(Some(route.clone()));
        let overflow = chat.overflow.clone();
        let worker = std::thread::spawn(move || {
            let generation = AtomicU64::new(0);
            let send = |event| { let _ = event_tx.send(TaggedChatEvent { generation: generation.load(Ordering::Relaxed), event }); };
            let mut pending = VecDeque::new();
            let exit = run_proxy(provider, &config, &mut pending, &msg_rx, &mut Vec::new(), &mut history, &generation, &send, route, &overflow, poll);
            (exit, history, pending)
        });
        let events = interact(&chat);
        let overflow = chat.take_overflow();
        drop(chat);
        let (exit, history, pending) = worker.join().unwrap();
        let generates = node.generates.lock().unwrap().clone();
        ScriptedRun { events, generates, history, overflow, exit, pending }
    }

    /// Join before dropping the session so its Drop cannot cancel the user line
    /// the proxy is about to return for re-election.
    fn scripted_proxy_until_exit(
        node: Scripted, config: HubChatConfig,
        hold: impl Fn(&ChatEvent) -> bool + Send + 'static,
        interact: impl FnOnce(&HubChatSession) -> Vec<ChatEvent>,
    ) -> ScriptedRun {
        let (provider, route) = scripted_provider(node.clone(), &config);
        let (chat, msg_rx, event_tx) = scripted_session(Some(route.clone()));
        let overflow = chat.overflow.clone();
        let (release, gate) = channel::<()>();
        let worker = std::thread::spawn(move || {
            let generation = AtomicU64::new(0);
            let send = |event| {
                let held = hold(&event);
                let _ = event_tx.send(TaggedChatEvent { generation: generation.load(Ordering::Relaxed), event });
                if held { let _ = gate.recv(); }
            };
            let mut history = Vec::new();
            let mut pending = VecDeque::new();
            let exit = run_proxy(provider, &config, &mut pending, &msg_rx, &mut Vec::new(), &mut history,
                &generation, &send, route, &overflow, POLL);
            (exit, history, pending)
        });
        let events = interact(&chat);
        let overflow = chat.take_overflow();
        drop(release);
        let (exit, history, pending) = worker.join().unwrap();
        drop(chat);
        let generates = node.generates.lock().unwrap().clone();
        ScriptedRun { events, generates, history, overflow, exit, pending }
    }

    fn scripted_line(chat: &HubChatSession, text: &str) -> Vec<ChatEvent> {
        chat.send_user_turn(text.into());
        collect_until_turn_done(&chat.from_worker)
    }

    impl Scripted {
        fn probes(&self) -> usize {
            self.gets.lock().unwrap().iter().filter(|url| url.ends_with("/health") || url.ends_with("/models")).count()
        }
    }

    fn assert_only_prompt(body: &Value, prompt: &str) {
        let rows = body.get("chat_messages").and_then(Value::as_arr).unwrap();
        assert_eq!(rows.len(), 1, "a reset must remove all earlier turns");
        assert_eq!(rows[0].get("text").and_then(Value::as_str), Some(prompt));
        assert!(rows[0].get("images").is_none(), "a reset must remove queued and remembered images");
    }

    #[test]
    fn reset_history_cancels_in_flight_filters_stale_events_and_keeps_route() {
        let node = Scripted::default();
        node.partials.lock().unwrap().push_back(("/job/j1".into(), "partial old".into()));
        node.replies.lock().unwrap().push_back("fresh answer".into());
        let config = HubChatConfig { thinking: Some(false), ..scripted_config() };
        let (provider, route) = scripted_provider(node.clone(), &config);
        let (chat, input, events) = scripted_session(Some(route.clone()));
        let overflow = chat.overflow.clone();
        let (release, gate) = channel();
        let (finished, done) = channel();
        let worker = std::thread::spawn(move || {
            let generation = AtomicU64::new(0);
            let send = |event| {
                let partial = matches!(&event, ChatEvent::Delta(s) if s == "partial old");
                let terminal = matches!(event, ChatEvent::TurnDone { .. } | ChatEvent::Failed(_));
                let _ = events.send(TaggedChatEvent { generation: generation.load(Ordering::Relaxed), event });
                if partial { let _ = gate.recv(); }
                if terminal { let _ = finished.send(()); }
            };
            run_proxy(provider, &config, &mut VecDeque::new(), &input, &mut Vec::new(), &mut Vec::new(),
                &generation, &send, route, &overflow, NEVER_POLL)
        });
        chat.send_user_turn("old prompt".into());
        loop {
            match chat.from_worker.recv().unwrap().event {
                ChatEvent::Ready { .. } | ChatEvent::Loading { .. } => {}
                ChatEvent::Delta(s) if s == "partial old" => break,
                other => panic!("expected the open stream, got {other:?}"),
            }
        }
        let elected = chat.route();
        assert!(matches!(elected, Some(ChatRoute::Fleet { .. })));
        let probes = node.probes();
        chat.reset_history();
        assert_eq!(chat.route(), elected);
        chat.send_user_turn("fresh prompt".into());
        release.send(()).unwrap();
        done.recv().unwrap();
        let fresh = chat.poll();
        assert!(matches!(fresh.as_slice(), [ChatEvent::Delta(s), ChatEvent::TurnDone { tool_calls: 0, .. }] if s == "fresh answer"), "{fresh:?}");
        assert_eq!(chat.route(), elected);
        drop(chat);
        assert!(matches!(worker.join().unwrap(), ProxyExit::Ended));
        assert_eq!(node.probes(), probes);
        assert_eq!(node.cancels.lock().unwrap().as_slice(), ["http://192.0.2.1:8123/job/j1/cancel"]);
        assert_eq!(node.post_order.lock().unwrap().as_slice(), ["generate:j1", "cancel:j1", "generate:j2"]);
        let bodies = node.generates.lock().unwrap();
        assert_ne!(bodies[0].get("chat_session"), bodies[1].get("chat_session"));
        assert_only_prompt(&bodies[1], "fresh prompt");
    }

    #[test]
    fn reset_history_while_turn_done_already_queued_never_leaks_into_next_turn() {
        let (chat, _input, tx) = scripted_session(None);
        let send = |generation, event| tx.send(TaggedChatEvent { generation, event }).unwrap();
        let done = || ChatEvent::TurnDone { tool_calls: 0, tokens: 1, secs: 0.0, context_used: 1, context_max: 2 };
        chat.send_user_turn("old".into());
        *chat.overflow.lock().unwrap() = Some(ContextOverflow { need: 3, max: 2 });
        send(0, ChatEvent::Delta("old".into()));
        send(0, done());
        send(0, ChatEvent::Failed("old failure".into()));
        chat.reset_history();
        assert!(chat.take_overflow().is_none());
        assert!(matches!(chat.from_worker.try_recv(), Err(TryRecvError::Empty)), "reset must drain already queued events");
        chat.send_user_turn("new".into());
        send(0, done());
        send(1, ChatEvent::Delta("fresh".into()));
        send(1, done());
        assert!(matches!(chat.poll().as_slice(), [ChatEvent::Delta(s), ChatEvent::TurnDone { .. }] if s == "fresh"));

        chat.send_user_turn("still owed an answer".into());
        send(0, done());
        drop(tx);
        assert!(matches!(chat.poll().as_slice(), [ChatEvent::Failed(_)]), "a stale terminal event must leave the disconnect failure owed");
        assert!(chat.poll().is_empty());
    }

    #[test]
    fn reset_history_on_open_tool_round_and_idle_session_clears_history_without_reelect() {
        let node = Scripted::default();
        node.replies.lock().unwrap().extend([
            r#"<tool_call>{"name":"doc.read","arguments":{}}</tool_call>"#.into(), "second".into(), "third".into(),
        ]);
        let config = HubChatConfig { tools: tools(), thinking: Some(false), ..scripted_config() };
        let run = scripted_proxy(node.clone(), config, Vec::new(), |chat| {
            assert!(matches!(scripted_line(chat, "request a tool").last(), Some(ChatEvent::TurnDone { tool_calls: 1, .. })));
            let probes = node.probes();
            let png = crate::testpattern::encode_png_rgb8(&[1, 2, 3], 1, 1).unwrap();
            chat.attach_images(vec![ToolImage { label: "tool result".into(), png: png.into() }]).unwrap();
            chat.reset_history();
            chat.send_tool_results(vec![("stale result".into(), false)]);
            let second = scripted_line(chat, "after tool reset");
            assert!(matches!(second.as_slice(), [ChatEvent::Delta(s), ChatEvent::TurnDone { tool_calls: 0, .. }] if s == "second"), "{second:?}");
            chat.reset_history();
            let third = scripted_line(chat, "after idle reset");
            assert!(matches!(third.as_slice(), [ChatEvent::Delta(s), ChatEvent::TurnDone { tool_calls: 0, .. }] if s == "third"), "{third:?}");
            assert_eq!(node.probes(), probes);
            third
        });
        assert!(matches!(run.exit, ProxyExit::Ended));
        assert!(node.cancels.lock().unwrap().is_empty());
        assert_eq!(run.generates.len(), 3);
        let sessions: std::collections::HashSet<_> = run.generates.iter().map(|body| body.get("chat_session").and_then(Value::as_str).unwrap()).collect();
        assert_eq!(sessions.len(), 3, "both resets must rotate the session");
        assert_only_prompt(&run.generates[1], "after tool reset");
        assert_only_prompt(&run.generates[2], "after idle reset");
    }

    #[test]
    fn reset_history_during_election_wait_and_failure_delivers_new_turn_cleanly() {
        for queued_fresh in [true, false] {
            let (chat, input, _events) = scripted_session(None);
            let initial_stop = chat.cancel.current();
            chat.send_user_turn("A".into());
            chat.reset_history();
            if queued_fresh {
                chat.send_user_turn("B".into());
            } else {
                // A live orphan acts as a FIFO barrier. Its notification lets B
                // arrive after the nonblocking drain, without a timer or race.
                chat.to_worker.send(WorkerMsg::ToolResults(vec![], Arc::default())).unwrap();
            }
            let generation = AtomicU64::new(0);
            let mut pending = VecDeque::new();
            let mut history = vec![ChatMessage::new(ChatRole::User, "earlier")];
            let diagnostics = std::cell::RefCell::new(Vec::new());
            let next = if queued_fresh { "C" } else { "B" };
            let send = |event| {
                let ChatEvent::Failed(reason) = event else { panic!("unexpected event {event:?}") };
                diagnostics.borrow_mut().push((generation.load(Ordering::Relaxed), reason));
                chat.send_user_turn(next.into());
            };
            assert!(after_election_failure(&initial_stop, &mut pending, &input, &mut Vec::new(), &mut history,
                &generation, &chat.overflow, &send, "election failed".into()));
            let expected = if queued_fresh { "election failed" } else { ORPHAN_TOOL_RESULTS };
            assert_eq!(diagnostics.into_inner(), [(1, expected.to_string())]);
            assert!(history.is_empty());
            assert!(matches!(pending.front(), Some(WorkerMsg::UserTurn(line, stop)) if line == next && !stop.load(Ordering::Relaxed)));
        }

        let (commands, input) = channel();
        let (entered, waiting) = channel();
        let worker = std::thread::spawn(move || {
            let mut pending = VecDeque::new();
            let mut images = Vec::new();
            let mut history = Vec::new();
            let generation = AtomicU64::new(0);
            let mut waits = 0usize;
            let result = fleet_provider_with(ChatReach::Lan, &|_| {}, None, None, None, Vec::new(),
                Vec::new, Scripted::default, NEVER_POLL, NEVER_POLL, |duration| {
                    waits += 1;
                    entered.send(()).unwrap();
                    election_wait(duration, &input, &mut pending, &mut images, &mut history, &generation)
                });
            assert_eq!(result.err().as_deref(), Some("chat disconnected"));
            assert_eq!(waits, 1);
        });
        waiting.recv().unwrap();
        drop(commands);
        worker.join().unwrap();
    }

    #[test]
    fn reset_history_after_turn_error_clears_history_before_reelection() {
        let node = Scripted::default();
        let config = HubChatConfig { thinking: Some(false), ..scripted_config() };
        let (provider, route) = scripted_provider(node.clone(), &config);
        let (chat, input, events) = scripted_session(Some(route.clone()));
        let overflow = chat.overflow.clone();
        let reelected = node.clone();
        let worker = std::thread::spawn(move || {
            let generation = AtomicU64::new(0);
            let send = |event| { let _ = events.send(TaggedChatEvent { generation: generation.load(Ordering::Relaxed), event }); };
            let mut pending = VecDeque::new();
            let mut images = Vec::new();
            let mut history = Vec::new();
            let exit = run_proxy(provider, &config, &mut pending, &input, &mut images, &mut history, &generation, &send, route, &overflow, POLL);
            assert!(matches!(exit, ProxyExit::Reelect));
            assert_eq!(generation.load(Ordering::Relaxed), 1);
            assert!(history.is_empty());
            assert!(overflow.lock().unwrap().is_none());
            let (provider, route) = scripted_provider(reelected, &config);
            run_proxy(provider, &config, &mut pending, &input, &mut images, &mut history, &generation, &send, route, &overflow, POLL)
        });
        node.replies.lock().unwrap().push_back("seed answer".into());
        assert!(matches!(scripted_line(&chat, "seed history").last(), Some(ChatEvent::TurnDone { .. })));
        node.job_errors.lock().unwrap().push_back("session context overflow: need 37934 tokens, max_context is 32768".into());
        assert!(matches!(scripted_line(&chat, "too long").last(), Some(ChatEvent::Failed(_))));
        assert_eq!(*chat.overflow.lock().unwrap(), Some(ContextOverflow { need: 37934, max: 32768 }));
        chat.reset_history();
        node.replies.lock().unwrap().push_back("recovered".into());
        chat.send_user_turn("fresh after error".into());
        let mut fresh = Vec::new();
        loop {
            let envelope = chat.from_worker.recv().unwrap();
            assert_eq!(envelope.generation, 1);
            let terminal = matches!(envelope.event, ChatEvent::TurnDone { .. } | ChatEvent::Failed(_));
            fresh.push(envelope.event);
            if terminal { break; }
        }
        assert!(matches!(fresh.as_slice(), [ChatEvent::Ready { .. }, ChatEvent::Delta(s), ChatEvent::TurnDone { .. }] if s == "recovered"), "{fresh:?}");
        assert!(chat.take_overflow().is_none());
        drop(chat);
        assert!(matches!(worker.join().unwrap(), ProxyExit::Ended));
        let bodies = node.generates.lock().unwrap();
        assert_eq!(bodies.len(), 3);
        assert_only_prompt(&bodies[2], "fresh after error");
    }

    #[test]
    fn reset_history_on_machine_reach_busy_wait_cancels_promptly() {
        let node = Scripted {
            clock: Some(Arc::new(Mutex::new(Instant::now()))),
            models: Some(json::parse(br#"{"models":[{"id":"local","domain":"chat","backend":"llm","available":true}]}"#).unwrap()),
            ..Default::default()
        };
        node.submit_errors.lock().unwrap().push_back(FleetError::Http { status: 503, reason: "all lanes busy".into(), no_job: true });
        node.replies.lock().unwrap().push_back("new answer".into());
        let config = HubChatConfig { reach: ChatReach::Machine, thinking: Some(false), ..scripted_config() };
        let run = scripted_proxy_with_poll(node.clone(), config, Vec::new(), NEVER_POLL, |chat| {
            chat.send_user_turn("busy".into());
            loop {
                let envelope = chat.from_worker.recv().unwrap();
                match envelope.event {
                    ChatEvent::Ready { .. } => {}
                    ChatEvent::Loading { phase, .. } if phase == "waiting for this computer's AI Hub" => break,
                    other => panic!("expected machine admission backoff, got {other:?}"),
                }
            }
            chat.reset_history();
            chat.send_user_turn("next".into());
            let delta = chat.from_worker.recv().expect("reset must wake the proxy");
            assert_eq!(delta.generation, 1);
            assert!(matches!(delta.event, ChatEvent::Delta(ref s) if s == "new answer"));
            let done = chat.from_worker.recv().expect("next turn must finish");
            assert_eq!(done.generation, 1);
            assert!(matches!(done.event, ChatEvent::TurnDone { tool_calls: 0, .. }));
            vec![delta.event, done.event]
        });
        assert!(matches!(run.exit, ProxyExit::Ended));
        assert!(node.cancels.lock().unwrap().is_empty());
        assert_eq!(run.generates.len(), 2);
        assert_ne!(run.generates[0].get("chat_session"), run.generates[1].get("chat_session"));
        assert_only_prompt(&run.generates[1], "next");
    }

    /// Send each line after the preceding turn finishes, keeping the same provider and history.
    fn run_scripted_lines(
        node: Scripted, replies: &[&str], history: Vec<ChatMessage>, lines: &[&str],
    ) -> (Vec<ChatEvent>, Vec<Value>, Option<ContextOverflow>) {
        node.replies.lock().unwrap().extend(replies.iter().map(|reply| (*reply).to_string()));
        let run = scripted_proxy(node, scripted_config(), history, |chat| {
            lines.iter().flat_map(|line| scripted_line(chat, line)).collect()
        });
        assert!(matches!(run.exit, ProxyExit::Ended));
        (run.events, run.generates, run.overflow)
    }

    fn run_scripted_closed_think_turn(reply: &str) -> (Vec<ChatEvent>, Vec<Value>) {
        let node = Scripted::default();
        *node.think_open.lock().unwrap() = Some(false);
        node.replies.lock().unwrap().push_back(reply.to_string());
        let provider =
            FleetQwenChatProvider::new(node.clone(), vec!["http://192.0.2.76:8123".into()]);
        let config = HubChatConfig {
            reach: ChatReach::Lan,
            llm: LocalLlmConfig::new("".into()),
            preferred_model: None,
            claude_model: None,
            max_tokens: None,
            thinking: None,
            system_prompt: "Answer directly.".into(),
            tools: Vec::new(),
            wake: None,
        };
        let (to_worker, msg_rx) = channel();
        let (event_tx, events) = channel();
        let worker = std::thread::spawn(move || {
            let generation = AtomicU64::new(0);
            let send = |event| {
                let _ = event_tx.send(TaggedChatEvent { generation: generation.load(Ordering::Relaxed), event });
            };
            run_proxy(
                provider,
                &config,
                &mut VecDeque::new(),
                &msg_rx,
                &mut Vec::new(),
                &mut Vec::new(),
                &generation,
                &send,
                ChatRoute::Fleet {
                    base: "http://192.0.2.76:8123".into(),
                    model: "qwen3.8-27b".into(),
                },
                &Mutex::default(),
                POLL,
            )
        });
        to_worker.send(WorkerMsg::UserTurn("hello".into(), Arc::default())).unwrap();
        let output = collect_until_turn_done(&events);
        drop(to_worker);
        assert!(matches!(worker.join().unwrap(), ProxyExit::Ended));
        let generates = node.generates.lock().unwrap().clone();
        (output, generates)
    }

    fn proposal(slot: &str, body: &str) -> String {
        crate::local_llm::tool_call_markup("doc.edit", &[("slot".into(), slot.into()), ("text".into(), body.into())])
    }

    #[test]
    fn proxy_restore_undoes_trimmed_assistant_calls_and_multi_round_elisions_byte_for_byte() {
        use ChatRole::{Assistant, User, Tool};
        let old = "old content ".repeat(256);
        let pair = format!("{} between {}", proposal("a", &old), proposal("b", &old));
        let mut history = vec![
            ChatMessage::new(Tool, &old), ChatMessage::new(Assistant, &pair),
            ChatMessage::new(Tool, "first ok"), ChatMessage::new(Tool, "second ok"),
            ChatMessage::new(Assistant, format!("{}{}", proposal("a", "updated"), proposal("b", "updated"))),
            ChatMessage::new(Tool, "ok"), ChatMessage::new(Tool, "ok"), ChatMessage::new(User, "finish"),
        ];
        let node = Scripted::default();
        let mut provider = FleetQwenChatProvider::new(node.clone(), vec!["http://192.0.2.1:8123".into()]);
        // An actual raw reply distinguishes the warm wire prefix from cold-rendered history.
        let raw = "reasoning</think>ready";
        node.replies.lock().unwrap().push_back(raw.into());
        provider.begin_turn(&TurnInput::new("instructions", history.clone())).unwrap();
        let reply = provider.poll().into_iter().find_map(|e| match e { ProviderEvent::Done { text } => Some(text), _ => None }).unwrap();
        history.push(ChatMessage::new(Assistant, reply));
        let original = history.clone();
        let mut mark = ProxyTurn { start: history.len(), provider: provider.checkpoint(), elided: Vec::new(), stop: Arc::default() };
        history.push(ChatMessage::new(User, "abandoned"));
        assert!(mark.shorten(&mut history, 101, 125));
        assert_eq!(history[1].text.matches("[trimmed superseded tool call:").count(), 1);
        assert_eq!(history[0].text, old);
        assert!(mark.shorten(&mut history, 20_000, 125));
        assert_eq!(history[1].text.matches("[trimmed superseded tool call:").count(), 2);
        assert!(history[0].text.starts_with("[an earlier tool result"));
        provider.history_pruned(0);
        mark.restore(&mut provider, &mut history);
        assert_eq!(history, original);
        history.push(ChatMessage::new(User, "next"));
        provider.begin_turn(&TurnInput::new("instructions", history)).unwrap();
        let requests = node.generates.lock().unwrap();
        let first = requests[0].get("chat_messages").unwrap().as_arr().unwrap();
        let next = requests[1].get("chat_messages").unwrap().as_arr().unwrap();
        assert_eq!(&next[..first.len()], first);
        assert_eq!(next[first.len()].get("text").and_then(Value::as_str), Some(raw));
        assert_eq!(next.last().unwrap().get("text").and_then(Value::as_str), Some("next"));
    }

    #[test]
    fn overflow_retry_trims_superseded_proposal_payload_and_keeps_working_version() {
        use ChatRole::{Assistant, Tool};
        let old = proposal("architecture", &"a".repeat(6144));
        let refused = proposal("architecture", &"b".repeat(6144));
        let working = proposal("architecture", &"c".repeat(6144));
        let findings = r#"{"findings":["review these findings"]}"#;
        let history = vec![ChatMessage::new(Assistant, &old), ChatMessage::new(Tool, "ok"),
            ChatMessage::new(Assistant, &refused), ChatMessage::new(Tool, "ERROR: invalid change"),
            ChatMessage::new(Assistant, crate::local_llm::tool_call_markup("check.list", &[])), ChatMessage::new(Tool, findings),
            ChatMessage::new(Assistant, &working), ChatMessage::new(Tool, "ok")];
        let node = Scripted::default();
        node.job_errors.lock().unwrap().push_back(crate::llm_backend::overflow_message(9500, crate::llm_backend::DECODE_RESERVE, 8192).unwrap());
        node.windows.lock().unwrap().push_back(8192);
        let (events, sent, _) = run_scripted_lines(node, &["</think>done"], history, &["continue"]);
        assert!(matches!(events.last(), Some(ChatEvent::TurnDone { context_max: 8192, .. })));
        assert_eq!(sent.len(), 2);
        let rows = sent[1].get("chat_messages").unwrap().as_arr().unwrap();
        let text = |i: usize| rows[i].get("text").unwrap().as_str().unwrap();
        assert!(text(0).contains("[trimmed superseded tool call:") && !text(0).contains(&old));
        assert!(text(2).contains("[trimmed refused tool call:") && !text(2).contains(&refused));
        assert!(text(5).contains(findings));
        assert!(text(6).contains(&working));
    }

    #[test]
    fn long_proxy_session_either_shortens_or_fails_visibly_without_starving() {
        use ChatRole::{Assistant, Tool, User};
        let config = || HubChatConfig { max_tokens: Some(512), thinking: Some(false), ..scripted_config() };
        let overflow = |used| crate::llm_backend::overflow_message(used, 512, 4096).unwrap();
        let failures = |events: &[ChatEvent]| events.iter().filter(|e| matches!(e, ChatEvent::Failed(why) if why.starts_with("This line doesn't fit"))).count();
        let node = Scripted::default();
        node.replies.lock().unwrap().extend(["one".into(), "two".into()]);
        let run = scripted_proxy(node.clone(), config(), Vec::new(), |chat| {
            let mut events = scripted_line(chat, "first");
            events.extend(scripted_line(chat, "second"));
            node.job_errors.lock().unwrap().push_back(overflow(3600));
            events.extend(scripted_line(chat, "third"));
            events
        });
        assert_eq!(run.generates.len(), 3);
        assert_eq!(failures(&run.events), 1);
        assert_eq!(run.events.iter().filter(|e| matches!(e, ChatEvent::TurnDone { .. })).count(), 2);
        assert_eq!(run.history, vec![ChatMessage::new(User, "first"), ChatMessage::new(Assistant, "one"),
            ChatMessage::new(User, "second"), ChatMessage::new(Assistant, "two")]);

        let history = vec![ChatMessage::new(Assistant, proposal("draft", &"old".repeat(2048))), ChatMessage::new(Tool, "old result ".repeat(512)),
            ChatMessage::new(Assistant, proposal("draft", "working")), ChatMessage::new(Tool, "ok")];
        for visible in [false, true] {
            let node = Scripted::default();
            node.job_errors.lock().unwrap().push_back(overflow(4096));
            node.replies.lock().unwrap().push_back("finished".into());
            node.windows.lock().unwrap().push_back(4096);
            if visible { node.partials.lock().unwrap().push_back(("/job/j1".into(), "visible prose".into())); }
            let config = HubChatConfig { tools: tools(), ..config() };
            let run = scripted_proxy(node, config, history.clone(), |chat| scripted_line(chat, "continue"));
            if visible {
                assert_eq!(run.generates.len(), 1);
                assert!(run.events.iter().any(|e| matches!(e, ChatEvent::Delta(s) if s.contains("visible prose"))));
                assert_eq!(failures(&run.events), 1);
                assert!(!run.events.iter().any(|e| matches!(e, ChatEvent::TurnDone { .. })));
                assert_eq!(run.history, history);
            } else {
                assert_eq!(run.generates.len(), 2);
                assert!(run.generates[1].to_json().contains("[trimmed superseded tool call:"));
                assert!(matches!(run.events.last(), Some(ChatEvent::TurnDone { context_max: 4096, .. })));
            }
        }
    }

    #[test]
    fn orphaned_tool_results_fail_visibly_unless_cancelled() {
        let tool = r#"<tool_call>{"name":"doc.read","arguments":{}}</tool_call>"#;
        let config = || HubChatConfig { tools: tools(), thinking: Some(false), ..scripted_config() };
        let node = Scripted::default();
        node.replies.lock().unwrap().extend([tool.into(), "summary".into(), "next answer".into(), tool.into(), "fresh answer".into()]);
        let run = scripted_proxy(node, config(), Vec::new(), |chat| {
            let request_tool = || {
                assert!(matches!(scripted_line(chat, "run tool").last(), Some(ChatEvent::TurnDone { tool_calls: 1, .. })));
            };
            request_tool();
            assert!(matches!(scripted_line(chat, "summarize").last(), Some(ChatEvent::TurnDone { tool_calls: 0, .. })));
            chat.send_tool_results(vec![("late".into(), false)]);
            chat.send_user_turn("next".into());
            assert!(matches!(collect_until_turn_done(&chat.from_worker).last(), Some(ChatEvent::Failed(why)) if why == ORPHAN_TOOL_RESULTS));
            assert!(matches!(collect_until_turn_done(&chat.from_worker).last(), Some(ChatEvent::TurnDone { tool_calls: 0, .. })));

            request_tool();
            chat.cancel();
            chat.send_tool_results(vec![("stale after stop".into(), false)]);
            scripted_line(chat, "fresh")
        });
        assert_eq!(run.generates.len(), 5);
        assert!(matches!(run.events.last(), Some(ChatEvent::TurnDone { tool_calls: 0, .. })));
        assert!(run.events.iter().any(|e| matches!(e, ChatEvent::Delta(s) if s.contains("fresh answer"))));
        assert!(!run.events.iter().any(|e| matches!(e, ChatEvent::Failed(_))));

        let node = Scripted::default();
        node.job_errors.lock().unwrap().push_back("backend unavailable".into());
        let hold = |event: &ChatEvent| matches!(event, ChatEvent::Failed(why) if why == ORPHAN_TOOL_RESULTS);
        let run = scripted_proxy_until_exit(node, config(), hold, |chat| {
            assert!(matches!(scripted_line(chat, "summarize").last(), Some(ChatEvent::Failed(why)) if why.contains("backend unavailable")));
            chat.send_tool_results(vec![("late".into(), false)]);
            chat.send_user_turn("recover".into());
            collect_until_turn_done(&chat.from_worker)
        });
        assert!(matches!(run.events.last(), Some(ChatEvent::Failed(why)) if why == ORPHAN_TOOL_RESULTS));
        assert_eq!(run.generates.len(), 1);
        assert!(matches!(run.exit, ProxyExit::Reelect));
        assert!(matches!(run.pending.front(), Some(WorkerMsg::UserTurn(text, _)) if text == "recover"));
    }

    #[test]
    fn proxy_shortens_against_live_fitted_window_and_reports_context_max_on_turn_done() {
        let node = Scripted::default();
        node.windows.lock().unwrap().extend([114_688, 16_384]);
        let oldest = "old result ".repeat(5_000);
        let history = vec![
            ChatMessage::new(ChatRole::Tool, oldest.clone()),
            ChatMessage::new(ChatRole::Tool, "recent result ".repeat(30)),
            ChatMessage::new(ChatRole::Tool, "latest result ".repeat(30)),
        ];
        let (events, sent, _) = run_scripted_lines(node, &["</think>one", "</think>two", "</think>three"],
            history, &["first", "second", "third"]);
        let windows: Vec<_> = events.iter().filter_map(|e| match e {
            ChatEvent::TurnDone { context_max, .. } => Some(*context_max), _ => None,
        }).collect();
        assert_eq!(windows, [114_688, 16_384, 16_384]);
        assert_eq!(sent.len(), 3);
        assert!(sent[1].to_json().contains(&oldest));
        assert!(!sent[2].to_json().contains(&oldest));
        assert!(sent[2].to_json().contains("an earlier tool result, elided"));
        assert!(events.iter().any(|e| matches!(e, ChatEvent::Loading { phase, .. } if phase == "shortening the conversation")));
    }

    #[test]
    fn a_node_overflow_sends_one_plain_failure_after_the_proxy_finishes() {
        for (diagnostic, need) in [(
            "context overflow: the prompt is 33001 tokens and the answer needs 128 more, but this node's context is 32768 tokens (shorten the conversation, or POST /llm/context to raise it)", 33_129),
            ("session context overflow: need 37934 tokens, max_context is 32768", 37_934),
        ] {
            let node = Scripted { job_error: Some(diagnostic), ..Default::default() };
            let (events, generates, overflow) = run_scripted_lines(node, &[], Vec::new(), &["hello"]);
            assert_eq!(overflow, Some(ContextOverflow { need, max: 32_768 }));
            let failures: Vec<_> = events.iter().filter_map(|event| match event {
                ChatEvent::Failed(reason) => Some(reason.as_str()),
                _ => None,
            }).collect();
            assert_eq!(generates.len(), 1, "a line with no older tool results to shorten must be submitted once");
            assert_eq!(failures, ["This line doesn't fit in the room the model has left in this chat. Try it with fewer pictures, or start a new chat."],
                "the terminal job error must reach the engine once without the node diagnostic or route");
        }
    }

    #[test]
    fn an_accepted_think_only_completion_is_not_resubmitted() {
        let (events, generates) = run_scripted_turn(Scripted::default(), &[
            "reasoning without an answer",
            "new reasoning\n</think>\n\nA visible answer.",
        ]);
        assert_eq!(generates.len(), 1);
        assert!(events.iter().all(|event| !matches!(event, ChatEvent::Delta(_))));
        assert_eq!(events.iter().filter(|e| matches!(e, ChatEvent::Failed(_))).count(), 1);
        assert!(matches!(events.last(), Some(ChatEvent::Failed(error)) if error == "empty completion"));
    }

    #[test]
    fn empty_completions_fail_without_a_whitespace_delta() {
        for reply in ["", "\n", "</think>\n\n"] {
            let (events, generates) = run_scripted_turn(Scripted::default(), &[reply]);
            assert_eq!(generates.len(), 1);
            assert!(events.iter().all(|event| !matches!(event, ChatEvent::Delta(_))), "{events:?}");
            assert!(matches!(events.last(), Some(ChatEvent::Failed(error)) if error == "empty completion"));
        }
    }

    #[test]
    fn closed_think_short_answers_are_visible_and_never_retried() {
        for answer in ["x", "7", "✅"] {
            let (events, generates) = run_scripted_closed_think_turn(answer);
            let visible: String = events
                .iter()
                .filter_map(|event| match event {
                    ChatEvent::Delta(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(visible, answer);
            assert_eq!(generates.len(), 1, "{answer:?} was retried");
            assert!(matches!(events.last(), Some(ChatEvent::TurnDone { .. })));
        }
    }

    #[derive(Clone, Default)]
    struct DelayedModels;

    impl FleetTransport for DelayedModels {
        fn post_json(&mut self, _url: &str, _body: &Value) -> Result<Value, String> {
            unreachable!("discovery election does not submit a job")
        }

        fn get_json(&mut self, url: &str) -> Result<Value, String> {
            if url.ends_with("/health") {
                return Ok(json::obj(vec![(
                    "capabilities",
                    Value::Arr(vec![json::s("chat")]),
                )]));
            }
            let id = if url.starts_with("http://n2") {
                "qwen3.6-27b"
            } else {
                "qwen3.5-9b"
            };
            Ok(json::obj(vec![(
                "models",
                Value::Arr(vec![json::obj(vec![
                    ("id", json::s(id)),
                    ("domain", json::s("chat")),
                    ("available", Value::Bool(true)),
                    ("state", json::s("loaded")),
                ])]),
            )]))
        }
    }

    fn discovery_wait(duration: Duration) -> bool {
        if !duration.is_zero() { std::thread::sleep(duration); }
        true
    }

    #[test]
    fn discovery_waits_for_a_preferred_model_not_on_the_first_node() {
        let calls = std::cell::Cell::new(0usize);
        let (provider, base, model) = fleet_provider_with(
            ChatReach::Lan,
            &|_| {},
            Some("qwen3.6-27b"),
            None,
            None,
            Vec::new(),
            || {
                let call = calls.get();
                calls.set(call + 1);
                if call == 0 {
                    vec!["http://n1:1".into()]
                } else {
                    vec!["http://n1:1".into(), "http://n2:2".into()]
                }
            },
            || DelayedModels,
            Duration::from_secs(1),
            Duration::ZERO,
            discovery_wait,
        )
        .unwrap();
        assert_eq!(
            (base, model),
            ("http://n2:2".into(), "qwen3.6-27b".into())
        );
        drop(provider);
        assert!(calls.get() >= 2);
    }

    #[test]
    fn this_machines_hub_is_elected_before_a_lan_node_listed_first_alphabetically() {
        // Both serve a chat model; the caller ranked loopback first.
        let (_provider, base, _) = fleet_provider_with(
            ChatReach::Lan,
            &|_| {},
            None,
            None,
            None,
            Vec::new(),
            || vec!["http://127.0.0.1:8123".into(), "http://192.0.2.93:8123".into(), "http://127.0.0.1:8123".into()],
            || DelayedModels,
            Duration::from_secs(1),
            Duration::ZERO,
            discovery_wait,
        )
        .unwrap();
        assert_eq!(base, "http://127.0.0.1:8123", "LAN reach prefers the local node");
    }

    #[test]
    fn machine_reach_uses_only_this_machines_local_runtimes_and_follows_no_redirect() {
        let local = "http://127.0.0.1:8123";
        let lan = "http://192.0.2.93:8123";
        let tls = "https://127.0.0.1:8123";
        assert_eq!(ChatReach::default(), ChatReach::Lan, "existing chats must default to LAN reach");
        let node = Scripted {
            models: Some(json::parse(br#"{"models":[
                {"id":"claude-cli","domain":"chat","backend":"claude-cli","available":true},
                {"id":"unknown","domain":"chat","backend":"new-runtime","available":true},
                {"id":"legacy","domain":"chat","available":true},
                {"id":"qwen-local","domain":"text","backend":"llm","available":true}
            ]}"#).unwrap()),
            ..Default::default()
        };
        let elect = |wanted| fleet_provider_with(
            ChatReach::Machine, &|_| panic!("machine election must never wait for the fleet"),
            wanted, None, None, Vec::new(), || vec![lan.into(), tls.into(), local.into()],
            || node.clone(), FLEET_PATIENCE, Duration::ZERO, discovery_wait,
        );
        let (_, base, model) = elect(None).expect("the installed text runtime must be eligible");
        assert_eq!((base.as_str(), model.as_str()), (local, "qwen-local"), "only the loopback llm offer may win");
        for forbidden in [crate::claude_cli::MODEL_ID, "unknown", "legacy"] {
            assert!(elect(Some(forbidden)).is_err(), "naming {forbidden} cannot bypass its backend check");
        }
        let inventory = chat_offers_for(&mut node.clone(), local, ChatReach::Machine).unwrap();
        assert_eq!(inventory.iter().map(|o| (o.model.as_str(), o.backend.as_str())).collect::<Vec<_>>(),
            [("qwen-local", "llm")], "machine inventory must apply the same backend allowlist");
        assert!(node.gets.lock().unwrap().iter().all(|url| url.starts_with(&format!("{local}/"))),
            "even a supplied LAN or HTTPS base must receive no probes");

        let stops = TurnStops::default();
        let (queue, input) = channel();
        queue.send(WorkerMsg::UserTurn("refused before routing".into(), stops.for_line())).unwrap();
        let mut pending = VecDeque::new();
        let retry = after_election_failure(&stops.current(), &mut pending, &input, &mut Vec::new(), &mut Vec::new(),
            &AtomicU64::new(0), &Mutex::default(), &|event| {
            assert!(matches!(event, ChatEvent::Failed(_)), "a failed election must report the refusal");
            queue.send(WorkerMsg::UserTurn("try again".into(), stops.for_line())).unwrap();
        }, "no route".into());
        assert!(retry && matches!(pending.front(), Some(WorkerMsg::UserTurn(line, _)) if line == "try again"),
            "an input queued during the failed election must never become the next route's first request");

        let failure = |bases: Vec<String>| {
            let config = HubChatConfig {
                reach: ChatReach::Machine, llm: LocalLlmConfig::new("".into()), preferred_model: None,
                max_tokens: None, thinking: None, claude_model: None, system_prompt: String::new(),
                tools: Vec::new(), wake: None,
            };
            let (commands, input) = channel();
            drop(commands);
            let (events, output) = channel();
            let route = Arc::new(Mutex::new(None));
            let stops = Arc::new(TurnStops::default());
            elect_and_run(config, input, events, stops.clone(), stops.current(), route.clone(), Arc::default(), |discovery| {
                assert!(discovery.is_none(), "machine reach must never start a discovery listener");
                bases.clone()
            });
            assert!(route.lock().unwrap().is_none(), "failed election must publish no route");
            match output.into_iter().map(|e| e.event).collect::<Vec<_>>().as_slice() {
                [ChatEvent::Failed(reason)] => reason.clone(),
                other => panic!("the worker must emit just its failure: {other:?}"),
            }
        };
        assert_eq!(failure(Vec::new()), "AI Hub on this computer serves no local text model",
            "an empty machine must fail with the hub reason");

        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::{TcpListener, TcpStream};
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let origin = listener.local_addr().unwrap();
        let models = node.models.unwrap().to_json();
        let server = std::thread::spawn(move || {
            let mut paths = Vec::new();
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(&stream);
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 { break; }
                let path = line.split_whitespace().nth(1).unwrap().to_string();
                let mut length = 0;
                for header in reader.by_ref().lines() {
                    let header = header.unwrap();
                    if header.is_empty() { break; }
                    if let Some((name, value)) = header.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") { length = value.trim().parse().unwrap(); }
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let response = if paths.is_empty() || path == "/generate" {
                    format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{origin}/redirected\r\nContent-Length: 0\r\n\r\n")
                } else if path == "/models" || path == "/health" {
                    let body = if path == "/models" { &models[..] } else { r#"{"capabilities":["chat"]}"# };
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len())
                } else {
                    "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n".into()
                };
                paths.push(path);
                stream.write_all(response.as_bytes()).unwrap();
            }
            paths
        });
        let reason = failure(vec![format!("http://{origin}")]);
        let mut provider = FleetQwenChatProvider::for_reach(RequestPolicy::on_device(), vec![format!("http://{origin}")], ChatReach::Machine);
        let turn = TurnInput::new(String::new(), vec![ChatMessage::new(ChatRole::User, "private turn")]);
        let submission = provider.begin_turn(&turn);
        drop(TcpStream::connect(origin).unwrap());
        assert_eq!(server.join().unwrap(), ["/health", "/health", "/models", "/generate"],
            "neither discovery nor submission may follow a redirect or replay a request");
        assert!(reason.contains("307 redirect"), "the session must report the redirect: {reason}");
        assert!(submission.is_err_and(|why| why.contains("307 redirect")), "a redirected turn must fail admission");
    }

    #[test]
    fn machine_reach_waits_for_busy_submission_and_job_polls() {
        let local = "http://127.0.0.1:8123";
        for (submitting, failures) in [(true, 5), (false, 5), (false, 24)] {
            let clock = Arc::new(Mutex::new(Instant::now()));
            let node = Scripted {
                clock: Some(clock.clone()),
                models: Some(json::parse(br#"{"models":[{"id":"local","domain":"chat","backend":"llm","available":true}]}"#).unwrap()),
                ..Default::default()
            };
            let busy = FleetError::Http { status: 503, reason: "all lanes busy".into(), no_job: true };
            let errors = if submitting { &node.submit_errors } else { &node.poll_errors };
            errors.lock().unwrap().extend((0..failures).map(|index| match (failures, index) {
                (24, 5) => FleetError::Http { status: 429, reason: "busy".into(), no_job: false },
                (24, 6) => FleetError::Connection("connect refused".into()),
                (24, 7) => FleetError::Transport("connection closed in headers".into()),
                _ => busy.clone(),
            }));
            node.replies.lock().unwrap().push_back("the summary".into());
            let mut provider = FleetQwenChatProvider::for_reach(node.clone(), vec![local.into()], ChatReach::Machine)
                .with_thinking(Some(false));
            provider.begin_turn(&TurnInput::new("", vec![ChatMessage::new(ChatRole::User, "summarize")])).unwrap();
            let mut events = provider.poll();
            let mut attempts = node.gets.lock().unwrap().len() + node.generates.lock().unwrap().len();
            for failure in 0..failures {
                assert!(provider.waiting_for_machine(), "a busy hub must keep the turn alive");
                let delay_ms = [250, 500, 1000, 2000, 4000, 8000].get(failure).copied().unwrap_or(10000);
                *clock.lock().unwrap() += Duration::from_millis(delay_ms - 1);
                assert!(provider.poll().is_empty(), "nothing is emitted before the retry is due");
                assert_eq!(node.gets.lock().unwrap().len() + node.generates.lock().unwrap().len(), attempts,
                    "no request is sent during backoff");
                *clock.lock().unwrap() += Duration::from_millis(1);
                events.extend(provider.poll());
                attempts += 1;
                assert_eq!(node.gets.lock().unwrap().len() + node.generates.lock().unwrap().len(), attempts,
                    "a due retry makes exactly one request to the same hub");
            }
            if submitting { events.extend(provider.poll()); }
            assert!(matches!(events.first(), Some(ProviderEvent::Status { note, permille: 0 })
                if note == "waiting for this computer's AI Hub"), "the first refusal must explain the wait: {events:?}");
            assert_eq!(events.iter().filter(|e| matches!(e, ProviderEvent::Status { note, .. }
                if note == "waiting for this computer's AI Hub")).count(), 1, "announce a busy spell once");
            assert!(!events.iter().any(|e| matches!(e, ProviderEvent::Error(_))), "busy must never fail the turn: {events:?}");
            assert!(events.contains(&ProviderEvent::Done { text: "the summary".into() }), "the accepted job must finish: {events:?}");
            assert!(!provider.waiting_for_machine(), "success must end the busy spell");
            let posts = node.generates.lock().unwrap();
            assert_eq!(posts.len(), if submitting { failures + 1 } else { 1 }, "only a confirmed refusal allows resubmission");
            assert!(posts.iter().all(|body| body == &posts[0]), "every resubmission must preserve the original request");
        }
    }

    #[test]
    fn a_named_model_that_no_node_serves_is_an_error_not_another_model() {
        let result = fleet_provider_with(
            ChatReach::Lan,
            &|_| {},
            Some("qwen3.8-27b"),
            None,
            None,
            Vec::new(),
            || vec!["http://n1:1".into()],
            || DelayedModels,
            Duration::from_millis(50),
            Duration::ZERO,
            discovery_wait,
        );
        assert!(result.is_err());
    }
}

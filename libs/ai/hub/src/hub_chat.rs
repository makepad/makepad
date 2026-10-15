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
#[cfg(feature = "llm")]
use crate::local_llm::build_prefix;
use crate::local_llm::{parse_tool_call, system_text, ChatEvent, LocalLlmConfig, ToolSpec, WakeHook};
#[cfg(feature = "llm")]
use crate::machine::Claim;
use crate::machine::{self, ResidencyState};
use crate::providers::provider::{validate_tool_images, ChatProvider, ProviderEvent, ToolImage, TurnInput};
use crate::providers::qwen::{FleetQwenChatProvider, FleetTransport, HttpFleetTransport};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
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
    /// One phrase for a status chip: `qwen3.8-27b on 10.0.0.165`.
    pub fn label(&self) -> String {
        match self {
            ChatRoute::MachineNode { port } => format!("this machine's node :{port}"),
            ChatRoute::Fleet { base, model } => format!("{model} on {}", host_of(base)),
            ChatRoute::InProcess { .. } => "in-process".to_string(),
        }
    }
}

/// `http://10.0.0.165:8123` → `10.0.0.165`.
fn host_of(base: &str) -> &str {
    let rest = base.trim_start_matches("http://").trim_start_matches("https://");
    rest.split([':', '/']).next().unwrap_or(rest)
}

enum WorkerMsg {
    UserTurn(String),
    ToolResults(Vec<(String, bool)>),
    AttachImages(Vec<ToolImage>),
}

/// A running hub chat. Same shape as [`crate::local_llm::LocalLlmSession`].
pub struct HubChatSession {
    to_worker: Sender<WorkerMsg>,
    from_worker: Receiver<ChatEvent>,
    cancel: Arc<AtomicBool>,
    route: Arc<Mutex<Option<ChatRoute>>>,
}

pub struct HubChatConfig {
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
    pub system_prompt: String,
    pub tools: Vec<ToolSpec>,
    pub wake: Option<WakeHook>,
}

impl HubChatSession {
    pub fn start(config: HubChatConfig) -> Self {
        let (event_tx, from_worker) = channel();
        let (to_worker, msg_rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let route = Arc::new(Mutex::new(None));
        let worker_route = route.clone();
        std::thread::Builder::new()
            .name("ai-hub-chat".into())
            .spawn(move || elect_and_run(config, msg_rx, event_tx, worker_cancel, worker_route))
            .expect("spawn hub chat worker");
        Self {
            to_worker,
            from_worker,
            cancel,
            route,
        }
    }

    pub fn send_user_turn(&self, text: String) {
        self.cancel.store(false, Ordering::Relaxed);
        let _ = self.to_worker.send(WorkerMsg::UserTurn(text));
    }

    pub fn send_tool_results(&self, results: Vec<(String, bool)>) {
        let _ = self.to_worker.send(WorkerMsg::ToolResults(results));
    }

    /// Queue pictures on the same channel so the next input cannot overtake them.
    pub fn attach_images(&self, images: Vec<ToolImage>) -> Result<(), String> {
        validate_tool_images(&images)?;
        self.to_worker.send(WorkerMsg::AttachImages(images))
            .map_err(|_| "the hub chat worker stopped".to_string())
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn poll(&self) -> Vec<ChatEvent> {
        self.from_worker.try_iter().collect()
    }

    /// Where the conversation runs, once the election settled; `None`
    /// while it is still deciding (or when nothing answers).
    pub fn route(&self) -> Option<ChatRoute> {
        self.route.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Drop for HubChatSession {
    fn drop(&mut self) {
        // Covers election/probe and pre-admission waits before the worker
        // next observes its disconnected command receiver.
        self.cancel.store(true, Ordering::Relaxed);
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
    /// The node failed a turn. The person's NEXT line, already received,
    /// re-runs the election and is served first on whatever it picks.
    Reelect(WorkerMsg),
}

fn elect_and_run(
    config: HubChatConfig,
    msg_rx: Receiver<WorkerMsg>,
    event_tx: Sender<ChatEvent>,
    cancel: Arc<AtomicBool>,
    route_slot: Arc<Mutex<Option<ChatRoute>>>,
) {
    let wake = config.wake.clone();
    let events = event_tx.clone();
    let send = move |event: ChatEvent| {
        let _ = events.send(event);
        if let Some(wake) = &wake {
            wake();
        }
    };
    let set_route = |route: Option<ChatRoute>| {
        *route_slot.lock().unwrap_or_else(|e| e.into_inner()) = route;
    };
    // A turn that arrived while a previous route was failing: served first
    // on the next one.
    let mut first: Option<WorkerMsg> = None;
    let mut images = Vec::new();
    loop {
        set_route(None);

        // 1. A co-located holder serving this model over loopback.
        // A named model is strict: only a node serving exactly that id answers;
        // this machine's own weights are never loaded in its place.
        let strict = config.preferred_model.is_some();
        let has_local_file = config.llm.model.is_file() && !strict;
        if has_local_file {
            let key = election_key(&config.llm);
            if let Some(port) = machine_holder_port(&key, &send) {
                let base = format!("http://127.0.0.1:{port}");
                let route = ChatRoute::MachineNode { port };
                set_route(Some(route.clone()));
                let provider = FleetQwenChatProvider::new(HttpFleetTransport, vec![base])
                    .with_preferred_model(config.preferred_model.clone())
                    .with_max_tokens(config.max_tokens)
                    .with_thinking(config.thinking);
                match run_proxy(provider, &config, first.take(), &msg_rx, &mut images, &send, &cancel, route) {
                    ProxyExit::Ended => return,
                    ProxyExit::Reelect(msg) => {
                        first = Some(msg);
                        continue;
                    }
                }
            }
        }

        // 2. A fleet chat node on the LAN.
        let fleet_reason = match fleet_provider(
            &send,
            config.preferred_model.as_deref(),
            config.max_tokens,
            config.thinking,
        ) {
            Ok((provider, route)) => {
                set_route(Some(route.clone()));
                match run_proxy(provider, &config, first.take(), &msg_rx, &mut images, &send, &cancel, route) {
                    ProxyExit::Ended => return,
                    ProxyExit::Reelect(msg) => {
                        first = Some(msg);
                        continue;
                    }
                }
            }
            Err(reason) => reason,
        };

        // 3. The weights on this machine (a build with the `llm` runtime).
        #[cfg(feature = "llm")]
        if has_local_file {
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
            let local_rx = local_commands(first, msg_rx, images, event_tx.clone(), config.wake.clone());
            crate::local_llm::worker_main(
                config.llm,
                prefix,
                None,
                local_rx,
                event_tx,
                cancel,
                config.wake,
                guard,
            );
            return;
        }

        // 4. Nothing answers. Say so once, and try again on the person's
        // next line — a fleet node may have come up by then. The line that
        // just failed is dropped: handing it back would retry it forever.
        eprintln!("[hub-chat] no model: {fleet_reason}; local {}", config.llm.model.display());
        match config.preferred_model.as_deref() {
            Some(wanted) => send(ChatEvent::Failed(format!(
                "The required AI model {wanted} is not available: {fleet_reason}. No other model is used (MAKEPAD_AI_MODEL)."
            ))),
            None => send(ChatEvent::Failed(no_model_message(&config.llm.model))),
        }
        images.clear();
        match wait_next_user(&msg_rx, &mut images) {
            Some(msg) => first = Some(msg),
            None => return,
        }
    }
}

/// The port of a co-located node serving this model, waiting out one
/// that is still loading. `None` when the election is open, held by a
/// non-serving app (the documented duplicate-copy soft failure), or the
/// holder failed.
fn machine_holder_port(key: &str, send: &impl Fn(ChatEvent)) -> Option<u16> {
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
                    std::thread::sleep(POLL * 4);
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

/// Listen for the fleet, then let the provider's own honest probe pick the
/// node and model. `Err` names why there is none.
fn fleet_provider(
    send: &impl Fn(ChatEvent),
    preferred_model: Option<&str>,
    max_tokens: Option<u32>,
    thinking: Option<bool>,
) -> Result<(FleetQwenChatProvider<HttpFleetTransport>, ChatRoute), String> {
    let discovery = crate::discovery::start_listener();
    fleet_provider_with(
        send,
        preferred_model,
        max_tokens,
        thinking,
        || {
            let mut bases = fleet_chat_bases(&discovery.nodes());
            // A serving node on this machine does not need to announce over
            // LAN discovery. Entries may be stale; probing, not mere presence,
            // decides whether they are usable.
            bases.extend(crate::machine::read_node_entries().into_iter().filter_map(
                |(_, entry)| (entry.port != 0).then(|| format!("http://127.0.0.1:{}", entry.port)),
            ));
            // A pinned hub (MAKEPAD_AI_HUB, the client's `pinned` file) is
            // reached without its LAN beacon.
            bases.extend(crate::hub_order::pinned_bases());
            crate::hub_order::order(&mut bases);
            bases
        },
        || HttpFleetTransport,
        FLEET_PATIENCE,
        FLEET_POLL,
    )
}

/// Listen for the fleet and report the route a chat would take, without
/// starting a conversation or loading anything: the same discovery and
/// probe the election runs. Blocks up to the fleet patience, so call it
/// from a worker thread. `Err` names why no chat node answers.
pub fn probe_fleet_chat() -> Result<ChatRoute, String> {
    let preferred = std::env::var(crate::hub::REQUIRED_MODEL_ENV).ok().map(|m| m.trim().to_string()).filter(|m| !m.is_empty());
    fleet_provider(&|_| {}, preferred.as_deref(), None, None).map(|(_, route)| route)
}

/// The first of each base, in the order given.
fn dedup_in_order(bases: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    bases.into_iter().filter(|b| seen.insert(b.clone())).collect()
}

fn fleet_provider_with<T: FleetTransport>(
    send: &impl Fn(ChatEvent),
    preferred_model: Option<&str>,
    max_tokens: Option<u32>,
    thinking: Option<bool>,
    mut discovered_bases: impl FnMut() -> Vec<String>,
    mut transport: impl FnMut() -> T,
    patience: Duration,
    poll: Duration,
) -> Result<(FleetQwenChatProvider<T>, ChatRoute), String> {
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
            let mut provider = FleetQwenChatProvider::new(transport(), bases.clone())
                .with_preferred_model(preferred_model.map(str::to_string))
                .with_max_tokens(max_tokens)
                .with_thinking(thinking);
            match provider.availability() {
                ProviderAvailability::Available { model, detail } => {
                    let route = ChatRoute::Fleet { base: detail, model: model.clone() };
                    if preferred_model.is_none_or(|preferred| preferred.trim() == model) {
                        return Ok((provider, route));
                    }
                    // A healthy ordinary pick is remembered, but a preference
                    // gets the full discovery window to announce before this
                    // fallback is accepted.
                    fallback = Some((provider, route));
                }
                ProviderAvailability::Unavailable { reason } => last_reason = Some(reason),
            }
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
        if !poll.is_zero() {
            std::thread::sleep(poll);
        }
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

/// Block for the person's next line; stale tool results from the turn
/// that failed are dropped. `None` when the consumer is gone.
fn wait_next_user(msg_rx: &Receiver<WorkerMsg>, images: &mut Vec<ToolImage>) -> Option<WorkerMsg> {
    loop {
        match msg_rx.recv() {
            Ok(msg @ WorkerMsg::UserTurn(_)) => return Some(msg),
            Ok(WorkerMsg::ToolResults(_)) => images.clear(),
            Ok(WorkerMsg::AttachImages(attached)) => *images = attached,
            Err(_) => return None,
        }
    }
}

// The in-process worker only accepts text; reject image turns before forwarding.
#[cfg(feature = "llm")]
fn local_commands(
    first: Option<WorkerMsg>,
    msg_rx: Receiver<WorkerMsg>,
    images: Vec<ToolImage>,
    event_tx: Sender<ChatEvent>,
    wake: Option<WakeHook>,
) -> Receiver<crate::local_llm::WorkerMsg> {
    let (to_local, local_rx) = channel();
    let mut has_images = !images.is_empty();
    std::thread::Builder::new()
        .name("ai-hub-chat-input".into())
        .spawn(move || {
            for msg in first.into_iter().chain(msg_rx) {
                let msg = match msg {
                    WorkerMsg::AttachImages(images) => {
                        has_images = !images.is_empty();
                        continue;
                    }
                    WorkerMsg::UserTurn(text) => crate::local_llm::WorkerMsg::UserTurn(text),
                    WorkerMsg::ToolResults(results) => crate::local_llm::WorkerMsg::ToolResults(results),
                };
                if std::mem::take(&mut has_images) {
                    let _ = event_tx.send(ChatEvent::Failed("the in-process model cannot look at pictures".into()));
                    if let Some(wake) = &wake { wake(); }
                    continue;
                }
                if to_local.send(msg).is_err() { break; }
            }
        })
        .expect("spawn hub chat input worker");
    local_rx
}

/// The proxy branch: the conversation runs on a node; this worker holds
/// the transcript, hands the node the same tool-bearing system text the
/// in-process prefix carries, and splits the node's `<think>` and
/// `<tool_call>` markup out of the streamed text with the one parser.
fn run_proxy<T: FleetTransport>(
    provider: FleetQwenChatProvider<T>,
    config: &HubChatConfig,
    first: Option<WorkerMsg>,
    msg_rx: &Receiver<WorkerMsg>,
    images: &mut Vec<ToolImage>,
    send: &impl Fn(ChatEvent),
    cancel: &Arc<AtomicBool>,
    route: ChatRoute,
) -> ProxyExit {
    let mut provider = provider.with_cancel_signal(cancel.clone());
    let system = system_text(&config.system_prompt, &config.tools);
    send(ChatEvent::Ready {
        prefill_tokens: 0,
        secs: 0.0,
    });
    let mut history: Vec<ChatMessage> = Vec::new();
    let mut next: Option<WorkerMsg> = first;
    loop {
        let msg = match next.take() {
            Some(msg) => msg,
            None => match msg_rx.recv() {
                Ok(msg) => msg,
                Err(_) => return ProxyExit::Ended,
            },
        };
        match msg {
            WorkerMsg::AttachImages(attached) => {
                *images = attached;
                continue;
            }
            WorkerMsg::UserTurn(text) => history.push(ChatMessage::new(ChatRole::User, text)),
            WorkerMsg::ToolResults(results) => {
                // One `<tool_response>` turn per result, in call order — the
                // provider wraps each in the trained tags.
                for (text, is_error) in results {
                    let text = if is_error { format!("ERROR: {text}") } else { text };
                    history.push(ChatMessage::new(ChatRole::Tool, text));
                }
            }
        }
        // A turn the node refuses for its context (the conversation outgrew
        // it) is retried once with the oldest tool results elided.
        let mut compacted = false;
        loop {
        let mut retry = false;
        // Keep the prompt inside the node's context before sending it: a
        // refused prompt costs a round trip (and has knocked a node's model
        // over). About 3 bytes a token for code and JSON, room to answer.
        if let Some(max) = provider.context_tokens() {
            let estimate = (system.len() + history.iter().map(|m| m.text.len() + 16).sum::<usize>()) / 3 + ANSWER_ROOM;
            if estimate > max && elide_old_tool_results(&mut history, estimate, max) {
                send(ChatEvent::Loading { phase: "shortening the conversation".into(), fraction: 0.0 });
            }
        }
        let input = TurnInput::new(system.clone(), history.clone());
        'attempt: {
            if cancel.load(Ordering::Relaxed) {
                provider.cancel();
                images.clear();
                break 'attempt;
            }
            let result = provider.attach_tool_images(std::mem::take(images))
                .and_then(|()| provider.begin_turn(&input));
            if let Err(error) = result {
                send(ChatEvent::Failed(format!("{}: {error}", route.label())));
                return match wait_next_user(msg_rx, images) {
                    Some(msg) => ProxyExit::Reelect(msg),
                    None => ProxyExit::Ended,
                };
            }
            let started = Instant::now();
            let mut splitter = MarkupSplitter::default();
            let mut gen_tokens = 0u32;
            let mut pending_visible = String::new();
            let mut has_visible_text = false;
            let mut last_progress = Instant::now();
            let mut last_note = String::from("submitted");
            'turn: loop {
            if cancel.load(Ordering::Relaxed) {
                provider.cancel();
                break 'turn;
            }
            // A line typed mid-answer means the person moved on: the turn
            // is abandoned and that line is served next.
            match msg_rx.try_recv() {
                Err(TryRecvError::Empty) => {}
                Ok(WorkerMsg::AttachImages(attached)) => *images = attached,
                Ok(msg) => {
                    provider.cancel();
                    if matches!(msg, WorkerMsg::UserTurn(_)) {
                        next = Some(msg);
                    } else {
                        images.clear();
                    }
                    break 'turn;
                }
                Err(TryRecvError::Disconnected) => {
                    provider.cancel();
                    return ProxyExit::Ended;
                }
            }
            let mut done = false;
            let events = provider.poll();
            if !events.is_empty() {
                last_progress = Instant::now();
            } else if last_progress.elapsed() >= STALL && !last_note.contains("queued") {
                // Nothing came from the hub for a while: say so (with the last
                // thing it said) and give the line back, instead of a bar that
                // never moves. The hub's own job is cancelled; nothing is retried.
                provider.cancel();
                send(ChatEvent::Failed(format!(
                    "{}: no progress from the AI hub for {} s (last: {last_note}); send the message again to retry",
                    route.label(),
                    STALL.as_secs()
                )));
                return match wait_next_user(msg_rx, images) {
                    Some(msg) => ProxyExit::Reelect(msg),
                    None => ProxyExit::Ended,
                };
            }
            for event in events {
                match event {
                    ProviderEvent::Delta(text) => {
                        let visible = splitter.push(&text);
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
                        let mut tool_calls = 0usize;
                        for body in bodies {
                            match parse_tool_call(&body) {
                                Ok((name, args)) => {
                                    tool_calls += 1;
                                    send(ChatEvent::ToolCall { name, args });
                                }
                                Err(error) => send(ChatEvent::Delta(format!("[bad tool call: {error}]"))),
                            }
                        }
                        send(ChatEvent::TurnDone {
                            tool_calls,
                            tokens: gen_tokens as usize,
                            secs: started.elapsed().as_secs_f64(),
                            context_used: 0,
                            context_max: 0,
                        });
                        done = true;
                    }
                    ProviderEvent::Error(error) => {
                        if !compacted {
                            if let Some((need, max)) = context_overflow(&error) {
                                compacted = true;
                                if elide_old_tool_results(&mut history, need, max) {
                                    send(ChatEvent::Loading { phase: "shortening the conversation".into(), fraction: 0.0 });
                                    retry = true;
                                    break 'attempt;
                                }
                            }
                        }
                        send(ChatEvent::Failed(format!("{}: {error}", route.label())));
                        return match wait_next_user(msg_rx, images) {
                            Some(msg) => ProxyExit::Reelect(msg),
                            None => ProxyExit::Ended,
                        };
                    }
                    ProviderEvent::FunctionCall { .. } | ProviderEvent::FunctionCallPartial { .. } => {}
                }
            }
            if done {
                break 'turn;
            }
            std::thread::sleep(POLL);
            }
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

/// Tokens kept free for the answer when a conversation is fitted to a node.
const ANSWER_ROOM: usize = 4096;

/// `need` and `max_context` of a node's context-overflow refusal
/// ("session context overflow: need 37934 tokens, max_context is 32768").
fn context_overflow(error: &str) -> Option<(usize, usize)> {
    let rest = &error[error.find("context overflow")?..];
    let number_after = |key: &str| -> Option<usize> {
        let tail = &rest[rest.find(key)? + key.len()..];
        tail.trim_start().split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
    };
    Some((number_after("need")?, number_after("max_context is")?))
}

/// Make room for a turn that needs `need` tokens in a `max` context: the
/// oldest tool results (file reads, listings: what the model can ask for
/// again) become a one-line note, oldest first, until the estimate fits
/// with room to answer; the newest two results stay. False when nothing
/// could be elided.
fn elide_old_tool_results(history: &mut [ChatMessage], need: usize, max: usize) -> bool {
    const NOTE: &str = "[an earlier tool result, elided to keep the conversation within the model's context; call the tool again if you need it]";
    // About 3 bytes a token for code and JSON; aim for 80 % of the context.
    let target = need.saturating_sub(max * 4 / 5);
    let mut freed = 0usize;
    let tools: Vec<usize> = history.iter().enumerate().filter(|(_, m)| m.role == ChatRole::Tool && m.text.len() > NOTE.len() * 2).map(|(i, _)| i).collect();
    let keep = tools.len().saturating_sub(2);
    for &index in &tools[..keep] {
        if freed >= target {
            break;
        }
        freed += (history[index].text.len() - NOTE.len()) / 3;
        history[index].text = NOTE.to_string();
    }
    freed > 0
}

// ------------------------------------------------------------ the markup

/// The model's markup, split out of streamed text as it arrives: what is
/// inside `<think>…</think>` is dropped, what is inside
/// `<tool_call>…</tool_call>` is collected for [`parse_tool_call`], the
/// rest is the visible answer. A tag may arrive split across two deltas,
/// so a tail that could still become one is held back until it resolves.
#[derive(Default)]
pub(crate) struct MarkupSplitter {
    pending: String,
    in_think: bool,
    in_tool: bool,
    tool_body: String,
    calls: Vec<String>,
}

const TAGS: [&str; 4] = ["<think>", "</think>", "<tool_call>", "</tool_call>"];

impl MarkupSplitter {
    /// Feed one delta; get the visible text it released.
    pub(crate) fn push(&mut self, delta: &str) -> String {
        self.pending.push_str(delta);
        let mut visible = String::new();
        loop {
            let earliest = TAGS
                .iter()
                .filter_map(|tag| self.pending.find(tag).map(|at| (at, *tag)))
                .min_by_key(|(at, _)| *at);
            match earliest {
                Some((at, tag)) => {
                    let before = self.pending[..at].to_string();
                    self.route(&before, &mut visible);
                    match tag {
                        "<think>" => self.in_think = true,
                        "</think>" => self.in_think = false,
                        "<tool_call>" => {
                            self.in_tool = true;
                            self.tool_body.clear();
                        }
                        _ => {
                            if self.in_tool {
                                self.in_tool = false;
                                self.calls.push(std::mem::take(&mut self.tool_body));
                            }
                        }
                    }
                    self.pending.drain(..at + tag.len());
                }
                None => {
                    let hold = holdback_len(&self.pending);
                    let cut = self.pending.len() - hold;
                    let emit = self.pending[..cut].to_string();
                    self.pending.drain(..cut);
                    self.route(&emit, &mut visible);
                    break;
                }
            }
        }
        visible
    }

    /// The body of a `<tool_call>` that was opened and never closed.
    pub(crate) fn unfinished_call(&self) -> Option<String> {
        self.in_tool.then(|| format!("{}{}", self.tool_body, self.pending))
    }

    /// The turn ended: whatever is still held is text; the tool bodies
    /// collected so far are returned in order.
    pub(crate) fn finish(&mut self) -> (String, Vec<String>) {
        let rest = std::mem::take(&mut self.pending);
        let mut visible = String::new();
        self.route(&rest, &mut visible);
        (visible, std::mem::take(&mut self.calls))
    }

    fn route(&mut self, text: &str, visible: &mut String) {
        if text.is_empty() {
            return;
        }
        if self.in_tool {
            self.tool_body.push_str(text);
        } else if !self.in_think {
            visible.push_str(text);
        }
    }
}

/// The longest tail of `s` that is a proper prefix of some tag — held
/// back, since the rest of the tag may be in the next delta.
fn holdback_len(s: &str) -> usize {
    let longest = TAGS.iter().map(|t| t.len()).max().unwrap_or(0);
    for len in (1..longest).rev() {
        if s.len() < len || !s.is_char_boundary(s.len() - len) {
            continue;
        }
        let tail = &s[s.len() - len..];
        if TAGS.iter().any(|tag| tag.starts_with(tail)) {
            return len;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_strict_json::{self as json, Value};
    use std::collections::VecDeque;

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
        assert!(elide_old_tool_results(&mut history, 37934, 32768));
        let elided: Vec<bool> = history.iter().filter(|m| m.role == ChatRole::Tool).map(|m| m.text.starts_with("[an earlier tool result")).collect();
        // 11 712 tokens to free: one 10 000-token result is not enough, two are.
        assert_eq!(elided, vec![true, true, false, false]);
        assert_eq!(history[0].text, "change it");
        // Only the newest two results left: nothing to elide.
        let mut short = vec![ChatMessage::new(ChatRole::Tool, big.clone()), ChatMessage::new(ChatRole::Tool, big)];
        assert!(!elide_old_tool_results(&mut short, 40000, 32768));
    }

    #[test]
    fn dropping_a_session_signals_cancellation_before_worker_disconnect() {
        let (to_worker, msg_rx) = channel();
        let (_event_tx, from_worker) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let session = HubChatSession {
            to_worker, from_worker, cancel: cancel.clone(),
            route: Arc::new(Mutex::new(None)),
        };
        drop(session);
        assert!(cancel.load(Ordering::Relaxed));
        assert!(matches!(msg_rx.try_recv(), Err(TryRecvError::Disconnected)));
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
            ChatRoute::Fleet { base: "http://10.0.0.165:8123".into(), model: "qwen3.8-27b".into() }.label(),
            "qwen3.8-27b on 10.0.0.165"
        );
        assert_eq!(ChatRoute::MachineNode { port: 8123 }.label(), "this machine's node :8123");
        assert_eq!(ChatRoute::InProcess { model: "x.gguf".into() }.label(), "in-process");
    }

    #[test]
    fn the_splitter_drops_thinking_collects_tool_calls_and_survives_split_tags() {
        let mut s = MarkupSplitter::default();
        let mut visible = String::new();
        // The tags arrive in pieces, as a stream does.
        for delta in ["<thi", "nk>reasoning…</th", "ink>\n\nSure. <tool_ca", "ll>\n<function=os.launch>\n<parameter=app>\nterminal\n</parameter>\n</function>\n</tool_", "call>", " done"] {
            visible.push_str(&s.push(delta));
        }
        let (rest, calls) = s.finish();
        visible.push_str(&rest);
        assert_eq!(visible, "\n\nSure.  done");
        assert_eq!(calls.len(), 1);
        let (name, args) = parse_tool_call(&calls[0]).unwrap();
        assert_eq!(name, "os.launch");
        assert_eq!(args, vec![("app".to_string(), "terminal".to_string())]);
        // A lone `<` that never becomes a tag is text.
        let mut s = MarkupSplitter::default();
        let mut out = s.push("a <");
        out.push_str(&s.push("b"));
        let (rest, calls) = s.finish();
        out.push_str(&rest);
        assert_eq!(out, "a <b");
        assert!(calls.is_empty());
    }

    #[test]
    fn fleet_bases_are_the_chat_roles_sorted_once() {
        use crate::discovery::DiscoveredNode;
        let node = |base: &str| DiscoveredNode { base_url: base.into(), node_id: 1, fleet: "gen".into() };
        let bases = fleet_chat_bases(&[
            node("http://10.0.0.9:8123"),
            node("http://10.0.0.165:8123"),
            node("http://10.0.0.165:8123"),
        ]);
        // The default roles restrict .165 TO chat and leave others open.
        assert_eq!(bases, vec!["http://10.0.0.165:8123", "http://10.0.0.9:8123"]);
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
        generates: Arc<Mutex<Vec<Value>>>,
        replies: Arc<Mutex<VecDeque<String>>>,
        think_open: Arc<Mutex<Option<bool>>>,
    }

    impl FleetTransport for Scripted {
        fn post_json(&mut self, url: &str, body: &Value) -> Result<Value, String> {
            if url.ends_with("/cancel") {
                return Ok(Value::Obj(Vec::new()));
            }
            assert!(url.ends_with("/generate"), "{url}");
            let mut g = self.generates.lock().unwrap();
            g.push(body.clone());
            let mut fields = vec![("job_id", json::s(format!("j{}", g.len())))];
            if let Some(think_open) = *self.think_open.lock().unwrap() {
                fields.push(("think_open", Value::Bool(think_open)));
            }
            Ok(json::obj(fields))
        }
        fn get_json(&mut self, url: &str) -> Result<Value, String> {
            if url.ends_with("/health") {
                return Ok(json::obj(vec![("capabilities", Value::Arr(vec![json::s("chat")]))]));
            }
            if url.ends_with("/models") {
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
            let raw = self.replies.lock().unwrap().pop_front();
            match raw {
                Some(raw) => Ok(json::obj(vec![
                    ("state", json::s("done")),
                    ("partial_text", json::s(raw)),
                    ("text", json::s("")),
                ])),
                None => Ok(json::obj(vec![("state", json::s("error")), ("error", json::s("node went away"))])),
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

    fn collect_until_turn_done(rx: &Receiver<ChatEvent>) -> Vec<ChatEvent> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut out = Vec::new();
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(ev) => {
                    let done = matches!(ev, ChatEvent::TurnDone { .. } | ChatEvent::Failed(_));
                    out.push(ev);
                    if done {
                        return out;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
        }
        panic!("no TurnDone: {out:?}");
    }

    #[test]
    fn a_proxied_conversation_carries_the_tools_and_the_results() {
        let node = Scripted::default();
        node.replies.lock().unwrap().push_back(
            "<think>which app?</think>\n\n<tool_call>\n<function=os.launch>\n<parameter=app>\nterminal\n</parameter>\n</function>\n</tool_call>".into(),
        );
        node.replies.lock().unwrap().push_back("<think>done</think>\n\nOpened the terminal.".into());
        let provider = FleetQwenChatProvider::new(node.clone(), vec!["http://10.0.0.165:8123".into()]);
        let config = HubChatConfig {
            llm: LocalLlmConfig::new("".into()),
            preferred_model: None,
            max_tokens: Some(u32::MAX),
            thinking: None,
            system_prompt: "You run the desktop.".into(),
            tools: tools(),
            wake: None,
        };
        let (to_worker, msg_rx) = channel();
        let (event_tx, events) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = std::thread::spawn(move || {
            let send = |ev: ChatEvent| {
                let _ = event_tx.send(ev);
            };
            let route = ChatRoute::Fleet { base: "http://10.0.0.165:8123".into(), model: "qwen3.8-27b".into() };
            run_proxy(provider, &config, None, &msg_rx, &mut Vec::new(), &send, &cancel, route)
        });
        to_worker.send(WorkerMsg::UserTurn("open the terminal".into())).unwrap();
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
        to_worker.send(WorkerMsg::ToolResults(vec![("launched terminal".into(), false)])).unwrap();
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
        to_worker.send(WorkerMsg::UserTurn("and now?".into())).unwrap();
        let third = collect_until_turn_done(&events);
        match third.last() {
            Some(ChatEvent::Failed(text)) => assert!(text.contains("qwen3.8-27b on 10.0.0.165"), "{text}"),
            other => panic!("expected Failed, got {other:?}"),
        }
        to_worker.send(WorkerMsg::UserTurn("hello again".into())).unwrap();
        match worker.join().unwrap() {
            ProxyExit::Reelect(WorkerMsg::UserTurn(text)) => assert_eq!(text, "hello again"),
            _ => panic!("expected a re-election with the next line"),
        }
    }

    fn run_scripted_turn(replies: &[&str]) -> (Vec<ChatEvent>, Vec<Value>) {
        let node = Scripted::default();
        node.replies
            .lock()
            .unwrap()
            .extend(replies.iter().map(|reply| (*reply).to_string()));
        let provider =
            FleetQwenChatProvider::new(node.clone(), vec!["http://10.0.0.165:8123".into()]);
        let config = HubChatConfig {
            llm: LocalLlmConfig::new("".into()),
            preferred_model: None,
            max_tokens: Some(u32::MAX),
            thinking: None,
            system_prompt: "Answer directly.".into(),
            tools: Vec::new(),
            wake: None,
        };
        let (to_worker, msg_rx) = channel();
        let (event_tx, events) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = std::thread::spawn(move || {
            let send = |event| {
                let _ = event_tx.send(event);
            };
            run_proxy(
                provider,
                &config,
                None,
                &msg_rx,
                &mut Vec::new(),
                &send,
                &cancel,
                ChatRoute::Fleet {
                    base: "http://10.0.0.165:8123".into(),
                    model: "qwen3.8-27b".into(),
                },
            )
        });
        to_worker.send(WorkerMsg::UserTurn("hello".into())).unwrap();
        let output = collect_until_turn_done(&events);
        drop(to_worker);
        assert!(matches!(worker.join().unwrap(), ProxyExit::Ended));
        let generates = node.generates.lock().unwrap().clone();
        (output, generates)
    }

    fn run_scripted_closed_think_turn(reply: &str) -> (Vec<ChatEvent>, Vec<Value>) {
        let node = Scripted::default();
        *node.think_open.lock().unwrap() = Some(false);
        node.replies.lock().unwrap().push_back(reply.to_string());
        let provider =
            FleetQwenChatProvider::new(node.clone(), vec!["http://10.0.0.165:8123".into()]);
        let config = HubChatConfig {
            llm: LocalLlmConfig::new("".into()),
            preferred_model: None,
            max_tokens: None,
            thinking: None,
            system_prompt: "Answer directly.".into(),
            tools: Vec::new(),
            wake: None,
        };
        let (to_worker, msg_rx) = channel();
        let (event_tx, events) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = std::thread::spawn(move || {
            let send = |event| {
                let _ = event_tx.send(event);
            };
            run_proxy(
                provider,
                &config,
                None,
                &msg_rx,
                &mut Vec::new(),
                &send,
                &cancel,
                ChatRoute::Fleet {
                    base: "http://10.0.0.165:8123".into(),
                    model: "qwen3.8-27b".into(),
                },
            )
        });
        to_worker.send(WorkerMsg::UserTurn("hello".into())).unwrap();
        let output = collect_until_turn_done(&events);
        drop(to_worker);
        assert!(matches!(worker.join().unwrap(), ProxyExit::Ended));
        let generates = node.generates.lock().unwrap().clone();
        (output, generates)
    }

    #[test]
    fn an_accepted_think_only_completion_is_not_resubmitted() {
        let (events, generates) = run_scripted_turn(&[
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
            let (events, generates) = run_scripted_turn(&[reply]);
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

    #[test]
    fn discovery_waits_for_a_preferred_model_not_on_the_first_node() {
        let calls = std::cell::Cell::new(0usize);
        let (provider, route) = fleet_provider_with(
            &|_| {},
            Some("qwen3.6-27b"),
            None,
            None,
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
        )
        .unwrap();
        assert_eq!(
            route,
            ChatRoute::Fleet {
                base: "http://n2:2".into(),
                model: "qwen3.6-27b".into(),
            }
        );
        drop(provider);
        assert!(calls.get() >= 2);
    }

    #[test]
    fn this_machines_hub_is_elected_before_a_lan_node_listed_first_alphabetically() {
        // Both serve a chat model; the caller ranked loopback first.
        let (_provider, route) = fleet_provider_with(
            &|_| {},
            None,
            None,
            None,
            || vec!["http://127.0.0.1:8123".into(), "http://10.0.0.235:8123".into(), "http://127.0.0.1:8123".into()],
            || DelayedModels,
            Duration::from_secs(1),
            Duration::ZERO,
        )
        .unwrap();
        assert!(matches!(route, ChatRoute::Fleet { ref base, .. } if base == "http://127.0.0.1:8123"), "{route:?}");
    }

    #[test]
    fn a_named_model_that_no_node_serves_is_an_error_not_another_model() {
        let result = fleet_provider_with(
            &|_| {},
            Some("qwen3.8-27b"),
            None,
            None,
            || vec!["http://n1:1".into()],
            || DelayedModels,
            Duration::from_millis(50),
            Duration::ZERO,
        );
        assert!(result.is_err());
    }
}

//! The worker thread: owns the model session, runs the tool loop, and turns
//! the hub's `ChatEvent`s into `AgentEvent`s.
//!
//! Everything blocking (a model turn, a running command, an approval, a
//! question) waits in a loop that keeps draining the command channel, so
//! `Interrupt`, `Approve` and `Answer` always get through.

use super::context::{self, Item};
use super::event::*;
use super::hub_context::{ContextHub, Node};
use super::scope_log::ScopeLog;
use super::session_store::{self, Saved};
use super::{prompt, tools};
use makepad_ai_hub::hub_chat::{ChatReach, ChatRoute, ContextOverflow, HubChatConfig, HubChatSession};
use makepad_ai_hub::local_llm::{ChatEvent, LocalLlmConfig, ToolSpec};
use makepad_ai_hub::providers::provider::ToolImage;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(25);
/// Fold old turns into a summary when the context passes this share.
const COMPACT_AT: f32 = 0.70;
/// The verbatim tail kept after a fold, as a share of the window.
const TAIL_SHARE: f32 = 0.20;
/// Attached files of one request, as a share of the window.
const REFS_SHARE: f32 = 0.25;
const SUMMARY_PATIENCE: Duration = Duration::from_secs(180);

pub(super) enum Reply {
    Decision(Decision),
    Answer(String),
}

trait ChatSession: Send {
    fn send_user_turn(&self, text: String);
    fn send_tool_results(&self, results: Vec<(String, bool)>);
    fn attach_images(&self, images: Vec<ToolImage>) -> Result<(), String>;
    fn cancel(&self);
    fn poll(&self) -> Vec<ChatEvent>;
    fn route(&self) -> Option<ChatRoute>;
    fn take_overflow(&self) -> Option<ContextOverflow>;
}

impl ChatSession for HubChatSession {
    fn send_user_turn(&self, text: String) { HubChatSession::send_user_turn(self, text) }
    fn send_tool_results(&self, results: Vec<(String, bool)>) { HubChatSession::send_tool_results(self, results) }
    fn attach_images(&self, images: Vec<ToolImage>) -> Result<(), String> { HubChatSession::attach_images(self, images) }
    fn cancel(&self) { HubChatSession::cancel(self) }
    fn poll(&self) -> Vec<ChatEvent> { HubChatSession::poll(self) }
    fn route(&self) -> Option<ChatRoute> { HubChatSession::route(self) }
    fn take_overflow(&self) -> Option<ContextOverflow> { HubChatSession::take_overflow(self) }
}

/// One conversation with the model: the main agent, or a sub-agent.
pub(super) struct Lane {
    pub id: LaneId,
    pub read_only: bool,
    session: Box<dyn ChatSession>,
    pub specs: Vec<ToolSpec>,
    /// What the model has seen (summary first, then the verbatim turns).
    pub items: Vec<Item>,
    pub system_chars: usize,
    pub notes_chars: usize,
    /// What the session holds beyond `items`' own text: the full tool-call arguments (an item
    /// keeps a 240-char brief of them, the model's reply carried them whole).
    pub call_chars: usize,
    /// The session lost its history (a failed route re-elects with an empty
    /// one): the next request starts a fresh session seeded from `items`.
    pub needs_reseed: bool,
}

/// The prompt and tools that will be installed together in a fresh session.
struct LaneSystem {
    base: String,
    specs: Vec<ToolSpec>,
    chars: usize,
}

/// How a model turn (and the tool rounds inside it) ended.
#[derive(Debug, PartialEq)]
pub(super) enum End {
    /// The model answered without asking for more tools; its last text.
    Done(String),
    Interrupted,
    Failed,
}

enum Fold {
    Folded { before: u32, after: u32 },
    NothingToFold,
    FreesNothing { before: u32, after: u32 },
}

impl Fold {
    fn folded(&self) -> bool {
        matches!(self, Self::Folded { .. })
    }
}

pub(super) struct Engine {
    pub cfg: AgentConfig,
    /// Explicit request passed to new sessions, independent of the fitted window.
    context_override: Option<u32>,
    /// Latest known window. Zero until the hub reports one in auto mode.
    context_tokens: u32,
    cmd_rx: Receiver<Command>,
    evt_tx: Sender<AgentEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    queued: VecDeque<Command>,
    replies: HashMap<ToolId, Reply>,
    pub interrupted: bool,
    pub shutdown: bool,
    pub mode: ApprovalMode,
    pub allow: HashSet<String>,
    pub repo_allow: HashSet<String>,
    pub created: HashSet<PathBuf>,
    pub undo: HashMap<ToolId, (PathBuf, Option<String>)>,
    next_tool: ToolId,
    next_lane: LaneId,
    total_generated: u64,
    tokens_per_sec: f32,
    pub notes: String,
    ready: std::cell::Cell<bool>,
    /// Stage clock of the model load in progress (ETA from its own rate).
    load_track: std::cell::RefCell<makepad_ai_services::progress_eta::ProgressTracker>,
    session_id: String,
    session_started: u64,
    title: String,
    log: std::cell::RefCell<Option<ScopeLog>>,
    hub: Box<dyn ContextHub>,
    start_chat: Box<dyn Fn(HubChatConfig) -> Box<dyn ChatSession> + Send>,
    /// The node's context report was fetched (or the node has no such op).
    ctx_known: bool,
    /// The precision from the last node report, for switch notices.
    kv: Option<String>,
    /// This engine's token on the run manager's shell commands (runs.rs):
    /// the runs it started, which it may stop without asking.
    pub owner: u64,
    /// Commands that went on running after their run_command returned.
    pub background: Vec<Background>,
    /// One line per background run that ended, for the model's next turn.
    pub run_notes: Vec<(u64, String)>,
    /// Per run: the line run_output shows from next.
    pub read_marks: HashMap<u64, u64>,
    /// Runs whose end the model has already seen (run_output, run_stop).
    pub ended_seen: HashSet<u64>,
    last_background_pump: Instant,
    /// Real tokens per estimated token, from the node's count in a context-overflow refusal
    /// (1.0 until one happens; code and numbers tokenize denser than prose).
    token_scale: f32,
}

/// A command still running after its tool call returned: its output goes
/// on to the tool's card until it ends.
pub(super) struct Background {
    pub tool: ToolId,
    pub run: u64,
    /// Lines already sent to the card.
    pub shown: u64,
}

/// A fresh token per engine, so two sessions' runs are told apart.
fn fresh_owner() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub(super) fn run(
    cfg: AgentConfig,
    cmd_rx: Receiver<Command>,
    evt_tx: Sender<AgentEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    hub: Box<dyn ContextHub>,
) {
    let mut eng = Engine::build(cfg, cmd_rx, evt_tx, wake, hub);
    eng.notes = prompt::machine_notes() + &prompt::load_project_notes(&eng.cfg.root);
    eng.load_repo_allow();
    eng.reopen_log();
    let mut main = eng.new_lane(0, false, "");
    eng.emit_sessions();
    eng.emit_usage(&main);
    eng.main_loop(&mut main);
    eng.end_runs("the session ended");
}

impl Engine {
    fn build(
        cfg: AgentConfig,
        cmd_rx: Receiver<Command>,
        evt_tx: Sender<AgentEvent>,
        wake: Arc<dyn Fn() + Send + Sync>,
        hub: Box<dyn ContextHub>,
    ) -> Engine {
        let mode = cfg.approval;
        Engine {
        context_override: cfg.context_tokens,
        context_tokens: cfg.context_tokens.unwrap_or(0),
        cfg,
        cmd_rx,
        evt_tx,
        wake,
        queued: VecDeque::new(),
        replies: HashMap::new(),
        interrupted: false,
        shutdown: false,
        mode,
        allow: HashSet::new(),
        repo_allow: HashSet::new(),
        created: HashSet::new(),
        undo: HashMap::new(),
        next_tool: 1,
        next_lane: 1,
        total_generated: 0,
        tokens_per_sec: 0.0,
        notes: String::new(),
        ready: std::cell::Cell::new(false),
        load_track: std::cell::RefCell::new(Default::default()),
        session_id: session_store::new_id(),
        session_started: session_store::now_secs(),
        title: String::new(),
        log: std::cell::RefCell::new(None),
        hub,
        start_chat: Box::new(|config| Box::new(HubChatSession::start(config))),
        ctx_known: false,
        kv: None,
        owner: fresh_owner(),
        background: Vec::new(),
        run_notes: Vec::new(),
        read_marks: HashMap::new(),
        ended_seen: HashSet::new(),
        last_background_pump: Instant::now(),
            token_scale: 1.0,
        }
    }

    /// An engine without a model for the scripted agent (`mock`): its tools
    /// run for real and report through `evt_tx` like the core's. Its tool ids
    /// start at 1000, clear of the script's own.
    pub(crate) fn tools_only(cfg: AgentConfig, evt_tx: Sender<AgentEvent>, wake: Arc<dyn Fn() + Send + Sync>) -> Engine {
        let (_commands, cmd_rx) = std::sync::mpsc::channel();
        let mut eng = Engine::build(cfg, cmd_rx, evt_tx, wake, Box::new(super::hub_context::NodeHub));
        eng.next_tool = 1000;
        eng
    }

    fn main_loop(&mut self, main: &mut Lane) {
        while !self.shutdown {
            let command = match self.queued.pop_front() {
                Some(c) => Some(c),
                None => match self.cmd_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(c) => Some(c),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                },
            };
            match command {
                None => {
                    if !self.ctx_known {
                        self.query_context(main);
                    }
                    self.drain_idle(main);
                    self.pump_background();
                }
                Some(Command::Submit { text, refs }) => {
                    self.interrupted = false;
                    self.submit(main, text, refs);
                    self.interrupted = false;
                    self.emit(AgentEvent::Idle);
                }
                Some(Command::NewSession) => {
                    self.end_runs("the session ended");
                    self.session_id = session_store::new_id();
                    self.session_started = session_store::now_secs();
                    self.title.clear();
                    *main = self.new_lane(0, false, "");
                    self.reopen_log();
                    self.emit_usage(main);
                }
                Some(Command::Resume(id)) => {
                    self.end_runs("the session ended");
                    self.resume(main, &id)
                }
                Some(Command::SetApprovalMode(m)) => self.mode = m,
                Some(Command::SetContext(n)) => self.set_context(main, n),
                Some(Command::SetKv(kv)) => self.set_kv(main, kv),
                Some(Command::QueryContext) => self.query_context(main),
                Some(Command::Undo(id)) => self.undo_edit(id),
                Some(Command::Shutdown) => break,
                Some(_) => {}
            }
        }
    }
}

/// Whether a conversation of `used` tokens is past the fold threshold of a
/// `max`-token window.
pub(super) fn over_budget_at(used: u32, max: u32) -> bool {
    max > 0 && used as f32 > max as f32 * COMPACT_AT
}

fn args_brief(args: &[(String, String)]) -> String {
    let joined = args
        .iter()
        .map(|(k, v)| format!("{k}={}", v.replace('\n', "\\n")))
        .collect::<Vec<_>>()
        .join(", ");
    tools::truncate_middle(&joined, 240)
}

impl Engine {
    pub fn emit(&self, event: AgentEvent) {
        if let Some(log) = self.log.borrow_mut().as_mut() {
            log.observe(&event);
        }
        let _ = self.evt_tx.send(event);
        (self.wake)();
    }

    /// (Re)start the activity log for the current session id.
    fn reopen_log(&self) {
        let base = ScopeLog::base_for(self.cfg.session_dir.as_deref());
        *self.log.borrow_mut() =
            base.map(|b| ScopeLog::new(&b, &self.cfg.root, &self.session_id, &self.title));
    }

    pub fn root(&self) -> &PathBuf {
        &self.cfg.root
    }

    pub fn fresh_tool_id(&mut self) -> ToolId {
        let id = self.next_tool;
        self.next_tool += 1;
        id
    }

    pub fn fresh_lane_id(&mut self) -> LaneId {
        let id = self.next_lane;
        self.next_lane += 1;
        id
    }

    fn fresh_system(&self, id: LaneId, read_only: bool) -> LaneSystem {
        let specs = tools::specs(id != 0, read_only);
        let root = self.cfg.root.display().to_string();
        let base = if id == 0 {
            prompt::system_prompt(&root, &self.notes)
        } else {
            prompt::sub_agent_prompt(&root, read_only, &self.notes)
        };
        let tools_chars: usize = specs.iter().map(|t| t.name.len() + t.description.len() + t.parameters.len() + 20).sum();
        let chars = base.chars().count() + tools_chars;
        LaneSystem { base, specs, chars }
    }

    /// Start a model session for a lane (0 = the main agent). `seed` is text
    /// carried in the system turn: a summary and recent turns.
    pub fn new_lane(&self, id: LaneId, read_only: bool, seed: &str) -> Lane {
        self.start_lane(id, read_only, seed, self.fresh_system(id, read_only))
    }

    fn start_lane(&self, id: LaneId, read_only: bool, seed: &str, system: LaneSystem) -> Lane {
        let LaneSystem { base: mut prompt, specs, chars } = system;
        if !seed.is_empty() {
            prompt.push('\n');
            prompt.push_str(seed);
        }
        let session = self.start_session(prompt, &specs);
        Lane {
            id,
            read_only,
            session,
            specs,
            items: Vec::new(),
            // The seed is counted through the lane's items, not here.
            system_chars: chars,
            notes_chars: self.notes.chars().count(),
            call_chars: 0,
            needs_reseed: false,
        }
    }

    fn start_session(&self, system: String, specs: &[ToolSpec]) -> Box<dyn ChatSession> {
        let mut llm = LocalLlmConfig::new(PathBuf::new());
        llm.max_context = self.context_override;
        (self.start_chat)(HubChatConfig {
            reach: ChatReach::Lan,
            llm,
            preferred_model: self.cfg.model.clone(),
            max_tokens: Some(8192),
            thinking: Some(false),
            claude_model: None,
            system_prompt: system,
            tools: specs.to_vec(),
            wake: Some(self.wake.clone()),
        })
    }

    /// Background runs: new output to their cards, and an end becomes a
    /// `RunEnded` plus a note for the model's next turn. At most every 100 ms.
    pub fn pump_background(&mut self) {
        self.follow_background(false);
    }

    /// `pump_background` without its 100 ms spacing.
    pub(super) fn pump_background_now(&mut self) {
        self.follow_background(true);
    }

    fn follow_background(&mut self, now: bool) {
        if self.background.is_empty() || (!now && self.last_background_pump.elapsed() < Duration::from_millis(100)) {
            return;
        }
        self.last_background_pump = Instant::now();
        let m = super::runs::RunManager::global();
        let mut ended = Vec::new();
        let mut events = Vec::new();
        for (i, bg) in self.background.iter_mut().enumerate() {
            let Some(view) = m.shell(bg.run) else {
                ended.push(i);
                continue;
            };
            // The end is read before the lines, so no line is missed.
            let exit = view.exit.clone();
            if let Some((lines, next)) = m.shell_lines(bg.run, bg.shown) {
                bg.shown = next;
                if !lines.is_empty() {
                    let text = lines.join("\n") + "\n";
                    events.push(AgentEvent::ToolOutput { id: bg.tool, text });
                }
            }
            if let Some(exit) = exit {
                let secs = view.elapsed.as_secs_f32();
                let all = m.shell_lines(bg.run, 0).map(|(l, _)| l.join("\n")).unwrap_or_default();
                let summary = super::output::one_line(&all);
                events.push(AgentEvent::RunEnded { id: bg.tool, run: bg.run, exit: exit.text(), ok: exit.success(), secs, summary: summary.clone() });
                if !self.ended_seen.contains(&bg.run) {
                    let how = match &exit {
                        super::runs::ShellExit::Code(c) => format!("exited with code {c}"),
                        other => other.text(),
                    };
                    self.run_notes.push((bg.run, format!("run {} {how} after {secs:.1} s: {summary}", bg.run)));
                }
                ended.push(i);
            }
        }
        for i in ended.into_iter().rev() {
            self.background.remove(i);
        }
        for event in events {
            self.emit(event);
        }
    }

    /// The notes about ended background runs, as text for the model's next
    /// turn (empty when there are none).
    pub fn take_run_notes(&mut self) -> String {
        self.pump_background_now();
        let notes: Vec<String> = self.run_notes.drain(..).filter(|(run, _)| !self.ended_seen.contains(run)).map(|(_, n)| n).collect();
        notes.iter().map(|n| format!("[background {n}]\n")).collect()
    }

    /// The session ends: stop the commands it started, forget their notes.
    pub fn end_runs(&mut self, why: &str) {
        super::runs::RunManager::global().stop_shells_of(self.owner, why);
        // Their cards still hear the end; the next session's model does not.
        for bg in &self.background {
            self.ended_seen.insert(bg.run);
        }
        self.pump_background_now();
        self.run_notes.clear();
    }

    /// Commands that arrive while something blocks: act on the ones that
    /// matter now, hold the rest for the main loop.
    pub fn pump_commands(&mut self) {
        self.pump_background();
        while let Ok(command) = self.cmd_rx.try_recv() {
            match command {
                Command::Interrupt => self.interrupted = true,
                Command::Approve { id, decision } => {
                    self.replies.insert(id, Reply::Decision(decision));
                }
                Command::Answer { id, text } => {
                    self.replies.insert(id, Reply::Answer(text));
                }
                Command::SetApprovalMode(m) => self.mode = m,
                Command::Undo(id) => self.undo_edit(id),
                Command::Shutdown => {
                    self.shutdown = true;
                    self.interrupted = true;
                }
                other => self.queued.push_back(other),
            }
        }
    }

    /// Block until the front end answers `id` (or interrupts / shuts down).
    pub fn await_reply(&mut self, id: ToolId) -> Option<Reply> {
        loop {
            self.pump_commands();
            if let Some(reply) = self.replies.remove(&id) {
                return Some(reply);
            }
            if self.interrupted || self.shutdown {
                return None;
            }
            std::thread::sleep(POLL);
        }
    }

    /// Session events that arrive between turns: load progress, readiness.
    fn drain_idle(&self, lane: &Lane) {
        for event in lane.session.poll() {
            self.status_event(lane, &event);
        }
    }

    fn route_label(&self, lane: &Lane) -> String {
        lane.session.route().map(|r| r.label()).unwrap_or_else(|| "finding a model".into())
    }

    fn status_event(&self, lane: &Lane, event: &ChatEvent) {
        match event {
            ChatEvent::Loading { phase, fraction } => {
                let view = self.load_track.borrow_mut().observe(phase, if *fraction > 0.0 { *fraction } else { -1.0 }, std::time::Instant::now());
                // Per-turn "reading new input" progress is noise once ready;
                // a weight load or swap is not.
                if self.ready.get() && view.stage == "Warming up" {
                    return;
                }
                self.emit(AgentEvent::Status(Status {
                    route: self.route_label(lane),
                    ready: self.ready.get(),
                    detail: view.line(),
                    load: Some(view),
                }))
            }
            ChatEvent::Ready { .. } => {
                self.ready.set(true);
                self.load_track.borrow_mut().reset();
                self.emit(AgentEvent::Status(Status {
                    route: self.route_label(lane),
                    ready: true,
                    detail: String::new(),
                    load: None,
                }))
            }
            ChatEvent::Failed(why) => {
                self.ready.set(false);
                self.emit(AgentEvent::Status(Status {
                    route: self.route_label(lane),
                    ready: false,
                    detail: why.clone(),
                    load: None,
                }));
                self.emit(AgentEvent::Error(why.clone()));
            }
            // Anything else after a load means the load is over (the first
            // token, a tool call, an error): clear the bar once.
            _ if self.load_track.borrow().current().is_some() => {
                self.load_track.borrow_mut().reset();
                self.emit(AgentEvent::Status(Status { route: self.route_label(lane), ready: self.ready.get(), detail: String::new(), load: None }));
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ context

    pub fn emit_usage(&self, lane: &Lane) {
        if lane.id != 0 {
            return;
        }
        self.emit(AgentEvent::Usage(Usage {
            context_used: self.used_tokens(lane),
            context_max: self.context_tokens,
            tokens_per_sec: self.tokens_per_sec,
            total_generated: self.total_generated,
            segments: context::segments(lane.system_chars, lane.notes_chars, &lane.items),
        }));
    }

    /// The lane's context as the node counts it, estimated: our chars/3.5 estimate times what
    /// the node's own count showed the estimate to miss (`token_scale`, learnt from an overflow).
    fn used_tokens(&self, lane: &Lane) -> u32 {
        self.estimated_tokens(lane.system_chars + lane.call_chars, &lane.items)
    }

    fn estimated_tokens(&self, system_chars: usize, items: &[Item]) -> u32 {
        (context::total_tokens(system_chars, items) as f32 * self.token_scale).ceil() as u32
    }

    fn over_budget(&self, lane: &Lane) -> bool {
        over_budget_at(self.used_tokens(lane), self.context_tokens)
    }

    /// Where the lane's chat runs; `None` while the election is pending or
    /// when the weights are in this process.
    fn node_of(lane: &Lane) -> Option<Node> {
        match lane.session.route()? {
            ChatRoute::Fleet { base, model } => Some(Node { base, model: Some(model) }),
            ChatRoute::MachineNode { port } => Some(Node { base: format!("http://127.0.0.1:{port}"), model: None }),
            ChatRoute::InProcess { .. } => None,
        }
    }

    /// Ask the node what it offers and tell the front end. A node without the
    /// op stays silent here (the chip then offers nothing to switch). Waits
    /// for the election, then asks once.
    fn query_context(&mut self, lane: &Lane) {
        let node = Self::node_of(lane);
        if node.is_none() && lane.session.route().is_none() {
            return;
        }
        self.ctx_known = true;
        self.fetch_context(node, lane);
    }

    fn fetch_context(&mut self, node: Option<Node>, lane: &Lane) {
        if let Ok(report) = self.hub.report(node.as_ref()) {
            self.adopt_report(&report);
            self.emit(AgentEvent::ContextReport(report));
            self.emit_usage(lane);
        }
    }

    fn adopt_report(&mut self, report: &ContextReport) {
        if report.current > 0 {
            self.context_tokens = report.current;
        }
        self.kv = report.kv.clone();
    }

    /// Switch the node to `tokens` and keep the conversation: the node first
    /// (a refusal leaves everything as it was), then a fold when the
    /// conversation no longer fits the new budget, then the new max in `Usage`.
    pub fn set_context(&mut self, lane: &mut Lane, tokens: u32) {
        self.emit(AgentEvent::Notice(format!("Switching the node to a {} context...", short_tokens(tokens))));
        // A switch asked for right at start waits for the election.
        let waited = Instant::now();
        while lane.session.route().is_none() && waited.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(POLL);
        }
        let node = Self::node_of(lane);
        match self.hub.set_context(node.as_ref(), tokens) {
            Err(why) => {
                self.fetch_context(node, lane);
                let kept = if self.context_tokens > 0 { format!("at {}", short_tokens(self.context_tokens)) } else { "unchanged".into() };
                eprintln!("code: context stays {kept}: {why}");
                self.emit(AgentEvent::Error(format!("context stays {kept}: {why}")));
            }
            Ok(report) => {
                self.context_override = Some(tokens);
                self.context_tokens = if report.current > 0 { report.current } else { tokens };
                self.adopt_report(&report);
                let folded = self.over_budget(lane) && self.compact(lane, true).folded();
                let now = self.context_tokens;
                let mut note = format!("Context is now {}", short_tokens(now));
                if now != tokens {
                    note.push_str(&format!(" (fitted from {} to GPU memory)", short_tokens(tokens)));
                }
                if folded {
                    note.push_str("; older turns were folded into a summary to fit");
                }
                note.push('.');
                self.emit(AgentEvent::Notice(note));
                self.emit(AgentEvent::ContextReport(report));
                self.emit_usage(lane);
            }
        }
    }

    /// The share of the window the files attached to one request may take.
    fn refs_budget_chars(&self) -> usize {
        if self.context_tokens == 0 { return usize::MAX; }
        (self.context_tokens as f32 * REFS_SHARE * 3.5) as usize
    }

    /// Switch KV precision, keeping the conversation and adopting whatever
    /// context size and memory estimates the node reports after the switch.
    pub fn set_kv(&mut self, lane: &mut Lane, kv: String) {
        let label = kv_label(&kv);
        self.emit(AgentEvent::Notice(format!("Switching the node's KV cache to {label}...")));
        // A switch asked for right at start waits for the election.
        let waited = Instant::now();
        while lane.session.route().is_none() && waited.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(POLL);
        }
        let node = Self::node_of(lane);
        match self.hub.set_kv(node.as_ref(), &kv) {
            Err(why) => {
                let old = self.kv.as_deref().map_or_else(|| "unchanged".to_string(), |kv| format!("at {}", kv_label(kv)));
                eprintln!("code: KV cache stays {old}: {why}");
                self.emit(AgentEvent::Error(format!("KV cache stays {old}: {why}")));
                self.fetch_context(node, lane);
            }
            Ok(report) => {
                self.adopt_report(&report);
                self.emit(AgentEvent::Notice(format!("KV cache is now {label}.")));
                self.emit(AgentEvent::ContextReport(report));
                self.emit_usage(lane);
            }
        }
    }

    fn tail_chars(&self) -> usize {
        if self.context_tokens == 0 { return usize::MAX; }
        (self.context_tokens as f32 * TAIL_SHARE * 3.5) as usize
    }

    /// Ask the model (in its current session, whose history it still has) for
    /// a summary. `None` when it cannot answer; the caller falls back.
    fn ask_summary(&mut self, lane: &mut Lane) -> Option<String> {
        lane.session.send_user_turn(context::SUMMARY_REQUEST.to_string());
        let started = Instant::now();
        let mut text = String::new();
        loop {
            self.pump_commands();
            if self.interrupted || started.elapsed() > SUMMARY_PATIENCE {
                lane.session.cancel();
                return None;
            }
            let events = lane.session.poll();
            if events.is_empty() {
                std::thread::sleep(POLL);
                continue;
            }
            for event in events {
                match event {
                    ChatEvent::Delta(t) => text.push_str(&t),
                    ChatEvent::TurnDone { .. } => {
                        let t = text.trim().to_string();
                        return if t.is_empty() { None } else { Some(t) };
                    }
                    ChatEvent::Failed(_) | ChatEvent::ContextFull => return None,
                    _ => {}
                }
            }
        }
    }

    /// Continue in a fresh session only when folding the old items saves tokens.
    fn compact(&mut self, lane: &mut Lane, ask_model: bool) -> Fold {
        let cut = context::tail_start(&lane.items, self.tail_chars());
        if cut == 0 || (cut == 1 && matches!(lane.items.first(), Some(Item::Summary(_)))) {
            return Fold::NothingToFold;
        }
        let before = self.used_tokens(lane);
        let system = self.fresh_system(lane.id, lane.read_only);
        let mut items = vec![Item::Summary(String::new())];
        // The request being worked on stays word for word.
        if let Some(last_user) = lane.items.iter().rposition(|i| matches!(i, Item::User { .. })) {
            if last_user < cut {
                items.push(lane.items[last_user].clone());
            }
        }
        items.extend(lane.items[cut..].iter().cloned());
        // A fresh session counts its seed through items and starts with zero call_chars.
        // If its tail alone costs too much, leave the current session's pending turn alone.
        let tail_cost = self.estimated_tokens(system.chars, &items[1..]);
        if tail_cost >= before {
            return Fold::FreesNothing { before, after: tail_cost };
        }
        let asked_session = ask_model && !lane.needs_reseed;
        let asked = if asked_session { self.ask_summary(lane) } else { None };
        let asked_cost = asked.map(|summary| {
            items[0] = Item::Summary(summary);
            self.estimated_tokens(system.chars, &items)
        });
        let after = match asked_cost {
            Some(cost) if cost < before => cost,
            _ => {
                items[0] = Item::Summary(context::mechanical_summary(&lane.items[..cut], 2000));
                self.estimated_tokens(system.chars, &items)
            }
        };
        if after >= before {
            if asked_session {
                lane.needs_reseed = true;
            }
            return Fold::FreesNothing { before, after };
        }
        self.restart(lane, items, system);
        if lane.id == 0 {
            self.emit(AgentEvent::Summarised { upto: cut, tokens_saved: before - after });
        }
        self.emit_usage(lane);
        Fold::Folded { before, after }
    }

    /// Replace the lane's session by a fresh one seeded with `items`.
    fn restart(&self, lane: &mut Lane, items: Vec<Item>, system: LaneSystem) {
        let (summary, tail) = match items.first() {
            Some(Item::Summary(s)) => (Some(s.clone()), &items[1..]),
            _ => (None, &items[..]),
        };
        let seed = context::seed_text(summary.as_deref(), tail);
        let mut fresh = self.start_lane(lane.id, lane.read_only, &seed, system);
        fresh.items = items;
        lane.session.cancel();
        *lane = fresh;
    }

    fn reseed(&self, lane: &mut Lane) {
        let items = std::mem::take(&mut lane.items);
        let system = self.fresh_system(lane.id, lane.read_only);
        self.restart(lane, items, system);
    }

    // -------------------------------------------------------------- turns

    fn submit(&mut self, lane: &mut Lane, text: String, refs: Vec<String>) {
        if self.over_budget(lane) {
            self.compact(lane, true);
        }
        if lane.needs_reseed {
            self.reseed(lane);
        }
        if self.title.is_empty() {
            self.title = text.lines().next().unwrap_or("").chars().take(60).collect();
            if let Some(log) = self.log.borrow_mut().as_mut() {
                log.set_title(&self.title);
            }
        }
        if let Some(log) = self.log.borrow_mut().as_mut() {
            log.user(&text);
        }
        // Attached files go first, then the request; together at most a
        // quarter of the window.
        let context = context::refs_context(&self.cfg.root, &refs, self.refs_budget_chars());
        // Pictures go to the model with the turn (the chat session queues them
        // ahead of it): a screenshot dropped on the chat is looked at.
        let pictures: Vec<_> = refs
            .iter()
            .filter(|r| super::pictures::is_picture_ref(r))
            .take(super::pictures::MAX_PER_MESSAGE)
            .filter_map(|r| super::pictures::load(r).ok())
            .collect();
        if !pictures.is_empty() {
            if let Err(why) = lane.session.attach_images(pictures) {
                self.emit(AgentEvent::Notice(format!("the model cannot be shown the picture: {why}")));
            }
        }
        // Background runs that ended since the last turn come first.
        let context = self.take_run_notes() + &context;
        lane.items.push(Item::User { text: text.clone(), refs, context: context.clone() });
        let mut sent = context;
        sent.push_str(&tools::expand_mentions(&self.cfg.root, &text));
        self.turn(lane, sent);
        self.emit_usage(lane);
        self.save(lane);
    }

    fn save(&self, lane: &Lane) {
        let Some(dir) = &self.cfg.session_dir else { return };
        if lane.items.is_empty() {
            return;
        }
        let saved = Saved {
            id: self.session_id.clone(),
            title: self.title.clone(),
            started: self.session_started,
            items: lane.items.clone(),
        };
        match session_store::save(dir, &saved) {
            Ok(()) => {
                self.emit(AgentEvent::SessionSaved(self.session_id.clone()));
                self.emit_sessions();
            }
            Err(e) => self.emit(AgentEvent::Error(format!("cannot save the session: {e}"))),
        }
    }

    fn emit_sessions(&self) {
        if let Some(dir) = &self.cfg.session_dir {
            self.emit(AgentEvent::Sessions(session_store::list(dir)));
        }
    }

    fn resume(&mut self, lane: &mut Lane, id: &str) {
        let Some(dir) = self.cfg.session_dir.clone() else {
            return self.emit(AgentEvent::Error("sessions are not saved (no session folder)".into()));
        };
        let saved = match session_store::load(&dir, id) {
            Ok(s) => s,
            Err(e) => return self.emit(AgentEvent::Error(e)),
        };
        self.session_id = saved.id;
        self.session_started = saved.started;
        self.title = saved.title;
        let mut items = saved.items;
        // A long session is folded to fit before it is seeded.
        let cut = context::tail_start(&items, self.tail_chars());
        if cut > 0 {
            let summary = context::mechanical_summary(&items[..cut], 2000);
            let mut folded = vec![Item::Summary(summary)];
            folded.extend(items[cut..].iter().cloned());
            items = folded;
        }
        let system = self.fresh_system(lane.id, lane.read_only);
        self.restart(lane, items, system);
        self.reopen_log();
        let shown = lane
            .items
            .iter()
            .map(|i| match i {
                Item::User { text, refs, .. } => RestoredItem::User { text: text.clone(), refs: refs.clone() },
                Item::Assistant(t) => RestoredItem::Assistant(t.clone()),
                Item::Summary(t) => RestoredItem::Summary(t.clone()),
                Item::Tool { name, args, result, ok } => {
                    RestoredItem::Tool { title: format!("{name} {args}"), ok: *ok, output: result.clone() }
                }
            })
            .collect();
        self.emit(AgentEvent::Restored(shown));
        self.emit_usage(lane);
    }

    /// Run a helper on one task in a fresh context, to completion. Returns
    /// (ok, the helper's final text).
    pub fn run_sub_agent(&mut self, parent: LaneId, task: &str, read_only: bool) -> (bool, String) {
        let id = self.fresh_lane_id();
        let title: String = task.lines().next().unwrap_or("").chars().take(80).collect();
        self.emit(AgentEvent::LaneStarted { lane: id, parent, title });
        let mut sub = self.new_lane(id, read_only, "");
        sub.items.push(Item::user(task));
        let end = self.turn(&mut sub, task.to_string());
        sub.session.cancel();
        let (ok, text) = match end {
            End::Done(text) if !text.trim().is_empty() => (true, text.trim().to_string()),
            End::Done(_) => (false, "the helper finished without an answer".to_string()),
            End::Interrupted => (false, "the helper was interrupted".to_string()),
            End::Failed => (false, "the helper's model session failed".to_string()),
        };
        let text = tools::truncate_middle(&text, 3500);
        self.emit(AgentEvent::LaneFinished { lane: id, summary: text.lines().next().unwrap_or("").chars().take(200).collect() });
        (ok, text)
    }

    fn record_turn_done(&mut self, tokens: usize, secs: f64, context_max: usize) {
        if context_max > 0 {
            self.context_tokens = context_max.min(u32::MAX as usize) as u32;
        }
        self.total_generated += tokens as u64;
        if secs > 0.0 && tokens > 0 {
            self.tokens_per_sec = tokens as f32 / secs as f32;
        }
    }

    fn recover_overflow(&mut self, lane: &mut Lane, round: &mut usize, overflow: Option<ContextOverflow>) -> bool {
        let Some(ContextOverflow { need, max }) = overflow else { return false };
        *round += 1;
        if max > 0 {
            self.context_tokens = max.min(u32::MAX as usize) as u32;
        }
        let estimate = context::total_tokens(lane.system_chars + lane.call_chars, &lane.items).max(1);
        self.token_scale = (need as f32 / estimate as f32).clamp(self.token_scale, 4.0);
        let fold = self.compact(lane, false);
        let (before, after, outcome) = match fold {
            Fold::Folded { before, after } => (before, after, "retrying"),
            Fold::FreesNothing { before, after } => (before, after, "the fold freed nothing"),
            Fold::NothingToFold => {
                let used = self.used_tokens(lane);
                (used, used, "nothing left to fold")
            }
        };
        eprintln!("code: overflow round {round}: used {before} -> {after}, node need {need}, max {max}: {outcome}");
        if !fold.folded() {
            self.emit_usage(lane);
            return false;
        }
        self.emit(AgentEvent::Notice("The conversation outgrew the model's context: older turns were folded into a summary.".into()));
        lane.session.send_user_turn("Continue the task. The latest tool results are in the recent turns above.".into());
        true
    }

    /// One user request: model turns and tool rounds until the model stops
    /// asking for tools. Returns how it ended.
    pub fn turn(&mut self, lane: &mut Lane, text: String) -> End {
        lane.session.send_user_turn(text);
        let mut nudged = false;
        let mut overflow_round = 0;
        let mut rounds = 0usize;
        let mut seen: HashMap<String, u32> = HashMap::new();
        loop {
            let mut calls: Vec<(String, Vec<(String, String)>)> = Vec::new();
            let mut answer = String::new();
            let mut overflowed = false;
            // Wait for this model turn to finish.
            loop {
                self.pump_commands();
                if self.interrupted {
                    lane.session.cancel();
                    // The worker drops a cancelled turn without a closing
                    // event; give stragglers a moment, then discard them.
                    std::thread::sleep(Duration::from_millis(250));
                    let _ = lane.session.poll();
                    if !answer.trim().is_empty() {
                        lane.items.push(Item::Assistant(answer));
                    }
                    return End::Interrupted;
                }
                let events = lane.session.poll();
                if events.is_empty() {
                    std::thread::sleep(POLL);
                    continue;
                }
                let mut finished = false;
                for event in events {
                    match event {
                        ChatEvent::Delta(text) => {
                            answer.push_str(&text);
                            self.emit(AgentEvent::Delta { lane: lane.id, text });
                        }
                        ChatEvent::Working(_) => {}
                        ChatEvent::ToolCall { name, args } => calls.push((name, args)),
                        ChatEvent::TurnDone { tokens, secs, context_max, .. } => {
                            self.record_turn_done(tokens, secs, context_max);
                            finished = true;
                        }
                        ChatEvent::ContextFull => {
                            self.emit(AgentEvent::Error("the context window is full".into()));
                            lane.needs_reseed = true;
                            return End::Failed;
                        }
                        other @ (ChatEvent::Loading { .. } | ChatEvent::Ready { .. }) => {
                            self.status_event(lane, &other)
                        }
                        ChatEvent::Failed(why) => {
                            // The route re-elects with an empty history.
                            lane.needs_reseed = true;
                            let overflow = lane.session.take_overflow();
                            if self.recover_overflow(lane, &mut overflow_round, overflow) {
                                overflowed = true;
                                break;
                            }
                            self.status_event(lane, &ChatEvent::Failed(why));
                            return End::Failed;
                        }
                    }
                }
                if finished || overflowed {
                    break;
                }
            }
            if overflowed {
                continue;
            }
            if !answer.trim().is_empty() {
                lane.items.push(Item::Assistant(answer.clone()));
            }
            self.emit_usage(lane);
            if calls.is_empty() {
                // The model sometimes ends a turn on "I'll add the test:" with
                // no call. Once per request, tell it to act.
                if !nudged {
                    if let Some(why) = nudge_for(&answer) {
                        nudged = true;
                        lane.session.send_user_turn(why.to_string());
                        continue;
                    }
                }
                return End::Done(answer);
            }
            rounds += 1;
            if rounds > MAX_ROUNDS {
                self.emit(AgentEvent::Error(format!("stopped after {MAX_ROUNDS} tool rounds without finishing; say continue to go on")));
                return End::Done(String::new());
            }
            let mut results = Vec::with_capacity(calls.len());
            for (name, args) in calls {
                let (mut result, is_error) = self.run_tool(lane.id, &name, &args);
                let n = seen.entry(format!("{name} {}", args_brief(&args))).or_insert(0);
                *n += 1;
                if *n >= 3 && !is_error {
                    result.push_str(&format!("\n[you have made this exact call {n} times in this task and the result does not change; do something different]"));
                }
                let brief = args_brief(&args);
                let full: usize = args.iter().map(|(k, v)| k.len() + v.chars().count() + 4).sum();
                lane.call_chars += full.saturating_sub(brief.chars().count());
                lane.items.push(Item::Tool { name, args: brief, result: result.clone(), ok: !is_error });
                results.push((result, is_error));
                if self.interrupted {
                    break;
                }
            }
            if self.interrupted {
                return End::Interrupted;
            }
            // Background runs that ended meanwhile ride on the last result.
            let notes = self.take_run_notes();
            if !notes.is_empty() {
                if let Some(last) = results.last_mut() {
                    last.0.push('\n');
                    last.0.push_str(&notes);
                }
                if let Some(Item::Tool { result, .. }) = lane.items.last_mut() {
                    result.push('\n');
                    result.push_str(&notes);
                }
            }
            self.emit_usage(lane);
            if self.over_budget(lane) && self.compact(lane, true).folded() {
                continue_after_restart(lane);
            } else if lane.needs_reseed {
                // The summary request abandoned the pending tool turn.
                self.reseed(lane);
                continue_after_restart(lane);
            } else {
                lane.session.send_tool_results(results);
            }
        }
    }
}

/// Most tool rounds one request may take before it stops and asks.
const MAX_ROUNDS: usize = 80;
const NUDGE_ANNOUNCED: &str = "You described the next step but made no tool call. Make the tool call now. If the task is already finished, say so and summarize the result.";
const NUDGE_CUT: &str = "Your last tool call was cut off or malformed, so nothing ran. Send it again, shorter: split a big edit into several smaller edit_file calls.";

fn nudge_for(answer: &str) -> Option<&'static str> {
    if answer.contains("[bad tool call") {
        Some(NUDGE_CUT)
    } else if announces_work(answer) {
        Some(NUDGE_ANNOUNCED)
    } else {
        None
    }
}

/// A short final text that only announces work ("I'll add a test:").
fn announces_work(answer: &str) -> bool {
    let t = answer.trim();
    if t.is_empty() || t.chars().count() > 300 {
        return false;
    }
    let low = t.to_lowercase();
    t.ends_with(':')
        || ["i'll ", "i will ", "let me ", "now i", "next, i", "i'm going to ", "i need to "].iter().any(|p| low.starts_with(p))
}

/// The fresh session has the tool results in its seed and needs a new user turn.
fn continue_after_restart(lane: &Lane) {
    lane.session.send_user_turn("Continue the task. The latest tool results are in the recent turns above.".into());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::sync::Mutex;

    /// A node that records what it was asked and answers from a script.
    struct FakeHub {
        asked: Arc<Mutex<Vec<u32>>>,
        refuse: Option<String>,
        kv: String,
    }

    impl ContextHub for FakeHub {
        fn report(&mut self, _: Option<&Node>) -> Result<ContextReport, String> {
            if let Some(why) = &self.refuse {
                return Err(why.clone());
            }
            Ok(ContextReport { current: 32768, kv: Some(self.kv.clone()), max_fit: 49152, options: vec![] })
        }
        fn set_context(&mut self, _: Option<&Node>, tokens: u32) -> Result<ContextReport, String> {
            self.asked.lock().unwrap().push(tokens);
            match &self.refuse {
                Some(why) => Err(why.clone()),
                None => Ok(ContextReport { current: tokens.min(65_536), kv: Some(self.kv.clone()), max_fit: 65_536, options: vec![] }),
            }
        }
        fn set_kv(&mut self, node: Option<&Node>, kv: &str) -> Result<ContextReport, String> {
            match &self.refuse {
                Some(why) => Err(why.clone()),
                None => {
                    self.kv = kv.to_string();
                    self.report(node)
                }
            }
        }
    }

    #[derive(Default)]
    struct SessionRecord {
        system_prompt: String,
        user_turns: Vec<String>,
        result_batches: usize,
        cancelled: bool,
        pending_tool_turn: bool,
        events: Vec<ChatEvent>,
    }

    #[derive(Default)]
    struct ChatScript {
        replies: VecDeque<Vec<ChatEvent>>,
        sessions: Vec<SessionRecord>,
    }

    struct ScriptedSession {
        script: Arc<Mutex<ChatScript>>,
        index: usize,
    }

    impl ScriptedSession {
        fn queue_reply(&self, script: &mut ChatScript) {
            let reply = script.replies.pop_front().unwrap_or_default();
            let session = &mut script.sessions[self.index];
            session.pending_tool_turn = reply.iter().any(|event| matches!(event, ChatEvent::TurnDone { tool_calls, .. } if *tool_calls > 0));
            session.events = reply;
        }
    }

    impl ChatSession for ScriptedSession {
        fn send_user_turn(&self, text: String) {
            let mut script = self.script.lock().unwrap();
            script.sessions[self.index].user_turns.push(text);
            self.queue_reply(&mut script);
        }
        fn send_tool_results(&self, _: Vec<(String, bool)>) {
            let mut script = self.script.lock().unwrap();
            let session = &mut script.sessions[self.index];
            session.result_batches += 1;
            if session.pending_tool_turn {
                self.queue_reply(&mut script);
            }
        }
        fn attach_images(&self, _: Vec<ToolImage>) -> Result<(), String> {
            panic!("unexpected images in scripted chat")
        }
        fn cancel(&self) {
            let mut script = self.script.lock().unwrap();
            let session = &mut script.sessions[self.index];
            session.cancelled = true;
            session.pending_tool_turn = false;
        }
        fn poll(&self) -> Vec<ChatEvent> {
            let events = std::mem::take(&mut self.script.lock().unwrap().sessions[self.index].events);
            assert!(!events.is_empty(), "session {} polled without a scripted reply", self.index);
            events
        }
        fn route(&self) -> Option<ChatRoute> { Some(ChatRoute::InProcess { model: "test".into() }) }
        fn take_overflow(&self) -> Option<ContextOverflow> { None }
    }

    fn rig(context: Option<u32>, refuse: Option<&str>) -> (Engine, Lane, Receiver<AgentEvent>, Arc<Mutex<Vec<u32>>>) {
        rig_with_script(context, refuse, Arc::default())
    }

    fn rig_with_script(context: Option<u32>, refuse: Option<&str>, script: Arc<Mutex<ChatScript>>)
        -> (Engine, Lane, Receiver<AgentEvent>, Arc<Mutex<Vec<u32>>>)
    {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let (_cmd_tx, cmd_rx) = channel();
        let (evt_tx, evt_rx) = channel();
        let cfg = AgentConfig {
            root: std::env::temp_dir(),
            context_tokens: context,
            model: None,
            approval: ApprovalMode::Ask,
            session_dir: None,
        };
        let hub = FakeHub { asked: asked.clone(), refuse: refuse.map(String::from), kv: "q8".into() };
        let mut eng = Engine::build(cfg, cmd_rx, evt_tx, Arc::new(|| {}), Box::new(hub));
        eng.start_chat = Box::new(move |config| {
            let mut state = script.lock().unwrap();
            let index = state.sessions.len();
            state.sessions.push(SessionRecord { system_prompt: config.system_prompt, ..Default::default() });
            Box::new(ScriptedSession { script: script.clone(), index })
        });
        let mut lane = eng.new_lane(0, false, "");
        // No node answers in a test: fold without asking the model.
        lane.needs_reseed = true;
        (eng, lane, evt_rx, asked)
    }

    fn chat(lane: &mut Lane, turns: usize) {
        for i in 0..turns {
            lane.items.push(Item::user(format!("question {i}")));
            lane.items.push(Item::Assistant("a".repeat(2000)));
        }
    }

    fn events(rx: &Receiver<AgentEvent>) -> Vec<AgentEvent> {
        rx.try_iter().collect()
    }

    #[test]
    fn growing_the_context_moves_the_budget_and_keeps_the_conversation() {
        let (mut eng, mut lane, rx, asked) = rig(Some(16384), None);
        chat(&mut lane, 6);
        let before = lane.items.clone();
        eng.set_context(&mut lane, 65536);
        assert_eq!(*asked.lock().unwrap(), vec![65536]);
        assert_eq!(eng.context_tokens, 65536);
        assert_eq!(lane.items, before);
        let ev = events(&rx);
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Usage(u) if u.context_max == 65536)));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ContextReport(r) if r.current == 65536)));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("64k"))));
    }

    #[test]
    fn auto_context_learns_completed_turn_and_uses_the_applied_window() {
        let (mut eng, mut lane, rx, _) = rig(None, None);
        assert_eq!((eng.context_override, eng.context_tokens), (None, 0));
        assert!(!over_budget_at(100_000, 0));
        assert_eq!((eng.refs_budget_chars(), eng.tail_chars()), (usize::MAX, usize::MAX));

        eng.record_turn_done(100, 1.0, 114_688);
        eng.record_turn_done(1, 0.0, 0);
        assert_eq!((eng.context_override, eng.context_tokens), (None, 114_688));
        eng.set_context(&mut lane, 131_072);
        assert_eq!((eng.context_override, eng.context_tokens), (Some(131_072), 65_536));
        assert!(events(&rx).iter().any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("fitted from 128k"))));
    }

    #[test]
    fn shrinking_below_the_conversation_folds_it() {
        let (mut eng, mut lane, rx, _) = rig(Some(65536), None);
        chat(&mut lane, 8); // ~4.6k tokens
        assert!(!eng.over_budget(&lane));
        eng.set_context(&mut lane, 4096); // 70% = 2.9k
        assert_eq!(eng.context_tokens, 4096);
        assert!(matches!(lane.items.first(), Some(Item::Summary(_))));
        assert!(lane.items.len() < 16);
        // The fold made it fit, and the last request is still there.
        assert!(!eng.over_budget(&lane) || lane.items.len() <= 3);
        assert!(lane.items.iter().any(|i| matches!(i, Item::User { text, .. } if text == "question 7")));
        let ev = events(&rx);
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Summarised { .. })));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Usage(u) if u.context_max == 4096)));
    }

    #[test]
    fn a_refusal_leaves_the_old_size_and_the_conversation() {
        let (mut eng, mut lane, rx, _) = rig(Some(32768), Some("not enough free GPU memory"));
        chat(&mut lane, 4);
        let before = lane.items.clone();
        eng.set_context(&mut lane, 65536);
        assert_eq!(eng.context_override, Some(32768));
        assert_eq!(eng.context_tokens, 32768);
        assert_eq!(lane.items, before);
        let ev = events(&rx);
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Error(m) if m.contains("stays at 32k") && m.contains("GPU memory"))));
        assert!(ev.iter().all(|e| !matches!(e, AgentEvent::Usage(u) if u.context_max != 32768)));

        let (mut cold, mut lane, rx, _) = rig(None, Some("no model loaded"));
        cold.set_context(&mut lane, 65536);
        assert_eq!((cold.context_override, cold.context_tokens), (None, 0));
        assert!(events(&rx).iter().any(|e| matches!(e, AgentEvent::Error(m) if m == "context stays unchanged: no model loaded")));
    }

    #[test]
    fn lone_summary_gives_nothing_to_fold() {
        let script = Arc::new(Mutex::new(ChatScript::default()));
        let (mut eng, mut lane, _, _) = rig_with_script(Some(2048), None, script.clone());
        lane.needs_reseed = false;
        lane.items = vec![Item::Summary("earlier work".into()), Item::user("continue")];
        assert_eq!(context::tail_start(&lane.items, eng.tail_chars()), 1);
        assert!(matches!(eng.compact(&mut lane, true), Fold::NothingToFold));
        let state = script.lock().unwrap();
        assert_eq!(state.sessions.len(), 1);
        assert!(state.sessions[0].user_turns.is_empty() && state.sessions[0].result_batches == 0);
        assert!(!state.sessions[0].cancelled && !lane.needs_reseed);
    }

    #[test]
    fn rejected_tail_measurement_returns_frees_nothing_without_asking_model() {
        let script = Arc::new(Mutex::new(ChatScript::default()));
        let (mut eng, mut lane, _, _) = rig_with_script(Some(2048), None, script.clone());
        lane.needs_reseed = false;
        // The last assistant stays in the tail and the latest user must be kept verbatim.
        lane.items = vec![Item::user("u".repeat(1500)), Item::Assistant("a".repeat(1500))];
        let kept = lane.items.clone();
        assert!(matches!(eng.compact(&mut lane, true), Fold::FreesNothing { before, after } if before == after));
        assert_eq!(lane.items, kept);
        let state = script.lock().unwrap();
        assert_eq!(state.sessions.len(), 1);
        assert!(state.sessions[0].user_turns.is_empty() && state.sessions[0].result_batches == 0);
        assert!(!state.sessions[0].cancelled && !lane.needs_reseed);
    }

    #[test]
    fn refused_fold_during_tool_round_reseeds_and_finishes_turn() {
        let done = |tool_calls| ChatEvent::TurnDone { tool_calls, tokens: 1, secs: 1.0, context_used: 0, context_max: 3000 };
        let script = Arc::new(Mutex::new(ChatScript {
            replies: VecDeque::from([
                vec![ChatEvent::ToolCall { name: "todo".into(), args: vec![("items".into(), "[x] check".into())] }, done(1)],
                vec![ChatEvent::Delta("s".repeat(2000)), done(0)],
                vec![ChatEvent::Delta("Finished.".into()), done(0)],
            ]),
            sessions: Vec::new(),
        }));
        let (mut eng, mut lane, rx, _) = rig_with_script(Some(3000), None, script.clone());
        lane.needs_reseed = false;
        for i in 0..10 {
            lane.items.push(Item::user(format!("q{i}")));
            lane.items.push(Item::Assistant(format!("a{i}")));
        }
        lane.items.push(Item::user("u".repeat(1600)));
        lane.items.push(Item::Assistant("a".repeat(1600)));
        let original = lane.items.clone();
        assert!(eng.over_budget(&lane));
        assert_eq!(eng.turn(&mut lane, "Update the plan.".into()), End::Done("Finished.".into()));
        assert!(!lane.needs_reseed);
        assert_eq!(lane.items[..original.len()], original);
        assert_eq!(lane.items.len(), original.len() + 2);
        assert!(matches!(&lane.items[original.len()], Item::Tool { name, result, ok: true, .. } if name == "todo" && result == "plan updated"));
        assert!(events(&rx).iter().all(|e| !matches!(e, AgentEvent::Summarised { .. })));
        let state = script.lock().unwrap();
        assert_eq!(state.sessions.len(), 2);
        let first = &state.sessions[0];
        assert_eq!(first.user_turns, ["Update the plan.", context::SUMMARY_REQUEST]);
        assert!(first.cancelled);
        assert_eq!(first.result_batches, 0);
        let second = &state.sessions[1];
        assert!(second.system_prompt.ends_with(&context::seed_text(None, &lane.items[..original.len() + 1])));
        assert!(second.system_prompt.contains("plan updated"));
        assert_eq!(second.user_turns, ["Continue the task. The latest tool results are in the recent turns above."]);
    }

    #[test]
    fn no_node_yet_is_an_error_not_a_guess() {
        // Straight on the hub seam: no engine, no session, nothing on the network.
        let mut hub = super::super::hub_context::NodeHub;
        let why = hub.set_context(None, 32768).unwrap_err();
        assert!(why.contains("no node elected"), "{why}");
        let why = hub.set_kv(None, "q8").unwrap_err();
        assert!(why.contains("no node elected"), "{why}");
        assert!(hub.report(None).is_err());
    }

    #[test]
    fn the_nodes_report_sets_the_budget_at_start() {
        let (mut eng, lane, rx, _) = rig(Some(16384), None);
        eng.fetch_context(None, &lane);
        assert_eq!(eng.context_tokens, 32768);
        assert_eq!(eng.kv.as_deref(), Some("q8"));
        assert!(events(&rx).iter().any(|e| matches!(e, AgentEvent::ContextReport(r) if r.max_fit == 49152)));
    }

    #[test]
    fn announcements_are_told_from_answers() {
        assert!(announces_work("I'll add a test after the existing one:"));
        assert!(announces_work("Let me check the other file."));
        assert!(!announces_work("Done. I renamed it in 3 files and the build passes."));
        assert!(!announces_work(""));
        assert!(!announces_work(&"x".repeat(400)));
    }

    #[test]
    fn terminal_overflow_folds_while_each_round_frees_tokens() {
        let (mut eng, mut lane, rx, _) = rig(None, None);
        assert_eq!(eng.fresh_system(0, false).chars, lane.system_chars);
        chat(&mut lane, 8);
        assert_eq!(eng.context_tokens, 0);
        assert_eq!(eng.token_scale, 1.0);
        assert_eq!(lane.items.len(), 16);

        let mut round = 0;
        let initial_estimate = eng.used_tokens(&lane);
        let need = initial_estimate as usize + 8_192;
        // Grow the next session's base prompt to exercise a changed system size offline.
        eng.notes.push_str("Keep the original units in all calculations.");
        let fresh_chars = eng.fresh_system(0, false).chars;
        assert!(fresh_chars > lane.system_chars);
        assert!(eng.recover_overflow(&mut lane, &mut round, Some(ContextOverflow { need, max: 8_192 })));
        assert_eq!(lane.system_chars, fresh_chars);
        assert_eq!(eng.context_tokens, 8_192);
        assert!(eng.token_scale > 1.0);
        assert!(matches!(lane.items.first(), Some(Item::Summary(_))));
        let count = lane.items.len();
        assert!(count > 3 && count < 16);
        let after_first = eng.used_tokens(&lane);
        let emitted = events(&rx);
        let saved = (initial_estimate as f32 * eng.token_scale).ceil() as u32 - after_first;
        assert!(saved > 0);
        assert!(emitted.iter().any(|e| matches!(e, AgentEvent::Summarised { tokens_saved, .. } if *tokens_saved == saved)));
        assert!(emitted.iter().any(|e| matches!(e, AgentEvent::Usage(u) if u.context_max == 8_192)));
        assert!(emitted.iter().any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("folded"))));

        assert!(eng.recover_overflow(&mut lane, &mut round, Some(ContextOverflow { need: 5_000, max: 4_096 })));
        assert_eq!(eng.context_tokens, 4_096);
        assert!(lane.items.len() < count);
        assert!(eng.used_tokens(&lane) < after_first);
        assert!(events(&rx).iter().any(|e| matches!(e, AgentEvent::Summarised { .. })));

        let kept = lane.items.clone();
        assert!(!eng.recover_overflow(&mut lane, &mut round, Some(ContextOverflow { need: 3_000, max: 2_048 })));
        assert_eq!(eng.context_tokens, 2_048);
        assert_eq!(lane.items, kept, "a fold that freed nothing must preserve the conversation");
        assert!(events(&rx).iter().all(|e| !matches!(e, AgentEvent::Summarised { .. })));
        assert_eq!(round, 3);
        assert!(!eng.recover_overflow(&mut lane, &mut round, None));
        assert_eq!(round, 3);
    }
}

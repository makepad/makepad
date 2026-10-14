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
use makepad_ai_hub::hub_chat::{ChatRoute, HubChatConfig, HubChatSession};
use makepad_ai_hub::local_llm::{ChatEvent, LocalLlmConfig, ToolSpec};
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

/// One conversation with the model: the main agent, or a sub-agent.
pub(super) struct Lane {
    pub id: LaneId,
    pub read_only: bool,
    pub session: HubChatSession,
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

/// How a model turn (and the tool rounds inside it) ended.
#[derive(Debug, PartialEq)]
pub(super) enum End {
    /// The model answered without asking for more tools; its last text.
    Done(String),
    Interrupted,
    Failed,
}

pub(super) struct Engine {
    pub cfg: AgentConfig,
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
    used as f32 > max as f32 * COMPACT_AT
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

    /// Start a model session for a lane (0 = the main agent). `seed` is text
    /// carried in the system turn: a summary and recent turns.
    pub fn new_lane(&mut self, id: LaneId, read_only: bool, seed: &str) -> Lane {
        let specs = tools::specs(id != 0, read_only);
        let root = self.cfg.root.display().to_string();
        let base = if id == 0 {
            prompt::system_prompt(&root, &self.notes)
        } else {
            prompt::sub_agent_prompt(&root, read_only, &self.notes)
        };
        let mut system = base.clone();
        if !seed.is_empty() {
            system.push('\n');
            system.push_str(seed);
        }
        let tools_chars: usize = specs.iter().map(|t| t.name.len() + t.description.len() + t.parameters.len() + 20).sum();
        let session = self.start_session(system.clone(), &specs);
        Lane {
            id,
            read_only,
            session,
            specs,
            items: Vec::new(),
            // The seed is counted through the lane's items, not here.
            system_chars: base.chars().count() + tools_chars,
            notes_chars: self.notes.chars().count(),
            call_chars: 0,
            needs_reseed: false,
        }
    }

    fn start_session(&self, system: String, specs: &[ToolSpec]) -> HubChatSession {
        let mut llm = LocalLlmConfig::new(PathBuf::new());
        llm.max_context = self.cfg.context_tokens;
        HubChatSession::start(HubChatConfig {
            llm,
            preferred_model: self.cfg.model.clone(),
            max_tokens: Some(8192),
            thinking: Some(false),
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
            context_max: self.cfg.context_tokens,
            tokens_per_sec: self.tokens_per_sec,
            total_generated: self.total_generated,
            segments: context::segments(lane.system_chars, lane.notes_chars, &lane.items),
        }));
    }

    /// The lane's context as the node counts it, estimated: our chars/3.5 estimate times what
    /// the node's own count showed the estimate to miss (`token_scale`, learnt from an overflow).
    fn used_tokens(&self, lane: &Lane) -> u32 {
        let est = context::total_tokens(lane.system_chars + lane.call_chars, &lane.items);
        (est as f32 * self.token_scale).ceil() as u32
    }

    fn over_budget(&self, lane: &Lane) -> bool {
        over_budget_at(self.used_tokens(lane), self.cfg.context_tokens)
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
            self.cfg.context_tokens = report.current;
        }
        self.kv = report.kv.clone();
    }

    /// Switch the node to `tokens` and keep the conversation: the node first
    /// (a refusal leaves everything as it was), then a fold when the
    /// conversation no longer fits the new budget, then the new max in `Usage`.
    pub fn set_context(&mut self, lane: &mut Lane, tokens: u32) {
        let old = self.cfg.context_tokens;
        self.emit(AgentEvent::Notice(format!("Switching the node to a {} context...", short_tokens(tokens))));
        // A switch asked for right at start waits for the election.
        let waited = Instant::now();
        while lane.session.route().is_none() && waited.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(POLL);
        }
        let node = Self::node_of(lane);
        match self.hub.set_context(node.as_ref(), tokens) {
            Err(why) => {
                self.emit(AgentEvent::Error(format!(
                    "context stays at {}: {why}",
                    short_tokens(old)
                )));
                self.fetch_context(node, lane);
            }
            Ok(report) => {
                self.cfg.context_tokens = tokens;
                self.adopt_report(&report);
                let folded = self.over_budget(lane) && self.compact(lane, true);
                let now = self.cfg.context_tokens;
                let note = if folded {
                    format!("Context is now {}; older turns were folded into a summary to fit.", short_tokens(now))
                } else {
                    format!("Context is now {}.", short_tokens(now))
                };
                self.emit(AgentEvent::Notice(note));
                self.emit(AgentEvent::ContextReport(report));
                self.emit_usage(lane);
            }
        }
    }

    /// The share of the window the files attached to one request may take.
    fn refs_budget_chars(&self) -> usize {
        (self.cfg.context_tokens as f32 * REFS_SHARE * 3.5) as usize
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
        (self.cfg.context_tokens as f32 * TAIL_SHARE * 3.5) as usize
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

    /// Fold the old items into one summary and continue in a fresh session
    /// that carries it. Returns whether anything was folded.
    fn compact(&mut self, lane: &mut Lane, ask_model: bool) -> bool {
        let cut = context::tail_start(&lane.items, self.tail_chars());
        if cut == 0 {
            return false;
        }
        let before = self.used_tokens(lane);
        let asked = if ask_model && !lane.needs_reseed { self.ask_summary(lane) } else { None };
        let summary = asked.unwrap_or_else(|| context::mechanical_summary(&lane.items[..cut], 2000));
        let mut items = vec![Item::Summary(summary)];
        // The request being worked on stays word for word.
        if let Some(last_user) = lane.items.iter().rposition(|i| matches!(i, Item::User { .. })) {
            if last_user < cut {
                items.push(lane.items[last_user].clone());
            }
        }
        items.extend(lane.items[cut..].iter().cloned());
        self.restart(lane, items);
        let after = self.used_tokens(lane);
        if lane.id == 0 {
            self.emit(AgentEvent::Summarised { upto: cut, tokens_saved: before.saturating_sub(after) });
        }
        self.emit_usage(lane);
        true
    }

    /// Replace the lane's session by a fresh one seeded with `items`.
    fn restart(&mut self, lane: &mut Lane, items: Vec<Item>) {
        let (summary, tail) = match items.first() {
            Some(Item::Summary(s)) => (Some(s.clone()), &items[1..]),
            _ => (None, &items[..]),
        };
        let seed = context::seed_text(summary.as_deref(), tail);
        let fresh = self.new_lane(lane.id, lane.read_only, &seed);
        lane.session.cancel();
        lane.session = fresh.session;
        lane.system_chars = fresh.system_chars;
        lane.call_chars = 0;
        lane.items = items;
        lane.needs_reseed = false;
    }

    // -------------------------------------------------------------- turns

    fn submit(&mut self, lane: &mut Lane, text: String, refs: Vec<String>) {
        if lane.needs_reseed {
            let items = std::mem::take(&mut lane.items);
            self.restart(lane, items);
        }
        if self.over_budget(lane) {
            self.compact(lane, true);
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
        self.restart(lane, items);
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

    /// One user request: model turns and tool rounds until the model stops
    /// asking for tools. Returns how it ended.
    pub fn turn(&mut self, lane: &mut Lane, text: String) -> End {
        lane.session.send_user_turn(text);
        let mut nudged = false;
        let mut refolded = false;
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
                        ChatEvent::ToolCall { name, args } => calls.push((name, args)),
                        ChatEvent::TurnDone { tokens, secs, .. } => {
                            self.total_generated += tokens as u64;
                            if secs > 0.0 && tokens > 0 {
                                self.tokens_per_sec = tokens as f32 / secs as f32;
                            }
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
                            // The node counted more than we estimated: learn its ratio, fold the
                            // old turns (mechanically: the session is gone) and go on, once.
                            if let (false, Some(real)) = (refolded, overflow_tokens(&why)) {
                                let est = context::total_tokens(lane.system_chars + lane.call_chars, &lane.items).max(1);
                                self.token_scale = (real as f32 / est as f32).clamp(self.token_scale, 4.0);
                                if self.compact(lane, false) {
                                    refolded = true;
                                    self.emit(AgentEvent::Notice("The conversation outgrew the model's context: older turns were folded into a summary.".into()));
                                    lane.session.send_user_turn("Continue the task. The latest tool results are in the recent turns above.".to_string());
                                    overflowed = true;
                                    break;
                                }
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
            if self.over_budget(lane) && self.compact(lane, true) {
                // The fresh session has no pending turn to answer: tell it to go on.
                lane.session.send_user_turn(
                    "Continue the task. The latest tool results are in the recent turns above.".to_string(),
                );
                continue_after_restart(lane);
            } else {
                lane.session.send_tool_results(results);
            }
        }
    }
}

/// The prompt size in a node's context-overflow refusal ("the prompt is 49172 tokens and ...").
fn overflow_tokens(why: &str) -> Option<u32> {
    let rest = &why[why.find("context overflow")?..];
    let after = &rest[rest.find("the prompt is ")? + "the prompt is ".len()..];
    after.split_whitespace().next()?.parse().ok()
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

/// After a mid-task fold the session already got its "continue" turn.
fn continue_after_restart(_lane: &mut Lane) {}

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
            Ok(ContextReport { current: 32768, kv: Some(self.kv.clone()), max_fit: 49152, options: vec![] })
        }
        fn set_context(&mut self, _: Option<&Node>, tokens: u32) -> Result<ContextReport, String> {
            self.asked.lock().unwrap().push(tokens);
            match &self.refuse {
                Some(why) => Err(why.clone()),
                None => Ok(ContextReport { current: tokens, kv: Some(self.kv.clone()), max_fit: 49152, options: vec![] }),
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

    fn rig(context: u32, refuse: Option<&str>) -> (Engine, Lane, Receiver<AgentEvent>, Arc<Mutex<Vec<u32>>>) {
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
        let (mut eng, mut lane, rx, asked) = rig(16384, None);
        chat(&mut lane, 6);
        let before = lane.items.clone();
        eng.set_context(&mut lane, 65536);
        assert_eq!(*asked.lock().unwrap(), vec![65536]);
        assert_eq!(eng.cfg.context_tokens, 65536);
        assert_eq!(lane.items, before);
        let ev = events(&rx);
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Usage(u) if u.context_max == 65536)));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ContextReport(r) if r.current == 65536)));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("64k"))));
    }

    #[test]
    fn shrinking_below_the_conversation_folds_it() {
        let (mut eng, mut lane, rx, _) = rig(65536, None);
        chat(&mut lane, 8); // ~4.6k tokens
        assert!(!eng.over_budget(&lane));
        eng.set_context(&mut lane, 4096); // 70% = 2.9k
        assert_eq!(eng.cfg.context_tokens, 4096);
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
        let (mut eng, mut lane, rx, _) = rig(32768, Some("not enough free GPU memory"));
        chat(&mut lane, 4);
        let before = lane.items.clone();
        eng.set_context(&mut lane, 65536);
        assert_eq!(eng.cfg.context_tokens, 32768);
        assert_eq!(lane.items, before);
        let ev = events(&rx);
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Error(m) if m.contains("stays at 32k") && m.contains("GPU memory"))));
        assert!(ev.iter().all(|e| !matches!(e, AgentEvent::Usage(u) if u.context_max != 32768)));
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
        let (mut eng, lane, rx, _) = rig(16384, None);
        eng.fetch_context(None, &lane);
        assert_eq!(eng.cfg.context_tokens, 32768);
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
    fn a_context_overflow_names_the_nodes_count() {
        let why = "qwen3.8-27b on 127.0.0.1: bad request: context overflow: the prompt is 49172 tokens and the answer needs 128 more, but this node's context is 49152 tokens";
        assert_eq!(overflow_tokens(why), Some(49172));
        assert_eq!(overflow_tokens("model unavailable: not enough GPU memory"), None);
    }
}

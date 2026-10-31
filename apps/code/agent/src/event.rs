//! The contract between the agent core and any front end (window, headless).
//! The core owns the model loop on its own threads; a front end sends
//! `Command`s and drains `AgentEvent`s on its own schedule (the wake hook
//! tells it when there is something to drain, so an idle window costs nothing).

use std::path::PathBuf;

pub type ToolId = u64;

#[derive(Clone, Debug)]
pub struct AgentConfig {
    /// The workspace root every file tool is confined to.
    pub root: PathBuf,
    /// Context window in tokens the core budgets against.
    pub context_tokens: u32,
    /// Exact model id to prefer; `None` lets the hub elect.
    pub model: Option<String>,
    pub approval: ApprovalMode,
    /// Where finished sessions are written for resume. `None` = not saved.
    pub session_dir: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Every command and every edit asks.
    Ask,
    /// The default: normal work inside the repo (reads, searches, edits,
    /// build/test/check, git status/diff/log) runs without asking; risky
    /// actions ask (see `agent::risk`). Edits stay reviewable and undoable.
    Allowlist,
    /// Nothing asks (edits still shown as diffs).
    Auto,
}

#[derive(Clone, Debug)]
pub enum Command {
    /// A request; `refs` are files (paths relative to the root) the person
    /// attached: their text goes to the model with it.
    Submit { text: String, refs: Vec<String> },
    /// Esc: stop generating / stop the running tool; the loop returns to idle.
    /// Commands already running in the background go on (run_stop, the RUN
    /// panel or the session's end stop them).
    Interrupt,
    Approve { id: ToolId, decision: Decision },
    /// Put an edit back as it was before the tool ran.
    Undo(ToolId),
    Answer { id: ToolId, text: String },
    SetApprovalMode(ApprovalMode),
    /// Switch the node's context size live (the conversation is kept; it is
    /// folded when it no longer fits). Answered by `ContextReport` or `Error`.
    SetContext(u32),
    /// Switch the node's attention KV cache to "q8" or "f16", keeping the
    /// conversation. Answered by `ContextReport` or `Error`.
    SetKv(String),
    /// Ask the node about its context sizes and KV precision (`ContextReport`).
    QueryContext,
    /// Drop the context and start a fresh session.
    NewSession,
    /// Reload a saved session by its id (see `SessionInfo`).
    Resume(String),
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Allow this and every identical command from now on (session allowlist).
    AllowAlways,
    /// Allow this and remember it for this repo (kept on disk).
    AllowRepo,
    Deny,
    /// Edit approvals only: apply just these hunk indexes of the diff.
    Hunks(Vec<usize>),
}

#[derive(Clone, Debug)]
pub enum AgentEvent {
    /// Where the model runs, and that it is ready (or not).
    Status(Status),
    /// A piece of assistant text for the current turn (markdown).
    Delta { lane: LaneId, text: String },
    /// Model reasoning text, kept apart from the answer when the model emits it.
    Thinking { lane: LaneId, text: String },
    ToolStarted(ToolCall),
    /// Live output from a running tool (run_command).
    ToolOutput { id: ToolId, text: String },
    ToolFinished { id: ToolId, ok: bool, summary: String, output: String, diff: Option<Diff> },
    /// run_command returned while its command goes on as run `run` (sent
    /// just before its `ToolFinished`): early, on a failure `pattern`, or at
    /// its yield time (`pattern` None). Its output keeps coming as
    /// `ToolOutput` of the same tool until `RunEnded`.
    RunBackground { id: ToolId, run: u64, pattern: Option<String> },
    /// A background run ended: `exit` like `exit 1` or `stopped (run_stop)`,
    /// `ok` for exit 0, and one line about its output.
    RunEnded { id: ToolId, run: u64, exit: String, ok: bool, secs: f32, summary: String },
    /// The core is blocked until `Command::Approve`.
    ApprovalNeeded { id: ToolId, prompt: String },
    /// ask_user: blocked until `Command::Answer`.
    Question { id: ToolId, text: String },
    Todos(Vec<Todo>),
    Usage(Usage),
    /// What the node reports about its context: current size and the options.
    ContextReport(ContextReport),
    /// A plain line for the transcript (a context switch done, say).
    Notice(String),
    /// A file the agent touched, for the file tree / editor pane / Scope log.
    FileTouched { path: String, how: Touch },
    /// Sub-agent lifecycle; lane 0 is the main agent.
    LaneStarted { lane: LaneId, parent: LaneId, title: String },
    LaneFinished { lane: LaneId, summary: String },
    /// Old turns were folded into a summary: context items [0, upto) are now one block.
    Summarised { upto: usize, tokens_saved: u32 },
    /// The turn (and every tool loop inside it) finished; input is accepted again.
    Idle,
    Error(String),
    Sessions(Vec<SessionInfo>),
    /// This conversation was saved under this session id (after each turn).
    SessionSaved(String),
    /// A resumed session's conversation, oldest first, for the transcript view.
    Restored(Vec<RestoredItem>),
}

#[derive(Clone, Debug)]
pub enum RestoredItem {
    User { text: String, refs: Vec<String> },
    Assistant(String),
    Summary(String),
    /// A finished tool call: `title` like the live card (`grep foo`).
    Tool { title: String, ok: bool, output: String },
}

pub type LaneId = u32;

#[derive(Clone, Debug)]
pub struct Status {
    pub route: String,
    pub ready: bool,
    pub detail: String,
    /// The model's current load/swap stage with its bar and ETA, while one runs.
    pub load: Option<makepad_ai_services::progress_eta::ProgressView>,
}

#[derive(Clone, Debug)]
pub struct ToolCall {
    pub id: ToolId,
    pub lane: LaneId,
    pub name: String,
    /// One line for the collapsed card: `read src/main.rs:1-80`.
    pub title: String,
    pub args: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Touch {
    Read,
    Searched,
    Edited,
    Created,
    Ran,
}

#[derive(Clone, Debug)]
pub struct Diff {
    pub path: String,
    pub hunks: Vec<Hunk>,
}

#[derive(Clone, Debug)]
pub struct Hunk {
    pub old_start: u32,
    pub new_start: u32,
    /// Lines with their kind, in order.
    pub lines: Vec<(DiffLine, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLine {
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug)]
pub struct Todo {
    pub text: String,
    pub state: TodoState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TodoState {
    Pending,
    Active,
    Done,
}

#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub context_used: u32,
    pub context_max: u32,
    pub tokens_per_sec: f32,
    pub total_generated: u64,
    /// Share of the context by kind, for the heat strip.
    pub segments: Vec<ContextSegment>,
}

/// The node's answer about its context window and attention KV cache.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContextReport {
    /// The size the node runs at now, in tokens.
    pub current: u32,
    /// Current KV precision: `kv_loaded`, falling back to `kv` when unloaded.
    pub kv: Option<String>,
    /// The largest size that fits in the node's free GPU memory.
    pub max_fit: u32,
    pub options: Vec<ContextOption>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextOption {
    pub tokens: u32,
    /// GPU memory the node needs at this size (MB); `None` when not reported.
    pub vram_mb: Option<u32>,
    pub fits: bool,
}

#[derive(Clone, Debug)]
pub struct ContextSegment {
    pub kind: SegmentKind,
    pub tokens: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentKind {
    System,
    Project,
    Summary,
    User,
    Assistant,
    ToolResult,
}

#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub when: String,
    pub turns: usize,
}

/// A human-readable label for the node's attention KV precision.
pub fn kv_label(kv: &str) -> &str {
    match kv {
        "q8" => "8-bit (q8)",
        "f16" => "16-bit (f16)",
        _ => kv,
    }
}

/// Tokens for a person: `16k`, `64k` for window sizes (multiples of 1024),
/// `3.2k` / `850` for counts.
pub fn short_tokens(n: u32) -> String {
    if n >= 1024 && n % 1024 == 0 {
        format!("{}k", n / 1024)
    } else if n >= 10_000 {
        format!("{}k", n / 1000)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

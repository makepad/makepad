//! Studio owns attachment clients, while its tools/screen Rust host owns each
//! agent's real PTY. Stopping this worker or dropping a terminal never stops
//! that server. Only an explicit `stop` request terminates a durable session.

use makepad_widgets::makepad_micro_serde::*;
use makepad_widgets::makepad_platform::thread::{
    SignalToUI, TaskHandle, ThreadOptions, ThreadSpawner,
};
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_REQUESTS: usize = 32;
pub const SCROLLBACK_LINES: usize = 5000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, SerRon, DeRon)]
pub enum AgentProvider {
    Shell,
    Codex,
    Fable,
    Grok,
    Unknown,
}
impl AgentProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Codex => "codex",
            Self::Fable => "claude",
            Self::Grok => "grok",
            Self::Unknown => "unknown",
        }
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn from_program(path: &str) -> Option<Self> {
        match std::path::Path::new(path).file_name()?.to_str()? {
            "codex" => Some(Self::Codex),
            "claude" => Some(Self::Fable),
            "grok" => Some(Self::Grok),
            _ => None,
        }
    }
}

/// Provider-owned conversation identity, proven against the live root PID and
/// retained separately from terminal presentation. Contains no credentials.
#[derive(Clone, Debug, PartialEq, Eq, SerRon, DeRon)]
pub struct ResumeIdentity {
    pub provider: AgentProvider,
    pub conversation_id: String,
    pub cwd: String,
    pub evidence_path: String,
    pub program: String,
    pub provider_home: String,
    pub observed_pid: u32,
    pub process_start: String,
    pub verified_at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryPhase {
    PreservingIdentity,
    LoggingOut,
    BrowserLogin,
    Resuming,
    Resumed,
    LoginRequested,
    Failed,
    Interrupted,
    Ended,
}
impl RecoveryPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreservingIdentity => "preserving_identity",
            Self::LoggingOut => "logging_out",
            Self::BrowserLogin => "browser_login",
            Self::Resuming => "resuming",
            Self::Resumed => "resumed",
            Self::LoginRequested => "login_requested",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Ended => "ended",
        }
    }
    pub fn active(self) -> bool {
        matches!(
            self,
            Self::PreservingIdentity | Self::LoggingOut | Self::BrowserLogin | Self::Resuming
        )
    }
}

#[derive(Clone, Debug)]
pub struct RecoveryInfo {
    pub provider: AgentProvider,
    pub phase: RecoveryPhase,
    pub conversation_id: String,
    pub message: String,
    pub started_at_ms: u64,
}

#[derive(Clone, Debug)]
pub struct SessionSpec {
    pub session_id: String,
    pub cwd: PathBuf,
    /// An explicitly requested shell command. None opens the login shell.
    /// This is used only on first creation; reattachment never executes it.
    pub command: Option<String>,
    /// User-supplied conversation id: start this provider by resuming that
    /// session instead of opening a fresh chat.
    pub resume_conversation: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub session_id: String,
    pub state_dir: PathBuf,
    pub title: String,
    pub activity: String,
    pub started_at_ms: u64,
    /// Monitor this process tree, not MpTerm's short-lived attachment PID.
    pub supervisor_pid: u32,
    /// Pass to MpTerm::restart_with. Attach connects only to the saved live PTY;
    /// it cannot create or replace an agent.
    pub attach_command: String,
    pub backend: &'static str,
    pub transport_program: String,
    pub transport_version: String,
    pub transport_warning: Option<String>,
    pub scrollback_lines: usize,
    pub cwd: PathBuf,
    pub clients: usize,
    pub provider: AgentProvider,
    pub resume: Option<ResumeIdentity>,
    pub resume_error: Option<String>,
    pub recovery: Option<RecoveryInfo>,
}

impl SessionInfo {
    pub fn key(&self) -> String {
        format!("{}\n{}", self.state_dir.display(), self.session_id)
    }
    pub fn menu_label(&self, duplicate: bool) -> String {
        let since = self.started_at_ms / 1000;
        let time = if since == 0 {
            "unknown".into()
        } else {
            format!("{:02}:{:02} UTC", since / 3600 % 24, since / 60 % 60)
        };
        let mut title: String = self.title.chars().take(48).collect();
        if self.title.chars().count() > 48 {
            title.push('…');
        }
        let suffix = if duplicate {
            format!(
                " · {}",
                &self.session_id[self.session_id.len().saturating_sub(8)..]
            )
        } else {
            String::new()
        };
        format!("{} · {} · since {}{}", title, self.activity, time, suffix)
    }
}

/// Persist scope and opaque identity together; old unscoped links use Studio's directory.
pub fn view_target(key: &str, default: &std::path::Path) -> Result<(PathBuf, String), String> {
    let (state, id) = match key.split_once('\n') {
        Some((state, id)) => (PathBuf::from(state), id),
        None => (default.to_owned(), key),
    };
    validate_id(id)?;
    if !state.is_absolute() {
        return Err("Session state directory must be absolute".into());
    }
    Ok((state, id.into()))
}

pub fn terminal_menu(sessions: &[SessionInfo]) -> Vec<(String, String)> {
    sessions
        .iter()
        .map(|info| {
            (
                info.key(),
                info.menu_label(
                    sessions
                        .iter()
                        .filter(|other| other.title == info.title)
                        .count()
                        > 1,
                ),
            )
        })
        .collect()
}

#[derive(Clone, Debug)]
pub enum SessionOutcome {
    Ready(SessionInfo),
    Inventory {
        state_dir: PathBuf,
        sessions: Vec<SessionInfo>,
        views: Vec<(u64, String)>,
    },
    ViewReady {
        tab: u64,
        session: Option<SessionInfo>,
    },
    /// None means that the durable session has ended. It is not recreated.
    Status(Option<SessionInfo>),
    Stopped,
    StoppedWithResume(ResumeIdentity),
    ResumeCaptured(ResumeIdentity),
    /// The worker proved the live Claude root; UI must still preserve drafts
    /// and submit /login through that existing terminal's input path.
    FableLoginReady(SessionInfo),
    RecoveryStatus(Option<SessionInfo>, RecoveryInfo),
}

#[derive(Clone, Debug)]
pub struct SessionReply {
    pub request_id: u64,
    pub session_id: String,
    pub result: Result<SessionOutcome, String>,
}

enum Operation {
    List(PathBuf),
    NewView(u64, SessionSpec),
    SelectView(u64, Option<String>),
    Prepare(SessionSpec),
    PrepareProvider(SessionSpec, AgentProvider),
    Attach,
    Inspect,
    Stop,
    CaptureResume,
    Restore,
    RestoreEnvironment(String, PathBuf),
    Recover(String, PathBuf),
}

struct Request {
    request_id: u64,
    session_id: String,
    operation: Operation,
}

/// Generate once for a new terminal, then persist the returned value before
/// calling prepare. Reopened terminals must use attach with that saved value.
pub fn new_session_id(tab_id: u64) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let serial = NEXT.fetch_add(1, Ordering::Relaxed) as u32;
    format!("s{tab_id:016x}-{time:016x}-{serial:08x}")
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 48
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    {
        return Err(
            "Session IDs require 1–48 ASCII letters, digits, underscores or hyphens".into(),
        );
    }
    Ok(())
}

/// Called only by the iteration worker after an explicit lane deletion.
/// Stop uses the same identity checks and process-group shutdown as Stop agent.
pub(crate) fn delete_lane_session(state: PathBuf, tab: u64) -> Result<(), String> {
    let id = format!("term-{tab:016x}");
    let records = state.join("agent_sessions");
    if !records.join(format!("{id}.ron")).exists() {
        if records.join(format!("{id}.claim")).exists()
            || records.join(format!("{id}.json")).exists()
        {
            return Err("Lane session identity is incomplete; cannot safely delete it".into());
        }
        return Ok(());
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    native::delete_lane_session(state, tab, &id)?;
    #[cfg(windows)]
    windows::delete_lane_session(state, tab, &id)?;
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    return Err("Lane terminal deletion is unsupported on this platform".into());
    // The host has acknowledged exit before any session state is removed.
    for entry in std::fs::read_dir(&records).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(&format!("{id}."))
        {
            std::fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub struct AgentSessionWorker {
    commands: SyncSender<Request>,
    replies: Receiver<SessionReply>,
    pending: VecDeque<Request>,
    outstanding: BTreeMap<u64, String>,
    next_request: u64,
    stop: Arc<AtomicBool>,
    task: TaskHandle<()>,
}

impl AgentSessionWorker {
    pub fn start(spawner: &ThreadSpawner, state_dir: PathBuf) -> Result<Self, String> {
        if !cfg!(any(target_os = "macos", target_os = "linux", windows)) {
            return Err("Studio PTY sessions are supported on macOS, Linux and Windows".into());
        }
        if !state_dir.is_absolute() {
            return Err("Agent session state requires an absolute directory".into());
        }
        let (commands, rx) = mpsc::sync_channel::<Request>(4);
        let (tx, replies) = mpsc::sync_channel::<SessionReply>(MAX_REQUESTS);
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let task = spawner
            .spawn_worker(
                ThreadOptions {
                    name: Some("studio-agent-sessions".into()),
                    ..Default::default()
                },
                move || {
                    #[cfg(any(target_os = "macos", target_os = "linux"))]
                    native::run(state_dir, rx, tx, cancelled);
                    #[cfg(windows)]
                    windows::run(state_dir, rx, tx, cancelled);
                    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
                    {
                        drop(state_dir);
                        while !cancelled.load(Ordering::Relaxed) {
                            let request = match rx.recv_timeout(Duration::from_millis(50)) {
                                Ok(request) => request,
                                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            };
                            // Unsupported platforms never execute a queued
                            // command or silently acknowledge its acceptance.
                            let operation = match request.operation {
                                Operation::Prepare(spec) => { drop(spec); "Terminal creation" }
                                Operation::PrepareProvider(spec, provider) => { drop(spec); provider.as_str() }
                                Operation::RestoreEnvironment(command, cwd) | Operation::Recover(command, cwd) => { drop((command, cwd)); "Agent resume/recovery" }
                                _ => "Agent session operation"
                            };
                            if tx.send(SessionReply { request_id: request.request_id, session_id: request.session_id,
                                result: Err(format!("{operation} is unavailable: Studio PTY sessions require macOS, Linux or Windows")) }).is_err() { break; }
                            SignalToUI::set_ui_signal();
                        }
                    }
                },
            )
            .map_err(|e| e.to_string())?;
        Ok(Self {
            commands,
            replies,
            pending: VecDeque::new(),
            outstanding: BTreeMap::new(),
            next_request: 1,
            stop,
            task,
        })
    }

    /// Create a new durable session, or recover its already-running creation.
    /// A prior creation marker with no live server is an error, never a restart.
    pub fn prepare(&mut self, spec: SessionSpec) -> Result<u64, String> {
        if !spec.cwd.is_absolute() || spec.cwd.as_os_str().len() > 4096 {
            return Err("Agent working directories must be absolute and at most 4096 bytes".into());
        }
        if spec
            .command
            .as_ref()
            .is_some_and(|s| s.len() > 16 * 1024 || s.contains('\0'))
        {
            return Err("Agent commands must contain no NUL and be at most 16 KiB".into());
        }
        self.enqueue(spec.session_id.clone(), Operation::Prepare(spec))
    }

    /// A deliberate new provider lane. Existing identities only reattach; this
    /// never replaces an ended conversation with a fresh chat.
    pub fn prepare_provider(
        &mut self,
        spec: SessionSpec,
        provider: AgentProvider,
    ) -> Result<u64, String> {
        if !matches!(
            provider,
            AgentProvider::Codex | AgentProvider::Fable | AgentProvider::Grok
        ) {
            return Err("Choose Codex, Fable or Grok for a provider lane".into());
        }
        if !spec.cwd.is_absolute()
            || spec.cwd.as_os_str().len() > 4096
            || spec
                .command
                .as_ref()
                .is_some_and(|s| s.len() > 16384 || s.contains('\0'))
        {
            return Err("Invalid provider working directory or launch configuration".into());
        }
        if let Some(token) = spec.resume_conversation.as_deref() {
            crate::iteration::validate_resume_token(token)?;
        }
        self.enqueue(
            spec.session_id.clone(),
            Operation::PrepareProvider(spec, provider),
        )
    }

    pub fn capture_resume(&mut self, session_id: String) -> Result<u64, String> {
        self.enqueue(session_id, Operation::CaptureResume)
    }

    /// Explicit restore: attach the same live PTY, or resume a proven saved
    /// conversation. Missing provider metadata is an error, not a new chat.
    pub fn restore(&mut self, session_id: String) -> Result<u64, String> {
        self.enqueue(session_id, Operation::Restore)
    }

    pub fn restore_with_environment(
        &mut self,
        session_id: String,
        studio_command: String,
        expected_cwd: PathBuf,
    ) -> Result<u64, String> {
        if studio_command.len() > 16384
            || studio_command.contains('\0')
            || !expected_cwd.is_absolute()
        {
            return Err("Invalid Studio restore environment".into());
        }
        self.enqueue(
            session_id,
            Operation::RestoreEnvironment(studio_command, expected_cwd),
        )
    }

    /// Explicit account recovery only. Never called by an inspection/retry.
    pub fn recover_with_environment(
        &mut self,
        session_id: String,
        studio_command: String,
        expected_cwd: PathBuf,
    ) -> Result<u64, String> {
        if studio_command.len() > 16384
            || studio_command.contains('\0')
            || !expected_cwd.is_absolute()
        {
            return Err("Invalid Studio recovery environment".into());
        }
        self.enqueue(session_id, Operation::Recover(studio_command, expected_cwd))
    }

    pub fn attach(&mut self, session_id: String) -> Result<u64, String> {
        self.enqueue(session_id, Operation::Attach)
    }

    pub fn inspect(&mut self, session_id: String) -> Result<u64, String> {
        self.enqueue(session_id, Operation::Inspect)
    }

    pub fn list(&mut self, repo: PathBuf) -> Result<u64, String> {
        self.enqueue("studio-inventory".into(), Operation::List(repo))
    }

    pub fn select_view(&mut self, tab: u64, session: Option<String>) -> Result<u64, String> {
        if let Some(id) = &session {
            view_target(id, std::path::Path::new("/"))?;
        }
        self.enqueue("studio-view".into(), Operation::SelectView(tab, session))
    }

    pub fn new_view(
        &mut self,
        tab: u64,
        cwd: PathBuf,
        command: Option<String>,
    ) -> Result<u64, String> {
        let spec = SessionSpec {
            session_id: new_session_id(tab),
            cwd,
            command,
            resume_conversation: None,
        };
        self.enqueue(spec.session_id.clone(), Operation::NewView(tab, spec))
    }

    /// The sole operation that intentionally terminates the agent and its PTY.
    pub fn stop(&mut self, session_id: String) -> Result<u64, String> {
        self.enqueue(session_id, Operation::Stop)
    }

    fn enqueue(&mut self, session_id: String, operation: Operation) -> Result<u64, String> {
        validate_id(&session_id)?;
        if self.stop.load(Ordering::Relaxed) || self.task.is_finished() {
            return Err(
                "Agent session manager is stopped; running agents are still detached".into(),
            );
        }
        if self.outstanding.len() >= MAX_REQUESTS {
            return Err(
                "Agent session queue is full; retry after an outstanding request completes".into(),
            );
        }
        let request_id = self.next_request;
        self.next_request += 1;
        self.outstanding.insert(request_id, session_id.clone());
        self.pending.push_back(Request {
            request_id,
            session_id,
            operation,
        });
        self.retry();
        Ok(request_id)
    }

    fn retry(&mut self) {
        while let Some(request) = self.pending.pop_front() {
            match self.commands.try_send(request) {
                Ok(()) => {}
                Err(TrySendError::Full(request) | TrySendError::Disconnected(request)) => {
                    self.pending.push_front(request);
                    break;
                }
            }
        }
    }

    /// Poll every UI tick/Signal; this also retries accepted commands that
    /// encountered a full bounded channel. It never waits or takes a lock.
    pub fn poll(&mut self) -> Vec<SessionReply> {
        self.retry();
        let mut result = Vec::new();
        while let Ok(reply) = self.replies.try_recv() {
            self.outstanding.remove(&reply.request_id);
            result.push(reply);
        }
        if self.task.is_finished() {
            while let Ok(reply) = self.replies.try_recv() {
                self.outstanding.remove(&reply.request_id);
                result.push(reply);
            }
            for (request_id, session_id) in std::mem::take(&mut self.outstanding) {
                result.push(SessionReply { request_id, session_id, result: Err("Agent session manager stopped before acknowledging the request; existing agents were not stopped".into()) });
            }
            self.pending.clear();
        }
        result
    }

    /// Shutdown of management only. Screen servers deliberately outlive this.
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

impl Drop for AgentSessionWorker {
    fn drop(&mut self) {
        self.request_stop();
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
const PROVIDER_BOOTSTRAP: &str = r#"You are this Studio lane's root AI. Follow repository AGENTS.md and apps/studio/AGENTS.md. At startup/resume fetch current lane context: curl -fsS "$("$MAKEPAD_STUDIO_CLI" --url)/brief". Its delegation context overrides stale worktree roles. EVERY user terminal request: before coding, summarize new scope and 3-8 word todos via curl -sS --json '{"i":"unique-id","v":0,"q":"New request scope","u":[["id","w","Short action"]]}' "$("$MAKEPAD_STUDIO_CLI" --url)/feedback". Use current v, then returned v; stable IDs, q=queued/w=working/d=implemented UNVERIFIED/b=blocked. Later deltas omit q and unchanged labels: {"i":"next-id","v":1,"u":[["id","d"]]}. Preserve unfinished work; don't echo chats, full plans or unchanged todos into callbacks. Keep exact details in chat; include constraints in the brief scope. Retry uncertain requests with the SAME i/body; pending -> GET /feedback/i; stale -> GET /brief and reconcile. Resolve --url per call after UI restarts or lane splits; the running PTY supplies the lane identity automatically. Never expose that URL. Fetch /tools or /state only when needed. Other lane operations use POST /call {id,tool,args}; observe their results. No invented success or human acceptance. Code may proceed, but the next build/check waits for the human to close the prior app. Continue the assigned work; do not create another root conversation. Understand code through Studio: read code_brief and code_impact before editing; discover tools with $MAKEPAD_STUDIO_CLI --tools; read apps/studio/AGENTS.md."#;

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
// The only supported shell preamble is Studio's own, single-quoted export
// wrapper. Decode it into argv environment values; never execute its text.
fn studio_environment(command: Option<&str>) -> Result<Vec<(String, String)>, String> {
    let Some(command) = command else {
        return Ok(Vec::new());
    };
    let mut rest = command
        .strip_suffix("exec \"${SHELL:-/bin/sh}\" -l")
        .ok_or("Provider launch requires Studio's explicit environment wrapper")?;
    let mut env = Vec::new();
    while !rest.is_empty() {
        rest = rest
            .strip_prefix("export ")
            .ok_or("Invalid Studio environment assignment")?;
        let (name, tail) = rest
            .split_once('=')
            .ok_or("Invalid Studio environment assignment")?;
        if !matches!(
            name,
            "MAKEPAD_STUDIO_FLOW_ID"
                | "MAKEPAD_STUDIO_CONTROL_DIR"
                | "MAKEPAD_STUDIO_CLI"
                | "MAKEPAD_STUDIO_MCP_TOKEN"
        ) || env.iter().any(|(key, _)| key == name)
        {
            return Err("Unsupported Studio environment assignment".into());
        }
        rest = tail
            .strip_prefix('\'')
            .ok_or("Studio environment values must be quoted")?;
        let mut value = String::new();
        loop {
            let at = rest.find('\'').ok_or("Unclosed Studio environment value")?;
            value.push_str(&rest[..at]);
            rest = &rest[at + 1..];
            if let Some(tail) = rest.strip_prefix("\\''") {
                value.push('\'');
                rest = tail;
            } else {
                break;
            }
        }
        if value.len() > 4096 || value.chars().any(char::is_control) {
            return Err("Invalid Studio environment value".into());
        }
        rest = rest
            .strip_prefix("; ")
            .ok_or("Invalid Studio environment separator")?;
        env.push((name.to_owned(), value));
    }
    Ok(env)
}

#[cfg(windows)]
#[path = "agent_session_windows.rs"]
mod windows;

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod native {
    use super::*;
    use makepad_strict_json::{self as json, Value};
    use std::{
        ffi::OsStr,
        fs::{self, File, OpenOptions},
        io::{BufRead, BufReader, Read, Write},
        os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        path::Path,
        process::{Command, ExitStatus, Stdio},
        time::Instant,
    };

    const MAX_OUTPUT: u64 = 64 * 1024;

    #[derive(SerRon, DeRon)]
    struct Record {
        version: u32,
        session_id: String,
        cwd: String,
        created_at_ms: u64,
    }

    #[derive(Clone, SerRon, DeRon)]
    struct LaunchRecord {
        version: u32,
        provider: AgentProvider,
        program: String,
        environment: Vec<(String, String)>,
    }

    #[derive(SerRon, DeRon)]
    struct RecoveryRecord {
        version: u32,
        identity: ResumeIdentity,
        stage_file: String,
        started_at_ms: u64,
    }

    #[derive(Clone, SerRon, DeRon)]
    struct TransportRecord {
        version: u32,
        program: String,
        config: String,
        screen_version: String,
        truecolor: bool,
    }

    struct Process {
        pid: u32,
        parent: u32,
        program: String,
    }

    #[derive(Clone)]
    struct Backend {
        program: PathBuf,
        records: PathBuf,
        sockets: PathBuf,
        uid: u32,
        /// Per-lane MCP bearer tokens, minted at provider launch and revoked
        /// when the lane's terminal lifecycle ends (never on UI detach).
        mcp_tokens: std::sync::Arc<crate::mcp::TokenStore>,
    }

    extern "C" {
        fn geteuid() -> u32;
    }

    fn private_directory(path: &Path, uid: u32) -> Result<(), String> {
        match fs::symlink_metadata(path) {
            Ok(meta) => {
                if !meta.is_dir() || meta.file_type().is_symlink() || meta.uid() != uid {
                    return Err(format!(
                        "{} must be a private directory owned by this user",
                        path.display()
                    ));
                }
                if meta.permissions().mode() & 0o077 != 0 {
                    return Err(format!("{} must have permissions 0700", path.display()));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(path)
                    .map_err(|e| e.to_string())?;
            }
            Err(error) => return Err(error.to_string()),
        }
        Ok(())
    }

    fn read_regular(path: &Path, limit: u64, uid: u32) -> Result<Option<String>, String> {
        let meta = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.to_string()),
        };
        if !meta.is_file()
            || meta.file_type().is_symlink()
            || meta.uid() != uid
            || meta.len() > limit
        {
            return Err(format!(
                "Invalid agent session state file {}",
                path.display()
            ));
        }
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|e| e.to_string())?
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > limit {
            return Err("Agent session state exceeds its size limit".into());
        }
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|e| e.to_string())
    }

    // Hard-linking the completely synced temporary file claims the final name
    // without ever replacing a concurrent creator's durable identity marker.
    fn create_record(dir: &Path, name: &str, text: &str) -> Result<bool, String> {
        let temp = dir.join(format!(".{}-{}.tmp", name, new_session_id(0)));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp)
                .map_err(|e| e.to_string())?;
            file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            match fs::hard_link(&temp, dir.join(name)) {
                Ok(()) => {
                    if let Ok(dir) = File::open(dir) {
                        let _ = dir.sync_all();
                    }
                    Ok(true)
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
                Err(e) => Err(e.to_string()),
            }
        })();
        let _ = fs::remove_file(temp);
        result
    }

    fn shell_quote(value: &OsStr) -> Result<String, String> {
        let value = value
            .to_str()
            .ok_or("Terminal attachment paths must be valid UTF-8")?;
        if value.contains('\0') {
            return Err("Terminal attachment paths cannot contain NUL".into());
        }
        Ok(format!("'{}'", value.replace('\'', "'\\''")))
    }

    fn timestamp_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
    fn uuid(value: &str) -> bool {
        value.len() == 36
            && value.bytes().enumerate().all(|(i, b)| {
                if matches!(i, 8 | 13 | 18 | 23) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit()
                }
            })
    }
    fn provider_home(provider: AgentProvider) -> Result<PathBuf, String> {
        let (variable, directory) = match provider {
            AgentProvider::Codex => ("CODEX_HOME", ".codex"),
            AgentProvider::Fable => ("CLAUDE_CONFIG_DIR", ".claude"),
            AgentProvider::Grok => ("GROK_HOME", ".grok"),
            _ => return Err("This process has no supported conversation store".into()),
        };
        let path = std::env::var_os(variable)
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(directory)))
            .ok_or("Provider home directory is unavailable")?;
        path.canonicalize()
            .map_err(|_| "Provider conversation directory is unavailable".into())
    }
    fn executable(provider: AgentProvider) -> Result<PathBuf, String> {
        let name = provider.as_str();
        std::env::var_os("PATH")
            .into_iter()
            .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
            .map(|path| path.join(name))
            .find(|path| {
                path.is_absolute()
                    && fs::metadata(path).is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0)
            })
            .ok_or_else(|| format!("{name} CLI is unavailable; no replacement chat was started"))
    }

    fn replace_state(dir: &Path, name: &str, text: &str) -> Result<(), String> {
        if text.len() > 32 * 1024 {
            return Err("Provider resume metadata exceeds 32 KiB".into());
        }
        let temp = dir.join(format!(".resume-{}.tmp", new_session_id(0)));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp)
                .map_err(|e| e.to_string())?;
            file.write_all(text.as_bytes())
                .and_then(|_| file.sync_all())
                .map_err(|e| e.to_string())?;
            fs::rename(&temp, dir.join(name)).map_err(|e| e.to_string())?;
            File::open(dir)
                .and_then(|dir| dir.sync_all())
                .map_err(|e| e.to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }

    fn metadata_lines(path: &Path, uid: u32) -> Result<Vec<Value>, String> {
        let meta =
            fs::symlink_metadata(path).map_err(|_| "Conversation evidence is unavailable")?;
        if !meta.file_type().is_file() || meta.uid() != uid {
            return Err("Conversation evidence must be an owned regular file".into());
        }
        let file = File::open(path).map_err(|_| "Conversation evidence cannot be read")?;
        let mut reader = BufReader::new(file.take(256 * 1024));
        let mut values = Vec::new();
        for _ in 0..64 {
            let mut line = String::new();
            if reader
                .read_line(&mut line)
                .map_err(|_| "Invalid conversation metadata")?
                == 0
            {
                break;
            }
            if let Ok(value) = json::parse_depth(line.as_bytes(), 32) {
                values.push(value);
            }
        }
        Ok(values)
    }

    fn evidence_matches(identity: &ResumeIdentity, uid: u32) -> Result<(), String> {
        if !uuid(&identity.conversation_id)
            || !Path::new(&identity.cwd).is_absolute()
            || !Path::new(&identity.program).is_absolute()
        {
            return Err("Saved conversation identity is invalid".into());
        }
        let path = Path::new(&identity.evidence_path)
            .canonicalize()
            .map_err(|_| "Saved conversation evidence is missing; refusing a fresh chat")?;
        let home = Path::new(&identity.provider_home)
            .canonicalize()
            .map_err(|_| "Provider home is unavailable")?;
        if !path.starts_with(&home) {
            return Err("Saved conversation evidence escaped its provider directory".into());
        }
        let rows = metadata_lines(&path, uid)?;
        let matches = rows.iter().any(|row| {
            let (value, id_field) = match identity.provider {
                AgentProvider::Codex
                    if row.get("type").and_then(Value::as_str) == Some("session_meta") =>
                {
                    (row.get("payload").unwrap_or(&Value::Null), "id")
                }
                AgentProvider::Fable => (row, "sessionId"),
                AgentProvider::Grok => (row.get("info").unwrap_or(&Value::Null), "id"),
                _ => return false,
            };
            value.get(id_field).and_then(Value::as_str) == Some(identity.conversation_id.as_str())
                && value.get("cwd").and_then(Value::as_str).is_some_and(|cwd| {
                    match (
                        Path::new(cwd).canonicalize(),
                        Path::new(&identity.cwd).canonicalize(),
                    ) {
                        (Ok(actual), Ok(expected)) => actual == expected,
                        _ => false,
                    }
                })
        });
        if !matches {
            return Err(
                "Stored provider history does not prove this conversation and working directory"
                    .into(),
            );
        }
        Ok(())
    }

    impl Backend {
        fn open(state_dir: PathBuf) -> Result<Self, String> {
            let uid = unsafe { geteuid() };
            let mcp_tokens = std::sync::Arc::new(crate::mcp::TokenStore::open(&state_dir)?);
            let records = state_dir.join("agent_sessions");
            private_directory(&records, uid)?;
            let records = fs::canonicalize(records).map_err(|e| e.to_string())?;
            let hash = records
                .as_os_str()
                .as_encoded_bytes()
                .iter()
                .fold(0xcbf29ce484222325u64, |h, b| {
                    (h ^ *b as u64).wrapping_mul(0x100000001b3)
                });
            let sockets = PathBuf::from(format!("/tmp/mps-{uid}-{hash:016x}"));
            let program = std::env::current_exe()
                .map_err(|e| e.to_string())?
                .parent()
                .ok_or("Studio executable has no directory")?
                .join("makepad-screen");
            if !fs::metadata(&program).is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0) {
                return Err("Build the repository's tools/screen binary alongside Studio: cargo build --release -p makepad-screen -p makepad-studio. No external software was installed or started".into());
            }
            Ok(Self {
                program,
                records,
                sockets,
                uid,
                mcp_tokens,
            })
        }

        fn for_session(
            &self,
            id: &str,
            creating: bool,
            restoring: bool,
            stop: &AtomicBool,
        ) -> Result<Self, String> {
            validate_id(id)?;
            let name = format!("{id}.transport.ron");
            let saved = read_regular(&self.records.join(&name), 8192, self.uid)?;
            if let Some(text) = &saved {
                let pin = TransportRecord::deserialize_ron(text)
                    .map_err(|_| "Invalid saved terminal transport identity")?;
                if pin.version != 2 || pin.config != "makepad-screen-v1" {
                    if !restoring {
                        return Err("This lane belongs to the previous GNU Screen backend. Its process was left untouched; explicitly resume it after the old session has ended".into());
                    }
                    self.verify_no_endpoint(id)?;
                    if let Some(identity) = self.saved_resume(id)? {
                        self.verify_root_exited(&identity, stop)?;
                    }
                } else {
                    return Ok(self.clone());
                }
            } else if !creating && !restoring {
                return Err(
                    "Unknown Studio PTY identity; attach cannot start a replacement".into(),
                );
            }
            let (status, version) =
                self.run_command(Command::new(&self.program).arg("--version"), stop)?;
            if !status.success() || !version.trim().starts_with("makepad-screen ") {
                return Err(
                    "The sibling executable is not the repository's makepad-screen helper".into(),
                );
            }
            let pin = TransportRecord {
                version: 2,
                program: self.program.to_string_lossy().into(),
                config: "makepad-screen-v1".into(),
                screen_version: version.trim().into(),
                truecolor: true,
            };
            if saved.is_some() {
                replace_state(&self.records, &name, &pin.serialize_ron())?;
            } else if !create_record(&self.records, &name, &pin.serialize_ron())? {
                return Err(
                    "Terminal identity changed while creating the session; inspect before retrying"
                        .into(),
                );
            }
            Ok(self.clone())
        }

        fn command(&self) -> Command {
            let mut command = Command::new(&self.program);
            command
                .env("TERM", "xterm-256color")
                .env("COLORTERM", "truecolor")
                .env("TERM_PROGRAM", "terminal")
                .env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"))
                .env_remove("NO_COLOR")
                .env_remove("STY")
                .env_remove("TMUX")
                .env_remove("STUDIO_HOST")
                .env_remove("STUDIO_BUILD")
                .env_remove("STUDIO_CRATE")
                .env_remove("MAKEPAD_STDIN_LOOP");
            command
        }

        fn start_command(&self, id: &str, cwd: &Path, restarting: bool) -> Command {
            let mut command = self.command();
            command
                .arg("start")
                .arg("--state-dir")
                .arg(&self.records)
                .arg("--session")
                .arg(id)
                .arg("--cwd")
                .arg(cwd);
            if restarting {
                command.arg("--restart");
            }
            command.arg("--").current_dir(cwd);
            command
        }

        fn run_command(
            &self,
            command: &mut Command,
            stop: &AtomicBool,
        ) -> Result<(ExitStatus, String), String> {
            self.run_bounded(command, stop, MAX_OUTPUT)
        }

        fn run_bounded(
            &self,
            command: &mut Command,
            stop: &AtomicBool,
            limit: u64,
        ) -> Result<(ExitStatus, String), String> {
            self.run_bounded_tracking_spawn(command, stop, limit, &mut false)
        }

        fn run_bounded_tracking_spawn(
            &self,
            command: &mut Command,
            stop: &AtomicBool,
            limit: u64,
            spawned: &mut bool,
        ) -> Result<(ExitStatus, String), String> {
            if stop.load(Ordering::Relaxed) {
                return Err(
                    "Agent session operation cancelled; existing sessions remain running".into(),
                );
            }
            let path = self
                .records
                .join(format!(".control-{}.tmp", new_session_id(0)));
            let file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&path)
                .map_err(|e| e.to_string())?;
            let result = (|| {
                let stderr = file.try_clone().map_err(|e| e.to_string())?;
                let mut child = command
                    .stdin(Stdio::null())
                    .stdout(file)
                    .stderr(stderr)
                    .spawn()
                    .map_err(|e| e.to_string())?;
                *spawned = true;
                let deadline = Instant::now() + Duration::from_secs(5);
                let status = loop {
                    match child.try_wait() {
                        Ok(Some(status)) => break status,
                        Ok(None) => {}
                        Err(error) => {
                            let _ = child.kill();
                            let _ = child.wait();
                            return Err(error.to_string());
                        }
                    }
                    if stop.load(Ordering::Relaxed)
                        || Instant::now() >= deadline
                        || fs::metadata(&path).is_ok_and(|m| m.len() > limit)
                    {
                        // Only the short-lived control invocation, never a
                        // separately detached server or arbitrary process tree.
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err("Studio PTY control exceeded its deadline or was cancelled; inspect the session before retrying".into());
                    }
                    std::thread::sleep(Duration::from_millis(20));
                };
                if fs::metadata(&path).is_ok_and(|m| m.len() > limit) {
                    return Err("Provider control output exceeded its bounded read limit".into());
                }
                let output = read_regular(&path, limit, self.uid)?.unwrap_or_default();
                Ok((status, output))
            })();
            let _ = fs::remove_file(path);
            result
        }

        fn record(&self, id: &str) -> Result<Option<Record>, String> {
            let Some(text) = read_regular(&self.records.join(format!("{id}.ron")), 8192, self.uid)?
            else {
                return Ok(None);
            };
            let record = Record::deserialize_ron(&text)
                .map_err(|e| format!("Invalid durable session identity: {e:?}"))?;
            if record.version != 1
                || record.session_id != id
                || !Path::new(&record.cwd).is_absolute()
            {
                return Err("Durable agent session identity is inconsistent".into());
            }
            Ok(Some(record))
        }

        fn launch_record(&self, id: &str) -> Result<Option<LaunchRecord>, String> {
            read_regular(
                &self.records.join(format!("{id}.provider.ron")),
                32 * 1024,
                self.uid,
            )?
            .map(|text| {
                LaunchRecord::deserialize_ron(&text)
                    .map_err(|_| "Invalid provider launch metadata".into())
            })
            .transpose()
        }
        fn saved_resume(&self, id: &str) -> Result<Option<ResumeIdentity>, String> {
            read_regular(
                &self.records.join(format!("{id}.resume.ron")),
                32 * 1024,
                self.uid,
            )?
            .map(|text| {
                ResumeIdentity::deserialize_ron(&text)
                    .map_err(|_| "Invalid saved conversation identity".into())
            })
            .transpose()
        }
        fn recovery_record(&self, id: &str) -> Result<Option<RecoveryRecord>, String> {
            let Some(text) = read_regular(
                &self.records.join(format!("{id}.recovery.ron")),
                32 * 1024,
                self.uid,
            )?
            else {
                return Ok(None);
            };
            let record = RecoveryRecord::deserialize_ron(&text)
                .map_err(|_| "Invalid account recovery metadata")?;
            if record.version != 1
                || record.identity.provider != AgentProvider::Codex
                || !uuid(&record.identity.conversation_id)
                || !record.stage_file.starts_with(&format!("{id}.recovery-"))
                || !record.stage_file.ends_with(".stage")
                || !record
                    .stage_file
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
            {
                return Err("Account recovery metadata is inconsistent".into());
            }
            Ok(Some(record))
        }

        fn recovery_info(&self, id: &str, live: bool) -> Result<Option<RecoveryInfo>, String> {
            let Some(record) = self.recovery_record(id)? else {
                return Ok(None);
            };
            let stage = read_regular(&self.records.join(&record.stage_file), 128, self.uid)?
                .unwrap_or_default();
            let (mut phase, mut message) = match stage.trim() {
                "preserving_identity" => (
                    RecoveryPhase::PreservingIdentity,
                    "Conversation saved; preparing account recovery",
                ),
                "logging_out" => (
                    RecoveryPhase::LoggingOut,
                    "Signing out Codex; this changes the shared Codex account",
                ),
                "browser_login" => (
                    RecoveryPhase::BrowserLogin,
                    "Complete Codex sign-in in the browser; other lanes remain running",
                ),
                "resuming" => (
                    RecoveryPhase::Resuming,
                    "Sign-in completed; reconnecting the saved conversation",
                ),
                "resumed" => (
                    RecoveryPhase::Resumed,
                    "The same Codex conversation resumed; waiting for actual working output",
                ),
                "ended" => (
                    RecoveryPhase::Ended,
                    "The recovered conversation ended; its resume ID remains saved",
                ),
                "logout_failed" => (
                    RecoveryPhase::Failed,
                    "Codex sign-out failed; no login or new conversation was started",
                ),
                "login_failed" => (
                    RecoveryPhase::Failed,
                    "Codex sign-in failed or was cancelled; the conversation remains saved",
                ),
                "resume_failed" => (
                    RecoveryPhase::Failed,
                    "Codex could not resume the saved conversation; retry only explicitly",
                ),
                _ => (
                    RecoveryPhase::Interrupted,
                    "Recovery was interrupted; inspect this lane before explicitly retrying",
                ),
            };
            if !live && phase.active() {
                phase = RecoveryPhase::Interrupted;
                message = "Recovery process ended before completion; the conversation remains saved and login was not retried";
            }
            Ok(Some(RecoveryInfo {
                provider: AgentProvider::Codex,
                phase,
                conversation_id: record.identity.conversation_id,
                message: message.into(),
                started_at_ms: record.started_at_ms,
            }))
        }

        fn inspect_session(&self, id: &str, stop: &AtomicBool) -> Result<SessionOutcome, String> {
            let info = self.existing(id, stop)?;
            if info.is_none() {
                if let Some(recovery) = self.recovery_info(id, false)? {
                    return Ok(SessionOutcome::RecoveryStatus(None, recovery));
                }
            }
            Ok(SessionOutcome::Status(info))
        }
        fn process_start(&self, pid: u32, stop: &AtomicBool) -> Result<String, String> {
            let (status, output) = self.run_command(
                Command::new("/bin/ps")
                    .env("LC_ALL", "C")
                    .env("TZ", "UTC")
                    .args(["-p", &pid.to_string(), "-o", "stat=,lstart="]),
                stop,
            )?;
            if !status.success() || output.trim().is_empty() {
                return Err("Root provider process is no longer available".into());
            }
            let mut fields = output.split_whitespace();
            if fields.next().is_none_or(|state| state.starts_with('Z')) {
                return Err("Root provider process has exited".into());
            }
            Ok(fields.collect::<Vec<_>>().join(" "))
        }

        fn verify_root_exited(
            &self,
            identity: &ResumeIdentity,
            stop: &AtomicBool,
        ) -> Result<(), String> {
            if identity.observed_pid == 0
                || identity.observed_pid > i32::MAX as u32
                || identity.process_start.is_empty()
            {
                return Err(
                    "Saved root process identity is invalid; resume was not attempted".into(),
                );
            }
            let (status, output) = self.run_command(
                Command::new("/bin/ps")
                    .env("LC_ALL", "C")
                    .env("TZ", "UTC")
                    .args([
                        "-p",
                        &identity.observed_pid.to_string(),
                        "-o",
                        "stat=,lstart=",
                    ]),
                stop,
            )?;
            // ps reports an absent PID with exit 1 and no output. A timeout,
            // permission error or malformed reply does not prove an exit.
            if status.code() == Some(1) && output.trim().is_empty() {
                return Ok(());
            }
            if !status.success()
                || output
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .count()
                    != 1
            {
                return Err(
                    "Cannot verify that the saved root AI exited; resume was not attempted".into(),
                );
            }
            let mut fields = output.split_whitespace();
            let state = fields
                .next()
                .ok_or("Cannot inspect the saved root AI state")?;
            let started = fields.collect::<Vec<_>>().join(" ");
            if started.is_empty() {
                return Err("Cannot verify the saved root AI start time".into());
            }
            if state.starts_with('Z') || started != identity.process_start {
                return Ok(());
            }
            Err("The saved root AI is still running outside this terminal; refusing a duplicate conversation process".into())
        }

        fn verify_no_endpoint(&self, id: &str) -> Result<(), String> {
            if !self.sockets.exists() {
                return Ok(());
            }
            for (index, entry) in fs::read_dir(&self.sockets)
                .map_err(|e| e.to_string())?
                .enumerate()
            {
                if index >= 1024 {
                    return Err(
                        "Too many Screen endpoints to verify a safe transport upgrade".into(),
                    );
                }
                let entry = entry.map_err(|e| e.to_string())?;
                if entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.split_once('.'))
                    .is_some_and(|(_, session)| session == id)
                {
                    return Err(
                        "The previous Screen endpoint still exists; transport was not changed"
                            .into(),
                    );
                }
            }
            Ok(())
        }
        fn descendants(&self, supervisor: u32, stop: &AtomicBool) -> Result<Vec<Process>, String> {
            let (status, output) = self.run_bounded(
                Command::new("/bin/ps").env("LC_ALL", "C").args([
                    "-U",
                    &self.uid.to_string(),
                    "-o",
                    "pid=,ppid=,comm=",
                ]),
                stop,
                512 * 1024,
            )?;
            if !status.success() {
                return Err("Cannot inspect the managed provider process tree".into());
            }
            let rows: Vec<_> = output
                .lines()
                .filter_map(|line| {
                    let mut fields = line.split_whitespace();
                    let pid = fields.next()?.parse().ok()?;
                    let parent = fields.next()?.parse().ok()?;
                    let program = fields.collect::<Vec<_>>().join(" ");
                    Some(Process {
                        pid,
                        parent,
                        program,
                    })
                })
                .collect();
            let mut owned = std::collections::BTreeSet::from([supervisor]);
            for _ in 0..64 {
                let before = owned.len();
                for row in &rows {
                    if owned.contains(&row.parent) {
                        owned.insert(row.pid);
                    }
                }
                if owned.len() == before {
                    break;
                }
            }
            Ok(rows
                .into_iter()
                .filter(|row| row.pid != supervisor && owned.contains(&row.pid))
                .collect())
        }
        fn open_files(&self, pid: u32, stop: &AtomicBool) -> Result<Vec<PathBuf>, String> {
            let program = ["/usr/sbin/lsof", "/usr/bin/lsof"]
                .into_iter()
                .find(|p| Path::new(p).is_file())
                .ok_or(
                    "lsof is unavailable; this provider's conversation identity cannot be proven",
                )?;
            let (status, output) = self.run_bounded(
                Command::new(program).args(["-a", "-p", &pid.to_string(), "-Fn"]),
                stop,
                512 * 1024,
            )?;
            if !status.success() {
                return Err("Cannot read the root provider's open conversation files".into());
            }
            Ok(output
                .lines()
                .filter_map(|line| line.strip_prefix('n'))
                .filter(|path| path.starts_with('/') && !path.chars().any(char::is_control))
                .map(PathBuf::from)
                .collect())
        }
        fn discover_resume(
            &self,
            id: &str,
            info: &SessionInfo,
            stop: &AtomicBool,
        ) -> Result<Option<ResumeIdentity>, String> {
            let record = self
                .record(id)?
                .ok_or("Durable terminal identity is missing")?;
            let rows = self.descendants(info.supervisor_pid, stop)?;
            let providers: Vec<_> = rows
                .iter()
                .filter(|row| AgentProvider::from_program(&row.program).is_some())
                .collect();
            let roots: Vec<_> = providers
                .iter()
                .filter(|row| {
                    let mut parent = row.parent;
                    for _ in 0..64 {
                        if providers.iter().any(|candidate| candidate.pid == parent) {
                            return false;
                        }
                        let Some(ancestor) = rows.iter().find(|candidate| candidate.pid == parent)
                        else {
                            break;
                        };
                        parent = ancestor.parent;
                    }
                    true
                })
                .copied()
                .collect();
            let Some(root) = roots.first() else {
                if self
                    .launch_record(id)?
                    .is_some_and(|launch| launch.provider != AgentProvider::Shell)
                {
                    if let Some(saved) = self.saved_resume(id)? {
                        if self.process_start(saved.observed_pid, stop).ok().as_deref()
                            != Some(saved.process_start.as_str())
                        {
                            evidence_matches(&saved, self.uid)?;
                            return Ok(Some(saved));
                        }
                    }
                    return Err(
                        "The configured root AI is not observable yet; keeping its session running"
                            .into(),
                    );
                }
                return Ok(None);
            };
            if roots.len() != 1 {
                return Err("Several independent root AIs share this terminal; their resume identity is ambiguous".into());
            }
            let provider = AgentProvider::from_program(&root.program).unwrap();
            let start = self.process_start(root.pid, stop)?;
            let home = provider_home(provider)?;
            let expected_cwd = Path::new(&record.cwd)
                .canonicalize()
                .map_err(|_| "Agent working directory is unavailable")?;
            let mut candidates = Vec::new();
            if provider == AgentProvider::Fable {
                let path = home.join("sessions").join(format!("{}.json", root.pid));
                let text = read_regular(&path, 32 * 1024, self.uid)?
                    .ok_or("Claude has not published its root session metadata")?;
                let metadata = json::parse_depth(text.as_bytes(), 12)
                    .map_err(|_| "Invalid Claude session metadata")?;
                let meta_start = metadata
                    .get("procStart")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                #[cfg(target_os = "linux")]
                let linux_start = fs::read_to_string(format!("/proc/{}/stat", root.pid))
                    .ok()
                    .and_then(|s| {
                        s.rsplit_once(')')
                            .and_then(|(_, tail)| tail.split_whitespace().nth(19))
                            .map(str::to_owned)
                    });
                #[cfg(not(target_os = "linux"))]
                let linux_start: Option<String> = None;
                if metadata.get("pid").and_then(Value::as_u64) != Some(root.pid as u64)
                    || (meta_start != start && linux_start.as_deref() != Some(meta_start.as_str()))
                    || metadata.get("kind").and_then(Value::as_str) != Some("interactive")
                {
                    return Err("Claude metadata does not match the live root process; refusing PID reuse or a child session".into());
                }
                let conversation = metadata
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .filter(|v| uuid(v))
                    .ok_or("Claude has no valid conversation UUID")?;
                let cwd = metadata
                    .get("cwd")
                    .and_then(Value::as_str)
                    .ok_or("Claude metadata has no working directory")?;
                if Path::new(cwd).canonicalize().ok().as_ref() != Some(&expected_cwd) {
                    return Err("Claude root session belongs to another working directory".into());
                }
                let slug: String = cwd
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                    .collect();
                let direct = home
                    .join("projects")
                    .join(slug)
                    .join(format!("{conversation}.jsonl"));
                if direct.is_file() {
                    candidates.push((conversation.to_owned(), direct));
                } else {
                    for entry in fs::read_dir(home.join("projects"))
                        .map_err(|_| "Claude project history is unavailable")?
                        .take(2048)
                        .flatten()
                    {
                        if entry.file_type().is_ok_and(|ty| ty.is_dir()) {
                            let path = entry.path().join(format!("{conversation}.jsonl"));
                            if path.is_file() {
                                candidates.push((conversation.to_owned(), path));
                            }
                        }
                    }
                }
            } else {
                for path in self.open_files(root.pid, stop)? {
                    let Ok(path) = path.canonicalize() else {
                        continue;
                    };
                    if !path.starts_with(&home) {
                        continue;
                    }
                    if provider == AgentProvider::Codex
                        && path
                            .file_name()
                            .and_then(|p| p.to_str())
                            .is_some_and(|p| p.starts_with("rollout-") && p.ends_with(".jsonl"))
                    {
                        for row in metadata_lines(&path, self.uid)?.into_iter().take(1) {
                            if row.get("type").and_then(Value::as_str) != Some("session_meta") {
                                continue;
                            }
                            let payload = row.get("payload").unwrap_or(&Value::Null);
                            if payload.get("source").and_then(Value::as_str) != Some("cli") {
                                continue;
                            }
                            if let Some(conversation) = payload
                                .get("id")
                                .and_then(Value::as_str)
                                .filter(|v| uuid(v))
                            {
                                candidates.push((conversation.to_owned(), path.clone()));
                            }
                        }
                    } else if provider == AgentProvider::Grok {
                        for parent in path.ancestors().take(3) {
                            let Some(conversation) = parent
                                .file_name()
                                .and_then(|p| p.to_str())
                                .filter(|v| uuid(v))
                            else {
                                continue;
                            };
                            let summary = parent.join("summary.json");
                            if summary.is_file() {
                                candidates.push((conversation.to_owned(), summary));
                            }
                        }
                    }
                }
            }
            candidates.sort();
            candidates.dedup();
            let mut verified = Vec::new();
            for (conversation_id, path) in candidates {
                let identity = ResumeIdentity {
                    provider,
                    conversation_id,
                    cwd: expected_cwd.to_string_lossy().into(),
                    evidence_path: path.to_string_lossy().into(),
                    program: executable(provider)?.to_string_lossy().into(),
                    provider_home: home.to_string_lossy().into(),
                    observed_pid: root.pid,
                    process_start: start.clone(),
                    verified_at_ms: timestamp_ms(),
                };
                if evidence_matches(&identity, self.uid).is_ok() {
                    verified.push(identity);
                }
            }
            if verified.len() != 1 {
                return Err("Cannot prove one persisted root conversation for this AI; session remains running".into());
            }
            if self.process_start(root.pid, stop)? != start {
                return Err(
                    "Root AI changed during identity capture; retry before stopping".into(),
                );
            }
            let identity = verified.pop().unwrap();
            if let Some(recovery) = self.recovery_record(id)? {
                if recovery.identity.provider != identity.provider
                    || recovery.identity.conversation_id != identity.conversation_id
                {
                    return Err("The observed provider differs from the saved recovery conversation; its original identity was retained".into());
                }
            }
            let mut prior = self.saved_resume(id)?;
            if let Some(old) = &mut prior {
                old.verified_at_ms = identity.verified_at_ms;
            }
            if prior.as_ref() != Some(&identity) {
                replace_state(
                    &self.records,
                    &format!("{id}.resume.ron"),
                    &identity.serialize_ron(),
                )?;
            }
            let mut launch = self.launch_record(id)?.unwrap_or(LaunchRecord {
                version: 1,
                provider,
                program: identity.program.clone(),
                environment: Vec::new(),
            });
            if launch.provider != provider
                || launch.program != identity.program
                || self.launch_record(id)?.is_none()
            {
                launch.provider = provider;
                launch.program = identity.program.clone();
                replace_state(
                    &self.records,
                    &format!("{id}.provider.ron"),
                    &launch.serialize_ron(),
                )?;
            }
            Ok(Some(identity))
        }

        fn observed(&self, id: &str, mut info: SessionInfo, stop: &AtomicBool) -> SessionInfo {
            match self.recovery_info(id, true) {
                Ok(Some(recovery)) => {
                    info.provider = recovery.provider;
                    info.resume = self.saved_resume(id).ok().flatten();
                    let probe = matches!(
                        recovery.phase,
                        RecoveryPhase::Resuming | RecoveryPhase::Resumed | RecoveryPhase::Ended
                    );
                    info.recovery = Some(recovery);
                    if !probe {
                        return info;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    info.resume_error = Some(error);
                    return info;
                }
            }
            match self.launch_record(id) {
                Ok(Some(launch)) => info.provider = launch.provider,
                _ => {}
            }
            match self.discover_resume(id, &info, stop) {
                Ok(Some(identity)) => {
                    info.provider = identity.provider;
                    info.resume = Some(identity);
                }
                Ok(None) => info.resume = self.saved_resume(id).ok().flatten(),
                Err(error) => {
                    info.resume = self.saved_resume(id).ok().flatten();
                    info.resume_error = Some(error);
                }
            }
            if info.resume_error.is_none()
                && info
                    .recovery
                    .as_ref()
                    .is_some_and(|recovery| recovery.phase == RecoveryPhase::Resuming)
            {
                if let (Some(identity), Ok(Some(record))) = (&info.resume, self.recovery_record(id))
                {
                    if identity.observed_pid != record.identity.observed_pid
                        || identity.process_start != record.identity.process_start
                    {
                        if replace_state(&self.records, &record.stage_file, "resumed\n").is_ok() {
                            info.recovery = self.recovery_info(id, true).ok().flatten();
                        }
                    }
                }
            }
            info
        }

        fn live(&self, id: &str, stop: &AtomicBool) -> Result<Option<SessionInfo>, String> {
            let (status, output) = self.run_command(
                self.command()
                    .arg("status")
                    .arg("--state-dir")
                    .arg(&self.records)
                    .arg("--session")
                    .arg(id),
                stop,
            )?;
            if !status.success() {
                return Err(format!("Cannot inspect Studio PTY: {}", output.trim()));
            }
            let value = json::parse(output.as_bytes())
                .map_err(|e| format!("Invalid Studio PTY status: {e}"))?;
            self.session_info(id, &value)
        }

        fn session_info(&self, id: &str, value: &Value) -> Result<Option<SessionInfo>, String> {
            if value.get("running").and_then(Value::as_bool) == Some(false) {
                return Ok(None);
            }
            if value.get("running").and_then(Value::as_bool) != Some(true) {
                return Err("Missing PTY running state".into());
            }
            if value.get("version").and_then(Value::as_u64) != Some(1)
                || value.get("session_id").and_then(Value::as_str) != Some(id)
            {
                return Err("Studio PTY returned a different session identity".into());
            }
            let pid = value
                .get("pid")
                .and_then(Value::as_u64)
                .filter(|pid| *pid > 1 && *pid <= i32::MAX as u64)
                .ok_or("Studio PTY returned an invalid supervisor PID")?
                as u32;
            let state_dir = value
                .get("state_dir")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .unwrap_or_else(|| self.records.clone());
            let attach_command = format!(
                "exec {} attach --state-dir {} --session {}",
                shell_quote(self.program.as_os_str())?,
                shell_quote(state_dir.as_os_str())?,
                shell_quote(OsStr::new(id))?
            );
            Ok(Some(SessionInfo {
                session_id: id.into(),
                state_dir,
                title: value
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(id)
                    .into(),
                activity: value
                    .get("activity")
                    .and_then(Value::as_str)
                    .unwrap_or("running")
                    .into(),
                started_at_ms: value
                    .get("started_at_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                supervisor_pid: pid,
                attach_command,
                backend: "Makepad Screen",
                transport_program: self.program.to_string_lossy().into(),
                transport_version: "1".into(),
                transport_warning: None,
                scrollback_lines: SCROLLBACK_LINES,
                cwd: PathBuf::from(
                    value
                        .get("cwd")
                        .and_then(Value::as_str)
                        .ok_or("Missing session working directory")?,
                ),
                clients: value.get("clients").and_then(Value::as_u64).unwrap_or(0) as usize,
                provider: self
                    .launch_record(id)?
                    .map(|launch| launch.provider)
                    .or_else(|| {
                        value
                            .get("program")
                            .and_then(Value::as_str)
                            .and_then(AgentProvider::from_program)
                    })
                    .unwrap_or(AgentProvider::Shell),
                resume: None,
                resume_error: None,
                recovery: None,
            }))
        }

        fn view_links(&self) -> Result<Vec<(u64, String)>, String> {
            let Some(text) = read_regular(
                &self.records.join("terminal-views.ron"),
                32 * 1024,
                self.uid,
            )?
            else {
                return Ok(Vec::new());
            };
            let links = Vec::<(u64, String)>::deserialize_ron(&text)
                .map_err(|_| "Invalid terminal view links")?;
            if links.len() > 256 {
                return Err("Too many terminal view links".into());
            }
            for (_, id) in &links {
                view_target(id, &self.records)?;
            }
            Ok(links)
        }

        fn inventory(&self, repo: &Path, stop: &AtomicBool) -> Result<SessionOutcome, String> {
            let mut sessions = Vec::new();
            for studio in [true, false] {
                let mut command = self.command();
                command.arg("list");
                if studio {
                    command.arg("--state-dir").arg(&self.records);
                } else {
                    command.arg("--cwd").arg(repo);
                }
                let (status, output) = self.run_command(&mut command, stop)?;
                if !status.success() {
                    return Err(format!("Cannot list running agents: {}", output.trim()));
                }
                let value = json::parse(output.as_bytes()).map_err(|e| e.to_string())?;
                let values = value
                    .as_arr()
                    .or_else(|| value.get("sessions").and_then(Value::as_arr))
                    .ok_or("Invalid agent inventory")?;
                if values.len() > 1024 {
                    return Err("Agent inventory exceeds its bound".into());
                }
                for value in values {
                    let id = value
                        .get("session_id")
                        .and_then(Value::as_str)
                        .ok_or("Missing session identity")?;
                    validate_id(id)?;
                    if let Some(info) = self.session_info(id, value)? {
                        if !sessions
                            .iter()
                            .any(|old: &SessionInfo| old.key() == info.key())
                        {
                            sessions.push(info);
                        }
                    }
                }
            }
            Ok(SessionOutcome::Inventory {
                state_dir: self.records.clone(),
                sessions,
                views: self.view_links()?,
            })
        }

        fn select_view(
            &self,
            tab: u64,
            selected: Option<String>,
            stop: &AtomicBool,
        ) -> Result<SessionOutcome, String> {
            let session = if let Some(id) = &selected {
                let (records, id) = view_target(id, &self.records)?;
                let mut backend = self.clone();
                backend.records = records;
                Some(
                    backend
                        .live(&id, stop)?
                        .ok_or("The selected agent has ended")?,
                )
            } else {
                None
            };
            let mut views = self.view_links()?;
            views.retain(|(existing, _)| *existing != tab);
            if let Some(id) = selected {
                if views.len() >= 256 {
                    return Err("Too many terminal view connections".into());
                }
                views.push((tab, id));
            }
            replace_state(&self.records, "terminal-views.ron", &views.serialize_ron())?;
            Ok(SessionOutcome::ViewReady { tab, session })
        }

        fn existing(&self, id: &str, stop: &AtomicBool) -> Result<Option<SessionInfo>, String> {
            if self.record(id)?.is_none() {
                return Err(
                    "Unknown durable agent session; reattachment cannot create a replacement"
                        .into(),
                );
            }
            self.live(id, stop)
                .map(|info| info.map(|info| self.observed(id, info, stop)))
        }

        fn attach_existing(&self, id: &str, stop: &AtomicBool) -> Result<SessionOutcome, String> {
            if let Some(info) = self.existing(id, stop)? {
                return Ok(SessionOutcome::Ready(info));
            }
            if let Some(recovery) = self.recovery_info(id, false)? {
                return Ok(SessionOutcome::RecoveryStatus(None, recovery));
            }
            Err("Agent session ended or is unavailable; no replacement process was started".into())
        }

        fn launch_provider(
            &self,
            record: &Record,
            launch: &LaunchRecord,
            resume: Option<&ResumeIdentity>,
            stop: &AtomicBool,
        ) -> Result<SessionOutcome, String> {
            let mut command = self.provider_command(record, launch, resume)?;
            self.launch_prepared_provider(record, &mut command, stop, &mut false)
        }

        fn provider_command(
            &self,
            record: &Record,
            launch: &LaunchRecord,
            resume: Option<&ResumeIdentity>,
        ) -> Result<Command, String> {
            if launch.version != 1
                || !Path::new(&launch.program).is_absolute()
                || !fs::metadata(&launch.program)
                    .is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0)
            {
                return Err(
                    "The saved provider executable is unavailable; no fresh chat was started"
                        .into(),
                );
            }
            if let Some(identity) = resume {
                if identity.evidence_path.is_empty() {
                    crate::iteration::validate_resume_token(&identity.conversation_id)?;
                } else {
                    evidence_matches(identity, self.uid)?;
                }
            }
            let cwd = Path::new(&record.cwd)
                .canonicalize()
                .map_err(|_| "The agent worktree is unavailable")?;
            let mut command = self.start_command(&record.session_id, &cwd, resume.is_some());
            command.arg(&launch.program);
            for (key, value) in &launch.environment {
                if !matches!(
                    key.as_str(),
                    "MAKEPAD_STUDIO_FLOW_ID"
                        | "MAKEPAD_STUDIO_CONTROL_DIR"
                        | "MAKEPAD_STUDIO_CLI"
                        | "MAKEPAD_STUDIO_MCP_TOKEN"
                ) || value.len() > 4096
                    || value.chars().any(char::is_control)
                {
                    return Err("Saved Studio provider environment is invalid".into());
                }
                command.env(key, value);
            }
            if launch.provider == AgentProvider::Shell {
                command.arg("-l");
            } else {
                if let Some(identity) = resume {
                    if identity.provider != launch.provider {
                        return Err("Saved provider and conversation identities disagree".into());
                    }
                    match launch.provider {
                        AgentProvider::Codex => {
                            command
                                .arg("resume")
                                .arg(&identity.conversation_id)
                                .env("CODEX_HOME", &identity.provider_home);
                        }
                        AgentProvider::Fable => {
                            command
                                .arg("--resume")
                                .arg(&identity.conversation_id)
                                .env("CLAUDE_CONFIG_DIR", &identity.provider_home);
                        }
                        AgentProvider::Grok => {
                            command
                                .arg("--resume")
                                .arg(&identity.conversation_id)
                                .env("GROK_HOME", &identity.provider_home);
                        }
                        _ => return Err("This provider has no verified resume adapter".into()),
                    }
                }
                command.arg(PROVIDER_BOOTSTRAP);
            }
            Ok(command)
        }

        fn launch_prepared_provider(
            &self,
            record: &Record,
            command: &mut Command,
            stop: &AtomicBool,
            spawned: &mut bool,
        ) -> Result<SessionOutcome, String> {
            let (status, _) =
                self.run_bounded_tracking_spawn(command, stop, MAX_OUTPUT, spawned)?;
            if !status.success() {
                return Err(
                    "Provider terminal launch failed; its saved conversation identity was retained"
                        .into(),
                );
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if let Some(info) = self.live(&record.session_id, stop)? {
                    return Ok(SessionOutcome::Ready(self.observed(
                        &record.session_id,
                        info,
                        stop,
                    )));
                }
                if Instant::now() >= deadline {
                    return Err(
                        "Provider exited during startup; it will not be retried as a fresh chat"
                            .into(),
                    );
                }
                std::thread::sleep(Duration::from_millis(30));
            }
        }

        fn prepare_provider(
            &self,
            spec: SessionSpec,
            provider: AgentProvider,
            stop: &AtomicBool,
        ) -> Result<SessionOutcome, String> {
            let cwd = spec
                .cwd
                .canonicalize()
                .map_err(|_| "Provider working directory is unavailable")?;
            if let Some(record) = self.record(&spec.session_id)? {
                if Path::new(&record.cwd).canonicalize().ok().as_ref() != Some(&cwd) {
                    return Err("Saved agent session belongs to another worktree; it was not attached, stopped or replaced".into());
                }
                return self.attach_existing(&spec.session_id, stop);
            }
            if self.live(&spec.session_id, stop)?.is_some() {
                return Err(
                    "Unrecorded live session has this identity; refusing to replace it".into(),
                );
            }
            let mut environment = studio_environment(spec.command.as_deref())?;
            // The lane's MCP credential: minted here, owned by the durable
            // terminal origin (the session id), exported through the same
            // allowlisted launch environment so a re-attach after a Studio
            // restart keeps it. The endpoint URL is wired by the provider
            // launch profile, not here.
            environment.retain(|(key, _)| key != "MAKEPAD_STUDIO_MCP_TOKEN");
            let token = self.mcp_tokens.mint(&spec.session_id, &spec.session_id)?;
            environment.push(("MAKEPAD_STUDIO_MCP_TOKEN".into(), token.as_str().into()));
            let launch = LaunchRecord {
                version: 1,
                provider,
                program: executable(provider)?.to_string_lossy().into(),
                environment,
            };
            let record = Record {
                version: 1,
                session_id: spec.session_id,
                cwd: cwd.to_string_lossy().into(),
                created_at_ms: timestamp_ms(),
            };
            if !create_record(
                &self.records,
                &format!("{}.ron", record.session_id),
                &record.serialize_ron(),
            )? {
                return Err("This provider lane was concurrently claimed; inspect it first".into());
            }
            replace_state(
                &self.records,
                &format!("{}.provider.ron", record.session_id),
                &launch.serialize_ron(),
            )?;
            let resume = spec
                .resume_conversation
                .as_ref()
                .map(|token| -> Result<ResumeIdentity, String> {
                    let token = crate::iteration::validate_resume_token(token)?;
                    Ok(ResumeIdentity {
                        provider,
                        conversation_id: token,
                        cwd: record.cwd.clone(),
                        evidence_path: String::new(),
                        program: launch.program.clone(),
                        provider_home: provider_home(provider)?.to_string_lossy().into(),
                        observed_pid: 0,
                        process_start: String::new(),
                        verified_at_ms: 0,
                    })
                })
                .transpose()?;
            self.launch_provider(&record, &launch, resume.as_ref(), stop)
        }

        fn capture_resume(&self, id: &str, stop: &AtomicBool) -> Result<SessionOutcome, String> {
            if let Some(info) = self.existing(id, stop)? {
                if let Some(error) = info.resume_error {
                    return Err(error);
                }
                return info
                    .resume
                    .map(SessionOutcome::ResumeCaptured)
                    .ok_or_else(|| "This shell has no proven AI conversation to resume".into());
            }
            let identity = self
                .saved_resume(id)?
                .ok_or("The ended terminal has no saved AI conversation")?;
            evidence_matches(&identity, self.uid)?;
            Ok(SessionOutcome::ResumeCaptured(identity))
        }

        fn restore(&self, id: &str, stop: &AtomicBool) -> Result<SessionOutcome, String> {
            if let Some(info) = self.existing(id, stop)? {
                return Ok(SessionOutcome::Ready(info));
            }
            let record = self.record(id)?.ok_or("Unknown terminal identity")?;
            let saved = self.saved_resume(id)?;
            let launch = if let Some(identity) = &saved {
                evidence_matches(identity, self.uid)?;
                self.verify_root_exited(identity, stop)?;
                let mut launch = self.launch_record(id)?.unwrap_or(LaunchRecord {
                    version: 1,
                    provider: identity.provider,
                    program: identity.program.clone(),
                    environment: Vec::new(),
                });
                launch.provider = identity.provider;
                launch.program = identity.program.clone();
                launch
            } else {
                let launch = self.launch_record(id)?.unwrap_or(LaunchRecord {
                    version: 1,
                    provider: AgentProvider::Shell,
                    program: std::env::var("SHELL")
                        .ok()
                        .filter(|s| Path::new(s).is_absolute())
                        .unwrap_or_else(|| "/bin/sh".into()),
                    environment: Vec::new(),
                });
                if launch.provider != AgentProvider::Shell {
                    return Err("No proven conversation identity was saved; refusing to start a fresh AI chat".into());
                }
                launch
            };
            let mut command = self.provider_command(&record, &launch, saved.as_ref())?;
            if saved.is_none() {
                command = self.start_command(&record.session_id, Path::new(&record.cwd), true);
                command.arg(&launch.program).arg("-l");
                for (key, value) in &launch.environment {
                    command.env(key, value);
                }
            }
            self.launch_prepared_provider(&record, &mut command, stop, &mut false)
        }

        fn restore_environment(
            &self,
            id: &str,
            command: &str,
            expected_cwd: &Path,
            stop: &AtomicBool,
        ) -> Result<SessionOutcome, String> {
            let record = self
                .record(id)?
                .ok_or("Unknown durable terminal; restore cannot create a new identity")?;
            let cwd = expected_cwd
                .canonicalize()
                .map_err(|_| "Flow worktree is unavailable")?;
            if Path::new(&record.cwd).canonicalize().ok().as_ref() != Some(&cwd) {
                return Err("Saved agent session belongs to another worktree; restore will not move or replace it".into());
            }
            let environment = studio_environment(Some(command))?;
            let mut launch = self.launch_record(id)?.unwrap_or(LaunchRecord {
                version: 1,
                provider: AgentProvider::Shell,
                program: std::env::var("SHELL")
                    .ok()
                    .filter(|s| Path::new(s).is_absolute())
                    .unwrap_or_else(|| "/bin/sh".into()),
                environment: Vec::new(),
            });
            launch.environment = environment;
            replace_state(
                &self.records,
                &format!("{id}.provider.ron"),
                &launch.serialize_ron(),
            )?;
            self.restore(id, stop)
        }

        fn recover(
            &self,
            id: &str,
            studio_command: &str,
            expected_cwd: &Path,
            stop: &AtomicBool,
        ) -> Result<SessionOutcome, String> {
            let record = self
                .record(id)?
                .ok_or("Recovery requires an existing durable agent identity")?;
            let cwd = expected_cwd
                .canonicalize()
                .map_err(|_| "Flow worktree is unavailable")?;
            if Path::new(&record.cwd).canonicalize().ok().as_ref() != Some(&cwd) {
                return Err("Recovery refused: this terminal belongs to another worktree".into());
            }
            let environment = studio_environment(Some(studio_command))?;
            let current = self.existing(id, stop)?;
            if current
                .as_ref()
                .and_then(|info| info.recovery.as_ref())
                .is_some_and(|recovery| recovery.phase.active())
            {
                return Err("Account recovery is already running in this terminal; complete its browser sign-in".into());
            }
            if let Some(info) = &current {
                if let Some(error) = &info.resume_error {
                    return Err(format!("Recovery blocked before account changes: {error}"));
                }
                if info.provider == AgentProvider::Fable {
                    if info.resume.is_none() {
                        return Err(
                            "Cannot prove this is the live Fable root; /login was not submitted"
                                .into(),
                        );
                    }
                    return Ok(SessionOutcome::FableLoginReady(info.clone()));
                }
            }
            let identity = current
                .as_ref()
                .and_then(|info| info.resume.clone())
                .or(self.saved_resume(id)?)
                .ok_or("Recovery blocked: no verified conversation identity was saved")?;
            if identity.provider != AgentProvider::Codex {
                return Err("Only a live Fable root or a saved Codex conversation supports account recovery".into());
            }
            evidence_matches(&identity, self.uid)?;
            if !fs::metadata(&identity.program)
                .is_ok_and(|meta| meta.is_file() && meta.mode() & 0o111 != 0)
            {
                return Err(
                    "Saved Codex executable is unavailable; no account changes were made".into(),
                );
            }
            // Preserve the proof before either terminating this root or touching
            // credentials. This method is only reached by an explicit action.
            replace_state(
                &self.records,
                &format!("{id}.resume.ron"),
                &identity.serialize_ron(),
            )?;
            if current.is_some() {
                self.stop_session(id, stop)?;
            }
            if self
                .process_start(identity.observed_pid, stop)
                .ok()
                .as_deref()
                == Some(identity.process_start.as_str())
            {
                return Err("The saved root is still alive; recovery will not change its account or duplicate it".into());
            }
            let launch = LaunchRecord {
                version: 1,
                provider: AgentProvider::Codex,
                program: identity.program.clone(),
                environment,
            };
            replace_state(
                &self.records,
                &format!("{id}.provider.ron"),
                &launch.serialize_ron(),
            )?;
            let stage_file = format!("{id}.recovery-{}.stage", new_session_id(0));
            let recovery = RecoveryRecord {
                version: 1,
                identity: identity.clone(),
                stage_file: stage_file.clone(),
                started_at_ms: timestamp_ms(),
            };
            replace_state(&self.records, &stage_file, "preserving_identity\n")?;
            let previous = self.recovery_record(id)?;
            replace_state(
                &self.records,
                &format!("{id}.recovery.ron"),
                &recovery.serialize_ron(),
            )?;
            if let Some(previous) = previous {
                let _ = fs::remove_file(self.records.join(previous.stage_file));
            }
            // A fixed script with positional arguments, never interpolated
            // user shell text. The PTY host owns it through UI/worker restarts. Its
            // only files are bounded stage words; auth output stays in the PTY.
            const RECOVER: &str = r#"set -u
stage_file=$1
program=$2
conversation=$3
bootstrap=$4
stage() {
    (umask 077; set -C; printf '%s\n' "$1" > "${stage_file}.next") && /bin/mv -f "${stage_file}.next" "$stage_file"
}
stage logging_out || exit 90
if ! "$program" logout; then stage logout_failed; exit 1; fi
stage browser_login || exit 90
if ! "$program" login; then stage login_failed; exit 1; fi
if ! "$program" login status >/dev/null 2>&1; then stage login_failed; exit 1; fi
stage resuming || exit 90
"$program" resume "$conversation" "$bootstrap"
result=$?
if [ "$result" -eq 0 ]; then stage ended; else stage resume_failed; fi
exit "$result"
"#;
            let mut command = self.start_command(id, &cwd, true);
            command
                .args(["/bin/sh", "-c", RECOVER, "studio-codex-recovery"])
                .arg(self.records.join(stage_file))
                .arg(&identity.program)
                .arg(&identity.conversation_id)
                .arg(PROVIDER_BOOTSTRAP)
                .current_dir(cwd)
                .env("CODEX_HOME", &identity.provider_home);
            for (key, value) in &launch.environment {
                command.env(key, value);
            }
            let (status, _) = self.run_command(&mut command, stop)?;
            if !status.success() {
                return Err("Recovery terminal could not start; the saved conversation remains available and login was not retried".into());
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if let Some(info) = self.live(id, stop)? {
                    return Ok(SessionOutcome::Ready(self.observed(id, info, stop)));
                }
                if let Some(recovery) = self.recovery_info(id, false)? {
                    if matches!(recovery.phase, RecoveryPhase::Failed | RecoveryPhase::Ended) {
                        return Ok(SessionOutcome::RecoveryStatus(None, recovery));
                    }
                }
                if Instant::now() >= deadline {
                    return Ok(SessionOutcome::RecoveryStatus(
                        None,
                        self.recovery_info(id, false)?
                            .ok_or("Recovery state is unavailable")?,
                    ));
                }
                std::thread::sleep(Duration::from_millis(30));
            }
        }

        fn prepare(&self, spec: SessionSpec, stop: &AtomicBool) -> Result<SessionOutcome, String> {
            let cwd =
                fs::canonicalize(&spec.cwd).map_err(|e| format!("Agent working directory: {e}"))?;
            if !cwd.is_dir() {
                return Err("Agent working directory is not a directory".into());
            }
            if let Some(record) = self.record(&spec.session_id)? {
                if Path::new(&record.cwd).canonicalize().ok().as_ref() != Some(&cwd) {
                    return Err("Saved agent session belongs to another worktree; it was not attached, stopped or replaced".into());
                }
                return self.attach_existing(&spec.session_id, stop);
            }
            if self.live(&spec.session_id, stop)?.is_some() {
                return Err("A live server has this identity without its creation record; refusing to adopt it".into());
            }
            let record = Record {
                version: 1,
                session_id: spec.session_id.clone(),
                cwd: cwd
                    .to_str()
                    .ok_or("Agent working directory must be valid UTF-8")?
                    .into(),
                created_at_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
            };
            if !create_record(
                &self.records,
                &format!("{}.ron", spec.session_id),
                &record.serialize_ron(),
            )? {
                return self.existing(&spec.session_id, stop)?.map(SessionOutcome::Ready).ok_or_else(|| "This session was already claimed; inspect it before requesting another session".into());
            }
            let shell = std::env::var_os("SHELL")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute() && p.is_file())
                .unwrap_or_else(|| PathBuf::from("/bin/sh"));
            let launch = LaunchRecord {
                version: 1,
                provider: AgentProvider::Shell,
                program: shell.to_string_lossy().into(),
                environment: studio_environment(spec.command.as_deref()).unwrap_or_default(),
            };
            replace_state(
                &self.records,
                &format!("{}.provider.ron", spec.session_id),
                &launch.serialize_ron(),
            )?;
            let mut command = self.start_command(&spec.session_id, &cwd, false);
            command.arg(shell).arg("-l");
            if let Some(text) = spec.command {
                command.arg("-c").arg(text);
            }
            let (status, output) = self.run_command(&mut command, stop)?;
            if !status.success() {
                return Err(format!(
                    "Makepad Screen could not start this session (identity retained): {}",
                    output.trim()
                ));
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if let Some(info) = self.live(&spec.session_id, stop)? {
                    return Ok(SessionOutcome::Ready(self.observed(
                        &spec.session_id,
                        info,
                        stop,
                    )));
                }
                if Instant::now() >= deadline {
                    return Err("Agent session exited during startup or the Studio PTY host is unavailable; the command will not be automatically rerun".into());
                }
                std::thread::sleep(Duration::from_millis(30));
            }
        }

        fn stop_session(&self, id: &str, stop: &AtomicBool) -> Result<SessionOutcome, String> {
            let Some(info) = self.existing(id, stop)? else {
                return Ok(self
                    .saved_resume(id)?
                    .map(SessionOutcome::StoppedWithResume)
                    .unwrap_or(SessionOutcome::Stopped));
            };
            if info
                .recovery
                .as_ref()
                .is_some_and(|recovery| recovery.phase.active())
            {
                return Err("Complete or cancel the active browser login in its terminal before stopping this lane".into());
            }
            if let Some(error) = &info.resume_error {
                return Err(format!("Stop blocked: {error}"));
            }
            if info.provider != AgentProvider::Shell && info.resume.is_none() {
                return Err("Stop blocked: no proven root conversation was saved; the agent remains running".into());
            }
            // Explicit Stop first signals only the proven root, then asks
            // our PTY host to finish its owned process group;
            // worker shutdown and terminal detach never enter this path.
            if let Some(identity) = &info.resume {
                if self
                    .process_start(identity.observed_pid, stop)
                    .ok()
                    .as_deref()
                    == Some(identity.process_start.as_str())
                {
                    let (status, _) = self.run_command(
                        Command::new("/bin/kill")
                            .args(["-TERM", &identity.observed_pid.to_string()]),
                        stop,
                    )?;
                    if !status.success()
                        && self
                            .process_start(identity.observed_pid, stop)
                            .ok()
                            .as_deref()
                            == Some(identity.process_start.as_str())
                    {
                        return Err("Stop could not signal the proven root AI; its session and resume identity are retained".into());
                    }
                    let deadline = Instant::now() + Duration::from_secs(3);
                    while self
                        .process_start(identity.observed_pid, stop)
                        .ok()
                        .as_deref()
                        == Some(identity.process_start.as_str())
                    {
                        if Instant::now() >= deadline {
                            return Err("The root AI has not acknowledged Stop; its terminal and resume identity are retained".into());
                        }
                        std::thread::sleep(Duration::from_millis(30));
                    }
                }
            }
            if self.live(id, stop)?.is_none() {
                return Ok(info
                    .resume
                    .map(SessionOutcome::StoppedWithResume)
                    .unwrap_or(SessionOutcome::Stopped));
            }
            let (status, output) = self.run_command(
                self.command()
                    .arg("stop")
                    .arg("--state-dir")
                    .arg(&self.records)
                    .arg("--session")
                    .arg(id),
                stop,
            )?;
            if !status.success() {
                return Err(format!(
                    "Makepad Screen did not acknowledge Stop agent: {}",
                    output.trim()
                ));
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            while self.live(id, stop)?.is_some() {
                if Instant::now() >= deadline {
                    return Err(
                        "The agent session still appears live after Stop; inspect before retrying"
                            .into(),
                    );
                }
                std::thread::sleep(Duration::from_millis(30));
            }
            if let Some(identity) = &info.resume {
                while self
                    .process_start(identity.observed_pid, stop)
                    .ok()
                    .as_deref()
                    == Some(identity.process_start.as_str())
                {
                    if Instant::now() >= deadline {
                        return Err("The terminal ended but its root AI has not exited; Stop is not confirmed and the resume identity is retained".into());
                    }
                    std::thread::sleep(Duration::from_millis(30));
                }
            }
            // The lane's terminal lifecycle has ended: its MCP credential
            // stops working now. A failed revoke is reported, never hidden.
            self.mcp_tokens.revoke(id)?;
            Ok(info
                .resume
                .map(SessionOutcome::StoppedWithResume)
                .unwrap_or(SessionOutcome::Stopped))
        }
    }

    pub(super) fn delete_lane_session(state: PathBuf, tab: u64, id: &str) -> Result<(), String> {
        let backend = Backend::open(state)?;
        let stop = AtomicBool::new(false);
        backend.stop_session(id, &stop)?;
        backend.select_view(tab, None, &stop)?;
        backend.mcp_tokens.revoke(id)?;
        Ok(())
    }

    pub(super) fn run(
        state_dir: PathBuf,
        commands: Receiver<Request>,
        replies: SyncSender<SessionReply>,
        stop: Arc<AtomicBool>,
    ) {
        let backend = Backend::open(state_dir);
        while !stop.load(Ordering::Relaxed) {
            let request = match commands.recv_timeout(Duration::from_millis(50)) {
                Ok(request) => request,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            let result = match &backend {
                Err(error) => Err(error.clone()),
                Ok(backend) if matches!(&request.operation, Operation::List(_)) => {
                    if let Operation::List(repo) = request.operation {
                        backend.inventory(&repo, &stop)
                    } else {
                        unreachable!()
                    }
                }
                Ok(backend) if matches!(&request.operation, Operation::NewView(_, _)) => {
                    if let Operation::NewView(tab, spec) = request.operation {
                        backend
                            .for_session(&spec.session_id, true, false, &stop)
                            .and_then(|session| session.prepare(spec, &stop))
                            .and_then(|outcome| {
                                if let SessionOutcome::Ready(info) = outcome {
                                    backend.select_view(tab, Some(info.key()), &stop)
                                } else {
                                    Err("New terminal did not become ready".into())
                                }
                            })
                    } else {
                        unreachable!()
                    }
                }
                Ok(backend) if matches!(&request.operation, Operation::SelectView(_, _)) => {
                    if let Operation::SelectView(tab, session) = request.operation {
                        backend.select_view(tab, session, &stop)
                    } else {
                        unreachable!()
                    }
                }
                Ok(backend) => backend
                    .for_session(
                        &request.session_id,
                        matches!(
                            &request.operation,
                            Operation::Prepare(_) | Operation::PrepareProvider(_, _)
                        ),
                        matches!(
                            &request.operation,
                            Operation::Restore | Operation::RestoreEnvironment(_, _)
                        ),
                        &stop,
                    )
                    .and_then(|backend| match request.operation {
                        Operation::List(_)
                        | Operation::NewView(_, _)
                        | Operation::SelectView(_, _) => unreachable!(),
                        Operation::Prepare(spec) => backend.prepare(spec, &stop),
                        Operation::PrepareProvider(spec, provider) => {
                            backend.prepare_provider(spec, provider, &stop)
                        }
                        Operation::Attach => backend.attach_existing(&request.session_id, &stop),
                        Operation::Inspect => backend.inspect_session(&request.session_id, &stop),
                        Operation::Stop => backend.stop_session(&request.session_id, &stop),
                        Operation::CaptureResume => {
                            backend.capture_resume(&request.session_id, &stop)
                        }
                        Operation::Restore => backend.restore(&request.session_id, &stop),
                        Operation::RestoreEnvironment(command, cwd) => {
                            backend.restore_environment(&request.session_id, &command, &cwd, &stop)
                        }
                        Operation::Recover(command, cwd) => {
                            backend.recover(&request.session_id, &command, &cwd, &stop)
                        }
                    }),
            };
            if replies
                .send(SessionReply {
                    request_id: request.request_id,
                    session_id: request.session_id,
                    result,
                })
                .is_err()
            {
                break;
            }
            SignalToUI::set_ui_signal();
        }
        // Deliberately no server cleanup: agents must outlive Studio's UI.
    }
}

#[cfg(test)]
mod terminal_inventory_tests {
    use super::*;
    fn session(id: &str, state: &str) -> SessionInfo {
        SessionInfo {
            session_id: id.into(),
            state_dir: PathBuf::from(state),
            title: "hello".into(),
            activity: "idle".into(),
            started_at_ms: 1_783_300_000_000,
            supervisor_pid: 42,
            attach_command: String::new(),
            backend: "test",
            transport_program: String::new(),
            transport_version: "1".into(),
            transport_warning: None,
            scrollback_lines: 100,
            cwd: PathBuf::from("/repo"),
            clients: 1,
            provider: AgentProvider::Shell,
            resume: None,
            resume_error: None,
            recovery: None,
        }
    }
    #[test]
    fn menu_uses_names_and_scoped_ids_allow_duplicate_names() {
        let sessions = [
            session("term-00000001", "/studio"),
            session("term-00000002", "/shell"),
        ];
        let menu = terminal_menu(&sessions);
        assert_eq!(menu.len(), 2);
        for (index, (key, label)) in menu.iter().enumerate() {
            assert!(label.starts_with("hello · idle · since "));
            assert!(label.contains(&format!("0000000{}", index + 1)));
            let (state, id) = view_target(key, std::path::Path::new("/default")).unwrap();
            assert_eq!(state, sessions[index].state_dir);
            assert_eq!(id, sessions[index].session_id);
        }
        assert_ne!(menu[0].0, menu[1].0);
        let same_id = [session("same-id", "/studio"), session("same-id", "/shell")];
        assert_ne!(same_id[0].key(), same_id[1].key());
        assert!(view_target("/scope\n../bad", std::path::Path::new("/default")).is_err());
    }
}

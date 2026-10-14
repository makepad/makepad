//! The run manager: builds and runs the workspace's makepad apps for the RUN
//! panel and the agent's tools alike, so both see the same builds and the
//! same running apps. One per process (`RunManager::global()`); no UI types.
//!
//! - Apps come from the shared registry (`makepad-app-registry`, the table
//!   the WM and Builder use) and must have their sources in the workspace.
//! - Builds run one at a time, in a queue: `cargo build --release` of the
//!   app's package (the registry's `build_argv`), at normal priority with
//!   cargo's own job count. When
//!   `CODE_BUILD_LOCK=<path>` is set the build also waits for that lock file
//!   (`lockf -k` on macOS/BSD, `flock` on Linux), so builds from here never
//!   overlap other heavy builds that take the same lock.
//! - Native runs build first, then start the release binary with `--remote`
//!   (the agent's `ui`/`app_call` tools reach it by pid). `makepad run` runs
//!   start `cargo run --release -p makepad-run --bin makepad -- run <package>`
//!   in the checkout: cargo rebuilds the `makepad` tool when its sources (the
//!   provided crates it carries) changed, so it never refuses a checkout newer
//!   than a prebuilt binary; cargo's progress shows on the run, then cargo
//!   becomes the tool (same pid). A Builder installation starts its published
//!   `makepad`. A save reaches the run through the tool's own file watcher,
//!   which patches the running code, relaunches the app domain when it must,
//!   or refuses an edit with errors and keeps the app running (the errors
//!   are read from its output and marked in the editor); Code adds nothing to
//!   that (a relaunch of its own threw the patched state away). A save the
//!   running tool cannot take (a provided crate: "rebuild makepad") restarts
//!   it through cargo.
//! - The agent's shell commands (run_command) run here too, as `Shell`s:
//!   their output is kept line by line (run_output reads it from a line
//!   number on), a command that outlives the tool call keeps running in the
//!   background, and the RUN panel lists and stops them with the apps.
//! - Children inherit the environment (MAKEPAD_HIDE_WINDOWS and the like)
//!   and each starts its own process group; Stop and Cancel signal only the
//!   groups this manager started.

use makepad_app_registry::{self as registry, AppEntry, AppTarget, ProcessOutput};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// An app by registry id, package, binary or label, in any case and spacing ("Stage", "stage",
/// "makepad-stage", "Task Manager").
pub fn find_entry(apps: &[&'static AppEntry], name: &str) -> Option<&'static AppEntry> {
    let name = name.trim();
    let norm = |s: &str| s.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>();
    let want = norm(name);
    apps.iter()
        .find(|a| a.id == name || a.package == name || a.bin == name)
        .or_else(|| apps.iter().find(|a| [a.id, a.label, a.package, a.bin].iter().any(|n| norm(n) == want)))
        .or_else(|| apps.iter().find(|a| [a.package, a.bin].iter().any(|n| norm(n) == format!("makepadapp{want}") || norm(n) == format!("makepad{want}"))))
        .copied()
}

/// The agent's tools over the manager (declared and dispatched in remote.rs).
pub const NAMES: [&str; 7] = ["list_apps", "start_app", "build_app", "run_app", "stop_app", "app_status", "app_logs"];

/// The `makepad` tool that runs apps from source (tools/run).
pub const MAKEPAD_RUN: AppTarget<'static> = registry::MAKEPAD_RUN;

/// Lines kept per build and per running app.
const LOG_LINES: usize = 4000;
/// Finished builds kept for the panel and `app_status`.
const KEEP_JOBS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The release binary, built by cargo.
    Native,
    /// `makepad run <package>`: from source, warm relaunch on save.
    MakepadRun,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Native => "native",
            Mode::MakepadRun => "makepad run",
        }
    }

    pub fn parse(text: &str) -> Option<Mode> {
        match text.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
            "" | "native" | "release" => Some(Mode::Native),
            "makepad_run" | "makepad" | "jit" | "run" => Some(Mode::MakepadRun),
            _ => None,
        }
    }
}

/// What a build is for: just the build, or a run once it succeeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Goal {
    Build,
    Run(Mode),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Building,
    Succeeded,
    Failed(String),
    Cancelled,
}

impl JobState {
    pub fn is_done(&self) -> bool {
        matches!(self, JobState::Succeeded | JobState::Failed(_) | JobState::Cancelled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
}

/// One compiler message: where it points (relative to the workspace root
/// when inside it), 1-based line and column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub level: Level,
    pub message: String,
    pub path: Option<String>,
    pub line: u32,
    pub col: u32,
}

impl Diagnostic {
    pub fn location(&self) -> String {
        match &self.path {
            Some(path) => format!("{path}:{}:{}", self.line, self.col),
            None => String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// Crates finished and in the build (cargo's `N/M`); 0/0 before cargo
    /// printed its first count.
    pub done: u32,
    pub total: u32,
    /// The crate cargo is on.
    pub current: String,
}

/// Cargo's text output read line by line: progress and diagnostics.
#[derive(Default)]
pub struct BuildParser {
    root: PathBuf,
    pub progress: Progress,
    pub diags: Vec<Diagnostic>,
    /// Waiting for a lock (the build lock, or cargo's own) before compiling.
    pub waiting: bool,
    /// The last diagnostic still waits for its `-->` line.
    pending: bool,
}

impl BuildParser {
    pub fn new(root: &Path) -> Self {
        BuildParser { root: root.to_path_buf(), ..Default::default() }
    }

    pub fn errors(&self) -> usize {
        self.diags.iter().filter(|d| d.level == Level::Error).count()
    }

    pub fn warnings(&self) -> usize {
        self.diags.iter().filter(|d| d.level == Level::Warning).count()
    }

    pub fn feed(&mut self, line: &str) {
        let t = line.trim();
        if t.is_empty() {
            return;
        }
        if t.starts_with("Blocking waiting for file lock") {
            self.waiting = true;
            return;
        }
        self.waiting = false;
        if let Some(rest) = t.strip_prefix("Building [") {
            // "=====>   ] 73/88: windows, makepad-script"
            let Some((_, rest)) = rest.split_once(']') else { return };
            let (count, crates) = rest.trim().split_once(':').unwrap_or((rest.trim(), ""));
            if let Some((done, total)) = count.trim().split_once('/') {
                if let (Ok(done), Ok(total)) = (done.trim().parse(), total.trim().parse()) {
                    self.progress.done = done;
                    self.progress.total = total;
                }
            }
            if let Some(first) = crates.split(',').map(str::trim).find(|c| !c.is_empty()) {
                self.progress.current = first.to_string();
            }
            return;
        }
        for verb in ["Compiling ", "Checking ", "Documenting "] {
            if let Some(rest) = t.strip_prefix(verb) {
                self.progress.current = rest.split(' ').next().unwrap_or(rest).to_string();
                return;
            }
        }
        if t.starts_with("Finished ") {
            self.progress.done = self.progress.total;
            self.progress.current.clear();
            return;
        }
        if let Some(loc) = t.strip_prefix("--> ") {
            if self.pending {
                self.pending = false;
                if let Some((path, line, col)) = split_location(loc.trim()) {
                    let path = self.relative(&path);
                    let last = self.diags.len() - 1;
                    let d = &mut self.diags[last];
                    d.path = Some(path);
                    d.line = line;
                    d.col = col;
                    // The same message at the same place (a lib and its
                    // bin, a test build) is shown once.
                    let dup = self.diags[..last].iter().any(|o| *o == self.diags[last]);
                    if dup {
                        self.diags.pop();
                    }
                }
            }
            return;
        }
        if let Some((level, message)) = header(t) {
            self.pending = true;
            self.diags.push(Diagnostic { level, message, path: None, line: 0, col: 0 });
            return;
        }
        if t.starts_with("note:") || t.starts_with("help:") || t.starts_with("= ") {
            self.pending = false;
        }
    }

    fn relative(&self, path: &str) -> String {
        let p = Path::new(path);
        let rel = if p.is_absolute() { p.strip_prefix(&self.root).unwrap_or(p) } else { p };
        rel.to_string_lossy().replace('\\', "/")
    }
}

/// `error[E0425]: msg` / `warning: msg` as a level and its message; None for
/// cargo's own summary lines (`could not compile`, `generated 3 warnings`).
fn header(t: &str) -> Option<(Level, String)> {
    let (level, rest) = if let Some(rest) = t.strip_prefix("error") {
        (Level::Error, rest)
    } else if let Some(rest) = t.strip_prefix("warning") {
        (Level::Warning, rest)
    } else {
        return None;
    };
    let message = if let Some(code) = rest.strip_prefix('[') {
        let (code, msg) = code.split_once("]:")?;
        format!("{} [{code}]", msg.trim())
    } else {
        rest.strip_prefix(':')?.trim().to_string()
    };
    let summary = message.starts_with("could not compile")
        || message.starts_with("aborting due to")
        || message.starts_with("build failed")
        || (message.starts_with('`') && message.contains(" generated ") && message.contains("warning"))
        || (message.ends_with("warnings emitted") || message.ends_with("warning emitted"));
    (!summary && !message.is_empty()).then_some((level, message))
}

/// `src/main.rs:10:9` as (path, line, col); the path may hold colons.
/// A `makepad run`'s output read line by line for the errors of its code:
/// what the editor marks inline. Three shapes carry a place:
/// - an edit refused with errors: `makepad live: <files>: NOT applied (errors
///   below); ...` then `makepad live:   <path>:<line>:<col>: <message>` lines;
/// - a compile error at start or after a relaunch: `... compile error in
///   <fn>: <path>:<line>:<col>: <code>: <message>`;
/// - a refused live edit's borrow check: `borrow check failed: <fn> at
///   <path>:<line>:<col>: <message> (live edit refused)`.
/// The next edit outcome (`patched`, `NOT applied`, a relaunch or restart)
/// replaces them: they are the errors of the code as last saved.
#[derive(Default)]
pub struct LiveParser {
    root: PathBuf,
    /// Inside a `NOT applied (errors below)` block.
    errors_below: bool,
    /// A compiler-style `error[E0502]: message` in the block, waiting for
    /// its `--> path:line:col` line.
    pending: Option<(Level, String)>,
    pub diags: Vec<Diagnostic>,
}

impl LiveParser {
    pub fn new(root: &Path) -> Self {
        LiveParser { root: root.to_path_buf(), ..Default::default() }
    }

    /// Feed one output line; true when the errors changed.
    pub fn feed(&mut self, line: &str) -> bool {
        let t = line.trim_end();
        // the place of a compiler-style error in a refused edit's block (the borrow checker's)
        if let Some(loc) = t.trim_start().strip_prefix("--> ") {
            let Some((level, message)) = self.pending.take() else { return false };
            return match self.located(level, &format!("{}: {message}", loc.trim()), "") {
                Some(d) => self.push(d),
                None => false,
            };
        }
        // the app domain failed to load (a relaunch): `makepad-run domain: <app>: <place>: <message>`
        if let Some(rest) = t.strip_prefix("makepad-run domain: ") {
            let Some((_, place)) = rest.split_once(": ") else { return false };
            return match self.located(Level::Error, place, "") {
                Some(d) => self.push(d),
                None => false,
            };
        }
        if let Some(rest) = t.strip_prefix("makepad live:") {
            if let Some(item) = rest.strip_prefix("   ") {
                if !self.errors_below {
                    return false;
                }
                if let Some((level, message)) = header(item.trim()) {
                    self.pending = Some((level, message));
                    return false;
                }
                return match self.located(Level::Error, item.trim(), "") {
                    Some(d) => self.push(d),
                    None => false,
                };
            }
            // an edit outcome: what it says replaces the errors so far
            let outcome = [": patched", "NOT applied", "relaunching the app domain", "needs a restart", "restarting with state"];
            if outcome.iter().any(|o| rest.contains(o)) {
                self.errors_below = rest.contains("NOT applied");
                self.pending = None;
                let had = !self.diags.is_empty();
                self.diags.clear();
                return had;
            }
            return false;
        }
        if let Some((_, rest)) = t.split_once("compile error in ") {
            // `<fn>: <path>:<line>:<col>: <code>: <message>`: the fn path holds `::`, the place
            // follows the first ": "
            let Some((_, place)) = rest.split_once(": ") else { return false };
            return match self.located(Level::Error, place, "") {
                Some(d) => self.push(d),
                None => false,
            };
        }
        if let Some(rest) = t.strip_prefix("borrow check failed: ") {
            let Some((_, place)) = rest.split_once(" at ") else { return false };
            let place = place.strip_suffix(" (live edit refused)").unwrap_or(place);
            return match self.located(Level::Error, place, "borrow check: ") {
                Some(d) => self.push(d),
                None => false,
            };
        }
        false
    }

    /// `<path>:<line>:<col>: <message>` (the path may hold `:` on Windows).
    fn located(&self, level: Level, text: &str, prefix: &str) -> Option<Diagnostic> {
        let mut parts = text.splitn(2, ": ");
        let mut loc = parts.next()?;
        let mut message = parts.next().unwrap_or("").to_string();
        // a drive letter: `C:\x.rs:3:4: msg` splits at the first ": " only after the column
        if split_location(loc).is_none() {
            let (l, m) = text.rsplit_once(": ")?;
            loc = l;
            message = m.to_string();
        }
        let (path, line, col) = split_location(loc)?;
        if line == 0 {
            return None;
        }
        let p = Path::new(&path);
        let rel = if p.is_absolute() { p.strip_prefix(&self.root).ok()?.to_path_buf() } else { p.to_path_buf() };
        Some(Diagnostic { level, message: format!("{prefix}{}", message.trim()), path: Some(rel.to_string_lossy().replace('\\', "/")), line, col })
    }

    fn push(&mut self, d: Diagnostic) -> bool {
        if self.diags.contains(&d) {
            return false;
        }
        self.diags.push(d);
        true
    }
}

fn split_location(loc: &str) -> Option<(String, u32, u32)> {
    let mut parts = loc.rsplitn(3, ':');
    let col = parts.next()?.trim().parse().ok()?;
    let line = parts.next()?.trim().parse().ok()?;
    let path = parts.next()?.trim().to_string();
    (!path.is_empty()).then_some((path, line, col))
}

/// The argv that runs `program args`, under the lock file when one is
/// given (unix only). On Windows, or with no lock, the command is unchanged.
pub fn build_argv_with_lock(program: &str, args: &[String], lock: Option<&str>) -> Vec<String> {
    let mut argv: Vec<String> = Vec::new();
    if cfg!(unix) {
        if let Some(lock) = lock {
            if cfg!(target_os = "linux") {
                argv.extend(["flock".to_string(), lock.to_string()]);
            } else {
                argv.extend(["lockf".to_string(), "-k".to_string(), lock.to_string()]);
            }
        }
    }
    argv.push(program.to_string());
    argv.extend(args.iter().cloned());
    argv
}

/// `CODE_BUILD_LOCK`, when set to a path.
pub fn build_lock() -> Option<String> {
    std::env::var("CODE_BUILD_LOCK").ok().filter(|v| !v.trim().is_empty())
}

#[derive(Default)]
struct Ring(VecDeque<String>);

impl Ring {
    fn push(&mut self, line: String) {
        if self.0.len() == LOG_LINES {
            self.0.pop_front();
        }
        self.0.push_back(line);
    }

    fn tail(&self, n: usize) -> Vec<String> {
        self.0.iter().skip(self.0.len().saturating_sub(n)).cloned().collect()
    }
}

struct Job {
    id: u64,
    app: &'static AppEntry,
    /// What cargo builds: the app, or the `makepad` tool for a `makepad run`.
    target: AppTarget<'static>,
    goal: Goal,
    state: JobState,
    parser: BuildParser,
    log: Ring,
    started: Instant,
    ended: Option<Instant>,
    pid: Option<u32>,
    /// Still waiting for `CODE_BUILD_LOCK` (no output yet).
    lock_wait: bool,
}

struct Running {
    id: u64,
    app: &'static AppEntry,
    mode: Mode,
    pid: u32,
    started: Instant,
    exited: Option<String>,
    stopping: bool,
    /// Build and launch again once it has exited (a native Restart).
    restart: bool,
    /// Cargo's stage while `cargo run` compiles the `makepad` tool, until it
    /// runs it: "compiling 12/40 crates…", "waiting for another build…".
    building: Option<String>,
    /// A save relaunched it: a "rebuild makepad" refusal after that restarts it
    /// through cargo (never for a refusal at its own start: no restart loop).
    saved: bool,
    /// The errors of its code as last saved, from its output (marked inline).
    live: LiveParser,
    log: Ring,
}

/// How a shell command ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellExit {
    Code(i32),
    /// Ended by a signal it was not sent by us.
    Signal,
    /// Stopped here (run_stop, the panel, a session end, its time limit).
    Stopped(String),
}

impl ShellExit {
    pub fn text(&self) -> String {
        match self {
            ShellExit::Code(c) => format!("exit {c}"),
            ShellExit::Signal => "killed by a signal".into(),
            ShellExit::Stopped(why) => format!("stopped ({why})"),
        }
    }

    pub fn success(&self) -> bool {
        *self == ShellExit::Code(0)
    }
}

/// A shell command the agent started (run_command).
struct Shell {
    id: u64,
    command: String,
    /// Who started it (an agent engine's token): stops by owner touch only these.
    owner: u64,
    pid: u32,
    started: Instant,
    ended: Option<Instant>,
    exit: Option<ShellExit>,
    /// Why it is being stopped, once a stop was sent.
    stopping: Option<String>,
    /// Output lines, oldest first; `first` is the number of the oldest kept.
    lines: VecDeque<String>,
    first: u64,
    /// Output streams still open.
    open: u8,
}

impl Shell {
    fn total(&self) -> u64 {
        self.first + self.lines.len() as u64
    }
}

/// A shell command as the panel and the agent see it.
#[derive(Clone, Debug)]
pub struct ShellView {
    pub id: u64,
    pub command: String,
    pub owner: u64,
    pub pid: u32,
    /// Running time so far, or until it ended.
    pub elapsed: Duration,
    pub exit: Option<ShellExit>,
    /// Lines it printed so far.
    pub lines: u64,
}

/// A build as the panel and the agent see it.
#[derive(Clone, Debug)]
pub struct JobView {
    pub id: u64,
    pub app: &'static AppEntry,
    pub target: &'static str,
    pub goal: Goal,
    pub state: JobState,
    pub progress: Progress,
    pub waiting: bool,
    pub diags: Vec<Diagnostic>,
    pub elapsed: Duration,
}

impl JobView {
    pub fn errors(&self) -> usize {
        self.diags.iter().filter(|d| d.level == Level::Error).count()
    }

    pub fn warnings(&self) -> usize {
        self.diags.iter().filter(|d| d.level == Level::Warning).count()
    }

    /// One line: `compiling 143/512  makepad-widgets`, `build ok 12.3 s`, ...
    pub fn status_line(&self) -> String {
        let counts = format!("{} errors  {} warnings", self.errors(), self.warnings());
        let secs = self.elapsed.as_secs_f32();
        match &self.state {
            JobState::Queued => "queued".into(),
            JobState::Building if self.waiting => "waiting for the build lock".into(),
            JobState::Building if self.progress.total > 0 => {
                format!("compiling {}/{}  {}", self.progress.done, self.progress.total, self.progress.current)
            }
            JobState::Building if !self.progress.current.is_empty() => format!("compiling  {}", self.progress.current),
            JobState::Building => "starting cargo".into(),
            JobState::Succeeded => format!("build ok  {secs:.1} s  {counts}"),
            JobState::Failed(why) => format!("failed  {secs:.1} s  {counts}  {why}"),
            JobState::Cancelled => "cancelled".into(),
        }
    }
}

/// A running (or exited) app as the panel and the agent see it.
#[derive(Clone, Debug)]
pub struct RunningView {
    pub id: u64,
    pub app: &'static AppEntry,
    pub mode: Mode,
    pub pid: u32,
    pub uptime: Duration,
    pub exited: Option<String>,
    /// Cargo's stage while `cargo run` still compiles the `makepad` tool.
    pub building: Option<String>,
    /// The errors of its code as last saved (paths relative to the root).
    pub diags: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Oldest first.
    pub jobs: Vec<JobView>,
    pub running: Vec<RunningView>,
    /// The agent's shell commands, oldest first.
    pub shells: Vec<ShellView>,
}

/// What `run` started: a build that launches when it succeeds, or the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Started {
    Job(u64),
    Running(u64),
}

#[derive(Default)]
struct State {
    root: Option<PathBuf>,
    jobs: Vec<Job>,
    running: Vec<Running>,
    shells: Vec<Shell>,
    next_id: u64,
    /// Per app id: the folders (relative to the root) of its package and of
    /// every in-repo path dependency, transitively; read by `cargo metadata`
    /// on a worker the first time the app runs, again after a Cargo.toml save.
    deps: HashMap<&'static str, Vec<String>>,
    /// Apps whose dependency folders are being read.
    deps_pending: HashSet<&'static str>,
}

impl State {
    fn fresh_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn job(&mut self, id: u64) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    fn run(&mut self, id: u64) -> Option<&mut Running> {
        self.running.iter_mut().find(|r| r.id == id)
    }

    fn shell(&mut self, id: u64) -> Option<&mut Shell> {
        self.shells.iter_mut().find(|s| s.id == id)
    }
}

fn shell_view(s: &Shell, now: Instant) -> ShellView {
    ShellView {
        id: s.id,
        command: s.command.clone(),
        owner: s.owner,
        pid: s.pid,
        elapsed: s.ended.unwrap_or(now).duration_since(s.started),
        exit: s.exit.clone(),
        lines: s.total(),
    }
}

pub type Wake = Arc<dyn Fn() + Send + Sync>;

pub struct RunManager {
    state: Mutex<State>,
    generation: AtomicU64,
    wakes: Mutex<Vec<Wake>>,
}

impl RunManager {
    /// The one manager of this process.
    pub fn global() -> &'static RunManager {
        static M: OnceLock<RunManager> = OnceLock::new();
        M.get_or_init(|| RunManager { state: Mutex::new(State::default()), generation: AtomicU64::new(0), wakes: Mutex::new(Vec::new()) })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Something changed: bump the generation and wake the front ends.
    fn changed(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let wakes = self.wakes.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for wake in wakes {
            wake();
        }
    }

    /// Changes so far: a front end redraws when this moved.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Called (from any thread) after every change.
    pub fn on_change(&self, wake: Wake) {
        self.wakes.lock().unwrap_or_else(|e| e.into_inner()).push(wake);
    }

    /// The makepad checkout at or above the project folder; false when the
    /// project is not in one (there is nothing to build then).
    /// The checkout the apps come from: the project's, else this machine's (`registry::repo_root`:
    /// `MAKEPAD_WM_ROOT`, the Builder's sources, the checkout this program was built in), so a
    /// project folder outside any checkout still builds and starts the machine's apps.
    pub fn set_root(&self, project: &Path) -> bool {
        let root = registry::checkout_at_or_above(project)
            .or_else(|| registry::repo_root().filter(|r| r.join("Cargo.toml").is_file()));
        let found = root.is_some();
        let mut st = self.lock();
        if st.root.is_none() {
            st.root = root;
        }
        found
    }

    pub fn root(&self) -> Option<PathBuf> {
        self.lock().root.clone()
    }

    /// The registry's apps whose sources are in the checkout, menu order.
    pub fn apps(&self) -> Vec<&'static AppEntry> {
        let Some(root) = self.root() else { return Vec::new() };
        registry::apps().iter().filter(|a| registry::has_sources(&a.target(), &root)).collect()
    }

    /// An app by registry id, package, binary or label (any case).
    pub fn find_app(&self, name: &str) -> Option<&'static AppEntry> {
        find_entry(&self.apps(), name)
    }

    pub fn is_built(&self, app: &AppEntry) -> bool {
        registry::built_binary(&app.target(), self.root().as_deref()).is_some()
    }

    /// The window is closing: cancel builds and stop every app this manager
    /// started (only its own process groups).
    pub fn shutdown(&'static self) {
        let snap = self.snapshot();
        for job in snap.jobs.iter().filter(|j| !j.state.is_done()) {
            self.cancel(job.id);
        }
        for run in snap.running.iter().filter(|r| r.exited.is_none()) {
            let _ = self.stop(run.id);
        }
        for shell in snap.shells.iter().filter(|s| s.exit.is_none()) {
            let _ = self.stop_shell(shell.id, "the window closed");
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let st = self.lock();
        let now = Instant::now();
        Snapshot {
            jobs: st
                .jobs
                .iter()
                .map(|j| JobView {
                    id: j.id,
                    app: j.app,
                    target: j.target.package,
                    goal: j.goal,
                    state: j.state.clone(),
                    progress: j.parser.progress.clone(),
                    waiting: j.parser.waiting,
                    diags: j.parser.diags.clone(),
                    elapsed: j.ended.unwrap_or(now).duration_since(j.started),
                })
                .collect(),
            running: st
                .running
                .iter()
                .map(|r| RunningView {
                    id: r.id,
                    app: r.app,
                    mode: r.mode,
                    pid: r.pid,
                    uptime: now.duration_since(r.started),
                    exited: r.exited.clone(),
                    building: r.building.clone(),
                    diags: r.live.diags.clone(),
                })
                .collect(),
            shells: st.shells.iter().map(|s| shell_view(s, now)).collect(),
        }
    }

    pub fn job(&self, id: u64) -> Option<JobView> {
        self.snapshot().jobs.into_iter().find(|j| j.id == id)
    }

    /// The last `n` lines of a build's output.
    pub fn job_log(&self, id: u64, n: usize) -> Vec<String> {
        self.lock().jobs.iter().find(|j| j.id == id).map(|j| j.log.tail(n)).unwrap_or_default()
    }

    /// The last `n` lines a running app printed.
    pub fn run_log(&self, id: u64, n: usize) -> Vec<String> {
        self.lock().running.iter().find(|r| r.id == id).map(|r| r.log.tail(n)).unwrap_or_default()
    }

    /// The errors and warnings in the file `rel` (relative to the root): of
    /// each app's newest build and of its runs' code as last saved. What the
    /// editor marks inline.
    pub fn diagnostics_for(&self, rel: &str) -> Vec<Diagnostic> {
        let st = self.lock();
        let mut newest: HashMap<&str, &Job> = HashMap::new();
        for j in &st.jobs {
            newest.insert(j.app.id, j);
        }
        let builds = newest.values().flat_map(|j| j.parser.diags.iter());
        let runs = st.running.iter().flat_map(|r| r.live.diags.iter());
        let mut out: Vec<Diagnostic> = Vec::new();
        for d in builds.chain(runs) {
            if d.path.as_deref() == Some(rel) && !out.contains(d) {
                out.push(d.clone());
            }
        }
        out
    }

    // ---- builds -----------------------------------------------------------

    /// Queue a build of `app` (one build runs at a time).
    pub fn build(&'static self, app: &'static AppEntry) -> Result<u64, String> {
        self.queue(app, app.target(), Goal::Build)
    }

    /// Build and start `app`: natively (release build, then the binary with
    /// `--remote`) or under `makepad run` (from a checkout `cargo run` of the
    /// `makepad` tool; a Builder installation's tool is built first when it
    /// is missing).
    pub fn run(&'static self, app: &'static AppEntry, mode: Mode) -> Result<Started, String> {
        match mode {
            Mode::Native => self.queue(app, app.target(), Goal::Run(mode)).map(Started::Job),
            Mode::MakepadRun => {
                let root = self.root().ok_or("not a makepad checkout")?;
                if registry::builder_install().is_none() || registry::built_binary(&MAKEPAD_RUN, Some(&root)).is_some() {
                    self.launch(app, mode).map(Started::Running)
                } else {
                    self.queue(app, MAKEPAD_RUN, Goal::Run(mode)).map(Started::Job)
                }
            }
        }
    }

    fn queue(&'static self, app: &'static AppEntry, target: AppTarget<'static>, goal: Goal) -> Result<u64, String> {
        let id = {
            let mut st = self.lock();
            let root = st.root.clone().ok_or("not a makepad checkout")?;
            if !registry::has_sources(&target, &root) {
                return Err(format!("sources not found: {}", target.dir));
            }
            if let Some(same) = st.jobs.iter().find(|j| j.app.id == app.id && j.goal == goal && !j.state.is_done()) {
                return Ok(same.id);
            }
            let id = st.fresh_id();
            st.jobs.push(Job {
                id,
                app,
                target,
                goal,
                state: JobState::Queued,
                parser: BuildParser::new(&root),
                log: Ring::default(),
                started: Instant::now(),
                ended: None,
                pid: None,
                lock_wait: false,
            });
            let finished = st.jobs.iter().filter(|j| j.state.is_done()).count();
            if finished > KEEP_JOBS {
                let mut drop = finished - KEEP_JOBS;
                st.jobs.retain(|j| {
                    let gone = drop > 0 && j.state.is_done();
                    if gone {
                        drop -= 1;
                    }
                    !gone
                });
            }
            id
        };
        self.pump();
        self.changed();
        Ok(id)
    }

    /// Start the next queued build when none runs.
    fn pump(&'static self) {
        let next = {
            let mut st = self.lock();
            if st.jobs.iter().any(|j| j.state == JobState::Building) {
                return;
            }
            let Some(root) = st.root.clone() else { return };
            let Some(job) = st.jobs.iter_mut().find(|j| j.state == JobState::Queued) else { return };
            job.state = JobState::Building;
            job.started = Instant::now();
            job.parser.waiting = build_lock().is_some();
            job.lock_wait = job.parser.waiting;
            (job.id, job.target, root)
        };
        let (id, target, root) = next;
        if let Err(e) = self.spawn_build(id, &target, &root) {
            self.finish_job(id, Err(e));
        }
    }

    fn spawn_build(&'static self, id: u64, target: &AppTarget<'static>, root: &Path) -> Result<(), String> {
        let (program, args) = registry::build_argv(target, root);
        let argv = build_argv_with_lock(&program.to_string_lossy(), &args, build_lock().as_deref());
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]).current_dir(root);
        // the same cargo options as every other launcher: no rebuilds between them
        registry::cargo_env(&mut command);
        let child = spawn_group(command).map_err(|e| format!("cannot start {}: {e}", argv[0]))?;
        if let Some(job) = self.lock().job(id) {
            job.pid = Some(child.id());
            job.log.push(format!("$ {}", argv.join(" ")));
        }
        self.watch(child, move |m, out| m.job_output(id, out), move |m, status| m.finish_job(id, Ok(status)));
        Ok(())
    }

    fn job_output(&self, id: u64, out: ProcessOutput) {
        if let ProcessOutput::Line(line) = out {
            if let Some(job) = self.lock().job(id) {
                // The time counts from when the build lock was ours (the
                // first output; cargo's own lock waits count as building).
                if job.lock_wait {
                    job.lock_wait = false;
                    job.started = Instant::now();
                }
                job.parser.feed(&line);
                if !line.starts_with("Building [") {
                    job.log.push(line);
                }
            }
            self.changed();
        }
    }

    fn finish_job(&'static self, id: u64, result: Result<ExitStatus, String>) {
        let launch = {
            let mut st = self.lock();
            let Some(job) = st.job(id) else { return };
            job.ended = Some(Instant::now());
            job.pid = None;
            job.parser.waiting = false;
            if job.state == JobState::Cancelled {
                None
            } else {
                match result {
                    Ok(status) if status.success() => {
                        job.state = JobState::Succeeded;
                        job.parser.progress.done = job.parser.progress.total;
                        match job.goal {
                            Goal::Run(mode) => Some((job.app, mode)),
                            Goal::Build => None,
                        }
                    }
                    Ok(status) => {
                        let why = if job.parser.errors() > 0 { String::new() } else { exit_text(&status) };
                        job.state = JobState::Failed(why);
                        None
                    }
                    Err(e) => {
                        job.log.push(e.clone());
                        job.state = JobState::Failed(e);
                        None
                    }
                }
            }
        };
        if let Some((app, mode)) = launch {
            if let Err(e) = self.launch(app, mode) {
                if let Some(job) = self.lock().job(id) {
                    job.log.push(e.clone());
                    job.state = JobState::Failed(e);
                }
            }
        }
        self.changed();
        self.pump();
    }

    /// Stop a queued or running build.
    pub fn cancel(&'static self, id: u64) {
        let pid = {
            let mut st = self.lock();
            let Some(job) = st.job(id) else { return };
            if job.state.is_done() {
                return;
            }
            let was_queued = job.state == JobState::Queued;
            job.state = JobState::Cancelled;
            if was_queued {
                job.ended = Some(Instant::now());
            }
            job.pid
        };
        if let Some(pid) = pid {
            signal_group(pid, false);
        }
        self.changed();
    }

    // ---- running apps -------------------------------------------------------

    fn launch(&'static self, app: &'static AppEntry, mode: Mode) -> Result<u64, String> {
        let root = self.root().ok_or("not a makepad checkout")?;
        let (program, args) = match mode {
            Mode::Native => {
                let program = registry::built_binary(&app.target(), Some(&root)).ok_or_else(|| format!("not built yet: {}", app.bin))?;
                let mut args: Vec<String> = app.args.iter().map(|a| a.to_string()).collect();
                args.push("--remote".into());
                (program, args)
            }
            Mode::MakepadRun => makepad_run_argv(app, &root, registry::builder_install().is_some())?,
        };
        let cargo = mode == Mode::MakepadRun && registry::builder_install().is_none();
        let mut command = Command::new(&program);
        command.args(&args).current_dir(&root);
        if cargo {
            registry::cargo_env(&mut command);
        }
        let child = spawn_group(command).map_err(|e| format!("cannot start {}: {e}", program.display()))?;
        let id = {
            let mut st = self.lock();
            let id = st.fresh_id();
            let mut log = Ring::default();
            log.push(format!("$ {} {}", program.display(), args.join(" ")));
            st.running.push(Running {
                id,
                app,
                mode,
                pid: child.id(),
                started: Instant::now(),
                exited: None,
                stopping: false,
                restart: false,
                building: cargo.then(|| "starting cargo…".to_string()),
                saved: false,
                live: LiveParser::new(&root),
                log,
            });
            id
        };
        self.watch(child, move |m, out| m.run_output(id, out), move |m, status| m.run_exited(id, status));
        if mode == Mode::MakepadRun {
            self.load_deps(app);
        }
        self.changed();
        Ok(id)
    }

    fn run_output(&'static self, id: u64, out: ProcessOutput) {
        if let ProcessOutput::Line(line) = out {
            let mut rebuild = false;
            if let Some(r) = self.lock().run(id) {
                if r.building.is_some() {
                    r.building = cargo_stage(&line, r.building.take());
                    if line.starts_with("Building [") {
                        self.changed();
                        return;
                    }
                }
                // The tool refuses sources newer than itself (a provided crate
                // saved while it runs): cargo rebuilds it on a restart.
                rebuild = r.mode == Mode::MakepadRun && r.saved && !r.restart && !r.stopping && line.contains("rebuild makepad");
                r.live.feed(&line);
                r.log.push(line);
                if rebuild {
                    r.restart = true;
                    r.log.push("[the saved sources need a newer makepad: restarting through cargo, which rebuilds it]".into());
                }
            }
            if rebuild {
                let _ = self.stop(id);
            }
            self.changed();
        }
    }

    fn run_exited(&'static self, id: u64, status: ExitStatus) {
        let again = {
            let mut st = self.lock();
            let Some(r) = st.run(id) else { return };
            r.exited = Some(if r.stopping { "stopped".into() } else { exit_text(&status) });
            r.log.push(format!("[exited: {}]", r.exited.as_deref().unwrap_or("")));
            let again = r.restart.then_some((r.app, r.mode));
            if again.is_some() {
                st.running.retain(|r| r.id != id);
            }
            again
        };
        if let Some((app, mode)) = again {
            let _ = self.run(app, mode);
        }
        self.changed();
    }

    /// Stop a running app (its whole process group); an exited one leaves
    /// the list.
    pub fn stop(&'static self, id: u64) -> Result<(), String> {
        let pid = {
            let mut st = self.lock();
            let r = st.run(id).ok_or("no such run")?;
            if r.exited.is_some() {
                st.running.retain(|r| r.id != id);
                drop(st);
                self.changed();
                return Ok(());
            }
            r.stopping = true;
            r.pid
        };
        signal_group(pid, false);
        // A window that ignores the request is ended after 3 s.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(3));
            let alive = RunManager::global().lock().running.iter().any(|r| r.id == id && r.exited.is_none());
            if alive {
                signal_group(pid, true);
            }
        });
        self.changed();
        Ok(())
    }

    /// Restart: a `makepad run` app relaunches warm (window and GPU kept); a
    /// native one stops, builds and starts again. An exited app starts again.
    pub fn restart(&'static self, id: u64) -> Result<(), String> {
        let (app, mode, exited) = {
            let mut st = self.lock();
            let r = st.run(id).ok_or("no such run")?;
            (r.app, r.mode, r.exited.is_some())
        };
        if exited {
            self.lock().running.retain(|r| r.id != id);
            return self.run(app, mode).map(|_| ());
        }
        match mode {
            Mode::MakepadRun => self.relaunch(id),
            Mode::Native => {
                if let Some(r) = self.lock().run(id) {
                    r.restart = true;
                }
                self.stop(id)
            }
        }
    }

    /// Warm relaunch of a `makepad run` app (`session relaunch` on its
    /// endpoint), off this thread; the answer lands in its log.
    pub fn relaunch(&'static self, id: u64) -> Result<(), String> {
        let pid = {
            let mut st = self.lock();
            let r = st.run(id).ok_or("no such run")?;
            if r.mode != Mode::MakepadRun || r.exited.is_some() {
                return Err("only a running `makepad run` app relaunches warm".into());
            }
            r.log.push("[relaunch]".into());
            r.pid
        };
        std::thread::spawn(move || {
            let answer = session_relaunch(pid);
            let m = RunManager::global();
            // A run without a HAL session has no app domain to
            // relaunch: start the process again (makepad run's cache keeps it quick).
            let cold = matches!(&answer, Err(e) if e.contains("no hosted app domain"));
            if let Some(r) = m.lock().run(id) {
                r.log.push(match &answer {
                    Ok(text) => format!("[relaunch: {text}]"),
                    Err(_) if cold => "[relaunch: no app domain here, restarting the process]".into(),
                    Err(e) => format!("[relaunch failed: {e}]"),
                });
                if cold {
                    r.restart = true;
                }
            }
            if cold {
                let _ = m.stop(id);
            }
            m.changed();
        });
        self.changed();
        Ok(())
    }

    /// A file was saved (`rel` relative to the checkout root): the running
    /// `makepad run` apps whose package or path dependencies hold it. Their
    /// own watcher applies the save; here they only learn that a save came
    /// (a "rebuild makepad" refusal after it restarts them through cargo).
    /// Returns them.
    pub fn on_saved(&'static self, rel: &str) -> Vec<u64> {
        let manifest = rel == "Cargo.toml" || rel.ends_with("/Cargo.toml");
        let ids: Vec<u64> = {
            let st = self.lock();
            st.running
                .iter()
                .filter(|r| r.mode == Mode::MakepadRun && r.exited.is_none())
                .filter(|r| {
                    in_package(r.app.dir, rel) || st.deps.get(r.app.id).is_some_and(|dirs| dirs.iter().any(|d| in_package(d, rel)))
                })
                .map(|r| r.id)
                .collect()
        };
        if manifest {
            // the dependency graph may have changed: read it again
            let apps: Vec<&'static AppEntry> = {
                let mut st = self.lock();
                st.deps.clear();
                st.running.iter().filter(|r| r.exited.is_none()).map(|r| r.app).collect()
            };
            for app in apps {
                self.load_deps(app);
            }
        }
        let mut st = self.lock();
        for id in &ids {
            if let Some(r) = st.run(*id) {
                r.saved = true;
            }
        }
        ids
    }

    /// Read `app`'s dependency folders on a worker unless known or underway.
    fn load_deps(&'static self, app: &'static AppEntry) {
        let root = {
            let mut st = self.lock();
            let Some(root) = st.root.clone() else { return };
            if st.deps.contains_key(app.id) || !st.deps_pending.insert(app.id) {
                return;
            }
            root
        };
        std::thread::spawn(move || {
            let dirs = metadata_deps(&root, app.package);
            let m = RunManager::global();
            let mut st = m.lock();
            st.deps_pending.remove(app.id);
            match dirs {
                Ok(dirs) => {
                    st.deps.insert(app.id, dirs);
                }
                Err(e) => eprintln!("code: dependencies of {}: {e}", app.id),
            }
        });
    }

    /// The folders a save in which relaunches `app` (empty until read).
    pub fn dep_dirs(&self, app: &AppEntry) -> Vec<String> {
        self.lock().deps.get(app.id).cloned().unwrap_or_default()
    }

    /// Running apps and shell commands not yet exited plus builds not done.
    pub fn live_count(&self) -> usize {
        let st = self.lock();
        st.running.iter().filter(|r| r.exited.is_none()).count()
            + st.jobs.iter().filter(|j| !j.state.is_done()).count()
            + st.shells.iter().filter(|s| s.exit.is_none()).count()
    }

    /// What `live_count` counts, as words: `calculator (makepad run)`, `build of notes`.
    pub fn live_names(&self) -> Vec<String> {
        let st = self.lock();
        let runs = st.running.iter().filter(|r| r.exited.is_none()).map(|r| format!("{} ({})", r.app.id, r.mode.name()));
        let builds = st.jobs.iter().filter(|j| !j.state.is_done()).map(|j| format!("build of {}", j.app.id));
        let shells = st.shells.iter().filter(|s| s.exit.is_none()).map(|s| format!("run {}: {}", s.id, cut_command(&s.command, 40)));
        runs.chain(builds).chain(shells).collect()
    }

    /// The live run of `app`, newest first.
    pub fn running_of(&self, app: &AppEntry) -> Option<RunningView> {
        self.snapshot().running.into_iter().rev().find(|r| r.app.id == app.id && r.exited.is_none())
    }

    /// Wait until the build ends (or `timeout`); its last state.
    pub fn wait_job(&self, id: u64, timeout: Duration) -> Option<JobView> {
        let end = Instant::now() + timeout;
        loop {
            let job = self.job(id)?;
            if job.state.is_done() || Instant::now() >= end {
                return Some(job);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Pipe a child's output to `line`, call `exit` when it ended and both
    /// streams are drained.
    fn watch(
        &'static self,
        mut child: Child,
        line: impl Fn(&'static RunManager, ProcessOutput) + Send + Sync + 'static,
        exit: impl FnOnce(&'static RunManager, ExitStatus) + Send + 'static,
    ) {
        let line = Arc::new(line);
        let mut readers = Vec::new();
        if let Some(out) = child.stdout.take() {
            let line = line.clone();
            readers.push(std::thread::spawn(move || registry::read_output(out, None, |o| {
                line(RunManager::global(), o);
                true
            })));
        }
        if let Some(err) = child.stderr.take() {
            let line = line.clone();
            readers.push(std::thread::spawn(move || registry::read_output(err, None, |o| {
                line(RunManager::global(), o);
                true
            })));
        }
        std::thread::spawn(move || {
            let status = child.wait();
            for r in readers {
                let _ = r.join();
            }
            match status {
                Ok(status) => exit(RunManager::global(), status),
                Err(e) => eprintln!("code: waiting for a child failed: {e}"),
            }
        });
    }

    // ---- the agent's shell commands --------------------------------------------

    /// Start `sh -c command` in `cwd` as its own process group, owned by
    /// `owner`; it is stopped after `limit`. Returns the run id.
    pub fn start_shell(&'static self, command: &str, cwd: &Path, owner: u64, limit: Duration) -> Result<u64, String> {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command).current_dir(cwd);
        let mut child = spawn_group(c).map_err(|e| format!("cannot start: {e}"))?;
        let pid = child.id();
        let id = {
            let mut st = self.lock();
            let id = st.fresh_id();
            st.shells.push(Shell {
                id,
                command: command.to_string(),
                owner,
                pid,
                started: Instant::now(),
                ended: None,
                exit: None,
                stopping: None,
                lines: VecDeque::new(),
                first: 0,
                open: 2,
            });
            // Finished commands beyond the last few leave the list.
            let finished = st.shells.iter().filter(|s| s.exit.is_some()).count();
            if finished > KEEP_JOBS {
                let mut drop = finished - KEEP_JOBS;
                st.shells.retain(|s| {
                    let gone = drop > 0 && s.exit.is_some();
                    if gone {
                        drop -= 1;
                    }
                    !gone
                });
            }
            id
        };
        let streams: [Option<Box<dyn std::io::Read + Send>>; 2] = [
            child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
            child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        ];
        for stream in streams {
            match stream {
                Some(stream) => {
                    std::thread::spawn(move || {
                        read_lines(stream, |lines| RunManager::global().shell_output(id, lines));
                        RunManager::global().shell_closed(id);
                    });
                }
                None => self.shell_closed(id),
            }
        }
        std::thread::spawn(move || {
            let status = child.wait();
            let m = RunManager::global();
            // Take what the pipes still hold; a background process may keep
            // them open, so do not wait for their end past a moment.
            let end = Instant::now() + Duration::from_millis(300);
            while Instant::now() < end && m.lock().shell(id).is_some_and(|s| s.open > 0) {
                std::thread::sleep(Duration::from_millis(10));
            }
            {
                let mut st = m.lock();
                if let Some(s) = st.shell(id) {
                    s.ended = Some(Instant::now());
                    s.exit = Some(match (&s.stopping, status.as_ref().ok().and_then(|st| st.code())) {
                        (Some(why), _) => ShellExit::Stopped(why.clone()),
                        (None, Some(code)) => ShellExit::Code(code),
                        (None, None) => ShellExit::Signal,
                    });
                }
            }
            m.changed();
        });
        std::thread::spawn(move || {
            let end = Instant::now() + limit;
            loop {
                let m = RunManager::global();
                if m.lock().shell(id).is_none_or(|s| s.exit.is_some()) {
                    return;
                }
                if Instant::now() >= end {
                    let _ = m.stop_shell(id, &format!("time limit {} s", limit.as_secs()));
                    return;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        });
        self.changed();
        Ok(id)
    }

    fn shell_output(&self, id: u64, lines: Vec<String>) {
        if lines.is_empty() {
            return;
        }
        if let Some(s) = self.lock().shell(id) {
            for line in lines {
                if s.lines.len() == LOG_LINES {
                    s.lines.pop_front();
                    s.first += 1;
                }
                s.lines.push_back(line);
            }
        }
        self.changed();
    }

    fn shell_closed(&self, id: u64) {
        if let Some(s) = self.lock().shell(id) {
            s.open = s.open.saturating_sub(1);
        }
    }

    pub fn shell(&self, id: u64) -> Option<ShellView> {
        let now = Instant::now();
        self.lock().shells.iter().find(|s| s.id == id).map(|s| shell_view(s, now))
    }

    /// Output lines from line number `since` on (0 = the first line), and
    /// the number to ask from next time. Lines older than the kept window
    /// are skipped (the first returned is then later than `since`).
    pub fn shell_lines(&self, id: u64, since: u64) -> Option<(Vec<String>, u64)> {
        let mut st = self.lock();
        let s = st.shell(id)?;
        let from = since.max(s.first);
        let lines = s.lines.iter().skip((from - s.first) as usize).cloned().collect();
        Some((lines, s.total()))
    }

    /// Stop a shell command: its whole process group, then a hard kill
    /// after 3 s. `why` is how its end reads. Stopping an ended one is fine.
    pub fn stop_shell(&'static self, id: u64, why: &str) -> Result<(), String> {
        let pid = {
            let mut st = self.lock();
            let s = st.shell(id).ok_or_else(|| format!("no run {id}"))?;
            if s.exit.is_some() {
                return Ok(());
            }
            s.stopping.get_or_insert_with(|| why.to_string());
            s.pid
        };
        signal_group(pid, false);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(3));
            let alive = RunManager::global().lock().shell(id).is_some_and(|s| s.exit.is_none());
            if alive {
                signal_group(pid, true);
            }
        });
        self.changed();
        Ok(())
    }

    /// An ended shell command leaves the list (the panel's ×).
    pub fn dismiss_shell(&self, id: u64) {
        self.lock().shells.retain(|s| s.id != id || s.exit.is_none());
        self.changed();
    }

    /// Stop every running shell command `owner` started (its session ended).
    pub fn stop_shells_of(&'static self, owner: u64, why: &str) -> Vec<u64> {
        let ids: Vec<u64> = self.lock().shells.iter().filter(|s| s.owner == owner && s.exit.is_none()).map(|s| s.id).collect();
        for id in &ids {
            let _ = self.stop_shell(*id, why);
        }
        ids
    }

    /// Wait until the shell command has ended (or `timeout`).
    pub fn wait_shell(&self, id: u64, timeout: Duration) -> Option<ShellView> {
        let end = Instant::now() + timeout;
        loop {
            let s = self.shell(id)?;
            if s.exit.is_some() || Instant::now() >= end {
                return Some(s);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    // ---- text for the agent --------------------------------------------------

    pub fn apps_text(&self) -> String {
        let snap = self.snapshot();
        let apps = self.apps();
        if apps.is_empty() {
            return "no makepad apps here (the project is not a makepad checkout)".into();
        }
        let mut out = String::new();
        for a in apps {
            let mut state = if self.is_built(a) { "built" } else { "not built" }.to_string();
            for r in snap.running.iter().filter(|r| r.app.id == a.id && r.exited.is_none()) {
                state.push_str(&format!(", running pid {} ({})", r.pid, r.mode.name()));
            }
            out.push_str(&format!("{}  {}  package {}  {}\n", a.id, a.label, a.package, state));
        }
        out
    }

    pub fn job_text(&self, job: &JobView, max_diags: usize) -> String {
        let what = match job.goal {
            Goal::Build => format!("build of {}", job.app.id),
            Goal::Run(mode) => format!("build of {} for a {} run", job.target, mode.name()),
        };
        let mut out = format!("{what}: {}\n", job.status_line());
        for d in job.diags.iter().take(max_diags) {
            let level = if d.level == Level::Error { "error" } else { "warning" };
            out.push_str(&format!("{level}: {}  {}\n", d.message, d.location()));
        }
        if job.diags.len() > max_diags {
            out.push_str(&format!("[{} more]\n", job.diags.len() - max_diags));
        }
        out
    }

    pub fn running_text(r: &RunningView) -> String {
        match (&r.exited, &r.building) {
            (None, Some(stage)) => format!("{} pid {} ({}) cargo: {stage} (it starts when cargo is done; app_logs shows cargo's output)", r.app.id, r.pid, r.mode.name()),
            (None, None) => format!("{} pid {} ({}) up {}s: reach it with ui/app_tools target={}", r.app.id, r.pid, r.mode.name(), r.uptime.as_secs(), r.pid),
            (Some(how), _) => format!("{} pid {} ({}) exited: {how}", r.app.id, r.pid, r.mode.name()),
        }
    }

    /// Builds and runs, all or of one app.
    pub fn status_text(&self, app: Option<&AppEntry>) -> String {
        let snap = self.snapshot();
        let mut out = String::new();
        for j in snap.jobs.iter().filter(|j| app.is_none_or(|a| a.id == j.app.id)) {
            out.push_str(&self.job_text(j, 10));
        }
        for r in snap.running.iter().filter(|r| app.is_none_or(|a| a.id == r.app.id)) {
            out.push_str(&RunManager::running_text(r));
            out.push('\n');
        }
        if out.is_empty() {
            out = "no builds or runs yet".into();
        }
        out
    }
}

/// `cargo metadata` of the checkout, then [`path_deps`] of `package`.
fn metadata_deps(root: &Path, package: &str) -> Result<Vec<String>, String> {
    let manifest = root.join("Cargo.toml");
    let mut last = String::new();
    // The full graph knows path crates outside the workspace too; offline
    // it never fetches. Without a resolvable graph the members still do.
    for extra in [&["--offline"][..], &["--offline", "--no-deps"][..]] {
        let out = Command::new(registry::cargo_bin())
            .args(["metadata", "--format-version", "1", "--manifest-path"])
            .arg(&manifest)
            .args(extra)
            .current_dir(root)
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("cargo metadata: {e}"))?;
        if out.status.success() {
            return path_deps(&out.stdout, root, package);
        }
        last = String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("").to_string();
    }
    Err(format!("cargo metadata failed: {last}"))
}

/// The folders (relative to `root`, `/` separators) of `package` and of
/// every path dependency it reaches, transitively, inside `root`; dev
/// dependencies are not followed. `metadata` is `cargo metadata` JSON.
pub fn path_deps(metadata: &[u8], root: &Path, package: &str) -> Result<Vec<String>, String> {
    use makepad_strict_json::Value;
    let json = makepad_strict_json::parse(metadata).map_err(|e| format!("cargo metadata: {e:?}"))?;
    let packages = json.get("packages").and_then(Value::as_arr).ok_or("cargo metadata without packages")?;
    let dir_of = |p: &Value| -> Option<PathBuf> { Path::new(p.get("manifest_path")?.as_str()?).parent().map(Path::to_path_buf) };
    let mut by_dir: HashMap<PathBuf, &Value> = HashMap::new();
    for p in packages {
        if let Some(dir) = dir_of(p) {
            by_dir.insert(dir, p);
        }
    }
    let start = packages
        .iter()
        .filter(|p| p.get("name").and_then(Value::as_str) == Some(package))
        .find(|p| dir_of(p).is_some_and(|d| d.starts_with(root)))
        .ok_or_else(|| format!("{package} is not in the metadata"))?;
    let mut seen: Vec<PathBuf> = vec![dir_of(start).unwrap_or_default()];
    let mut todo = vec![start];
    while let Some(p) = todo.pop() {
        for dep in p.get("dependencies").and_then(Value::as_arr).unwrap_or(&[]) {
            if dep.get("kind").and_then(Value::as_str) == Some("dev") {
                continue;
            }
            let Some(path) = dep.get("path").and_then(Value::as_str).map(PathBuf::from) else { continue };
            if !path.starts_with(root) || seen.contains(&path) {
                continue;
            }
            seen.push(path.clone());
            if let Some(next) = by_dir.get(&path) {
                todo.push(next);
            }
        }
    }
    let mut dirs: Vec<String> = seen
        .iter()
        .filter_map(|d| d.strip_prefix(root).ok())
        .map(|d| d.to_string_lossy().replace('\\', "/"))
        .filter(|d| !d.is_empty())
        .collect();
    dirs.sort();
    Ok(dirs)
}

/// `rel` is inside the package folder `dir` (both relative to the root).
pub fn in_package(dir: &str, rel: &str) -> bool {
    let dir = dir.trim_end_matches('/');
    rel.strip_prefix(dir).is_some_and(|rest| rest.starts_with('/'))
}

fn exit_text(status: &ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit {code}"),
        None => "killed by a signal".into(),
    }
}

/// A command line cut for a one-line label.
pub fn cut_command(command: &str, max: usize) -> String {
    let one = command.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        one
    } else {
        one.chars().take(max.saturating_sub(3)).collect::<String>() + "..."
    }
}

/// Read a pipe to its end, handing over whole lines a chunk at a time:
/// escape sequences removed, indentation kept, and a carriage return
/// starts its line over (progress that redraws itself counts once). A
/// line without its newline goes out at the end.
pub fn read_lines(mut stream: impl std::io::Read, mut emit: impl FnMut(Vec<String>)) {
    let mut buf = [0u8; 8192];
    let mut line: Vec<u8> = Vec::new();
    let mut cr = false;
    loop {
        let n = match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let mut out = Vec::new();
        for &b in &buf[..n] {
            match b {
                b'\n' => {
                    out.push(clean_line(&line));
                    line.clear();
                    cr = false;
                }
                b'\r' => cr = true,
                _ => {
                    if cr {
                        line.clear();
                        cr = false;
                    }
                    if line.len() < 64 * 1024 {
                        line.push(b);
                    }
                }
            }
        }
        emit(out);
    }
    if !line.is_empty() {
        emit(vec![clean_line(&line)]);
    }
}

/// One output line as text without terminal escapes.
fn clean_line(bytes: &[u8]) -> String {
    let raw = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some('[') => {
                    for d in chars.by_ref() {
                        if ('@'..='~').contains(&d) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    for d in chars.by_ref() {
                        if d == '\x07' {
                            break;
                        }
                    }
                }
                _ => {}
            },
            c => out.push(c),
        }
    }
    out.trim_end().to_string()
}

/// Spawn with piped output, no stdin, as the leader of a new process group.
fn spawn_group(mut command: Command) -> std::io::Result<Child> {
    // Code runs hosted in the WM: an app it starts is not Code's client and never takes the
    // direct display; it finds the WM itself (MAKEPAD_WM_SOCKET) and becomes its own window.
    registry::child_env::scrub(&mut command);
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn()
}

/// Signal the process group this manager started (its leader is `pid`).
#[cfg(unix)]
fn signal_group(pid: u32, hard: bool) {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    let sig = if hard { 9 } else { 15 };
    if pid > 1 {
        // SAFETY: plain syscall on the group of a child this manager spawned.
        unsafe {
            kill(-(pid as i32), sig);
        }
    }
}

#[cfg(not(unix))]
fn signal_group(pid: u32, _hard: bool) {
    let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).status();
}

/// `session relaunch` on a `makepad run` process's endpoint.
/// How a `makepad run` of `app` starts. From a checkout: `cargo run --release
/// -p makepad-run --bin makepad -- run <package> [-- <app args>]`, so cargo
/// rebuilds the tool whenever its sources changed (never a prebuilt `makepad`
/// older than the checkout's provided crates). A Builder installation
/// (`builder`) starts its published `makepad` with the same arguments.
fn makepad_run_argv(app: &AppEntry, root: &Path, builder: bool) -> Result<(PathBuf, Vec<String>), String> {
    let mut args = vec!["run".to_string(), app.package.to_string()];
    if !app.args.is_empty() {
        args.push("--".into());
        args.extend(app.args.iter().map(|a| a.to_string()));
    }
    if builder {
        let program = registry::built_binary(&MAKEPAD_RUN, Some(root)).ok_or("the makepad tool is not built yet")?;
        return Ok((program, args));
    }
    Ok(registry::run_argv(&MAKEPAD_RUN, root, &args))
}

/// The stage of a `cargo run` after `line`: cargo's progress wording
/// ("waiting for another build…" while another cargo holds the build lock),
/// None once cargo runs the tool.
fn cargo_stage(line: &str, stage: Option<String>) -> Option<String> {
    if line.trim_start().starts_with("Running ") {
        return None;
    }
    registry::cargo_progress(line).map(|(text, _)| text).or(stage)
}

fn session_relaunch(pid: u32) -> Result<String, String> {
    let client = makepad_remote::Client::connect(&pid.to_string()).map_err(|e| e.to_string())?;
    client.set_timeout(Some(Duration::from_secs(10)));
    let mut lines = Vec::new();
    for line in client.cmd("session relaunch").map_err(|e| e.to_string())? {
        lines.push(line.map_err(|e| e.to_string())?);
    }
    client.close();
    let text = lines.join(" ");
    if text.starts_with("{\"err\"") { Err(text) } else { Ok(text) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn apps_are_found_by_any_name_a_person_says() {
        let all: Vec<&'static AppEntry> = registry::apps().iter().collect();
        assert_eq!(super::find_entry(&all, "Stage").map(|a| a.id), Some("stage"));
        assert_eq!(super::find_entry(&all, "stage").map(|a| a.id), Some("stage"));
        assert_eq!(super::find_entry(&all, "makepad-stage").map(|a| a.id), Some("stage"));
        assert_eq!(super::find_entry(&all, "Calculator").map(|a| a.id), Some("calculator"));
        assert_eq!(super::find_entry(&all, "makepad-app-calculator").map(|a| a.id), Some("calculator"));
        assert_eq!(super::find_entry(&all, "task manager").map(|a| a.id), Some("task"));
        assert!(super::find_entry(&all, "no such thing").is_none());
    }

    use super::*;

    /// Captured from `cargo build --release` of a package with one warning
    /// and two errors (paths relative to the workspace root), progress lines
    /// split at their carriage returns as `read_output` gives them.
    const CAPTURED: &str = "\
Blocking waiting for file lock on build directory
Compiling makepad-live-id v0.1.0 (/ws/libs/live_id)
Building [=======>                 ] 143/512: makepad-widgets, makepad-draw
Compiling makepad-app-calculator v0.1.0 (/ws/apps/calculator)
warning: unused variable: `x`
--> apps/calculator/src/app.rs:10:9
|
10 |     let x = 1;
|         ^ help: if this is intentional, prefix it with an underscore: `_x`
|
= note: `#[warn(unused_variables)]` on by default
error[E0425]: cannot find value `y` in this scope
--> apps/calculator/src/app.rs:12:5
|
12 |     y
|     ^ not found in this scope
error[E0308]: mismatched types
--> /ws/apps/calculator/src/main.rs:3:14
|
note: expected because of this
--> apps/calculator/src/other.rs:1:1
error: linking with `cc` failed: exit status: 1
warning: `makepad-app-calculator` (bin \"makepad-app-calculator\") generated 1 warning
error: could not compile `makepad-app-calculator` (bin \"makepad-app-calculator\") due to 2 previous errors; 1 warning emitted
warning: unused variable: `x`
--> apps/calculator/src/app.rs:10:9
";

    fn parsed() -> BuildParser {
        let mut p = BuildParser::new(Path::new("/ws"));
        for line in CAPTURED.lines() {
            p.feed(line);
        }
        p
    }

    #[test]
    fn progress_counts_and_crate() {
        let mut p = BuildParser::new(Path::new("/ws"));
        p.feed("Blocking waiting for file lock on build directory");
        assert!(p.waiting);
        p.feed("Building [=======>                 ] 143/512: makepad-widgets, makepad-draw");
        assert!(!p.waiting);
        assert_eq!(p.progress, Progress { done: 143, total: 512, current: "makepad-widgets".into() });
        p.feed("Compiling makepad-app-calculator v0.1.0 (/ws/apps/calculator)");
        assert_eq!(p.progress.current, "makepad-app-calculator");
        p.feed("Finished `release` profile [optimized] target(s) in 1m 02s");
        assert_eq!(p.progress.done, 512);
    }

    #[test]
    fn diagnostics_with_locations_relative_to_the_root() {
        let p = parsed();
        let got: Vec<(Level, &str, String)> = p.diags.iter().map(|d| (d.level, d.message.as_str(), d.location())).collect();
        assert_eq!(
            got,
            vec![
                (Level::Warning, "unused variable: `x`", "apps/calculator/src/app.rs:10:9".to_string()),
                (Level::Error, "cannot find value `y` in this scope [E0425]", "apps/calculator/src/app.rs:12:5".to_string()),
                (Level::Error, "mismatched types [E0308]", "apps/calculator/src/main.rs:3:14".to_string()),
                (Level::Error, "linking with `cc` failed: exit status: 1", String::new()),
            ]
        );
        assert_eq!((p.errors(), p.warnings()), (3, 1));
    }

    #[test]
    fn locations_split_from_the_right() {
        assert_eq!(split_location("C:\\ws\\a.rs:3:4"), Some(("C:\\ws\\a.rs".into(), 3, 4)));
        assert_eq!(split_location("src/a.rs:30:1"), Some(("src/a.rs".into(), 30, 1)));
        assert_eq!(split_location("src/a.rs"), None);
    }

    #[test]
    fn builds_run_as_is_and_under_the_lock_only_when_set() {
        let args = vec!["build".to_string(), "--release".to_string()];
        let plain = build_argv_with_lock("cargo", &args, None);
        let locked = build_argv_with_lock("cargo", &args, Some("/tmp/x.lock"));
        assert_eq!(plain, ["cargo", "build", "--release"]);
        if cfg!(unix) {
            let mut want = if cfg!(target_os = "linux") { vec!["flock", "/tmp/x.lock"] } else { vec!["lockf", "-k", "/tmp/x.lock"] };
            want.extend(["cargo", "build", "--release"]);
            assert_eq!(locked, want);
        } else {
            assert_eq!(locked, ["cargo", "build", "--release"]);
        }
    }

    #[test]
    fn path_dependencies_are_followed_inside_the_root() {
        let meta = br#"{"packages":[
            {"name":"app","manifest_path":"/ws/apps/app/Cargo.toml","dependencies":[
                {"name":"widgets","kind":null,"path":"/ws/widgets"},
                {"name":"tester","kind":"dev","path":"/ws/libs/tester"},
                {"name":"cc","kind":"build"},
                {"name":"outside","kind":null,"path":"/elsewhere/outside"}]},
            {"name":"widgets","manifest_path":"/ws/widgets/Cargo.toml","dependencies":[
                {"name":"draw","kind":null,"path":"/ws/draw"},
                {"name":"gen","kind":"build","path":"/ws/libs/gen"}]},
            {"name":"draw","manifest_path":"/ws/draw/Cargo.toml","dependencies":[
                {"name":"widgets","kind":null,"path":"/ws/widgets"}]},
            {"name":"tester","manifest_path":"/ws/libs/tester/Cargo.toml","dependencies":[
                {"name":"never","kind":null,"path":"/ws/libs/never"}]}
        ],"resolve":null}"#;
        let dirs = path_deps(meta, Path::new("/ws"), "app").unwrap();
        assert_eq!(dirs, ["apps/app", "draw", "libs/gen", "widgets"]);
        assert!(path_deps(meta, Path::new("/ws"), "nope").is_err());
    }

    /// The real checkout: calculator reaches widgets/ and strict_json.
    #[test]
    fn this_checkout_s_calculator_dependencies() {
        let root = registry::checkout_at_or_above(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let dirs = metadata_deps(&root, "makepad-app-calculator").unwrap();
        for want in ["apps/calculator", "widgets", "libs/strict_json", "platform"] {
            assert!(dirs.iter().any(|d| d == want), "{want} missing from {dirs:?}");
        }
    }

    #[test]
    fn live_counts_runs_and_builds() {
        let m = RunManager::global();
        let app = registry::find("notes").unwrap();
        let id = {
            let mut st = m.lock();
            let id = st.fresh_id();
            st.running.push(Running { id, app, mode: Mode::MakepadRun, pid: 0, started: Instant::now(), exited: None, stopping: false, restart: false, building: None, saved: false, live: LiveParser::default(), log: Ring::default() });
            id
        };
        // other tests share the manager: count what this one added
        assert!(m.live_count() >= 1);
        assert!(m.live_names().contains(&"notes (makepad run)".to_string()));
        m.lock().running.retain(|r| r.id != id);
        assert!(!m.live_names().contains(&"notes (makepad run)".to_string()));
    }

    #[test]
    fn modes_and_packages() {
        assert_eq!(Mode::parse("makepad_run"), Some(Mode::MakepadRun));
        assert_eq!(Mode::parse("makepad run"), Some(Mode::MakepadRun));
        assert_eq!(Mode::parse(""), Some(Mode::Native));
        assert_eq!(Mode::parse("bogus"), None);
        assert!(in_package("apps/calculator", "apps/calculator/src/app.rs"));
        assert!(!in_package("apps/calc", "apps/calculator/src/app.rs"));
        assert!(!in_package("apps/calculator", "apps/calculator"));
    }

    /// The manager over this checkout: the registry's apps with sources
    /// here, found by id, package or label; nothing built or running yet.
    #[test]
    fn the_manager_lists_the_registry() {
        let m = RunManager::global();
        assert!(m.set_root(Path::new(env!("CARGO_MANIFEST_DIR"))));
        let apps = m.apps();
        assert!(apps.iter().any(|a| a.id == "calculator"), "{apps:?}");
        assert!(apps.iter().any(|a| a.id == "code"));
        assert_eq!(m.find_app("makepad-app-calculator").map(|a| a.id), Some("calculator"));
        assert_eq!(m.find_app("Calculator").map(|a| a.id), Some("calculator"));
        assert!(m.find_app("no-such-app").is_none());
        let text = m.apps_text();
        assert!(text.contains("calculator  Calculator  package makepad-app-calculator"), "{text}");
        assert_eq!(m.status_text(registry::find("notes")), "no builds or runs yet");
    }

    /// A real child through the manager's process plumbing: output lands
    /// in the log, the exit is seen, and stopping signals its group only.
    #[cfg(unix)]
    #[test]
    fn a_child_is_watched_and_stopped_by_its_group() {
        let m = RunManager::global();
        m.set_root(Path::new(env!("CARGO_MANIFEST_DIR")));
        let app = registry::find("calculator").unwrap();
        let mut command = Command::new("sh");
        command.args(["-c", "echo hello; echo world 1>&2; exec sleep 30"]);
        let child = spawn_group(command).unwrap();
        let id = {
            let mut st = m.lock();
            let id = st.fresh_id();
            st.running.push(Running { id, app, mode: Mode::Native, pid: child.id(), started: Instant::now(), exited: None, stopping: false, restart: false, building: None, saved: false, live: LiveParser::default(), log: Ring::default() });
            id
        };
        m.watch(child, move |m, out| m.run_output(id, out), move |m, status| m.run_exited(id, status));
        let end = Instant::now() + Duration::from_secs(5);
        while m.run_log(id, 10).len() < 2 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        let log = m.run_log(id, 10);
        assert!(log.contains(&"hello".to_string()) && log.contains(&"world".to_string()), "{log:?}");
        m.stop(id).unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while m.snapshot().running.iter().any(|r| r.id == id && r.exited.is_none()) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        let r = m.snapshot().running.into_iter().find(|r| r.id == id).unwrap();
        assert_eq!(r.exited.as_deref(), Some("stopped"));
        // A second stop of an exited run removes it.
        m.stop(id).unwrap();
        assert!(m.snapshot().running.iter().all(|r| r.id != id));
    }
    /// From a checkout `makepad run` is a release `cargo run` of the tool with
    /// the app's package and arguments: cargo rebuilds a `makepad` older than
    /// the checkout (its provided crates), which a prebuilt binary refused
    /// ("provided crate .. differs from the library built into makepad").
    #[test]
    fn a_makepad_run_from_a_checkout_is_a_release_cargo_run_of_the_tool() {
        let root = Path::new("/checkout");
        let app = registry::apps().iter().find(|a| !a.args.is_empty()).or_else(|| registry::find("calculator")).unwrap();
        let (program, args) = makepad_run_argv(app, root, false).unwrap();
        assert_eq!(program, registry::cargo_bin());
        let manifest = root.join("Cargo.toml");
        let mut want: Vec<String> = ["run", "--release", "--manifest-path", &manifest.to_string_lossy(), "-p", "makepad-run", "--bin", "makepad", "--", "run", app.package]
            .iter()
            .map(|a| a.to_string())
            .collect();
        if !app.args.is_empty() {
            want.push("--".into());
            want.extend(app.args.iter().map(|a| a.to_string()));
        }
        assert_eq!(args, want);
        // the stage cargo is in shows on the run until cargo runs the tool
        let mut stage = Some("starting cargo…".to_string());
        for (line, want) in [
            ("Blocking waiting for file lock on build directory", Some("waiting for another build…")),
            ("Compiling makepad-hal v1.0.0 (/checkout/platform/hal)", Some("compiling makepad-hal v1.0.0…")),
            ("warning: unused variable: `x`", Some("compiling makepad-hal v1.0.0…")),
            ("Finished `release` profile [optimized] target(s) in 41.20s", Some("launching…")),
            ("Running `/checkout/target/release/makepad run makepad-example-counter`", None),
        ] {
            stage = cargo_stage(line, stage);
            assert_eq!(stage.as_deref(), want, "{line}");
        }
    }
    /// What `makepad run` prints for errors in the code becomes the run's
    /// diagnostics, relative to the root; the next edit outcome replaces them.
    #[test]
    fn a_runs_errors_are_read_from_its_output() {
        let mut p = LiveParser::new(Path::new("/ws"));
        let feed = |p: &mut LiveParser, lines: &[&str]| for l in lines { p.feed(l); };
        feed(&mut p, &[
            "makepad live: main.rs: NOT applied (errors below); the app runs the previous code",
            "makepad live:   /ws/examples/counter/src/main.rs:51:24: E0308: mismatched types: expected `u32`, found `&str`",
        ]);
        assert_eq!(p.diags, [Diagnostic { level: Level::Error, message: "E0308: mismatched types: expected `u32`, found `&str`".into(), path: Some("examples/counter/src/main.rs".into()), line: 51, col: 24 }]);
        // fixed: the patch clears them
        feed(&mut p, &["makepad live: main.rs: patched (1 item(s) changed, 1 fn(s) installed) in 54.1 ms"]);
        assert!(p.diags.is_empty());
        // a restart's reasons are not errors
        feed(&mut p, &[
            "makepad live: main.rs: relaunching the app domain (window kept)",
            "makepad live:   /ws/examples/counter/src/main.rs:1:1: impl `` removed -- removing this kind of item is not applied live yet: restart the app",
        ]);
        assert!(p.diags.is_empty());
        // the borrow checker's refusal: compiler-style, the place on the `-->` line
        feed(&mut p, &[
            "borrow check failed: app::App::tick at /ws/examples/counter/src/main.rs:51:42: cannot borrow `v` as mutable (live edit refused)",
            "makepad live: /ws/examples/counter/src/main.rs: NOT applied (errors below); the app runs the previous code",
            "makepad live:   refused by the borrow checker (1 error(s))",
            "makepad live:   error[E0502]: cannot borrow `v` as mutable because it is also borrowed as immutable",
            "  --> /ws/examples/counter/src/main.rs:51:42",
            "   |",
        ]);
        assert_eq!(p.diags.iter().map(|d| (d.line, d.col, d.message.as_str())).collect::<Vec<_>>(), [(51, 42, "cannot borrow `v` as mutable because it is also borrowed as immutable [E0502]")]);
        // a domain that failed to load
        feed(&mut p, &["makepad live: main.rs: relaunching the app domain (window kept)"]);
        feed(&mut p, &["makepad-run domain: makepad-example-counter: /ws/examples/counter/src/main.rs:51:5: expected item (found KwLet `let`)"]);
        assert_eq!(p.diags.iter().map(|d| (d.line, d.col, d.message.as_str())).collect::<Vec<_>>(), [(51, 5, "expected item (found KwLet `let`)")]);
        feed(&mut p, &["makepad live: main.rs: patched (1 item(s) changed, 1 fn(s) installed) in 54.1 ms"]);
        // a compile error at start or after a relaunch, and a refused borrow check
        feed(&mut p, &[
            "makepad-run: [t=170471 ms] compile error in makepad_example_counter::::handle_actions: /ws/examples/counter/src/main.rs:51:24: E0308: mismatched types",
            "borrow check failed: app::App::tick at /ws/apps/x/src/app.rs:7:13: cannot borrow `v` as mutable more than once at a time (live edit refused)",
        ]);
        let places: Vec<_> = p.diags.iter().map(|d| (d.path.clone().unwrap(), d.line, d.col, d.message.clone())).collect();
        assert_eq!(places, [
            ("examples/counter/src/main.rs".to_string(), 51, 24, "E0308: mismatched types".to_string()),
            ("apps/x/src/app.rs".to_string(), 7, 13, "borrow check: cannot borrow `v` as mutable more than once at a time".to_string()),
        ]);
    }
}

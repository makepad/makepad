//! Client processes and the app-launching model, behavior read from
//! omarchy's source (local/agent_state/wm/omarchy-launch-model.md):
//!
//! - the terminal is ALWAYS a fresh instance, opened in the cwd of the
//!   focused terminal (omarchy-launch-terminal + omarchy-cmd-terminal-cwd;
//!   our children report pwd over OSC 7 -> Layer B custom message),
//! - other apps use launch-or-focus: `\b<pattern>\b` case-insensitive
//!   against window class OR title focuses an existing window, else spawns
//!   (bin/omarchy-launch-or-focus).
//!
//! Children are Makepad apps launched with `--stdin-loop` and
//! `STUDIO_HOST`/`STUDIO_BUILD` pointing at the in-process hub, exactly
//! like studio launches run targets.
//!
//! **They are launched through cargo** (`cargo run --release -p <pkg>`)
//! whenever wm is running out of a checkout, so a stale or missing
//! binary is rebuilt on launch instead of failing or showing yesterday's
//! app. cargo passes stdio and the environment straight through, so the
//! protocol is unaffected; its "Compiling …" output lands in the client's
//! log. An installed wm with no checkout around it falls back to
//! exec'ing the sibling binary.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
#[cfg(unix)]
use std::os::unix::process::CommandExt;

use makepad_widgets::makepad_platform::thread::SignalToUI;

use crate::hub::ClientId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchPolicy {
    /// Every invocation spawns a new instance (the terminal, viewers).
    AlwaysNew,
    /// Focus a running instance of this app if one exists, else spawn.
    OrFocus,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppDef {
    /// Registry id, also the launch-or-focus window pattern.
    pub id: String,
    /// The name a human reads in the menu.
    pub label: String,
    /// Binary name, for the installed (no checkout) fallback.
    pub bin: String,
    /// Cargo package name — what `cargo run -p` gets.
    pub package: String,
    /// Package directory relative to the checkout root.
    pub dir: String,
    /// A crate outside the root workspace (its own workspace root) needs
    /// its manifest named explicitly; relative to the checkout root.
    pub manifest: Option<String>,
    pub args: Vec<String>,
    pub policy: LaunchPolicy,
}

impl AppDef {
    fn app(
        id: &str,
        label: &str,
        package: &str,
        dir: &str,
        bin: &str,
        policy: LaunchPolicy,
    ) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            bin: bin.to_string(),
            package: package.to_string(),
            dir: dir.to_string(),
            manifest: None,
            args: Vec::new(),
            policy,
        }
    }

    /// True when this app can actually be started right now — the honest
    /// filter behind the menu (no row that cannot run).
    pub fn is_available(&self) -> bool {
        if let Some(root) = repo_root() {
            let manifest = self
                .manifest
                .clone()
                .unwrap_or_else(|| format!("{}/Cargo.toml", self.dir));
            return root.join(manifest).exists();
        }
        resolve_bin(&self.bin).is_some()
    }
}

/// The curated applications, in menu order. Every one of these is a real
/// window we host; servers and headless tools are deliberately absent.
fn curated() -> Vec<AppDef> {
    use LaunchPolicy::*;
    vec![
        AppDef::app("browser", "Browser", "makepad-browser", "apps/browser", "browser", OrFocus),
        {
            // Recording default: the Files row in the menu opens the demo
            // VFS (virtual home over repo assets), never the real disk.
            // Drop the arg (or set MAKEPAD_WM_FILES_REAL=1) to browse for real.
            let mut files =
                AppDef::app("files", "Files", "makepad-files", "apps/files", "files", OrFocus);
            if std::env::var("MAKEPAD_WM_FILES_REAL").is_err() {
                files.args.push("--demo".to_string());
            }
            files
        },
        AppDef::app("terminal", "Terminal", "makepad-terminal", "apps/terminal", "terminal", AlwaysNew),
        AppDef::app("mixer", "Mixer", "makepad-mixer", "apps/mixer", "makepad-mixer", OrFocus),
        AppDef::app("task", "Task Manager", "makepad-task", "apps/task", "task", OrFocus),
        AppDef::app("sheets", "Sheets", "makepad-sheets", "apps/sheets", "sheets", OrFocus),
        AppDef::app(
            "score",
            "Score",
            "makepad-app-score",
            "apps/score",
            "makepad-app-score",
            OrFocus,
        ),
        // A viewer instance per file, so previews never steal each other's
        // window.
        // Image and PDF viewers are NOT menu rows — they open through
        // Files / previews (see find_app's hidden entries).
        AppDef::app("video", "Video Player", "makepad-video", "apps/video", "video", AlwaysNew),
        AppDef::app(
            "route",
            "Route",
            "makepad-app-route",
            "apps/route",
            "makepad-app-route",
            OrFocus,
        ),
        AppDef::app("vj", "VJ", "makepad-vj", "apps/vj", "makepad-vj", OrFocus),
        {
            // Fab opens the pretty house when the converted model is
            // around (children run with cwd = repo root); the built-in
            // demo house otherwise.
            let mut fab = AppDef::app("fab", "Fab", "makepad-fab", "apps/fab", "makepad-fab", OrFocus);
            let house = "local/fab/models/woodside.glb";
            let exists = repo_root().map(|r| r.join(house).exists()).unwrap_or(false);
            if exists {
                fab.args.push("--open".to_string());
                fab.args.push(house.to_string());
            }
            fab
        },
        AppDef::app(
            "studio",
            "Studio",
            "makepad-studio",
            "studio/desktop",
            "makepad-studio",
            OrFocus,
        ),
    ]
}

/// One `name = "..."` value out of a Cargo.toml `[package]` table.
#[cfg(test)]
fn manifest_value(manifest: &str, key: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() != key {
            continue;
        }
        return Some(v.trim().trim_matches('"').to_string());
    }
    None
}

/// The app registry: the applications this WM is built around, in menu
/// order. Curated on purpose — every row is one we run and verify, not a
/// scan of whatever the workspace happens to contain.
pub fn registry() -> &'static [AppDef] {
    static REGISTRY: OnceLock<Vec<AppDef>> = OnceLock::new();
    REGISTRY.get_or_init(curated)
}

/// Resolve an app id for LAUNCHING: the curated menu first, then hidden
/// test-only entries (protocol/pacing rigs via MAKEPAD_WM_TEST_APP) that never
/// appear in a menu.
pub fn find_app(id: &str) -> Option<AppDef> {
    if let Some(app) = registry().iter().find(|a| a.id == id) {
        return Some(app.clone());
    }
    // The file associations (`makepad_wm_api::viewer_for`) name binaries, since
    // standalone apps spawn them as siblings; here those resolve to their
    // curated entries (terminal → terminal, browser → browser, …).
    if let Some(app) = registry().iter().find(|a| a.bin == id) {
        return Some(app.clone());
    }
    use LaunchPolicy::*;
    match id {
        // The file viewers: launchable (previews, Open With) but not menu
        // rows.
        "image" => Some(AppDef::app(
            "image",
            "Image Viewer",
            "makepad-image",
            "apps/image",
            "image",
            AlwaysNew,
        )),
        "pdf" => Some(AppDef::app(
            "pdf",
            "PDF Viewer",
            "makepad-pdf",
            "apps/pdf",
            "pdf",
            AlwaysNew,
        )),
        // A continuously animating client: the pacing/hiccup instrument.
        "splash" => Some(AppDef::app(
            "splash",
            "Splash",
            "makepad-example-splash",
            "examples/splash",
            "makepad-example-splash",
            AlwaysNew,
        )),
        "counter" => Some(AppDef::app(
            "counter",
            "Counter",
            "makepad-example-counter",
            "examples/counter",
            "makepad-example-counter",
            AlwaysNew,
        )),
        _ => None,
    }
}

/// `bin/omarchy-launch-or-focus`'s window test, verbatim:
/// `test("\\b" + pattern + "\\b"; "i")` — a case-insensitive WHOLE-WORD
/// match, where a word boundary is any non-alphanumeric/underscore.
pub fn word_match(haystack: &str, pattern: &str) -> bool {
    if pattern.is_empty() {
        return false;
    }
    let hay = haystack.to_lowercase();
    let pat = pattern.to_lowercase();
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let bytes: Vec<char> = hay.chars().collect();
    let needle: Vec<char> = pat.chars().collect();
    if needle.len() > bytes.len() {
        return false;
    }
    for start in 0..=bytes.len() - needle.len() {
        if bytes[start..start + needle.len()] != needle[..] {
            continue;
        }
        let before_ok = start == 0 || !word(bytes[start - 1]);
        let end = start + needle.len();
        let after_ok = end == bytes.len() || !word(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
    }
    false
}

/// The checkout root when wm runs from `target/<profile>/wm`, else
/// MAKEPAD_WM_ROOT, else none.
pub fn repo_root() -> Option<PathBuf> {
    if let Ok(root) = std::env::var("MAKEPAD_WM_ROOT") {
        return Some(PathBuf::from(root));
    }
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?.to_path_buf();
    for _ in 0..4 {
        if dir.join("Cargo.toml").exists() && dir.join("local").exists() {
            return Some(dir);
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

/// Resolve a sibling binary of the running wm executable.
pub fn resolve_bin(bin: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let path = dir.join(bin);
    path.exists().then_some(path)
}

/// The cargo to launch with: whatever is on PATH, else the rustup default.
fn cargo_bin() -> PathBuf {
    if let Ok(cargo) = std::env::var("CARGO") {
        return PathBuf::from(cargo);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let rustup = PathBuf::from(home).join(".cargo/bin/cargo");
        if rustup.exists() {
            return rustup;
        }
    }
    PathBuf::from("cargo")
}

// ======================================================================
// The warm-instance pool
// ======================================================================

/// How many DORMANT instances of an app the pool keeps standing by, so a
/// new window is a swap instead of a launch. The user's sizing: terminals
/// get two (people burst-open them), the rest one each. An app that is not
/// in this table is never pre-spawned.
///
/// This is the whole registry of warmable apps — `is_warm_app` and the
/// startup top-up both read it, so adding an app here is the only edit an
/// app needs to join the pool.
pub const WARM_CAPACITY: &[(&str, usize)] = &[
    ("terminal", 2),
    ("browser", 1),
    ("files", 1),
    ("task", 1),
];

/// The env a warm instance is spawned with. `makepad_wm_api::warm_start()` reads
/// exactly this: the app boots its window and draws once, then IDLES — no
/// samplers, no refresh timers, no polling — until `WmEvent::Adopted`
/// arrives. Without it a cached task manager would sit there sampling
/// every process on the machine for nothing.
pub const WARM_ENV: (&str, &str) = ("MAKEPAD_WM_WARM_START", "1");

/// Crash budget: this many UNEXPECTED warm deaths per app inside
/// `WARM_CRASH_WINDOW`, after which the pool gives that app up quietly and
/// every launch takes the cold path (which always works). Adoption
/// replacements are NOT crashes and are never capped — capping those would
/// switch the pool off for anyone who opens four terminals in a minute,
/// which is exactly who it exists for.
pub const WARM_CRASH_LIMIT: usize = 3;
pub const WARM_CRASH_WINDOW: Duration = Duration::from_secs(60);

/// What the WM knows about one pooled instance right now, handed to
/// `WarmPool::adopt` so the pool itself stays free of WM state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WarmStatus {
    pub client: ClientId,
    /// Still in the client table: the process has not been reaped.
    pub alive: bool,
    /// Connected to the hub AND past `CreateWindow` — it has a framebuffer
    /// and a drawn frame, so a tile can show it this instant. A warm
    /// instance that is still building (or still starting) is not one.
    pub connected: bool,
}

/// The pool: per app, the ids of the instances standing by.
///
/// Deliberately a plain state machine over ids — no processes, no cx, no
/// layout — so the rules that matter (adopt clears and tops back up, a
/// dead instance falls back to a cold spawn, a cwd override skips the pool,
/// MAKEPAD_WM_NO_WARM turns it off, crash loops give up) are unit-testable
/// without a running window manager.
#[derive(Debug)]
pub struct WarmPool {
    enabled: bool,
    /// app id -> the warm clients of that app, oldest first.
    ready: HashMap<String, Vec<ClientId>>,
    /// app id -> when its warm instances died unexpectedly, newest last.
    crashes: HashMap<String, Vec<Instant>>,
}

impl Default for WarmPool {
    fn default() -> Self {
        Self::from_env()
    }
}

/// MAKEPAD_WM_NO_WARM disables the pool entirely. An empty or `0` value is not a
/// request — `MAKEPAD_WM_NO_WARM=` in a stale profile should not silently cost
/// everyone the feature.
pub fn warm_enabled(no_warm: Option<&str>) -> bool {
    match no_warm {
        None => true,
        Some(v) => matches!(v.trim(), "" | "0"),
    }
}

impl WarmPool {
    pub fn from_env() -> Self {
        Self::new(warm_enabled(std::env::var("MAKEPAD_WM_NO_WARM").ok().as_deref()))
    }

    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ready: HashMap::new(),
            crashes: HashMap::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// How many instances of this app the pool wants standing by; 0 for an
    /// app that is not pooled at all.
    pub fn capacity(app: &str) -> usize {
        WARM_CAPACITY
            .iter()
            .find(|(id, _)| *id == app)
            .map(|(_, n)| *n)
            .unwrap_or(0)
    }

    pub fn is_warm_app(app: &str) -> bool {
        Self::capacity(app) > 0
    }

    /// How many instances of this app are currently held.
    pub fn held(&self, app: &str) -> usize {
        self.ready.get(app).map(|v| v.len()).unwrap_or(0)
    }

    /// Every warm client, whatever the app — the shutdown / close-all
    /// paths walk this so no pooled process is ever left behind.
    pub fn clients(&self) -> Vec<ClientId> {
        let mut all: Vec<ClientId> = self.ready.values().flatten().copied().collect();
        all.sort_unstable();
        all
    }

    pub fn holds(&self, client: ClientId) -> bool {
        self.ready.values().any(|v| v.contains(&client))
    }

    /// True while this app is under capacity and inside its crash budget:
    /// the WM may spawn one more standby instance now.
    pub fn wants(&self, app: &str, now: Instant) -> bool {
        self.enabled
            && self.held(app) < Self::capacity(app)
            && self.recent_crashes(app, now) < WARM_CRASH_LIMIT
    }

    /// The next app that is short an instance, in table order — the tick
    /// tops the pool up ONE spawn at a time so a cold start never forks
    /// five cargo builds into the same target-dir lock at once.
    pub fn next_missing(&self, now: Instant) -> Option<String> {
        WARM_CAPACITY
            .iter()
            .map(|(app, _)| *app)
            .find(|app| self.wants(app, now))
            .map(str::to_string)
    }

    /// A standby instance was spawned for `app`.
    pub fn note_spawned(&mut self, app: &str, client: ClientId) {
        self.ready.entry(app.to_string()).or_default().push(client);
    }

    /// A warm instance died on its own. Counted against the crash budget;
    /// a DELIBERATE close (WM shutdown, close-all) calls `forget` instead.
    pub fn note_crash(&mut self, app: &str, now: Instant) {
        self.crashes.entry(app.to_string()).or_default().push(now);
    }

    fn recent_crashes(&self, app: &str, now: Instant) -> usize {
        self.crashes
            .get(app)
            .map(|v| {
                v.iter()
                    .filter(|t| now.saturating_duration_since(**t) < WARM_CRASH_WINDOW)
                    .count()
            })
            .unwrap_or(0)
    }

    /// Drop a client from the pool however it left (died, was closed with
    /// everything else). Returns the app it was standing by for, which is
    /// the app the caller then tops back up.
    pub fn forget(&mut self, client: ClientId) -> Option<String> {
        let mut which = None;
        for (app, ids) in self.ready.iter_mut() {
            if let Some(pos) = ids.iter().position(|c| *c == client) {
                ids.remove(pos);
                which = Some(app.clone());
                break;
            }
        }
        which
    }

    /// THE decision, for one launch of `app`.
    ///
    /// `Some(client)` = adopt that instance into a real tile (it leaves the
    /// pool; the caller tops the app back up immediately). `None` = spawn
    /// cold exactly as before. Dead entries are pruned on the way past, so
    /// a crashed instance both falls back cleanly AND frees its slot for
    /// the next respawn.
    ///
    /// THE CWD CARVE-OUT (omarchy's rule, `omarchy-cmd-terminal-cwd`): a
    /// new terminal opens in the FOCUSED terminal's directory. A warm
    /// terminal's shell started long ago, in the default directory — it
    /// cannot be moved after the fact without lying about where it is — so
    /// when a cwd is being inherited the pool stands aside and the launch
    /// goes cold. Correct beats instant; the instant path is what you get
    /// from the desktop, the bar and any non-terminal focus.
    pub fn adopt(
        &mut self,
        app: &str,
        cwd_override: bool,
        status: &[WarmStatus],
    ) -> Option<ClientId> {
        if !self.enabled {
            return None;
        }
        let ids = self.ready.get_mut(app)?;
        ids.retain(|id| {
            status
                .iter()
                .any(|s| s.client == *id && s.alive)
        });
        if cwd_override {
            return None;
        }
        let pos = ids.iter().position(|id| {
            status
                .iter()
                .any(|s| s.client == *id && s.connected)
        })?;
        Some(ids.remove(pos))
    }
}

pub struct ClientSlot {
    #[allow(dead_code)]
    pub id: ClientId,
    /// Registry id of the app this client runs.
    pub app: String,
    pub title: String,
    pub child: Option<Child>,
    pub sender: Option<Sender<Vec<u8>>>,
    pub socket: Option<u64>,
    /// The child's main window id in the studio protocol (0 until
    /// CreateWindow says otherwise).
    pub window_id: usize,
    /// CreateWindow arrived: the child is ready for a swapchain.
    pub ready: bool,
    /// Working directory reported by the child (terminals, via OSC 7).
    pub pwd: Option<PathBuf>,
    /// Opened as a Quick-Look preview: a centered float that Escape or
    /// Space dismisses.
    pub is_preview: bool,
    /// A DORMANT warm-pool instance (see `WarmPool`): the process is up,
    /// connected and drawing into its own off-desk framebuffer, but it has
    /// NO tile. Everything the desk enumerates works off the LAYOUT, which
    /// a warm client is never in, so this flag is only needed where the WM
    /// walks the client table itself — launch-or-focus matching, and the
    /// tile plumbing that must stay away until adoption.
    pub warm: bool,
    /// When this client was opened as a real window (launched cold, or
    /// adopted out of the pool) and whether that open was the warm path —
    /// the pair behind the "first frame in Nms" log line that measures the
    /// pool honestly.
    pub open_at: Option<Instant>,
    pub opened_warm: bool,
    /// FOCUS RULE: a Quick-Look preview never takes key focus — keys keep
    /// flowing to the requesting tile (files). `focus_client` refuses to
    /// focus a client with this false; every normal client defaults true.
    pub takes_focus: bool,
    /// Launched through cargo, so the tile can say "building…" until the
    /// child actually connects.
    #[allow(dead_code)]
    pub via_cargo: bool,
    /// The newest line the child (or cargo) wrote, shown on the tile
    /// under "starting…" until the first frame arrives.
    pub status: String,
    /// cargo has finished linking and handed over: the child's first exec
    /// is the one macOS scans.
    pub linked: bool,
    pub linked_at: Option<std::time::Instant>,
    /// A polite close was sent at this instant (omarchy's
    /// `hl.dsp.window.close()`); the hard kill is only the fallback.
    pub closing: Option<std::time::Instant>,
}

/// How long a client gets to honor a close request before it is killed.
pub const CLOSE_GRACE: std::time::Duration = std::time::Duration::from_millis(1500);

/// SIGTERM-to-SIGKILL escalation gap inside `kill_child_group`, once a
/// caller has already decided to hard-kill (past `CLOSE_GRACE`, or the
/// client never got that far — still building when it was closed).
pub const GROUP_KILL_GRACE: std::time::Duration = std::time::Duration::from_millis(300);

/// Put `cmd`'s child at the head of a brand-new process group (unix only):
/// `process_group(0)` is `setpgid(0, 0)` before exec, so the pgid becomes
/// the child's own pid. Every process it forks (rustc, the app `cargo run`
/// execs into a further child) inherits that same pgid, so the whole tree
/// can be reached by one negative-pid signal later.
#[cfg(unix)]
fn own_process_group(cmd: &mut Command) {
    cmd.process_group(0);
}

/// `kill(2)` by hand — this crate has no `libc` dependency, and a
/// two-liner beats pulling one in for a single syscall pair.
#[cfg(unix)]
mod signal {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    pub const SIGTERM: i32 = 15;
    pub const SIGKILL: i32 = 9;

    /// Signal the whole process group led by `pid` — the POSIX convention
    /// of a negative pid.
    pub fn kill_group(pid: i32, sig: i32) {
        unsafe { kill(-pid, sig) };
    }

    /// `kill(pid, 0)` sends nothing; a zero return means the process (or
    /// group leader) still exists.
    pub fn alive(pid: i32) -> bool {
        unsafe { kill(pid, 0) == 0 }
    }
}

/// Kill the whole process group a `spawn_client` child heads — the fix for
/// the leak `Child::kill()` had: through cargo, that call only ever reached
/// cargo itself, leaving the exec'd app (and any still-building rustc)
/// running as orphans. SIGTERM now, SIGKILL after `grace` for whatever is
/// still alive; the escalation runs off-thread so a UI-thread caller never
/// blocks on it. Windows keeps the plain `Child::kill()` this replaced.
#[cfg(unix)]
pub fn kill_child_group(child: &mut Child, grace: std::time::Duration) {
    let pid = child.id() as i32;
    signal::kill_group(pid, signal::SIGTERM);
    std::thread::spawn(move || {
        std::thread::sleep(grace);
        if signal::alive(pid) {
            signal::kill_group(pid, signal::SIGKILL);
        }
    });
}

#[cfg(not(unix))]
pub fn kill_child_group(child: &mut Child, _grace: std::time::Duration) {
    let _ = child.kill();
}

impl ClientSlot {
    pub fn display_title(&self) -> &str {
        if self.title.is_empty() {
            &self.app
        } else {
            &self.title
        }
    }
}

/// One line of a child's output, on its way to the tile.
#[derive(Clone, Debug)]
pub struct ClientLine {
    pub client: ClientId,
    pub text: String,
}

/// Cargo (and rustc) paint their progress; a pipe usually turns that off,
/// but a stray CSI sequence must never reach a Label.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            if c != '\r' {
                out.push(c);
            }
            continue;
        }
        // ESC [ … <final byte 0x40..0x7e>, or ESC ] … BEL (OSC).
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                for c in chars.by_ref() {
                    if c == '\u{7}' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Read a child stream line by line into the log file and the UI channel.
fn pump<R: std::io::Read + Send + 'static>(
    client: ClientId,
    stream: R,
    mut log: Option<std::fs::File>,
    lines: Sender<ClientLine>,
) {
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader, Write};
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if let Some(file) = log.as_mut() {
                let _ = writeln!(file, "{}", line);
            }
            let text = strip_ansi(&line).trim().to_string();
            if text.is_empty() {
                continue;
            }
            if lines.send(ClientLine { client, text }).is_err() {
                break;
            }
            SignalToUI::set_ui_signal();
        }
    });
}

/// The command line a launch runs, split out so the release-only law is
/// testable: children are ALWAYS `--release`, never debug.
pub fn launch_argv(
    app: &AppDef,
    root: Option<&Path>,
    extra_args: &[String],
) -> Result<(PathBuf, Vec<String>), String> {
    let mut args: Vec<String> = Vec::new();
    let program = match root {
        Some(root) => {
            // `cargo run --release` so a stale binary is rebuilt on launch.
            // --manifest-path keeps it working whatever the cwd ends up
            // being (the terminal opens in the focused shell's directory).
            let manifest = app
                .manifest
                .clone()
                .unwrap_or_else(|| "Cargo.toml".to_string());
            args.push("run".to_string());
            args.push("--release".to_string());
            args.push("--manifest-path".to_string());
            args.push(root.join(manifest).to_string_lossy().to_string());
            args.push("-p".to_string());
            args.push(app.package.clone());
            args.push("--".to_string());
            cargo_bin()
        }
        None => resolve_bin(&app.bin).ok_or_else(|| format!("binary not found: {}", app.bin))?,
    };
    args.push("--stdin-loop".to_string());
    args.extend(app.args.iter().cloned());
    args.extend(extra_args.iter().cloned());
    Ok((program, args))
}

/// Spawn an app as a hub client.
pub fn spawn_client(
    app: &AppDef,
    id: ClientId,
    hub_port: u16,
    cwd: Option<&PathBuf>,
    term_colors: Option<&str>,
    // `extra_args` is appended after the app's own args: the file to open,
    // with `--preview` in front of it for a Quick-Look popup.
    extra_args: &[String],
    // A DORMANT warm-pool instance: same launch in every other way — same
    // cargo, same env, same log — plus `WARM_ENV`, which tells the app to
    // come up and then idle until it is adopted.
    warm: bool,
    // Every output line the child writes is forwarded here, so the tile
    // can show cargo's progress instead of a bare "starting…".
    lines: Sender<ClientLine>,
) -> Result<ClientSlot, String> {
    let root = repo_root();
    let via_cargo = root.is_some();
    let (program, args) = launch_argv(app, root.as_deref(), extra_args)?;
    let mut cmd = Command::new(program);
    cmd.args(&args);
    // Give the process its own group (unix): `cargo run` does not
    // exec-replace itself, so the compiled app (and, mid-build, rustc) are
    // further children of the process we hold, not exec'd into it. Sharing
    // one fresh pgid lets `kill_child_group` reach the whole tree.
    #[cfg(unix)]
    own_process_group(&mut cmd);
    cmd.env("STUDIO_HOST", format!("http://127.0.0.1:{}", hub_port))
        .env("STUDIO_BUILD", id.to_string())
        .env("STUDIO_CRATE", &app.bin)
        .stdin(Stdio::null())
        // Both streams are piped so a reader thread can put the newest
        // line on the tile while the app builds — cargo talks on stderr.
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Cargo colors its output when it thinks a terminal is watching; the
    // pipe already turns that off, and this makes it certain.
    cmd.env("CARGO_TERM_COLOR", "never");
    if let Some(cwd) = cwd {
        // The terminal's Omarchy behavior: open where the focused one is.
        cmd.arg("--cwd").arg(cwd);
        cmd.current_dir(cwd);
    } else if let Some(root) = &root {
        // Apps resolve their data (route's local/maps/, resources)
        // relative to the checkout root, like a `cargo run` from the repo.
        cmd.current_dir(root);
    }
    if let Some(colors) = term_colors {
        cmd.env("MAKEPAD_TERMINAL_COLORS", colors);
        // Truly translucent terminals over the wallpaper (the user's
        // default; omarchy gets this from ghostty background-opacity —
        // its window rule alone, 0.985/0.96, reads as opaque).
        // "focused unfocused"; MAKEPAD_WM_TERM_OPACITY overrides.
        let opacity = std::env::var("MAKEPAD_WM_TERM_OPACITY")
            .unwrap_or_else(|_| "0.88 0.84".to_string());
        cmd.env("MAKEPAD_TERMINAL_OPACITY", opacity);
    }
    // Every Makepad app styles itself from the WM's theme.splash.
    if let Ok(theme) = std::env::var("MAKEPAD_WM_THEME_SPLASH") {
        cmd.env("MAKEPAD_WM_THEME_SPLASH", theme);
    }
    if warm {
        cmd.env(WARM_ENV.0, WARM_ENV.1);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn {}: {}", app.package, e))?;
    // Child output (and cargo's "Compiling …") goes to a per-client log —
    // silent children are undebuggable — and every line also reaches the
    // UI so the tile can show what the build is doing.
    let log_path = std::env::temp_dir().join(format!("wm-client-{}.log", id));
    let log = std::fs::File::create(&log_path).ok();
    if let Some(out) = child.stdout.take() {
        pump(id, out, log.as_ref().and_then(|f| f.try_clone().ok()), lines.clone());
    }
    if let Some(err) = child.stderr.take() {
        pump(id, err, log, lines);
    }
    Ok(ClientSlot {
        id,
        app: app.id.to_string(),
        title: String::new(),
        child: Some(child),
        sender: None,
        socket: None,
        window_id: 0,
        ready: false,
        pwd: None,
        is_preview: false,
        warm,
        open_at: (!warm).then(Instant::now),
        opened_warm: false,
        // A warm instance is not a window yet: nothing may focus it until
        // adoption hands it a tile.
        takes_focus: !warm,
        via_cargo,
        status: String::new(),
        linked: false,
        linked_at: None,
        closing: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn children_are_always_release_never_debug() {
        // USER LAW: a hosted app is a release build, always.
        let app = curated().into_iter().find(|a| a.id == "terminal").unwrap();
        let root = std::path::PathBuf::from("/checkout");
        let (program, args) = launch_argv(&app, Some(&root), &[]).unwrap();
        assert!(program.to_string_lossy().ends_with("cargo"), "{:?}", program);
        let sep = args.iter().position(|a| a == "--").expect("no -- separator");
        let release = args
            .iter()
            .position(|a| a == "--release")
            .expect("no --release");
        assert!(release < sep, "--release must be a cargo flag: {:?}", args);
        assert_eq!(args[0], "run");
        assert_eq!(
            args[sep + 1],
            "--stdin-loop",
            "the app's own args start after the separator: {:?}",
            args
        );
        assert!(args.contains(&"/checkout/Cargo.toml".to_string()), "{:?}", args);
        assert!(args.contains(&"makepad-terminal".to_string()), "{:?}", args);
        // A preview's file lands after --stdin-loop, still past the --.
        let (_, args) = launch_argv(
            &app,
            Some(&root),
            &["--preview".to_string(), "/a.png".to_string()],
        )
        .unwrap();
        assert_eq!(&args[args.len() - 2..], &["--preview", "/a.png"]);
    }

    #[test]
    fn the_installed_fallback_execs_the_sibling_binary() {
        // No checkout: run the binary next to the running wm, which for
        // a release wm is target/release/<bin>.
        let app = curated().into_iter().find(|a| a.id == "terminal").unwrap();
        match launch_argv(&app, None, &[]) {
            Ok((program, args)) => {
                let exe = std::env::current_exe().unwrap();
                assert_eq!(program.parent(), exe.parent());
                assert_eq!(program.file_name().unwrap(), "terminal");
                assert_eq!(args, vec!["--stdin-loop".to_string()]);
            }
            // The test binary does not sit next to terminal; the law that
            // matters is that it resolves a SIBLING or fails, never cargo.
            Err(e) => assert!(e.contains("binary not found"), "{}", e),
        }
    }

    #[test]
    fn launch_or_focus_matches_whole_words_either_side() {
        // `\bfiles\b`, case-insensitive, over class OR title.
        assert!(word_match("files", "files"));
        assert!(word_match("Files", "files"));
        assert!(word_match("~/Pictures — files", "FILES"));
        assert!(word_match("makepad-files - files (2)", "files"));
        // Not a word boundary: no match.
        assert!(!word_match("makepadfiles", "files"));
        assert!(!word_match("filesystem", "files"));
        assert!(!word_match("", "files"));
        assert!(!word_match("files", ""));
        // A dot/dash counts as a boundary, like the regex \b.
        assert!(word_match("org.omarchy.btop", "btop"));
        assert!(word_match("btop-tui", "btop"));
    }

    #[test]
    fn the_curated_list_is_in_the_order_it_is_shown_in() {
        // The menu IS this list, top to bottom.
        let order: Vec<String> = curated().iter().map(|a| a.label.clone()).collect();
        assert_eq!(
            order,
            [
                "Browser",
                "Files",
                "Terminal",
                "Mixer",
                "Task Manager",
                "Sheets",
                "Score",
                "Video Player",
                "Route",
                "VJ",
                "Fab",
                "Studio",
            ]
            .map(str::to_string)
        );
    }

    #[test]
    fn the_curated_list_is_distinct_and_honest() {
        let apps = curated();
        let mut ids: Vec<&str> = apps.iter().map(|a| a.id.as_str()).collect();
        ids.sort_unstable();
        let len = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), len, "duplicate registry ids");
        // The terminal and the per-file viewers are the always-new ones.
        // The delisted viewers stay launchable through find_app.
        for id in ["image", "pdf"] {
            let app = find_app(id).expect(id);
            assert_eq!(app.policy, LaunchPolicy::AlwaysNew, "{}", id);
        }
        for (id, policy) in [
            ("terminal", LaunchPolicy::AlwaysNew),
            ("video", LaunchPolicy::AlwaysNew),
            ("vj", LaunchPolicy::OrFocus),
            ("fab", LaunchPolicy::OrFocus),
            ("studio", LaunchPolicy::OrFocus),
        ] {
            let app = apps.iter().find(|a| a.id == id).expect(id);
            assert_eq!(app.policy, policy, "{}", id);
            assert!(!app.package.is_empty() && !app.dir.is_empty());
        }
    }

    #[test]
    fn the_association_table_resolves_by_binary_name() {
        // `makepad_wm_api::viewer_for` names binaries (standalone apps spawn
        // them as siblings); the WM must resolve those to curated entries
        // or every text/html/csv preview dies with "no app".
        for (bin, id) in [
            ("terminal", "terminal"),
            ("browser", "browser"),
            ("sheets", "sheets"),
        ] {
            assert_eq!(find_app(bin).expect(bin).id, id);
        }
        // Registry ids still win over bin names.
        assert_eq!(find_app("terminal").expect("terminal").id, "terminal");
    }

    #[test]
    fn every_curated_app_names_a_real_crate() {
        // The no-fake-UI law: a menu row must be startable. In a checkout
        // that means the package directory really is there.
        let Some(root) = repo_root() else {
            return; // installed layout: nothing to check against
        };
        for app in curated() {
            let manifest = app
                .manifest
                .clone()
                .unwrap_or_else(|| format!("{}/Cargo.toml", app.dir));
            let path = root.join(&manifest);
            if !path.exists() {
                // Optional private clones (sandbox) may be absent; they are
                // filtered out of the menu by is_available().
                assert!(!app.is_available(), "{} claims to be available", app.id);
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            assert_eq!(
                manifest_value(&text, "name").as_deref(),
                Some(app.package.as_str()),
                "{} points at the wrong package",
                app.id
            );
            assert!(app.is_available(), "{} should be available", app.id);
        }
    }

    // ------------------------------------------------------------------
    // The warm pool
    // ------------------------------------------------------------------

    /// A pool holding `ids` for `app`, every one of them live and ready.
    fn pool_with(app: &str, ids: &[ClientId]) -> (WarmPool, Vec<WarmStatus>) {
        let mut pool = WarmPool::new(true);
        for id in ids {
            pool.note_spawned(app, *id);
        }
        let status = ids
            .iter()
            .map(|id| WarmStatus {
                client: *id,
                alive: true,
                connected: true,
            })
            .collect();
        (pool, status)
    }

    #[test]
    fn the_pool_sizes_are_the_users_two_terminals_and_one_of_the_rest() {
        assert_eq!(WarmPool::capacity("terminal"), 2);
        for app in ["browser", "files", "task"] {
            assert_eq!(WarmPool::capacity(app), 1, "{}", app);
        }
        // Everything else launches cold, as it always did.
        for app in ["vj", "fab", "studio", "image", "nonesuch"] {
            assert_eq!(WarmPool::capacity(app), 0, "{}", app);
            assert!(!WarmPool::is_warm_app(app), "{}", app);
        }
        // Every pooled app is a real registry entry we can actually spawn.
        for (app, _) in WARM_CAPACITY {
            assert!(find_app(app).is_some(), "{} is not in the registry", app);
        }
    }

    #[test]
    fn adopting_clears_the_slot_and_asks_for_a_respawn() {
        let (mut pool, status) = pool_with("browser", &[7]);
        // Fill the rest of the shelf, so a full pool asks for nothing and
        // the top-up below names exactly the app that was adopted.
        pool.note_spawned("terminal", 1);
        pool.note_spawned("terminal", 2);
        pool.note_spawned("files", 3);
        pool.note_spawned("task", 4);
        assert!(!pool.wants("browser", Instant::now()), "already full");
        assert_eq!(pool.next_missing(Instant::now()), None, "nothing missing");
        assert_eq!(pool.adopt("browser", false, &status), Some(7));
        // Out of the pool, and the pool now wants its replacement.
        assert_eq!(pool.held("browser"), 0);
        assert!(!pool.holds(7));
        assert!(pool.wants("browser", Instant::now()));
        assert_eq!(pool.next_missing(Instant::now()).as_deref(), Some("browser"));
        // The same instance can never be adopted twice.
        assert_eq!(pool.adopt("browser", false, &status), None);
    }

    #[test]
    fn two_terminals_stand_by_and_both_open_instantly() {
        let (mut pool, status) = pool_with("terminal", &[3, 4]);
        assert_eq!(pool.held("terminal"), 2);
        assert!(!pool.wants("terminal", Instant::now()));
        // Back-to-back opens: both are swaps, oldest first.
        assert_eq!(pool.adopt("terminal", false, &status), Some(3));
        assert_eq!(pool.held("terminal"), 1);
        assert_eq!(pool.adopt("terminal", false, &status), Some(4));
        assert_eq!(pool.held("terminal"), 0);
        // A third open in the same breath falls back to cold, and the pool
        // is two short — one spawn per tick, so it tops up twice.
        assert_eq!(pool.adopt("terminal", false, &status), None);
        assert!(pool.wants("terminal", Instant::now()));
        pool.note_spawned("terminal", 9);
        assert!(pool.wants("terminal", Instant::now()));
        pool.note_spawned("terminal", 10);
        assert!(!pool.wants("terminal", Instant::now()));
        assert_eq!(pool.next_missing(Instant::now()).as_deref(), Some("browser"));
    }

    #[test]
    fn a_dead_or_unconnected_warm_instance_falls_back_to_a_cold_spawn() {
        // Killed behind our back: not in the client table any more.
        let (mut pool, _) = pool_with("terminal", &[3, 4]);
        let gone = [
            WarmStatus { client: 3, alive: false, connected: false },
            WarmStatus { client: 4, alive: true, connected: true },
        ];
        assert_eq!(pool.adopt("terminal", false, &gone), Some(4));
        // The dead one was pruned on the way past, so the pool asks for
        // two replacements rather than counting a corpse.
        assert_eq!(pool.held("terminal"), 0);

        // Still building / still starting: alive but not connected. No
        // adoption (there is no frame to show), and it KEEPS its slot —
        // it will be ready for the next launch.
        let (mut pool, _) = pool_with("browser", &[5]);
        let starting = [WarmStatus { client: 5, alive: true, connected: false }];
        assert_eq!(pool.adopt("browser", false, &starting), None);
        assert_eq!(pool.held("browser"), 1);
        assert!(!pool.wants("browser", Instant::now()));

        // An app with nothing standing by: cold, quietly.
        let mut empty = WarmPool::new(true);
        assert_eq!(empty.adopt("terminal", false, &[]), None);
    }

    #[test]
    fn a_cwd_override_skips_adoption_and_keeps_the_instance() {
        // THE CARVE-OUT: a new terminal must open in the focused
        // terminal's cwd, and the warm shell already started elsewhere.
        let (mut pool, status) = pool_with("terminal", &[3, 4]);
        assert_eq!(pool.adopt("terminal", true, &status), None);
        // Nothing was consumed: the next launch WITHOUT an override is
        // still instant.
        assert_eq!(pool.held("terminal"), 2);
        assert_eq!(pool.adopt("terminal", false, &status), Some(3));
    }

    #[test]
    fn wm_no_warm_turns_the_pool_off_entirely() {
        assert!(warm_enabled(None));
        // An empty or 0 value is not a request.
        assert!(warm_enabled(Some("")));
        assert!(warm_enabled(Some("0")));
        assert!(!warm_enabled(Some("1")));
        assert!(!warm_enabled(Some("yes")));

        let mut off = WarmPool::new(false);
        assert!(!off.enabled());
        // Nothing is ever spawned…
        assert!(!off.wants("terminal", Instant::now()));
        assert_eq!(off.next_missing(Instant::now()), None);
        // …and even a hand-fed instance is never adopted.
        off.note_spawned("terminal", 1);
        let status = [WarmStatus { client: 1, alive: true, connected: true }];
        assert_eq!(off.adopt("terminal", false, &status), None);
    }

    #[test]
    fn a_crash_loop_gives_up_quietly_after_three_a_minute() {
        let now = Instant::now();
        let mut pool = WarmPool::new(true);
        for i in 0..WARM_CRASH_LIMIT {
            assert!(pool.wants("browser", now), "attempt {}", i);
            pool.note_spawned("browser", i as ClientId);
            // Up, then dead before anyone could adopt it.
            let app = pool.forget(i as ClientId).expect("pooled");
            pool.note_crash(&app, now);
        }
        assert!(!pool.wants("browser", now), "the budget should be spent");
        assert_eq!(pool.next_missing(now).as_deref(), Some("terminal"));
        // The budget is per app…
        assert!(pool.wants("terminal", now));
        // …and it is a WINDOW: a minute later the app is tried again.
        assert!(pool.wants("browser", now + WARM_CRASH_WINDOW + Duration::from_secs(1)));
    }

    #[test]
    fn a_deliberate_close_costs_no_budget_and_tops_back_up() {
        // CTRL+ALT+DELETE closes the warm instances with everything else;
        // that is not a crash, so the pool refills at once instead of
        // spending the loop budget on the user's own gesture.
        let now = Instant::now();
        let mut pool = WarmPool::new(true);
        for id in 0..6 {
            pool.note_spawned("terminal", id);
            assert_eq!(pool.forget(id).as_deref(), Some("terminal"));
        }
        assert!(pool.wants("terminal", now));
        assert_eq!(pool.forget(99), None, "an unknown client is not ours");
    }

    #[test]
    fn every_warm_client_is_reachable_for_shutdown() {
        let mut pool = WarmPool::new(true);
        pool.note_spawned("terminal", 3);
        pool.note_spawned("terminal", 4);
        pool.note_spawned("browser", 1);
        assert_eq!(pool.clients(), vec![1, 3, 4]);
        assert!(pool.holds(4) && !pool.holds(5));
    }

    #[test]
    fn a_warm_instance_launches_exactly_like_a_cold_one() {
        // Same argv — the registry's own args included, which is how the
        // warm Files inherits `--demo` without the pool knowing about it.
        let files = find_app("files").unwrap();
        let root = std::path::PathBuf::from("/checkout");
        let (program, args) = launch_argv(&files, Some(&root), &[]).unwrap();
        assert!(program.to_string_lossy().ends_with("cargo"));
        assert!(args.contains(&"makepad-files".to_string()), "{:?}", args);
        if std::env::var("MAKEPAD_WM_FILES_REAL").is_err() {
            assert_eq!(args.last().map(String::as_str), Some("--demo"), "{:?}", args);
        }
    }

    #[test]
    fn the_warm_env_is_the_one_the_apps_read() {
        // The contract with `makepad_wm_api::warm_start()`: a dormant app idles
        // (no samplers, no refresh) until `WmEvent::Adopted`. If these two
        // ever drift, a warm task manager silently burns a core.
        assert!(!makepad_wm_api::warm_start(), "MAKEPAD_WM_WARM_START leaked in");
        std::env::set_var(WARM_ENV.0, WARM_ENV.1);
        assert!(makepad_wm_api::warm_start());
        std::env::remove_var(WARM_ENV.0);
        assert!(!makepad_wm_api::warm_start());
    }

    #[test]
    fn manifest_values_come_from_the_package_table_only() {
        let toml = "[package]\nname = \"a\"\n\n[dependencies]\nname = \"b\"\n";
        assert_eq!(manifest_value(toml, "name").as_deref(), Some("a"));
    }

    /// `own_process_group` really does make the child its own group leader
    /// — the precondition `kill_child_group`'s negative-pid signal relies
    /// on. `getpgid` is one more syscall this crate has no `libc` for.
    #[cfg(unix)]
    #[test]
    fn spawned_children_lead_their_own_process_group() {
        extern "C" {
            fn getpgid(pid: i32) -> i32;
        }
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("sleep 5");
        own_process_group(&mut cmd);
        let mut child = cmd.spawn().expect("spawn /bin/sh");
        let pid = child.id() as i32;
        let pgid = unsafe { getpgid(pid) };
        assert_eq!(pgid, pid, "the child should lead its own new group");
        kill_child_group(&mut child, std::time::Duration::from_millis(50));
        let _ = child.wait();
    }

    /// The bug this fixes: a child launched through a wrapper (`cargo run`
    /// stands in for it here as any process that forks a grandchild rather
    /// than exec-replacing itself) leaks that grandchild when only the
    /// wrapper is killed. Reproduce it with a shell that backgrounds a
    /// `sleep` and prints its pid, then confirm `kill_child_group` reaps
    /// BOTH — the regression `Child::kill()` alone could not clear.
    #[cfg(unix)]
    #[test]
    fn killing_the_group_reaps_a_grandchild_the_wrapper_leaked() {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("sleep 30 & echo $!; wait");
        own_process_group(&mut cmd);
        cmd.stdout(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn /bin/sh");

        use std::io::BufRead;
        let stdout = child.stdout.take().expect("piped stdout");
        let mut reader = std::io::BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read grandchild pid");
        let grandchild_pid: i32 = line.trim().parse().expect("a pid line");

        // The grandchild is alive and NOT the pid we hold — it really is
        // one generation further down, like the app under `cargo run`.
        assert_ne!(grandchild_pid, child.id() as i32);
        assert_eq!(unsafe { kill(grandchild_pid, 0) }, 0, "grandchild not up yet");

        kill_child_group(&mut child, std::time::Duration::from_millis(50));
        // Past the SIGTERM->SIGKILL escalation: nothing in the group is
        // still standing, wrapper or grandchild.
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(
            unsafe { kill(grandchild_pid, 0) },
            -1,
            "the grandchild the wrapper orphaned should be gone too"
        );
        let _ = child.wait();
    }
}

impl Drop for ClientSlot {
    fn drop(&mut self) {
        // Through cargo the child we hold is `cargo run`, not the app, so
        // killing the handle would orphan the window. Ask the app to go
        // first over its own socket, then reap the wrapper.
        if let Some(sender) = self.sender.take() {
            crate::hub::send_to_app(
                &sender,
                vec![makepad_studio_protocol::StudioToApp::Kill],
            );
        }
        if let Some(mut child) = self.child.take() {
            kill_child_group(&mut child, GROUP_KILL_GRACE);
            let _ = child.wait();
        }
    }
}

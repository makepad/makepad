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
//! **wm compiles nothing behind the person's back.** A child is always an
//! existing binary: out of a checkout (a developer's clone, or the source
//! a Makepad Builder downloaded and starts wm in) the release binary
//! cargo built, else the sibling of an installed wm. The warm pool, the
//! AI pane at startup and previews start only apps that are built and up
//! to date, so no build of wm's own ever holds cargo's lock. The person
//! OPENING an app (a click, F10 for the pane) builds it when it is missing
//! or out of date: one hidden `cargo build`, its crate count on the tile,
//! and the app starts in that tile when the build is done.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;
use crate::host;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

use makepad_widgets::makepad_platform::thread::{Lane, SignalToUI, TaskPool, ThreadSpawner, ThreadOptions};
#[cfg(any(unix, test))]
use makepad_widgets::makepad_platform::thread::CancellationToken;
#[cfg(any(unix, test))]
use makepad_widgets::Cx;

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
    /// Cargo package name — what `cargo build -p` gets.
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

    /// Its binary exists: starting it runs it, no compile.
    pub fn is_built(&self) -> bool {
        built_binary(self, repo_root().as_deref()).is_some()
    }

    /// When its binary was built (None: not built).
    pub fn built_at(&self) -> Option<std::time::SystemTime> {
        std::fs::metadata(built_binary(self, repo_root().as_deref())?).and_then(|m| m.modified()).ok()
    }

    /// Its binary exists and is up to date: out of a checkout, newer than
    /// every source file cargo's dep-info lists for it; an installed binary
    /// is what it is. Stats every source of the app (a thousand files or
    /// so): asked on an open and before a warm spawn, never per frame.
    pub fn is_current(&self) -> bool {
        let root = repo_root();
        match built_binary(self, root.as_deref()) {
            Some(binary) if root.is_some() => binary_is_current(&binary),
            Some(_) => true,
            None => false,
        }
    }

    /// True when this app can actually be started right now — the honest
    /// filter behind the menu (no row that cannot run). Out of a checkout
    /// that includes a deck app that is not built yet: opening it builds it.
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
        // The picture wall over a baked library.
        AppDef::app("photos", "Photos", "makepad-photos", "apps/photos", "photos", OrFocus),
        AppDef::app("clock", "Clock", "makepad-clock", "apps/clock", "clock", OrFocus),
        AppDef::app("weather", "Weather", "makepad-weather", "apps/weather", "weather", OrFocus),
        // The ledger: accounts, imports and charts over its own database.
        AppDef::app("finance", "Finance", "makepad-finance", "apps/finance", "finance", OrFocus),
        AppDef::app("mail", "Mail", "makepad-mail", "apps/mail", "mail", OrFocus),
        AppDef::app("notes", "Notes", "makepad-notes", "apps/notes", "notes", OrFocus),
        AppDef::app("calendar", "Calendar", "makepad-calendar", "apps/calendar", "calendar", OrFocus),
        AppDef::app("reminders", "Reminders", "makepad-reminders", "apps/reminders", "reminders", OrFocus),
        AppDef::app("calculator", "Calculator", "makepad-calculator", "apps/calculator", "calculator", OrFocus),
        // Sewing patterns from a body measurement: camera, body model, PDF/SVG.
        AppDef::app("fabric", "Fabric", "makepad-fabric", "apps/fabric", "makepad-fabric", OrFocus),
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
            "apps/studio",
            "studio",
            OrFocus,
        ),
        // Scope is an optional private checkout; cloned into apps/scope it is
        // a member of this workspace and builds into its target/.
        AppDef::app("scope", "Scope", "makepad-scope", "apps/scope", "scope", OrFocus),
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
        // The assistant: a special child seated in the pane slot, never a
        // menu row or a tile (see ai_bus.rs / shell/ai_pane.rs). The WM
        // launches it through its own pane path, never `launch_app`, so
        // the policy is moot — AlwaysNew keeps launch-or-focus's window
        // scan from ever "focusing" a pane.
        "aichat" => Some(AppDef::app(
            "aichat",
            "AI",
            "makepad-aichat",
            "apps/aichat",
            "aichat",
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

/// The checkout root: `MAKEPAD_WM_ROOT`, else the checkout above the
/// running exe (`target/<profile>/wm`), else the checkout at or above the
/// current directory — a wm started from the repo root with its target
/// dir elsewhere (CARGO_TARGET_DIR) is still running out of a checkout,
/// and every app of the deck is one `cargo build` away.
pub fn repo_root() -> Option<PathBuf> {
    if let Ok(root) = std::env::var("MAKEPAD_WM_ROOT") {
        return Some(PathBuf::from(root));
    }
    // A wm the Makepad Builder published: its sources are in the Builder's
    // folder, whether the Builder started it (in them) or it was opened on
    // its own (wm.exe, the Dock, the wm command).
    if let Some(install) = builder_install() {
        return install.checkout();
    }
    let from_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .and_then(|dir| checkout_at_or_above(&dir));
    from_exe.or_else(|| std::env::current_dir().ok().and_then(|cwd| checkout_at_or_above(&cwd)))
}

/// The Makepad Builder installation a published wm runs from: `home` is the
/// folder people see (makepad-builder.exe or the `makepad` command, and the
/// apps), `state` the Builder's own `builder/` folder in it (sources,
/// toolchains, the Unix `<app>.bin`s). Found from the executable: Windows
/// `home/wm.exe`, Unix `builder/wm.bin`, a macOS bundle's `installation`
/// link. Apps build through the Builder there (`build_argv`), so they get its
/// compiler, flags, features and target however wm was started.
pub struct BuilderInstall {
    pub home: PathBuf,
    pub state: PathBuf,
}

impl BuilderInstall {
    /// The Builder's command that builds one app offline.
    fn command(&self) -> Option<PathBuf> {
        let command = if cfg!(windows) { self.home.join("makepad-builder.exe") } else { self.home.join("makepad") };
        command.is_file().then_some(command)
    }
    /// Where the Builder publishes `bin`.
    fn published(&self, bin: &str) -> PathBuf {
        if cfg!(windows) {
            self.home.join(format!("{bin}.exe"))
        } else {
            self.state.join(format!("{bin}.bin"))
        }
    }
    /// The Makepad source wm builds from: the snapshot the working directory
    /// is in (the Builder starts wm there), else the newest one with wm in it.
    fn checkout(&self) -> Option<PathBuf> {
        let sources = self.state.join("sources");
        if let Some(cwd) = std::env::current_dir().ok().and_then(|cwd| checkout_at_or_above(&cwd)) {
            if cwd.starts_with(&sources) {
                return Some(cwd);
            }
        }
        std::fs::read_dir(&sources).ok()?.flatten()
            .map(|snapshot| snapshot.path().join("makepad"))
            .filter(|root| root.join("apps/wm/Cargo.toml").is_file())
            .max_by_key(|root| std::fs::metadata(root.join("Cargo.toml")).and_then(|m| m.modified()).ok())
    }
}

pub fn builder_install() -> Option<BuilderInstall> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    for state in [dir.join("builder"), dir.to_path_buf(), dir.join("installation")] {
        // Its downloaded sources and its compiler (the email record is
        // missing in a folder nobody logged into).
        if state.join("sources").is_dir() && state.join("toolchain").is_dir() {
            let state = state.canonicalize().unwrap_or(state);
            // An installation from before the builder/ folder keeps
            // everything in the folder itself.
            let home = if state.file_name().is_some_and(|name| name == "builder") {
                state.parent()?.to_path_buf()
            } else {
                state.clone()
            };
            return Some(BuilderInstall { home, state });
        }
    }
    None
}

/// The nearest directory at or above `start` (four levels at most) that is
/// a makepad checkout: the workspace `Cargo.toml` with this wm's own crate
/// in it. A developer's clone and the source a Makepad Builder downloaded
/// (which starts wm in it, with its compiler environment) both are; the
/// Builder's has no `local/`.
fn checkout_at_or_above(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    for _ in 0..5 {
        if dir.join("Cargo.toml").exists() && dir.join("apps/wm/Cargo.toml").exists() {
            return Some(dir);
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

/// The release binary a checkout's cargo builds for `app`: under
/// `CARGO_TARGET_DIR` when set (the Makepad Builder points it at its one
/// shared target), else the workspace's own `target/`.
fn checkout_binary(app: &AppDef, root: &Path, target_dir: Option<&std::ffi::OsStr>) -> PathBuf {
    let workspace = app
        .manifest
        .as_ref()
        .and_then(|manifest| root.join(manifest).parent().map(Path::to_path_buf))
        .unwrap_or_else(|| root.to_path_buf());
    // A relative target dir is cargo's, relative to where it runs: here.
    let target = target_dir
        .map(|dir| workspace.join(dir))
        .unwrap_or_else(|| workspace.join("target"));
    let mut path = target.join("release").join(&app.bin);
    if cfg!(windows) {
        path.set_extension("exe");
    }
    path
}

/// The binary that starts `app` without compiling anything: the checkout's
/// release build when wm runs out of a checkout, else the sibling of an
/// installed wm. None: not built.
pub fn built_binary(app: &AppDef, root: Option<&Path>) -> Option<PathBuf> {
    if let Some(install) = builder_install() {
        return Some(install.published(&app.bin)).filter(|path| path.is_file());
    }
    match root {
        Some(root) => {
            let target_dir = std::env::var_os("CARGO_TARGET_DIR");
            Some(checkout_binary(app, root, target_dir.as_deref())).filter(|path| path.is_file())
        }
        None => resolve_bin(&app.bin),
    }
}

/// Newer than every file in the dep-info cargo writes beside it
/// (`target/release/<bin>.d`: "<binary>: <source> <source> …", spaces in
/// paths escaped as `\ `). No dep-info, or a source gone or newer: stale.
fn binary_is_current(binary: &Path) -> bool {
    let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let (Some(built), Ok(deps)) = (modified(binary), std::fs::read_to_string(binary.with_extension("d"))) else {
        return false;
    };
    let Some((_, sources)) = deps.lines().next().and_then(|line| line.split_once(": ")) else {
        return false;
    };
    let current = dep_info_paths(sources).all(|source| modified(Path::new(&source)).is_some_and(|at| at <= built));
    current
}

/// The paths of a dep-info rule's right-hand side.
fn dep_info_paths(sources: &str) -> impl Iterator<Item = String> + '_ {
    let mut rest = sources.trim();
    std::iter::from_fn(move || {
        rest = rest.trim_start();
        if rest.is_empty() {
            return None;
        }
        let bytes = rest.as_bytes();
        let end = (0..bytes.len()).find(|&i| bytes[i] == b' ' && (i == 0 || bytes[i - 1] != b'\\')).unwrap_or(bytes.len());
        let path = rest[..end].replace("\\ ", " ");
        rest = &rest[end..];
        Some(path)
    })
}

/// Resolve a sibling binary of the running wm executable (`.exe` on
/// Windows, where a bare name never exists).
pub fn resolve_bin(bin: &str) -> Option<PathBuf> {
    // Android: every app is a library the launcher runs (host.rs).
    #[cfg(target_os = "android")]
    {
        crate::host::android_app_binary(bin).map(|(launcher, _)| launcher)
    }
    #[cfg(not(target_os = "android"))]
    {
        let exe = std::env::current_exe().ok()?;
        let dir = exe.parent()?;
        let mut path = dir.join(bin);
        if cfg!(windows) {
            path.set_extension("exe");
        }
        path.exists().then_some(path)
    }
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
pub const WARM_CRASH_WINDOW: f64 = 60.0;

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
    /// Appearance in which browser pages were warmed. A loaded page may
    /// choose its theme only once, so a media-query update is not sufficient.
    browser_dark: Option<bool>,
    /// app id -> the warm clients of that app, oldest first.
    ready: HashMap<String, Vec<ClientId>>,
    /// app id -> when (platform seconds) its warm instances died unexpectedly, newest last.
    crashes: HashMap<String, Vec<f64>>,
    /// app id -> the build time of a binary found out of date: not warmed
    /// (warming never compiles) until a new build replaces it.
    stale: HashMap<String, std::time::SystemTime>,
}

impl Default for WarmPool {
    /// Before the build is read: the platform's capability alone. The
    /// startup replaces it with `from_env(App::processes())`.
    fn default() -> Self {
        Self::from_env(host::processes_available())
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
    /// `processes` is the host's answer (`App::processes`): a build without
    /// processes has nothing to keep warm.
    pub fn from_env(processes: bool) -> Self {
        Self::new(processes && warm_enabled(std::env::var("MAKEPAD_WM_NO_WARM").ok().as_deref()))
    }

    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            browser_dark: None,
            ready: HashMap::new(),
            crashes: HashMap::new(),
            stale: HashMap::new(),
        }
    }

    /// `app` may be warmed: built and up to date. An out-of-date binary is
    /// remembered, so the check (a stat of every source) runs once per
    /// build, not on every tick that tops the pool up.
    pub fn warmable(&mut self, app: &AppDef) -> bool {
        let Some(at) = app.built_at() else { return false };
        if self.known_stale(app) {
            return false;
        }
        if app.is_current() {
            return true;
        }
        self.stale.insert(app.id.clone(), at);
        false
    }

    /// Found out of date before, and not rebuilt since.
    pub fn known_stale(&self, app: &AppDef) -> bool {
        app.built_at().is_some_and(|at| self.stale.get(&app.id) == Some(&at))
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

    /// Retire only unused browsers on a light/dark change. Removing them
    /// from the adoption pool is immediate; the host closes their processes
    /// and refills after they exit. Deliberate retirement is not a crash.
    pub fn set_browser_appearance(&mut self, dark: bool) -> Vec<ClientId> {
        let previous = self.browser_dark.replace(dark);
        if previous.is_some_and(|previous| previous != dark) {
            self.ready.remove("browser").unwrap_or_default()
        } else {
            Vec::new()
        }
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
    pub fn wants(&self, app: &str, now: f64) -> bool {
        self.enabled
            && self.held(app) < Self::capacity(app)
            && self.recent_crashes(app, now) < WARM_CRASH_LIMIT
    }

    /// The next app that is short an instance, in table order — the tick
    /// tops the pool up ONE spawn at a time. Only an app that is `built`
    /// (and not known out of date): warming never compiles, an app that is
    /// not built stays cold until the person opens it.
    pub fn next_missing(&self, now: f64, built: impl Fn(&str) -> bool) -> Option<String> {
        WARM_CAPACITY
            .iter()
            .map(|(app, _)| *app)
            .find(|app| self.wants(app, now) && built(app))
            .map(str::to_string)
    }

    /// A standby instance was spawned for `app`.
    pub fn note_spawned(&mut self, app: &str, client: ClientId) {
        self.ready.entry(app.to_string()).or_default().push(client);
    }

    /// A warm instance died on its own. Counted against the crash budget;
    /// a DELIBERATE close (WM shutdown, close-all) calls `forget` instead.
    pub fn note_crash(&mut self, app: &str, now: f64) {
        self.crashes.entry(app.to_string()).or_default().push(now);
    }

    fn recent_crashes(&self, app: &str, now: f64) -> usize {
        self.crashes
            .get(app)
            .map(|v| {
                v.iter()
                    .filter(|t| now - **t < WARM_CRASH_WINDOW)
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
    /// The app's own background (a module's `theme.color_bg_app`): what
    /// the host clears the app's texture to, as the app's own window would.
    pub ground: Option<makepad_widgets::Vec4f>,
    pub child: Option<Child>,
    task_pool: Option<TaskPool>,
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
    pub open_at: Option<f64>,
    pub opened_warm: bool,
    /// FOCUS RULE: a Quick-Look preview never takes key focus — keys keep
    /// flowing to the requesting tile (files). `focus_client` refuses to
    /// focus a client with this false; every normal client defaults true.
    pub takes_focus: bool,
    /// The child is cargo: at launch, building an app that was not built
    /// (`build` holds what to start once it is), or the dylib compile.
    #[allow(dead_code)]
    pub via_cargo: bool,
    /// The child is the `cargo build` of this app; when it succeeds the app
    /// starts in this slot (`spawn_client`, same id, same tile).
    pub build: Option<PendingLaunch>,
    /// The newest line the child (or cargo) wrote, shown on the tile
    /// under "starting…" until the first frame arrives.
    pub status: String,
    /// cargo has finished linking and handed over: the child's first exec
    /// is the one macOS scans.
    pub linked: bool,
    pub linked_at: Option<f64>,
    /// A polite close was sent at this instant (omarchy's
    /// `hl.dsp.window.close()`); the hard kill is only the fallback.
    pub closing: Option<f64>,
    /// The aichat child seated in the AI pane: not in the layout, no tile.
    pub pane: bool,
}

impl ClientSlot {
    /// The slot of an IN-PROCESS module instance (aicontrol §3): a window
    /// in the layout like any other — the bar, alt-tab and the `os` service
    /// see it — with no process behind it: no child, no socket, no build.
    pub fn module(id: ClientId, app: &str, title: &str) -> ClientSlot {
        ClientSlot {
            id,
            app: app.to_string(),
            title: title.to_string(),
            ground: None,
            child: None,
            task_pool: None,
            sender: None,
            socket: None,
            window_id: 0,
            ready: true,
            pwd: None,
            is_preview: false,
            warm: false,
            open_at: Some(host::now()),
            opened_warm: false,
            takes_focus: true,
            via_cargo: false,
            build: None,
            status: String::new(),
            linked: false,
            linked_at: None,
            closing: None,
            pane: false,
        }
    }
}

/// How long a client gets to honor a close request before it is killed.
pub const CLOSE_GRACE: std::time::Duration = std::time::Duration::from_millis(1500);

/// SIGTERM-to-SIGKILL escalation gap inside `kill_child_group`, once a
/// caller has already decided to hard-kill (past `CLOSE_GRACE`, or the
/// client never got that far — still building when it was closed).
pub const GROUP_KILL_GRACE: std::time::Duration = std::time::Duration::from_millis(300);

/// Put `cmd`'s child at the head of a brand-new process group (unix only):
/// `process_group(0)` is `setpgid(0, 0)` before exec, so the pgid becomes
/// the child's own pid. Every process it forks (a build's rustc) inherits
/// that same pgid, so the whole tree can be reached by one negative-pid
/// signal later.
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
pub fn kill_child_group(child: &mut Child, grace: std::time::Duration, pool: &TaskPool) {
    let pid = child.id() as i32;
    signal::kill_group(pid, signal::SIGTERM);
    let wait = CancellationToken::new();
    let submitted = pool.submit(Lane::Heavy, move || {
        let _ = wait.wait_until(Cx::monotonic_now() + grace.as_secs_f64());
        if signal::alive(-pid) {
            signal::kill_group(pid, signal::SIGKILL);
        }
    });
    match submitted {
        Ok(task) => task.detach(),
        Err(_) if signal::alive(-pid) => signal::kill_group(pid, signal::SIGKILL),
        Err(_) => {}
    }
}

#[cfg(not(unix))]
pub fn kill_child_group(child: &mut Child, _grace: std::time::Duration, _pool: &TaskPool) {
    let _ = child.kill();
}

/// Final slot teardown owns the child from here on. Signal and reap it wholly
/// on a heavy pool worker; dropping a slot on the UI thread never waits for a
/// process or decoder wrapper to exit.
fn reap_child_group(mut child: Child, grace: std::time::Duration, pool: &TaskPool) {
    #[cfg(unix)]
    let pid = {
        let pid = child.id() as i32;
        signal::kill_group(pid, signal::SIGTERM);
        pid
    };
    match pool.reserve(Lane::Heavy) {
        Ok(slot) => slot
            .submit(move || {
                #[cfg(unix)]
                {
                    let wait = CancellationToken::new();
                    let _ = wait.wait_until(Cx::monotonic_now() + grace.as_secs_f64());
                    if signal::alive(-pid) {
                        signal::kill_group(pid, signal::SIGKILL);
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = grace;
                    let _ = child.kill();
                }
                let _ = child.wait();
            })
            .detach(),
        Err(_) => {
            #[cfg(unix)]
            signal::kill_group(pid, signal::SIGKILL);
            #[cfg(not(unix))]
            let _ = child.kill();
        }
    }
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

/// Read a child stream into the log file and the UI channel, one line per
/// `\n` or `\r`: cargo redraws its progress bar ("Building [==>  ] 73/88:
/// …") with carriage returns, and each redraw is the tile's next status.
fn pump<R: std::io::Read + Send + 'static>(
    spawner: &ThreadSpawner,
    client: ClientId,
    stream: R,
    mut log: Option<std::fs::File>,
    lines: Sender<ClientLine>,
) {
    // Each pipe lives for the child's entire lifetime. A blocking reader
    // must not occupy a finite pool worker: enough open apps would starve
    // new compile logs and even process cleanup.
    let submitted = spawner.spawn_worker(ThreadOptions {
        name: Some(format!("wm-client-{client}-output").into()),
        ..Default::default()
    }, move || {
        use std::io::{BufReader, Read, Write};
        let mut bytes = BufReader::new(stream).bytes();
        let mut line = Vec::new();
        loop {
            let byte = match bytes.next() {
                Some(Ok(byte)) => Some(byte),
                Some(Err(_)) => break,
                None => None,
            };
            if let Some(byte) = byte.filter(|b| *b != b'\n' && *b != b'\r') {
                line.push(byte);
                continue;
            }
            let raw = String::from_utf8_lossy(&line).into_owned();
            line.clear();
            let text = strip_ansi(&raw).trim().to_string();
            // The bar's redraws are for the tile; the log keeps the lines.
            if let Some(file) = log.as_mut().filter(|_| !text.is_empty() && !text.starts_with("Building [")) {
                let _ = writeln!(file, "{}", raw);
            }
            if !text.is_empty() {
                if lines.send(ClientLine { client, text }).is_err() {
                    break;
                }
                SignalToUI::set_ui_signal();
            }
            if byte.is_none() {
                break;
            }
        }
    });
    match submitted {
        Ok(task) => task.detach(),
        Err(error) => makepad_widgets::log!("wm: could not queue client output pump: {error}"),
    }
}

/// Cargo output that changes the launch panel. Compiler diagnostics stay in
/// the client log and do not overwrite a useful build stage with source text.
pub fn cargo_progress(raw: &str) -> Option<(String, bool)> {
    let raw = raw.trim();
    if raw.starts_with("Blocking waiting for file lock") {
        Some(("waiting for another build…".into(), false))
    } else if raw.starts_with("Running ") || raw.starts_with("Finished ") {
        Some(("launching…".into(), true))
    } else if let Some(rest) = raw.strip_prefix("Building [") {
        // "=====>   ] 73/88: windows, makepad-script"
        let (_, rest) = rest.split_once(']')?;
        let (count, crates) = rest.trim().split_once(':').unwrap_or((rest.trim(), ""));
        let crates = crates.trim();
        Some((
            if crates.is_empty() { format!("compiling {count} crates…") } else { format!("compiling {count} crates · {crates}…") },
            false,
        ))
    } else if let Some(rest) = raw.strip_prefix("Compiling ") {
        let package = rest.split(" (").next().unwrap_or(rest).trim();
        Some((format!("compiling {package}…"), false))
    } else if raw.starts_with("error:") || raw.starts_with("error[") {
        Some(("build failed — see the app log".into(), false))
    } else if raw.starts_with("extracting ")
        || raw.starts_with("unpacking ")
        || raw.starts_with("unpacked ")
        || raw.starts_with("inflating ")
        || raw.starts_with("provision")
        || raw.starts_with("bootstrapping")
        || raw.starts_with("aligning ")
        || raw.starts_with("target dir ")
        || raw.starts_with("cargo rustc")
        || raw.starts_with("rewriting ")
        || raw.starts_with("patch")
    {
        Some((raw.to_string(), false))
    } else {
        None
    }
}

/// The command line that starts an app: its built binary (see
/// `built_binary`; nothing is compiled here) with `--stdin-loop`, the app's
/// own args, then `extra_args`.
pub fn launch_argv(
    app: &AppDef,
    root: Option<&Path>,
    extra_args: &[String],
) -> Result<(PathBuf, Vec<String>), String> {
    let mut args: Vec<String> = Vec::new();
    let program = built_binary(app, root).ok_or_else(|| match root {
        Some(_) => format!("not built yet: {}", app.bin),
        None => format!("binary not found: {}", app.bin),
    })?;
    // Android: the launcher's first argument is the app library it runs;
    // with on-device builds on (`adb shell setprop debug.makepad.wm.ondevice
    // 1`, an APK packed with `--proc-toolchain`) it builds the app from the
    // shipped source first — the phone's `cargo run` — and falls back to
    // this library.
    #[cfg(target_os = "android")]
    if root.is_none() {
        if let Some((_, lib)) = crate::host::android_app_binary(&app.bin) {
            if crate::host::android_ondevice_builds() {
                args.push("--build".to_string());
                args.push(app.bin.clone());
            }
            args.push(lib.to_string_lossy().to_string());
        }
    }
    args.push("--stdin-loop".to_string());
    args.extend(app.args.iter().cloned());
    args.extend(extra_args.iter().cloned());
    Ok((program, args))
}

/// The build of a deck app the person opened while it was not built:
/// `cargo build --release` of exactly its package and binary (children are
/// ALWAYS release builds, never debug). `--manifest-path` keeps it
/// independent of the cwd.
pub fn build_argv(app: &AppDef, root: &Path) -> (PathBuf, Vec<String>) {
    // In a Builder installation the Builder builds it: the same command,
    // flags, features and target as its own builds, so nothing it compiled
    // compiles again (`makepad-builder build APP`, Unix `makepad build APP`).
    if let Some(command) = builder_install().and_then(|install| install.command()) {
        return (command, vec!["build".to_string(), app.id.clone()]);
    }
    let manifest = root.join(app.manifest.as_deref().unwrap_or("Cargo.toml"));
    let manifest = manifest.to_string_lossy();
    let args = [
        "build",
        "--release",
        "--manifest-path",
        &manifest,
        "-p",
        &app.package,
        "--bin",
        &app.bin,
    ];
    (cargo_bin(), args.iter().map(|arg| arg.to_string()).collect())
}

/// What starts in a building slot once its build succeeds.
#[derive(Clone, Debug, Default)]
pub struct PendingLaunch {
    pub cwd: Option<PathBuf>,
}

/// The log of client `id`'s process, or of its build.
pub fn client_log(id: ClientId, build: bool) -> PathBuf {
    host::homeless_root().join(format!("wm-client-{}{}.log", id, if build { "-build" } else { "" }))
}

/// Start `program` as client `id`'s process: hidden, stdin closed, both
/// output streams into `log` and onto its tile.
fn spawn_process(
    spawner: &ThreadSpawner,
    id: ClientId,
    log_path: PathBuf,
    mut cmd: Command,
    what: &str,
    lines: Sender<ClientLine>,
) -> Result<Child, String> {
    // Give the process its own group (unix), so `kill_child_group` reaches
    // whatever it starts (a build's rustc). Windows: cargo holds its
    // children in a job object of its own; killing cargo ends them.
    #[cfg(unix)]
    own_process_group(&mut cmd);
    host::no_console_window(&mut cmd);
    // Both streams are piped so a reader thread can put the newest line on
    // the tile — cargo talks on stderr.
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("spawn {what}: {e}"))?;
    // Child output goes to a per-client log — silent children are
    // undebuggable — and every line also reaches the UI.
    let log = std::fs::File::create(&log_path).ok();
    if let Some(out) = child.stdout.take() {
        pump(spawner, id, out, log.as_ref().and_then(|f| f.try_clone().ok()), lines.clone());
    }
    if let Some(err) = child.stderr.take() {
        pump(spawner, id, err, log, lines);
    }
    Ok(child)
}

fn client_slot(pool: &TaskPool, id: ClientId, app: &AppDef, child: Child, warm: bool) -> ClientSlot {
    ClientSlot {
        id,
        ground: None,
        app: app.id.to_string(),
        title: String::new(),
        child: Some(child),
        task_pool: Some(pool.clone()),
        sender: None,
        socket: None,
        window_id: 0,
        ready: false,
        pwd: None,
        is_preview: false,
        warm,
        open_at: (!warm).then(host::now),
        opened_warm: false,
        // A warm instance is not a window yet: nothing may focus it until
        // adoption hands it a tile.
        takes_focus: !warm,
        via_cargo: false,
        build: None,
        status: String::new(),
        linked: false,
        linked_at: None,
        closing: None,
        pane: false,
    }
}

/// Spawn an app as a hub client. It must be built (`launch_argv`); an app
/// that is not goes through `spawn_build` first.
pub fn spawn_client(
    pool: &TaskPool,
    spawner: &ThreadSpawner,
    app: &AppDef,
    id: ClientId,
    hub_port: u16,
    cwd: Option<&PathBuf>,
    term_colors: Option<&str>,
    // `extra_args` is appended after the app's own args: the file to open,
    // with `--preview` in front of it for a Quick-Look popup.
    extra_args: &[String],
    // A DORMANT warm-pool instance: same launch in every other way — same
    // binary, same env, same log — plus `WARM_ENV`, which tells the app to
    // come up and then idle until it is adopted.
    warm: bool,
    // Every output line the child writes is forwarded here.
    lines: Sender<ClientLine>,
) -> Result<ClientSlot, String> {
    let root = repo_root();
    let (program, args) = launch_argv(app, root.as_deref(), extra_args)?;
    let mut cmd = Command::new(program);
    cmd.args(&args);
    cmd.env("STUDIO_HOST", format!("http://127.0.0.1:{}", hub_port))
        .env("STUDIO_BUILD", id.to_string())
        .env("STUDIO_CRATE", &app.bin);
    // The app owns any controls it embeds in its caption. Keep that content
    // inside the tile; the WM still supplies the outer window decorations.
    cmd.env("MAKEPAD_WM_CAPTION_CONTENT", "1");
    if let Some(cwd) = cwd {
        // The terminal's Omarchy behavior: open where the focused one is.
        cmd.arg("--cwd").arg(cwd);
        cmd.current_dir(cwd);
    } else if let Some(root) = &root {
        // Apps resolve their data (route's local/maps/, resources)
        // relative to the checkout root, as when run from the repo.
        cmd.current_dir(root);
    }
    if let Some(colors) = term_colors {
        cmd.env("MAKEPAD_TERMINAL_COLORS", colors);
        // Truly translucent terminals over the wallpaper (the user's
        // default; omarchy gets this from ghostty background-opacity —
        // its window rule alone, 0.985/0.96, reads as opaque).
        // "focused unfocused"; MAKEPAD_WM_TERM_OPACITY overrides.
        let opacity = std::env::var("MAKEPAD_WM_TERM_OPACITY")
            .unwrap_or_else(|_| "0.78 0.70".to_string());
        cmd.env("MAKEPAD_TERMINAL_OPACITY", opacity);
    }
    // Every Makepad app styles itself from the WM's theme.splash.
    if let Ok(theme) = std::env::var("MAKEPAD_WM_THEME_SPLASH") {
        cmd.env("MAKEPAD_WM_THEME_SPLASH", theme);
    }
    if warm {
        cmd.env(WARM_ENV.0, WARM_ENV.1);
    }
    let child = spawn_process(spawner, id, client_log(id, false), cmd, &app.package, lines)?;
    Ok(client_slot(pool, id, app, child, warm))
}

/// The person opened a deck app that is not built: its slot's process is
/// the build (`build_argv`, in the environment wm got — the Makepad
/// Builder's compiler when it started wm). The tile shows the crate count;
/// when cargo is done the app starts in the same slot (`spawn_client`).
pub fn spawn_build(
    pool: &TaskPool,
    spawner: &ThreadSpawner,
    app: &AppDef,
    id: ClientId,
    root: &Path,
    launch: PendingLaunch,
    lines: Sender<ClientLine>,
) -> Result<ClientSlot, String> {
    let (program, args) = build_argv(app, root);
    let mut cmd = Command::new(program);
    cmd.args(&args).current_dir(root);
    // The progress bar even into a pipe (it needs a width then): its
    // "73/88" is the tile's count. No colors.
    cmd.env("CARGO_TERM_COLOR", "never")
        .env("CARGO_TERM_PROGRESS_WHEN", "always")
        .env("CARGO_TERM_PROGRESS_WIDTH", "100")
        // The Builder's build says its crate count the way cargo's bar does.
        .env("MAKEPAD_BUILD_PROGRESS_LINES", "1");
    let child = spawn_process(spawner, id, client_log(id, true), cmd, &format!("cargo build -p {}", app.package), lines)?;
    let mut slot = client_slot(pool, id, app, child, false);
    slot.via_cargo = true;
    slot.build = Some(launch);
    slot.status = "compiling…".into();
    Ok(slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_progress_keeps_the_build_stage_readable() {
        assert_eq!(cargo_progress("   Compiling makepad-photos v0.1.0 (/a/checkout)"), Some(("compiling makepad-photos v0.1.0…".into(), false)));
        assert_eq!(cargo_progress("Blocking waiting for file lock on build directory"), Some(("waiting for another build…".into(), false)));
        assert_eq!(cargo_progress("    Finished `release` profile in 2s"), Some(("launching…".into(), true)));
        assert_eq!(cargo_progress("     Running `/a/checkout/target/release/photos`"), Some(("launching…".into(), true)));
        // The super-app's provisioning lines reach the desk verbatim.
        for line in ["extracting tc: already on disk", "inflating tc 120/292 MB · 88 files", "provisioning wmdyn x: resuming", "provision failed: unpack tc.tar.lz4: archive is truncated", "provisioned in 212 s"] {
            assert_eq!(cargo_progress(line), Some((line.into(), false)), "{line}");
        }
        assert!(cargo_progress("warning: unused variable").is_none());
        assert!(cargo_progress(" --> /a/checkout/src/main.rs:2").is_none());
        assert!(cargo_progress("app: first frame").is_none());
        assert_eq!(cargo_progress("error[E0308]: type mismatch"), Some(("build failed — see the app log".into(), false)));
    }

    #[test]
    fn children_are_always_release_never_debug() {
        // USER LAW: a hosted app is a release build, always.
        let app = curated().into_iter().find(|a| a.id == "terminal").unwrap();
        let root = std::path::PathBuf::from("/checkout");
        let (program, args) = build_argv(&app, &root);
        assert!(program.to_string_lossy().ends_with("cargo"), "{:?}", program);
        assert_eq!(&args[..2], &["build", "--release"], "{:?}", args);
        assert!(args.contains(&"/checkout/Cargo.toml".to_string()), "{:?}", args);
        assert!(args.windows(2).any(|w| w == ["-p", "makepad-terminal"]), "{:?}", args);
        assert!(args.windows(2).any(|w| w == ["--bin", "terminal"]), "{:?}", args);
        // Starting it runs the release binary in the checkout's target
        // (CARGO_TARGET_DIR when set: the Builder's shared one).
        let binary = checkout_binary(&app, &root, None);
        let exe = if cfg!(windows) { "terminal.exe" } else { "terminal" };
        assert_eq!(binary, root.join("target/release").join(exe));
        let shared = checkout_binary(&app, &root, Some(std::ffi::OsStr::new("/builder/target")));
        assert_eq!(shared, std::path::Path::new("/builder/target/release").join(exe));
        let missing = std::env::temp_dir().join(format!("wm-release-test-{}", std::process::id()));
        assert_eq!(launch_argv(&app, Some(&missing), &[]).unwrap_err(), "not built yet: terminal");
    }

    #[test]
    fn dep_info_paths_keep_escaped_spaces() {
        let paths: Vec<String> = dep_info_paths(r"C:\b\makepad-builder\ (2)\a.rs /x/b.rs  C:\c.rs").collect();
        assert_eq!(paths, [r"C:\b\makepad-builder (2)\a.rs", "/x/b.rs", r"C:\c.rs"]);
    }

    #[test]
    fn a_binary_older_than_a_source_is_stale() {
        let dir = std::env::temp_dir().join(format!("wm-stale-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src dir")).unwrap();
        let source = dir.join("src dir/main.rs");
        let binary = dir.join("app");
        std::fs::write(&source, "").unwrap();
        assert!(!binary_is_current(&binary), "not built");
        std::fs::write(&binary, "").unwrap();
        assert!(!binary_is_current(&binary), "no dep-info");
        let escaped = source.to_string_lossy().replace(' ', "\\ ");
        std::fs::write(dir.join("app.d"), format!("{}: {escaped}\n\n{escaped}:\n", binary.display())).unwrap();
        assert!(binary_is_current(&binary));
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        std::fs::File::options().write(true).open(&source).unwrap().set_modified(later).unwrap();
        assert!(!binary_is_current(&binary), "a source changed after the build");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cargo_progress_bar_is_a_crate_count() {
        assert_eq!(
            cargo_progress("Building [=====================>     ] 73/88: windows, makepad-script"),
            Some(("compiling 73/88 crates · windows, makepad-script…".into(), false))
        );
        assert_eq!(cargo_progress("Building [>   ] 0/88"), Some(("compiling 0/88 crates…".into(), false)));
    }

    #[test]
    fn a_builder_source_without_local_is_a_checkout() {
        // The Makepad Builder starts wm in the source it downloaded: the
        // workspace with apps/wm in it, and no developer's local/.
        let root = std::env::temp_dir().join(format!("wm-checkout-test-{}", std::process::id()));
        let app = root.join("apps/terminal/src");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(root.join("apps/wm")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
        assert_eq!(checkout_at_or_above(&app), None, "a workspace without wm is not one");
        std::fs::write(root.join("apps/wm/Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(checkout_at_or_above(&app), Some(root.clone()));
        std::fs::remove_dir_all(&root).unwrap();
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
                "Photos",
                "Clock",
                "Weather",
                "Finance",
                "Mail",
                "Notes",
                "Calendar",
                "Reminders",
                "Calculator",
                "Fabric",
                "Score",
                "Video Player",
                "Route",
                "Fab",
                "Studio",
                "Scope",
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
    fn appearance_retires_only_unused_browsers_and_never_counts_as_a_crash() {
        let (mut pool,status)=pool_with("browser", &[40,41]);
        pool.note_spawned("files",42);
        assert!(pool.set_browser_appearance(true).is_empty());
        assert_eq!(pool.adopt("browser",false,&status),Some(40));
        assert!(pool.set_browser_appearance(true).is_empty());
        assert_eq!(pool.set_browser_appearance(false),vec![41]);
        assert_eq!(pool.adopt("browser",false,&status),None);
        assert!(pool.holds(42));
        // Reaping intentional retirements cannot charge the crash budget.
        assert_eq!(pool.forget(41),None);
        for dark in [true,false,true,false] {assert!(pool.set_browser_appearance(dark).is_empty());}
        assert!(pool.wants("browser",0.0));
        pool.note_spawned("browser",43);
        assert!(pool.set_browser_appearance(false).is_empty());
        assert!(pool.holds(43));
    }

    #[test]
    fn appearance_changes_do_not_enable_a_disabled_pool() {
        let mut pool=WarmPool::new(false);
        pool.set_browser_appearance(true);
        pool.set_browser_appearance(false);
        assert!(!pool.wants("browser",0.0));
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
        assert!(!pool.wants("browser", host::now()), "already full");
        assert_eq!(pool.next_missing(host::now(), |_| true), None, "nothing missing");
        assert_eq!(pool.adopt("browser", false, &status), Some(7));
        // Out of the pool, and the pool now wants its replacement.
        assert_eq!(pool.held("browser"), 0);
        assert!(!pool.holds(7));
        assert!(pool.wants("browser", host::now()));
        assert_eq!(pool.next_missing(host::now(), |_| true).as_deref(), Some("browser"));
        // The same instance can never be adopted twice.
        assert_eq!(pool.adopt("browser", false, &status), None);
    }

    #[test]
    fn two_terminals_stand_by_and_both_open_instantly() {
        let (mut pool, status) = pool_with("terminal", &[3, 4]);
        assert_eq!(pool.held("terminal"), 2);
        assert!(!pool.wants("terminal", host::now()));
        // Back-to-back opens: both are swaps, oldest first.
        assert_eq!(pool.adopt("terminal", false, &status), Some(3));
        assert_eq!(pool.held("terminal"), 1);
        assert_eq!(pool.adopt("terminal", false, &status), Some(4));
        assert_eq!(pool.held("terminal"), 0);
        // A third open in the same breath falls back to cold, and the pool
        // is two short — one spawn per tick, so it tops up twice.
        assert_eq!(pool.adopt("terminal", false, &status), None);
        assert!(pool.wants("terminal", host::now()));
        pool.note_spawned("terminal", 9);
        assert!(pool.wants("terminal", host::now()));
        pool.note_spawned("terminal", 10);
        assert!(!pool.wants("terminal", host::now()));
        assert_eq!(pool.next_missing(host::now(), |_| true).as_deref(), Some("browser"));
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
        assert!(!pool.wants("browser", host::now()));

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
        assert!(!off.wants("terminal", host::now()));
        assert_eq!(off.next_missing(host::now(), |_| true), None);
        // …and even a hand-fed instance is never adopted.
        off.note_spawned("terminal", 1);
        let status = [WarmStatus { client: 1, alive: true, connected: true }];
        assert_eq!(off.adopt("terminal", false, &status), None);
    }

    #[test]
    fn a_crash_loop_gives_up_quietly_after_three_a_minute() {
        let now = host::now();
        let mut pool = WarmPool::new(true);
        for i in 0..WARM_CRASH_LIMIT {
            assert!(pool.wants("browser", now), "attempt {}", i);
            pool.note_spawned("browser", i as ClientId);
            // Up, then dead before anyone could adopt it.
            let app = pool.forget(i as ClientId).expect("pooled");
            pool.note_crash(&app, now);
        }
        assert!(!pool.wants("browser", now), "the budget should be spent");
        assert_eq!(pool.next_missing(now, |_| true).as_deref(), Some("terminal"));
        // The budget is per app…
        assert!(pool.wants("terminal", now));
        // …and it is a WINDOW: a minute later the app is tried again.
        assert!(pool.wants("browser", now + WARM_CRASH_WINDOW + 1.0));
    }

    #[test]
    fn a_deliberate_close_costs_no_budget_and_tops_back_up() {
        // CTRL+ALT+DELETE closes the warm instances with everything else;
        // that is not a crash, so the pool refills at once instead of
        // spending the loop budget on the user's own gesture.
        let now = host::now();
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
        // A checkout whose files is built (in its own target/: a shared
        // CARGO_TARGET_DIR of the run is left alone).
        if std::env::var_os("CARGO_TARGET_DIR").is_some() {
            return;
        }
        let files = find_app("files").unwrap();
        let root = std::env::temp_dir().join(format!("wm-warm-test-{}", std::process::id()));
        let binary = checkout_binary(&files, &root, None);
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, b"").unwrap();
        let (program, args) = launch_argv(&files, Some(&root), &[]).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(program, binary);
        assert_eq!(args[0], "--stdin-loop", "{:?}", args);
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
        let pool = Cx::new(Box::new(|_, _| {})).task_pool();
        kill_child_group(&mut child, std::time::Duration::from_millis(50), &pool);
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

        let pool = Cx::new(Box::new(|_, _| {})).task_pool();
        kill_child_group(&mut child, std::time::Duration::from_millis(50), &pool);
        // Past the SIGTERM->SIGKILL escalation: nothing in the group is
        // still standing, wrapper or grandchild.
        let _ = child.wait();
        // The wrapper can exit before the asynchronous escalation and before
        // launchd reaps the orphan. Wait only in this test, never on the UI.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while unsafe { kill(grandchild_pid, 0) } == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            unsafe { kill(grandchild_pid, 0) },
            -1,
            "the grandchild the wrapper orphaned should be gone too"
        );
    }
}

impl Drop for ClientSlot {
    fn drop(&mut self) {
        // Ask the app to go first over its own socket (it may be a wrapper's
        // child), then reap the process we hold.
        if let Some(sender) = self.sender.take() {
            crate::hub::send_to_app(
                &sender,
                vec![makepad_studio_protocol::StudioToApp::Kill],
            );
        }
        if let Some(mut child) = self.child.take() {
            if let Some(pool) = &self.task_pool {
                reap_child_group(child, GROUP_KILL_GRACE, pool);
            } else {
                let _ = child.kill();
            }
        }
    }
}

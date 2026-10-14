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

use makepad_widgets::makepad_platform::thread::{SignalToUI, TaskPool, ThreadSpawner, ThreadOptions};

use makepad_app_host::ClientId;
use makepad_app_registry::{AppLaunch, BuildPolicy, LaunchEvent, LaunchKind, ProcessOutput};

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
    /// WM arguments appended after the registry defaults.
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

    /// Its build/run identity, for the shared registry's checkout helpers.
    pub fn target(&self) -> makepad_app_registry::AppTarget<'_> {
        makepad_app_registry::AppTarget {
            id: &self.id,
            package: &self.package,
            dir: &self.dir,
            bin: &self.bin,
            manifest: self.manifest.as_deref(),
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
        match repo_root() {
            Some(root) => makepad_app_registry::has_sources(&self.target(), &root),
            None => resolve_bin(&self.bin).is_some(),
        }
    }
}

/// A shared registry entry as the WM launches it: its launch model plus the
/// WM's own runtime defaults on top.
fn registry_app(entry: &makepad_app_registry::AppEntry) -> AppDef {
    let policy = match entry.policy {
        makepad_app_registry::LaunchPolicy::AlwaysNew => LaunchPolicy::AlwaysNew,
        makepad_app_registry::LaunchPolicy::OrFocus => LaunchPolicy::OrFocus,
    };
    let mut app = AppDef::app(
        entry.id,
        entry.label,
        entry.package,
        entry.dir,
        entry.bin,
        policy,
    );
    app.manifest = entry.manifest.map(str::to_string);
    match entry.id {
        // Recording default: the menu opens the demo VFS, unless the
        // person explicitly asks to browse the real disk.
        "files" if std::env::var("MAKEPAD_WM_FILES_REAL").is_err() => {
            app.args.push("--demo".to_string());
        }
        "fab" => {
            // Children run from the checkout; use the built-in demo house
            // when the converted model is not there.
            let house = "local/fab/models/woodside.glb";
            if repo_root().map(|r| r.join(house).exists()).unwrap_or(false) {
                app.args.push("--open".to_string());
                app.args.push(house.to_string());
            }
        }
        _ => {}
    }
    app
}

/// The curated applications, in menu order: the menu rows of the shared
/// registry (`libs/app_registry/apps.json`). Every one of these is a real
/// window we host; servers and headless tools are deliberately absent.
fn curated() -> Vec<AppDef> {
    makepad_app_registry::apps()
        .iter()
        .filter(|entry| entry.menu_visible && entry.wm_launchable)
        .map(registry_app)
        .collect()
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
    // A binary name (`makepad-app-terminal`, as a standalone app's sibling
    // spawn or a process list names it) resolves to its curated entry too.
    if let Some(app) = registry().iter().find(|a| a.bin == id) {
        return Some(app.clone());
    }
    // The hidden entries, by id only: the file viewers (previews, Open
    // With), the pacing/protocol rigs (splash, counter) and the assistant,
    // which the WM seats in the pane slot through its own path, never
    // `launch_app` (AlwaysNew keeps launch-or-focus from "focusing" a
    // pane). Builder-only entries are not WM clients.
    makepad_app_registry::find(id)
        .filter(|entry| entry.wm_launchable && !entry.menu_visible)
        .map(registry_app)
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

/// `sheet` with the own artwork (`<dir>/resources/icon.svg`) of every registry app the
/// style has no bundled icon for (Stage, an app added later): the dock, the taskbars and
/// the launcher draw the app's icon, not the generic one. Children get it with the sheet.
#[cfg_attr(target_arch = "wasm32", allow(unused_mut))] // the web has no registry dirs to read artwork from
pub fn with_app_artwork(mut sheet: makepad_widgets::desktop_style::StyleSheet) -> makepad_widgets::desktop_style::StyleSheet {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(root) = repo_root() {
        for app in registry() {
            let name = makepad_widgets::app_icon::canonical_name(&app.id);
            if sheet.icons.iter().any(|a| a.name == name) {
                continue;
            }
            if let Ok(svg) = std::fs::read_to_string(root.join(&app.dir).join("resources/icon.svg")) {
                sheet.icons.push(makepad_widgets::app_icon::IconAsset { name: name.into(), svg });
            }
        }
    }
    sheet
}

/// The checkout root (`makepad_app_registry::repo_root`: `MAKEPAD_WM_ROOT`,
/// else a Builder installation's sources, else the checkout above the
/// running exe or at/above the cwd). The WM and Stage share this answer.
pub fn repo_root() -> Option<PathBuf> {
    makepad_app_registry::repo_root()
}

/// The binary that starts `app` without compiling anything
/// (`makepad_app_registry::built_binary`: a Builder's published one, the
/// checkout's release build, or an installed sibling). None: not built.
pub fn built_binary(app: &AppDef, root: Option<&Path>) -> Option<PathBuf> {
    makepad_app_registry::built_binary(&app.target(), root)
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
    {
        makepad_app_registry::installed_binary(bin)
    }
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
    /// The dark/light the WM gives browsers and apps (None before the first).
    pub fn browser_appearance(&self) -> Option<bool> {
        self.browser_dark
    }
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
    pub launch: Option<AppLaunch>,
    launch_cwd: Option<PathBuf>,
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
    /// Came to the WM by itself through its socket (adopt.rs): its launcher owns the
    /// process, and the tile ends when its hub socket closes.
    pub external: bool,
    /// One of an app's later windows (`client_windows.rs`): the app's client. No process
    /// of its own; its sender is the app's.
    pub parent: Option<ClientId>,
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
            launch: None,
            launch_cwd: None,
            status: String::new(),
            linked: false,
            linked_at: None,
            closing: None,
            pane: false,
            external: false,
            parent: None,
        }
    }

    /// The slot of `window_id`, a later window of the app in `owner` (client `parent`):
    /// a window on the desk with the app's sender and no process.
    pub fn later_window(id: ClientId, parent: ClientId, owner: &ClientSlot, window_id: usize) -> ClientSlot {
        let mut slot = ClientSlot::module(id, &owner.app, owner.display_title());
        slot.sender = owner.sender.clone();
        slot.window_id = window_id;
        slot.ground = owner.ground;
        slot.parent = Some(parent);
        slot
    }
}

impl ClientSlot {
    /// The slot of an app the WM socket admitted: a window like a launched one, without a
    /// process handle (adopt.rs).
    pub fn adopted(id: ClientId, app: &str) -> ClientSlot {
        let mut slot = ClientSlot::module(id, app, "");
        slot.ready = false;
        slot.external = true;
        slot.status = "starting…".into();
        slot
    }

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
    pub output: ProcessOutput,
}

/// Read a child stream into the log file and the UI channel, one line per
/// `\n` or `\r`: cargo redraws its progress bar ("Building [==>  ] 73/88:
/// …") with carriage returns, and each redraw is the tile's next status.
fn pump<R: std::io::Read + Send + 'static>(
    spawner: &ThreadSpawner,
    client: ClientId,
    stream: R,
    log: Option<std::fs::File>,
    lines: Sender<ClientLine>,
) -> Result<(), String> {
    // Each pipe lives for the child's entire lifetime. A blocking reader
    // must not occupy a finite pool worker: enough open apps would starve
    // new compile logs and even process cleanup.
    let submitted = spawner.spawn_worker(ThreadOptions {
        name: Some(format!("wm-client-{client}-output").into()),
        ..Default::default()
    }, move || {
        makepad_app_registry::read_output(stream, log, |output| {
            if lines.send(ClientLine { client, output }).is_err() {
                return false;
            }
            SignalToUI::set_ui_signal();
            true
        });
    });
    submitted.map_err(|error| format!("client output: {error}"))?.detach();
    Ok(())
}

/// The log of client `id`'s process, or of its build.
pub fn client_log(id: ClientId, build: bool) -> PathBuf {
    host::homeless_root().join(format!("wm-client-{}{}.log", id, if build { "-build" } else { "" }))
}

/// Start `program` as client `id`'s process: hidden, stdin closed, both
/// output streams into `log` and onto its tile.
fn spawn_process(
    pool: &TaskPool,
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
    // (`configure` did that for an app; a build gets it here.)
    #[cfg(unix)]
    makepad_app_host::launch::own_process_group(&mut cmd);
    host::no_console_window(&mut cmd);
    // Both streams are piped so a reader thread can put the newest line on
    // the tile — cargo talks on stderr.
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // A client (or its build) never outlives the WM: it ends when the WM's UI thread, the one
    // spawning it, goes (quit, crash or kill), as app_host's hosted children do.
    #[cfg(target_os = "linux")]
    makepad_app_host::launch::exit_with_parent(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("spawn {what}: {e}"))?;
    #[cfg(windows)]
    makepad_app_host::launch::exit_with_parent(&child);
    // Child output goes to a per-client log — silent children are
    // undebuggable — and every line also reaches the UI.
    let log = std::fs::File::create(&log_path).ok();
    let result = (|| {
        if let Some(out) = child.stdout.take() {
            pump(spawner, id, out, log.as_ref().and_then(|file| file.try_clone().ok()), lines.clone())?;
        }
        if let Some(err) = child.stderr.take() {
            pump(spawner, id, err, log, lines)?;
        }
        Ok::<(), String>(())
    })();
    if let Err(error) = result {
        makepad_app_host::launch::reap_child_group(child, makepad_app_host::launch::GROUP_KILL_GRACE, pool);
        return Err(error);
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
        launch: None,
        launch_cwd: None,
        status: String::new(),
        linked: false,
        linked_at: None,
        closing: None,
        pane: false,
        external: false,
        parent: None,
    }
}

/// Start a registry app through the shared build/launch state machine.
/// The WM supplies its hub, environment, log streams and stale-source policy.
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
    policy: BuildPolicy,
    // Every output line the child writes is forwarded here.
    lines: Sender<ClientLine>,
) -> Result<ClientSlot, String> {
    let entry = makepad_app_registry::find(&app.id).ok_or_else(|| format!("unknown app: {}", app.id))?;
    let extra = app.args.iter().chain(extra_args).cloned().collect();
    let (launch, child) = AppLaunch::start(entry, repo_root(), extra, policy, false, |cmd, kind| {
        spawn_for_client(pool, spawner, id, &app.bin, cwd, term_colors, warm, hub_port, cmd, kind, lines.clone())
    })?;
    let mut slot = client_slot(pool, id, app, child, warm);
    slot.status = launch.status().to_string();
    slot.launch = Some(launch);
    slot.launch_cwd = cwd.cloned();
    Ok(slot)
}

/// Host-specific configuration; both initial launch and build completion use it.
fn spawn_for_client(
    pool: &TaskPool,
    spawner: &ThreadSpawner,
    id: ClientId,
    bin: &str,
    cwd: Option<&PathBuf>,
    term_colors: Option<&str>,
    warm: bool,
    hub_port: u16,
    mut cmd: Command,
    kind: LaunchKind,
    lines: Sender<ClientLine>,
) -> Result<Child, String> {
    if kind == LaunchKind::Build {
        return spawn_process(pool, spawner, id, client_log(id, true), cmd, bin, lines);
    }
    // The app, or `cargo run` of it (cargo's progress on the tile, then the app): the
    // app's environment either way.
    // The hub environment, and the child as its own process group.
    makepad_app_host::launch::configure(&mut cmd, hub_port, id, bin);
    // The app owns any controls it embeds in its caption. Keep that content
    // inside the tile; the WM still supplies the outer window decorations.
    cmd.env("MAKEPAD_WM_CAPTION_CONTENT", "1");
    // The WM draws the app's file and folder pickers (shell/file_picker.rs): a hosted
    // app on Linux has no display of its own to open a system one on. Elsewhere the
    // app keeps its system dialogs.
    #[cfg(target_os = "linux")]
    cmd.env(makepad_widgets::makepad_platform::hosted_relay::HOST_FILE_DIALOGS_ENV, "1");
    if let Some(cwd) = cwd {
        // The terminal's Omarchy behavior: open where the focused one is.
        cmd.arg("--cwd").arg(cwd);
        cmd.current_dir(cwd);
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
    // MAKEPAD_WIDGET_STYLE is inherited at spawn; makepad_style events
    // deliver live changes to the WM's system stylesheet.
    if warm {
        cmd.env(WARM_ENV.0, WARM_ENV.1);
    }
    spawn_process(pool, spawner, id, client_log(id, false), cmd, bin, lines)
}

impl ClientSlot {
    pub fn is_building(&self) -> bool {
        self.launch.as_ref().is_some_and(AppLaunch::is_building)
    }

    pub fn poll_launch(
        &mut self,
        spawner: &ThreadSpawner,
        hub_port: u16,
        term_colors: Option<&str>,
        lines: Sender<ClientLine>,
    ) -> Result<Option<LaunchEvent>, String> {
        let Some(launch) = &mut self.launch else { return Ok(None) };
        let id = self.id;
        let app = &self.app;
        let cwd = self.launch_cwd.as_ref();
        let warm = self.warm;
        let pool = self.task_pool.as_ref().ok_or("no task pool for the client")?;
        launch.poll(&mut self.child, |cmd, kind| {
            let entry = makepad_app_registry::find(app).ok_or_else(|| format!("unknown app: {app}"))?;
            spawn_for_client(pool, spawner, id, entry.bin, cwd, term_colors, warm, hub_port, cmd, kind, lines.clone())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_app_registry::{cargo_progress, launch_argv};

    #[test]
    fn cargo_progress_keeps_the_build_stage_readable() {
        assert_eq!(cargo_progress("   Compiling makepad-app-photos v0.1.0 (/a/checkout)"), Some(("compiling makepad-app-photos v0.1.0…".into(), false)));
        assert_eq!(cargo_progress("Blocking waiting for file lock on build directory"), Some(("waiting for another build…".into(), false)));
        assert_eq!(cargo_progress("    Finished `release` profile in 2s"), Some(("launching…".into(), true)));
        assert_eq!(cargo_progress("     Running `/a/checkout/target/release/makepad-app-photos`"), Some(("launching…".into(), true)));
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
        let (program, args) = makepad_app_registry::build_argv(&app.target(), &root);
        assert!(program.to_string_lossy().ends_with("cargo"), "{:?}", program);
        assert_eq!(&args[..2], &["build", "--release"], "{:?}", args);
        assert!(args.contains(&"/checkout/Cargo.toml".to_string()), "{:?}", args);
        assert!(args.windows(2).any(|w| w == ["-p", "makepad-app-terminal"]), "{:?}", args);
        assert!(args.windows(2).any(|w| w == ["--bin", "makepad-app-terminal"]), "{:?}", args);
        // Starting it runs the release binary in the checkout's target
        // (CARGO_TARGET_DIR when set: the Builder's shared one).
        let binary = makepad_app_registry::checkout_binary(&app.target(), &root, None);
        let exe = if cfg!(windows) { "makepad-app-terminal.exe" } else { "makepad-app-terminal" };
        assert_eq!(binary, root.join("target/release").join(exe));
        let shared = makepad_app_registry::checkout_binary(&app.target(), &root, Some(std::ffi::OsStr::new("/builder/target")));
        assert_eq!(shared, std::path::Path::new("/builder/target/release").join(exe));
        let missing = std::env::temp_dir().join(format!("wm-release-test-{}", std::process::id()));
        assert_eq!(launch_argv(makepad_app_registry::find(&app.id).unwrap(), Some(&missing), &app.args).unwrap_err(), "not built yet: makepad-app-terminal");
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
        assert_eq!(makepad_app_registry::checkout_at_or_above(&app), None, "a workspace without wm is not one");
        std::fs::write(root.join("apps/wm/Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(makepad_app_registry::checkout_at_or_above(&app), Some(root.clone()));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_installed_fallback_execs_the_sibling_binary() {
        // No checkout: run the binary next to the running wm, which for
        // a release wm is target/release/<bin>.
        let app = curated().into_iter().find(|a| a.id == "terminal").unwrap();
        match launch_argv(makepad_app_registry::find(&app.id).unwrap(), None, &app.args) {
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
        assert!(word_match("makepad-app-files - files (2)", "files"));
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
                "Amp",
                "Code",
                "Task Manager",
                "Sheets",
                "Photos",
                "Camera",
                "Clock",
                "Weather",
                "Translator",
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
                "Scope",
                "Stage",
                "Sandbox",
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
        // Builder-only registry rows are not WM clients, and hidden entries
        // resolve by id only, never by binary.
        for id in ["wm", "wm-all", "ai-hub"] {
            assert!(find_app(id).is_none(), "{} must not be launchable by the WM", id);
        }
        assert!(find_app("splash").is_some());
        assert!(find_app("makepad-example-splash").is_none());
        for (id, policy) in [
            ("terminal", LaunchPolicy::AlwaysNew),
            ("video", LaunchPolicy::AlwaysNew),
            ("fab", LaunchPolicy::OrFocus),
        ] {
            let app = apps.iter().find(|a| a.id == id).expect(id);
            assert_eq!(app.policy, policy, "{}", id);
            assert!(!app.package.is_empty() && !app.dir.is_empty());
        }
    }

    #[test]
    fn the_association_table_resolves_by_binary_name() {
        // Binary names (what a standalone app spawns as a sibling) resolve
        // to their curated entries, as do the registry ids
        // `makepad_wm_api::viewer_for` answers.
        for (bin, id) in [
            ("makepad-app-terminal", "terminal"),
            ("makepad-app-browser", "browser"),
            ("makepad-app-sheets", "sheets"),
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
        for app in ["vj", "fab", "image", "nonesuch"] {
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
        let binary = makepad_app_registry::checkout_binary(&files.target(), &root, None);
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, b"").unwrap();
        let (program, args) = launch_argv(makepad_app_registry::find(&files.id).unwrap(), Some(&root), &files.args).unwrap();
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
}

impl Drop for ClientSlot {
    fn drop(&mut self) {
        // Ask the app to go first over its own socket (it may be a wrapper's
        // child), then reap the process we hold. A later window's slot holds the
        // app's sender: it never ends the app.
        if self.parent.is_some() {
            return;
        }
        if let Some(sender) = self.sender.take() {
            makepad_app_host::send_to_app(
                &sender,
                vec![makepad_studio_protocol::StudioToApp::Kill],
            );
        }
        if let Some(mut child) = self.child.take() {
            if let Some(pool) = &self.task_pool {
                makepad_app_host::launch::reap_child_group(child, makepad_app_host::launch::GROUP_KILL_GRACE, pool);
            } else {
                let _ = child.kill();
            }
        }
    }
}

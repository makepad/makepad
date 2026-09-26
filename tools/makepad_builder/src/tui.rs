//! The setup process owns blocking work. On Windows MpTerm hosts this process
//! and all of its children; Unix bootstraps use the matching terminal menu.
//!
//! One full-screen view in the terminal's own colours: YOUR APPS, MAKEPAD
//! EXPERIMENTS, CODING AGENTS and SETUP rows, one status line where
//! questions, progress and results appear, and a key-hint footer. The file
//! reads top to bottom: startup, the main menu, then one section per row
//! group, then helpers and terminal input. Drawing lives in tui_view.rs.

use crate::{
    catalog::{self, Release},
    progress,
    rustc,
    runtime::{self, Dependency, Environment, RustChoice, WindowsChain},
};

use std::{
    env, fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

// ---- Texts and links -------------------------------------------------------

/// App states and their status/action texts: `state|status|action`.
const MENU: &str = include_str!("../menu.txt");

/// The subtitle under the header.
const ABOUT: &str = include_str!("../about.txt");

/// License agreements, opened in the browser from the Agreements page and
/// the consent screens.
const MAKEPAD_LICENSE_URL: &str = "https://makepad.nl/commercial-license";
const BUILD_TOOLS_LICENSE_URL: &str = "https://visualstudio.microsoft.com/license-terms/vs2022-ga-diagnosticbuildtools/";
const WINDOWS_SDK_LICENSE_URL: &str = "https://learn.microsoft.com/legal/windows-sdk/windows-sdk-license";
const RUST_LICENSE_URL: &str = "https://www.rust-lang.org/policies/licenses";
const CUDA_LICENSE_URL: &str = "https://docs.nvidia.com/cuda/eula/";

/// Present once the GPU notice was read ("I understand") in this folder.
const GPU_NOTICE_READ: &str = "graphics-notice-read";

/// The GPU driver notice (Windows and Linux, until it is read).
const GPU_NOTICE: &str = "Makepad heavily relies on your GPU to draw its UI and implement AI functionality. Old hardware and broken drivers can cause your computer to reboot unexpectedly.";

/// Windows Defender and similar tools briefly lock a freshly unpacked rustc.
const STILL_SCANNING: &str = "Windows security software is still scanning the staged Rust compiler. Select the app again to check again.";

/// Returned as an error by sub-screens when the person pressed q.
const QUIT: &str = "\u{1}quit";

#[path = "tui_view.rs"]
mod view;
pub use view::with_progress;
use view::{activity, done, text, Item, Nav, Row, Screen, View, DIM, OK, PLAIN, WARN};

// ---- Startup: log in, graphics, then the main menu -------------------------

/// `makepad-builder tui`: the whole session, from the first screen to quit.
pub fn run() -> Result<(), String> {
    let root = crate::validate_install_root(&crate::default_root())?;
    let bootstrap = match makepad_loader_bundle::load_from(&root)? {
        Some(bootstrap) => Some(bootstrap),
        None => makepad_loader_bundle::load()?.map(|(_, b)| b),
    };
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let _lock = Lock::take(&root)?;
    let email = env::var("MAKEPAD_LOADER_EMAIL").ok().or_else(|| bootstrap.map(|b| b.email));
    let mut setup = Setup::open(root, email)?;
    view::set_log(setup.root.join("builder.log"));
    // The host window's palette is for this TUI only, not for the shells,
    // agents and apps started from it.
    env::remove_var("MAKEPAD_TERMINAL_COLORS");
    let screen = Screen::enter();
    // The email comes first (downloads are not personalized): asked once per
    // folder, kept in its makepad-builder.json and never asked again.
    if setup.email.is_empty() && !setup.root.join(".login-asked").exists() {
        setup.log_in_screen()?;
        let _ = fs::write(setup.root.join(".login-asked"), "");
    }
    setup.gpu_read = gpu_warning_acknowledged(&setup.root, &setup.shown_email())?;
    if !setup.gpu_read {
        drop(screen);
        println!("Setup cancelled. Nothing was downloaded or installed.");
        return Ok(());
    }
    show_menu(&mut setup)?;
    drop(screen);
    if io::stdout().is_terminal() { print!("\x1b[0m"); }
    println!("Makepad Builder closed.");
    Ok(())
}

fn show_menu(setup: &mut Setup) -> Result<(), String> {
    fs::write(setup.root.join("selected-app"), &setup.app).map_err(|e| e.to_string())?;
    setup.measure_disk();
    // Every start refreshes the licenses, then stops at the menu with the
    // first app selected: which app to build and open is the person's
    // choice. Build tools are set up when an app is first built.
    let startup = (|| -> Result<(), String> {
        // The log-in screen may have checked already.
        if setup.licenses.is_none() {
            if let Err(error) = setup.check_licenses() {
                setup.report(error);
            }
        }
        view::set_view(setup.main_view());
        Ok(())
    })();
    match startup {
        Err(error) if error == QUIT => return Ok(()),
        Err(error) => setup.report(error),
        Ok(()) => {}
    }
    // The menu opens on the first of YOUR APPS.
    view::forget_worked_on();
    let mut selected = setup.first_license_row(&setup.main_view());
    loop {
        let view = setup.main_view();
        let nav = view::menu(view, &mut selected, &|| setup.disk_changed())?;
        match nav {
            Nav::Refresh => {}
            Nav::Back | Nav::Quit => break,
            Nav::Select(id) => match setup.select(&id) {
                Err(error) if error == QUIT => break,
                Err(error) => setup.report(error),
                Ok(()) => {}
            },
        }
    }
    Ok(())
}

/// Linux and Windows show the GPU driver notice before anything is probed,
/// refreshed, downloaded or compiled, until the person chooses "I
/// understand"; that answer is kept in the folder (`graphics-notice-read`) and
/// the notice is not shown on later starts. macOS does not show it. Cancel,
/// Escape or closed input quit setup.
fn gpu_warning_acknowledged(root: &Path, email: &str) -> Result<bool, String> {
    if cfg!(target_os = "macos") || root.join(GPU_NOTICE_READ).is_file() {
        return Ok(true);
    }
    if !gpu_notice(email)? {
        return Ok(false);
    }
    record_gpu_notice(root)?;
    Ok(true)
}
/// Written through a temporary name, like the other records.
fn record_gpu_notice(root: &Path) -> Result<(), String> {
    let next = root.join(format!("{GPU_NOTICE_READ}.next"));
    fs::write(&next, "read\n").map_err(|e| e.to_string())?;
    fs::rename(&next, root.join(GPU_NOTICE_READ)).map_err(|e| e.to_string())
}

/// The GPU driver notice as a screen of its own: "I understand" (selected)
/// or Cancel; Escape and closed input cancel.
fn gpu_notice(email: &str) -> Result<bool, String> {
    let mut rows = vec![Row::Note(Vec::new())];
    rows.extend(wrap(GPU_NOTICE, 74).into_iter().map(|line| Row::Note(text(line, PLAIN))));
    rows.push(item("continue", "I understand", "", Vec::new(), ""));
    rows.push(item("cancel", "Cancel", "", Vec::new(), ""));
    let view = View {
        crumb: " › Graphics".into(),
        subtitle: "Before you continue.".into(),
        email: email.into(),
        rows,
        back: true,
        footer: Some("↑↓ move   ⏎ select   esc cancel"),
        ..View::default()
    };
    let mut selected = 0;
    loop {
        match view::menu(view.clone(), &mut selected, &|| false)? {
            Nav::Select(id) => return Ok(id == "continue"),
            Nav::Back | Nav::Quit => return Ok(false),
            Nav::Refresh => {}
        }
    }
}

/// One Builder per folder: `.setup-lock/` holding the owner's pid.
struct Lock(PathBuf);
impl Lock {
    fn take(root: &Path) -> Result<Self, String> {
        let lock = root.join(".setup-lock");
        // A closed terminal can end this process before Drop runs. Reclaim only
        // our own lock format with a provably exited owner, never an active setup.
        if let Ok(pid) = fs::read_to_string(lock.join("pid")) {
            if pid.trim().parse::<u32>().is_ok_and(|p| p > 0 && !console::alive(p)) {
                release_lock(&lock);
            }
        }
        fs::create_dir(&lock).map_err(|_| {
            "Makepad Builder is already using this folder. Close it before starting another.".to_owned()
        })?;
        let lock = Lock(lock);
        fs::write(lock.0.join("pid"), std::process::id().to_string()).map_err(|e| e.to_string())?;
        Ok(lock)
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        release_lock(&self.0);
    }
}

/// The lock is a folder holding only our pid file; remove_dir refuses
/// anything else.
fn release_lock(lock: &Path) {
    if let Some(root) = lock.parent() {
        let _ = crate::remove_inside(root, &lock.join("pid"));
    }
    let _ = fs::remove_dir(lock);
}

// ---- Setup state -----------------------------------------------------------

#[derive(Clone, Copy)]
struct Ready {
    tools: bool,
    rust: bool,
}
impl Ready {
    fn compiler(self) -> bool {
        self.tools && self.rust
    }
}

/// Last validation of a recorded installed Rust (macOS/Linux). Redraws only
/// confirm the binaries still exist; builds re-validate through `selected_rust`.
#[derive(Clone)]
struct RustCheck {
    recorded: String,
    version: String,
    sysroot: Option<PathBuf>,
}

#[derive(Clone)]
struct SourceCheck {
    built: std::time::SystemTime,
    at: std::time::Instant,
    changed: bool,
}

/// The source files of a Cargo dep-info file (`target: a.rs b\ c.rs`): paths
/// after the first `: `, split on spaces not escaped with a backslash; a
/// backslash before a newline continues the line.
fn dep_file_sources(text: &str) -> Vec<String> {
    let Some(first) = text.lines().next() else { return Vec::new() };
    let Some((_, deps)) = first.split_once(": ") else { return Vec::new() };
    let mut sources = Vec::new();
    let mut current = String::new();
    let mut chars = deps.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&' ') => {
                current.push(' ');
                chars.next();
            }
            ' ' => {
                if !current.is_empty() {
                    sources.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        sources.push(current);
    }
    sources
}

#[cfg(test)]
mod dep_file_tests {
    use super::dep_file_sources;

    #[test]
    fn dep_files_list_their_sources_with_escaped_spaces_and_drive_letters() {
        assert_eq!(dep_file_sources("/t/release/app: /s/a.rs /s/my\\ dir/b.rs\n\n/s/a.rs:\n"), vec!["/s/a.rs", "/s/my dir/b.rs"]);
        assert_eq!(
            dep_file_sources("C:\\Users\\p\\target\\release\\scope.exe: C:\\Users\\p\\src\\main.rs C:\\Users\\p\\makepad-builder\\ (2)\\lib.rs"),
            vec!["C:\\Users\\p\\src\\main.rs", "C:\\Users\\p\\makepad-builder (2)\\lib.rs"]
        );
    }
}

#[derive(Clone)]
struct Setup {
    root: PathBuf,
    project: PathBuf,
    service: String,
    email: String,
    app: String,
    release: Option<Release>,
    cuda: bool,
    compiler_retry: bool,
    rust_check: std::cell::RefCell<Option<RustCheck>>,
    /// Per app: whether its sources changed after its last build, rechecked
    /// when the built app changes or after a few seconds.
    source_checks: std::cell::RefCell<std::collections::HashMap<String, SourceCheck>>,
    /// Licensed releases from the last catalog check; None before one succeeded.
    licenses: Option<Vec<Release>>,
    license_error: Option<String>,
    /// The newest public Makepad release known (for the free apps).
    public: Option<Release>,
    /// The last update check or build: (downloaded updates, "HH:MM").
    checked: Option<(bool, String)>,
    disk: Arc<Mutex<(u64, Option<(u64, u64)>)>>,
    disk_seen: std::cell::Cell<u64>,
    /// Installed coding agents: (command, title).
    agents: Vec<(&'static str, &'static str)>,
    free_selected: usize,
    /// The GPU driver notice was acknowledged in this session.
    gpu_read: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    New,
    Partial,
    Compile,
    Update,
    Ready,
    Merge,
}

impl State {
    fn key(self) -> &'static str {
        match self {
            State::New => "new",
            State::Partial => "partial",
            State::Compile => "compile",
            State::Update => "update",
            State::Ready => "ready",
            State::Merge => "merge",
        }
    }
    /// Status and action texts from menu.txt: the status says where the app
    /// is in a word or two, the action (on the selected row) what Return
    /// does, which always ends with the app running. `bytes` is the
    /// download size when known.
    fn texts(self, bytes: Option<u64>) -> (view::Text, String) {
        let (status, action) = MENU
            .lines()
            .filter_map(|line| line.split_once('|'))
            .find(|(key, _)| *key == self.key())
            .and_then(|(_, rest)| rest.split_once('|'))
            .map(|(s, a)| (s.to_owned(), a.to_owned()))
            .unwrap_or_else(|| (self.key().to_owned(), "run".to_owned()));
        let status = match bytes {
            Some(bytes) => status.replace("{mb}", &bytes.div_ceil(1048576).to_string()),
            None if status == "{mb} MB" => "not downloaded".to_owned(),
            None => status.replace(" {mb} MB", ""),
        };
        let action = if action.is_empty() { "⏎".to_owned() } else { action };
        let status = match self {
            State::New => text(status, DIM),
            State::Ready => done(status),
            State::Compile => text(status, PLAIN),
            _ => text(status, WARN),
        };
        (status, action)
    }
}

/// Opening a session.
impl Setup {
    /// The session state for `root`, from what earlier runs left there.
    fn open(root: PathBuf, email: Option<String>) -> Result<Self, String> {
        let load = |path: &str| load_release(&root.join(path));
        Ok(Setup {
            project: env::var_os("MAKEPAD_LOADER_PROJECT")
                .map(PathBuf::from)
                .unwrap_or_else(|| root.clone())
                .canonicalize()
                .map_err(|e| e.to_string())?,
            service: env::var("MAKEPAD_LOADER_SERVICE").unwrap_or_else(|_| catalog::DEFAULT_SERVICE.into()),
            email: match email {
                Some(email) if !email.is_empty() => catalog::email(&email)?,
                _ => String::new(),
            },
            app: "scope".into(),
            release: load("available/scope.json")
                .or_else(|| load("latest.json").filter(|r| r.id == "scope"))
                .or_else(|| load("installed/scope.json")),
            public: fs::read_dir(root.join("available"))
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| load_release(&entry.path()))
                .filter(|release| release.public)
                .max_by(|a, b| a.release.cmp(&b.release)),
            cuda: crate::cuda::build_with(&root),
            compiler_retry: false,
            rust_check: Default::default(),
            source_checks: Default::default(),
            licenses: None,
            license_error: None,
            checked: None,
            disk: Arc::new(Mutex::new((0, None))),
            disk_seen: std::cell::Cell::new(0),
            agents: [("claude", "Claude Code"), ("codex", "Codex"), ("grok", "Grok")]
                .into_iter()
                .filter(|(command, _)| on_path(command))
                .collect(),
            free_selected: 0,
            gpu_read: false,
            root,
        })
    }
}

/// The main menu: rows, and what Return does on each.
impl Setup {
    /// YOUR APPS first, then the experiments, the coding agents and setup.
    /// There are no compiler rows: an app sets up what it needs when it is
    /// first built, and on Windows with an NVIDIA GPU building Makepad Amp
    /// asks once whether to turn on local AI (see `local_ai_screen`).
    fn main_view(&self) -> View {
        let mut rows = vec![Row::Head("YOUR APPS".into())];
        rows.extend(self.license_rows());
        let free = free_apps().unwrap_or_default();
        let built = free.iter().filter(|(id, _)| self.app_state(id, self.app_release(id).as_ref()) == State::Ready).count();
        rows.push(Row::Head("MAKEPAD EXPERIMENTS".into()));
        rows.push(item("free", "Experiments", "free", text(format!("{} · {built} ready", free.len()), PLAIN), "open list"));
        rows.push(Row::Head("CODING AGENTS · each one knows how to change and rebuild these apps".into()));
        for (command, title) in &self.agents {
            rows.push(item(format!("agent-{command}"), *title, "", Vec::new(), "open"));
        }
        rows.push(item("agent-shell", "Shell", "", text("with this folder's Rust on PATH", DIM), "open"));
        rows.push(Row::Head("SETUP".into()));
        rows.push(self.account_row());
        if !cfg!(target_os = "macos") {
            rows.push(self.graphics_row());
        }
        rows.push(self.agreements_row());
        rows.push(self.disk_row());
        rows.push(match &self.checked {
            None => item("updates", "Update", "", text("check for updates", DIM), "⏎"),
            Some((false, time)) => item("updates", "Update", "", done(format!("up to date · {time}")), "check again"),
            Some((true, time)) => item("updates", "Update", "", text(format!("updates downloaded · {time}"), PLAIN), "check again"),
        });
        View {
            subtitle: ABOUT.trim().into(),
            email: self.shown_email(),
            rows,
            ..View::default()
        }
    }
    fn account_row(&self) -> Row {
        if self.email.is_empty() {
            item("account", "Account", "", text("not logged in", WARN), "log in")
        } else {
            // The email proves itself by loading its license catalog.
            let status = match (&self.licenses, &self.license_error) {
                (Some(_), _) => done(self.email.clone()),
                (None, Some(error)) => vec![
                    view::Span(format!("{} ", self.email), PLAIN),
                    view::Span(format!("· {}", short_reason(error)), WARN),
                ],
                (None, None) => vec![view::Span(format!("{} ", self.email), PLAIN), view::Span("checking…".into(), DIM)],
            };
            item("account", "Account", "", status, "switch or log out")
        }
    }
    fn graphics_row(&self) -> Row {
        if self.gpu_read {
            item("gpu", "Graphics", "", done("driver notice read"), "")
        } else {
            item("gpu", "Graphics", "", text("read the driver notice", WARN), "read")
        }
    }
    /// ✓ once every agreement the build tools need is accepted (on the
    /// Agreements screen or a consent screen); NVIDIA joins when CUDA's is.
    fn agreements_row(&self) -> Row {
        let accepted = self.accepted_agreements();
        let required = required_for(&self.root);
        let status = if required.iter().all(|id| accepted.iter().any(|a| a == id)) {
            let mut names = vec!["Makepad"];
            if cfg!(windows) && accepted.iter().any(|a| a == "vs") {
                names.push("Microsoft");
            }
            names.push("Rust");
            if accepted.iter().any(|a| a == "cuda") {
                names.push("NVIDIA");
            }
            vec![view::Span("✓".into(), OK), view::Span(" accepted ".into(), PLAIN), view::Span(format!("· {}", names.join(", ")), DIM)]
        } else {
            text("not accepted", WARN)
        };
        item("terms", "Agreements", "", status, "read")
    }
    fn license_rows(&self) -> Vec<Row> {
        if self.email.is_empty() {
            return vec![item("login", "Log in with your email address", "", Vec::new(), "to see your licenses")];
        }
        match (&self.licenses, &self.license_error) {
            (Some(list), _) if list.is_empty() => vec![Row::Note(text("none on this email yet · buy or request beta access at makepad.nl", DIM))],
            (Some(list), _) => list.iter().map(|release| self.app_row(release)).collect(),
            (None, None) => vec![Row::Note(text(format!("checking licenses for {}…", self.email), DIM))],
            (None, Some(error)) => {
                // Offline: the apps cached from the last check still run.
                let mut rows = vec![Row::Note(text(format!("licenses not checked: {}", clean(error.lines().next().unwrap_or_default())), WARN))];
                let cached = fs::read_dir(self.root.join("available")).into_iter().flatten().flatten()
                    .filter_map(|e| load_release(&e.path()))
                    .filter(|r| !r.public);
                rows.extend(cached.map(|release| self.app_row(&release)));
                rows
            }
        }
    }
    fn app_row(&self, release: &Release) -> Row {
        let state = self.app_state(&release.id, Some(release));
        let (mut status, mut action) = state.texts(Some(release.repositories.iter().map(|r| r.bytes).sum()));
        if !release.supported() {
            status = text("not available for this platform yet", DIM);
            action = String::new();
        }
        let license = if release.license == "beta" { "beta" } else { "commercial" };
        item(format!("app:{}", release.id), release.title.clone(), license, status, action)
    }
    fn disk_row(&self) -> Row {
        // Folder total, then what "clear build data" can delete.
        match self.disk.lock().ok().and_then(|slot| slot.1) {
            None => item("disk", "Disk", "", text("measuring…", DIM), ""),
            Some((total, 0)) => item("disk", "Disk", "", text(format!("{} GB used", gb(total)), PLAIN), ""),
            Some((total, build)) => item(
                "disk",
                "Disk",
                "",
                vec![view::Span(format!("{} GB used ", gb(total)), PLAIN), view::Span(format!("· build {} GB", gb(build)), DIM)],
                "clear build data",
            ),
        }
    }
    /// Index of the first licensed app row, where the menu starts.
    fn first_license_row(&self, view: &View) -> usize {
        let mut index = 0;
        let mut in_licenses = false;
        for row in &view.rows {
            match row {
                Row::Head(title) => in_licenses = title == "YOUR APPS",
                Row::Item(_) if in_licenses => return index,
                Row::Item(_) => index += 1,
                Row::Note(_) => {}
            }
        }
        0
    }
    fn select(&mut self, id: &str) -> Result<(), String> {
        match id {
            "account" | "login" => self.switch_account(),
            "gpu" => self.graphics_screen(),
            "updates" => self.check_updates(),
            "disk" => self.clear_build(),
            "terms" => self.agreements_screen(),
            "free" => self.experiments_screen(),
            "agent-shell" => self.shell(),
            agent if agent.starts_with("agent-") => self.launch_agent(&agent["agent-".len()..], None, None),
            app => self.open_app(app.trim_start_matches("app:")),
        }
    }
    fn report(&self, error: String) {
        let error = if self.email.is_empty() { error } else { error.replace(&self.email, "[email]") };
        view::warn(&error);
    }
    /// The header's right side.
    fn shown_email(&self) -> String {
        if self.email.is_empty() { "not logged in".into() } else { self.email.clone() }
    }
}

/// Account and licenses.
impl Setup {
    /// First run: the Log in screen with an inline email editor. Empty
    /// continues with the open source experiments.
    fn log_in_screen(&mut self) -> Result<(), String> {
        let mut rows = vec![Row::Note(Vec::new())];
        rows.extend(wrap("Enter the email address you bought your Makepad product with, or have beta access for. Leave it empty for the free apps.", 74).into_iter().map(|line| Row::Note(text(line, PLAIN))));
        let mut hint = Vec::new();
        let mut typed = String::new();
        loop {
            // Redrawn each time: the license check in between shows its busy line.
            view::set_view(View {
                crumb: " › Log in".into(),
                subtitle: "Welcome to the Makepad Builder.".into(),
                email: self.shown_email(),
                rows: rows.clone(),
                back: true,
                ..View::default()
            });
            let Some(value) = view::edit("Email:", &typed, hint, "type your email   ⏎ continue")? else { return Ok(()) };
            if value.is_empty() {
                return Ok(());
            }
            typed = value.clone();
            let Ok(email) = catalog::email(&value) else {
                hint = text("That does not look like an email address.", WARN);
                continue;
            };
            // Check it now, on this page: a typo comes straight back to the editor.
            self.email = email;
            match self.check_licenses_on(false) {
                Err(error) if email_rejected(&error) => {
                    self.email.clear();
                    hint = rejected_hint(&value, &error);
                }
                _ => {
                    self.save_email();
                    return Ok(());
                }
            }
        }
    }
    /// The license check refused the email (unknown or not enabled): keep the
    /// typed text in the editor with the reason in yellow above the keys, and
    /// check again on Return. Escape keeps the email that was there before.
    fn correct_rejected_email(&mut self, error: String, keep: String) -> Result<(), String> {
        let mut typed = self.email.clone();
        let mut error = error;
        loop {
            let hint = rejected_hint(&typed, &error);
            let Some(value) = view::edit("Email for your licenses:", &typed, hint, "⏎ check again   esc keep")? else {
                // Back to the email that was there; its own check shows its state.
                if self.email != keep {
                    self.email = keep;
                    let _ = self.refresh_licenses();
                }
                return Ok(());
            };
            typed = value.clone();
            let Ok(email) = catalog::email(&value) else {
                error = "That does not look like an email address.".into();
                continue;
            };
            self.email = email;
            match self.refresh_licenses() {
                Err(next) if email_rejected(&next) => error = next,
                Err(next) => return Err(next),
                Ok(()) => {
                    self.save_email();
                    let count = self.licenses.as_ref().map_or(0, Vec::len);
                    view::message(done(format!("Logged in as {} · {count} license{}.", self.email, if count == 1 { "" } else { "s" })));
                    return Ok(());
                }
            }
        }
    }
    /// Check the licenses; a refused email opens the editor to correct it.
    fn check_licenses(&mut self) -> Result<(), String> {
        match self.refresh_licenses() {
            Err(error) if email_rejected(&error) => {
                let keep = self.email.clone();
                self.correct_rejected_email(error, keep)
            }
            other => other,
        }
    }
    fn switch_account(&mut self) -> Result<(), String> {
        if !self.email.is_empty() {
            match view::choose(&format!("Logged in as {}.", self.email), "", &["switch email", "log out"], 0)?.as_deref() {
                Some("log out") => return self.log_out(),
                Some(_) => {}
                None => return Ok(()),
            }
        }
        let (prompt, hint) = if self.email.is_empty() {
            ("Email you bought a Makepad app with:", Vec::new())
        } else {
            ("Email for your licenses:", text(format!("⏎ keeps {}", self.email), DIM))
        };
        let Some(value) = view::edit(prompt, "", hint, "⏎ log in   esc back")? else { return Ok(()) };
        if value.is_empty() || value == self.email {
            return Ok(());
        }
        let before = std::mem::replace(&mut self.email, catalog::email(&value).map_err(|_| "That does not look like an email address.".to_owned())?);
        self.release = None;
        if let Err(error) = self.refresh_licenses() {
            if !email_rejected(&error) {
                return Err(error);
            }
            return self.correct_rejected_email(error, before);
        }
        self.save_email();
        let count = self.licenses.as_ref().map_or(0, Vec::len);
        view::message(done(format!("Switched to {} · {count} license{}.", self.email, if count == 1 { "" } else { "s" })));
        Ok(())
    }
    fn ask_email(&mut self) -> Result<bool, String> {
        let Some(value) = view::edit("Email you bought a Makepad app with:", "", Vec::new(), "⏎ log in   esc back")? else { return Ok(false) };
        if value.is_empty() {
            return Ok(false);
        }
        self.email = catalog::email(&value).map_err(|_| "That does not look like an email address.".to_owned())?;
        self.save_email();
        Ok(true)
    }
    /// Forget the email in this folder: only the Makepad experiments show
    /// from then on. Licensed apps already built stay on disk untouched and
    /// come back when the person logs in again.
    fn log_out(&mut self) -> Result<(), String> {
        let was = std::mem::take(&mut self.email);
        self.release = None;
        self.licenses = Some(Vec::new());
        self.license_error = None;
        self.save_email();
        let _ = fs::write(self.root.join(".login-asked"), "");
        view::set_view(self.main_view());
        view::message(vec![
            view::Span("✓ ".into(), OK),
            view::Span(format!("Logged out of {was}. "), PLAIN),
            view::Span("Only the Makepad experiments are shown; log in again from the Account row.".into(), DIM),
        ]);
        Ok(())
    }
    /// Keep a switched email for the next start, in the folder's own bootstrap file.
    fn save_email(&self) {
        // Logged out: the bootstrap keeps no address, and names the free
        // apps (an empty email is only valid for those).
        let app = makepad_loader_bundle::load_from(&self.root).ok().flatten().map_or_else(|| self.app.clone(), |b| b.app);
        let app = if self.email.is_empty() { "makepad".to_owned() } else { app };
        match makepad_loader_bundle::Bootstrap::new(&self.email, &app) {
            Ok(bootstrap) => {
                if let Err(error) = fs::write(self.root.join(makepad_loader_bundle::BOOTSTRAP_FILE), bootstrap.encode()) {
                    activity(&format!("Email not saved for the next start: {error}"));
                }
            }
            Err(error) => activity(&format!("Email not saved for the next start: {error}")),
        }
    }
    /// The licensed apps for this email, from the catalog. Their releases
    /// are cached in `available/` for launches.
    fn refresh_licenses(&mut self) -> Result<(), String> {
        self.check_licenses_on(true)
    }
    /// `on_menu`: show the check on the main menu. The log-in screen checks
    /// on its own page, so the menu never flashes up between log-in and setup.
    fn check_licenses_on(&mut self, on_menu: bool) -> Result<(), String> {
        if self.email.is_empty() {
            self.licenses = Some(Vec::new());
            return Ok(());
        }
        self.licenses = None;
        self.license_error = None;
        if on_menu {
            view::set_view(self.main_view());
        }
        view::busy(&format!("Checking licenses for {}", self.email));
        let releases = match with_progress(|| catalog::fetch(&self.service, &self.email)) {
            Ok(releases) => releases,
            Err(error) => {
                self.license_error = Some(error.clone());
                return Err(error);
            }
        };
        let licensed: Vec<Release> = releases.into_iter().filter(|r| !r.public).collect();
        fs::create_dir_all(self.root.join("available")).map_err(|e| e.to_string())?;
        for release in &licensed {
            release.save(&self.root.join("available").join(format!("{}.json", release.id)))?;
        }
        self.release = licensed.iter().find(|r| r.id == self.app).cloned();
        if let Some(release) = &self.release {
            release.save(&self.root.join("latest.json"))?;
        }
        self.licenses = Some(licensed);
        Ok(())
    }
    fn licensed(&self, app: &str) -> bool {
        self.licenses.as_ref().is_some_and(|list| list.iter().any(|r| r.id == app))
    }
    fn release(&mut self) -> Result<Release, String> {
        if let Some(release) = &self.release {
            return Ok(release.clone());
        }
        self.refresh()
    }
    fn refresh(&mut self) -> Result<Release, String> {
        let public = is_public(&self.app);
        if !public && self.email.is_empty() && !self.ask_email()? {
            return Err("Log in with the email that has this app's license.".into());
        }
        let release = with_progress(|| {
            progress::stage(
                "Checking sources",
                "The source service may refresh its Git cache while we wait",
                0.0,
            );
            if !public {
                catalog::fetch(&self.service, &self.email)?.into_iter().find(|r| r.id == self.app).ok_or("This app is not available for this email".into())
            } else {
                let app = catalog::apps()?.into_iter().find(|a| a.get("id").and_then(makepad_strict_json::Value::as_str) == Some(&self.app)).ok_or("Unknown public app")?;
                catalog::fetch_public(&self.service)?.for_app(&app)
            }
        })?;
        if !release.supported() {
            return Err("No release for this platform yet".into());
        }
        fs::create_dir_all(self.root.join("available")).map_err(|e| e.to_string())?;
        release.save(&self.root.join("available").join(format!("{}.json", self.app)))?;
        release.save(&self.root.join("latest.json"))?;
        self.release = Some(release.clone());
        Ok(release)
    }
}

/// Graphics notice, license agreements and consent.
impl Setup {
    /// The Graphics row: the GPU driver notice, read again on request.
    fn graphics_screen(&mut self) -> Result<(), String> {
        // Reading it again changes nothing unless it was never read; Cancel
        // does not un-read it.
        if gpu_notice(&self.shown_email())? && !self.gpu_read {
            record_gpu_notice(&self.root)?;
            self.gpu_read = true;
        }
        Ok(())
    }
    /// The Agreements page: each agreement with its state (Return opens it),
    /// then Agree to all and Disagree. Disagreeing only withdraws the record:
    /// nothing installed is removed, and the next install or update asks again.
    fn agreements_screen(&self) -> Result<(), String> {
        let mut selected = 0;
        loop {
            let accepted = self.accepted_agreements();
            let mut rows: Vec<Row> = agreements()
                .into_iter()
                .map(|(id, name, url)| {
                    let state = if accepted.iter().any(|a| a == id) {
                        vec![view::Span("✓".into(), OK), view::Span(" accepted  ".into(), PLAIN), view::Span(host(url).into(), DIM)]
                    } else {
                        vec![view::Span("not accepted  ".into(), WARN), view::Span(host(url).into(), DIM)]
                    };
                    item(format!("url:{id}"), format!("{name:<36}"), "", state, "open in browser")
                })
                .collect();
            rows.push(Row::Note(Vec::new()));
            rows.push(item("agree", "Agree to all", "", Vec::new(), "accept every agreement above"));
            rows.push(item("disagree", "Disagree", "", Vec::new(), "withdraw acceptance"));
            let view = View {
                crumb: " › License agreements".into(),
                subtitle: "Return opens an agreement in your browser.".into(),
                email: self.shown_email(),
                rows,
                back: true,
                ..View::default()
            };
            match view::menu(view, &mut selected, &|| false)? {
                Nav::Select(id) if id == "agree" => {
                    let ids: Vec<&str> = agreements().iter().map(|a| a.0).collect();
                    self.record_agreements(&ids)?;
                    view::message(done("Agreements accepted."));
                }
                Nav::Select(id) if id == "disagree" => {
                    self.record_agreements(&[])?;
                    view::message(text("Agreements withdrawn: installing or updating build tools will ask again.", WARN));
                }
                Nav::Select(id) => open_agreement(&id),
                Nav::Back => return Ok(()),
                Nav::Quit => return Err(QUIT.into()),
                Nav::Refresh => {}
            }
        }
    }
    /// A consent screen of its own for the licenses a step needs: what it
    /// installs and where, each agreement by name (Return opens it), then
    /// "Agree to all" (selected at the start) and Cancel. Escape cancels.
    /// Agreements already accepted are not asked again; agreeing records them.
    fn consent(&self, title: &str, intro: &str, detail: &str, ids: &[&str], action: &str) -> Result<bool, String> {
        let accepted = self.accepted_agreements();
        if ids.iter().all(|id| accepted.iter().any(|a| a == id)) {
            return Ok(true);
        }
        let mut selected = agreement_rows(Some(ids)).len();
        loop {
            let mut rows = vec![Row::Note(Vec::new())];
            rows.extend(wrap(intro, 74).into_iter().map(|line| Row::Note(text(line, PLAIN))));
            if !detail.is_empty() {
                rows.push(Row::Note(text(detail, DIM)));
            }
            rows.push(Row::Head("READ THE AGREEMENTS".into()));
            rows.extend(agreement_rows(Some(ids)));
            rows.push(Row::Note(Vec::new()));
            rows.push(item("agree", "Agree to all", "", Vec::new(), action));
            rows.push(item("cancel", "Cancel", "", Vec::new(), ""));
            let view = View {
                crumb: format!(" › {title}"),
                subtitle: "Please read what you are agreeing to.".into(),
                email: self.shown_email(),
                rows,
                back: true,
                footer: Some("↑↓ move   ⏎ select   esc cancel"),
                ..View::default()
            };
            match view::menu(view, &mut selected, &|| false)? {
                Nav::Select(id) if id == "agree" => {
                    let mut all: Vec<&str> = accepted.iter().map(String::as_str).collect();
                    all.extend(ids.iter().filter(|id| !accepted.iter().any(|a| a == *id)));
                    self.record_agreements(&all)?;
                    return Ok(true);
                }
                Nav::Select(id) if id == "cancel" => return Ok(false),
                Nav::Select(id) => open_agreement(&id),
                Nav::Back | Nav::Quit => return Ok(false),
                Nav::Refresh => {}
            }
        }
    }
    /// Accepted agreement ids, from `agreements-accepted` in the folder.
    /// Folders set up before that record existed accepted the build tools'
    /// agreements when installing them (and NVIDIA's with CUDA).
    fn accepted_agreements(&self) -> Vec<String> {
        match fs::read_to_string(self.root.join("agreements-accepted")) {
            Ok(record) => record.split_whitespace().map(str::to_owned).collect(),
            Err(_) => {
                let mut ids = Vec::new();
                if self.ready().compiler() {
                    ids.extend(required_for(&self.root).iter().map(|id| id.to_string()));
                }
                if self.cuda_installed() {
                    ids.push("cuda".into());
                }
                ids
            }
        }
    }
    /// Replace the record; an empty list means withdrawn.
    fn record_agreements(&self, ids: &[&str]) -> Result<(), String> {
        let next = self.root.join("agreements-accepted.next");
        fs::write(&next, ids.join("\n") + "\n").map_err(|e| e.to_string())?;
        fs::rename(&next, self.root.join("agreements-accepted")).map_err(|e| e.to_string())
    }
}

/// Compiler and platform tools.
impl Setup {
    /// What compiling needs, checked on disk: the platform tools (Microsoft
    /// tools and SDK on Windows, Apple's or the distribution's elsewhere) and
    /// the pinned Rust.
    fn ready(&self) -> Ready {
        let tools = if cfg!(windows) {
            match runtime::windows_chain(&self.root) {
                // Rust's GNU toolchain is the whole compiler.
                WindowsChain::Gnu => true,
                WindowsChain::Msvc => crate::msvc::ready(&self.root.join("toolchain/msvc")),
                WindowsChain::Undecided => false,
            }
        } else {
            runtime::system_tools_ready().is_ok()
        };
        let rust = self.pinned_rust().is_some_and(|version| self.rust_ready(&version));
        Ready { tools, rust }
    }
    fn pinned_rust(&self) -> Option<String> {
        self.release.as_ref().or(self.public.as_ref()).map(|r| r.rust.clone())
    }
    /// Everything compiling needs, set up in order: the consent screen for
    /// what is missing, Apple's tools on macOS, then the downloads. True when
    /// the compiler is ready; false when the person cancelled.
    fn install_compiler(&mut self, release: &Release) -> Result<bool, String> {
        if self.compiler_retry {
            return self.retry_compiler(release);
        }
        // Windows with an NVIDIA GPU: the first build of an app with AI
        // features asks once, and the answer picks the compiler for good:
        // Microsoft's tools with CUDA, or Rust's GNU toolchain without.
        if self.local_ai_undecided(release) {
            let Some(chain) = self.local_ai_screen()? else {
                return Ok(self.nothing_installed());
            };
            runtime::record_windows_chain(&self.root, chain)?;
            self.cuda = crate::cuda::build_with(&self.root);
        }
        let mut ready = self.ready();
        if !cfg!(windows) && !ready.rust && self.choose_rust(&release.rust)? {
            ready = self.ready();
        }
        let cuda = self.cuda_wanted();
        if ready.compiler() && !cuda {
            return Ok(true);
        }
        if !self.compiler_consent(ready, release, cuda)? {
            return Ok(self.nothing_installed());
        }
        if !cfg!(windows) && !ready.tools {
            if cfg!(target_os = "macos") {
                // Apple's tools come right after the agreements.
                if !self.xcode_screen()? {
                    return Ok(self.nothing_installed());
                }
            } else {
                let _pause = Screen::pause();
                runtime::setup_system_tools()?;
            }
        }
        // The components that actually run, one line each on the work page;
        // they install side by side, each row with its own bar.
        let mut steps = Vec::new();
        let mut kinds = Vec::new();
        if cfg!(windows) && !ready.tools {
            let msvc = self.root.join("toolchain/msvc");
            if !crate::msvc::build_tools_ready(&msvc) {
                steps.push("Build tools");
            }
            if !crate::msvc::sdk_ready(&msvc) {
                steps.push("Windows SDK");
            }
            kinds.push(Dependency::Msvc);
        }
        if !ready.rust {
            steps.push("Rust");
            kinds.push(Dependency::Rust);
        }
        if cuda {
            steps.push("CUDA");
            kinds.push(Dependency::Cuda);
        }
        let row = if cfg!(windows) { "tools" } else { "rust" };
        view::work_begin("Install build tools", "This takes a few minutes; everything goes into this folder.", &self.shown_email(), row, &steps);
        crate::timing::reset();
        let result: Result<bool, String> = with_progress(|| {
            runtime::dependencies(&self.root, release, &kinds).map(|()| true)
        });
        for line in crate::timing::report() {
            activity(&line);
        }
        self.cuda = crate::cuda::build_with(&self.root);
        let result = match result {
            Err(error) if rustc::needs_compiler_retry(&error) => {
                self.compiler_retry = true;
                Err(STILL_SCANNING.into())
            }
            other => other,
        };
        view::work_end(result)
    }
    /// The local AI question is due: Windows, an NVIDIA driver, an app that
    /// uses CUDA, and no compiler chosen yet (older folders that already
    /// have Microsoft's tools count as chosen).
    fn local_ai_undecided(&self, release: &Release) -> bool {
        cfg!(windows)
            && release.cuda
            && crate::cuda::gpu_present()
            && !self.root.join("selected-compiler").is_file()
            && !crate::msvc::ready(&self.root.join("toolchain/msvc"))
    }
    /// One page: the three agreements (Return opens one in the browser),
    /// then "Agree and enable local AI" (selected) and "No local AI".
    /// Agreeing records the agreements and Microsoft's tools with CUDA; No
    /// keeps Rust's GNU toolchain. Neither is asked again. Escape goes back
    /// to the menu without deciding.
    fn local_ai_screen(&self) -> Result<Option<WindowsChain>, String> {
        let ids = ["cuda", "vs", "sdk"];
        let mut selected = ids.len();
        loop {
            let mut rows = vec![Row::Note(Vec::new()), Row::Note(done("NVIDIA GPU found")), Row::Note(Vec::new())];
            rows.extend(
                wrap("Local AI support requires Microsoft Build Tools and NVIDIA CUDA, which have their own license agreements you need to agree to.", 74)
                    .into_iter()
                    .map(|line| Row::Note(text(line, PLAIN))),
            );
            rows.push(Row::Note(Vec::new()));
            rows.extend(agreement_rows(Some(&ids)));
            rows.push(Row::Note(Vec::new()));
            rows.push(item("agree", "Agree and enable local AI", "", Vec::new(), ""));
            rows.push(item("no", "No local AI", "", Vec::new(), ""));
            let view = View {
                crumb: " › Local AI".into(),
                subtitle: "You can run local AI functionality.".into(),
                email: self.shown_email(),
                rows,
                back: true,
                footer: Some("↑↓ move   ⏎ select   esc back"),
                ..View::default()
            };
            match view::menu(view, &mut selected, &|| false)? {
                Nav::Select(id) if id == "agree" => {
                    let mut all = self.accepted_agreements();
                    for id in ["makepad", "rust", "vs", "sdk", "cuda"] {
                        if !all.iter().any(|a| a == id) {
                            all.push(id.into());
                        }
                    }
                    self.record_agreements(&all.iter().map(String::as_str).collect::<Vec<_>>())?;
                    activity("Local AI on: Microsoft's C++ tools and CUDA.");
                    return Ok(Some(WindowsChain::Msvc));
                }
                Nav::Select(id) if id == "no" => {
                    activity("No local AI: Rust's GNU toolchain.");
                    return Ok(Some(WindowsChain::Gnu));
                }
                Nav::Select(id) => open_agreement(&id),
                Nav::Back | Nav::Quit => return Ok(None),
                Nav::Refresh => {}
            }
        }
    }
    fn nothing_installed(&self) -> bool {
        activity("Compiler installation cancelled.");
        view::set_view(self.main_view());
        view::message(text("Nothing was installed.", DIM));
        false
    }
    /// Windows security software can hold a freshly unpacked rustc for a
    /// while; the staged copy is checked again without downloading.
    fn retry_compiler(&mut self, release: &Release) -> Result<bool, String> {
        activity("Retrying the staged Rust compiler check; no download is needed.");
        view::busy("Checking the staged Rust compiler");
        match rustc::retry_staged(&runtime::rust_dir(&self.root, &release.rust), &release.rust) {
            Ok(()) => {
                self.compiler_retry = false;
                activity("Compiler is ready.");
                Ok(true)
            }
            Err(error) if rustc::needs_compiler_retry(&error) => Err(STILL_SCANNING.into()),
            Err(error) => {
                self.compiler_retry = false;
                Err(error)
            }
        }
    }
    /// One consent screen covers every missing piece and its licenses.
    fn compiler_consent(&self, ready: Ready, release: &Release, cuda: bool) -> Result<bool, String> {
        let mut parts = Vec::new();
        if cfg!(windows) && !ready.tools {
            parts.push("Microsoft C++ Build Tools, Windows SDK".to_owned());
        }
        if !ready.rust {
            parts.push(format!("Rust {}", release.rust));
        }
        if cuda {
            parts.push(format!("NVIDIA CUDA Toolkit {} for your NVIDIA card (AI features in Makepad Amp)", crate::cuda::CUDA_VERSION));
        }
        let gnu = cfg!(windows) && runtime::windows_chain(&self.root) == WindowsChain::Gnu;
        let intro = match (cfg!(windows) || ready.tools, cfg!(target_os = "macos")) {
            (true, _) if gnu => "Makepad compiles from source with Rust's GNU toolchain.",
            (true, _) => "Makepad compiles from source, so it needs a compiler.",
            (false, true) => "Makepad compiles from source, so it needs a compiler. Apple's developer tools come from Apple's installer, which shows its own license.",
            (false, false) => "Makepad compiles from source, so it needs a compiler. Development packages come from your distribution's package manager.",
        };
        let detail = if parts.is_empty() { String::new() } else { format!("Installed in this folder only: {}", parts.join(", ")) };
        let mut ids = required_for(&self.root).to_vec();
        if cuda {
            ids.push("cuda");
        }
        let action = match (!ready.tools, !ready.rust, cuda) {
            (true, _, true) => "install the build tools, Rust and CUDA",
            (true, _, false) => "install the build tools and Rust",
            (false, true, true) => "install Rust and CUDA",
            (false, true, false) => "install Rust",
            (false, false, _) => "install CUDA",
        };
        self.consent("Install build tools", intro, &detail, &ids, action)
    }
    fn rust_ready(&self, version: &str) -> bool {
        // Windows never reads the choice file: private pinned Rust only.
        if cfg!(windows) {
            return runtime::rust_ready(&self.root, version);
        }
        let recorded = fs::read_to_string(self.root.join("selected-rust")).unwrap_or_default().trim().to_owned();
        if !recorded.starts_with('/') {
            self.rust_check.borrow_mut().take();
            return runtime::rust_ready(&self.root, version);
        }
        let mut cache = self.rust_check.borrow_mut();
        if let Some(check) = cache.as_ref().filter(|c| c.recorded == recorded && c.version == version) {
            return check.sysroot.as_ref().is_some_and(|s| {
                s.join("bin").join(runtime::exe("cargo")).is_file() && s.join("bin").join(runtime::exe("rustc")).is_file()
            });
        }
        let sysroot = runtime::validate_rust(version, &recorded).ok();
        let ready = sysroot.is_some();
        *cache = Some(RustCheck { recorded, version: version.to_owned(), sysroot });
        ready
    }
    /// macOS/Linux only: repair a recorded installed Rust that stopped
    /// validating, or make the same one-time offer as the bootstrap. Every
    /// change of the recorded choice is an explicit answer; backing out keeps
    /// the private default. Returns true when the record changed. Windows
    /// never reaches this.
    fn choose_rust(&self, version: &str) -> Result<bool, String> {
        let stale = match runtime::rust_choice(&self.root)? {
            RustChoice::Private => return Ok(false),
            RustChoice::External(recorded) => match runtime::validate_rust(version, &recorded) {
                Ok(_) => return Ok(false),
                Err(reason) => Some((recorded, reason)),
            },
            RustChoice::Undecided => None,
        };
        if let Some((recorded, reason)) = &stale {
            activity(&format!("The selected Rust at {recorded} cannot be used: {reason}"));
        }
        match runtime::probe_rust(version) {
            Ok(candidate) => {
                let sysroot = candidate.to_string_lossy().into_owned();
                let question = format!("Use your installed Rust, or a private Rust {version} in this folder?");
                let note = format!("installed: {} (not modified)", short_path(&candidate));
                let choice = match view::choose(&question, &note, &["private", "installed"], 0)?.as_deref() {
                    Some("installed") => {
                        activity("Using the installed Rust.");
                        RustChoice::External(sysroot)
                    }
                    Some(_) => {
                        activity("Using a private Rust in this folder.");
                        RustChoice::Private
                    }
                    None => return Ok(false),
                };
                runtime::record_rust_choice(&self.root, &choice)?;
                Ok(true)
            }
            Err(reason) => {
                let Some((recorded, _)) = stale else {
                    // Nothing to offer and nothing recorded: the private flow
                    // continues and the offer returns once a compatible Rust exists.
                    activity(&format!("Installed Rust not used: {reason}"));
                    return Ok(false);
                };
                let question = format!("The selected Rust at {recorded} cannot be used. Switch to a private Rust in this folder?");
                if view::choose(&question, "", &["switch", "keep"], 0)?.as_deref() == Some("switch") {
                    runtime::record_rust_choice(&self.root, &RustChoice::Private)?;
                    activity("Switching to a private Rust in this folder.");
                    Ok(true)
                } else {
                    Err(format!("Kept the selected Rust at {recorded}. Make it available again, or select the app again to switch."))
                }
            }
        }
    }
    /// macOS: the "› Apple developer tools" screen when clang, the SDK or
    /// git are missing, or Xcode's license is not accepted. Installing opens
    /// Apple's installer and waits for it; the license runs Apple's own
    /// `sudo xcodebuild -license` on the real terminal. Never accepts on the
    /// person's behalf. True once the tools are ready.
    fn xcode_screen(&mut self) -> Result<bool, String> {
        let mut selected = 0;
        loop {
            if runtime::system_tools_ready().is_ok() {
                return Ok(true);
            }
            let license = xcode_license_pending();
            let (intro, note, first) = if license {
                (
                    "Xcode is installed, but its license has not been accepted yet, so Apple's compiler will not run.",
                    "Apple shows the license here in the terminal and asks for your password; type agree at the end to accept it.",
                    item("license", format!("{:<34}", "Read and accept the Xcode license"), "", Vec::new(), "sudo xcodebuild -license"),
                )
            } else {
                (
                    "Makepad compiles with Apple's command line developer tools: clang, the macOS SDK and git. They are not installed on this Mac yet.",
                    "Apple's installer opens in its own window (about 1 GB); come back here when it has finished.",
                    item("install", format!("{:<34}", "Install the developer tools"), "", Vec::new(), "opens Apple's installer"),
                )
            };
            let mut rows = vec![Row::Note(Vec::new())];
            rows.extend(wrap(intro, 74).into_iter().map(|line| Row::Note(text(line, PLAIN))));
            rows.push(Row::Note(Vec::new()));
            rows.extend(wrap(note, 74).into_iter().map(|line| Row::Note(text(line, DIM))));
            rows.push(Row::Note(Vec::new()));
            rows.push(first);
            rows.push(item("check", "Check again", "", Vec::new(), ""));
            rows.push(item("cancel", "Cancel", "", Vec::new(), ""));
            let view = View {
                crumb: " › Apple developer tools".into(),
                subtitle: "Needed to compile on macOS.".into(),
                email: self.shown_email(),
                rows,
                back: true,
                footer: Some("↑↓ move   ⏎ select   esc cancel"),
                ..View::default()
            };
            match view::menu(view, &mut selected, &|| false)? {
                Nav::Select(id) if id == "install" => {
                    let _ = Command::new("/usr/bin/xcode-select").arg("--install").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
                    wait_for_apple_installer()?;
                }
                Nav::Select(id) if id == "license" => {
                    let _pause = Screen::pause();
                    println!("Apple's Xcode license follows. Type agree at the end to accept it.\n");
                    let _ = Command::new("sudo").args(["/usr/bin/xcodebuild", "-license"]).status();
                }
                Nav::Select(id) if id == "check" => {
                    view::busy("Checking clang, the macOS SDK, the linker and git");
                    if runtime::system_tools_ready().is_err() {
                        view::message(text("Still not ready.", WARN));
                    }
                }
                Nav::Select(_) | Nav::Back | Nav::Quit => return Ok(false),
                Nav::Refresh => {}
            }
        }
    }
    /// An NVIDIA card without the toolkit: CUDA joins the build tools.
    fn cuda_wanted(&self) -> bool {
        crate::cuda::gpu_present() && !self.cuda_installed() && runtime::windows_chain(&self.root) != WindowsChain::Gnu
    }
    fn cuda_installed(&self) -> bool {
        self.root.join("toolchain/cuda/bin").join(runtime::exe("nvcc")).is_file()
    }
}

/// Apps: their state, and downloading, compiling and running them.
impl Setup {
    /// Makepad apps: one scrolling list; each compiles on first run.
    fn experiments_screen(&mut self) -> Result<(), String> {
        let list = free_apps()?;
        loop {
            let rows: Vec<Row> = list
                .iter()
                .map(|(id, title)| {
                    // One shared Makepad download: rows show only readiness;
                    // the action appears on the selected row.
                    let (status, action) = match self.app_state(id, self.app_release(id).as_ref()) {
                        State::Ready => (done("ready"), "run"),
                        State::Compile => (text("needs compiling", PLAIN), "compile"),
                        State::Merge => (text("updated · your changes to merge", WARN), "merge with agent"),
                        _ => (Vec::new(), "download"),
                    };
                    item(format!("app:{id}"), title.clone(), "", status, action)
                })
                .collect();
            let view = View {
                crumb: " › Makepad experiments".into(),
                subtitle: "From our open source repository: experiments, not finished applications.".into(),
                email: self.shown_email(),
                rows: {
                    let note = match self.app_release("wm").filter(|r| !r.installed(&self.root)) {
                        Some(release) => format!(
                            "One {} MB download covers them all; each compiles on first run.",
                            release.repositories.iter().map(|r| r.bytes).sum::<u64>().div_ceil(1048576)
                        ),
                        None => "Each one compiles the first time you run it.".into(),
                    };
                    let mut all = vec![Row::Note(Vec::new()), Row::Note(text(note, DIM))];
                    all.extend(rows);
                    all
                },
                back: true,
                ..View::default()
            };
            let mut selected = self.free_selected;
            let nav = view::menu(view, &mut selected, &|| self.disk_changed())?;
            self.free_selected = selected;
            match nav {
                Nav::Select(id) => {
                    // The app returns here when it has been launched (or could
                    // not be), with the row still selected.
                    if let Err(error) = self.open_app(id.trim_start_matches("app:")) {
                        if error == QUIT {
                            return Err(error);
                        }
                        self.report(error);
                    }
                }
                Nav::Back => return Ok(()),
                Nav::Quit => return Err(QUIT.into()),
                Nav::Refresh => {}
            }
        }
    }
    /// The release an app's row refers to, without writing anything:
    /// cached, installed, or projected from the known public release.
    fn app_release(&self, app: &str) -> Option<Release> {
        if let Some(release) = load_release(&self.root.join("available").join(format!("{app}.json")))
            .or_else(|| load_release(&self.root.join("installed").join(format!("{app}.json"))))
        {
            return Some(release);
        }
        if app == self.app {
            return self.release.clone();
        }
        let definition = catalog::apps().ok()?.into_iter().find(|entry| entry.get("id").and_then(makepad_strict_json::Value::as_str) == Some(app))?;
        self.public.as_ref().or(self.release.as_ref())?.for_app(&definition).ok()
    }
    fn app_state(&self, app: &str, release: Option<&Release>) -> State {
        if self.merge_marker(app).is_file() {
            return State::Merge;
        }
        let Some(release) = release else { return State::New };
        let installed = load_release(&self.root.join("installed").join(format!("{app}.json")));
        // Where Environment::app_binary publishes it.
        let binary = if cfg!(windows) { crate::home_of(&self.root).join(runtime::exe(&release.binary)) } else { self.root.join(format!("{}.bin", release.binary)) };
        if installed.as_ref().is_some_and(|i| i.release == release.release) && binary.is_file() {
            // Built from these sources; edited since (a coding agent, the
            // person) means it compiles again.
            // So does one built with the other Windows compiler, or with
            // CUDA on or off since (switching to Microsoft's tools is how
            // CUDA gets turned on).
            if !runtime::built_with_current_toolchain(&self.root, app) || self.sources_changed(app, release, &binary) { State::Compile } else { State::Ready }
        } else if release.installed(&self.root) {
            State::Compile
        } else if installed.is_some() {
            State::Update
        } else if fs::read_dir(release.directory(&self.root).join(".builder-repositories")).is_ok_and(|mut d| d.next().is_some()) {
            State::Partial
        } else {
            State::New
        }
    }
    /// Whether any source file Cargo used for the built app (its snapshot's
    /// target, release/<binary>.d) is newer than the app, or gone. Unknown
    /// (no .d yet) counts as unchanged. Cached: this reads a thousand or so
    /// file dates and the menu redraws often.
    fn sources_changed(&self, app: &str, release: &Release, binary: &Path) -> bool {
        let Ok(built) = fs::metadata(binary).and_then(|m| m.modified()) else { return false };
        if let Some(check) = self.source_checks.borrow().get(app) {
            if check.built == built && check.at.elapsed() < std::time::Duration::from_secs(5) {
                return check.changed;
            }
        }
        let deps = release.target_dir(&self.root).join("release").join(format!("{}.d", release.binary));
        let changed = fs::read_to_string(&deps).is_ok_and(|text| {
            dep_file_sources(&text).iter().any(|source| fs::metadata(source).and_then(|m| m.modified()).map_or(true, |t| t > built))
        });
        self.source_checks.borrow_mut().insert(app.to_owned(), SourceCheck { built, at: std::time::Instant::now(), changed });
        changed
    }
    fn app_setup(&self, app: &str) -> Result<Self, String> {
        let mut selected = self.clone();
        selected.app = app.into();
        selected.release = self.app_release(app);
        if let Some(release) = &selected.release {
            let available = self.root.join("available");
            fs::create_dir_all(&available).map_err(|e| e.to_string())?;
            release.save(&available.join(format!("{app}.json")))?;
        }
        Ok(selected)
    }
    /// A row's action: merge saved changes, run a built app, or download,
    /// compile and run it.
    fn open_app(&mut self, app: &str) -> Result<(), String> {
        let release = self.app_release(app);
        if self.app_state(app, release.as_ref()) == State::Merge {
            return self.merge_changes(app);
        }
        self.run_app(app)
    }
    fn run_app(&mut self, app: &str) -> Result<(), String> {
        let mut selected = self.app_setup(app)?;
        let state = self.app_state(app, selected.release.as_ref());
        fs::write(self.root.join("selected-app"), app).map_err(|e| e.to_string())?;
        // A built app just opens; anything else gets the work page.
        if state == State::Ready {
            view::working(&format!("app:{app}"), "opening…");
        }
        let result = if state == State::Ready {
            selected.launch().map(|title| launched(&title))
        } else {
            selected.install_and_run()
        };
        fs::write(self.root.join("selected-app"), &self.app).map_err(|e| e.to_string())?;
        self.adopt(app, selected);
        self.measure_disk();
        result
    }
    /// An app runs on a copy of this session pointed at it; carry back what
    /// that copy learned (compiler state, releases, a new login).
    fn adopt(&mut self, app: &str, selected: Setup) {
        if selected.checked.is_some() {
            self.checked = selected.checked;
        }
        self.compiler_retry = selected.compiler_retry;
        self.cuda = selected.cuda;
        if selected.public.is_some() {
            self.public = selected.public;
        }
        if self.email.is_empty() && !selected.email.is_empty() {
            self.email = selected.email;
        }
        if app == self.app {
            self.release = selected.release;
        }
    }
    fn install_and_run(&mut self) -> Result<(), String> {
        // Resolve the release once so compiler setup and the following build
        // cannot disagree if a newer release appears while installing tools.
        let release = if self.compiler_retry {
            self.release.clone().ok_or("The compiler retry has no release metadata; select the app again")?
        } else {
            self.release()?
        };
        if !self.install_compiler(&release)? {
            return Ok(());
        }
        activity("Compiler ready. Continuing to download, compile and open the app.");
        self.build_release(release)
    }
    /// Download (when needed), compile and open an app on its work page.
    fn build_release(&mut self, release: Release) -> Result<(), String> {
        if !self.ready().compiler() {
            return Err(format!("The latest source requires Rust {}; it could not be set up.", release.rust));
        }
        let mut steps = Vec::new();
        if !release.installed(&self.root) {
            steps.push("Download source");
        }
        steps.extend(["Compile", "Open"]);
        let subtitle = if steps[0] == "Download source" { "Download the source, compile it, then run it." } else { "Compile the source, then run it." };
        view::work_begin(&release.title, subtitle, &self.shown_email(), &format!("app:{}", release.id), &steps);
        let cuda_before = crate::cuda::kernels_failed(&self.root).is_some();
        let title = view::work_end(self.compile_and_open(&release))?;
        activity(&format!("{title} is running; Builder remains open."));
        if !cuda_before && crate::cuda::kernels_failed(&self.root).is_some() {
            view::message(text(format!("{title} is running. The CUDA kernels did not build here, so its AI features run on the CPU."), WARN));
        } else if self.merge_marker(&release.id).is_file() {
            view::message(text(format!("{title} is updated. Your edits are in changes/; select it to merge."), WARN));
        } else {
            launched(&title);
        }
        Ok(())
    }
    fn compile_and_open(&mut self, release: &Release) -> Result<String, String> {
        // Say which snapshot builds, before anything downloads: shared
        // sources mean shared artifacts; a differing Makepad commit means
        // the dependencies compile again, and that is expected.
        activity(&release.describe_sources(&self.root));
        self.keep_edits(release);
        with_progress(|| {
            view::work_step("Download source");
            catalog::checkout(&self.service, &self.email, &self.root, release)?;
            view::work_step("Compile");
            let environment = Environment::prepare(&self.root, release, self.cuda)?;
            // Scope's release pins the Makepad tree the Builder itself is
            // compiled from. A Builder that fails to compile from the new
            // tree does not withhold the app; the next start tries again.
            if release.id == "scope" {
                match runtime::update_builder(&environment, release) {
                    Ok(Some(note)) => activity(&note),
                    Ok(None) => {}
                    Err(error) => activity(&format!("Builder not updated: {error}")),
                }
            }
            environment.build(release)
        })?;
        // A build that gave up on the CUDA kernels keeps this folder CPU-only.
        self.cuda = crate::cuda::build_with(&self.root);
        fs::create_dir_all(self.root.join("installed")).map_err(|e| e.to_string())?;
        release.save(&self.root.join("installed").join(format!("{}.json", release.id)))?;
        release.save(&self.root.join("installed-release.json"))?;
        match catalog::prune_snapshots(&self.root, std::slice::from_ref(release)) {
            Ok(removed) => {
                for label in removed {
                    activity(&format!("Removed the unused sources {label}; nothing was built from them any more and they held no edits."));
                }
            }
            Err(error) => activity(&format!("Unused sources kept: {error}")),
        }
        // Just downloaded and built: that is up to date.
        self.checked = Some((false, clock()));
        activity(&format!("Build complete. Opening {}.", release.title));
        view::work_step("Open");
        self.launch()
    }
    /// Present while an app's saved edits wait for an agent to merge them.
    fn merge_marker(&self, app: &str) -> PathBuf {
        self.root.join("changes").join(format!("{app}.merge"))
    }
    /// An update is about to move `release.id` off the snapshot it was built
    /// from: save the edits made there first (see `catalog::save_changes`).
    fn keep_edits(&self, release: &Release) {
        let Some(previous) = load_release(&self.root.join("installed").join(format!("{}.json", release.id))) else { return };
        if previous.release == release.release
            || previous.directory(&self.root) == release.directory(&self.root)
            || self.merge_marker(&release.id).is_file()
        {
            return;
        }
        let (year, month, day, _, _) = local_time();
        match catalog::save_changes(&self.root, &release.id, &previous, &format!("{year:04}-{month:02}-{day:02}")) {
            Ok(Some(name)) => activity(&format!("Saved your changes to {} in changes/{name} before updating; the edited source stays in {}.", release.title, crate::shown(&previous.directory(&self.root)))),
            Ok(None) => {}
            Err(error) => activity(&format!("Could not save a diff of your changes ({error}); the edited source stays in {}.", crate::shown(&previous.directory(&self.root)))),
        }
    }
    /// Launch the app recorded under installed/; returns its title.
    fn launch(&self) -> Result<String, String> {
        let release = load_release(&self.root.join("installed").join(format!("{}.json", self.app))).ok_or("Build the app first")?;
        let environment = Environment::prepare(&self.root, &release, self.cuda)?;
        let project = if self.project == self.root {
            release
                .repositories
                .iter()
                .find(|repo| repo.name == "makepad")
                .map(|repo| release.directory(&self.root).join(&repo.path))
                .unwrap_or_else(|| release.source(&self.root))
        } else {
            self.project.clone()
        };
        #[cfg(target_os = "macos")]
        let executable = crate::desktop::prepare(&self.root, &release, &project)?;
        #[cfg(not(target_os = "macos"))]
        let executable = environment.app_binary(&release);
        let mut app = Command::new(executable);
        #[cfg(target_os = "macos")]
        if release.id == "scope" {
            // This explicit Builder launch should open in front. Ordinary
            // Makepad windows retain their default non-activating startup.
            app.arg("--focus");
        }
        // The app is a GUI child of Builder. Give it its own process group (or
        // detached Windows process) so closing the terminal that hosts Builder
        // does not terminate an already launched app. Its output is redirected
        // below, so no child console or terminal ownership is needed.
        detach_application(&mut app);
        let log_path = environment.build.join(format!("{}-app.log", release.binary));
        let log = fs::File::create(&log_path).map_err(|e| e.to_string())?;
        view::busy(&format!("Opening {}", release.title));
        activity(&format!("Running {}", release.title));
        let child = app
            .args(["--cwd", &project.to_string_lossy()])
            .current_dir(&project)
            .envs(environment.vars)
            .env_remove("MAKEPAD_LOADER_EMAIL")
            // The app's Send feedback panel shows it as its first, removable
            // row. The launched app's own environment only: builds never see it.
            .env("MAKEPAD_FEEDBACK_EMAIL", &self.email)
            .env_remove("RUSTUP_TOOLCHAIN")
            .stdin(Stdio::null())
            .stderr(log.try_clone().map_err(|e| e.to_string())?)
            .stdout(log)
            .spawn()
            .map_err(|e| e.to_string())?;
        let title = release.title.clone();
        RUNNING_APPS.with(|apps| apps.borrow_mut().push(RunningApp {
            title: release.title.clone(), child, log: log_path,
        }));
        #[cfg(target_os = "macos")]
        note_older_bundle(&self.root, &release);
        Ok(title)
    }
    /// After an update saved an app's edits in changes/, a coding agent
    /// reapplies them onto the new source.
    fn merge_changes(&mut self, app: &str) -> Result<(), String> {
        let marker = self.merge_marker(app);
        let diff = fs::read_to_string(&marker).unwrap_or_default().trim().to_owned();
        let agent = match self.agents.iter().filter(|a| a.0 != "grok").collect::<Vec<_>>().as_slice() {
            [] => {
                view::message(text(format!("Install Claude Code or Codex to merge, or apply changes/{diff} yourself."), WARN));
                return Ok(());
            }
            [one] => **one,
            several => {
                let names: Vec<&str> = several.iter().map(|a| a.0).collect();
                let Some(chosen) = view::choose("Merge your changes with which agent?", "", &names, 0)? else { return Ok(()) };
                **several.iter().find(|a| a.0 == chosen).ok_or("Unknown agent")?
            }
        };
        let task = format!(
            "Reapply my changes saved in changes/{diff} in the installation root onto the new {app} source. Keep the intent of each change where the new release moved code, rebuild it until it compiles, and report any change that no longer applies."
        );
        self.launch_agent(agent.0, Some(app), Some(&task))?;
        let _ = crate::remove_inside(&self.root, &marker);
        view::message(done(format!("{} finished merging. Select the app to run it.", agent.1)));
        Ok(())
    }
}

/// Coding agents and the shell.
impl Setup {
    /// Coding agents and the shell work in an app whose source is here.
    fn agent_release(&self, app: Option<&str>) -> Option<Release> {
        match app {
            Some(app) => self.app_release(app),
            None => [self.release.clone(), self.app_release("wm")].into_iter().flatten().find(|r| r.installed(&self.root)),
        }
    }
    fn environment(&self, command: &mut Command) -> Result<(), String> {
        if let Some(release) = self.agent_release(None) {
            let environment = Environment::prepare(&self.root, &release, self.cuda)?;
            let context = crate::agent::write_context(&release, &environment)?;
            command.envs(environment.vars).env("MAKEPAD_AGENT_CONTEXT", context);
        }
        runtime::isolate(command);
        command.current_dir(&self.project);
        Ok(())
    }
    fn shell(&self) -> Result<(), String> {
        let mut command = if cfg!(windows) {
            let mut c = Command::new(env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into()));
            c.args(["/d", "/v:off"]);
            c
        } else {
            Command::new(env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()))
        };
        self.environment(&mut command)?;
        let _pause = Screen::pause();
        println!(
            "Project: {}\nThis folder's Rust is on PATH. Type exit to return to Makepad.\n",
            crate::shown(&self.project)
        );
        let status = command
            .status()
            .map_err(|e| format!("Could not start the shell: {e}"))?;
        if !status.success() {
            activity(&format!("Shell exited {status}"));
        }
        Ok(())
    }
    fn launch_agent(&self, name: &str, app: Option<&str>, task: Option<&str>) -> Result<(), String> {
        let release = self.agent_release(app).filter(|r| r.installed(&self.root)).ok_or(
            "Download an app first; coding agents work in its source.",
        )?;
        if !self.ready().tools || !self.rust_ready(&release.rust) {
            return Err("Set up the build tools first, so the agent can rebuild apps.".into());
        }
        let environment = Environment::prepare(&self.root, &release, self.cuda)?;
        let mut command = crate::agent::command(name, &release, &environment, task)?;
        fs::write(self.root.join("selected-app"), &release.id).map_err(|e| e.to_string())?;
        let status = {
            let _pause = Screen::pause();
            command.status().map_err(|e| format!("Could not start installed {name}: {e}"))
        };
        let _ = fs::write(self.root.join("selected-app"), &self.app);
        let status = status?;
        if !status.success() {
            return Err(format!("{name} exited {status}. It must already be installed and signed in."));
        }
        Ok(())
    }
}

/// Update and disk.
impl Setup {
    /// Updates: licenses first (new purchases appear, expired ones go), then
    /// every installed app is compared with its newest release. An app with
    /// a newer release gets clean new sources; edits made to its old source
    /// are saved in changes/ first and it waits for an agent to merge them.
    fn check_updates(&mut self) -> Result<(), String> {
        // Updates download Makepad sources; withdrawn agreements ask again.
        if !self.consent("Update", "Updates download the newest sources of your apps.", "", &["makepad"], "check for updates")? {
            view::set_view(self.main_view());
            view::message(text("Nothing was downloaded.", DIM));
            return Ok(());
        }
        self.check_licenses()?;
        view::work_begin("Update", "Newer sources replace unchanged ones; your edits are kept as a diff.", &self.shown_email(), "updates", &["Check for updates", "Download updates"]);
        let updated = view::work_end(self.pull_updates())?;
        self.report_updates(updated);
        Ok(())
    }
    /// Record the newest releases and download clean sources for installed
    /// apps that have one. Returns the updated titles and whether edits wait.
    fn pull_updates(&mut self) -> Result<(Vec<String>, bool), String> {
        view::work_step("Check for updates");
        // TODO: send the releases this folder has (the `X-Makepad-Have`
        // header) once the catalog service defines it; today the full catalog
        // is compared here.
        let public = with_progress(|| catalog::fetch_public(&self.service))?;
        self.public = Some(public.clone());
        let available = self.root.join("available");
        fs::create_dir_all(&available).map_err(|e| e.to_string())?;
        for definition in catalog::apps()? {
            let Some(id) = definition.get("id").and_then(makepad_strict_json::Value::as_str) else { continue };
            if available.join(format!("{id}.json")).is_file() || self.root.join("installed").join(format!("{id}.json")).is_file() {
                public.for_app(&definition)?.save(&available.join(format!("{id}.json")))?;
            }
        }
        let mut updated = Vec::new();
        let mut merges = false;
        for entry in fs::read_dir(self.root.join("installed")).into_iter().flatten().flatten() {
            let Some(installed) = load_release(&entry.path()) else { continue };
            let Some(latest) = load_release(&available.join(format!("{}.json", installed.id))) else { continue };
            if latest.release == installed.release || !latest.supported() || (!latest.public && !self.licensed(&latest.id)) {
                continue;
            }
            self.keep_edits(&latest);
            merges |= self.merge_marker(&latest.id).is_file();
            activity(&latest.describe_sources(&self.root));
            view::work_step("Download updates");
            with_progress(|| catalog::checkout(&self.service, &self.email, &self.root, &latest))?;
            updated.push(latest.title.clone());
        }
        if let Some(release) = self.release.as_ref().map(|r| r.id.clone()).and_then(|id| load_release(&available.join(format!("{id}.json")))) {
            self.release = Some(release);
        }
        Ok((updated, merges))
    }
    fn report_updates(&mut self, (updated, merges): (Vec<String>, bool)) {
        self.checked = Some((!updated.is_empty(), clock()));
        self.measure_disk();
        view::message(if updated.is_empty() {
            done("Licenses checked; everything is up to date.")
        } else if merges {
            done(format!("Updated {}. Your edits are saved in changes/; select to merge.", updated.join(", ")))
        } else {
            done(format!("Updated {}. Each compiles on its next run.", updated.join(", ")))
        });
    }
    fn measure_disk(&self) {
        let root = self.root.clone();
        let disk = self.disk.clone();
        std::thread::spawn(move || {
            let measured = measure(&root);
            if let Ok(mut slot) = disk.lock() {
                slot.0 += 1;
                slot.1 = Some(measured);
            }
        });
    }
    fn disk_changed(&self) -> bool {
        let generation = self.disk.lock().map(|slot| slot.0).unwrap_or(0);
        generation != self.disk_seen.replace(generation)
    }
    fn clear_build(&mut self) -> Result<(), String> {
        let target = self.root.join("target");
        if !target.exists() {
            view::message(text("There is no build data to delete.", DIM));
            return Ok(());
        }
        let question = if cfg!(windows) { "Delete the build data in target\\?" } else { "Delete the build data in target/?" };
        if view::choose(question, "your apps keep working; the next compile starts from scratch", &["keep", "delete"], 0)?.as_deref() != Some("delete") {
            return Ok(());
        }
        // target/ holds only Cargo output (one CARGO_TARGET_DIR per source
        // snapshot); published executables live beside Builder and keep
        // working. It is removed directly rather than through `cargo clean`,
        // which first loads the snapshot's whole workspace and so failed
        // whenever a source download had stopped halfway.
        view::working("disk", "cleaning…");
        view::busy("Deleting build data");
        let result = fs::remove_dir_all(&target);
        self.measure_disk();
        match result {
            Ok(()) => view::message(done("Build data cleaned. Built apps are unchanged.")),
            Err(error) => {
                activity(&format!("clear build data: {error}"));
                // Windows refuses to delete a file another process holds
                // (sharing violation 32, lock violation 33, or access
                // denied 5 for a running executable).
                let in_use = cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32 | 33));
                view::message(text(
                    if in_use {
                        "Some build data is in use by a running app or compile; close it and try again."
                    } else {
                        "Could not delete all build data; the activity log says why."
                    },
                    WARN,
                ));
            }
        }
        Ok(())
    }
}

// ---- Row helpers -----------------------------------------------------------

fn item(id: impl Into<String>, name: impl Into<String>, license: &'static str, status: view::Text, action: impl Into<String>) -> Row {
    Row::Item(Item { id: id.into(), name: name.into(), license, status, action: action.into() })
}

/// The agreements involved on this platform: (id, name, URL).
fn agreements() -> Vec<(&'static str, &'static str, &'static str)> {
    let mut list = vec![("makepad", "Makepad commercial license", MAKEPAD_LICENSE_URL)];
    if cfg!(windows) {
        list.push(("vs", "Microsoft Visual Studio Build Tools", BUILD_TOOLS_LICENSE_URL));
        list.push(("sdk", "Microsoft Windows SDK", WINDOWS_SDK_LICENSE_URL));
    }
    list.push(("rust", "Rust", RUST_LICENSE_URL));
    if crate::cuda::supported() {
        list.push(("cuda", "NVIDIA CUDA Toolkit", CUDA_LICENSE_URL));
    }
    list
}

/// The agreements installing the build tools needs on this platform.
fn required_agreements() -> &'static [&'static str] {
    if cfg!(windows) { &["makepad", "vs", "sdk", "rust"] } else { &["makepad", "rust"] }
}
/// The agreements the chosen compiler needs: Rust's GNU toolchain on
/// Windows needs no Microsoft agreement.
fn required_for(root: &Path) -> &'static [&'static str] {
    if cfg!(windows) && runtime::windows_chain(root) == WindowsChain::Gnu { &["makepad", "rust"] } else { required_agreements() }
}
/// Agreement rows: full name and host; Return opens the link.
fn agreement_rows(ids: Option<&[&str]>) -> Vec<Row> {
    agreements()
        .into_iter()
        .filter(|(id, _, _)| ids.is_none_or(|ids| ids.contains(id)))
        .map(|(id, name, url)| item(format!("url:{id}"), format!("{name:<36}"), "", text(host(url), DIM), "open in browser"))
        .collect()
}

/// Return on an agreement row: open it and confirm on the status line.
fn open_agreement(id: &str) {
    let Some((_, _, url)) = agreements().into_iter().find(|(key, _, _)| Some(*key) == id.strip_prefix("url:")) else { return };
    match open_url(url) {
        Ok(()) => view::message(done(format!("Opened {} in your browser.", url.trim_start_matches("https://")))),
        Err(error) => view::warn(&error),
    }
}

fn host(url: &str) -> &str {
    url.split_once("://").map_or(url, |(_, rest)| rest.split('/').next().unwrap_or(rest))
}

/// Makepad WM, then every registry app marked `menu: other`.
fn free_apps() -> Result<Vec<(String, String)>, String> {
    let mut list = Vec::new();
    let mut wm = None;
    for app in catalog::apps()? {
        let field = |name| app.get(name).and_then(makepad_strict_json::Value::as_str).map(str::to_owned);
        let (Some(id), Some(title)) = (field("id"), field("title")) else { return Err("Invalid app registry".into()) };
        match field("menu").as_deref() {
            Some("other") => list.push((id, title)),
            _ if id == "wm" => wm = Some((id, title)),
            _ => {}
        }
    }
    list.splice(0..0, wm);
    Ok(list)
}

fn is_public(app: &str) -> bool {
    catalog::apps().is_ok_and(|apps| apps.iter().any(|a| a.get("id").and_then(makepad_strict_json::Value::as_str) == Some(app)))
}

/// The catalog refused the email itself (unknown or not enabled), rather
/// than the network or the server failing.
fn email_rejected(error: &str) -> bool {
    let lower = error.to_lowercase();
    ["(http 401)", "(http 403)", "http 401 ", "http 403 ", "not enabled", "unknown email"].iter().any(|s| lower.contains(s))
}
/// The yellow line over a reopened email editor.
fn rejected_hint(typed: &str, _error: &str) -> view::Text {
    // The typed email stays in the editor; this only says it was not found.
    text(format!("Email not recognised: {typed} · fix any typo, then ⏎"), WARN)
}

/// The first line of an error, short enough for a row.
fn short_reason(error: &str) -> String {
    let line = clean(error.lines().next().unwrap_or_default());
    let line = line.split(" (HTTP").next().unwrap_or_default().to_owned();
    if line.chars().count() > 40 { line.chars().take(39).collect::<String>() + "…" } else { line }
}

thread_local! {
    /// Said once after a launch: an older bundle named after the binary.
    static OLDER_BUNDLE: std::cell::Cell<Option<String>> = const { std::cell::Cell::new(None) };
}
/// Bundles are now named after the title ("Makepad Scope.app"). An older
/// "Scope.app" is never deleted; it is mentioned once per folder.
#[cfg(target_os = "macos")]
fn note_older_bundle(root: &Path, release: &Release) {
    let Some(old) = crate::desktop::older_bundle(root, release) else { return };
    let noted = root.join("installed").join(format!("{}.older-bundle-noted", release.binary));
    if noted.exists() {
        return;
    }
    let _ = fs::write(&noted, "");
    let name = old.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    activity(&format!("{name} from an earlier build is left in place; the app is now {}.app.", crate::desktop::bundle_name(release)));
    OLDER_BUNDLE.with(|n| n.set(Some(format!("The older {name} is still here; you can remove it."))));
}

/// The success line after launching an app.
fn launched(title: &str) {
    if let Some(note) = OLDER_BUNDLE.with(|n| n.take()) {
        view::message(vec![view::Span("✓".into(), OK), view::Span(format!(" {title} is running. "), PLAIN), view::Span(note, WARN)]);
        return;
    }
    let hint = if cfg!(target_os = "macos") {
        "Right-click its Dock icon › Options › Keep in Dock."
    } else if cfg!(windows) {
        "Right-click its taskbar icon › Pin to taskbar."
    } else {
        "Run it again from here or from its command."
    };
    view::message(vec![
        view::Span("✓".into(), OK),
        view::Span(format!(" {title} is running. "), PLAIN),
        view::Span(hint.into(), DIM),
    ]);
}

fn short_path(path: &Path) -> String {
    let home = env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    match home.and_then(|home| path.strip_prefix(&home).ok().map(Path::to_path_buf)) {
        Some(rest) if !cfg!(windows) => format!("~/{}", rest.display()),
        _ => crate::shown(path),
    }
}

fn wrap(value: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in value.split_whitespace() {
        let last = lines.last_mut().unwrap();
        if !last.is_empty() && last.chars().count() + 1 + word.chars().count() > width {
            lines.push(word.to_owned());
        } else {
            if !last.is_empty() { last.push(' '); }
            last.push_str(word);
        }
    }
    lines
}

fn clean(s: &str) -> String {
    crate::plain_paths(s).chars().filter(|c| !c.is_control()).collect()
}

fn line(prompt: &str) -> Result<String, String> {
    print!("{prompt}");
    io::stdout().flush().map_err(|e| e.to_string())?;
    let mut value = String::new();
    if io::stdin()
        .read_line(&mut value)
        .map_err(|e| e.to_string())?
        == 0
    {
        return Err("Input closed".into());
    }
    Ok(value.trim().to_owned())
}

// ---- Launched apps ---------------------------------------------------------

struct RunningApp {
    title: String,
    child: std::process::Child,
    log: PathBuf,
}

thread_local! {
    static RUNNING_APPS: std::cell::RefCell<Vec<RunningApp>> = const { std::cell::RefCell::new(Vec::new()) };
}

// Menu polling owns child bookkeeping; no blocking waiter or per-app thread.
fn reap_apps() -> bool {
    RUNNING_APPS.with(|apps| {
        let mut changed = false;
        apps.borrow_mut().retain_mut(|app| match app.child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                if status.success() {
                    activity(&format!("{} closed", app.title));
                } else {
                    view::warn(&format!("{} exited {status}; see {}", app.title, crate::shown(&app.log)));
                }
                changed = true;
                false
            }
            Err(error) => {
                view::warn(&format!("{}: {error}", app.title));
                changed = true;
                false
            }
        });
        changed
    })
}

fn detach_application(command: &mut Command) {
    #[cfg(windows)] {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS prevents an accidental console window; the new
        // process group keeps Ctrl+C and console teardown scoped to Builder.
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        // Create a new session before exec. This removes the controlling
        // terminal as well as its process group, so the app is independent of
        // the shell hosting Builder on both macOS and Linux.
        unsafe extern "C" {
            fn setsid() -> i32;
        }
        command.pre_exec(|| {
            if setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    #[cfg(not(any(windows, unix)))] {
        let _ = command;
    }
}

fn load_release(path: &Path) -> Option<Release> {
    Release::parse(&makepad_strict_json::parse(&fs::read(path).ok()?).ok()?).ok()
}

// ---- Platform helpers ------------------------------------------------------

/// Open a link in the default browser without a console window.
fn open_url(url: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "shell32")]
        unsafe extern "system" {
            fn ShellExecuteW(window: *mut std::ffi::c_void, operation: *const u16, file: *const u16, parameters: *const u16, directory: *const u16, show: i32) -> isize;
        }
        let wide = |s: &str| std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect::<Vec<u16>>();
        let (operation, file) = (wide("open"), wide(url));
        let result = unsafe { ShellExecuteW(std::ptr::null_mut(), operation.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1) };
        if result > 32 { Ok(()) } else { Err(format!("Could not open {url} (error {result})")) }
    }
    #[cfg(not(windows))]
    {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        Command::new(opener)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Could not open {url}: {e}"))
    }
}

/// An executable of this name is on PATH (with a PATHEXT extension on Windows).
fn on_path(name: &str) -> bool {
    let Some(path) = env::var_os("PATH") else { return false };
    let extensions: Vec<String> = if cfg!(windows) {
        env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into()).split(';').filter(|e| !e.is_empty()).map(str::to_lowercase).collect()
    } else {
        vec![String::new()]
    };
    env::split_paths(&path).any(|directory| extensions.iter().any(|extension| directory.join(format!("{name}{extension}")).is_file()))
}

/// The local time as "HH:MM".
fn clock() -> String {
    let (_, _, _, hour, minute) = local_time();
    format!("{hour:02}:{minute:02}")
}

/// Local (year, month, day, hour, minute).
fn local_time() -> (i32, u32, u32, u32, u32) {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" { fn GetLocalTime(time: *mut [u16; 8]); }
        let mut t = [0u16; 8];
        unsafe { GetLocalTime(&mut t) };
        (t[0] as i32, t[1] as u32, t[3] as u32, t[4] as u32, t[5] as u32)
    }
    #[cfg(not(windows))]
    {
        unsafe extern "C" {
            fn time(out: *mut i64) -> i64;
            fn localtime_r(time: *const i64, out: *mut [i64; 8]) -> *mut [i64; 8];
        }
        // struct tm starts with int sec, min, hour, mday, mon, year.
        let mut tm = [0i64; 8];
        let now = unsafe { time(std::ptr::null_mut()) };
        if unsafe { localtime_r(&now, &mut tm) }.is_null() {
            return (1970, 1, 1, 0, 0);
        }
        let f = unsafe { std::slice::from_raw_parts(tm.as_ptr() as *const i32, 6) };
        (f[5] + 1900, f[4] as u32 + 1, f[3] as u32, f[2] as u32, f[1] as u32)
    }
}

fn tree_size(path: &Path) -> u64 {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::read_dir(path).map(|entries| entries.flatten().map(|e| tree_size(&e.path())).sum()).unwrap_or(0),
        Ok(meta) => meta.len(),
        Err(_) => 0,
    }
}

/// (everything this folder uses, the build data "clear build data" deletes: target/ only).
fn measure(root: &Path) -> (u64, u64) {
    (tree_size(root), tree_size(&root.join("target")))
}

fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1073741824.)
}

/// Poll until Apple's installer has finished; Escape stops waiting. Each
/// probe runs the tools, so they are checked every few seconds.
fn wait_for_apple_installer() -> Result<(), String> {
    let _input = console::Input::enter()?;
    let mut polls = 0;
    loop {
        view::busy("Waiting for Apple's installer to finish · esc stops waiting");
        if matches!(console::key()?, Key::Back | Key::Quit) {
            return Ok(());
        }
        polls += 1;
        if polls % 15 == 0 && runtime::system_tools_ready().is_ok() {
            return Ok(());
        }
    }
}

/// macOS: Apple's compiler refuses to run until Xcode's license is accepted,
/// and says so. Only reads; never accepts.
fn xcode_license_pending() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    Command::new("/usr/bin/xcrun")
        .args(["--sdk", "macosx", "clang", "--version"])
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|output| !output.status.success() && String::from_utf8_lossy(&output.stderr).to_lowercase().contains("license"))
}

// ---- Terminal input --------------------------------------------------------

enum Key {
    Left,
    Right,
    Up,
    Down,
    Enter,
    PageUp,
    PageDown,
    Home,
    End,
    #[cfg(not(windows))]
    WheelUp,
    #[cfg(not(windows))]
    WheelDown,
    /// Escape: back out of a screen or question.
    Back,
    Backspace,
    /// Ctrl-C or closed input.
    Quit,
    Char(char),
    /// No key within the poll interval: redraw, reap apps.
    Other,
}

/// A GUI-subsystem executable also serves as its ConPTY setup child.
pub fn attach_parent_console() {
    #[cfg(windows)] unsafe {
        #[link(name = "kernel32")]
        unsafe extern "system" { fn AttachConsole(pid: u32) -> i32; }
        let _ = AttachConsole(u32::MAX);
    }
}

#[cfg(windows)]
mod console {
    use super::*;
    use std::ffi::c_void;
    #[repr(C)]
    #[derive(Default)]
    struct Coord {
        x: i16,
        y: i16,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i16,
        top: i16,
        right: i16,
        bottom: i16,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Info {
        size: Coord,
        cursor: Coord,
        attrs: u16,
        window: Rect,
        maximum: Coord,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(kind: u32) -> *mut c_void;
        fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: *mut c_void, mode: u32) -> i32;
        fn GetConsoleScreenBufferInfo(handle: *mut c_void, info: *mut Info) -> i32;
        fn SetConsoleOutputCP(code_page: u32) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, timeout: u32) -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    unsafe extern "C" {
        fn _getwch() -> u16;
        fn _kbhit() -> i32;
    }
    pub fn enable() {
        unsafe {
            let h = GetStdHandle(-11i32 as u32);
            let mut mode = 0;
            if GetConsoleMode(h, &mut mode) != 0 {
                SetConsoleMode(h, mode | 4);
            }
            SetConsoleOutputCP(65001);
        }
    }
    pub fn size() -> (usize, usize) {
        unsafe {
            let mut info = Info::default();
            if GetConsoleScreenBufferInfo(GetStdHandle(-11i32 as u32), &mut info) != 0 {
                (
                    (info.window.right - info.window.left + 1).max(1) as usize,
                    (info.window.bottom - info.window.top + 1).max(1) as usize,
                )
            } else {
                (100, 32)
            }
        }
    }
    pub fn alive(pid: u32) -> bool {
        unsafe {
            let h = OpenProcess(0x00100000, 0, pid);
            if h.is_null() {
                return false;
            }
            let active = WaitForSingleObject(h, 0) == 258;
            CloseHandle(h);
            active
        }
    }
    pub struct Input;
    impl Input {
        pub fn enter() -> Result<Self, String> {
            Ok(Self)
        }
    }
    pub fn key() -> Result<Key, String> {
        // Returning periodically lets the caller redraw after a ConPTY resize.
        for _ in 0..10 {
            if unsafe { _kbhit() } != 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if unsafe { _kbhit() } == 0 {
            return Ok(Key::Other);
        }
        let value = unsafe { _getwch() };
        Ok(match value {
            0 | 224 => match unsafe { _getwch() } {
                72 => Key::Up,
                80 => Key::Down,
                75 => Key::Left,
                77 => Key::Right,
                73 => Key::PageUp,
                81 => Key::PageDown,
                71 => Key::Home,
                79 => Key::End,
                68 => Key::Quit,
                _ => Key::Other,
            },
            13 => Key::Enter,
            9 => Key::Right,
            8 => Key::Backspace,
            27 => Key::Back,
            3 => Key::Quit,
            _ => char::from_u32(value as u32)
                .map(Key::Char)
                .unwrap_or(Key::Other),
        })
    }
}

#[cfg(not(windows))]
mod console {
    use super::*;
    pub fn enable() {}
    #[repr(C)]
    #[derive(Default)]
    struct Size {
        rows: u16,
        cols: u16,
        x: u16,
        y: u16,
    }
    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    #[cfg(target_os = "macos")]
    type NFds = u32;
    #[cfg(not(target_os = "macos"))]
    type NFds = std::ffi::c_ulong;
    unsafe extern "C" {
        fn ioctl(fd: i32, request: usize, ...) -> i32;
        fn kill(pid: i32, signal: i32) -> i32;
        fn poll(fds: *mut PollFd, count: NFds, timeout: i32) -> i32;
        fn read(fd: i32, buffer: *mut u8, count: usize) -> isize;
    }
    pub fn size() -> (usize, usize) {
        let mut size = Size::default();
        let request = if cfg!(target_os = "macos") {
            0x40087468
        } else {
            0x5413
        };
        if unsafe { ioctl(0, request, &mut size) } == 0 && size.cols > 0 && size.rows > 0 {
            (size.cols as usize, size.rows as usize)
        } else {
            (100, 32)
        }
    }
    pub fn alive(pid: u32) -> bool {
        unsafe { kill(pid as i32, 0) == 0 }
    }
    pub struct Input(String);
    impl Input {
        pub fn enter() -> Result<Self, String> {
            let saved = Command::new("stty")
                .arg("-g")
                .stdin(std::process::Stdio::inherit())
                .output()
                .map_err(|e| e.to_string())?;
            let saved = String::from_utf8_lossy(&saved.stdout).trim().to_owned();
            if !saved.is_empty()
                && Command::new("stty")
                    .args(["-icanon", "-echo", "-isig", "min", "1", "time", "0"])
                    .status()
                    .map_err(|e| e.to_string())?
                    .success()
            {
                // Report mouse input only while a menu is consuming it with
                // echo disabled, never during builds or while an app runs.
                print!("\x1b[?1000h\x1b[?1006h");
                let _ = io::stdout().flush();
                Ok(Self(saved))
            } else {
                Err("Cannot read terminal keys".into())
            }
        }
    }
    impl Drop for Input {
        fn drop(&mut self) {
            print!("\x1b[?1000l\x1b[?1006l");
            let _ = io::stdout().flush();
            let _ = Command::new("stty").arg(&self.0).status();
        }
    }
    /// Input is waiting on the terminal within `ms` milliseconds.
    fn waiting(ms: i32) -> bool {
        let mut fd = PollFd { fd: 0, events: 1, revents: 0 };
        unsafe { poll(&mut fd, 1, ms) > 0 }
    }
    fn byte() -> Option<u8> {
        let mut value = 0u8;
        (unsafe { read(0, &mut value, 1) } == 1).then_some(value)
    }
    pub fn key() -> Result<Key, String> {
        // Poll, so a quiet terminal returns Other for redraws and child
        // reaping; a readable terminal that yields nothing is closed input.
        if !waiting(200) {
            return Ok(Key::Other);
        }
        let Some(first) = byte() else {
            // Closed input is never an answer: leave instead of spinning or
            // accepting a default.
            return Ok(Key::Quit);
        };
        Ok(match first {
            b'\r' | b'\n' => Key::Enter,
            9 => Key::Right,
            3 => Key::Quit,
            8 | 127 => Key::Backspace,
            27 => {
                let mut sequence = Vec::new();
                while sequence.len() < 32 && waiting(30) {
                    let Some(value) = byte() else { break };
                    sequence.push(value);
                    let introducer = sequence.len() == 1 && (value == b'[' || value == b'O');
                    if !introducer && (value.is_ascii_alphabetic() || value == b'~') {
                        break;
                    }
                }
                match sequence.as_slice() {
                    b"" => Key::Back,
                    b"[A" | b"OA" => Key::Up,
                    b"[B" | b"OB" => Key::Down,
                    b"[C" | b"OC" => Key::Right,
                    b"[D" | b"OD" => Key::Left,
                    b"[5~" => Key::PageUp,
                    b"[6~" => Key::PageDown,
                    b"[H" | b"OH" | b"[1~" | b"[7~" => Key::Home,
                    b"[F" | b"OF" | b"[4~" | b"[8~" => Key::End,
                    s if s.starts_with(b"[<64;") && s.ends_with(b"M") => Key::WheelUp,
                    s if s.starts_with(b"[<65;") && s.ends_with(b"M") => Key::WheelDown,
                    b"[21~" => Key::Quit,
                    _ => Key::Other,
                }
            }
            c if c.is_ascii() => Key::Char(c as char),
            _ => Key::Other,
        })
    }
}


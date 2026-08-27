//! Client processes and the app-launching model, behavior read from
//! omarchy's source (local/agent_state/mpwm/omarchy-launch-model.md):
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
//! whenever mpwm is running out of a checkout, so a stale or missing
//! binary is rebuilt on launch instead of failing or showing yesterday's
//! app. cargo passes stdio and the environment straight through, so the
//! protocol is unaffected; its "Compiling …" output lands in the client's
//! log. An installed mpwm with no checkout around it falls back to
//! exec'ing the sibling binary.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;
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
        AppDef::app("terminal", "Terminal", "mpterm", "apps/mpterm", "mpterm", AlwaysNew),
        AppDef::app("browser", "Browser", "mpbrowser", "apps/mpbrowser", "mpbrowser", OrFocus),
        {
            // Recording default: the Files row in the menu opens the demo
            // VFS (virtual home over repo assets), never the real disk.
            // Drop the arg (or set MPWM_FILES_REAL=1) to browse for real.
            let mut files =
                AppDef::app("files", "Files", "mpfiles", "apps/mpfiles", "mpfiles", OrFocus);
            if std::env::var("MPWM_FILES_REAL").is_err() {
                files.args.push("--demo".to_string());
            }
            files
        },
        AppDef::app("task", "Task Manager", "mptask", "apps/mptask", "mptask", OrFocus),
        AppDef::app("sheets", "Sheets", "mpsheets", "apps/mpsheets", "mpsheets", OrFocus),
        // A viewer instance per file, so previews never steal each other's
        // window.
        AppDef::app("mpimage", "Image Viewer", "mpimage", "apps/mpimage", "mpimage", AlwaysNew),
        // Same contract as the image viewer, including `--preview <path>`.
        AppDef::app("mpvideo", "Video Player", "mpvideo", "apps/mpvideo", "mpvideo", AlwaysNew),
        AppDef::app("mppdf", "PDF Viewer", "mppdf", "apps/mppdf", "mppdf", AlwaysNew),
        AppDef::app(
            "route",
            "Route",
            "makepad-app-route",
            "apps/route",
            "makepad-app-route",
            OrFocus,
        ),
        AppDef::app("mixer", "Mixer", "makepad-mixer", "apps/mixer", "makepad-mixer", OrFocus),
        AppDef::app("vj", "VJ", "makepad-vj", "apps/vj", "makepad-vj", OrFocus),
        AppDef::app("fab", "Fab", "makepad-fab", "apps/fab", "makepad-fab", OrFocus),
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
/// test-only entries (protocol/pacing rigs via MPWM_TEST_APP) that never
/// appear in a menu.
pub fn find_app(id: &str) -> Option<AppDef> {
    if let Some(app) = registry().iter().find(|a| a.id == id) {
        return Some(app.clone());
    }
    use LaunchPolicy::*;
    match id {
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

/// The checkout root when mpwm runs from `target/<profile>/mpwm`, else
/// MPWM_ROOT, else none.
pub fn repo_root() -> Option<PathBuf> {
    if let Ok(root) = std::env::var("MPWM_ROOT") {
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

/// Resolve a sibling binary of the running mpwm executable.
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
    /// FOCUS RULE: a Quick-Look preview never takes key focus — keys keep
    /// flowing to the requesting tile (mpfiles). `focus_client` refuses to
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
        cmd.env("MPTERM_COLORS", colors);
        // Truly translucent terminals over the wallpaper (the user's
        // default; omarchy gets this from ghostty background-opacity —
        // its window rule alone, 0.985/0.96, reads as opaque).
        // "focused unfocused"; MPWM_TERM_OPACITY overrides.
        let opacity = std::env::var("MPWM_TERM_OPACITY")
            .unwrap_or_else(|_| "0.88 0.84".to_string());
        cmd.env("MPTERM_OPACITY", opacity);
    }
    // Every mp* app styles itself from the WM's theme.splash.
    if let Ok(theme) = std::env::var("MPWM_THEME_SPLASH") {
        cmd.env("MPWM_THEME_SPLASH", theme);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn {}: {}", app.package, e))?;
    // Child output (and cargo's "Compiling …") goes to a per-client log —
    // silent children are undebuggable — and every line also reaches the
    // UI so the tile can show what the build is doing.
    let log_path = std::env::temp_dir().join(format!("mpwm-client-{}.log", id));
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
        takes_focus: true,
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
        assert!(args.contains(&"mpterm".to_string()), "{:?}", args);
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
        // No checkout: run the binary next to the running mpwm, which for
        // a release mpwm is target/release/<bin>.
        let app = curated().into_iter().find(|a| a.id == "terminal").unwrap();
        match launch_argv(&app, None, &[]) {
            Ok((program, args)) => {
                let exe = std::env::current_exe().unwrap();
                assert_eq!(program.parent(), exe.parent());
                assert_eq!(program.file_name().unwrap(), "mpterm");
                assert_eq!(args, vec!["--stdin-loop".to_string()]);
            }
            // The test binary does not sit next to mpterm; the law that
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
        assert!(word_match("mpfiles - files (2)", "files"));
        // Not a word boundary: no match.
        assert!(!word_match("mpfiles", "files"));
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
                "Terminal",
                "Browser",
                "Files",
                "Task Manager",
                "Sheets",
                "Image Viewer",
                "Video Player",
                "PDF Viewer",
                "Route",
                "Mixer",
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
        for (id, policy) in [
            ("terminal", LaunchPolicy::AlwaysNew),
            ("mpimage", LaunchPolicy::AlwaysNew),
            ("mpvideo", LaunchPolicy::AlwaysNew),
            ("mppdf", LaunchPolicy::AlwaysNew),
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

//! The shared launch flow for hosted registry apps. Hosts supply process I/O and their
//! hub environment; this module chooses the command. From a checkout an app starts as
//! `cargo run --release` (cargo recompiles what changed, nothing when fresh, then runs
//! it), so a host never starts a binary older than its sources; an installation (no
//! checkout) starts the built binary, building it first when allowed.

use crate::{build_argv_features, built_binary, has_sources, run_argv_features, AppEntry, AppTarget, MAKEPAD_RUN};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};

/// From a checkout every start is `cargo run` (never a binary older than its sources);
/// installed, `IfMissing` starts the binary and `Rebuild` builds it first. A host that must
/// not compile in the background (a warm pool, previews) checks `AppEntry::is_current`
/// before it starts anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildPolicy {
    IfMissing,
    Rebuild,
}

/// What runs the app's code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runner {
    /// Its own release binary.
    Native,
    /// The JIT: `makepad run <package>` (tools/run). From a checkout that is `cargo run --release -p
    /// makepad-run --bin makepad -- run <package>`, so the tool is never older than its sources.
    Jit,
}

/// How the host wants the app to open its window, for either runner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildMode {
    /// Connect to the host as its `--stdin-loop` child.
    Wire,
    /// Use the window system (on Linux the HAL discovers the WM's socket).
    Window,
}

impl Runner {
    /// The runner a start of `app` uses here. An installed app among compiled ones (a Mac's
    /// folder of app bundles, the Runner's Applications/Makepad, whose builds the Runner keeps
    /// current) starts the compiled build beside it: the apps a person installed run compiled,
    /// never under the JIT (user, 11-19). Elsewhere the JIT needs a checkout (the app's sources) or
    /// an installed `makepad` tool; without either an installed build of the app runs as itself.
    pub fn available(self, app: &AppEntry, root: Option<&Path>) -> Runner {
        match self {
            Runner::Jit if crate::bundled_sibling(app.target().bin).is_some() => Runner::Native,
            Runner::Jit if root.is_none() && built_binary(&MAKEPAD_RUN, None).is_none() && built_binary(&app.target(), None).is_some() => Runner::Native,
            runner => runner,
        }
    }
}

/// What a spawned process is, for the host's environment and output handling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchKind {
    /// A build only (`cargo build`): no app environment.
    Build,
    /// The app's binary.
    App,
    /// `cargo run`: the app, compiled first when needed. It gets the app's environment,
    /// and its output (cargo's progress, then the app's log) is piped like a build's.
    Run,
}

pub enum LaunchEvent {
    Started,
    Exited(ExitStatus),
}

/// Each piped stream ends after its final line, so even a very fast failed
/// build delivers all diagnostics before the host displays its result.
#[derive(Clone, Debug)]
pub enum ProcessOutput {
    Line(String),
    Closed,
}

fn app_args(app: &AppEntry, mode: ChildMode, extra: &[String]) -> Vec<String> {
    let mut args = Vec::new();
    if mode == ChildMode::Wire {
        args.push("--stdin-loop".to_string());
    }
    // The transport belongs to the host even when saved app arguments contain the flag.
    let supplied = app.args.iter().copied().chain(extra.iter().map(String::as_str));
    args.extend(supplied.filter(|arg| *arg != "--stdin-loop").map(str::to_owned));
    args
}

/// The built binary and its arguments in the host's chosen mode.
pub fn launch_argv(app: &AppEntry, mode: ChildMode, root: Option<&Path>, extra: &[String]) -> Result<(PathBuf, Vec<String>), String> {
    let program = built_binary(&app.target(), root).ok_or_else(|| match root {
        Some(_) => format!("not built yet: {}", app.bin),
        None => format!("binary not found: {}", app.bin),
    })?;
    Ok((program, app_args(app, mode, extra)))
}

/// A host keeps the returned child for its normal process-group cleanup.
/// `poll` only uses `try_wait`: compilation never blocks the UI thread.
pub struct AppLaunch {
    app: &'static AppEntry,
    runner: Runner,
    mode: ChildMode,
    root: Option<PathBuf>,
    extra: Vec<String>,
    building: bool,
    /// Started as `cargo run`: one process compiles and becomes the app.
    run: bool,
    /// Cargo said it runs the app (`Running ...`): `poll` reports `Started` once.
    ran: bool,
    status: String,
    progress: String,
    diagnostic: String,
    last_output: String,
    closed_streams: usize,
    /// A JIT run as the Runner's releases run the app ([`Self::start_release`]).
    release: bool,
}

impl AppLaunch {
    /// The spawn callback pipes both build streams through `read_output`
    /// and feeds its events to `output`. App I/O is the host's choice.
    /// `held`: the host compiles now and starts the app later (`poll` waits until it
    /// polls again: DECK's next slides), so the build runs on its own (`cargo build`)
    /// and the fresh binary starts when released. What `spawn` makes of the command is the host's
    /// (`C`: its child, or the run a host starts the command as, under a wrapper of its own).
    pub fn start<C>(
        app: &'static AppEntry,
        runner: Runner,
        mode: ChildMode,
        root: Option<PathBuf>,
        extra: Vec<String>,
        policy: BuildPolicy,
        held: bool,
        spawn: impl FnMut(Command, LaunchKind) -> Result<C, String>,
    ) -> Result<(Self, C), String> {
        Self::start_as(app, runner, mode, root, extra, policy, held, false, spawn)
    }

    /// [`Self::start`] as the Runner's releases run an app (its pinned Dock entries, a Mac node's
    /// dock): under the JIT without the package's default features (the design overlay,
    /// `tweaker`), with the registry's own (`AppEntry::features`, the Runner's list's), and
    /// standalone in a window of its own, never a `--stdin-loop` child (with no host on its wire
    /// a hosted app draws nowhere and ends after the host's silence limit). Another feature set
    /// is another build of the app and of `makepad-widgets` under it: a run with the defaults
    /// leaves a Dock entry's start cold.
    pub fn start_release<C>(
        app: &'static AppEntry,
        root: Option<PathBuf>,
        extra: Vec<String>,
        spawn: impl FnMut(Command, LaunchKind) -> Result<C, String>,
    ) -> Result<(Self, C), String> {
        Self::start_as(app, Runner::Jit, ChildMode::Window, root, extra, BuildPolicy::IfMissing, false, true, spawn)
    }

    #[allow(clippy::too_many_arguments)]
    fn start_as<C>(
        app: &'static AppEntry,
        runner: Runner,
        mode: ChildMode,
        root: Option<PathBuf>,
        extra: Vec<String>,
        policy: BuildPolicy,
        held: bool,
        release: bool,
        mut spawn: impl FnMut(Command, LaunchKind) -> Result<C, String>,
    ) -> Result<(Self, C), String> {
        let policy = if held { BuildPolicy::Rebuild } else { policy };
        let target = Self::target_of(app, runner);
        // an installed app's compiled sibling (`Runner::available`) starts as it is: the Runner
        // keeps it current, and a checkout beside it builds nothing for it
        let sibling = runner == Runner::Native && !held && crate::bundled_sibling(target.bin).is_some();
        let run = !held && !sibling
            && root.as_deref().is_some_and(|root| has_sources(&target, root));
        let building = !sibling && (run || root.is_some() && match policy {
            BuildPolicy::IfMissing => built_binary(&target, root.as_deref()).is_none(),
            BuildPolicy::Rebuild => true,
        });
        let mut launch = Self {
            app, runner, mode, root, extra, building, run, ran: false,
            status: if run { format!("starting {}…", app.label) } else if building { format!("building {}…", app.label) } else { format!("starting {}…", app.label) },
            progress: String::new(),
            diagnostic: String::new(),
            last_output: String::new(),
            closed_streams: 0,
            release,
        };
        let kind = if run { LaunchKind::Run } else if building { LaunchKind::Build } else { LaunchKind::App };
        // the loader runs the app at once, compiling as it runs: started, no build to wait for
        if launch.mount_loader().is_some() {
            (launch.building, launch.ran) = (false, true);
            launch.status = format!("starting {}…", app.label);
        }
        let child = spawn(launch.command()?, kind)?;
        Ok((launch, child))
    }

    /// The `cargo build` of what a checkout launch of `app` under `runner` compiles (the app,
    /// or the `makepad` tool for the JIT), with the launch's environment ([`crate::child_env`]
    /// scrub, [`crate::cargo_env`]): a deploy that prebuilds with it leaves the launch's `cargo
    /// run` nothing to compile. Any other environment (a script's `CARGO_INCREMENTAL=0`, its
    /// `MAKEPAD=`) builds other artifacts, and every launch compiles again.
    pub fn build_command(app: &'static AppEntry, runner: Runner, root: PathBuf) -> Result<Command, String> {
        let launch = Self {
            app, runner, mode: ChildMode::Window, root: Some(root), extra: Vec::new(), building: true, run: false, ran: false,
            status: String::new(), progress: String::new(), diagnostic: String::new(), last_output: String::new(), closed_streams: 0, release: false,
        };
        launch.command()
    }

    pub fn is_building(&self) -> bool {
        self.building
    }

    pub fn runner(&self) -> Runner {
        self.runner
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    /// The parsed compile stage, without the app name or a host's wording.
    pub fn progress(&self) -> &str {
        &self.progress
    }

    /// Feed cleaned output from the build's pipes before polling its child.
    /// Keep compiler diagnostics for a failed build, without rescanning logs.
    pub fn output(&mut self, output: &ProcessOutput) -> Option<(String, bool)> {
        let text = match output {
            ProcessOutput::Line(text) => text,
            ProcessOutput::Closed => {
                if self.building { self.closed_streams += 1; }
                return None;
            }
        };
        if !self.building {
            // After cargo handed over, the lines are the app's own log.
            return if self.run { None } else { cargo_progress(text) };
        }
        self.last_output.clone_from(text);
        if text.starts_with("error:") || text.starts_with("error[") || !self.diagnostic.is_empty() {
            self.diagnostic.push_str(text);
            self.diagnostic.push('\n');
        }
        let (stage, linked) = cargo_progress(text)?;
        if self.run && text.trim_start().starts_with("Running ") {
            // cargo starts the app now (in its own process): no second launch.
            self.building = false;
            self.ran = true;
            self.progress.clear();
            self.diagnostic.clear();
            self.status = format!("starting {}…", self.app.label);
            return Some((self.status.clone(), true));
        }
        if self.progress == stage { return None; }
        self.status = format!("building {}… {stage}", self.app.label);
        self.progress = stage;
        Some((self.status.clone(), linked))
    }

    /// What cargo builds and runs for `app` under `runner`: the app itself, or the `makepad` tool.
    fn target_of(app: &'static AppEntry, runner: Runner) -> AppTarget<'static> {
        match runner {
            Runner::Native => app.target(),
            Runner::Jit => MAKEPAD_RUN,
        }
    }

    /// The app's arguments, or the tool's run command with the app's arguments after `--`.
    fn process_args(&self) -> Vec<String> {
        match self.runner {
            Runner::Native => app_args(self.app, self.mode, &self.extra),
            Runner::Jit => self.run_args(),
        }
    }

    /// The tool's `run <package> --bin <bin>`, with the app's arguments after `--`.
    fn run_args(&self) -> Vec<String> {
        let app_args = app_args(self.app, self.mode, &self.extra);
        // the app's own binary, as a native launch's `cargo run --bin` names it (a package may
        // hold several)
        let mut args = vec!["run".to_string(), self.app.package.to_string(), "--bin".to_string(), self.app.bin.to_string()];
        if self.release {
            args.push("--no-default-features".to_string());
            if !self.app.features.is_empty() {
                args.push("--features".to_string());
                args.push(self.app.features.join(","));
            }
        }
        // A native launch on the mount loader can still get app arguments from its host
        // (the WM appends an OS path for --cwd), just as a native cargo run can.
        if !app_args.is_empty() || self.runner == Runner::Native {
            args.push("--".to_string());
            args.extend(app_args);
        }
        args
    }

    /// The cargo features its build adds: the app's own (`AppEntry::build_features`); the JIT
    /// tool's build none.
    fn features(&self) -> String {
        if self.runner == Runner::Native { self.app.build_features() } else { String::new() }
    }

    /// The JIT loader this host runs under (`RELAX_LOADER`).
    fn host_loader() -> Option<PathBuf> {
        std::env::var_os("RELAX_LOADER").map(PathBuf::from).filter(|l| l.is_file())
    }

    /// The loader that runs a native run from the JIT's virtual source tree ([`crate::on_disk`]:
    /// a devhub overlay): cargo, a process of its own, finds no checkout there ("manifest path
    /// `<work>/dev/src/Cargo.toml` does not exist": a live session's browser host, 11-16), and the
    /// loader serves the tree to a run it starts in it. The app uses the host's chosen mode,
    /// its code built as it runs (without its native build features).
    fn mount_loader(&self) -> Option<PathBuf> {
        (self.run && self.runner == Runner::Native && self.root.as_deref().is_some_and(|root| crate::on_disk(root).is_err())).then(Self::host_loader).flatten()
    }

    fn command(&self) -> Result<Command, String> {
        let target = Self::target_of(self.app, self.runner);
        // a host that runs under the JIT loader starts JIT apps with that loader (`$RELAX_LOADER
        // run ...`, the runner's convention), never a cargo build of another one: a CI tree's cargo
        // (no CARGO_TARGET_DIR once scrubbed) built a whole host makepad on every launch
        let in_mount = self.mount_loader().is_some();
        let loader = (self.run && self.runner == Runner::Jit).then(Self::host_loader).flatten().or_else(|| self.mount_loader());
        let (program, args) = if let Some(loader) = &loader {
            self.root.as_deref().ok_or("no checkout to run from")?;
            (loader.clone(), self.run_args())
        } else if self.run {
            let root = self.root.as_deref().ok_or("no checkout to run from")?;
            crate::on_disk(root)?;
            run_argv_features(&target, root, &self.features(), &self.process_args())
        } else if self.building {
            let root = self.root.as_deref().ok_or("no checkout to build from")?;
            if !has_sources(&target, root) {
                return Err(format!("sources not found: {}", target.dir));
            }
            build_argv_features(&target, root, &self.features())
        } else if self.runner == Runner::Native {
            launch_argv(self.app, self.mode, self.root.as_deref(), &self.extra)?
        } else {
            let program = built_binary(&target, self.root.as_deref()).ok_or_else(|| format!("not built yet: {}", target.bin))?;
            (program, self.process_args())
        };
        let mut command = Command::new(program);
        command.args(args);
        crate::child_env::scrub(&mut command);
        if (self.building || self.run) && loader.is_none() {
            crate::cargo_env(&mut command);
        }
        // The registry's launch, whatever its launcher's environment: a lane shell's target dir
        // and test switches reach neither the app nor a checkout's cargo (which builds into the
        // checkout's own target/, as the prebuild did).
        crate::child_env::scrub_lane(&mut command);
        // A run in the JIT's virtual source tree (a devhub overlay: its host runs from the mount),
        // as the JIT or on the host's loader, keeps the edits over it and their live patching: the
        // JIT hands its child the mount, and the edits and the live switch only when the launcher
        // does not clear them. Cleared, the apps a WM in an overlay live session started ran the
        // overlay's base commit, not the lane's saved edits, and a UI proof through that WM was
        // stale with nothing said (11-20; only `--tree` gave the edited apps).
        if loader.is_some() && self.root.as_deref().is_some_and(|root| crate::on_disk(root).is_err()) {
            for key in ["RELAX_EDITS", "RELAX_STORE_LIVE"] {
                if let Some(value) = std::env::var_os(key) { command.env(key, value); }
            }
        }
        // The app itself gets this machine's settings for it (`machine::launch_env`); a host
        // adds its own after this.
        if !self.building || self.run {
            command.envs(crate::machine::launch_env(self.app.id, self.runner == Runner::Jit || in_mount));
            // A checkout devhub syncs (its `FLEET-REVISION.txt`): the JIT applies changes to it
            // only after a sync of landed work, never what a lane or an edit writes there
            // (relax's `DockGate`).
            if let (Runner::Jit, Some(revision)) = (self.runner, self.root.as_ref().map(|r| r.join(crate::machine::FLEET_REVISION)).filter(|r| r.is_file())) {
                command.env("RELAX_DOCK_REVISION", revision);
            }
        }
        if let Some(root) = &self.root {
            command.current_dir(root);
        }
        Ok(command)
    }

    /// A successful build launches the same registry entry exactly once.
    /// Hosts only react to Started/Exited or display the returned error.
    pub fn poll(
        &mut self,
        child: &mut Option<Child>,
        mut spawn: impl FnMut(Command, LaunchKind) -> Result<Child, String>,
    ) -> Result<Option<LaunchEvent>, String> {
        if self.run {
            if std::mem::take(&mut self.ran) {
                return Ok(Some(LaunchEvent::Started));
            }
            let Some(process) = child.as_mut() else { return Ok(None) };
            let Some(exit) = process.try_wait().map_err(|error| format!("{}: {error}", self.app.label))? else { return Ok(None) };
            if !self.building {
                return Ok(Some(LaunchEvent::Exited(exit)));
            }
            // cargo ended before it ran the app: the build failed.
            if self.closed_streams < 2 { return Ok(None); }
            child.take();
            self.building = false;
            self.progress.clear();
            self.status = format!("building {} failed ({exit})", self.app.label);
            let detail = if self.diagnostic.is_empty() { &self.last_output } else { &self.diagnostic };
            if !detail.is_empty() {
                self.status.push_str(":\n");
                self.status.push_str(detail.trim_end());
            }
            return Err(self.status.clone());
        }
        // The two streams are stdout and stderr, both piped by the host's
        // build spawn. Their workers never block this polling thread.
        if self.building && self.closed_streams < 2 { return Ok(None); }
        let Some(process) = child.as_mut() else { return Ok(None) };
        let Some(exit) = process.try_wait().map_err(|error| format!("{}: {error}", self.app.label))? else { return Ok(None) };
        if !self.building {
            // Keep the handle for the host's eventual process-group cleanup.
            return Ok(Some(LaunchEvent::Exited(exit)));
        }
        child.take();
        self.building = false;
        self.progress.clear();
        if !exit.success() {
            self.status = format!("building {} failed ({exit})", self.app.label);
            let detail = if self.diagnostic.is_empty() { &self.last_output } else { &self.diagnostic };
            if !detail.is_empty() {
                self.status.push_str(":\n");
                self.status.push_str(detail.trim_end());
            }
            return Err(self.status.clone());
        }
        self.diagnostic.clear();
        self.last_output.clear();
        *child = Some(spawn(self.command()?, LaunchKind::App)?);
        self.status = format!("starting {}…", self.app.label);
        Ok(Some(LaunchEvent::Started))
    }
}

/// Read a pipe on a host's long-lived worker, splitting both newlines and
/// Cargo's carriage-return progress redraws. The host owns wakeups/channels.
pub fn read_output(stream: impl Read, mut log: Option<std::fs::File>, mut emit: impl FnMut(ProcessOutput) -> bool) {
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
        let raw = String::from_utf8_lossy(&line);
        let text = strip_ansi(&raw).trim().to_string();
        if let Some(file) = log.as_mut().filter(|_| !text.is_empty() && !text.starts_with("Building [")) {
            let _ = writeln!(file, "{raw}");
        }
        if !text.is_empty() && !emit(ProcessOutput::Line(text)) {
            return;
        }
        line.clear();
        if byte.is_none() {
            break;
        }
    }
    emit(ProcessOutput::Closed);
}

/// Cargo (and rustc) paint their progress; a pipe usually turns that off,
/// but a stray CSI sequence must never reach a Label.
fn strip_ansi(s: &str) -> String {
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
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The command `start` hands its host, captured (the spawn refuses, so nothing runs).
    fn started(id: &str, runner: Runner, mode: ChildMode, root: &Path) -> (Vec<String>, LaunchKind) {
        let mut seen = None;
        let entry = crate::find(id).unwrap();
        let _ = AppLaunch::start::<()>(entry, runner, mode, Some(root.to_path_buf()), vec!["--x".into()], BuildPolicy::IfMissing, false, |cmd, kind| {
            seen = Some((cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect(), kind));
            Err("captured".into())
        });
        seen.expect("start spawned")
    }

    /// Under the JIT a checkout launch is `run <package> --bin <bin>` with the app's arguments after `--` (on
    /// the wire `--stdin-loop` comes first), given to the loader this host
    /// runs under (`RELAX_LOADER`), else to `cargo run` of the makepad tool.
    #[test]
    fn a_jit_launch_runs_the_package_under_the_makepad_tool() {
        let root = crate::checkout_at_or_above(Path::new(env!("CARGO_MANIFEST_DIR"))).expect("tests run in a checkout");
        for (mode, flags) in [(ChildMode::Wire, &["--stdin-loop"][..]), (ChildMode::Window, &[][..])] {
            let (args, kind) = started("terminal", Runner::Jit, mode, &root);
            assert_eq!(kind, LaunchKind::Run, "a checkout JIT launch runs the tool");
            let run = [&["run", "makepad-app-terminal", "--bin", "makepad-app-terminal", "--"], flags, &["--x"]].concat();
            if std::env::var_os("RELAX_LOADER").is_some_and(|l| Path::new(&l).is_file()) {
                assert_eq!(args, run, "the JIT loader passes the {mode:?} host's app arguments");
            } else {
                let tail: Vec<&str> = args.iter().map(String::as_str).skip_while(|a| *a != "-p").collect();
                assert_eq!(tail, [&["-p", "makepad-run", "--bin", "makepad", "--"][..], &run[..]].concat(), "the JIT tool passes the {mode:?} host's app arguments");
            }
            let (native, _) = started("terminal", Runner::Native, mode, &root);
            assert_eq!(native.iter().filter(|a| *a == "--stdin-loop").count(), flags.len(), "the native launch passes --stdin-loop exactly once for a wire host and omits it for a window host");
            assert!(native.iter().any(|a| a == "makepad-app-terminal"), "the native launch names the app's binary");
            if crate::on_disk(&root).is_err() {
                assert_eq!(native, run, "a native launch from the virtual tree preserves the {mode:?} host's arguments");
            }
        }
        // as the Runner's releases run it: the registry's features instead of the defaults (no
        // design overlay), an empty list too, standalone (no `--stdin-loop` on any OS)
        let release = |id: &str| {
            let entry = crate::find(id).unwrap();
            let mut seen: Vec<String> = Vec::new();
            let _ = AppLaunch::start_release::<()>(entry, Some(root.clone()), vec!["--stdin-loop".into()], |cmd, _| {
                seen = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
                Err("captured".into())
            });
            // the tool's `run <package> ...` (after `cargo run ... --` when not under a loader)
            let at = seen.iter().position(|a| a == entry.package).expect("the package is named");
            seen[at - 1..].to_vec()
        };
        let calc = release("calculator");
        assert_eq!(&calc[..6], ["run", "makepad-app-calculator", "--bin", "makepad-app-calculator", "--no-default-features", "--features"]);
        assert_eq!(&calc[6..], ["standalone"]);
        assert_eq!(&release("terminal")[4..], ["--no-default-features"]);
    }

    /// A JIT launch from a checkout devhub syncs (it holds `FLEET-REVISION.txt`) carries the dock
    /// gate (`RELAX_DOCK_REVISION`: only a sync of landed work changes that app's code); a
    /// launch from any other tree (a lane's) does not.
    #[test]
    fn a_launch_from_the_synced_checkout_is_gated_on_its_revision() {
        let real = crate::checkout_at_or_above(Path::new(env!("CARGO_MANIFEST_DIR"))).expect("tests run in a checkout");
        let dock = std::env::temp_dir().join(format!("app-registry-dock-{}", std::process::id()));
        std::fs::create_dir_all(dock.join("tools/run")).unwrap();
        std::fs::copy(real.join("tools/run/Cargo.toml"), dock.join("tools/run/Cargo.toml")).unwrap();
        let gate = |root: &Path| {
            let mut env = None;
            let entry = crate::find("calculator").unwrap();
            let _ = AppLaunch::start::<()>(entry, Runner::Jit, ChildMode::Window, Some(root.to_path_buf()), Vec::new(), BuildPolicy::IfMissing, false, |cmd, _| {
                env = cmd.get_envs().find(|(k, _)| *k == "RELAX_DOCK_REVISION").and_then(|(_, v)| v.map(|v| v.to_os_string()));
                Err("captured".into())
            });
            env
        };
        assert_eq!(gate(&dock), None, "no revision file: not a synced checkout");
        std::fs::write(dock.join(crate::machine::FLEET_REVISION), "makepad aaaa\n").unwrap();
        assert_eq!(gate(&dock), Some(dock.join(crate::machine::FLEET_REVISION).into_os_string()));
        let _ = std::fs::remove_dir_all(&dock);
    }

    /// A deploy's prebuild is the launch's own cargo command with `build` for `run` and the same
    /// environment, so the launch finds every artifact fresh (a different CARGO_INCREMENTAL or
    /// MAKEPAD is another profile/fingerprint: a node recompiled 97 crates at a JIT launch).
    /// A host already under a JIT loader launches it directly, without cargo's build environment,
    /// and from the JIT's virtual tree a native launch too.
    #[test]
    fn the_prebuild_is_the_launch_with_build_for_run() {
        let root = crate::checkout_at_or_above(Path::new(env!("CARGO_MANIFEST_DIR"))).expect("tests run in a checkout");
        for (id, runner) in [("sandbox", Runner::Jit), ("terminal", Runner::Native)] {
            let entry = crate::find(id).unwrap();
            let mut run = None;
            let mode = if runner == Runner::Jit && cfg!(target_os = "linux") { ChildMode::Window } else { ChildMode::Wire };
            let _ = AppLaunch::start::<()>(entry, runner, mode, Some(root.clone()), Vec::new(), BuildPolicy::IfMissing, false, |cmd, _| {
                run = Some(cmd);
                Err("captured".into())
            });
            let run = run.unwrap();
            let build = AppLaunch::build_command(entry, runner, root.clone()).unwrap();
            let args = |c: &Command| c.get_args().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>();
            // what this machine sets for the app at run time is not a build input, nor is the
            // dock gate a JIT launch from a devhub-synced checkout carries (read by the running JIT)
            let mut machine: Vec<String> = crate::machine::launch_env(id, runner == Runner::Jit).into_iter().map(|(k, _)| k).collect();
            machine.push("RELAX_DOCK_REVISION".to_string());
            let envs = |c: &Command| {
                let mut v: Vec<_> = c.get_envs().filter(|(k, _)| !machine.iter().any(|m| k.to_str() == Some(m.as_str()))).map(|(k, v)| (k.to_os_string(), v.map(|v| v.to_os_string()))).collect();
                v.sort();
                v
            };
            let (run_args, build_args) = (args(&run), args(&build));
            let loader = std::env::var_os("RELAX_LOADER").map(PathBuf::from).filter(|p| p.is_file());
            if let Some(loader) = loader.as_ref().filter(|_| runner == Runner::Jit) {
                assert_eq!(run.get_program(), loader.as_os_str());
                let mut expected_args = vec!["run".to_string(), entry.package.to_string(), "--bin".to_string(), entry.bin.to_string()];
                let mut app_args = Vec::new();
                if !cfg!(target_os = "linux") { app_args.push("--stdin-loop".to_string()); }
                app_args.extend(entry.args.iter().map(|arg| arg.to_string()));
                if !app_args.is_empty() {
                    expected_args.push("--".to_string());
                    expected_args.extend(app_args);
                }
                assert_eq!(run_args, expected_args, "{id}");
                let (program, expected_args) = crate::build_argv(&MAKEPAD_RUN, &root);
                assert_eq!(build.get_program(), program.as_os_str());
                assert_eq!(build_args, expected_args, "{id}");
                let scrubbed = || {
                    let mut c = Command::new(loader);
                    crate::child_env::scrub(&mut c);
                    crate::child_env::scrub_lane(&mut c);
                    c
                };
                let mut expected = scrubbed();
                // from the JIT's virtual tree (a test run in a devhub overlay) the host's edits and
                // their live patching go along: the app runs the lane's saved sources, not the
                // overlay's base commit (a WM's apps in an overlay live session, 11-20)
                if crate::on_disk(&root).is_err() {
                    for key in ["RELAX_EDITS", "RELAX_STORE_LIVE"] {
                        if let Some(value) = std::env::var_os(key) { expected.env(key, value); }
                    }
                }
                assert_eq!(envs(&run), envs(&expected), "{id}");
                let mut expected = scrubbed();
                crate::cargo_env(&mut expected);
                assert_eq!(envs(&build), envs(&expected), "{id}");
            } else if let Some(loader) = loader.as_ref().filter(|_| crate::on_disk(&root).is_err()) {
                // the loader builds as it runs: no cargo build to match
                assert_eq!(run.get_program(), loader.as_os_str(), "{id}");
            } else {
                let cut = run_args.iter().position(|a| a == "--").unwrap();
                assert_eq!(run.get_program(), build.get_program());
                assert_eq!(run_args[0], "run");
                assert_eq!(build_args[0], "build");
                assert_eq!(run_args[1..cut], build_args[1..], "{id}");
                assert_eq!(envs(&run), envs(&build), "{id}");
            }
            assert_eq!(run.get_current_dir(), build.get_current_dir());
        }
    }
}

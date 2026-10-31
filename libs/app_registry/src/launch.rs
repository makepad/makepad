//! The shared launch flow for hosted registry apps. Hosts supply process I/O and their
//! hub environment; this module chooses the command. From a checkout an app starts as
//! `cargo run --release` (cargo recompiles what changed, nothing when fresh, then runs
//! it), so a host never starts a binary older than its sources; an installation (no
//! checkout, or the Builder's) starts the built binary, building it first when allowed.

use crate::{build_argv, builder_install, built_binary, has_sources, run_argv, AppEntry, AppTarget, MAKEPAD_RUN};
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
    /// Its own release binary, a `--stdin-loop` child of its host.
    Native,
    /// The JIT: `makepad run <package>` (tools/run). From a checkout that is `cargo run --release -p
    /// makepad-run --bin makepad -- run <package>`, so the tool is never older than its sources.
    /// It is not a `--stdin-loop` child: on Linux its HAL asks the WM's socket for its window
    /// (`makepad_hal::wm_socket`), and the WM pairs that request with this launch by pid.
    Jit,
}

/// What a spawned process is, for the host's environment and output handling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchKind {
    /// A build only (an installation's Builder or `cargo build`): no app environment.
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

/// The built binary, `--stdin-loop`, registry arguments, then host arguments.
pub fn launch_argv(app: &AppEntry, root: Option<&Path>, extra: &[String]) -> Result<(PathBuf, Vec<String>), String> {
    let program = built_binary(&app.target(), root).ok_or_else(|| match root {
        Some(_) => format!("not built yet: {}", app.bin),
        None => format!("binary not found: {}", app.bin),
    })?;
    let mut args = vec!["--stdin-loop".to_string()];
    args.extend(app.args.iter().map(|arg| arg.to_string()));
    args.extend(extra.iter().cloned());
    Ok((program, args))
}

/// A host keeps the returned child for its normal process-group cleanup.
/// `poll` only uses `try_wait`: compilation never blocks the UI thread.
pub struct AppLaunch {
    app: &'static AppEntry,
    runner: Runner,
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
}

impl AppLaunch {
    /// The spawn callback pipes both build streams through `read_output`
    /// and feeds its events to `output`. App I/O is the host's choice.
    /// `held`: the host compiles now and starts the app later (`poll` waits until it
    /// polls again: DECK's next slides), so the build runs on its own (`cargo build`)
    /// and the fresh binary starts when released.
    pub fn start(
        app: &'static AppEntry,
        runner: Runner,
        root: Option<PathBuf>,
        extra: Vec<String>,
        policy: BuildPolicy,
        held: bool,
        mut spawn: impl FnMut(Command, LaunchKind) -> Result<Child, String>,
    ) -> Result<(Self, Child), String> {
        let policy = if held { BuildPolicy::Rebuild } else { policy };
        let target = Self::target_of(app, runner);
        let run = !held && builder_install().is_none()
            && root.as_deref().is_some_and(|root| has_sources(&target, root));
        let building = run || root.is_some() && match policy {
            BuildPolicy::IfMissing => built_binary(&target, root.as_deref()).is_none(),
            BuildPolicy::Rebuild => true,
        };
        let launch = Self {
            app, runner, root, extra, building, run, ran: false,
            status: if run { format!("starting {}…", app.label) } else if building { format!("building {}…", app.label) } else { format!("starting {}…", app.label) },
            progress: String::new(),
            diagnostic: String::new(),
            last_output: String::new(),
            closed_streams: 0,
        };
        let kind = if run { LaunchKind::Run } else if building { LaunchKind::Build } else { LaunchKind::App };
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
            app, runner, root: Some(root), extra: Vec::new(), building: true, run: false, ran: false,
            status: String::new(), progress: String::new(), diagnostic: String::new(), last_output: String::new(), closed_streams: 0,
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

    /// The arguments of the started process: the app's (`--stdin-loop`, registry, host), or the
    /// tool's `run <package>` with the app's after `--`.
    fn process_args(&self) -> Vec<String> {
        let app_args = self.app.args.iter().map(|arg| arg.to_string()).chain(self.extra.iter().cloned());
        match self.runner {
            Runner::Native => std::iter::once("--stdin-loop".to_string()).chain(app_args).collect(),
            Runner::Jit => {
                let app_args: Vec<String> = app_args.collect();
                let mut args = vec!["run".to_string(), self.app.package.to_string()];
                if !app_args.is_empty() {
                    args.push("--".to_string());
                    args.extend(app_args);
                }
                args
            }
        }
    }

    fn command(&self) -> Result<Command, String> {
        let target = Self::target_of(self.app, self.runner);
        let (program, args) = if self.run {
            let root = self.root.as_deref().ok_or("no checkout to run from")?;
            run_argv(&target, root, &self.process_args())
        } else if self.building {
            let root = self.root.as_deref().ok_or("no checkout to build from")?;
            if !has_sources(&target, root) {
                return Err(format!("sources not found: {}", target.dir));
            }
            build_argv(&target, root)
        } else if self.runner == Runner::Native {
            launch_argv(self.app, self.root.as_deref(), &self.extra)?
        } else {
            let program = built_binary(&target, self.root.as_deref()).ok_or_else(|| format!("not built yet: {}", target.bin))?;
            (program, self.process_args())
        };
        let mut command = Command::new(program);
        command.args(args);
        crate::child_env::scrub(&mut command);
        if self.building || self.run {
            crate::cargo_env(&mut command);
        }
        // The registry's launch, whatever its launcher's environment: a lane shell's target dir
        // and test switches reach neither the app nor a checkout's cargo (which builds into the
        // checkout's own target/, as the prebuild did). A Builder build keeps its shared target.
        if !self.building || self.run || crate::builder_install().is_none() {
            crate::child_env::scrub_lane(&mut command);
        }
        // The app itself gets this machine's settings for it (`machine::launch_env`); a host
        // adds its own after this.
        if !self.building || self.run {
            command.envs(crate::machine::launch_env(self.app.id, self.runner == Runner::Jit));
            // A checkout devhub syncs (its `FLEET-REVISION.txt`): the JIT applies changes to it
            // only after a sync of landed work, never what a lane or an edit writes there
            // (relax's `DockGate`).
            if let (Runner::Jit, Some(revision)) = (self.runner, self.root.as_ref().map(|r| r.join(crate::machine::FLEET_REVISION)).filter(|r| r.is_file())) {
                command.env("MAKEPAD_RUN_DOCK_REVISION", revision);
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
    fn started(id: &str, runner: Runner, root: &Path) -> (Vec<String>, LaunchKind) {
        let mut seen = None;
        let entry = crate::find(id).unwrap();
        let _ = AppLaunch::start(entry, runner, Some(root.to_path_buf()), vec!["--x".into()], BuildPolicy::IfMissing, false, |cmd, kind| {
            seen = Some((cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect(), kind));
            Err("captured".into())
        });
        seen.expect("start spawned")
    }

    /// Under the JIT a checkout launch is `cargo run` of the makepad tool with `run <package>`, the
    /// app's arguments after `--`, and no `--stdin-loop` (the HAL finds the WM by its socket).
    #[test]
    fn a_jit_launch_runs_the_package_under_the_makepad_tool() {
        let root = crate::checkout_at_or_above(Path::new(env!("CARGO_MANIFEST_DIR"))).expect("tests run in a checkout");
        let (args, kind) = started("terminal", Runner::Jit, &root);
        assert_eq!(kind, LaunchKind::Run);
        let tail: Vec<&str> = args.iter().map(String::as_str).skip_while(|a| *a != "-p").collect();
        assert_eq!(tail, ["-p", "makepad-run", "--bin", "makepad", "--", "run", "makepad-app-terminal", "--", "--x"]);
        let (native, _) = started("terminal", Runner::Native, &root);
        assert!(native.iter().any(|a| a == "--stdin-loop"), "{native:?}");
        assert!(native.iter().any(|a| a == "makepad-app-terminal"));
    }

    /// A JIT launch from a checkout devhub syncs (it holds `FLEET-REVISION.txt`) carries the dock
    /// gate (`MAKEPAD_RUN_DOCK_REVISION`: only a sync of landed work changes that app's code); a
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
            let _ = AppLaunch::start(entry, Runner::Jit, Some(root.to_path_buf()), Vec::new(), BuildPolicy::IfMissing, false, |cmd, _| {
                env = cmd.get_envs().find(|(k, _)| *k == "MAKEPAD_RUN_DOCK_REVISION").and_then(|(_, v)| v.map(|v| v.to_os_string()));
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
    /// MAKEPAD is another profile/fingerprint: 165 recompiled 97 crates at a JIT launch).
    #[test]
    fn the_prebuild_is_the_launch_with_build_for_run() {
        let root = crate::checkout_at_or_above(Path::new(env!("CARGO_MANIFEST_DIR"))).expect("tests run in a checkout");
        for (id, runner) in [("sandbox", Runner::Jit), ("terminal", Runner::Native)] {
            let entry = crate::find(id).unwrap();
            let mut run = None;
            let _ = AppLaunch::start(entry, runner, Some(root.clone()), Vec::new(), BuildPolicy::IfMissing, false, |cmd, _| {
                run = Some(cmd);
                Err("captured".into())
            });
            let run = run.unwrap();
            let build = AppLaunch::build_command(entry, runner, root.clone()).unwrap();
            let args = |c: &Command| c.get_args().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>();
            // what this machine sets for the app at run time is not a build input
            let machine: Vec<String> = crate::machine::launch_env(id, runner == Runner::Jit).into_iter().map(|(k, _)| k).collect();
            let envs = |c: &Command| {
                let mut v: Vec<_> = c.get_envs().filter(|(k, _)| !machine.iter().any(|m| k.to_str() == Some(m.as_str()))).map(|(k, v)| (k.to_os_string(), v.map(|v| v.to_os_string()))).collect();
                v.sort();
                v
            };
            let (run_args, build_args) = (args(&run), args(&build));
            let cut = run_args.iter().position(|a| a == "--").unwrap();
            assert_eq!(run_args[0], "run");
            assert_eq!(build_args[0], "build");
            assert_eq!(run_args[1..cut], build_args[1..], "{id}");
            assert_eq!(envs(&run), envs(&build), "{id}");
            assert_eq!(run.get_current_dir(), build.get_current_dir());
        }
    }
}

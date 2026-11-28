//! The owned app process: native release build or JIT run, `--remote` launch, and shutdown.
//!
//! Everything here blocks on the test runner's thread only. The child's
//! stdout/stderr go to files in the artifact directory, so no reader threads
//! are needed and the transcripts survive as failure artifacts.

use crate::error::{TestError, TestResult};
use crate::remote::RemoteClient;
use makepad_micro_serde::JsonValue;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const GRACEFUL_EXIT_TIMEOUT: Duration = Duration::from_secs(8);
const LOST_RESPONSE_EXIT_TIMEOUT: Duration = Duration::from_millis(250);
const TAIL_BYTES: usize = 4096;

#[derive(Clone, Debug)]
pub struct LaunchSpec {
    pub manifest_dir: PathBuf,
    pub artifacts_dir: PathBuf,
    pub env: HashMap<String, String>,
    pub args: Vec<String>,
    pub visible: bool,
    pub startup_timeout: Duration,
    /// Under the JIT, how long the launched processes may sit idle before their run announces its
    /// endpoint ([`Startup`]).
    pub startup_idle: Duration,
    pub poll_interval: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListeningLine {
    pub pid: u32,
    pub app: String,
    pub socket: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownOutcome {
    /// The owned process exited after a graceful shutdown request, even if
    /// the response was lost as the remote service stopped.
    Graceful,
    /// The process had already exited before shutdown was asked for.
    AlreadyExited,
    /// The process outlived graceful shutdown and the owned child was killed.
    Killed,
}

pub struct OwnedApp {
    /// The process launched: the app, or under the JIT on macOS and Windows the parent its run
    /// stands behind (relax restart_parent.rs), which ends its run as it ends.
    child: Child,
    /// The app's process, the one its endpoint names: the launched one, or the run behind it.
    pub pid: u32,
    pub executable: PathBuf,
    pub client: RemoteClient,
    /// Where `grab` and `gq` write their PNG files (client side): the folder
    /// `makepad_remote::app` names `<tmp>/makepad-remote/<app>-<pid>`.
    pub grab_dir: PathBuf,
    pub stderr_path: PathBuf,
    exit_status: Option<ExitStatus>,
}

impl OwnedApp {
    /// Exit status if the process is gone, without blocking.
    pub fn poll_exit(&mut self) -> Option<ExitStatus> {
        if self.exit_status.is_none() {
            if let Ok(Some(status)) = self.child.try_wait() {
                self.exit_status = Some(status);
            }
        }
        self.exit_status
    }

    pub fn wait_exit(&mut self, timeout: Duration, poll: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.poll_exit() {
                return Some(status);
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(poll.min(deadline.saturating_duration_since(Instant::now())));
        }
    }

    /// Capture a final frame and shut down the owned process.
    pub fn shutdown(&mut self, poll: Duration) -> ShutdownOutcome {
        self.shutdown_with_grab(poll, true)
    }

    /// Ask the app to quit and confirm this exact process exits. Failures, drops
    /// and passes keeping artifacts capture a final frame with `gq`. A passing
    /// test can send `/quit` directly to skip GPU readback and PNG encoding.
    /// Only the owned child is killed, and only when graceful shutdown fails.
    pub fn shutdown_with_grab(&mut self, poll: Duration, grab_frame: bool) -> ShutdownOutcome {
        if self.poll_exit().is_some() {
            return ShutdownOutcome::AlreadyExited;
        }
        if grab_frame {
            let grabbed = self.client.grab_and_quit().is_ok();
            // A remote can close its connection while exiting. Always give the
            // child time to exit before treating a failed response as failure.
            let wait = if grabbed {
                GRACEFUL_EXIT_TIMEOUT
            } else {
                LOST_RESPONSE_EXIT_TIMEOUT
            };
            if self.wait_exit(wait, poll).is_some() {
                return ShutdownOutcome::Graceful;
            }
        }
        let _ = self.client.quit();
        if self.wait_exit(GRACEFUL_EXIT_TIMEOUT, poll).is_some() {
            return ShutdownOutcome::Graceful;
        }
        let _ = self.child.kill();
        if let Ok(status) = self.child.wait() {
            self.exit_status = Some(status);
        }
        ShutdownOutcome::Killed
    }

    pub fn stderr_tail(&self) -> String {
        file_tail(&self.stderr_path)
    }
}

impl Drop for OwnedApp {
    fn drop(&mut self) {
        // A panic outside the normal runner must follow the same ownership
        // and graceful cleanup rules as an ordinary test failure.
        if self.poll_exit().is_none() {
            self.shutdown(Duration::from_millis(20));
        }
    }
}

/// `cargo build --release -p <package>` from the package directory (so the
/// owning workspace is found whether it is the root or a nested one) and
/// return the executable Cargo reports for the package's bin target.
pub fn build_release_binary(
    manifest_dir: &Path,
    package_name: &str,
    bin_name: Option<&str>,
    artifacts_dir: &Path,
    target_dir: Option<&str>,
) -> TestResult<PathBuf> {
    fs::create_dir_all(artifacts_dir)?;
    let stderr_path = artifacts_dir.join("build-stderr.txt");
    let stderr_file = File::create(&stderr_path)?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    if let Some(target_dir) = target_dir {
        command.env("CARGO_TARGET_DIR", target_dir);
    }
    let output = command.args([
            "build",
            "--release",
            "-p",
            package_name,
            "--message-format=json-render-diagnostics",
        ])
        .current_dir(manifest_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr_file))
        .output()
        .map_err(|err| TestError::new(format!("failed to run cargo build: {err}")))?;
    if !output.status.success() {
        return Err(TestError::new(format!(
            "cargo build --release -p {package_name} failed ({}); stderr tail:\n{}",
            output.status,
            file_tail(&stderr_path)
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    select_executable(&stdout, package_name, bin_name)
}

/// Pick the package's bin executable out of Cargo's JSON message stream.
pub fn select_executable(
    messages: &str,
    package_name: &str,
    bin_name: Option<&str>,
) -> TestResult<PathBuf> {
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    for line in messages.lines() {
        if !line.contains("\"compiler-artifact\"") {
            continue;
        }
        let Ok(json) = crate::remote::parse_json(line) else {
            continue;
        };
        if json.key("reason").and_then(JsonValue::string).map(String::as_str)
            != Some("compiler-artifact")
        {
            continue;
        }
        let Some(executable) = json.key("executable").and_then(JsonValue::string) else {
            continue;
        };
        let Some(target) = json.key("target") else {
            continue;
        };
        let is_bin = match target.key("kind") {
            Some(JsonValue::Array(kinds)) => kinds
                .iter()
                .any(|kind| kind.string().map(String::as_str) == Some("bin")),
            _ => false,
        };
        if !is_bin {
            continue;
        }
        let package_id = json
            .key("package_id")
            .and_then(JsonValue::string)
            .cloned()
            .unwrap_or_default();
        if !package_id_matches(&package_id, package_name) {
            continue;
        }
        let name = target
            .key("name")
            .and_then(JsonValue::string)
            .cloned()
            .unwrap_or_default();
        candidates.push((name, PathBuf::from(executable)));
    }
    if candidates.is_empty() {
        return Err(TestError::new(format!(
            "cargo build produced no bin executable for package `{package_name}`"
        )));
    }
    if let Some(bin_name) = bin_name {
        return candidates
            .iter()
            .find(|(name, _)| name == bin_name)
            .map(|(_, path)| path.clone())
            .ok_or_else(|| {
                TestError::new(format!(
                    "package `{package_name}` built no bin named `{bin_name}` (built: {})",
                    candidate_names(&candidates)
                ))
            });
    }
    if candidates.len() == 1 {
        return Ok(candidates.remove(0).1);
    }
    candidates
        .iter()
        .find(|(name, _)| name == package_name)
        .map(|(_, path)| path.clone())
        .ok_or_else(|| {
            TestError::new(format!(
                "package `{package_name}` built several bins ({}); set TestConfig::bin_name",
                candidate_names(&candidates)
            ))
        })
}

fn candidate_names(candidates: &[(String, PathBuf)]) -> String {
    candidates
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Cargo spells package ids as `name version (source)` or
/// `source#name@version`; both carry the bare name in a delimited position.
fn package_id_matches(package_id: &str, package_name: &str) -> bool {
    if package_id.starts_with(&format!("{package_name} ")) || package_id == package_name {
        return true;
    }
    if let Some((_, tail)) = package_id.rsplit_once('#') {
        let name = tail.split('@').next().unwrap_or(tail);
        if name == package_name {
            return true;
        }
        // `path+file:///dir/counter#1.0.0`: the name is the last path segment.
        if !tail.contains('@') {
            if let Some((head, _)) = package_id.rsplit_once('#') {
                return head.rsplit('/').next() == Some(package_name);
            }
        }
    }
    false
}

/// Run through the harness's loader and VFS under the JIT; otherwise build
/// and launch the package's native release binary.
pub fn launch_package(
    spec: &LaunchSpec,
    package_name: &str,
    bin_name: Option<&str>,
) -> TestResult<OwnedApp> {
    if let Some(loader) = std::env::var_os("RELAX_LOADER") {
        let loader = PathBuf::from(loader);
        let mut command = Command::new(&loader);
        // no one attends a test's app: nothing parks there (a compile error or a panic ends it,
        // and the test fails with its text)
        command.arg("run").arg("--unattended").arg(package_name);
        if let Some(bin_name) = bin_name {
            command.arg("--bin").arg(bin_name);
        }
        command.args(["--release", "--", "--remote"]).args(&spec.args);
        // Keep the same environment and package cwd. JIT std's Command
        // carries a virtual cwd and its mounts to the child without chdir.
        launch_command(spec, &loader, command, true)
    } else {
        let executable = build_release_binary(
            &spec.manifest_dir,
            package_name,
            bin_name,
            &spec.artifacts_dir,
            spec.env.get("CARGO_TARGET_DIR").map(String::as_str),
        )?;
        launch(spec, &executable)
    }
}

/// Spawn `executable --remote` and wait for its listening line.
pub fn launch(spec: &LaunchSpec, executable: &Path) -> TestResult<OwnedApp> {
    let mut command = Command::new(executable);
    command.arg("--remote").args(&spec.args);
    launch_command(spec, executable, command, false)
}

fn launch_command(
    spec: &LaunchSpec,
    executable: &Path,
    mut command: Command,
    jit: bool,
) -> TestResult<OwnedApp> {
    fs::create_dir_all(&spec.artifacts_dir)?;
    let stdout_path = spec.artifacts_dir.join("app-stdout.txt");
    let stderr_path = spec.artifacts_dir.join("app-stderr.txt");
    let stdout_file = File::create(&stdout_path)?;
    let stderr_file = File::create(&stderr_path)?;
    let mut stderr = BufReader::new(File::open(&stderr_path)?);

    command
        .current_dir(&spec.manifest_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    // TestConfig::visible controls visibility; no inherited or extra env
    // setting may bring a test window forward.
    command.env_remove("MAKEPAD_FOCUS");
    if spec.visible {
        command.env_remove("MAKEPAD_HIDE_WINDOWS");
    } else {
        command.env("MAKEPAD_HIDE_WINDOWS", "1");
    }
    command.env(
        "MAKEPAD_LOOKUP_CLASHES",
        std::path::absolute(spec.artifacts_dir.join("lookup-clashes.txt"))?,
    );
    let mut child = command.spawn().map_err(|err| {
        TestError::new(format!(
            "failed to launch {}: {err}",
            executable.display()
        ))
    })?;
    let pid = child.id();

    // Native compilation has no deadline. JIT compilation happens inside
    // startup, including lazy calls through the first window, so it must
    // remain uncapped too. Child exits still fail every readiness loop.
    let deadline = if jit {
        None
    } else {
        Some(Instant::now() + spec.startup_timeout)
    };
    // where the run announces itself: the registry the child was given, else this process's
    let registry = spec
        .env
        .get("MAKEPAD_REMOTE_REGISTRY")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(makepad_remote::discovery::registry_dir);
    let mut startup = jit.then(|| Startup::new(pid, spec.startup_idle, registry.clone()));
    let mut pending = Vec::new();
    let listening = loop {
        match listening_on_stderr(&mut stderr, &mut pending, pid, jit) {
            Ok(Some(listening)) => break listening,
            Ok(None) => {}
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err);
            }
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(startup_exit(
                executable,
                &status,
                "before its remote came up",
                format!(
                    "stdout tail:\n{}\nstderr tail:\n{}",
                    file_tail(&stdout_path),
                    file_tail(&stderr_path)
                ),
            ));
        }
        // A silent JIT run may only register its endpoint. The record also identifies
        // the app behind a restart parent when its startup line names a different pid.
        if jit {
            if let Some(app) = announced(&registry, pid) {
                break ListeningLine {
                    pid: app.pid,
                    app: app.app,
                    socket: PathBuf::from(app.socket),
                };
            }
            if let Some(idle) = startup.as_mut().and_then(|startup| startup.idle(Instant::now())) {
                // (what the registry holds is read while the processes still run: a record of a
                // process that ended is gone)
                let held = unannounced(&registry, pid);
                let _ = child.kill();
                let _ = child.wait();
                return Err(TestError::new(format!(
                    "{} never announced its endpoint: {idle}; {held}; stderr tail:\n{}",
                    executable.display(),
                    file_tail(&stderr_path)
                )));
            }
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(TestError::new(format!(
                "timed out waiting for {} to print its remote listening line; stderr tail:\n{}",
                executable.display(),
                file_tail(&stderr_path)
            )));
        }
        thread::sleep(spec.poll_interval);
    };
    if !jit && listening.pid != pid {
        let _ = child.kill();
        let _ = child.wait();
        return Err(TestError::new(format!(
            "remote endpoint names pid {} but the owned process is {pid}",
            listening.pid
        )));
    }

    // The socket is bound before the line is printed, but a dial can still
    // race the first accept: retry until the deadline, if any.
    let mut client = loop {
        match RemoteClient::connect(&listening.socket) {
            Ok(client) => break client,
            Err(err) => {
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(startup_exit(
                        executable,
                        &status,
                        "during startup",
                        format!("stderr tail:\n{}", file_tail(&stderr_path)),
                    ));
                }
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(err);
                }
                if let Some(idle) = startup.as_mut().and_then(|startup| startup.idle(Instant::now())) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(TestError::new(format!(
                        "{}'s endpoint {} never took a connection ({err}): {idle}; stderr tail:\n{}",
                        executable.display(),
                        listening.socket.display(),
                        file_tail(&stderr_path)
                    )));
                }
                thread::sleep(spec.poll_interval);
            }
        }
    };
    client.jit = jit;
    // the app's own pid names its grab folder (makepad_remote app.rs `grab_dir`)
    let mut app = OwnedApp {
        child,
        pid: listening.pid,
        executable: executable.to_path_buf(),
        client,
        grab_dir: std::env::temp_dir()
            .join("makepad-remote")
            .join(format!("{}-{}", listening.app, listening.pid)),
        stderr_path,
        exit_status: None,
    };

    // The service is up once `help` answers; a window follows on the first
    // frame. Waiting here keeps `snap` from answering an empty tree to the
    // test's first locator. Under the JIT, the window exists before its first
    // frame has been compiled: wait for that frame and for the program not to be
    // waiting on the JIT, without applying the native startup deadline.
    // Under the JIT this wait has no deadline either: it ends when the app sits idle short of
    // up ([`Startup`]), saying what it waited for.
    let mut help_seen = false;
    let mut waits_for = String::from("its endpoint to answer `help`");
    loop {
        if let Some(status) = app.poll_exit() {
            return Err(startup_exit(
                &app.executable,
                &status,
                "during startup",
                format!("stderr tail:\n{}", app.stderr_tail()),
            ));
        }
        if !help_seen {
            help_seen = app.client.help().is_ok();
        }
        if help_seen {
            let windows = app.client.windows().unwrap_or_default();
            let up = if jit { windows.iter().any(|window| window.pass_dpi > 0.0) } else { !windows.is_empty() };
            if up && (!jit || !app.client.jit_foreground()?) {
                return Ok(app);
            }
            waits_for = match (windows.is_empty(), up) {
                (true, _) => "a window: it has none".into(),
                (false, false) => format!("a drawn frame: none of its {} window(s) drew one", windows.len()),
                (false, true) => "its program to stop waiting on the JIT: it still waits".into(),
            };
        }
        if let Some(idle) = startup.as_mut().and_then(|startup| startup.idle(Instant::now())) {
            let why = format!("{} never came up on {}: it waited for {waits_for}, and {idle}", app.executable.display(), app.client.socket());
            // what the JIT says of the app's state, before it goes
            let capture = app.client.jit_capture(&why).map_or(String::new(), |c| format!("; the JIT's capture: {c}"));
            let _ = app.shutdown(spec.poll_interval);
            return Err(TestError::new(format!("{why}{capture}; stderr tail:\n{}", app.stderr_tail())));
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            let _ = app.shutdown(spec.poll_interval);
            return Err(TestError::new(format!(
                "timed out waiting for {} to open a window on {}",
                app.executable.display(),
                app.client.socket()
            )));
        }
        thread::sleep(spec.poll_interval);
    }
}

/// The endpoint `registry` holds for launched process `pid`: its own, or that of the run it stands
/// behind ([`makepad_remote::discovery::App::answers_to`]).
fn announced(registry: &Path, pid: u32) -> Option<makepad_remote::discovery::App> {
    makepad_remote::discovery::apps_in(registry)
        .into_iter()
        .find(|app| app.answers_to(pid) && !app.app.is_empty() && !app.socket.is_empty())
}

/// What `registry` holds of the processes under launched process `pid`, for a launch that found
/// no endpoint there.
fn unannounced(registry: &Path, pid: u32) -> String {
    unannounced_in(registry, pid, &makepad_remote::procs::tree(pid))
}

/// [`unannounced`] of the processes `tree` (`pid` first).
fn unannounced_in(registry: &Path, pid: u32, tree: &[u32]) -> String {
    let theirs: Vec<String> = makepad_remote::discovery::apps_in(registry)
        .iter()
        .filter(|app| tree.contains(&app.pid))
        .map(|app| match app.restart_parent {
            Some(parent) => format!("pid {} ({}) says it stands behind {parent}", app.pid, app.app),
            None => format!("pid {} ({}) names no parent", app.pid, app.app),
        })
        .collect();
    let tree: Vec<String> = tree.iter().map(u32::to_string).collect();
    if theirs.is_empty() {
        format!("{} holds no endpoint of {pid} or its processes ({})", registry.display(), tree.join(" "))
    } else {
        format!("{} holds none that answers to {pid}, of its processes ({}): {}", registry.display(), tree.join(" "), theirs.join("; "))
    }
}

/// Under the JIT a launch has no deadline: its run compiles for as long as it needs. It waits for
/// the run to announce its endpoint only while the launched process and the processes under it
/// work: a look, every `idle` (a test's `action_timeout`, the time an app may sit idle in a wait),
/// that finds them all under 1/[`IDLE_SHARE`](crate::runtime::IDLE_SHARE) of a core over it,
/// with no process started or ended since the last, is no progress, and the launch fails, saying
/// what the registry holds. Where the platform cannot say what a process ran, it waits.
/// A thread in an uninterruptible kernel wait runs no CPU: one seen so in a window, at looks
/// [`HELD_LOOKS`] to a window, holds the launch there (the disk, a filesystem lock), and it waits
/// on, saying so. An app whose parse thread slept in `balance_dirty_pages` holding btrfs's inode
/// lock, its other JIT threads queued on that lock, read "1.8% of a core in the last 10 s" and
/// failed as never come up (11-21).
struct Startup {
    root: u32,
    idle: Duration,
    registry: PathBuf,
    since: Instant,
    at: Option<Look>,
    /// the tree's threads seen in an uninterruptible kernel wait this window
    held: Vec<makepad_remote::procs::Blocked>,
    /// when its threads are looked at next for such waits
    held_look: Instant,
    /// what the last window that held it said, for a verdict after it
    was_held: Option<String>,
}

/// Looks for threads in an uninterruptible kernel wait in a startup window ([`Startup`]): a
/// tenth of it apart, so a wait the disk holds for a part of the window is seen.
const HELD_LOOKS: u32 = 10;

/// The launched process's tree at a look: its processes, root first, the CPU they ran and the
/// time their threads waited on a run queue (None where the platform cannot say), in ns.
#[derive(Clone, Debug)]
struct Look {
    tree: Vec<u32>,
    cpu_ns: u64,
    wait_ns: Option<u64>,
}

impl Startup {
    fn new(root: u32, idle: Duration, registry: PathBuf) -> Startup {
        let now = Instant::now();
        Startup { root, idle, registry, since: now, at: Self::look(root), held: Vec::new(), held_look: now, was_held: None }
    }

    /// The launched process's tree, the CPU it ran and its run-queue wait, None where the
    /// platform cannot say what it ran.
    fn look(root: u32) -> Option<Look> {
        use makepad_remote::procs::{cpu_ns, run_wait_ns};
        let tree = makepad_remote::procs::tree(root);
        let own = cpu_ns(root)?;
        let rest: u64 = tree[1..].iter().filter_map(|&pid| cpu_ns(pid)).sum();
        let wait_ns = run_wait_ns(root).map(|own| own + tree[1..].iter().filter_map(|&pid| run_wait_ns(pid)).sum::<u64>());
        Some(Look { tree, cpu_ns: own + rest, wait_ns })
    }

    /// Why the launch is idle, once a window ends without progress.
    fn idle(&mut self, now: Instant) -> Option<String> {
        if now >= self.held_look {
            self.held_look = now + self.idle / HELD_LOOKS;
            for &pid in self.at.as_ref().map_or(&[][..], |at| at.tree.as_slice()) {
                for b in makepad_remote::procs::blocked_threads(pid) {
                    if !self.held.iter().any(|h| h.tid == b.tid) {
                        self.held.push(b);
                    }
                }
            }
        }
        let span = now.duration_since(self.since);
        if span < self.idle {
            return None;
        }
        self.since = now;
        let next = Self::look(self.root);
        let before = std::mem::replace(&mut self.at, next.clone());
        let held = std::mem::take(&mut self.held);
        match idle_over(span, &before?, &next?, &held) {
            Window::Works => None,
            // (said, as a disk's hold is: a launch slowed this much is worth a line)
            Window::Starved(said) => {
                eprintln!("[makepad_test] the launch of pid {} waits for a CPU: {said}", self.root);
                None
            }
            Window::Held(said) => {
                eprintln!("[makepad_test] the launch of pid {} waits on: {said}", self.root);
                self.was_held = Some(said);
                None
            }
            Window::Idle(said) => {
                // (what each thread does at the verdict: a lock's or a channel's waiters, or
                // threads that wait for a CPU, tell a hang from a box that starves it)
                let tree = self.at.as_ref().map_or(Vec::new(), |at| at.tree.clone());
                let threads = threads_said(&tree.iter().map(|&pid| (pid, makepad_remote::procs::threads(pid))).collect::<Vec<_>>());
                let waits = makepad_remote::procs::lock_waits(&tree);
                let locks = if waits.is_empty() { String::new() } else {
                    lock_waits_said(&waits, &makepad_remote::discovery::apps_in(&self.registry))
                };
                Some(match &self.was_held {
                    Some(held) => format!("{said}{threads}{locks} (an earlier window: {held})"),
                    None => format!("{said}{threads}{locks}"),
                })
            }
        }
    }
}

/// How a startup window went ([`idle_over`]).
#[derive(Debug, PartialEq)]
enum Window {
    /// it ran, or a process started or ended
    Works,
    /// it ran next to nothing, but its threads waited for a CPU about as long as a working
    /// launch runs: a saturated box's, not idle
    Starved(String),
    /// it ran next to nothing, with threads in an uninterruptible kernel wait: held there
    Held(String),
    /// it sat idle: why
    Idle(String),
}

/// How the tree went from `before` to `now` over `span`, its threads seen in an uninterruptible
/// kernel wait meanwhile `held`, and how it read.
fn idle_over(span: Duration, before: &Look, now: &Look, held: &[makepad_remote::procs::Blocked]) -> Window {
    if before.tree != now.tree {
        return Window::Works;
    }
    let cores = now.cpu_ns.saturating_sub(before.cpu_ns) as f64 / span.as_nanos().max(1) as f64;
    if cores * crate::runtime::IDLE_SHARE as f64 >= 1.0 {
        return Window::Works;
    }
    let ran = format!("it and the processes under it ran {:.1}% of a core in the last {:.1} s", cores * 100.0, span.as_secs_f64());
    // what it ran and what it waited on a run queue for, together a working launch's share: it
    // wants the CPU a saturated box does not give it (a JIT run's start among four test jobs ran
    // 2.4% of a core and was taken for idle, 11-26). Where the platform cannot say, the rule is
    // the CPU it ran alone.
    if let (Some(a), Some(b)) = (before.wait_ns, now.wait_ns) {
        let waited = b.saturating_sub(a) as f64 / span.as_nanos().max(1) as f64;
        if (cores + waited) * crate::runtime::IDLE_SHARE as f64 >= 1.0 {
            return Window::Starved(format!("{ran} and waited {:.1}% of a core on a run queue", waited * 100.0));
        }
    }
    if held.is_empty() {
        return Window::Idle(format!("{ran}, with no process started or ended"));
    }
    // (by the kernel function they sleep in: a lock's waiters are one line)
    let mut by_wait: Vec<(&str, Vec<String>)> = Vec::new();
    for b in held {
        let wait = if b.wchan.is_empty() { "a kernel wait it does not name" } else { b.wchan.as_str() };
        let who = format!("{} ({}) of {}", b.tid, b.name, b.pid);
        match by_wait.iter_mut().find(|(w, _)| *w == wait) {
            Some((_, who_all)) => who_all.push(who),
            None => by_wait.push((wait, vec![who])),
        }
    }
    let waits: Vec<String> = by_wait.iter().map(|(wait, who)| format!("{} in {wait} ({})", if who.len() == 1 { "thread".to_string() } else { format!("{} threads", who.len()) }, who.join(", "))).collect();
    Window::Held(format!("the disk or a filesystem lock holds it: {ran}, its threads in uninterruptible kernel waits: {}", waits.join("; ")))
}

/// Every thread of each process, by its state and the kernel function it sleeps in (a lock's
/// waiters on one line): `; its threads: pid 2: 2 S in futex_wait_queue (7 main, 8 jit-1), 1 R
/// (9 parse)`. Empty where the platform says nothing of its threads.
fn threads_said(procs: &[(u32, Vec<makepad_remote::procs::Thread>)]) -> String {
    let mut said = Vec::new();
    for (pid, threads) in procs.iter().filter(|(_, threads)| !threads.is_empty()) {
        said.push(format!("pid {pid}: {}", format_threads(threads)));
    }
    if said.is_empty() { String::new() } else { format!("; its threads: {}", said.join("; ")) }
}

/// Groups one process's threads by state and kernel wait channel.
fn format_threads(threads: &[makepad_remote::procs::Thread]) -> String {
    let mut by_wait: Vec<((char, &str), Vec<String>)> = Vec::new();
    for t in threads {
        let who = format!("{} {}", t.tid, t.name);
        match by_wait.iter_mut().find(|(k, _)| *k == (t.state, t.wchan.as_str())) {
            Some((_, who_all)) => who_all.push(who),
            None => by_wait.push(((t.state, t.wchan.as_str()), vec![who])),
        }
    }
    by_wait.iter()
        .map(|((state, wchan), who)| format!("{} {state}{} ({})", who.len(), if wchan.is_empty() { String::new() } else { format!(" in {wchan}") }, who.join(", ")))
        .collect::<Vec<_>>().join(", ")
}

/// Identifies a holder by its announced app or by the process details still readable.
fn holder_said(holder: &makepad_remote::procs::LockHolder, apps: &[makepad_remote::discovery::App]) -> String {
    let Some(pid) = holder.pid else { return format!("pid unknown ({} {})", holder.kind, holder.access) };
    let identity = if let Some(app) = apps.iter().find(|a| a.answers_to(pid)) {
        format!("app={}", app.app)
    } else {
        let mut command = holder.cmdline.join(" ");
        if command.chars().nth(160).is_some() {
            command = command.chars().take(157).collect();
            command.push_str("...");
        }
        match (holder.name.as_deref(), command.is_empty()) {
            (Some(name), false) => format!("{name}: {command}"),
            (Some(name), true) => name.to_owned(),
            (None, false) => command,
            (None, true) => "gone or unreadable".into(),
        }
    };
    let threads = if holder.threads.is_empty() { "unreadable".into() } else { format_threads(&holder.threads) };
    format!("pid {pid} ({identity}; {}; threads: {threads})", holder.access)
}

/// Lock requests observed at the idle verdict, including inode identity when the path is unknown.
fn lock_waits_said(waits: &[makepad_remote::procs::LockWait], apps: &[makepad_remote::discovery::App]) -> String {
    let mut said = String::new();
    for w in waits {
        let path = w.path.as_deref().map_or_else(|| "unknown path".into(), |p| p.display().to_string());
        let holders = if w.holders.is_empty() {
            "holder gone before inspect".into()
        } else {
            format!("held by {}", w.holders.iter().map(|h| holder_said(h, apps)).collect::<Vec<_>>().join(", "))
        };
        said.push_str(&format!("; lock wait: pid {} waits for {} {} on {path} (dev {:02x}:{:02x} ino {}), {holders}",
            w.waiter_pid, w.kind, w.want, w.dev_major, w.dev_minor, w.ino));
    }
    said
}

/// Read new stderr lines once. A file read can end in the middle of the socket path.
fn listening_on_stderr(stderr: &mut impl BufRead, pending: &mut Vec<u8>, pid: u32, jit: bool) -> TestResult<Option<ListeningLine>> {
    while stderr.read_until(b'\n', pending)? != 0 {
        if !pending.ends_with(b"\n") { break; }
        let line = String::from_utf8_lossy(pending);
        let tag_and_fields = line.strip_prefix('[').and_then(|s| s.split_once("] "));
        if let Some((_, fields)) = tag_and_fields.filter(|(tag, _)| tag.ends_with("-remote")) {
            if fields.starts_with("cannot listen") || fields.starts_with("iOS has no local remote-control route") {
                return Err(TestError::new(format!("app remote failed to start: {}", line.trim_end())));
            }
            if fields.starts_with("pid=") {
                let listening = parse_listening_line(&line)?;
                if !jit || listening.pid == pid { return Ok(Some(listening)); }
            }
        }
        pending.clear();
    }
    Ok(None)
}

/// `[makepad-remote] pid=9931 app=NAME socket=/tmp/makepad-remote-9931.sock`
pub fn parse_listening_line(line: &str) -> TestResult<ListeningLine> {
    let (pid, app, socket) = makepad_remote::parse_startup_line(line)
        .ok_or_else(|| TestError::new(format!("invalid remote listening line: {}", line.trim_end())))?;
    Ok(ListeningLine { pid, app, socket })
}

/// An app that ended before it was up. An app on a JIT older than its tree in a crate linked
/// into that JIT stops at startup ("The JIT itself changed (<crate>)"): the run's stale JIT,
/// never the app's own failure.
fn startup_exit(
    executable: &Path,
    status: &dyn std::fmt::Display,
    when: &str,
    tails: String,
) -> TestError {
    const STALE: &str = "The JIT itself changed (";
    if let Some(krate) = tails.split(STALE).nth(1).and_then(|rest| rest.split(')').next()) {
        return TestError::new(format!(
            "{} stopped {when}: a stale JIT, not a test failure: the JIT running it is older \
             than the app's tree in {krate}, a crate linked into the JIT; run it on a JIT built \
             from this tree\n{tails}",
            executable.display()
        ));
    }
    TestError::new(format!(
        "{} exited ({status}) {when}; {tails}",
        executable.display()
    ))
}

pub fn file_tail(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    if text.len() <= TAIL_BYTES {
        return text;
    }
    let mut start = text.len() - TAIL_BYTES;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_wait(holder: makepad_remote::procs::LockHolder) -> makepad_remote::procs::LockWait {
        makepad_remote::procs::LockWait {
            waiter_pid: 201, kind: "FLOCK".into(), want: "WRITE".into(),
            dev_major: 0x103, dev_minor: 2, ino: 987654, path: Some("/cache/core.rcs".into()), holders: vec![holder],
        }
    }

    #[test]
    fn lock_waits_said_names_path_announced_app_and_holder_threads() {
        use makepad_remote::procs::{LockHolder, Thread};
        let wait = lock_wait(LockHolder {
            pid: Some(42), kind: "FLOCK".into(), access: "WRITE".into(), name: None, cmdline: vec![],
            threads: vec![
                Thread { tid: 42, name: "main".into(), state: 'R', wchan: String::new() },
                Thread { tid: 43, name: "writer".into(), state: 'D', wchan: "btrfs_commit".into() },
            ],
        });
        let app = makepad_remote::discovery::parse(r#"{"pid":42,"app":"demo-app","socket":"/s","caps":[]}"#).unwrap();
        assert_eq!(lock_waits_said(&[wait], &[app]),
            "; lock wait: pid 201 waits for FLOCK WRITE on /cache/core.rcs (dev 103:02 ino 987654), held by pid 42 (app=demo-app; WRITE; threads: 1 R (42 main), 1 D in btrfs_commit (43 writer))");
    }

    #[test]
    fn lock_waits_said_falls_back_to_cmdline_and_handles_unreadable_threads() {
        let wait = lock_wait(makepad_remote::procs::LockHolder {
            pid: Some(99), kind: "FLOCK".into(), access: "WRITE".into(), name: Some("tool".into()),
            cmdline: vec!["tool".into(), "run".into(), "demo".into()], threads: vec![],
        });
        let gone = makepad_remote::procs::LockWait { path: None, holders: vec![], ino: 123, ..wait.clone() };
        let said = lock_waits_said(&[wait, gone], &[]);
        assert!(said.contains("pid 99 (tool: tool run demo; WRITE; threads: unreadable)"), "{said}");
        assert!(said.contains("(dev 103:02 ino 123), holder gone before inspect"), "{said}");
        assert_eq!(lock_waits_said(&[], &[]), "");
    }

    #[test]
    fn listening_line_parses_every_field() {
        let line = "[makepad-remote] pid=9931 app=makepad-example-counter socket=/tmp/my dir/makepad-remote-9931.sock";
        let parsed = parse_listening_line(line).unwrap();
        assert_eq!(
            parsed,
            ListeningLine {
                pid: 9931,
                app: "makepad-example-counter".to_string(),
                socket: PathBuf::from("/tmp/my dir/makepad-remote-9931.sock"),
            }
        );
        assert!(parse_listening_line("[makepad-remote] cannot listen: denied").is_err());
        assert!(parse_listening_line("[makepad-remote] pid=1 app=x").is_err());
        assert!(parse_listening_line("[makepad-remote] pid=nope app=x socket=/s").is_err());
        let (start, end) = line.split_at(line.find(" dir").unwrap());
        let mut pending = Vec::new();
        assert_eq!(listening_on_stderr(&mut start.as_bytes(), &mut pending, 9931, false).unwrap(), None, "a partial socket path waits");
        assert_eq!(listening_on_stderr(&mut format!("{end}\n").as_bytes(), &mut pending, 9931, false).unwrap(), Some(parsed), "the next read completes the socket path");
    }

    #[test]
    fn executable_selection_prefers_the_package_bin() {
        let messages = concat!(
            r#"{"reason":"compiler-artifact","package_id":"path+file:///r/libs/x#makepad-x@1.0.0","target":{"kind":["custom-build"],"name":"build-script-build"},"executable":"/t/build/x-1/build-script-build"}"#,
            "\n",
            r#"{"reason":"compiler-artifact","package_id":"path+file:///r/examples/counter#makepad-example-counter@1.0.0","target":{"kind":["bin"],"name":"makepad-example-counter"},"executable":"/t/release/makepad-example-counter"}"#,
            "\n",
            r#"{"reason":"build-finished","success":true}"#,
        );
        assert_eq!(
            select_executable(messages, "makepad-example-counter", None).unwrap(),
            PathBuf::from("/t/release/makepad-example-counter")
        );
        assert!(select_executable(messages, "makepad-x", None).is_err());
        assert!(select_executable(messages, "makepad-example-counter", Some("other")).is_err());
    }

    #[test]
    fn executable_selection_handles_multiple_bins_and_old_ids() {
        let messages = concat!(
            r#"{"reason":"compiler-artifact","package_id":"makepad-studio 0.1.0 (path+file:///r/apps/studio)","target":{"kind":["bin"],"name":"studio"},"executable":"/t/release/studio"}"#,
            "\n",
            r#"{"reason":"compiler-artifact","package_id":"makepad-studio 0.1.0 (path+file:///r/apps/studio)","target":{"kind":["bin"],"name":"studio-cli"},"executable":"/t/release/studio-cli"}"#,
        );
        assert!(select_executable(messages, "makepad-studio", None).is_err());
        assert_eq!(
            select_executable(messages, "makepad-studio", Some("studio")).unwrap(),
            PathBuf::from("/t/release/studio")
        );
        assert!(package_id_matches("path+file:///r/examples/counter#1.0.0", "counter"));
        assert!(!package_id_matches("path+file:///r/examples/counter#1.0.0", "count"));
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "makepad-test-{name}-{}-{:?}-{}",
            std::process::id(),
            thread::current().id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        fs::create_dir(&dir).unwrap();
        dir
    }

    fn fake_app_spec(artifacts_dir: PathBuf) -> LaunchSpec {
        LaunchSpec {
            manifest_dir: artifacts_dir.clone(),
            artifacts_dir,
            env: HashMap::new(),
            args: Vec::new(),
            visible: false,
            startup_timeout: Duration::from_secs(10),
            // a fake app's shell sits idle while it waits to be asked: the tests that judge
            // idleness set their own window
            startup_idle: Duration::from_secs(5),
            poll_interval: Duration::from_millis(20),
        }
    }

    /// A shell script standing in for the app binary: it prints the remote
    /// startup line (pointing at a fixture endpoint), then runs `script_tail`
    /// (`exec sleep 60`: it lives until killed, ignoring shutdown requests like
    /// a wedged app would; `when_asked(.., "quit")`: it exits when asked to quit).
    fn write_fake_app(dir: &Path, socket: &Path, script_tail: &str) -> PathBuf {
        write_script(
            dir,
            "fake-app.sh",
            &format!(
                "#!/bin/sh\necho \"[makepad-remote] pid=$$ app=fake-app socket={}\" >&2\n{script_tail}\n",
                socket.display()
            ),
        )
    }

    /// An executable script `dir/name` holding `text`. Every spawn forks this process, and until
    /// the child execs it holds all our open files: a script written here could still be open for
    /// writing in another test's child when this test runs it ("Text file busy"). So `install`, a
    /// process of its own and waited for, writes the script from a side file: no handle that
    /// writes it ever lives in this process.
    fn write_script(dir: &Path, name: &str, text: &str) -> PathBuf {
        let source = dir.join(format!("{name}.text"));
        fs::write(&source, text).unwrap();
        let path = dir.join(name);
        assert!(Command::new("install").arg("-m").arg("755").arg(&source).arg(&path).status().unwrap().success());
        path
    }

    fn launch_error(result: TestResult<OwnedApp>) -> TestError {
        match result {
            Ok(mut app) => {
                let _ = app.shutdown(Duration::from_millis(20));
                panic!("launch unexpectedly succeeded");
            }
            Err(err) => err,
        }
    }

    #[cfg(unix)]
    fn fixture_with_window() -> crate::fixture::FixtureServer {
        let server = crate::fixture::FixtureServer::start();
        server.route("help", "makepad-remote app=fake-app");
        server.route(
            "status",
            r#"{"app":"fake-app","pid":1,"w":[{"i":0,"t":"fake","sz":[10,10],"px":[10,10],"dpi":1,"pass_dpi":1,"pos":[0,0]}]}"#,
        );
        server
    }

    /// A fake app's script lines that return once the app is asked `command` (the app's own
    /// next step, as on `quit`): they wait on a FIFO the fixture writes on the first `command`
    /// (a later one would find no reader).
    #[cfg(unix)]
    fn when_asked(dir: &Path, server: &crate::fixture::FixtureServer, command: &str) -> String {
        let fifo = dir.join(format!("{command}.fifo"));
        assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        let asked = std::sync::atomic::AtomicBool::new(false);
        let path = fifo.clone();
        server.on(command, move || {
            if !asked.swap(true, std::sync::atomic::Ordering::Relaxed) {
                fs::write(&path, "asked\n").unwrap();
            }
        });
        format!("read _ < '{}'", fifo.display())
    }

    /// A JIT launch as on macOS and Windows: the process launched is a parent that starts the run
    /// and waits, and the run announces its endpoint in the registry under its own pid, naming the
    /// parent when `names_parent` (relax restart_parent.rs). The run lives until the parent ends:
    /// it reads a FIFO only the parent holds open, as the real run reads its lifeline. The parent
    /// ends when `parent_tail` returns (`wait`: when it is killed).
    #[cfg(unix)]
    fn write_fake_jit_app(dir: &Path, socket: &Path, names_parent: bool, parent_tail: &str) -> PathBuf {
        // (`%.0s` takes the parent's pid and prints nothing)
        let parent = if names_parent { r#","restart_parent":%s"# } else { "%.0s" };
        let run = format!(
            r#"printf '{{"pid":%s,"app":"fake-app","socket":"%s","caps":[]{parent}}}\n' $$ "$1" "$2" > "$3/.next-$$" && mv "$3/.next-$$" "$3/makepad-remote-$$.json"; read _ < "$4""#
        );
        write_script(
            dir,
            "fake-jit-app.sh",
            &format!(
                "#!/bin/sh\nlife=\"$MAKEPAD_REMOTE_REGISTRY/life-$$\"\nmkfifo \"$life\"\nsh -c '{}' run '{}' \"$$\" \"$MAKEPAD_REMOTE_REGISTRY\" \"$life\" &\nexec 3> \"$life\"\n{parent_tail}\n",
                run.replace('\'', r#"'\''"#),
                socket.display()
            ),
        )
    }

    #[cfg(unix)]
    #[test]
    fn a_jit_launch_finds_the_run_behind_its_parent_and_fails_by_name_where_it_sits_idle() {
        let dir = scratch_dir("jit-parent");
        let registry = dir.join("registry");
        fs::create_dir(&registry).unwrap();
        let server = fixture_with_window();
        // (the owner's queue state as the JIT answers it, its pending app check too)
        server.route("owner", r#"{"queued":{"foreground":0,"main":0,"demand":0,"check":0}}"#);
        let mut spec = fake_app_spec(dir.clone());
        spec.env.insert("MAKEPAD_REMOTE_REGISTRY".into(), registry.display().to_string());
        // (the fake spec's 5 s idle window: a shell starting on a loaded box runs a few ms in a
        // short one, idle by the rule, which a JIT run's start never is)

        // the run's record names the launched process as the parent it stands behind
        let script = write_fake_jit_app(&dir, server.socket(), true, "wait");
        let mut app = launch_command(&spec, &script, Command::new(&script), true).unwrap();
        assert_ne!(app.pid, app.child.id(), "the app's pid is the run's, not the parent's");
        assert!(registry.join(format!("makepad-remote-{}.json", app.pid)).exists(), "the run announced itself under its own pid");
        assert_eq!(app.grab_dir.file_name().unwrap().to_string_lossy(), format!("fake-app-{}", app.pid), "grabs are in the run's folder");
        assert_eq!(app.client.socket(), server.socket().display().to_string());
        // the parent ends, and with it the run (the fixture answers no quit)
        app.child.kill().unwrap();
        app.child.wait().unwrap();
        let run = app.pid;
        let ended = (0..250).any(|_| !makepad_remote::procs::is_alive(run) || {
            thread::sleep(Duration::from_millis(20));
            false
        });
        assert!(ended, "the run {run} ends with its parent");

        // a run whose record answers to no pid the launch knows: the launch fails once its
        // processes sit idle, naming the registry ([`unannounced_in`] has what it says)
        spec.startup_idle = Duration::from_millis(200);
        let script = write_fake_jit_app(&dir, server.socket(), false, "wait");
        let err = launch_error(launch_command(&spec, &script, Command::new(&script), true)).to_string();
        assert!(err.contains("never announced its endpoint: it and the processes under it ran") && err.contains(&registry.display().to_string()), "{err}");

        // an app that is up but whose window never draws, sitting idle: the launch fails saying
        // what it waited for, and the app is asked to quit (it ends when asked)
        spec.startup_idle = Duration::from_secs(1);
        let undrawn = crate::fixture::FixtureServer::start();
        undrawn.route("help", "makepad-remote app=fake-app");
        undrawn.route("status", r#"{"app":"fake-app","pid":1,"w":[{"i":0,"t":"fake","sz":[10,10],"px":[10,10],"dpi":1,"pass_dpi":0,"pos":[0,0]}]}"#);
        let script = write_fake_jit_app(&dir, undrawn.socket(), true, &when_asked(&dir, &undrawn, "quit"));
        let err = launch_error(launch_command(&spec, &script, Command::new(&script), true)).to_string();
        assert!(err.contains("never came up on") && err.contains("it waited for a drawn frame: none of its 1 window(s) drew one, and it and the processes under it ran"), "{err}");
        assert!(undrawn.requests().iter().any(|r| r == "quit"), "the app was asked to quit: {:?}", undrawn.requests());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_launch_is_idle_only_when_its_processes_sat_still_and_says_what_the_registry_holds() {
        let span = Duration::from_secs(10);
        let tree = |pids: &[u32], cpu_ms: u64| Look { tree: pids.to_vec(), cpu_ns: cpu_ms * 1_000_000, wait_ns: None };
        let waited = |pids: &[u32], cpu_ms: u64, wait_ms: u64| Look { wait_ns: Some(wait_ms * 1_000_000), ..tree(pids, cpu_ms) };
        // a twentieth of a core and up is work, a process started or ended is progress
        assert_eq!(idle_over(span, &tree(&[1, 2], 0), &tree(&[1, 2], 500), &[]), Window::Works, "5% of a core works");
        assert_eq!(idle_over(span, &tree(&[1], 0), &tree(&[1, 2], 0), &[]), Window::Works, "a process started");
        assert_eq!(idle_over(span, &tree(&[1, 2], 0), &tree(&[1], 0), &[]), Window::Works, "a process ended");
        let Window::Idle(idle) = idle_over(span, &tree(&[1, 2], 100), &tree(&[1, 2], 400), &[]) else { panic!("3% of a core with the same processes is idle") };
        assert!(idle.contains("3.0% of a core in the last 10.0 s"), "{idle}");
        // a launch that waited on a run queue for a CPU, with what it ran a working share, is a
        // saturated box's, not idle; one that waited little is idle, as one whose platform cannot
        // say how long it waited
        let Window::Starved(starved) = idle_over(span, &waited(&[1, 2], 100, 1_000), &waited(&[1, 2], 340, 1_400), &[]) else { panic!("2.4% of a core run and 4% waited is starved") };
        assert!(starved.contains("2.4% of a core in the last 10.0 s") && starved.contains("waited 4.0% of a core on a run queue"), "{starved}");
        assert!(matches!(idle_over(span, &waited(&[1, 2], 100, 1_000), &waited(&[1, 2], 340, 1_100), &[]), Window::Idle(_)), "2.4% run and 1% waited is idle");
        assert!(matches!(idle_over(span, &waited(&[1, 2], 100, 1_000), &tree(&[1, 2], 340), &[]), Window::Idle(_)), "no wait to compare: the CPU alone decides");
        // the wait read from a schedstat line (ns on a CPU, ns waited, timeslices); this process's
        // threads waited some time where the kernel keeps the statistics
        assert_eq!(makepad_remote::procs::schedstat_wait_ns("8321554 120998 37\n"), Some(120_998));
        assert_eq!(makepad_remote::procs::schedstat_wait_ns("8321554 120998"), None, "a short line says nothing");
        assert_eq!(makepad_remote::procs::schedstat_wait_ns("cpu wait slices"), None);
        if std::path::Path::new("/proc/self/task").exists() && std::path::Path::new(&format!("/proc/self/task/{}/schedstat", std::process::id())).exists() {
            assert!(makepad_remote::procs::run_wait_ns(std::process::id()).is_some(), "this process's run-queue wait");
        }
        // threads in an uninterruptible kernel wait run no CPU: the disk holds the launch, which
        // waits on, saying which threads sleep where (a lock's waiters on one line)
        let blocked = |tid: u32, name: &str, wchan: &str| makepad_remote::procs::Blocked { pid: 2, tid, name: name.into(), wchan: wchan.into() };
        let held = [blocked(7, "parse-1", "balance_dirty_pages"), blocked(8, "jit-2", "btrfs_inode_lock"), blocked(9, "jit-3", "btrfs_inode_lock")];
        let Window::Held(said) = idle_over(span, &tree(&[1, 2], 100), &tree(&[1, 2], 280), &held) else { panic!("held by the disk, not idle") };
        assert!(said.contains("1.8% of a core in the last 10.0 s") && said.contains("thread in balance_dirty_pages (7 (parse-1) of 2)") && said.contains("2 threads in btrfs_inode_lock (8 (jit-2) of 2, 9 (jit-3) of 2)"), "{said}");
        // read from a thread's stat line: its state D, its name whatever it holds, the kernel's
        // `0` for an unnamed wait; any other state is no such wait
        let stat = |state: &str| format!("4711 (relax parse (2)) {state} 4700 4700 0 -1 4194560 12 0 0 0 9 3 0 0 20 0 31 0");
        assert_eq!(makepad_remote::procs::blocked_of(4700, 4711, &stat("D"), "balance_dirty_pages\n"), Some(makepad_remote::procs::Blocked { pid: 4700, tid: 4711, name: "relax parse (2)".into(), wchan: "balance_dirty_pages".into() }));
        assert_eq!(makepad_remote::procs::blocked_of(4700, 4711, &stat("D"), "0").map(|b| b.wchan), Some(String::new()));
        // an idle verdict says what every thread does, a lock's waiters on one line; read from a
        // stat line whatever its state, so a thread that waits for a CPU (R) shows as one
        let thread = |tid: u32, name: &str, state: char, wchan: &str| makepad_remote::procs::Thread { tid, name: name.into(), state, wchan: wchan.into() };
        assert_eq!(makepad_remote::procs::thread_of(4711, &stat("R"), "0\n"), Some(thread(4711, "relax parse (2)", 'R', "")));
        let said = threads_said(&[(2, vec![thread(7, "main", 'S', "futex_wait_queue"), thread(8, "jit-1", 'S', "futex_wait_queue"), thread(9, "parse", 'R', "")]), (3, Vec::new())]);
        assert_eq!(said, "; its threads: pid 2: 2 S in futex_wait_queue (7 main, 8 jit-1), 1 R (9 parse)");
        assert_eq!(threads_said(&[(3, Vec::new())]), "", "a platform that says nothing of its threads adds nothing");
        if std::path::Path::new(&format!("/proc/self/task/{}/stat", std::process::id())).exists() {
            let mine = makepad_remote::procs::threads(std::process::id());
            assert!(mine.iter().any(|t| t.tid == std::process::id() && t.state != 'Z'), "this process's own threads: {mine:?}");
        }
        assert!(["R", "S", "Z"].iter().all(|state| makepad_remote::procs::blocked_of(4700, 4711, &stat(state), "do_wait").is_none()));

        // a record of a process under the launched one that names another parent, or none at all
        // (the registry keeps only live processes' records: this process stands in for the run)
        let registry = scratch_dir("unannounced");
        let run = std::process::id();
        fs::write(registry.join(format!("makepad-remote-{run}.json")), format!(r#"{{"pid":{run},"app":"fake-app","socket":"/s","caps":[],"restart_parent":1}}"#)).unwrap();
        let said = unannounced_in(&registry, u32::MAX, &[u32::MAX, run]);
        assert!(said.contains(&format!("holds none that answers to {}", u32::MAX)) && said.contains(&format!("pid {run} (fake-app) says it stands behind 1")), "{said}");
        let said = unannounced_in(&registry, u32::MAX, &[u32::MAX]);
        assert!(said.contains(&format!("holds no endpoint of {} or its processes", u32::MAX)), "{said}");
        let _ = fs::remove_dir_all(&registry);
    }

    #[cfg(unix)]
    #[test]
    fn launch_reads_the_listening_line_and_kill_fallback_ends_the_owned_pid() {
        let dir = scratch_dir("launch");
        let server = fixture_with_window();
        let script = write_fake_app(&dir, server.socket(), "exec sleep 60");
        let spec = fake_app_spec(dir.clone());

        let mut app = launch(&spec, &script).unwrap();
        assert_eq!(app.client.socket(), server.socket().display().to_string());
        assert_eq!(
            app.grab_dir,
            std::env::temp_dir()
                .join("makepad-remote")
                .join(format!("fake-app-{}", app.pid))
        );
        assert!(app.poll_exit().is_none());
        assert!(app.stderr_tail().contains("socket="), "stderr carries the startup line");
        assert!(fs::read(spec.artifacts_dir.join("app-stdout.txt")).unwrap().is_empty(), "the silent fake app's stdout stays empty");
        assert!(server.requests().iter().any(|target| target == "help"));
        assert!(server.requests().iter().any(|target| target == "status"));

        // `gq` and `quit` are both refused (unknown command) and the
        // process stays up, so shutdown must fall back to killing exactly this pid.
        let outcome = app.shutdown(Duration::from_millis(20));
        assert_eq!(outcome, ShutdownOutcome::Killed);
        assert!(app.poll_exit().is_some());
        let requests = server.requests();
        assert!(requests.iter().any(|target| target == "quit"));
        assert!(requests.iter().position(|t| t.starts_with("frames ")).unwrap()
            < requests.iter().position(|target| target == "quit").unwrap());
        assert!(requests.iter().filter(|target| *target == "quit").count() >= 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn graceful_quit_is_reported_when_the_process_exits_on_its_own() {
        let dir = scratch_dir("graceful");
        let server = fixture_with_window();
        server.route("quit", r#"{"ok":1}"#);
        // Lives until it is asked to quit, then exits like an app answering `/quit` would.
        let script = write_fake_app(&dir, server.socket(), &when_asked(&dir, &server, "quit"));
        let spec = fake_app_spec(dir.clone());

        let mut app = launch(&spec, &script).unwrap();
        assert_eq!(app.shutdown(Duration::from_millis(20)), ShutdownOutcome::Graceful);
        assert_eq!(app.shutdown(Duration::from_millis(20)), ShutdownOutcome::AlreadyExited);
        // `gq` (frames written client side, then `quit`) was enough: no second `quit`.
        let requests = server.requests();
        assert!(requests.iter().any(|target| target.starts_with("frames ")));
        assert_eq!(requests.iter().filter(|target| *target == "quit").count(), 1);
        assert!(app.poll_exit().unwrap().success());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn passing_shutdown_skips_frame_grab_and_waits_for_graceful_exit() {
        let dir = scratch_dir("passing-shutdown");
        let server = fixture_with_window();
        server.route("quit", r#"{"ok":1}"#);
        let script = write_fake_app(&dir, server.socket(), &when_asked(&dir, &server, "quit"));
        let spec = fake_app_spec(dir.clone());
        let mut app = launch(&spec, &script).unwrap();

        assert_eq!(app.shutdown_with_grab(spec.poll_interval, false), ShutdownOutcome::Graceful);
        assert!(app.poll_exit().unwrap().success());
        assert_eq!(app.shutdown_with_grab(spec.poll_interval, false), ShutdownOutcome::AlreadyExited);
        let requests = server.requests();
        assert!(!requests.iter().any(|r| r.starts_with("frames ")), "{requests:?}");
        assert_eq!(requests.iter().filter(|r| *r == "quit").count(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn passing_shutdown_handles_lost_quit_response_and_kill_fallback() {
        let dir = scratch_dir("passing-lost-quit");
        let spec = fake_app_spec(dir.clone());
        for expected in [ShutdownOutcome::Graceful, ShutdownOutcome::Killed] {
            let server = fixture_with_window();
            server.disconnect("quit");
            let tail = if expected == ShutdownOutcome::Graceful {
                when_asked(&dir, &server, "quit")
            } else {
                "exec sleep 60".to_string()
            };
            let script = write_fake_app(&dir, server.socket(), &tail);
            let mut app = launch(&spec, &script).unwrap();
            assert_eq!(app.shutdown_with_grab(spec.poll_interval, false), expected);
            assert_eq!(app.poll_exit().unwrap().success(), expected == ShutdownOutcome::Graceful);
            assert!(!makepad_remote::procs::is_alive(app.pid), "owned pid {} survived {expected:?}", app.pid);
            let requests = server.requests();
            assert!(!requests.iter().any(|r| r.starts_with("frames ")), "{requests:?}");
            assert_eq!(requests.iter().filter(|r| *r == "quit").count(), 1);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn drop_on_live_owned_app_grabs_frame_before_quit() {
        let dir = scratch_dir("drop-live-app");
        let server = fixture_with_window();
        server.route("quit", r#"{"ok":1}"#);
        let script = write_fake_app(&dir, server.socket(), &when_asked(&dir, &server, "quit"));
        let mut app = launch(&fake_app_spec(dir.clone()), &script).unwrap();
        assert!(app.poll_exit().is_none());
        let pid = app.pid;
        drop(app);

        let requests = server.requests();
        let frames = requests.iter().position(|r| r.starts_with("frames ")).expect("final frame request");
        let quit = requests.iter().position(|r| r == "quit").expect("quit request");
        assert!(frames < quit, "{requests:?}");
        assert!(!makepad_remote::procs::is_alive(pid), "owned pid {pid} survived drop");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn lost_shutdown_responses_still_allow_the_owned_child_to_exit() {
        let dir = scratch_dir("lost-shutdown-responses");
        let server = fixture_with_window();
        // `gq` ends in `quit`, whose answer is lost: the exiting child still counts as graceful.
        server.disconnect("quit");
        let script = write_fake_app(&dir, server.socket(), &when_asked(&dir, &server, "quit"));
        let spec = fake_app_spec(dir.clone());
        let mut app = launch(&spec, &script).unwrap();

        assert_eq!(app.shutdown(Duration::from_millis(20)), ShutdownOutcome::Graceful);
        assert!(app.poll_exit().unwrap().success());
        let requests = server.requests();
        assert!(requests.iter().any(|target| target.starts_with("frames ")));
        assert!(requests.iter().any(|target| target == "quit"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// (A script, not this test's own binary run again: under the JIT that binary is a JIT start of
    /// the whole test binary, which passed the native launch's deadline before it exited in a full
    /// run beside other packages, 11-22.)
    #[cfg(unix)]
    #[test]
    fn early_exit_before_the_listening_line_is_an_error_with_stderr() {
        let dir = scratch_dir("early-exit");
        let spec = fake_app_spec(dir.clone());
        let script = write_script(&dir, "early-exit.sh", "#!/bin/sh\necho boom >&2\nexit 3\n");
        let err = launch_error(launch_command(&spec, &script, Command::new(&script), false));
        assert!(err.message().contains("exited"), "{}", err.message());
        assert!(err.message().contains("boom"), "{}", err.message());
        let _ = fs::remove_dir_all(&dir);
    }

    /// A JIT launch has no deadline, so the child's exit is what ends its wait. An app whose remote
    /// came up and that then fails before its first window (an unattended run's compile error, which
    /// does not park) fails the launch at once, with its exit status and stderr tail. A test polled
    /// its parked app's windows every 50 ms for 64 min (11-07).
    #[cfg(unix)]
    #[test]
    fn a_jit_app_that_exits_before_its_window_fails_the_launch_at_once() {
        let dir = scratch_dir("jit-exit");
        let server = crate::fixture::FixtureServer::start();
        server.route("help", "makepad-remote app=fake-app");
        server.route("status", r#"{"app":"fake-app","pid":1,"w":[]}"#);
        // It fails once the launch has asked for its windows, as a JIT app's first frame does.
        let fails = format!("{}\necho 'compile error in app::start: boom' >&2\nexit 101", when_asked(&dir, &server, "status"));
        let script = write_fake_app(&dir, server.socket(), &fails);
        let spec = fake_app_spec(dir.clone());
        let t0 = Instant::now();
        let err = launch_error(launch_command(&spec, &script, Command::new(&script), true));
        assert!(err.message().contains("exited") && err.message().contains("during startup"), "{}", err.message());
        assert!(err.message().contains("compile error in app::start: boom"), "{}", err.message());
        assert!(server.requests().iter().any(|target| target == "status"), "it waited for a window");
        assert!(t0.elapsed() < Duration::from_secs(5), "at once: {:?}", t0.elapsed());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_listening_line_for_another_pid_is_rejected() {
        let dir = scratch_dir("wrong-pid");
        let server = fixture_with_window();
        let script = write_script(
            &dir,
            "fake-app.sh",
            &format!("#!/bin/sh\necho \"[makepad-remote] pid=1 app=x socket={}\" >&2\nexec sleep 60\n", server.socket().display()),
        );
        let spec = fake_app_spec(dir.clone());
        let err = launch_error(launch(&spec, &script));
        assert!(err.message().contains("pid 1"), "{}", err.message());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_jit_at_startup_is_named_as_one() {
        let tail = "stderr tail:\nrelax live: The JIT itself changed (makepad_hal) -- Restart to update: a restart applies it\n";
        let stale = startup_exit(Path::new("app"), &"exit status: 0", "during startup", tail.into());
        assert!(stale.message().contains("stopped during startup: a stale JIT, not a test failure"), "{}", stale.message());
        assert!(stale.message().contains("tree in makepad_hal, a crate linked into the JIT"), "{}", stale.message());
        let exited = startup_exit(Path::new("app"), &"exit status: 1", "during startup", "stderr tail:\nboom".into());
        assert_eq!(exited.message(), "app exited (exit status: 1) during startup; stderr tail:\nboom");
    }

    #[test]
    fn file_tail_keeps_the_end() {
        let dir = scratch_dir("tail");
        let path = dir.join("long.txt");
        fs::write(&path, "x".repeat(TAIL_BYTES + 10) + "END").unwrap();
        let tail = file_tail(&path);
        assert_eq!(tail.len(), TAIL_BYTES);
        assert!(tail.ends_with("END"));
        let _ = fs::remove_dir_all(&dir);
    }
}

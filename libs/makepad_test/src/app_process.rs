//! The owned app process: release build, `--remote` launch, and shutdown.
//!
//! Everything here blocks on the test runner's thread only. The child's
//! stdout/stderr go to files in the artifact directory, so no reader threads
//! are needed and the transcripts survive as failure artifacts.

use crate::error::{TestError, TestResult};
use crate::remote::RemoteClient;
use makepad_micro_serde::JsonValue;
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const LISTENING_PREFIX: &str = "[makepad-remote] listening on ";
const BIND_FAILED_PREFIX: &str = "[makepad-remote] bind ";
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
    pub poll_interval: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListeningLine {
    pub host: String,
    pub port: u16,
    pub pid: u32,
    pub app: String,
    pub grab_dir: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownOutcome {
    /// The owned process exited after a graceful shutdown request, even if
    /// the response was lost as the remote service stopped.
    Graceful,
    /// The process had already exited before shutdown was asked for.
    AlreadyExited,
    /// The process outlived both graceful requests; the owned pid was killed.
    Killed,
}

pub struct OwnedApp {
    child: Child,
    pub pid: u32,
    pub executable: PathBuf,
    pub client: RemoteClient,
    /// Where the app writes its own grabs (`/g`), from the listening line.
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

    /// Ask the app to quit and confirm this exact process exits. Only the owned
    /// pid is ever killed, and only when the graceful path fails.
    pub fn shutdown(&mut self, poll: Duration) -> ShutdownOutcome {
        if self.poll_exit().is_some() {
            return ShutdownOutcome::AlreadyExited;
        }
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

/// Spawn `executable --remote` and wait for its listening line.
pub fn launch(spec: &LaunchSpec, executable: &Path) -> TestResult<OwnedApp> {
    fs::create_dir_all(&spec.artifacts_dir)?;
    let stdout_path = spec.artifacts_dir.join("app-stdout.txt");
    let stderr_path = spec.artifacts_dir.join("app-stderr.txt");
    let stdout_file = File::create(&stdout_path)?;
    let stderr_file = File::create(&stderr_path)?;

    let mut command = Command::new(executable);
    command
        .arg("--remote")
        .args(&spec.args)
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
    let mut child = command.spawn().map_err(|err| {
        TestError::new(format!(
            "failed to launch {}: {err}",
            executable.display()
        ))
    })?;
    let pid = child.id();

    let deadline = Instant::now() + spec.startup_timeout;
    let listening = loop {
        let stdout = fs::read_to_string(&stdout_path).unwrap_or_default();
        if let Some(line) = stdout.lines().find(|line| line.starts_with(LISTENING_PREFIX)) {
            match parse_listening_line(line) {
                Ok(listening) => break listening,
                Err(err) => {
                    // No valid owned endpoint is available for graceful quit.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(err);
                }
            }
        }
        if let Some(line) = stdout.lines().find(|line| line.starts_with(BIND_FAILED_PREFIX)) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(TestError::new(format!("app remote failed to start: {line}")));
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(TestError::new(format!(
                "{} exited ({status}) before its remote came up; stdout tail:\n{}\nstderr tail:\n{}",
                executable.display(),
                file_tail(&stdout_path),
                file_tail(&stderr_path)
            )));
        }
        if Instant::now() >= deadline {
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
    if listening.pid != pid {
        let _ = child.kill();
        let _ = child.wait();
        return Err(TestError::new(format!(
            "remote listening line names pid {} but the owned process is {pid}",
            listening.pid
        )));
    }

    let client = RemoteClient::new(listening.host.clone(), listening.port);
    let mut app = OwnedApp {
        child,
        pid,
        executable: executable.to_path_buf(),
        client,
        grab_dir: listening.grab_dir,
        stderr_path,
        exit_status: None,
    };

    // The service is up once `GET /` answers; a window follows on the first
    // frame. Waiting here keeps `/snap` from answering an empty tree to the
    // test's first locator.
    let mut help_seen = false;
    loop {
        if let Some(status) = app.poll_exit() {
            return Err(TestError::new(format!(
                "{} exited ({status}) during startup; stderr tail:\n{}",
                app.executable.display(),
                app.stderr_tail()
            )));
        }
        if !help_seen {
            help_seen = app.client.help().is_ok();
        }
        if help_seen && app.client.windows().is_ok_and(|windows| !windows.is_empty()) {
            return Ok(app);
        }
        if Instant::now() >= deadline {
            let _ = app.shutdown(spec.poll_interval);
            return Err(TestError::new(format!(
                "timed out waiting for {} to open a window on {}",
                app.executable.display(),
                app.client.endpoint()
            )));
        }
        thread::sleep(spec.poll_interval);
    }
}

/// `[makepad-remote] listening on 127.0.0.1:53412 pid=9931 app=NAME grabs=DIR`
pub fn parse_listening_line(line: &str) -> TestResult<ListeningLine> {
    let rest = line
        .strip_prefix(LISTENING_PREFIX)
        .ok_or_else(|| TestError::new(format!("not a remote listening line: {line}")))?;
    let mut parts = rest.splitn(2, ' ');
    let endpoint = parts.next().unwrap_or_default();
    let (host, port) = endpoint
        .rsplit_once(':')
        .and_then(|(host, port)| port.parse::<u16>().ok().map(|port| (host, port)))
        .ok_or_else(|| TestError::new(format!("bad endpoint in listening line: {line}")))?;
    let tail = parts.next().unwrap_or_default();
    let grab_dir = tail
        .split_once("grabs=")
        .map(|(_, dir)| dir.trim().to_string())
        .unwrap_or_default();
    let mut pid = None;
    let mut app = String::new();
    for token in tail.split(' ') {
        if let Some(value) = token.strip_prefix("pid=") {
            pid = value.parse::<u32>().ok();
        } else if let Some(value) = token.strip_prefix("app=") {
            app = value.to_string();
        }
    }
    let pid = pid.ok_or_else(|| TestError::new(format!("no pid in listening line: {line}")))?;
    Ok(ListeningLine {
        host: host.to_string(),
        port,
        pid,
        app,
        grab_dir: PathBuf::from(grab_dir),
    })
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

    #[test]
    fn listening_line_parses_every_field() {
        let line = "[makepad-remote] listening on 127.0.0.1:53412 pid=9931 app=makepad-example-counter grabs=/tmp/makepad-remote/makepad-example-counter-9931";
        let parsed = parse_listening_line(line).unwrap();
        assert_eq!(
            parsed,
            ListeningLine {
                host: "127.0.0.1".to_string(),
                port: 53412,
                pid: 9931,
                app: "makepad-example-counter".to_string(),
                grab_dir: PathBuf::from("/tmp/makepad-remote/makepad-example-counter-9931"),
            }
        );
        assert!(parse_listening_line("[makepad-remote] bind 127.0.0.1:0 failed").is_err());
        assert!(parse_listening_line("[makepad-remote] listening on nope pid=1").is_err());
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
            poll_interval: Duration::from_millis(20),
        }
    }

    /// A shell script standing in for the app binary: it prints the remote
    /// listening line (pointing at a fixture server) and then lives until
    /// killed, ignoring shutdown requests like a wedged app would.
    fn write_fake_app(dir: &Path, port: u16, script_tail: &str) -> PathBuf {
        let path = dir.join("fake-app.sh");
        fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"[makepad-remote] listening on 127.0.0.1:{port} pid=$$ app=fake-app grabs={}\"\n{script_tail}\n",
                dir.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    fn launch_error(spec: &LaunchSpec, script: &Path) -> TestError {
        match launch(spec, script) {
            Ok(mut app) => {
                let _ = app.shutdown(Duration::from_millis(20));
                panic!("launch unexpectedly succeeded");
            }
            Err(err) => err,
        }
    }

    fn fixture_with_window() -> crate::fixture::FixtureServer {
        let server = crate::fixture::FixtureServer::start();
        server.route("/", 200, "makepad-remote app=fake-app");
        server.route(
            "/s",
            200,
            r#"{"app":"fake-app","pid":1,"w":[{"i":0,"t":"fake","sz":[10,10],"px":[10,10],"dpi":1,"pos":[0,0]}]}"#,
        );
        server.route("/quit", 200, r#"{"ok":1}"#);
        server
    }

    #[test]
    fn launch_reads_the_listening_line_and_kill_fallback_ends_the_owned_pid() {
        let dir = scratch_dir("launch");
        let server = fixture_with_window();
        let script = write_fake_app(&dir, server.port(), "exec sleep 60");
        let spec = fake_app_spec(dir.clone());

        let mut app = launch(&spec, &script).unwrap();
        assert_eq!(app.client.endpoint(), format!("127.0.0.1:{}", server.port()));
        assert_eq!(app.grab_dir, dir);
        assert!(app.poll_exit().is_none());
        assert!(fs::read_to_string(spec.artifacts_dir.join("app-stdout.txt"))
            .unwrap()
            .contains("listening on"));
        assert!(server.requests().iter().any(|target| target == "/"));
        assert!(server.requests().iter().any(|target| target == "/s"));

        // `/gq` is unavailable and `/quit` is acknowledged, but the process stays up, so
        // shutdown must fall back to killing exactly this pid.
        let outcome = app.shutdown(Duration::from_millis(20));
        assert_eq!(outcome, ShutdownOutcome::Killed);
        assert!(app.poll_exit().is_some());
        assert!(server.requests().iter().any(|target| target == "/quit"));
        let requests = server.requests();
        assert!(requests.iter().position(|target| target == "/gq").unwrap()
            < requests.iter().position(|target| target == "/quit").unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn graceful_quit_is_reported_when_the_process_exits_on_its_own() {
        let dir = scratch_dir("graceful");
        let server = fixture_with_window();
        server.route("/gq", 200, r#"{"quit":1}"#);
        // Lives just long enough for startup to complete, then exits like an
        // app answering `/quit` would.
        let script = write_fake_app(&dir, server.port(), "exec sleep 0.5");
        let spec = fake_app_spec(dir.clone());

        let mut app = launch(&spec, &script).unwrap();
        assert_eq!(app.shutdown(Duration::from_millis(20)), ShutdownOutcome::Graceful);
        assert_eq!(app.shutdown(Duration::from_millis(20)), ShutdownOutcome::AlreadyExited);
        assert!(server.requests().iter().any(|target| target == "/gq"));
        assert!(!server.requests().iter().any(|target| target == "/quit"));
        assert!(app.poll_exit().unwrap().success());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lost_shutdown_responses_still_allow_the_owned_child_to_exit() {
        let dir = scratch_dir("lost-shutdown-responses");
        let server = fixture_with_window();
        server.disconnect("/gq");
        server.disconnect("/quit");
        let script = write_fake_app(&dir, server.port(), "exec sleep 1");
        let spec = fake_app_spec(dir.clone());
        let mut app = launch(&spec, &script).unwrap();

        assert_eq!(app.shutdown(Duration::from_millis(20)), ShutdownOutcome::Graceful);
        assert!(app.poll_exit().unwrap().success());
        assert!(server.requests().iter().any(|target| target == "/gq"));
        assert!(server.requests().iter().any(|target| target == "/quit"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn early_exit_before_the_listening_line_is_an_error_with_stderr() {
        let dir = scratch_dir("early-exit");
        let script = dir.join("fake-app.sh");
        fs::write(&script, "#!/bin/sh\necho boom >&2\nexit 3\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let spec = fake_app_spec(dir.clone());
        let err = launch_error(&spec, &script);
        assert!(err.message().contains("exited"), "{}", err.message());
        assert!(err.message().contains("boom"), "{}", err.message());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_listening_line_for_another_pid_is_rejected() {
        let dir = scratch_dir("wrong-pid");
        let server = fixture_with_window();
        let script = dir.join("fake-app.sh");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"[makepad-remote] listening on 127.0.0.1:{} pid=1 app=x grabs=/tmp\"\nexec sleep 60\n",
                server.port()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let spec = fake_app_spec(dir.clone());
        let err = launch_error(&spec, &script);
        assert!(err.message().contains("pid 1"), "{}", err.message());
        let _ = fs::remove_dir_all(&dir);
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

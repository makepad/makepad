use crate::app_process::{self, LaunchSpec, OwnedApp, ShutdownOutcome};
use crate::error::{IntoTestResult, TestError, TestResult};
use crate::remote::{KeyKind, MouseInput, MouseKind};
use crate::selector::Selector;
use crate::remote::{NodeMatch, RawFrame, UiSnapshot};
use makepad_studio_protocol::{KeyCode, KeyModifiers, StudioToApp};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt::Write;
use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(600);
const ACTION_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// How often a wait looks at what the app has spent ([`Budget`]).
const LOOK: Duration = Duration::from_secs(1);
/// A process using under this share of one core (1/n) for its own work (not serving remote
/// peers), with nothing pending, is idle.
pub(crate) const IDLE_SHARE: u32 = 20;
/// An app whose main thread runs nothing and whose other threads stay under this share of one core
/// (1/n), with nothing pending, only ticks (a simulation stepping, a poll): idle too. A thread at
/// work runs more.
const TICK_SHARE: u32 = 2;
/// Major page faults a second at which a look that ran nothing waited on memory (a swapping box):
/// a few faults are a mapped file's first read, not swapping.
const SWAP_FAULTS_PER_S: f64 = 50.0;
const DRAG_STEPS: usize = 6;
/// How long a held drag rests at its end before letting go, which keeps it from flinging.
const DRAG_HOLD: Duration = Duration::from_millis(300);
const RECENT_LOG_LINES: usize = 200;

static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

fn artifacts_root(manifest_dir: &Path) -> TestResult<PathBuf> {
    // devhub gives each lease its own artifacts and app homes.
    if let Some(root) = std::env::var_os("MAKEPAD_TEST_ARTIFACTS").filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(root));
    }
    if let Some(target) = std::env::var_os("CARGO_TARGET_DIR").filter(|s| !s.is_empty()) {
        return Ok(std::path::absolute(target)?.join("makepad_test"));
    }
    let manifest_dir = std::path::absolute(manifest_dir)?;
    for dir in manifest_dir.ancestors() {
        if fs::read_to_string(dir.join("Cargo.toml"))
            .is_ok_and(|toml| toml.lines().any(|line| line.trim() == "[workspace]"))
        {
            return Ok(dir.join("target/makepad_test"));
        }
    }

    // FNV-1a keeps the full path, including non-UTF-8 bytes, in a short, stable tree ID.
    let mut tree = 0xcbf29ce484222325_u64;
    for &byte in manifest_dir.as_os_str().as_encoded_bytes() {
        tree = (tree ^ u64::from(byte)).wrapping_mul(0x100000001b3);
    }
    Ok(std::env::temp_dir().join("makepad_test").join(format!("{tree:016x}")))
}

#[derive(Clone, Debug)]
pub struct TestConfig {
    pub package_name: String,
    /// The bin target to launch when the package builds more than one.
    pub bin_name: Option<String>,
    pub manifest_dir: PathBuf,
    pub test_name: String,
    pub artifacts_dir: PathBuf,
    /// Extra environment for the app process. `CARGO_TARGET_DIR`, when set,
    /// also applies to the build. `MAKEPAD_TEST_DPI=N` scales screenshots
    /// to N pixels per layout point. `MAKEPAD_TEST_HOME` is the app's
    /// stand-in for the user's home folder: an empty `home/` in the test's
    /// artifacts unless the test names one. An app keeps its per-user state
    /// (preferences, documents, linked devices) below it, so a test starts
    /// from the defaults and never reads or changes the person's.
    pub env: HashMap<String, String>,
    /// Extra arguments appended after `--remote`.
    pub app_args: Vec<String>,
    /// Show the window (unfocused). Default is hidden (`MAKEPAD_HIDE_WINDOWS=1`).
    pub visible: bool,
    /// Native startup after the build. JIT startup includes compilation and is uncapped.
    pub startup_timeout: Duration,
    /// What a wait may spend ([`Budget`]): this much of the app's main thread's CPU, or this
    /// long with the app idle.
    pub action_timeout: Duration,
    pub poll_interval: Duration,
    pub startup_pause: Duration,
    pub action_delay: Duration,
    pub keep_open: Duration,
    /// Run beside other tests' apps (default: `MAKEPAD_TEST_PARALLEL`). App sessions in one test
    /// executable otherwise take turns: several at once load the box, and a suite whose waits
    /// count wall time grows flaky under load. A suite whose waits count the app's own time (a
    /// sim's clock, its frames) can run its apps side by side.
    pub concurrent: bool,
    /// Capture a final frame with `gq` on a pass. Defaults to
    /// `MAKEPAD_TEST_KEEP_ARTIFACTS`, otherwise false. Failures, panics,
    /// lookup clashes, startup failures and raw OwnedApp drops still grab.
    pub keep_artifacts_on_pass: bool,
}

impl TestConfig {
    pub fn new(
        manifest_dir: impl Into<PathBuf>,
        package_name: impl Into<String>,
        test_name: impl Into<String>,
    ) -> TestResult<Self> {
        let manifest_dir = manifest_dir.into();
        let package_name = package_name.into();
        let test_name = test_name.into();
        let artifacts_dir = artifacts_root(&manifest_dir)?
            .join(sanitize_path_component(&package_name))
            .join(sanitize_path_component(&test_name));

        let mut env = HashMap::new();
        env.insert("RUST_BACKTRACE".to_string(), "1".to_string());

        Ok(Self {
            package_name,
            bin_name: None,
            manifest_dir,
            test_name,
            artifacts_dir,
            env,
            app_args: Vec::new(),
            visible: visible_mode_enabled(),
            startup_timeout: STARTUP_TIMEOUT,
            action_timeout: ACTION_TIMEOUT,
            poll_interval: POLL_INTERVAL,
            startup_pause: env_duration_ms("MAKEPAD_TEST_STARTUP_DELAY_MS"),
            action_delay: env_duration_ms("MAKEPAD_TEST_ACTION_DELAY_MS"),
            keep_open: env_duration_ms("MAKEPAD_TEST_KEEP_OPEN_MS"),
            concurrent: parallel_tests_enabled(),
            keep_artifacts_on_pass: keep_artifacts_on_pass_enabled(),
        })
    }

    pub fn current_package(
        manifest_dir: impl Into<PathBuf>,
        package_name: impl Into<String>,
        test_name: impl Into<String>,
    ) -> TestResult<Self> {
        Self::new(manifest_dir, package_name, test_name)
    }

    fn launch_spec(&self) -> LaunchSpec {
        LaunchSpec {
            manifest_dir: self.manifest_dir.clone(),
            artifacts_dir: self.artifacts_dir.clone(),
            env: self.env.clone(),
            args: self.app_args.clone(),
            visible: self.visible,
            startup_timeout: self.startup_timeout,
            startup_idle: self.action_timeout,
            poll_interval: self.poll_interval,
        }
    }

    /// Screenshot scale that honours `MAKEPAD_TEST_DPI` from `env`: the
    /// screenshot is rescaled by the native remote capture path; a suite that
    /// asks for `1` expects layout points to equal PNG pixels.
    fn requested_screenshot_dpi(&self) -> Option<f64> {
        self.env
            .get("MAKEPAD_TEST_DPI")
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|dpi| *dpi > 0.0)
    }
}

struct TestAppInner {
    config: TestConfig,
    app: OwnedApp,
    /// Window that received the last pointer interaction; app-level key and
    /// text input follows it, as keyboard input follows a click on a desktop.
    focus_window: Option<usize>,
    /// For each cursor [`TestApp::log_since`] was asked with, the newest line it answered with: a
    /// wait that reads from one cursor on every poll has seen what the log drops afterwards.
    log_reads: HashMap<u64, u64>,
}

#[derive(Clone)]
pub struct TestApp {
    inner: Rc<RefCell<TestAppInner>>,
}

impl TestApp {
    fn start(mut config: TestConfig) -> TestResult<Self> {
        // Clear the previous test's artifacts here so app launches in this test share one clash report.
        if config.artifacts_dir.exists() {
            fs::remove_dir_all(&config.artifacts_dir)?;
        }
        fs::create_dir_all(&config.artifacts_dir)?;
        if !config.env.contains_key("MAKEPAD_TEST_HOME") {
            let home = config.artifacts_dir.join("home");
            fs::create_dir_all(&home)?;
            config.env.insert("MAKEPAD_TEST_HOME".to_string(), home.display().to_string());
        }

        let app = app_process::launch_package(
            &config.launch_spec(),
            &config.package_name,
            config.bin_name.as_deref(),
        )?;

        if config.startup_pause > Duration::ZERO {
            thread::sleep(config.startup_pause);
        }

        Ok(Self {
            inner: Rc::new(RefCell::new(TestAppInner {
                config,
                app,
                focus_window: None,
                log_reads: HashMap::new(),
            })),
        })
    }

    pub fn locator(&self, selector: Selector) -> Locator {
        Locator {
            app: self.clone(),
            selector,
        }
    }

    /// The app's process id, the one its endpoint names (under the JIT on macOS and Windows the
    /// run behind the process launched), for diagnostics and remote calls by pid.
    pub fn pid(&self) -> u32 {
        self.inner.borrow().app.pid
    }

    /// The Unix socket path of the owned app's remote endpoint, for diagnostics.
    pub fn remote_endpoint(&self) -> String {
        self.inner.borrow().app.client.socket()
    }

    /// The directory `grab` and `gq` write PNG files into (client side).
    pub fn grab_dir(&self) -> PathBuf {
        self.inner.borrow().app.grab_dir.clone()
    }

    pub fn type_text(&self, text: impl AsRef<str>) {
        if let Err(err) = self.try_type_text(text) {
            panic_for_error(err);
        }
    }

    pub fn try_type_text(&self, text: impl AsRef<str>) -> TestResult<()> {
        self.ensure_running()?;
        let window = self.focus_window();
        self.client().text(window, text.as_ref(), true)?;
        self.pace_after_action();
        Ok(())
    }

    pub fn press_return(&self) {
        if let Err(err) = self.try_press_return() {
            panic_for_error(err);
        }
    }

    pub fn try_press_return(&self) -> TestResult<()> {
        self.try_press_key(KeyCode::ReturnKey)
    }

    pub fn press_key(&self, key_code: KeyCode) {
        if let Err(err) = self.try_press_key(key_code) {
            panic_for_error(err);
        }
    }

    pub fn try_press_key(&self, key_code: KeyCode) -> TestResult<()> {
        self.try_press_key_with_modifiers(key_code, KeyModifiers::default())
    }

    pub fn press_key_with_modifiers(&self, key_code: KeyCode, modifiers: KeyModifiers) {
        if let Err(err) = self.try_press_key_with_modifiers(key_code, modifiers) {
            panic_for_error(err);
        }
    }

    pub fn try_press_key_with_modifiers(
        &self,
        key_code: KeyCode,
        modifiers: KeyModifiers,
    ) -> TestResult<()> {
        self.ensure_running()?;
        let window = self.focus_window();
        self.client()
            .key(window, KeyKind::Press, key_code, modifiers, true)?;
        self.pace_after_action();
        Ok(())
    }

    /// Grab the primary window to a PNG in the app's own grab directory.
    pub fn screenshot(&self) -> PathBuf {
        match self.try_screenshot() {
            Ok(path) => path,
            Err(err) => panic_for_error(err),
        }
    }

    pub fn try_screenshot(&self) -> TestResult<PathBuf> {
        let (window, scale) = self.capture_target()?;
        self.client().grab(Some(window), scale)
    }

    /// Read the primary window directly in RGBA8 at the requested screenshot DPI.
    pub fn grab_rgba(&self) -> RawFrame {
        match self.try_grab_rgba() {
            Ok(frame) => frame,
            Err(err) => panic_for_error(err),
        }
    }

    pub fn try_grab_rgba(&self) -> TestResult<RawFrame> {
        let (window, scale) = self.capture_target()?;
        self.client().grab_rgba(window, scale).map_err(TestError::new)
    }

    fn capture_target(&self) -> TestResult<(usize, f64)> {
        self.ensure_running()?;
        let windows = self.client().windows()?;
        let window = windows
            .first()
            .ok_or_else(|| TestError::new("screenshot: the app has no window"))?;
        let scale = match self.inner.borrow().config.requested_screenshot_dpi() {
            Some(dpi) if window.dpi > 0.0 => dpi / window.dpi,
            _ => 1.0,
        };
        Ok((window.id, scale))
    }

    /// The app's GPU device is lost, as a driver reset or a GPU hang recovery loses it (`hal
    /// lose`): the app hears `GpuLost`, then `GpuReset` on the new device the HAL makes. Fails on
    /// a backend that cannot replace a lost device.
    pub fn lose_gpu_device(&self) {
        if let Err(err) = self.try_lose_gpu_device() {
            panic_for_error(err);
        }
    }

    pub fn try_lose_gpu_device(&self) -> TestResult<()> {
        self.ensure_running()?;
        self.client().get_text("hal lose", &[]).map(|_| ())
    }

    /// Runs `frames` frames of the app's virtual clock (`--virtual-clock` in `app_args`), at
    /// 60 fps; with `wait_loads` each frame opens only once the app's asynchronous loads have
    /// landed. Answers the `step` command's line, `{"frame":..,"time":..}` after the last frame.
    pub fn try_step(&self, frames: u64, wait_loads: bool) -> TestResult<String> {
        self.ensure_running()?;
        let frames = frames.to_string();
        let wait_loads = if wait_loads { "1" } else { "0" };
        self.client().get_text("step", &[("frames", frames.as_str()), ("wait_loads", wait_loads)])
    }

    pub fn widget_dump(&self) -> String {
        match self.try_widget_dump() {
            Ok(dump) => dump,
            Err(err) => panic_for_error(err),
        }
    }

    pub fn try_widget_dump(&self) -> TestResult<String> {
        self.ensure_running()?;
        self.client().dump()
    }

    pub fn widget_snapshot(&self) -> UiSnapshot {
        match self.try_widget_snapshot() {
            Ok(widgets) => widgets,
            Err(err) => panic_for_error(err),
        }
    }

    pub fn try_widget_snapshot(&self) -> TestResult<UiSnapshot> {
        self.ensure_running()?;
        self.client().snapshot()
    }

    /// Wait for three successful event-loop snapshots, 100 ms apart.
    pub fn wait_until_ready(&self, timeout: Duration) -> Duration {
        let started = Instant::now();
        let mut consecutive = 0;
        while started.elapsed() < timeout {
            if let Err(err) = self.ensure_running() {
                panic_for_error(err);
            }
            if self.client().snapshot().is_ok() {
                consecutive += 1;
                if consecutive == 3 {
                    break;
                }
            } else {
                consecutive = 0;
            }
            let delay = if consecutive == 0 {
                Duration::from_millis(250)
            } else {
                Duration::from_millis(100)
            };
            thread::sleep(delay.min(timeout.saturating_sub(started.elapsed())));
        }

        let elapsed = started.elapsed();
        if elapsed >= timeout || elapsed > Duration::from_secs(10) {
            let recent_log = self
                .log_since(0)
                .map(|tail| tail.lines[tail.lines.len().saturating_sub(40)..].join("\n"))
                .unwrap_or_else(|err| format!("could not read app log: {err}"));
            let secs = elapsed.as_secs_f64();
            if elapsed >= timeout {
                panic!(
                    "timed out after {secs:.1} s waiting for app readiness; recent app log:\n{recent_log}"
                );
            }
            println!(
                "[makepad_test] app needed {secs:.1} s before it answered; recent app log:\n{recent_log}"
            );
        }
        elapsed
    }

    /// The app's log lines after line `since`, without forcing a frame. The app's log keeps only
    /// its newest lines: when some after `since` are gone before a read from `since` saw them, a
    /// line the caller looks for may be among them and the answer is unknown, so that is an error.
    /// (A wait that reads from the same cursor on every poll saw the lines an earlier poll was
    /// given.) `since` 0 asks for what the log keeps (a tail, the newest `last_seq`): never an
    /// error, [`crate::LogTail::dropped`] counts the older lines. [`Self::log_since_tolerant`]
    /// reads past a gap after a cursor.
    pub fn log_since(&self, since: u64) -> TestResult<crate::LogTail> {
        let tail = self.log_since_tolerant(since)?;
        if since == 0 {
            return Ok(tail);
        }
        let seen = self.inner.borrow().log_reads.get(&since).copied();
        if let Some((after, unseen)) = unseen_gap(since, tail.dropped, seen) {
            return Err(self.log_gap(after, unseen, "this read"));
        }
        self.inner.borrow_mut().log_reads.insert(since, tail.last_seq);
        Ok(tail)
    }

    /// As [`Self::log_since`], but a gap is no error: [`crate::LogTail::dropped`] counts the
    /// lines it lost. For a caller that wants what is kept (a recent tail for a message), which
    /// says why where it calls this.
    pub fn log_since_tolerant(&self, since: u64) -> TestResult<crate::LogTail> {
        self.ensure_running()?;
        self.client().log_since(since)
    }

    /// The error for `dropped` lines after line `since` that the app's log no longer keeps.
    fn log_gap(&self, since: u64, dropped: u64, before: &str) -> TestError {
        let stderr = self.inner.borrow().app.stderr_path.display().to_string();
        TestError::new(format!(
            "the log dropped {dropped} lines after line {since} before {before}: the app's log keeps only its \
             newest lines, and a line looked for may be among them. They are not lost: the app's full log is in {stderr}"
        ))
    }

    pub fn wait_for_log_contains(&self, needle: &str) {
        if let Err(err) = self.try_wait_for_log_contains(needle) {
            panic_for_error(err);
        }
    }

    /// Waits until a line of the app's log, logged before the wait or during it, contains
    /// `needle`. Lines the log no longer kept when the wait began are named when it times out;
    /// lines it drops during the wait end the wait at once, since the needle may be among them.
    pub fn try_wait_for_log_contains(&self, needle: &str) -> TestResult<()> {
        let mut budget = Budget::start(self, self.action_timeout())?;
        // What the log keeps from before the wait; a gap there is no error yet (the line may still come).
        let first = self.log_since(0)?;
        if first.lines.iter().any(|line| line.contains(needle)) {
            return Ok(());
        }
        let mut cursor = first.last_seq;
        loop {
            let tail = self.log_since(cursor).map_err(|err| TestError::new(format!("waiting for log containing `{needle}`: {err}")))?;
            if tail.lines.iter().any(|line| line.contains(needle)) {
                return Ok(());
            }
            cursor = tail.last_seq;
            if budget.look_due() {
                if let Some(spent) = budget.spent(self)? {
                    let gap = if first.dropped > 0 {
                        format!("; {}", self.log_gap(0, first.dropped, "this wait"))
                    } else {
                        String::new()
                    };
                    return Err(TestError::new(format!(
                        "timed out waiting for log containing `{needle}`: {spent}{gap}"
                    )));
                }
            }
            thread::sleep(self.poll_interval());
        }
    }

    /// Waits until `pending` answers `None`. While it answers `Some(what)`, what is still missing
    /// in words, the wait goes on (asked every poll interval), and once the app has spent the
    /// wait's [`Budget`] without it, the wait fails with the last `what`. For what a locator
    /// cannot say (several nodes together, the app's log, files the app writes), waited for as a
    /// locator waits.
    pub fn wait_until(&self, pending: impl FnMut() -> Option<String>) {
        if let Err(err) = self.try_wait_until(pending) {
            panic_for_error(err);
        }
    }

    pub fn try_wait_until(&self, pending: impl FnMut() -> Option<String>) -> TestResult<()> {
        self.try_wait_until_within(self.action_timeout(), pending)
    }

    /// [`Self::wait_until`] for what takes the app long (a slice, a restart): the wait may spend
    /// `budget` of the app's main thread's CPU instead of `action_timeout`. The idle rule stays
    /// `action_timeout`: an app that sits idle with nothing pending fails the wait as soon.
    pub fn wait_until_within(&self, budget: Duration, pending: impl FnMut() -> Option<String>) {
        if let Err(err) = self.try_wait_until_within(budget, pending) {
            panic_for_error(err);
        }
    }

    /// One AI tool call of the app (`call <service>.<tool> <json args>`, as
    /// `makepad-remote app <pid> call` sends it): its answer lines; an `{"err":..}` line is an
    /// error. The answer is waited for as [`Self::wait_until_within`] waits with `budget`, so a
    /// call the app takes long over (a render while its scene builds, on a loaded box) is
    /// judged by the app's progress, never by a clock. A failed wait drops the call, which
    /// cancels it.
    pub fn call_tool(&self, budget: Duration, tool: &str, args: &str) -> Vec<String> {
        match self.try_call_tool(budget, tool, args) {
            Ok(lines) => lines,
            Err(err) => panic_for_error(err),
        }
    }

    pub fn try_call_tool(&self, budget: Duration, tool: &str, args: &str) -> TestResult<Vec<String>> {
        let call = self.client().start_tool_call(tool, args)?;
        self.wait_for_answer(budget, call)
    }

    /// Sends words and parameters on the app's cmd channel and waits for the answer lines
    /// with the main-thread CPU budget and idle rule of [`Self::wait_until_within`].
    pub fn command(&self, budget: Duration, words: &[&str], params: &[(&str, &str)]) -> Vec<String> {
        self.try_command(budget, words, params).unwrap_or_else(|err| panic_for_error(err))
    }

    pub fn try_command(&self, budget: Duration, words: &[&str], params: &[(&str, &str)]) -> TestResult<Vec<String>> {
        let call = self.client().start_command(&words.join(" "), params)?;
        self.wait_for_answer(budget, call)
    }

    fn wait_for_answer(&self, budget: Duration, mut call: crate::remote::PendingAnswer) -> TestResult<Vec<String>> {
        let mut answer = None;
        self.try_wait_until_within(budget, || match call.poll() {
            None => Some(format!("waiting for the answer of {}", call.command)),
            done => {
                answer = done;
                None
            }
        })?;
        answer.unwrap_or_else(|| Err(TestError::new(format!("{}: the wait ended without its answer", call.command))))
    }

    pub fn try_wait_until_within(&self, budget: Duration, mut pending: impl FnMut() -> Option<String>) -> TestResult<()> {
        let mut budget = Budget::start(self, budget)?;
        loop {
            self.ensure_running()?;
            let (since, before) = (Instant::now(), self.client().cpu(false)?);
            let Some(what) = pending() else { return Ok(()) };
            drove();
            budget.asked(since, before, self.client().cpu(false)?);
            if budget.look_due() {
                if let Some(spent) = budget.spent(self)? {
                    return Err(TestError::new(format!("{what}: {spent}")));
                }
            }
            thread::sleep(self.poll_interval());
        }
    }

    /// Raw escape hatch: replay protocol events through the remote input
    /// routes. Only pointer, scroll, key and text events have a standalone
    /// equivalent; anything else is an explicit error.
    pub fn forward(&self, msgs: Vec<StudioToApp>) {
        if let Err(err) = self.try_forward(msgs) {
            panic_for_error(err);
        }
    }

    pub fn try_forward(&self, msgs: Vec<StudioToApp>) -> TestResult<()> {
        self.ensure_running()?;
        let window = self.focus_window();
        let count = msgs.len();
        for (index, msg) in msgs.into_iter().enumerate() {
            let wait = index + 1 == count;
            match msg {
                StudioToApp::MouseDown(event) => self.client().mouse(&MouseInput {
                    button: button_index(event.button_raw_bits),
                    modifiers: event.modifiers.into_key_modifiers(),
                    wait,
                    ..MouseInput::at(MouseKind::Down, window, event.x, event.y)
                })?,
                // The test client speaks no cancellation: a cancel replayed as an
                // up would click, so it is refused rather than faked.
                StudioToApp::MouseCancel(_) => {
                    return Err(TestError::new(
                        "MouseCancel is not supported by the test client (it has no cancel input; replaying it as an up would click)",
                    ))
                }
                StudioToApp::MouseUp(event) => self.client().mouse(&MouseInput {
                    button: button_index(event.button_raw_bits),
                    modifiers: event.modifiers.into_key_modifiers(),
                    wait,
                    ..MouseInput::at(MouseKind::Up, window, event.x, event.y)
                })?,
                StudioToApp::MouseMove(event) => self.client().mouse(&MouseInput {
                    modifiers: event.modifiers.into_key_modifiers(),
                    wait,
                    ..MouseInput::at(MouseKind::Move, window, event.x, event.y)
                })?,
                StudioToApp::Scroll(event) => self.client().mouse(&MouseInput {
                    dx: event.sx,
                    dy: event.sy,
                    modifiers: event.modifiers.into_key_modifiers(),
                    wait,
                    ..MouseInput::at(MouseKind::Scroll, window, event.x, event.y)
                })?,
                StudioToApp::KeyDown(event) => self.client().key(
                    window,
                    KeyKind::Down,
                    event.key_code,
                    event.modifiers,
                    wait,
                )?,
                StudioToApp::KeyUp(event) => self.client().key(
                    window,
                    KeyKind::Up,
                    event.key_code,
                    event.modifiers,
                    wait,
                )?,
                StudioToApp::TextInput(event) => {
                    self.client().text(window, &event.input, wait)?
                }
                other => {
                    return Err(TestError::new(format!(
                        "forward: {} has no equivalent on the standalone app remote route",
                        studio_msg_name(&other)
                    )))
                }
            }
        }
        Ok(())
    }

    /// Move the mouse to a point of the first window, no button down: what is there is hovered.
    pub fn move_to(&self, x: f64, y: f64) {
        self.client().mouse(&MouseInput::at(MouseKind::Move, Some(0), x, y)).unwrap_or_else(|err| panic_for_error(err));
    }

    /// Double-click a point in the first window. Both presses share one app timestamp.
    pub fn double_click_at(&self, x: f64, y: f64) {
        self.client().mouse(&MouseInput::at(MouseKind::DoubleClick, Some(0), x, y)).unwrap_or_else(|err| panic_for_error(err));
    }

    /// Press the mouse at a point of the first window and hold it, for what has no node of its own
    /// to press (a dock's splitter bar): [`PointerDrag::move_by`], then [`PointerDrag::release`].
    pub fn press_at(&self, x: f64, y: f64) -> PointerDrag {
        let window = Some(0);
        self.client().mouse(&MouseInput::at(MouseKind::Down, window, x, y)).unwrap_or_else(|err| panic_for_error(err));
        PointerDrag { app: self.clone(), window, start: (x, y), at: (x, y) }
    }

    /// Scrolls at a point in the focused window: a mouse wheel when `phase` is `None`, else one step of
    /// a trackpad gesture (`"began"`, `"changed"`, `"ended"`, `"momentum"`, `"momentum_ended"`, `"touched"`).
    pub fn scroll_at(&self, x: f64, y: f64, sx: f64, sy: f64, phase: Option<&'static str>) {
        if let Err(err) = self.try_scroll_at(x, y, sx, sy, phase) {
            panic_for_error(err);
        }
    }

    pub fn try_scroll_at(
        &self,
        x: f64,
        y: f64,
        sx: f64,
        sy: f64,
        phase: Option<&'static str>,
    ) -> TestResult<()> {
        self.ensure_running()?;
        self.client().mouse(&MouseInput {
            dx: sx,
            dy: sy,
            phase,
            ..MouseInput::at(MouseKind::Scroll, self.focus_window(), x, y)
        })?;
        self.pace_after_action();
        Ok(())
    }

    /// Drags one finger from the target's center through each offset in `path` in turn.
    fn try_touch_drag_from(
        &self,
        target: &NodeMatch,
        path: &[(f64, f64)],
        is_held: bool,
    ) -> TestResult<()> {
        self.ensure_running()?;
        let (start_x, start_y) = snapshot_center_f64(target);
        let window = Some(target.window_index());
        let client = self.client();
        let touch = |phase, (dx, dy): (f64, f64)| MouseInput {
            phase: Some(phase),
            ..MouseInput::at(MouseKind::Touch, window, start_x + dx, start_y + dy)
        };
        client.mouse(&touch("start", (0.0, 0.0)))?;
        let mut from = (0.0, 0.0);
        for &to in path {
            for step in 1..=DRAG_STEPS {
                let progress = step as f64 / DRAG_STEPS as f64;
                let at = (from.0 + (to.0 - from.0) * progress, from.1 + (to.1 - from.1) * progress);
                client.mouse(&touch("move", at))?;
            }
            from = to;
        }
        if is_held {
            thread::sleep(DRAG_HOLD);
            client.mouse(&touch("move", from))?;
        }
        client.mouse(&touch("stop", from))?;
        self.inner.borrow_mut().focus_window = window;
        self.pace_after_action();
        Ok(())
    }

    fn try_drag_from(
        &self,
        target: &NodeMatch,
        dx: f64,
        dy: f64,
        is_held: bool,
    ) -> TestResult<()> {
        self.ensure_running()?;
        let (start_x, start_y) = snapshot_center_f64(target);
        let window = Some(target.window_index());
        let client = self.client();
        client.mouse(&MouseInput::at(MouseKind::Down, window, start_x, start_y))?;
        for step in 1..=DRAG_STEPS {
            let progress = step as f64 / DRAG_STEPS as f64;
            client.mouse(&MouseInput::at(
                MouseKind::Move,
                window,
                start_x + dx * progress,
                start_y + dy * progress,
            ))?;
        }
        if is_held {
            thread::sleep(DRAG_HOLD);
            client.mouse(&MouseInput::at(MouseKind::Move, window, start_x + dx, start_y + dy))?;
        }
        client.mouse(&MouseInput::at(
            MouseKind::Up,
            window,
            start_x + dx,
            start_y + dy,
        ))?;
        self.inner.borrow_mut().focus_window = window;
        self.pace_after_action();
        Ok(())
    }

    fn query_widgets(
        &self,
        selector: &Selector,
        visible_only: bool,
    ) -> TestResult<Vec<NodeMatch>> {
        self.ensure_running()?;
        let client = self.client();
        let extra = if visible_only { "state=visible" } else { "" };
        let mut budget = None;
        self.wait_looks(&mut budget, "ui find: the app built no tree",
            || client.find_with_timeout(selector, extra, LOOK), is_no_tree_yet)
    }

    fn collect_logs_text(&self) -> TestResult<String> {
        let tail = self.client().log_since(0)?;
        let mut lines = tail.lines;
        if lines.len() > RECENT_LOG_LINES {
            let split = lines.len() - RECENT_LOG_LINES;
            lines = lines.split_off(split);
        }
        let mut out = String::new();
        for line in lines {
            let _ = writeln!(&mut out, "{line}");
        }
        Ok(out)
    }

    fn client(&self) -> crate::remote::RemoteClient {
        self.inner.borrow().app.client.clone()
    }

    fn focus_window(&self) -> Option<usize> {
        self.inner.borrow().focus_window
    }

    /// `ask` again while `pending` reads its error as "not yet" (each answer takes the app's
    /// look), until it answers or the app has spent the wait's [`Budget`], started at the first
    /// "not yet": the verdict says what did not come and what the app spent.
    fn wait_looks<T>(&self, budget: &mut Option<Budget>, what: &str, ask: impl FnMut() -> TestResult<T>, pending: impl Fn(&str) -> bool) -> TestResult<T> {
        wait_looks_with(what, ask, pending, || {
            let budget = match budget {
                Some(budget) => budget,
                None => budget.insert(Budget::start(self, self.action_timeout())?),
            };
            budget.spent(self)
        })
    }

    fn action_timeout(&self) -> Duration {
        self.inner.borrow().config.action_timeout
    }

    fn poll_interval(&self) -> Duration {
        self.inner.borrow().config.poll_interval
    }

    fn artifacts_dir(&self) -> PathBuf {
        self.inner.borrow().config.artifacts_dir.clone()
    }

    fn pace_after_action(&self) {
        let delay = self.inner.borrow().config.action_delay;
        if delay > Duration::ZERO {
            thread::sleep(delay);
        }
    }

    fn pause_before_shutdown(&self) {
        let delay = self.inner.borrow().config.keep_open;
        if delay > Duration::ZERO {
            thread::sleep(delay);
        }
    }

    fn ensure_running(&self) -> TestResult<()> {
        let mut inner = self.inner.borrow_mut();
        if let Some(status) = inner.app.poll_exit() {
            let pid = inner.app.pid;
            return Err(TestError::new(format!(
                "app process {pid} exited unexpectedly ({status}); stderr tail:\n{}",
                inner.app.stderr_tail()
            )));
        }
        Ok(())
    }

    fn keep_artifacts_on_pass(&self) -> bool {
        self.inner.borrow().config.keep_artifacts_on_pass
    }

    fn shutdown(&self, grab_frame: bool) -> ShutdownOutcome {
        let mut inner = self.inner.borrow_mut();
        let poll = inner.config.poll_interval;
        let outcome = inner.app.shutdown_with_grab(poll, grab_frame);
        let note = shutdown_note(outcome, grab_frame);
        let _ = fs::write(
            inner.config.artifacts_dir.join("shutdown.txt"),
            format!("pid {}\n{note}\n", inner.app.pid),
        );
        outcome
    }
}

fn should_grab_on_shutdown(result_is_err: bool, keep_artifacts_on_pass: bool) -> bool {
    result_is_err || keep_artifacts_on_pass
}

fn shutdown_note(outcome: ShutdownOutcome, grab_frame: bool) -> &'static str {
    match (outcome, grab_frame) {
        (ShutdownOutcome::Graceful, true) => "graceful: owned process exited after /gq or /quit",
        (ShutdownOutcome::Graceful, false) => "graceful: owned process exited after /quit (passing test; frame grab skipped)",
        (ShutdownOutcome::AlreadyExited, _) => "process had already exited before shutdown",
        (ShutdownOutcome::Killed, true) => "fallback: owned pid killed after /gq and /quit did not end it",
        (ShutdownOutcome::Killed, false) => "fallback: owned pid killed after /quit did not end it",
    }
}

pub struct Locator {
    app: TestApp,
    selector: Selector,
}

/// A mouse drag in progress: see [`Locator::press_and_drag`].
pub struct PointerDrag {
    app: TestApp,
    window: Option<usize>,
    start: (f64, f64),
    at: (f64, f64),
}

impl PointerDrag {
    /// Move in steps to `(dx, dy)` from the press point.
    pub fn move_by(&mut self, dx: f64, dy: f64) -> &mut Self {
        let client = self.app.client();
        let from = self.at;
        let to = (self.start.0 + dx, self.start.1 + dy);
        for step in 1..=DRAG_STEPS {
            let t = step as f64 / DRAG_STEPS as f64;
            self.at = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
            client.mouse(&MouseInput::at(MouseKind::Move, self.window, self.at.0, self.at.1)).unwrap_or_else(|err| panic_for_error(err));
        }
        self
    }

    /// A flick to `(dx, dy)` from the press point, `seconds` long, and let go at its end. Its events
    /// carry their own times, counted from zero, so what it throws is the same however long the
    /// app takes over each.
    pub fn fling_by(mut self, dx: f64, dy: f64, seconds: f64) {
        let client = self.app.client();
        let from = self.at;
        let to = (self.start.0 + dx, self.start.1 + dy);
        for step in 1..=DRAG_STEPS {
            let t = step as f64 / DRAG_STEPS as f64;
            self.at = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
            let moved = MouseInput { time: Some(seconds * t), ..MouseInput::at(MouseKind::Move, self.window, self.at.0, self.at.1) };
            client.mouse(&moved).unwrap_or_else(|err| panic_for_error(err));
        }
        let up = MouseInput { time: Some(seconds), ..MouseInput::at(MouseKind::Up, self.window, self.at.0, self.at.1) };
        client.mouse(&up).unwrap_or_else(|err| panic_for_error(err));
        self.app.inner.borrow_mut().focus_window = self.window;
        self.app.pace_after_action();
    }

    /// Let go where the pointer is.
    pub fn release(self) {
        self.app.client().mouse(&MouseInput::at(MouseKind::Up, self.window, self.at.0, self.at.1)).unwrap_or_else(|err| panic_for_error(err));
        self.app.inner.borrow_mut().focus_window = self.window;
        self.app.pace_after_action();
    }
}

impl Locator {
    pub fn wait_visible(self) -> Self {
        if let Err(err) = self.try_wait_visible() {
            panic_for_error(err);
        }
        self
    }

    pub fn try_wait_visible(&self) -> TestResult<()> {
        self.wait_query("state=visible", &[])
    }

    pub fn wait_hidden(self) -> Self {
        if let Err(err) = self.try_wait_hidden() {
            panic_for_error(err);
        }
        self
    }

    pub fn try_wait_hidden(&self) -> TestResult<()> {
        self.wait_query("state=visible", &[("gone", "1")])
    }

    pub fn wait_count(self, expected: usize) -> Self {
        if let Err(err) = self.try_wait_count(expected) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_wait_count(&self, expected: usize) -> TestResult<()> {
        self.wait_query("state=visible", &[("count", &expected.to_string())])
    }

    pub fn assert_text(self, expected: impl AsRef<str>) -> Self {
        if let Err(err) = self.try_assert_text(expected) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_assert_text(&self, expected: impl AsRef<str>) -> TestResult<()> {
        let widget = self.resolve_unique_readable()?;
        if widget.name == expected.as_ref() { Ok(()) }
        else { Err(TestError::new(format!("expected text {:?}, found {:?}", expected.as_ref(), widget.name))) }
    }

    pub fn wait_text(self, expected: impl AsRef<str>) -> Self {
        if let Err(err) = self.try_wait_text(expected) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_wait_text(&self, expected: impl AsRef<str>) -> TestResult<()> {
        self.wait_query(&format!("name={}", makepad_a11y::query::quote(expected.as_ref())), &[])
    }

    pub fn assert_value(self, expected: impl AsRef<str>) -> Self {
        if let Err(err) = self.try_assert_value(expected) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_assert_value(&self, expected: impl AsRef<str>) -> TestResult<()> {
        let widget = self.resolve_unique_readable()?;
        if widget.value == expected.as_ref() { Ok(()) }
        else { Err(TestError::new(format!("expected value {:?}, found {:?}", expected.as_ref(), widget.value))) }
    }

    pub fn wait_value(self, expected: impl AsRef<str>) -> Self {
        if let Err(err) = self.try_wait_value(expected) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_wait_value(&self, expected: impl AsRef<str>) -> TestResult<()> {
        self.wait_query(&format!("value={}", makepad_a11y::query::quote(expected.as_ref())), &[])
    }

    pub fn assert_checked(self, expected: bool) -> Self {
        if let Err(err) = self.try_assert_checked(expected) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_assert_checked(&self, expected: bool) -> TestResult<()> {
        let widget = self.resolve_unique_readable()?;
        if widget.checked() == Some(expected) { Ok(()) }
        else { Err(TestError::new(format!("expected checked={expected}, found {:?}", widget.toggle))) }
    }

    pub fn wait_checked(self, expected: bool) -> Self {
        if let Err(err) = self.try_wait_checked(expected) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_wait_checked(&self, expected: bool) -> TestResult<()> {
        self.wait_query(if expected { "state=checked" } else { "state=!checked,!mixed" }, &[])
    }

    pub fn assert_enabled(self, expected: bool) -> Self {
        if let Err(err) = self.try_assert_enabled(expected) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_assert_enabled(&self, expected: bool) -> TestResult<()> {
        let widget = self.resolve_unique_readable()?;
        if widget.enabled() == expected { Ok(()) }
        else { Err(TestError::new(format!("expected enabled={expected}, found {}", widget.enabled()))) }
    }

    pub fn wait_enabled(self, expected: bool) -> Self {
        if let Err(err) = self.try_wait_enabled(expected) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_wait_enabled(&self, expected: bool) -> TestResult<()> {
        self.wait_query(if expected { "state=enabled" } else { "state=disabled" }, &[])
    }

    pub fn click(self) -> Self {
        if let Err(err) = self.try_click() {
            panic_for_error(err);
        }
        self
    }

    pub fn try_click(&self) -> TestResult<()> {
        self.act("press", &[])
    }

    /// Raw pointer input for hit-testing tests, located by semantic bounds.
    pub fn click_at_center(self) -> Self {
        if let Err(err) = self.raw_pointer(MouseKind::Click, 0.0, 0.0) {
            panic_for_error(err);
        }
        self
    }

    /// Raw wheel input for containment, clipping and gesture tests.
    pub fn wheel(self, dx: f64, dy: f64) -> Self {
        if let Err(err) = self.raw_pointer(MouseKind::Scroll, dx, dy) {
            panic_for_error(err);
        }
        self
    }

    fn raw_pointer(&self, kind: MouseKind, dx: f64, dy: f64) -> TestResult<()> {
        let target = self.resolve_unique_visible()?;
        let (x, y) = snapshot_center_f64(&target);
        self.app.client().mouse(&MouseInput {
            dx,
            dy,
            ..MouseInput::at(kind, Some(target.window_index()), x, y)
        })
    }

    pub fn type_text(self, text: impl AsRef<str>) -> Self {
        if let Err(err) = self.try_type_text(text) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_type_text(&self, text: impl AsRef<str>) -> TestResult<()> {
        self.type_in(&[("t",text.as_ref())])
    }

    pub fn clear(self) -> Self {
        if let Err(err) = self.try_clear() {
            panic_for_error(err);
        }
        self
    }

    pub fn try_clear(&self) -> TestResult<()> {
        self.try_fill("")
    }

    pub fn fill(self, text: impl AsRef<str>) -> Self {
        if let Err(err) = self.try_fill(text) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_fill(&self, text: impl AsRef<str>) -> TestResult<()> {
        let node = self.resolve_unique_readable()?;
        let len = node.text.as_ref().map_or(node.value.len(), |t| t.content.len());
        self.type_in(&[("t",text.as_ref()),("from","0"),("to",&len.to_string())])
    }

    /// `ui type`'s steps, each an act waited for under a wait's [`Budget`]: the focus and its
    /// frame, then the text: through the node's replace_text, or typed into the focus as text
    /// input when it has none (the code editor).
    fn type_in(&self, extra: &[(&str,&str)]) -> TestResult<()> {
        self.act("focus", &[])?;
        let node = self.resolve_unique_readable()?;
        self.act(if node.actions.contains(makepad_a11y::Actions::REPLACE_TEXT) { "replace_text" } else { "type" }, extra)
    }

    pub fn press_key(self, key_code: KeyCode) -> Self {
        if let Err(err) = self.try_press_key(key_code) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_press_key(&self, key_code: KeyCode) -> TestResult<()> {
        self.act("focus", &[])?;
        self.app.try_press_key(key_code)
    }

    pub fn press_key_with_modifiers(self, key_code: KeyCode, modifiers: KeyModifiers) -> Self {
        if let Err(err) = self.try_press_key_with_modifiers(key_code, modifiers) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_press_key_with_modifiers(
        &self,
        key_code: KeyCode,
        modifiers: KeyModifiers,
    ) -> TestResult<()> {
        self.act("focus", &[])?;
        self.app.try_press_key_with_modifiers(key_code, modifiers)
    }

    pub fn scroll(self, sx: f64, sy: f64) -> Self {
        if let Err(err) = self.try_scroll(sx, sy) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_scroll(&self, sx: f64, sy: f64) -> TestResult<()> {
        self.act("scroll", &[("dx",&sx.to_string()),("dy",&sy.to_string())])
    }

    pub fn drag_by(self, dx: f64, dy: f64) -> Self {
        if let Err(err) = self.try_drag_by(dx, dy) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_drag_by(&self, dx: f64, dy: f64) -> TestResult<()> {
        let target = self.resolve_unique_visible()?;
        self.app.try_drag_from(&target, dx, dy, false)
    }

    /// Like [`Self::drag_by`], but rests at the end before letting go, so the drag doesn't fling.
    pub fn drag_by_and_hold(self, dx: f64, dy: f64) -> Self {
        if let Err(err) = self.try_drag_by_and_hold(dx, dy) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_drag_by_and_hold(&self, dx: f64, dy: f64) -> TestResult<()> {
        let target = self.resolve_unique_visible()?;
        self.app.try_drag_from(&target, dx, dy, true)
    }

    /// Press here and keep the button down: [`PointerDrag::move_by`], rest as long as the test
    /// wants (what the app does meanwhile is part of the gesture), then [`PointerDrag::release`].
    pub fn press_and_drag(self) -> PointerDrag {
        let target = self.resolve_unique_visible().unwrap_or_else(|err| panic_for_error(err));
        let (x, y) = snapshot_center_f64(&target);
        let window = Some(target.window_index());
        let client = self.app.client();
        client.mouse(&MouseInput::at(MouseKind::Down, window, x, y)).unwrap_or_else(|err| panic_for_error(err));
        PointerDrag { app: self.app, window, start: (x, y), at: (x, y) }
    }

    /// Like [`Self::drag_by`], with one finger on a touch screen instead of the mouse.
    pub fn touch_drag_by(self, dx: f64, dy: f64) -> Self {
        if let Err(err) = self.try_touch_drag_by(dx, dy) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_touch_drag_by(&self, dx: f64, dy: f64) -> TestResult<()> {
        let target = self.resolve_unique_visible()?;
        self.app.try_touch_drag_from(&target, &[(dx, dy)], false)
    }

    /// Like [`Self::drag_by_and_hold`], with one finger on a touch screen instead of the mouse.
    pub fn touch_drag_by_and_hold(self, dx: f64, dy: f64) -> Self {
        if let Err(err) = self.try_touch_drag_by_and_hold(dx, dy) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_touch_drag_by_and_hold(&self, dx: f64, dy: f64) -> TestResult<()> {
        let target = self.resolve_unique_visible()?;
        self.app.try_touch_drag_from(&target, &[(dx, dy)], true)
    }

    /// Like [`Self::touch_drag_by_and_hold`], through each offset from the start in turn, like a finger that turns.
    pub fn touch_drag_through_and_hold(self, path: &[(f64, f64)]) -> Self {
        if let Err(err) = self.try_touch_drag_through_and_hold(path) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_touch_drag_through_and_hold(&self, path: &[(f64, f64)]) -> TestResult<()> {
        let target = self.resolve_unique_visible()?;
        self.app.try_touch_drag_from(&target, path, true)
    }

    pub fn snapshot(&self) -> NodeMatch {
        match self.try_snapshot() {
            Ok(widget) => widget,
            Err(err) => panic_for_error(err),
        }
    }

    pub fn try_snapshot(&self) -> TestResult<NodeMatch> {
        self.resolve_unique_visible()
    }

    pub fn count(&self) -> usize {
        match self.try_count() {
            Ok(count) => count,
            Err(err) => panic_for_error(err),
        }
    }

    pub fn try_count(&self) -> TestResult<usize> {
        Ok(self.app.query_widgets(&self.selector, true)?.len())
    }

    /// An action, then the present it brings, each waited for a look at a time under a wait's
    /// [`Budget`]: an app busy taking it (its main thread compiling the code the action runs for
    /// the first time, say) is no app that sits idle.
    fn act(&self, action: &str, extra: &[(&str,&str)]) -> TestResult<()> {
        self.app.ensure_running()?;
        let command = format!("ui act {action}");
        let q = self.selector.as_query();
        let look = LOOK.as_millis().to_string();
        let mut args = vec![("q",q.as_str()),("json","1"),("timeout_ms",look.as_str())];
        if let Some(w) = self.selector.window_arg() { args.push(("w",w)); }
        args.extend_from_slice(extra);
        // the node's window, read before the act: a press that removes its own button (a
        // dialog's Cancel) leaves no node to look up after it, and its window's frame is what comes
        let window = match self.selector.window_arg() {
            Some(w) => w.to_string(),
            None => self.resolve_unique_visible()?.window_arg(),
        };
        let mut budget = None;
        // The app builds its tree for the act's own request: one with none yet (its first, a
        // look's time on a busy box) did nothing, and is asked again under the same Budget.
        let acted = self.app.wait_looks(&mut budget, &format!("{command}: the app built no tree to act in"),
            || self.app.client().get_json(&command, &args), is_no_tree_yet);
        if let Err(refused) = acted {
            let Some(req) = pending_action(refused.message()) else {
                // The app's refusal ("no such node", "ambiguous") names no selector: when the
                // selector does not pick one visible widget, say which selector and what it matched.
                self.resolve_unique_visible()?;
                return Err(refused);
            };
            let req = req.to_string();
            let done = [("req",req.as_str()),("json","1"),("timeout_ms",look.as_str())];
            self.app.wait_looks(&mut budget, &format!("{command}: the app did not take the action"),
                || self.app.client().get_json("ui done", &done), |e| pending_action(e).is_some())?;
        }
        let present = [("w",window.as_str()),("json","1"),("timeout_ms",look.as_str())];
        self.app.wait_looks(&mut budget, &format!("{command}: the action's frame did not present"),
            || self.app.client().get_json("ui present", &present), |e| e.contains("presentation timed out"))?;
        if let Some(node) = self.app.query_widgets(&self.selector, false)?.first() {
            self.app.inner.borrow_mut().focus_window = Some(node.window_index());
        }
        self.app.pace_after_action();
        Ok(())
    }
    /// The app's `ui wait`, a look at a time: it ends on what it waits for, or once the app
    /// has spent the wait's [`Budget`].
    fn wait_query(&self, extra: &str, options: &[(&str,&str)]) -> TestResult<()> {
        self.app.ensure_running()?;
        let q = format!("{} {extra}", self.selector.as_query());
        let look = LOOK.as_millis().to_string();
        let mut args = vec![("q",q.as_str()),("json","1"),("timeout_ms",look.as_str())];
        if let Some(w) = self.selector.window_arg() { args.push(("w",w)); }
        args.extend_from_slice(options);
        let mut budget = Budget::start(&self.app, self.app.action_timeout())?;
        loop {
            match self.app.client().get_json("ui wait", &args) {
                Err(err) if err.message().contains("ui wait timed out") => {
                    if let Some(spent) = budget.spent(&self.app)? {
                        return Err(TestError::new(format!("app remote ui wait: `{q}` did not come: {spent}")));
                    }
                }
                done => return done.map(|_| ()),
            }
        }
    }
    pub fn press(self) -> Self { self.click() }
    pub fn focus(self) -> Self { if let Err(e) = self.act("focus", &[]) { panic_for_error(e); } self }
    pub fn toggle(self) -> Self { if let Err(e) = self.act("toggle", &[]) { panic_for_error(e); } self }
    pub fn check(self, checked: bool) -> Self {
        let result = self.resolve_unique_readable().and_then(|node| {
            if node.checked() == Some(checked) { Ok(()) } else { self.act("toggle", &[])?; self.try_wait_checked(checked) }
        });
        if let Err(e) = result { panic_for_error(e); } self
    }
    pub fn select(self) -> Self { if let Err(e) = self.act("select", &[]) { panic_for_error(e); } self }
    pub fn add_selection(self) -> Self { if let Err(e) = self.act("add_selection", &[]) { panic_for_error(e); } self }
    pub fn expand(self) -> Self { if let Err(e) = self.act("expand", &[]) { panic_for_error(e); } self }
    pub fn show_menu(self) -> Self { if let Err(e) = self.act("show_menu", &[]) { panic_for_error(e); } self }
    pub fn select_option(self, value: impl AsRef<str>) -> Self {
        if let Err(e) = self.act("select", &[("v",value.as_ref())]) { panic_for_error(e); } self
    }
    pub fn scroll_into_view(self) -> Self {
        if let Err(e) = self.act("scroll_into_view", &[]) { panic_for_error(e); } self
    }
    pub fn set_value(self, value: impl ToString) -> Self {
        if let Err(e) = self.act("set_value", &[("v", &value.to_string())]) { panic_for_error(e); } self
    }

    fn resolve_unique_visible(&self) -> TestResult<NodeMatch> {
        let query = self.selector.describe();
        let matches = self.app.query_widgets(&self.selector, true)?.into_iter().filter(snapshot_is_visible).collect();
        Self::unique(query, matches, "matched no visible widgets")
    }

    /// Resolution for state reads (`wait_text`, `assert_checked`, …) rather
    /// than interaction.
    ///
    /// A widget the app has drawn but a container clips away — content below
    /// the fold of a scrolling page, a status label under a long form — has no
    /// on-screen rect, so there is nothing to click and `visible` is false.
    /// Its *state* is still perfectly readable, though, and asserting on it is
    /// a normal thing for a test to want. So prefer a visible match (identical
    /// behaviour whenever one exists) and fall back to an off-screen one only
    /// when nothing visible matches.
    fn resolve_unique_readable(&self) -> TestResult<NodeMatch> {
        let query = self.selector.describe();
        let visible = self.app.query_widgets(&self.selector, true)?;
        if !visible.is_empty() {
            return Self::unique(query, visible, "matched no visible widgets");
        }
        let all = self.app.query_widgets(&self.selector, false)?;
        Self::unique(query, all, "matched no widgets")
    }

    fn unique(
        query: String,
        matches: Vec<NodeMatch>,
        empty_message: &str,
    ) -> TestResult<NodeMatch> {
        match matches.as_slice() {
            [] => Err(TestError::new(format!("selector `{query}` {empty_message}"))),
            [single] => Ok(single.clone()),
            _ => Err(TestError::new(format!(
                "selector `{query}` matched multiple widgets:\n{}",
                matches
                    .iter()
                    .map(snapshot_summary)
                    .collect::<Vec<_>>()
                    .join("\n")
            ))),
        }
    }
}

pub fn run_with_config<F, R>(config: TestConfig, test: F) -> TestResult<()>
where
    F: FnOnce(TestApp) -> R,
    R: IntoTestResult,
{
    // Tests are serialised by default: each one drives a whole app process, so
    // running several at once oversubscribes the machine and makes timing-
    // sensitive assertions flaky. A suite whose waits count the app's own time can opt out.
    let _guard = (!config.concurrent).then(|| {
        TEST_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    });

    // what the test runs is no app's progress for the waits of the tests beside it
    let _drove = Drove::enter();
    let app = TestApp::start(config)?;
    let result = catch_unwind(AssertUnwindSafe(|| test(app.clone()).into_test_result()));

    let clashes = read_lookup_clashes(&app);
    let with_clashes = |err: TestError| {
        let err = with_jit_capture(&app, err);
        match &clashes {
            Ok(lines) if !lines.is_empty() => {
                TestError::new(format!("{err}\n  ambiguous widget lookups:\n{lines}"))
            }
            Err(read_err) => TestError::new(format!("{err}\n  {read_err}")),
            _ => err,
        }
    };
    let result = match result {
        Ok(Ok(())) => clashes.and_then(|lines| {
            if lines.is_empty() {
                Ok(())
            } else {
                Err(TestError::new(format!(
                    "ambiguous widget lookups: a lookup for one widget matched several; rename one or look it up by a longer path:\n{lines}"
                )))
            }
        }).map_err(|err| with_jit_capture(&app, err)),
        Ok(Err(err)) => Err(with_clashes(err)),
        Err(payload) => Err(with_clashes(TestError::from_panic_payload(payload))),
    };
    let grab_frame = should_grab_on_shutdown(result.is_err(), app.keep_artifacts_on_pass());
    if let Err(err) = &result {
        capture_failure_artifacts(&app, err.message());
    }
    app.pause_before_shutdown();
    app.shutdown(grab_frame);
    result
}

fn read_lookup_clashes(app: &TestApp) -> TestResult<String> {
    let path = app.artifacts_dir().join("lookup-clashes.txt");
    match fs::read_to_string(&path) {
        Ok(lines) => Ok(lines.trim_end().to_owned()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(TestError::new(format!("could not read {}: {err}", path.display()))),
    }
}

pub fn run_current_package_test<F, R>(
    package_name: &str,
    manifest_dir: &str,
    module_path: &str,
    test_name: &str,
    test: F,
) where
    F: FnOnce(TestApp) -> R,
    R: IntoTestResult,
{
    let full_test_name = if module_path.is_empty() {
        test_name.to_string()
    } else {
        format!("{module_path}::{test_name}")
    };
    let config = match TestConfig::current_package(manifest_dir, package_name, full_test_name) {
        Ok(config) => config,
        Err(err) => panic_for_error(err),
    };
    if let Err(err) = run_with_config(config, test) {
        panic_for_error(err);
    }
}

/// What a wait may spend before it fails: `limit` of the app's main thread's CPU (`action_timeout`,
/// or a long wait's own), or `action_timeout` with the app idle. A loaded box spends neither for
/// the app: a thread that waits for a core runs no CPU of its own, and one that waits for the JIT
/// to compile code it runs for the first time has work pending (`waits_on_jit`): neither is idle.
/// So the idle rule reads what the app's threads wanted, not only what they got: what they ran,
/// and what its threads (not its UI thread, its serving ones or the JIT's) spent runnable,
/// waiting for a core (`AppCpu::waited`, Linux's run_delay; macOS and Windows keep none per thread
/// and read what the app ran alone). (A Fab test's part drag beside 24 busy loops: its brep worker
/// ran at 17-24% of a core and waited for one the rest, each step 13-16 s, and read as a tick that
/// sat idle 10 s, J11427, 11-11.)
/// An app with nothing running (its own threads under 1/[`IDLE_SHARE`] of a core: not what its
/// threads serving remote peers run, the wait's own asks cost the app CPU, nor what the JIT's own
/// threads run beside it, its background checks and tier-ups, which bring the app nothing) and
/// nothing pending (`settled`, no install it waits on) has nothing left that would bring what the
/// wait waits for: `action_timeout` of that ends the wait. (A cold JIT's background type check ran
/// a whole core for 26 s beside an app that sat idle: the wait counted it as the app's, 11-09.) What
/// the UI thread runs answering an ask (this wait's own looks included) the app counts as served;
/// what the closure's asks set off beyond their answers (the frame an input brings) is left out with
/// the closure's time, so the share is the app's while the wait left it alone: what it ran outside
/// the wait's closure, over the time outside it. Three stalls run no CPU either and are no idle
/// app, and the verdict names them: a look whose process took major page faults at a swapping rate
/// ([`SWAP_FAULTS_PER_S`]) waited on memory (a thread that waits for its page runs no CPU; one fault
/// in 10 s read as "waiting on memory", 11-10), one that found threads of it waiting in the kernel
/// uninterruptibly (`AppCpu::io_threads`: a file's sync, a read) waited on the disk (a store's
/// syncs on a loaded box ran no CPU for seconds and read as "sat idle", 11-21), and one with only
/// the app pool's queued jobs pending, none of them taken since the last look, is a starved pool
/// (its jobs wait for a CPU permit the process's budget does not grant). Such a stretch ends the
/// wait as soon.
/// (A test unit at 29.8 GB beside another suite on a swapping box read as "sat idle", with 7,324
/// Background jobs queued, 11-10.) A starve counts from the second look that finds the jobs queued,
/// none taken (a job queued since the look before has waited for nothing yet). The verdict names
/// a stall only while it lasts to the wait's end: an app whose jobs got their permits, or whose
/// faults or disk waits stopped, with nothing pending since, is an idle app, and the verdict says
/// the stall before as a clause. (On a loaded box a counter's wait read as a starved pool for one
/// look in 10 that found its font scan still queued for a permit, and as "waiting on memory
/// (swapping)" for one look's 89 page faults with no memory pressure on the box, 11-12.) And a
/// memory or disk wait is the verdict only when it is why the app ran nothing, every look of the
/// idle stretch finding it: one that began after the app had sat idle with nothing pending is a
/// clause too. (An app idle 9.3 s took 80 major page faults in the wait's last look, on a box at
/// 1.2% memory pressure, and read as "waiting on memory (swapping)" for 1.0 of the 10.3 s, 11-22.)
/// And a test may run what
/// the app waits on in its own process (an AI hub on loopback with its GPU device): what the test
/// process runs beside the threads driving its tests ([`beside_now`]) is progress the app waits
/// for, at half a core or more of what its threads wanted (as an app's own other threads): the app
/// is not idle while it runs. (A grown part's refine on a cold cache: the app at 0%, the test's hub
/// compiling its GPU programs, read as "sat idle 10.2 s", 11-10.) Below that it is a steady tick,
/// no progress, and the verdict names it: held to the idle share, a hub the test process ran,
/// started by an earlier test, ticked 5-9% of a core in its idle worker's 500 ms wakes, every look
/// read as progress, and a wait for a panel line that never came ran 40 min beside an app at 0%
/// (`grow_live`, 11-27). An app whose main thread runs nothing, its
/// other threads under 1/[`TICK_SHARE`] of a core, with nothing pending, is idle too: a steady
/// tick is no progress, while a thread at work, at half a core or more, still is. (A Fab test's
/// wait for a button that never came ran 19 min beside its app's sim ticking at 4-12% of a core,
/// its main thread parked in poll, L798418, 11-10.) An app that runs nothing (or only ticks) while
/// it says something is pending (a load held on its way, a fetch nobody answers) is no idle app,
/// but nothing it runs brings what it waits on either: that stretch counts against the wait's
/// budget as wall time, and once it lasts the budget (the idle limit at least) the wait fails,
/// naming what the app said was pending. (A Fab test's wait for its view at rest sat 17 min beside
/// a load the test held, its app at 0% and saying the load was on its way, 11-20.) That verdict
/// names disk, memory and pool stalls by the same rules, over the stretch with work pending.
struct Budget {
    /// The main thread's CPU the wait may spend.
    limit: Duration,
    /// How long the app may sit idle.
    idle_limit: Duration,
    /// The main thread's CPU when the wait began.
    main: Duration,
    /// When the app was last looked at, its own CPU then (the process's but serving), its main
    /// thread's, and what its other threads had waited for a core.
    last: (Instant, Duration, Duration, Duration),
    /// How long the app has been idle, to the last look.
    idle: Duration,
    /// Of the idle stretch, how long only a tick ran (its main thread nothing, its other threads
    /// under 1/[`TICK_SHARE`] of a core), and the CPU they ran in it.
    ticked: (Duration, Duration),
    /// The process's major page faults at the last look, and of the idle or pending stretch, the memory wait
    /// that lasts to the last look (how long its looks took them at a swapping rate, and how many),
    /// and the memory waits before it, whose faults stopped.
    faults: u64,
    memory: (Duration, u64),
    paged: (Duration, u64),
    /// Of the idle or pending stretch, the disk wait that lasts to the last look (how long its looks found
    /// threads of it waiting in the kernel uninterruptibly, and the most at once), and the disk
    /// waits before it, which ended.
    disk: (Duration, u32),
    disked: (Duration, u32),
    /// The jobs the app pool's workers had taken at the last look, and whether it found jobs queued.
    taken: u64,
    queued: bool,
    /// Of the idle or pending stretch, the starve that lasts to the last look (how long its looks found the
    /// jobs the look before found queued still there, none taken), and the jobs queued per class at
    /// the last; and how long the starves before it lasted, whose jobs got their permits.
    starved: (Duration, [u64; 3]),
    granted: Duration,
    /// Who held the CPU budget's slots at the last look that found the pool starved.
    slots: Vec<crate::remote::BudgetSlot>,
    /// What the test process had run and waited for beside its driving threads at the last look
    /// ([`beside_now`]), and how long, over the whole wait, that alone kept the app from counting
    /// as idle.
    beside: (Beside, Duration),
    /// Of the idle stretch, how long the test process's own threads only ticked (over
    /// 1/[`IDLE_SHARE`] of a core, under 1/[`TICK_SHARE`] of what they wanted), and the CPU they
    /// ran in it.
    beside_ticked: (Duration, Duration),
    /// The time the wait spent asking the app (its closure ran), and the app's own CPU and its main
    /// thread's in it, since the last look.
    asked: (Duration, Duration, Duration),
    /// How long the app has run nothing while it said something was pending, to the last look, and
    /// what it said then.
    pending: (Duration, Vec<String>),
    /// What the processes the app started have run, which the wait counts as its own CPU.
    started: Started,
}

/// What the processes an app started ran (a tool it runs for a call, a picture it hands to another
/// program), which its own CPU does not show: a wait on a call it answers once that program ends
/// would read it as running nothing. Each one's CPU since a look first found it, all of it for one
/// started since, summed; one that ended keeps what it ran to the last look that found it.
#[derive(Default)]
struct Started {
    seen: HashMap<u32, u64>,
    ran: Duration,
    now: Vec<(u32, u64)>,
}

impl Started {
    /// What they ran so far, the processes `pid` started that run now looked at.
    fn look(&mut self, pid: u32) -> Duration {
        self.now.clear();
        makepad_hal::clock::started_cpu_ns(pid, &mut self.now);
        let seen = std::mem::take(&mut self.seen);
        for &(child, cpu) in &self.now {
            self.ran += Duration::from_nanos(cpu.saturating_sub(seen.get(&child).copied().unwrap_or(0)));
            self.seen.insert(child, cpu);
        }
        self.ran
    }
}

impl Budget {
    fn start(app: &TestApp, limit: Duration) -> TestResult<Budget> {
        let mut started = Started::default();
        let mut cpu = app.client().cpu(true)?;
        cpu.own += started.look(app.pid());
        Ok(Budget { started, ..Budget::at(limit, app.action_timeout(), cpu, Instant::now(), beside_now()) })
    }

    /// A wait's budget from the app's CPU `cpu` and the test process's `beside` it at `now`.
    fn at(limit: Duration, idle_limit: Duration, cpu: crate::remote::AppCpu, now: Instant, beside: Beside) -> Budget {
        Budget { limit, idle_limit, main: cpu.main, last: (now, cpu.own, cpu.main, cpu.waited), idle: Duration::ZERO, ticked: (Duration::ZERO, Duration::ZERO), faults: cpu.faults, memory: (Duration::ZERO, 0), paged: (Duration::ZERO, 0), disk: (Duration::ZERO, 0), disked: (Duration::ZERO, 0), taken: 0, queued: false, starved: (Duration::ZERO, [0; 3]), granted: Duration::ZERO, slots: Vec::new(), beside: (beside, Duration::ZERO), beside_ticked: (Duration::ZERO, Duration::ZERO), asked: (Duration::ZERO, Duration::ZERO, Duration::ZERO), pending: (Duration::ZERO, Vec::new()), started: Started::default() }
    }

    /// The wait asked the app from `since` (its closure ran): its CPU before and after.
    fn asked(&mut self, since: Instant, before: crate::remote::AppCpu, after: crate::remote::AppCpu) {
        self.asked.0 += since.elapsed();
        self.asked.1 += after.own.saturating_sub(before.own);
        self.asked.2 += after.main.saturating_sub(before.main);
    }

    fn look_due(&self) -> bool {
        self.last.0.elapsed() >= LOOK
    }

    /// Looks at the app: once it has spent the whole budget, what it spent, in words.
    fn spent(&mut self, app: &TestApp) -> TestResult<Option<String>> {
        let mut cpu = app.client().cpu(true)?;
        cpu.own += self.started.look(app.pid());
        self.judge(cpu, Instant::now(), beside_now(), &mut || app.client().settled(), &mut || app.client().waits_on_jit())
    }

    /// [`Self::spent`] from a look that found `cpu` and the test process's `beside` at `now`;
    /// `settled` and `waits_on_jit` ask the app, only when the app ran nothing.
    fn judge(&mut self, cpu: crate::remote::AppCpu, now: Instant, beside: Beside, settled: &mut dyn FnMut() -> TestResult<crate::remote::AppSettled>, waits_on_jit: &mut dyn FnMut() -> TestResult<bool>) -> TestResult<Option<String>> {
        let main = cpu.main.saturating_sub(self.main);
        if main >= self.limit {
            return Ok(Some(format!("its main thread ran {:.1} s of CPU (budget {:.0} s)", main.as_secs_f64(), self.limit.as_secs_f64())));
        }
        let (then, own, main_then, waited) = self.last;
        let over = now - then;
        // what the app ran left alone, over the time it was
        let (asking, asked, asked_main) = std::mem::take(&mut self.asked);
        let alone = over.saturating_sub(asking);
        let ran = cpu.own.saturating_sub(own).saturating_sub(asked);
        // and what it wanted: that, and what its threads waited for a core meanwhile
        let wanted = ran + cpu.waited.saturating_sub(waited);
        let quiet = wanted * IDLE_SHARE < alone;
        // its main thread ran nothing and its other threads only tick: no progress either
        let ticking = !quiet && cpu.main.saturating_sub(main_then).saturating_sub(asked_main) * IDLE_SHARE < alone && wanted * TICK_SHARE < alone;
        // what the test process ran and waited for beside its driving threads: progress at half a
        // core of what it wanted (TICK_SHARE), a steady tick below that over the idle share (no
        // progress, as the app's other threads)
        let beside_ran = beside.cpu.saturating_sub(self.beside.0.cpu);
        let beside_wanted = beside_ran + beside.waited.saturating_sub(self.beside.0.waited);
        let busy_beside = beside_wanted * TICK_SHARE >= over;
        let beside_ticking = !busy_beside && beside_ran * IDLE_SHARE >= over;
        self.beside.0 = beside;
        if (quiet || ticking) && busy_beside {
            self.beside.1 += over;
        }
        let mut settled = if (quiet || ticking) && !busy_beside { Some(settled()?) } else { None };
        let slots = settled.as_mut().map(|s| std::mem::take(&mut s.slots)).unwrap_or_default();
        let (queued, running, taken) = settled.as_ref().and_then(|s| s.pool).unwrap_or(([0; 3], 0, self.taken));
        // the app ran nothing (or only ticked), with nothing pending (or only pool jobs, none taken
        // since the last look), and nothing ran beside the test in its own process
        let stalled = settled.as_ref().is_some_and(|s| s.settled || (s.only_pool && taken == self.taken)) && !waits_on_jit()?;
        // it ran nothing (or only ticked) with something pending that is not its pool: what it
        // waits on runs nowhere it would show
        let waiting = !stalled && settled.as_ref().is_some_and(|s| !s.settled && !s.only_pool) && !waits_on_jit()?;
        let continuing = (stalled && !self.idle.is_zero()) || (waiting && !self.pending.0.is_zero());
        if !continuing {
            (self.memory, self.paged, self.disk, self.disked, self.starved, self.granted) = Default::default();
            self.queued = false;
            self.slots.clear();
        }
        match (waiting, settled.as_mut()) {
            (true, Some(s)) => self.pending = (self.pending.0 + over, std::mem::take(&mut s.pending)),
            _ => self.pending = (Duration::ZERO, Vec::new()),
        }
        let faults = cpu.faults.saturating_sub(self.faults);
        let queue = queued.iter().any(|&n| n > 0);
        if stalled {
            self.idle += over;
            if ticking {
                self.ticked = (self.ticked.0 + over, self.ticked.1 + ran);
            }
            if beside_ticking {
                self.beside_ticked = (self.beside_ticked.0 + over, self.beside_ticked.1 + beside_ran);
            }
        } else {
            self.idle = Duration::ZERO;
            self.ticked = (Duration::ZERO, Duration::ZERO);
            self.beside_ticked = (Duration::ZERO, Duration::ZERO);
        }
        if stalled || waiting {
            if faults as f64 >= SWAP_FAULTS_PER_S * over.as_secs_f64() {
                self.memory = (self.memory.0 + over, self.memory.1 + faults);
            } else {
                // its faults stopped: the memory wait ended, and what stalls the app now is not it
                let (waited, n) = std::mem::take(&mut self.memory);
                self.paged = (self.paged.0 + waited, self.paged.1 + n);
            }
            if cpu.io_threads > 0 {
                self.disk = (self.disk.0 + over, self.disk.1.max(cpu.io_threads));
            } else {
                // no thread of it waits on the disk now: that wait ended
                let (waited, n) = std::mem::take(&mut self.disk);
                self.disked = (self.disked.0 + waited, self.disked.1.max(n));
            }
            if queue && self.queued && taken == self.taken {
                // the jobs the last look found queued are there still, none taken: they waited the
                // whole look for a permit (a job queued since has waited for nothing yet)
                self.starved = (self.starved.0 + over, queued);
                self.slots = slots;
            } else {
                // its queue emptied or a worker took a job: any previous starve ended
                self.granted += std::mem::take(&mut self.starved).0;
            }
        }
        self.last = (now, cpu.own, cpu.main, cpu.waited);
        self.faults = cpu.faults;
        self.taken = taken;
        self.queued = queue;
        let pending_limit = self.limit.max(self.idle_limit);
        let pending = self.pending.0 >= pending_limit;
        let (stretch, limit, waited_on) = if pending {
            (self.pending.0, pending_limit, format!(" while it said it waited on {}", self.pending.1.join(", ")))
        } else if self.idle >= self.idle_limit {
            (self.idle, self.idle_limit, String::new())
        } else {
            return Ok(None);
        };
        let (seconds, budget, main) = (stretch.as_secs_f64(), limit.as_secs_f64(), main.as_secs_f64());
        let beside = match self.beside.1 {
            busy if !busy.is_zero() && !pending => format!("; before that the test process's own threads (a service the test runs in it, as a hub) were busy for {:.1} s, which the wait counted as progress", busy.as_secs_f64()),
            _ => String::new(),
        };
        // A memory or disk wait explains the stretch only when every look found it.
        let covers = |waited: Duration| waited == stretch;
        let whole_stretch = || if pending {
            format!("{seconds:.1} s{waited_on}")
        } else {
            format!("{seconds:.1} of the {seconds:.1} s it ran nothing")
        };
        Ok(Some(match (self.memory, self.disk, self.starved) {
            ((waited, n), _, _) if n > 0 && covers(waited) => format!(
                "it was waiting on memory (swapping) for {} (budget {budget:.0} s): {n} major page faults, {:.0} a second while it waited{} (its main thread ran {main:.1} s of CPU){beside}",
                whole_stretch(),
                n as f64 / waited.as_secs_f64(),
                pressure("memory").map_or(String::new(), |p| format!("; the box's memory pressure: tasks waited on memory {p:.1}% of the last 10 s"))
            ),
            (_, (waited, n), _) if n > 0 && covers(waited) => format!(
                "it was waiting on the disk for {} (budget {budget:.0} s): up to {n} of its threads at a time waited in the kernel uninterruptibly (a file's sync, a read){} (its main thread ran {main:.1} s of CPU){beside}",
                whole_stretch(),
                pressure("io").map_or(String::new(), |p| format!("; the box's I/O pressure: tasks waited on the disk {p:.1}% of the last 10 s"))
            ),
            (_, _, (starved, queued)) if starved > Duration::ZERO => {
                let classes: Vec<String> = ["Interactive", "Visible", "Background"].iter().zip(queued).filter(|(_, n)| *n > 0).map(|(class, n)| format!("{n} jobs queued at {class}")).collect();
                let slots = match self.slots.is_empty() {
                    true => "; no slot of the process's CPU budget was held at the last look".to_string(),
                    false => format!("; the process's CPU budget slots at the last look: {}", self.slots.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")),
                };
                format!(
                    "its pool was starved: {}, none granted for {:.1} s of the {seconds:.1} s it ran nothing{waited_on} (budget {budget:.0} s; its main thread ran {main:.1} s of CPU){slots}{beside}",
                    classes.join(", "),
                    starved.as_secs_f64()
                )
            }
            _ if pending => format!(
                "it ran nothing for {seconds:.1} s{waited_on} (budget {budget:.0} s): nothing it ran brought that on{} (its main thread ran {main:.1} s of CPU)",
                self.stall_clauses(seconds)
            ),
            _ => format!(
                "it sat idle for {seconds:.1} s (budget {budget:.0} s), nothing running and nothing pending{}{}{}{} (its main thread ran {main:.1} s of CPU){beside}",
                if running > 0 { format!(" ({running} pool jobs on its workers ran no CPU)") } else { String::new() },
                match self.ticked {
                    (Duration::ZERO, _) => String::new(),
                    (ticked, cpu) => format!("; for {:.1} s of it only a steady tick ran, {:.0}% of a core, its main thread nothing (no progress)", ticked.as_secs_f64(), 100.0 * cpu.as_secs_f64() / ticked.as_secs_f64()),
                },
                match self.beside_ticked {
                    (Duration::ZERO, _) => String::new(),
                    (ticked, cpu) => format!("; for {:.1} s of it the test process's own threads ticked {:.0}% of a core (no progress)", ticked.as_secs_f64(), 100.0 * cpu.as_secs_f64() / ticked.as_secs_f64()),
                },
                self.stall_clauses(seconds)
            ),
        }))
    }

    /// Stalls that accounted for only part of the idle or pending stretch.
    fn stall_clauses(&self, seconds: f64) -> String {
        format!("{}{}{}{}{}",
            match self.granted {
                Duration::ZERO => String::new(),
                granted => format!("; for {:.1} s of it its pool's jobs waited for a CPU permit, granted since", granted.as_secs_f64()),
            },
            match self.paged {
                (_, 0) => String::new(),
                (waited, n) => format!("; for {:.1} s of it it waited on memory ({n} major page faults), {}", waited.as_secs_f64(), if self.memory.1 == 0 { "none since" } else { "which stopped" }),
            },
            match self.memory {
                (_, 0) => String::new(),
                (waited, n) => format!("; for the last {:.1} s of it it took {n} major page faults ({:.0} a second), which began after it had run nothing for {:.1} s", waited.as_secs_f64(), n as f64 / waited.as_secs_f64(), seconds - waited.as_secs_f64()),
            },
            match self.disked {
                (_, 0) => String::new(),
                (waited, n) => format!("; for {:.1} s of it up to {n} of its threads waited on the disk, {}", waited.as_secs_f64(), if self.disk.1 == 0 { "none since" } else { "which ended" }),
            },
            match self.disk {
                (_, 0) => String::new(),
                (waited, n) => format!("; for the last {:.1} s of it up to {n} of its threads waited on the disk, which began after it had run nothing for {:.1} s", waited.as_secs_f64(), seconds - waited.as_secs_f64()),
            }
        )
    }
}

/// The box's pressure on `resource` (`memory`, `io`) where the OS says it (Linux PSI): the share of
/// the last 10 s in which some task waited on it, in percent.
fn pressure(resource: &str) -> Option<f64> {
    let text = fs::read_to_string(format!("/proc/pressure/{resource}")).ok()?;
    let some = text.lines().find(|line| line.starts_with("some "))?;
    some.split_whitespace().find_map(|field| field.strip_prefix("avg10="))?.parse().ok()
}

fn wait_looks_with<T>(
    what: &str,
    mut ask: impl FnMut() -> TestResult<T>,
    pending: impl Fn(&str) -> bool,
    mut spent: impl FnMut() -> TestResult<Option<String>>,
) -> TestResult<T> {
    loop {
        match ask() {
            Err(err) if pending(err.message()) => {
                if let Some(reason) = spent()? {
                    return Err(TestError::new(format!("app remote {what}: {reason}")));
                }
            }
            done => return done,
        }
    }
}

fn is_no_tree_yet(message: &str) -> bool {
    message.contains(makepad_hal::a11y::remote::NO_TREE_YET)
}

/// The number of an action the app has not taken yet, from its `action N timed out` answer.
fn pending_action(message: &str) -> Option<u64> {
    let at = message.find(" timed out")?;
    message[..at].rsplit_once("action ")?.1.parse().ok()
}

/// What the threads driving tests have run, each to its last [`drove`].
static DRIVEN_NS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The calling thread's CPU at its last [`drove`].
    static DROVE_AT: Cell<u64> = const { Cell::new(0) };
}

/// Counts what the calling thread has run since it last did (since it started, the first time) as
/// driving a test: a test's own work, which no app waits for.
fn drove() {
    let now = makepad_hal::clock::thread_cpu_ns();
    DRIVEN_NS.fetch_add(now.saturating_sub(DROVE_AT.with(|at| at.replace(now))), Ordering::Relaxed);
}

/// The OS ids of the threads driving tests now ([`Drove`]): what they wait for a core is no service
/// the test process runs beside them ([`beside_now`]).
static DRIVING_TIDS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// A test's thread drives it while this lives ([`DRIVING_TIDS`]), and the rest of what it ran
/// counts as driving once the test ends.
struct Drove {
    tid: u64,
}

impl Drove {
    fn enter() -> Drove {
        let tid = makepad_hal::clock::thread_id();
        if tid != 0 {
            DRIVING_TIDS.lock().unwrap_or_else(|e| e.into_inner()).push(tid);
        }
        Drove { tid }
    }
}

impl Drop for Drove {
    fn drop(&mut self) {
        drove();
        if self.tid != 0 {
            let mut tids = DRIVING_TIDS.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(i) = tids.iter().position(|&t| t == self.tid) {
                tids.swap_remove(i);
            }
        }
    }
}

/// What the test process has run and waited for beside the threads driving its tests
/// ([`beside_now`]).
#[derive(Clone, Copy, Debug, Default)]
struct Beside {
    cpu: Duration,
    waited: Duration,
}

/// The CPU the test process has run beside the threads driving its tests: what a service the test
/// runs in its own process runs (an AI hub on loopback the app asks, its GPU device's thread
/// compiling its programs), less what the runtime's own threads ran (under makepad run, the JIT's
/// checks and tier-ups of the test), what serving remote peers cost it, and what its connections to
/// the app ran carrying the test's asks. Tests run side by side (`concurrent`) drive from threads of
/// their own, and each counts what it runs as it waits ([`drove`]): one test's looks are no progress
/// of another's app. (Three Fab tests side by side, their apps idle, each a failure: one test's
/// looks every millisecond read as its hub at work for the other two, whose 20 looks a second did
/// the same for it, and the three waited 1h20m, L1249194, 11-17.) And what its threads but those
/// (by id), the remote's own and the runtime's spent runnable, waiting for a core: a service that
/// wants a core a loaded box does not give it is at work all the same.
fn beside_now() -> Beside {
    drove();
    let others = makepad_hal::clock::process_cpu_ns().saturating_sub(DRIVEN_NS.load(Ordering::Relaxed));
    let cpu = Duration::from_nanos(others.saturating_sub(makepad_remote::runtime_cpu_ns()).saturating_sub(makepad_remote::served_cpu_ns()).saturating_sub(makepad_remote::asked_cpu_ns()));
    let driving = DRIVING_TIDS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let waited = makepad_hal::clock::threads_waited_ns_filtered(|tid, name| makepad_remote::own_thread(name) || driving.contains(&tid));
    Beside { cpu, waited: Duration::from_nanos(waited.saturating_sub(makepad_remote::runtime_waited_ns())) }
}

/// A failed test of a JIT app names the app's state at the failure (first, before the
/// screenshots): a wait that timed out says what the app was doing, without a rerun.
fn with_jit_capture(app: &TestApp, err: TestError) -> TestError {
    let first = err.message().lines().next().unwrap_or("");
    let why: String = format!("test failed: {first}").chars().take(200).collect();
    match app.client().jit_capture(&why) {
        Some(state) => TestError::new(format!("{}\n  JIT state at the failure: {state}", err.message())),
        None => err,
    }
}

fn capture_failure_artifacts(app: &TestApp, failure_message: &str) {
    let artifact_dir = app.artifacts_dir();
    let _ = fs::create_dir_all(&artifact_dir);
    let _ = fs::write(artifact_dir.join("failure.txt"), failure_message);

    match app.try_screenshot() {
        Ok(path) => {
            if let Err(err) = fs::copy(&path, artifact_dir.join("failure-screenshot.png")) {
                let _ = fs::write(
                    artifact_dir.join("failure-screenshot-error.txt"),
                    err.to_string(),
                );
            }
        }
        Err(err) => {
            let _ = fs::write(
                artifact_dir.join("failure-screenshot-error.txt"),
                err.message(),
            );
        }
    }

    match app.try_widget_dump() {
        Ok(dump) => {
            let _ = fs::write(artifact_dir.join("widget-tree.txt"), dump);
        }
        Err(err) => {
            let _ = fs::write(artifact_dir.join("widget-tree-error.txt"), err.message());
        }
    }

    match app.client().get_text("ui", &[("json", "1")]) {
        Ok(snapshot) => {
            let _ = fs::write(
                artifact_dir.join("widget-snapshot.json"),
                snapshot,
            );
        }
        Err(err) => {
            let _ = fs::write(
                artifact_dir.join("widget-snapshot-error.txt"),
                err.message(),
            );
        }
    }

    match app.collect_logs_text() {
        Ok(logs) => {
            let _ = fs::write(artifact_dir.join("logs.txt"), logs);
        }
        Err(err) => {
            let _ = fs::write(artifact_dir.join("logs-error.txt"), err.message());
        }
    }
}

fn panic_for_error(err: TestError) -> ! {
    panic!("{}", err.message())
}

fn studio_msg_name(msg: &StudioToApp) -> &'static str {
    match msg {
        StudioToApp::Screenshot(_) => "Screenshot",
        StudioToApp::RunViewFrameRequest(_) => "RunViewFrameRequest",
        StudioToApp::A11yState { .. } => "A11yState",
        StudioToApp::A11yAction { .. } => "A11yAction",
        StudioToApp::A11yRead { .. } => "A11yRead",
        StudioToApp::A11yLookup { .. } => "A11yLookup",
        StudioToApp::A11yType { .. } => "A11yType",
        StudioToApp::KeepAlive => "KeepAlive",
        StudioToApp::LiveChange { .. } => "LiveChange",
        StudioToApp::Swapchain(_) => "Swapchain",
        StudioToApp::WindowGeomChange { .. } => "WindowGeomChange",
        StudioToApp::WindowFocus { .. } => "WindowFocus",
        StudioToApp::Tick => "Tick",
        StudioToApp::MouseDown(_) => "MouseDown",
        StudioToApp::MouseUp(_) => "MouseUp",
        StudioToApp::MouseCancel(_) => "MouseCancel",
        StudioToApp::MouseMove(_) => "MouseMove",
        StudioToApp::TweakRay(_) => "TweakRay",
        StudioToApp::KeyDown(_) => "KeyDown",
        StudioToApp::KeyUp(_) => "KeyUp",
        StudioToApp::TextInput(_) => "TextInput",
        StudioToApp::NativeApp(_) => "NativeApp",
        StudioToApp::TextCopy => "TextCopy",
        StudioToApp::TextCut => "TextCut",
        StudioToApp::DropFiles(_) => "DropFiles",
        StudioToApp::Scroll(_) => "Scroll",
        StudioToApp::Pinch(_) => "Pinch",
        StudioToApp::GameInput(_) => "GameInput",
        StudioToApp::Custom(_) => "Custom",
        StudioToApp::None => "None",
        StudioToApp::Kill => "Kill",
        StudioToApp::Relay(_) => "Relay",
    }
}

/// The remote takes a button index (`b=0` left); protocol events carry the
/// raw bit set, where the primary button is bit 0.
fn button_index(raw_bits: u32) -> u32 {
    if raw_bits == 0 {
        return 0;
    }
    raw_bits.trailing_zeros()
}

fn sanitize_path_component(value: &str) -> String {
    let mut sanitized = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => sanitized.push(ch),
            ':' | '/' | '\\' | ' ' => sanitized.push('_'),
            _ => sanitized.push('_'),
        }
    }
    sanitized.trim_matches('_').to_string()
}

fn snapshot_is_visible(widget: &NodeMatch) -> bool { widget.visible() }
fn snapshot_center_f64(widget: &NodeMatch) -> (f64, f64) {
    let (x,y) = widget.bounds.center(); (x as f64, y as f64)
}
fn snapshot_summary(widget: &NodeMatch) -> String {
    format!("w={} {}", widget.window_arg(), makepad_a11y::render::node_line(&widget.node))
}

fn parallel_tests_enabled() -> bool { env_truthy("MAKEPAD_TEST_PARALLEL") }

fn keep_artifacts_on_pass_enabled() -> bool { env_truthy("MAKEPAD_TEST_KEEP_ARTIFACTS") }

fn visible_mode_enabled() -> bool {
    env_truthy("MAKEPAD_TEST_VISIBLE")
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn env_duration_ms(name: &str) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::ZERO)
}


/// The lines a read after line `since` lost that no earlier read after `since` was given, as
/// (the line they follow, how many): the read lost `dropped` lines, since+1 ..= since+dropped,
/// and an earlier read from the same cursor was given every line up to `seen`.
fn unseen_gap(since: u64, dropped: u64, seen: Option<u64>) -> Option<(u64, u64)> {
    let after = seen.unwrap_or(since).max(since);
    let unseen = since.saturating_add(dropped).saturating_sub(after);
    (unseen > 0).then_some((after, unseen))
}

#[cfg(test)]
mod tests {
    use super::{
        button_index, env_duration_ms, sanitize_path_component,
        snapshot_is_visible, unseen_gap, visible_mode_enabled, Beside, Budget, NodeMatch,
    };
    use crate::remote::{AppCpu, AppSettled, BudgetSlot};
    use std::time::Instant;

    #[test]
    fn query_widgets_waits_while_the_app_has_no_tree() {
        use super::{is_no_tree_yet, wait_looks_with, TestError};
        use makepad_hal::a11y::remote::NO_TREE_YET;

        let what = "ui find: the app built no tree";
        let mut calls = 0;
        let result = wait_looks_with(what, || {
            calls += 1;
            if calls <= 2 { Err(TestError::new(NO_TREE_YET)) } else { Ok(7) }
        }, is_no_tree_yet, || Ok(None));
        assert_eq!(result.unwrap(), 7);
        assert_eq!(calls, 3, "two pending looks before the query succeeds");

        let mut calls = 0;
        let failed = wait_looks_with::<()>(what, || {
            calls += 1;
            Err(TestError::new("invalid query"))
        }, is_no_tree_yet, || panic!("a query error must bypass the budget"));
        assert_eq!(failed.unwrap_err().message(), "invalid query");
        assert_eq!(calls, 1);

        let idle = wait_looks_with::<()>(what, || Err(TestError::new(NO_TREE_YET)), is_no_tree_yet,
            || Ok(Some("it sat idle for 10.0 s".into())));
        assert_eq!(idle.unwrap_err().message(), "app remote ui find: the app built no tree: it sat idle for 10.0 s");
    }

    /// A wait for what never comes fails on the idle rule while the app's other thread ticks a
    /// steady fifth of a core, its main thread running nothing and nothing pending (a Fab sim
    /// beside a wait for a button that never came ran 19 min, L798418, 11-10). A thread at work, at
    /// half a core or more, keeps the wait going, and so does a main thread that runs, and a thread
    /// a loaded box starves: one that runs a fifth of a core and waits for one the rest wants a
    /// whole one (J11427, 11-11). An app with nothing running fails as soon on a loaded box.
    /// The test process's own threads beside its driving ones (a service it runs, as a hub) are
    /// held to the same rule: a steady tick is no progress, and the verdict names it (an
    /// in-process hub's idle worker at 5-9% of a core kept a wait for a panel line going 40 min
    /// beside an app at 0%, `grow_live`, 11-27), while one at work, or starved of a core it wants,
    /// keeps the wait going.
    #[test]
    fn a_steady_tick_is_no_progress_and_work_is() {
        let secs = Duration::from_secs_f64;
        // its other threads' CPU, its main thread's and what they waited for a core, and the test
        // process's own beside them and what those waited, a second
        let wait_beside = |other: f64, main: f64, waited: f64, beside: f64, beside_waited: f64| -> Option<(u64, String)> {
            let (t0, cpu) = (Instant::now(), |m: f64, own: f64, waited: f64| AppCpu { main: secs(m), own: secs(own), faults: 0, waited: secs(waited), io_threads: 0 });
            let mut budget = Budget::at(secs(300.0), secs(10.0), cpu(0.0, 0.0, 0.0), t0, Beside::default());
            (1..=60u64).find_map(|look| {
                let n = look as f64;
                let now = t0 + Duration::from_secs(look);
                let mut settled = || -> crate::TestResult<AppSettled> { Ok(AppSettled { settled: true, only_pool: false, pool: None, slots: Vec::new(), pending: Vec::new() }) };
                let mut on_jit = || -> crate::TestResult<bool> { Ok(false) };
                let beside = Beside { cpu: secs(beside * n), waited: secs(beside_waited * n) };
                budget.judge(cpu(main * n, (main + other) * n, waited * n), now, beside, &mut settled, &mut on_jit).unwrap().map(|why| (look, why))
            })
        };
        let wait = |other: f64, main: f64, waited: f64| wait_beside(other, main, waited, 0.0, 0.0);
        let (look, why) = wait(0.2, 0.0, 0.0).expect("a steady tick ends the wait");
        assert_eq!(look, 10, "after the idle budget: {why}");
        assert!(why.starts_with("it sat idle for 10.0 s") && why.contains("only a steady tick ran, 20% of a core"), "{why}");
        assert!(wait(0.6, 0.0, 0.0).is_none(), "a thread at 60% of a core is at work");
        assert!(wait(0.2, 0.1, 0.0).is_none(), "a main thread that runs is progress");
        assert!(wait(0.2, 0.0, 0.8).is_none(), "a worker starved of a core it wants is at work");
        assert_eq!(wait(0.2, 0.0, 0.1).map(|(look, _)| look), Some(10), "a tick that waits a little on a loaded box is still a tick");
        // the test process's own threads beside the app
        let (look, why) = wait_beside(0.0, 0.0, 0.0, 0.09, 0.0).expect("a steady tick beside the test ends the wait");
        assert!(look == 10 && why.starts_with("it sat idle for 10.0 s") && why.contains("for 10.0 s of it the test process's own threads ticked 9% of a core (no progress)"), "{look}: {why}");
        assert!(wait_beside(0.0, 0.0, 0.0, 0.6, 0.0).is_none(), "the test process's own threads at 60% of a core are at work (a hub compiling its GPU programs)");
        assert!(wait_beside(0.0, 0.0, 0.0, 0.2, 0.8).is_none(), "the test process's own threads starved of a core they want are at work");
        let (look, why) = wait_beside(0.0, 0.0, 0.0, 0.09, 0.1).expect("a tick beside the test that waits a little is still a tick");
        assert!(look == 10 && why.contains("ticked 9% of a core"), "{look}: {why}");
        let (look, why) = wait_beside(0.0, 0.0, 0.0, 0.0, 0.08).expect("nothing beside the test but a little run delay is quiet");
        assert!(look == 10 && !why.contains("tick"), "{look}: {why}");
        // at work 5 s (a hub compiling), then ticking: the idle stretch counts from the tick, and
        // the verdict says both
        let t0 = Instant::now();
        let mut budget = Budget::at(secs(300.0), secs(10.0), AppCpu { main: Duration::ZERO, own: Duration::ZERO, faults: 0, waited: Duration::ZERO, io_threads: 0 }, t0, Beside::default());
        let (look, why) = (1..=60u64).find_map(|look| {
            let ran = if look <= 5 { 0.8 * look as f64 } else { 4.0 + 0.09 * (look - 5) as f64 };
            let quiet = AppCpu { main: Duration::ZERO, own: Duration::ZERO, faults: 0, waited: Duration::ZERO, io_threads: 0 };
            let mut settled = || -> crate::TestResult<AppSettled> { Ok(AppSettled { settled: true, only_pool: false, pool: None, slots: Vec::new(), pending: Vec::new() }) };
            budget.judge(quiet, t0 + Duration::from_secs(look), Beside { cpu: secs(ran), waited: Duration::ZERO }, &mut settled, &mut || Ok(false)).unwrap().map(|why| (look, why))
        }).expect("a tick after work ends the wait");
        assert_eq!(look, 15, "the idle limit after the work ended: {why}");
        assert!(why.contains("for 10.0 s of it the test process's own threads ticked 9% of a core (no progress)"), "{why}");
        assert!(why.contains("before that the test process's own threads (a service the test runs in it, as a hub) were busy for 5.0 s, which the wait counted as progress"), "{why}");
        let (look, why) = wait(0.0, 0.0, 0.02).expect("an app with nothing running is idle on a loaded box");
        assert!(look == 10 && why.starts_with("it sat idle for 10.0 s") && !why.contains("tick"), "{look}: {why}");
    }

    /// A wait beside an app that runs nothing while it says a load is on its way (a load the test
    /// holds) fails once that lasted the wait's budget, naming the load: a wait with a budget ends.
    /// One whose main thread works meanwhile is held to its CPU budget instead. (A Fab test's wait
    /// sat 17 min beside a held load, its app at 0%, 11-20.) A pending wait names the stall that
    /// lasted, or says how long a shorter stall lasted, counting only looks with work pending.
    /// (The store test's startup thread spent the whole wait in a file sync on a loaded box and
    /// the verdict read "it ran nothing" for 40.2 s, 11-24.)
    #[test]
    fn a_pending_app_that_runs_nothing_fails_within_the_budget() {
        let secs = Duration::from_secs_f64;
        let cpu = |main: f64| AppCpu { main: secs(main), own: secs(main), faults: 0, waited: Duration::ZERO, io_threads: 0 };
        let quiet = |_| cpu(0.0);
        let load = || AppSettled {
            settled: false, only_pool: false, pool: None, slots: Vec::new(),
            pending: vec!["loads (build: 1)".into()],
        };
        let pending = |_| load();
        // and the test process's own threads beside the app, a second
        let wait_beside = |cpu_at: &dyn Fn(u64) -> AppCpu, settled_at: &dyn Fn(u64) -> AppSettled, beside: f64| -> Option<(u64, String)> {
            let t0 = Instant::now();
            let mut budget = Budget::at(secs(30.0), secs(10.0), cpu(0.0), t0, Beside::default());
            (1..=120u64).find_map(|look| {
                let now = t0 + Duration::from_secs(look);
                budget.judge(cpu_at(look), now, Beside { cpu: secs(beside * look as f64), waited: Duration::ZERO }, &mut || Ok(settled_at(look)), &mut || Ok(false))
                    .unwrap().map(|why| (look, why))
            })
        };
        let wait = |cpu_at: &dyn Fn(u64) -> AppCpu, settled_at: &dyn Fn(u64) -> AppSettled| wait_beside(cpu_at, settled_at, 0.0);
        let (look, why) = wait(&quiet, &pending).expect("a load that never comes ends the wait");
        assert_eq!(look, 30, "the wait ends after its budget, not at the idle limit: {why}");
        assert!(why.starts_with("it ran nothing for 30.0 s while it said it waited on loads (build: 1) (budget 30 s)"), "the verdict names what was pending: {why}");
        let (look, why) = wait_beside(&quiet, &pending, 0.09).expect("a load that never comes ends the wait beside a tick in the test process");
        assert!(look == 30 && why.starts_with("it ran nothing for 30.0 s while it said it waited on loads (build: 1) (budget 30 s)"), "a tick beside the test is no progress of the load: {look}: {why}");
        let (look, why) = wait(&|n| cpu(0.5 * n as f64), &pending).expect("a main thread at work spends the CPU budget");
        assert!(look == 60 && why.starts_with("its main thread ran 30.0 s"), "a working app is held to its CPU budget: {look}: {why}");

        let disk = |waiting: bool| AppCpu { io_threads: u32::from(waiting), ..cpu(0.0) };
        let (look, why) = wait(&|_| disk(true), &pending).expect("a load waiting on the disk spends the budget");
        assert_eq!(look, 30, "a disk wait ends at the same budget: {why}");
        assert!(why.starts_with("it was waiting on the disk for 30.0 s while it said it waited on loads (build: 1) (budget 30 s): up to 1 of its threads at a time waited in the kernel uninterruptibly"), "{why}");
        let (look, why) = wait(&|n| disk(n > 25), &pending).unwrap();
        assert!(look == 30 && why.starts_with("it ran nothing for 30.0 s while it said it waited on loads (build: 1)"), "{look}: {why}");
        assert!(why.contains("for the last 5.0 s of it up to 1 of its threads waited on the disk, which began after it had run nothing for 25.0 s"), "a late disk wait explains only part of the stretch: {why}");

        let (look, why) = wait(&|n| AppCpu { faults: 100 * n, ..cpu(0.0) }, &pending).unwrap();
        assert!(look == 30 && why.starts_with("it was waiting on memory (swapping) for 30.0 s while it said it waited on loads (build: 1) (budget 30 s): 3000 major page faults, 100 a second"), "{look}: {why}");
        let queued_load = |taken| {
            let mut state = load();
            state.pool = Some(([0, 2, 0], 0, taken));
            state.pending.push("pool".into());
            state
        };
        let (look, why) = wait(&quiet, &|_| queued_load(0)).unwrap();
        assert!(look == 30 && why.starts_with("its pool was starved: 2 jobs queued at Visible, none granted for 29.0 s of the 30.0 s it ran nothing while it said it waited on loads (build: 1), pool"), "{look}: {why}");
        assert!(why.contains("no slot of the process's CPU budget was held at the last look"), "{why}");

        let (look, why) = wait(
            &|n| AppCpu { faults: 100 * n.min(4), ..disk(n <= 6) },
            &|n| queued_load(u64::from(n >= 30)),
        ).unwrap();
        assert!(look == 30 && why.starts_with("it ran nothing for 30.0 s while it said it waited on loads (build: 1), pool"), "a job taken ends the starve even with jobs still queued: {look}: {why}");
        assert!(why.contains("for 28.0 s of it its pool's jobs waited for a CPU permit, granted since"), "{why}");
        assert!(why.contains("for 4.0 s of it it waited on memory (400 major page faults), none since"), "{why}");
        assert!(why.contains("for 6.0 s of it up to 1 of its threads waited on the disk, none since"), "{why}");

        let after_idle = |n| if n <= 5 {
            AppSettled { settled: true, pending: Vec::new(), ..load() }
        } else {
            load()
        };
        let (look, why) = wait(&|n| disk(n <= 5), &after_idle).unwrap();
        assert!(look == 35 && why.starts_with("it ran nothing for 30.0 s while it said it waited on loads (build: 1)") && !why.contains("disk"), "the preceding idle stretch's disk wait belongs to that stretch: {look}: {why}");
    }

    /// A wait whose app sits idle names a stall (a starved pool, a memory wait) only while it lasts
    /// to the wait's end; one that passed is a clause of the idle verdict. A starve counts once a
    /// look finds the jobs the look before found queued, none taken: a job just queued has waited
    /// for nothing yet. (A counter's wait on a loaded box read as a starved pool for one look's
    /// queued font scan, and as "waiting on memory (swapping)" for one look's 89 page faults with
    /// no memory pressure on the box, of 11 s it sat idle, 11-12.) A starve names who held the CPU
    /// budget's slots at its last look, a JIT build among them (strength_here's 19 Background jobs
    /// none granted for 10.1 s told no more, 11-13). A memory or disk wait is the verdict only when
    /// every look of the idle stretch found it: one that began after the app sat idle is a clause
    /// (80 page faults in the last look of 10.3 s read as "waiting on memory", 11-22).
    #[test]
    fn a_stall_is_the_verdict_only_while_it_lasts() {
        // the app runs nothing; `look(n)` is its Background jobs queued then, taken so far, its
        // major page faults so far, and its threads waiting on the disk then
        let wait = |look: &dyn Fn(u64) -> (u64, u64, u64, u32)| -> (u64, String) {
            let (t0, cpu) = (Instant::now(), |faults: u64, io_threads: u32| AppCpu { main: Duration::ZERO, own: Duration::ZERO, faults, waited: Duration::ZERO, io_threads });
            let mut budget = Budget::at(Duration::from_secs(300), Duration::from_secs(10), cpu(0, 0), t0, Beside::default());
            (1..=60u64).find_map(|n| {
                let (queued, taken, faults, io_threads) = look(n);
                // while jobs wait, a JIT build has held its slot since the wait began
                let slots = || match queued {
                    0 => Vec::new(),
                    _ => vec![BudgetSlot { class: "background".into(), client: "jit".into(), label: "another compile job".into(), lent: false, n: 1, held: Duration::from_secs(n) }],
                };
                let mut settled = || -> crate::TestResult<AppSettled> { Ok(AppSettled { settled: queued == 0, only_pool: queued > 0, pool: Some(([0, 0, queued], 0, taken)), slots: slots(), pending: Vec::new() }) };
                let mut on_jit = || -> crate::TestResult<bool> { Ok(false) };
                budget.judge(cpu(faults, io_threads), t0 + Duration::from_secs(n), Beside::default(), &mut settled, &mut on_jit).unwrap().map(|why| (n, why))
            }).expect("the wait ends")
        };
        let (n, why) = wait(&|n| if n == 1 { (1, 0, 0, 0) } else { (0, 1, 0, 0) });
        assert!(n == 10 && why.starts_with("it sat idle for 10.0 s") && !why.contains("permit"), "a job seen queued once was not starved: {n}: {why}");
        let (n, why) = wait(&|n| if n <= 3 { (1, 0, 0, 0) } else { (0, 1, 0, 0) });
        assert!(n == 10 && why.starts_with("it sat idle for 10.0 s") && why.contains("for 2.0 s of it its pool's jobs waited for a CPU permit, granted since"), "a starve that ended is not the verdict: {n}: {why}");
        let (n, why) = wait(&|_| (1, 0, 0, 0));
        assert!(n == 10 && why.starts_with("its pool was starved: 1 jobs queued at Background, none granted for 9.0 s of the 10.0 s"), "a starve to the end is: {n}: {why}");
        assert!(why.contains("; the process's CPU budget slots at the last look: 1x background jit \"another compile job\" held 10.0 s"), "a starve names who held the slots: {why}");
        let (n, why) = wait(&|n| (0, 0, if n >= 2 { 89 } else { 0 }, 0));
        assert!(n == 10 && why.starts_with("it sat idle for 10.0 s") && why.contains("for 1.0 s of it it waited on memory (89 major page faults), none since"), "a burst of page faults that stopped is not the verdict: {n}: {why}");
        let (n, why) = wait(&|n| (0, 0, 100 * n, 0));
        assert!(n == 10 && why.starts_with("it was waiting on memory (swapping) for 10.0 of the 10.0 s"), "a memory wait to the end is: {n}: {why}");
        let (n, why) = wait(&|n| (0, 0, if n >= 10 { 80 } else { 0 }, 0));
        assert!(n == 10 && why.starts_with("it sat idle for 10.0 s") && why.contains("; for the last 1.0 s of it it took 80 major page faults (80 a second), which began after it had run nothing for 9.0 s"), "page faults that began after the app sat idle are not the verdict: {n}: {why}");
        let (n, why) = wait(&|_| (0, 0, 0, 2));
        assert!(n == 10 && why.starts_with("it was waiting on the disk for 10.0 of the 10.0 s") && why.contains("up to 2 of its threads at a time waited in the kernel uninterruptibly"), "a disk wait to the end is: {n}: {why}");
        let (n, why) = wait(&|n| (0, 0, 0, u32::from(n <= 3)));
        assert!(n == 10 && why.starts_with("it sat idle for 10.0 s") && why.contains("; for 3.0 s of it up to 1 of its threads waited on the disk, none since"), "a disk wait that ended is not the verdict: {n}: {why}");
        let (n, why) = wait(&|n| (0, 0, 0, u32::from(n >= 9)));
        assert!(n == 10 && why.starts_with("it sat idle for 10.0 s") && why.contains("; for the last 2.0 s of it up to 1 of its threads waited on the disk, which began after it had run nothing for 8.0 s"), "a disk wait that began after the app sat idle is not: {n}: {why}");
    }
    use crate::{Selector, TestConfig};
    use makepad_studio_protocol::MouseButton;
    use makepad_a11y::{Node, Role, Rect};
    use std::path::Path;
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    static ENV_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

    fn restore_env_var(name: &str, value: Option<std::ffi::OsString>) {
        if let Some(value) = value {
            std::env::set_var(name, value);
        } else {
            std::env::remove_var(name);
        }
    }

    fn snapshot(id: &str) -> NodeMatch {
        NodeMatch { window: 0, node: Node { path: id.into(), name: id.into(), role: Role::Button, bounds: Rect { x:10.0, y:20.0, w:30.0, h:40.0 }, ..Default::default() } }
    }
    #[test]
    fn a_log_read_fails_only_on_lines_no_read_from_its_cursor_saw() {
        assert_eq!(unseen_gap(100, 0, None), None, "nothing dropped");
        assert_eq!(unseen_gap(100, 40, None), Some((100, 40)), "a first read from 100 lost 101..=140");
        assert_eq!(unseen_gap(100, 40, Some(150)), None, "a poll from 100 was given up to 150 before they went");
        assert_eq!(unseen_gap(100, 40, Some(120)), Some((120, 20)), "121..=140 went between two polls");
        assert_eq!(unseen_gap(u64::MAX, 0, None), None, "a cursor past the end asks for nothing");
    }

    #[test]
    fn semantic_node_bounds_locate_widget_centres() {
        let node = snapshot("input_singleline");
        assert_eq!(node.path, "input_singleline");
        assert_eq!(node.bounds.center(), (25.0,40.0));
    }
    #[test]
    fn semantic_queries_locate_dock_parts() {
        let mut node = snapshot("dock.math_tab"); node.node.role = Role::Tab;
        assert!(Selector::role(Role::Tab).query().unwrap().matches(&node));
        assert!(Selector::path("math_tab").query().unwrap().matches(&node));
    }

    #[test]
    fn config_uses_expected_artifact_dir_and_hidden_default() {
        let _guard = ENV_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
        let scratch = std::env::temp_dir().join(format!("makepad-test-config-{}-{}", std::process::id(), stamp.as_nanos()));
        std::fs::create_dir(&scratch).unwrap();
        let workspace = scratch.join("root");
        let pkg = workspace.join("pkg");
        let nested = pkg.join("nested");
        let first_tree = scratch.join("tree a/pkg");
        let second_tree = scratch.join("tree_a/pkg");
        for dir in [&pkg, &nested, &first_tree, &second_tree] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"makepad-example\"\n").unwrap();
        }
        std::fs::write(workspace.join("Cargo.toml"), "  [workspace]  \n").unwrap();

        let saved = ["MAKEPAD_TEST_VISIBLE", "MAKEPAD_TEST_ARTIFACTS", "CARGO_TARGET_DIR"]
            .map(|name| (name, std::env::var_os(name)));
        let config = |dir: &Path| TestConfig::current_package(dir, "makepad-example", "ui::test");
        let cwd = std::env::current_dir().unwrap();
        std::env::remove_var("MAKEPAD_TEST_VISIBLE");
        std::env::set_var("CARGO_TARGET_DIR", "custom-target");
        std::env::set_var("MAKEPAD_TEST_ARTIFACTS", "lease-artifacts");
        let lease = config(&pkg);
        std::env::set_var("MAKEPAD_TEST_ARTIFACTS", "");
        let cargo = config(&pkg);
        std::env::remove_var("MAKEPAD_TEST_ARTIFACTS");
        std::env::remove_var("CARGO_TARGET_DIR");
        let member = config(&pkg);
        let nested_member = config(&nested);
        std::env::set_var("CARGO_TARGET_DIR", "");
        let first = config(&first_tree);
        let second = config(&second_tree);
        for (name, value) in saved {
            restore_env_var(name, value);
        }
        let package_target_exists = pkg.join("target").exists();
        std::fs::remove_dir_all(&scratch).unwrap();

        let suffix = Path::new("makepad-example/ui__test");
        assert_eq!(lease.unwrap().artifacts_dir, Path::new("lease-artifacts").join(suffix),
            "the lease root takes precedence and stays as supplied");
        assert_eq!(cargo.unwrap().artifacts_dir, cwd.join("custom-target/makepad_test").join(suffix),
            "a relative Cargo target starts at the current directory");
        let expected = workspace.join("target/makepad_test").join(suffix);
        let member = member.unwrap();
        assert_eq!(member.artifacts_dir, expected, "a workspace member uses the workspace target");
        assert!(!package_target_exists, "the member has no package-local target directory");
        assert_eq!(nested_member.unwrap().artifacts_dir, expected, "an intervening package is skipped");

        let first = first.unwrap().artifacts_dir;
        let second = second.unwrap().artifacts_dir;
        for path in [&first, &second] {
            let relative = path.strip_prefix(std::env::temp_dir().join("makepad_test"))
                .expect("a standalone package uses the temporary artifact root");
            assert_eq!(relative.components().count(), 3, "the tree has one directory above the package and test");
            assert!(relative.ends_with(suffix), "the temporary path keeps the package and test names");
        }
        assert_ne!(first, second, "standalone checkouts have separate artifacts and app homes");
        assert!(!member.visible, "the window is hidden by default");
        assert!(!member.env.contains_key("MAKEPAD"), "the default app environment leaves MAKEPAD unset");
        assert!(!member.env.contains_key("CARGO_TARGET_DIR"), "the default app environment inherits the Cargo target");
        assert_eq!(member.requested_screenshot_dpi(), None, "screenshot DPI is unset by default");
    }

    #[test]
    fn keep_artifacts_on_pass_and_shutdown_grab_policy() {
        use super::{should_grab_on_shutdown, shutdown_note, ShutdownOutcome};

        let _guard = ENV_MUTEX.get_or_init(|| Mutex::new(())).lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let name = "MAKEPAD_TEST_KEEP_ARTIFACTS";
        let saved = std::env::var_os(name);
        let config = || TestConfig::new(".", "makepad-example", "shutdown");
        std::env::remove_var(name);
        let default = config();
        std::env::set_var(name, "1");
        let keeping = config();
        restore_env_var(name, saved);

        assert!(!default.unwrap().keep_artifacts_on_pass);
        assert!(keeping.unwrap().keep_artifacts_on_pass);
        assert!(!should_grab_on_shutdown(false, false));
        assert!(should_grab_on_shutdown(false, true));
        assert!(should_grab_on_shutdown(true, false));
        assert!(should_grab_on_shutdown(true, true));
        let pass = shutdown_note(ShutdownOutcome::Graceful, false);
        assert!(pass.contains("/quit") && pass.contains("frame grab skipped") && !pass.contains("/gq"));
        assert!(shutdown_note(ShutdownOutcome::Graceful, true).contains("/gq"));
    }

    #[test]
    fn visible_mode_comes_from_env() {
        let _guard = ENV_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_visible = std::env::var_os("MAKEPAD_TEST_VISIBLE");
        std::env::set_var("MAKEPAD_TEST_VISIBLE", "1");
        assert!(visible_mode_enabled());
        let config =
            TestConfig::current_package("/tmp/example", "makepad-example", "ui::test").unwrap();
        restore_env_var("MAKEPAD_TEST_VISIBLE", old_visible);
        assert!(config.visible);
    }

    #[test]
    fn requested_dpi_env_scales_screenshots() {
        let mut config =
            TestConfig::current_package("/tmp/example", "makepad-example", "ui::test").unwrap();
        config
            .env
            .insert("MAKEPAD_TEST_DPI".to_string(), "1".to_string());
        assert_eq!(config.requested_screenshot_dpi(), Some(1.0));
    }

    #[test]
    fn duration_env_parses_milliseconds() {
        let _guard = ENV_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_value = std::env::var_os("MAKEPAD_TEST_ACTION_DELAY_MS");
        std::env::set_var("MAKEPAD_TEST_ACTION_DELAY_MS", "250");
        assert_eq!(
            env_duration_ms("MAKEPAD_TEST_ACTION_DELAY_MS"),
            Duration::from_millis(250)
        );
        restore_env_var("MAKEPAD_TEST_ACTION_DELAY_MS", old_value);
    }

    #[test]
    fn sanitize_path_component_replaces_separators() {
        assert_eq!(
            sanitize_path_component("ui::captures failure"),
            "ui__captures_failure"
        );
    }

    #[test]
    fn selector_queries_remain_publicly_compatible() {
        assert_eq!(Selector::id("foo").as_query(), "path=\"foo\"");
    }

    #[test]
    fn window_scope_defaults_to_primary_window() {
        assert_eq!(Selector::all().window_arg(), Some("0"));
        assert_eq!(Selector::all().window_index(1).window_arg(), Some("1"));
    }
    #[test]
    fn snapshot_visibility_requires_geometry() {
        let mut widget = snapshot("hidden"); widget.node.bounds.w = 0.0;
        assert!(!snapshot_is_visible(&widget));
    }
    #[test]
    fn semantic_order_keeps_nth_in_tree_order() {
        let selector = Selector::all().nth(1);
        assert_eq!(selector.query().unwrap().nth, Some(1));
    }

    #[test]
    fn button_bits_map_to_remote_indices() {
        assert_eq!(button_index(MouseButton::PRIMARY.bits()), 0);
        assert_eq!(button_index(0b100), 2);
        assert_eq!(button_index(0), 0);
    }
}

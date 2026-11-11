use crate::app_process::{self, LaunchSpec, OwnedApp, ShutdownOutcome};
use crate::error::{IntoTestResult, TestError, TestResult};
use crate::remote::{KeyKind, MouseInput, MouseKind};
use crate::selector::Selector;
use crate::remote::{NodeMatch, UiSnapshot};
use makepad_studio_protocol::{KeyCode, KeyModifiers, StudioToApp};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write;
use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::rc::Rc;
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
const IDLE_SHARE: u32 = 20;
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
        // `MAKEPAD_TEST_ARTIFACTS`: a run's own root (devhub sets one per lease, so two jobs in
        // one tree never share a test's artifacts or app home)
        let root = std::env::var_os("MAKEPAD_TEST_ARTIFACTS")
            .filter(|root| !root.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| manifest_dir.join("target").join("makepad_test"));
        let artifacts_dir = root
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
}

#[derive(Clone)]
pub struct TestApp {
    inner: Rc<RefCell<TestAppInner>>,
}

impl TestApp {
    fn start(mut config: TestConfig) -> TestResult<Self> {
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
            })),
        })
    }

    pub fn locator(&self, selector: Selector) -> Locator {
        Locator {
            app: self.clone(),
            selector,
        }
    }

    /// The owned process id, for diagnostics.
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
        self.ensure_running()?;
        let windows = self.client().windows()?;
        let window = windows
            .first()
            .ok_or_else(|| TestError::new("screenshot: the app has no window"))?;
        let scale = match self.inner.borrow().config.requested_screenshot_dpi() {
            Some(dpi) if window.dpi > 0.0 => dpi / window.dpi,
            _ => 1.0,
        };
        self.client().grab(Some(window.id), scale)
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

    /// Draw and worker diagnostics after a sequence number, without forcing a frame.
    pub fn log_since(&self, since: u64) -> TestResult<crate::LogTail> {
        self.ensure_running()?;
        self.client().log_since(since)
    }

    pub fn wait_for_log_contains(&self, needle: &str) {
        if let Err(err) = self.try_wait_for_log_contains(needle) {
            panic_for_error(err);
        }
    }

    pub fn try_wait_for_log_contains(&self, needle: &str) -> TestResult<()> {
        let mut budget = Budget::start(self, self.action_timeout())?;
        loop {
            self.ensure_running()?;
            let tail = self.client().log_since(0)?;
            if tail.lines.iter().any(|line| line.contains(needle)) {
                return Ok(());
            }
            if budget.look_due() {
                if let Some(spent) = budget.spent(self)? {
                    return Err(TestError::new(format!(
                        "timed out waiting for log containing `{needle}`: {spent}"
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

    pub fn try_wait_until_within(&self, budget: Duration, mut pending: impl FnMut() -> Option<String>) -> TestResult<()> {
        let mut budget = Budget::start(self, budget)?;
        loop {
            self.ensure_running()?;
            let (since, before) = (Instant::now(), self.client().cpu(false)?);
            let Some(what) = pending() else { return Ok(()) };
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
        self.client().find(selector, if visible_only { "state=visible" } else { "" })
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
    fn wait_looks<T>(&self, budget: &mut Option<Budget>, what: &str, mut ask: impl FnMut() -> TestResult<T>, pending: impl Fn(&str) -> bool) -> TestResult<T> {
        loop {
            match ask() {
                Err(err) if pending(err.message()) => {
                    let budget = match budget {
                        Some(budget) => budget,
                        None => budget.insert(Budget::start(self, self.action_timeout())?),
                    };
                    if let Some(spent) = budget.spent(self)? {
                        return Err(TestError::new(format!("app remote {what}: {spent}")));
                    }
                }
                done => return done,
            }
        }
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

    fn shutdown(&self) -> ShutdownOutcome {
        let mut inner = self.inner.borrow_mut();
        let poll = inner.config.poll_interval;
        let outcome = inner.app.shutdown(poll);
        let note = match outcome {
            ShutdownOutcome::Graceful => "graceful: owned process exited after /gq or /quit",
            ShutdownOutcome::AlreadyExited => "process had already exited before shutdown",
            ShutdownOutcome::Killed => "fallback: owned pid killed after /gq and /quit did not end it",
        };
        let _ = fs::write(
            inner.config.artifacts_dir.join("shutdown.txt"),
            format!("pid {}\n{note}\n", inner.app.pid),
        );
        outcome
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
        if let Err(refused) = self.app.client().get_json(&command, &args) {
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

    let app = TestApp::start(config)?;
    let result = catch_unwind(AssertUnwindSafe(|| test(app.clone()).into_test_result()));

    match result {
        Ok(Ok(())) => {
            app.pause_before_shutdown();
            app.shutdown();
            Ok(())
        }
        Ok(Err(err)) => {
            let err = with_jit_capture(&app, err);
            capture_failure_artifacts(&app, err.message());
            app.pause_before_shutdown();
            app.shutdown();
            Err(err)
        }
        Err(payload) => {
            let err = with_jit_capture(&app, TestError::from_panic_payload(payload));
            capture_failure_artifacts(&app, err.message());
            app.pause_before_shutdown();
            app.shutdown();
            Err(err)
        }
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
/// the wait's closure, over the time outside it. Two stalls run no CPU either and are no idle app,
/// and the verdict names them: a look whose process took major page faults at a swapping rate
/// ([`SWAP_FAULTS_PER_S`]) waited on memory (a thread that waits for its page runs no CPU; one fault
/// in 10 s read as "waiting on memory", 11-10), and one with only the app pool's
/// queued jobs pending, none of them taken since the last look, is a starved pool (its jobs wait
/// for a CPU permit the process's budget does not grant). Such a stretch ends the wait as soon.
/// (A test unit at 29.8 GB beside another suite on a swapping box read as "sat idle", with 7,324
/// Background jobs queued, 11-10.) And a test may run what the app waits on in its own process (an
/// AI hub on loopback with its GPU device): what the test process runs beside the thread driving
/// the test ([`beside_cpu`]) is progress the app waits for, and while it runs at the idle share the
/// app is not idle. (A grown part's refine on a cold cache: the app at 0%, the test's hub compiling
/// its GPU programs, read as "sat idle 10.2 s", 11-10.) An app whose main thread runs nothing, its
/// other threads under 1/[`TICK_SHARE`] of a core, with nothing pending, is idle too: a steady
/// tick is no progress, while a thread at work, at half a core or more, still is. (A Fab test's
/// wait for a button that never came ran 19 min beside its app's sim ticking at 4-12% of a core,
/// its main thread parked in poll, L798418, 11-10.)
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
    /// The process's major page faults at the last look, and of the idle stretch, how long its
    /// looks took some and how many.
    faults: u64,
    memory: (Duration, u64),
    /// The jobs the app pool's workers had taken at the last look, and of the idle stretch, how
    /// long its looks found queued jobs and none taken, and the jobs queued per class at the last.
    taken: u64,
    starved: (Duration, [u64; 3]),
    /// What the test process had run beside its driving thread at the last look ([`beside_cpu`]),
    /// and how long, over the whole wait, that alone kept the app from counting as idle.
    beside: (Duration, Duration),
    /// The time the wait spent asking the app (its closure ran), and the app's own CPU and its main
    /// thread's in it, since the last look.
    asked: (Duration, Duration, Duration),
}

impl Budget {
    fn start(app: &TestApp, limit: Duration) -> TestResult<Budget> {
        Ok(Budget::at(limit, app.action_timeout(), app.client().cpu(true)?, Instant::now(), beside_cpu()))
    }

    /// A wait's budget from the app's CPU `cpu` and the test process's `beside` it at `now`.
    fn at(limit: Duration, idle_limit: Duration, cpu: crate::remote::AppCpu, now: Instant, beside: Duration) -> Budget {
        Budget { limit, idle_limit, main: cpu.main, last: (now, cpu.own, cpu.main, cpu.waited), idle: Duration::ZERO, ticked: (Duration::ZERO, Duration::ZERO), faults: cpu.faults, memory: (Duration::ZERO, 0), taken: 0, starved: (Duration::ZERO, [0; 3]), beside: (beside, Duration::ZERO), asked: (Duration::ZERO, Duration::ZERO, Duration::ZERO) }
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
        let cpu = app.client().cpu(true)?;
        self.judge(cpu, Instant::now(), beside_cpu(), &mut || app.client().settled(), &mut || app.client().waits_on_jit())
    }

    /// [`Self::spent`] from a look that found `cpu` and the test process's `beside` at `now`;
    /// `settled` and `waits_on_jit` ask the app, only when the app ran nothing.
    fn judge(&mut self, cpu: crate::remote::AppCpu, now: Instant, beside: Duration, settled: &mut dyn FnMut() -> TestResult<crate::remote::AppSettled>, waits_on_jit: &mut dyn FnMut() -> TestResult<bool>) -> TestResult<Option<String>> {
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
        // what the test process ran meanwhile beside this thread: a service the app waits on
        let busy_beside = beside.saturating_sub(self.beside.0) * IDLE_SHARE >= over;
        self.beside.0 = beside;
        if (quiet || ticking) && busy_beside {
            self.beside.1 += over;
        }
        let settled = if (quiet || ticking) && !busy_beside { Some(settled()?) } else { None };
        let (queued, running, taken) = settled.as_ref().and_then(|s| s.pool).unwrap_or(([0; 3], 0, self.taken));
        // the app ran nothing (or only ticked), with nothing pending (or only pool jobs, none taken
        // since the last look), and nothing ran beside the test in its own process
        let stalled = settled.as_ref().is_some_and(|s| s.settled || (s.only_pool && taken == self.taken)) && !waits_on_jit()?;
        let faults = cpu.faults.saturating_sub(self.faults);
        if stalled {
            self.idle += over;
            if ticking {
                self.ticked = (self.ticked.0 + over, self.ticked.1 + ran);
            }
            if faults as f64 >= SWAP_FAULTS_PER_S * over.as_secs_f64() {
                self.memory = (self.memory.0 + over, self.memory.1 + faults);
            }
            if queued.iter().any(|&n| n > 0) {
                self.starved = (self.starved.0 + over, queued);
            }
        } else {
            (self.idle, self.ticked, self.memory, self.starved) = (Duration::ZERO, (Duration::ZERO, Duration::ZERO), (Duration::ZERO, 0), (Duration::ZERO, [0; 3]));
        }
        self.last = (now, cpu.own, cpu.main, cpu.waited);
        self.faults = cpu.faults;
        self.taken = taken;
        if self.idle < self.idle_limit {
            return Ok(None);
        }
        let (idle, budget, main) = (self.idle.as_secs_f64(), self.idle_limit.as_secs_f64(), main.as_secs_f64());
        let beside = match self.beside.1 {
            Duration::ZERO => String::new(),
            busy => format!("; before that the test process's own threads (a service the test runs in it, as a hub) were busy for {:.1} s, which the wait counted as progress", busy.as_secs_f64()),
        };
        Ok(Some(match (self.memory, self.starved) {
            ((waited, n), _) if n > 0 => format!(
                "it was waiting on memory (swapping) for {:.1} of the {idle:.1} s it ran nothing (budget {budget:.0} s): {n} major page faults, {:.0} a second while it waited{} (its main thread ran {main:.1} s of CPU){beside}",
                waited.as_secs_f64(),
                n as f64 / waited.as_secs_f64(),
                memory_pressure().map_or(String::new(), |p| format!("; the box's memory pressure: tasks waited on memory {p:.1}% of the last 10 s"))
            ),
            (_, (starved, queued)) if starved > Duration::ZERO => {
                let classes: Vec<String> = ["Interactive", "Visible", "Background"].iter().zip(queued).filter(|(_, n)| *n > 0).map(|(class, n)| format!("{n} jobs queued at {class}")).collect();
                format!(
                    "its pool was starved: {}, none granted for {:.1} s of the {idle:.1} s it ran nothing (budget {budget:.0} s; its main thread ran {main:.1} s of CPU){beside}",
                    classes.join(", "),
                    starved.as_secs_f64()
                )
            }
            _ => format!(
                "it sat idle for {idle:.1} s (budget {budget:.0} s), nothing running and nothing pending{}{} (its main thread ran {main:.1} s of CPU){beside}",
                if running > 0 { format!(" ({running} pool jobs on its workers ran no CPU)") } else { String::new() },
                match self.ticked {
                    (Duration::ZERO, _) => String::new(),
                    (ticked, cpu) => format!("; for {:.1} s of it only a steady tick ran, {:.0}% of a core, its main thread nothing (no progress)", ticked.as_secs_f64(), 100.0 * cpu.as_secs_f64() / ticked.as_secs_f64()),
                }
            ),
        }))
    }
}

/// The box's memory pressure where the OS says it (Linux PSI): the share of the last 10 s in which
/// some task waited on memory, in percent.
fn memory_pressure() -> Option<f64> {
    let text = fs::read_to_string("/proc/pressure/memory").ok()?;
    let some = text.lines().find(|line| line.starts_with("some "))?;
    some.split_whitespace().find_map(|field| field.strip_prefix("avg10="))?.parse().ok()
}

/// The CPU the test process has run beside the calling thread (the one driving the test): what a
/// service the test runs in its own process runs (an AI hub on loopback the app asks, its GPU
/// device's thread compiling its programs), less what the runtime's own threads ran (under makepad
/// run, the JIT's checks and tier-ups of the test), what serving remote peers cost it, and what its
/// connections to the app ran carrying the test's asks. The process's other tests wait for the test
/// lock meanwhile, running nothing (a suite that runs them side by side counts their drivers too:
/// an idle app fails its wait once they quiet).
/// The number of an action the app has not taken yet, from its `action N timed out` answer.
fn pending_action(message: &str) -> Option<u64> {
    let at = message.find(" timed out")?;
    message[..at].rsplit_once("action ")?.1.parse().ok()
}

fn beside_cpu() -> Duration {
    use makepad_hal::clock;
    let others = clock::process_cpu_ns().saturating_sub(clock::thread_cpu_ns());
    Duration::from_nanos(others.saturating_sub(makepad_remote::runtime_cpu_ns()).saturating_sub(makepad_remote::served_cpu_ns()).saturating_sub(makepad_remote::asked_cpu_ns()))
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


#[cfg(test)]
mod tests {
    use super::{
        button_index, env_duration_ms, sanitize_path_component,
        snapshot_is_visible, visible_mode_enabled, Budget, NodeMatch,
    };
    use crate::remote::{AppCpu, AppSettled};
    use std::time::Instant;

    /// A wait for what never comes fails on the idle rule while the app's other thread ticks a
    /// steady fifth of a core, its main thread running nothing and nothing pending (a Fab sim
    /// beside a wait for a button that never came ran 19 min, L798418, 11-10). A thread at work, at
    /// half a core or more, keeps the wait going, and so does a main thread that runs, and a thread
    /// a loaded box starves: one that runs a fifth of a core and waits for one the rest wants a
    /// whole one (J11427, 11-11). An app with nothing running fails as soon on a loaded box.
    #[test]
    fn a_steady_tick_is_no_progress_and_work_is() {
        let secs = Duration::from_secs_f64;
        // its other threads' CPU, its main thread's and what they waited for a core, a second
        let wait = |other: f64, main: f64, waited: f64| -> Option<(u64, String)> {
            let (t0, cpu) = (Instant::now(), |m: f64, own: f64, waited: f64| AppCpu { main: secs(m), own: secs(own), faults: 0, waited: secs(waited) });
            let mut budget = Budget::at(secs(300.0), secs(10.0), cpu(0.0, 0.0, 0.0), t0, Duration::ZERO);
            (1..=60u64).find_map(|look| {
                let n = look as f64;
                let now = t0 + Duration::from_secs(look);
                let mut settled = || -> crate::TestResult<AppSettled> { Ok(AppSettled { settled: true, only_pool: false, pool: None }) };
                let mut on_jit = || -> crate::TestResult<bool> { Ok(false) };
                budget.judge(cpu(main * n, (main + other) * n, waited * n), now, Duration::ZERO, &mut settled, &mut on_jit).unwrap().map(|why| (look, why))
            })
        };
        let (look, why) = wait(0.2, 0.0, 0.0).expect("a steady tick ends the wait");
        assert_eq!(look, 10, "after the idle budget: {why}");
        assert!(why.starts_with("it sat idle for 10.0 s") && why.contains("only a steady tick ran, 20% of a core"), "{why}");
        assert!(wait(0.6, 0.0, 0.0).is_none(), "a thread at 60% of a core is at work");
        assert!(wait(0.2, 0.1, 0.0).is_none(), "a main thread that runs is progress");
        assert!(wait(0.2, 0.0, 0.8).is_none(), "a worker starved of a core it wants is at work");
        assert_eq!(wait(0.2, 0.0, 0.1).map(|(look, _)| look), Some(10), "a tick that waits a little on a loaded box is still a tick");
        let (look, why) = wait(0.0, 0.0, 0.02).expect("an app with nothing running is idle on a loaded box");
        assert!(look == 10 && why.starts_with("it sat idle for 10.0 s") && !why.contains("tick"), "{look}: {why}");
    }
    use crate::{Selector, TestConfig};
    use makepad_studio_protocol::MouseButton;
    use makepad_a11y::{Node, Role, Rect};
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    static ENV_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

    fn restore_env_var(name: &str, value: Option<String>) {
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
        // The default: neither variable set (a devhub lease sets its own artifact root).
        let old_visible = std::env::var("MAKEPAD_TEST_VISIBLE").ok();
        let old_artifacts = std::env::var("MAKEPAD_TEST_ARTIFACTS").ok();
        std::env::remove_var("MAKEPAD_TEST_VISIBLE");
        std::env::remove_var("MAKEPAD_TEST_ARTIFACTS");
        let config =
            TestConfig::current_package("/tmp/example", "makepad-example", "ui::test").unwrap();
        restore_env_var("MAKEPAD_TEST_VISIBLE", old_visible);
        restore_env_var("MAKEPAD_TEST_ARTIFACTS", old_artifacts);
        assert_eq!(
            config.artifacts_dir,
            PathBuf::from("/tmp/example")
                .join("target")
                .join("makepad_test")
                .join("makepad-example")
                .join("ui__test")
        );
        assert!(!config.visible);
        assert!(!config.env.contains_key("MAKEPAD"));
        assert!(!config.env.contains_key("CARGO_TARGET_DIR"));
        assert_eq!(config.requested_screenshot_dpi(), None);
    }

    #[test]
    fn visible_mode_comes_from_env() {
        let _guard = ENV_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_visible = std::env::var("MAKEPAD_TEST_VISIBLE").ok();
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
        let old_value = std::env::var("MAKEPAD_TEST_ACTION_DELAY_MS").ok();
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

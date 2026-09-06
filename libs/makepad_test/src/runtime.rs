use crate::app_process::{self, LaunchSpec, OwnedApp, ShutdownOutcome};
use crate::error::{IntoTestResult, TestError, TestResult};
use crate::remote::{KeyKind, MouseInput, MouseKind};
use crate::selector::Selector;
use makepad_micro_serde::SerJson;
use makepad_studio_protocol::{KeyCode, KeyModifiers, StudioToApp, WidgetSnapshot};
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
const DRAG_STEPS: usize = 6;
const RECENT_LOG_LINES: usize = 200;

static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WidgetMatch {
    pub raw: String,
    pub id: String,
    pub widget_type: String,
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

impl WidgetMatch {
    #[allow(dead_code)]
    fn parse(line: &str) -> Option<Self> {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() != 7 {
            return None;
        }
        Some(Self {
            raw: line.to_string(),
            id: tokens[1].to_string(),
            widget_type: tokens[2].to_string(),
            x: tokens[3].parse().ok()?,
            y: tokens[4].parse().ok()?,
            width: tokens[5].parse().ok()?,
            height: tokens[6].parse().ok()?,
        })
    }

    #[allow(dead_code)]
    fn center(&self) -> (i64, i64) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }
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
    /// also applies to the build. `MAKEPAD_HEADLESS_DPI=N` scales screenshots
    /// to N pixels per layout point.
    pub env: HashMap<String, String>,
    /// Extra arguments appended after `--remote`.
    pub app_args: Vec<String>,
    /// Show the window (unfocused). Default is hidden (`MAKEPAD_HIDE_WINDOWS=1`).
    pub visible: bool,
    pub startup_timeout: Duration,
    pub action_timeout: Duration,
    pub poll_interval: Duration,
    pub startup_pause: Duration,
    pub action_delay: Duration,
    pub keep_open: Duration,
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
        let artifacts_dir = manifest_dir
            .join("target")
            .join("makepad_test")
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

    /// Screenshot scale that honours `MAKEPAD_HEADLESS_DPI` from `env`: the
    /// old software backend rendered at that dpi, so a suite that asked for
    /// `1` expects layout points to equal PNG pixels.
    fn requested_screenshot_dpi(&self) -> Option<f64> {
        self.env
            .get("MAKEPAD_HEADLESS_DPI")
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
    fn start(config: TestConfig) -> TestResult<Self> {
        if config.artifacts_dir.exists() {
            fs::remove_dir_all(&config.artifacts_dir)?;
        }
        fs::create_dir_all(&config.artifacts_dir)?;

        let executable = app_process::build_release_binary(
            &config.manifest_dir,
            &config.package_name,
            config.bin_name.as_deref(),
            &config.artifacts_dir,
            config.env.get("CARGO_TARGET_DIR").map(String::as_str),
        )?;
        let app = app_process::launch(&config.launch_spec(), &executable)?;

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

    /// `host:port` of the owned app's remote surface, for diagnostics.
    pub fn remote_endpoint(&self) -> String {
        self.inner.borrow().app.client.endpoint()
    }

    /// The directory the app writes its own grabs into (`/g`).
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

    pub fn widget_snapshot(&self) -> Vec<WidgetSnapshot> {
        match self.try_widget_snapshot() {
            Ok(widgets) => widgets,
            Err(err) => panic_for_error(err),
        }
    }

    pub fn try_widget_snapshot(&self) -> TestResult<Vec<WidgetSnapshot>> {
        self.ensure_running()?;
        self.client().snapshot()
    }

    pub fn wait_for_log_contains(&self, needle: &str) {
        if let Err(err) = self.try_wait_for_log_contains(needle) {
            panic_for_error(err);
        }
    }

    pub fn try_wait_for_log_contains(&self, needle: &str) -> TestResult<()> {
        let deadline = Instant::now() + self.action_timeout();
        loop {
            self.ensure_running()?;
            let tail = self.client().log_since(0)?;
            if tail.lines.iter().any(|line| line.contains(needle)) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(TestError::new(format!(
                    "timed out waiting for log containing `{needle}`"
                )));
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

    fn try_click_center(&self, target: &WidgetSnapshot) -> TestResult<()> {
        self.ensure_running()?;
        let (x, y) = snapshot_center_f64(target);
        let window = Some(target.window_index);
        self.client()
            .mouse(&MouseInput::at(MouseKind::Click, window, x, y))?;
        self.inner.borrow_mut().focus_window = window;
        self.pace_after_action();
        Ok(())
    }

    fn try_scroll_center(&self, target: &WidgetSnapshot, sx: f64, sy: f64) -> TestResult<()> {
        self.ensure_running()?;
        let (x, y) = snapshot_center_f64(target);
        self.client().mouse(&MouseInput {
            dx: sx,
            dy: sy,
            ..MouseInput::at(MouseKind::Scroll, Some(target.window_index), x, y)
        })?;
        self.pace_after_action();
        Ok(())
    }

    fn try_drag_from(&self, target: &WidgetSnapshot, dx: f64, dy: f64) -> TestResult<()> {
        self.ensure_running()?;
        let (start_x, start_y) = snapshot_center_f64(target);
        let window = Some(target.window_index);
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
    ) -> TestResult<Vec<WidgetSnapshot>> {
        let widgets = self.try_widget_snapshot()?;
        let (primary_window_id, primary_window_index) = primary_window_scope(&widgets);
        let mut matches: Vec<_> = widgets
            .into_iter()
            .filter(|widget| selector.matches(widget, &primary_window_id, primary_window_index))
            .collect();
        if visible_only {
            matches.retain(snapshot_is_visible);
        }
        matches.sort_by(|left, right| snapshot_sort_key(left).cmp(&snapshot_sort_key(right)));
        if let Some(index) = selector.nth_index() {
            return Ok(matches.into_iter().nth(index).into_iter().collect());
        }
        Ok(matches)
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

impl Locator {
    pub fn wait_visible(self) -> Self {
        if let Err(err) = self.try_wait_visible() {
            panic_for_error(err);
        }
        self
    }

    pub fn try_wait_visible(&self) -> TestResult<()> {
        let query = self.selector.describe();
        let deadline = Instant::now() + self.app.action_timeout();
        while Instant::now() < deadline {
            if !self.app.query_widgets(&self.selector, true)?.is_empty() {
                return Ok(());
            }
            thread::sleep(self.app.poll_interval());
        }
        Err(TestError::new(format!(
            "timed out waiting for selector `{query}` to become visible"
        )))
    }

    pub fn wait_hidden(self) -> Self {
        if let Err(err) = self.try_wait_hidden() {
            panic_for_error(err);
        }
        self
    }

    pub fn try_wait_hidden(&self) -> TestResult<()> {
        let query = self.selector.describe();
        let deadline = Instant::now() + self.app.action_timeout();
        while Instant::now() < deadline {
            if self.app.query_widgets(&self.selector, true)?.is_empty() {
                return Ok(());
            }
            thread::sleep(self.app.poll_interval());
        }
        Err(TestError::new(format!(
            "timed out waiting for selector `{query}` to become hidden"
        )))
    }

    pub fn wait_count(self, expected: usize) -> Self {
        if let Err(err) = self.try_wait_count(expected) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_wait_count(&self, expected: usize) -> TestResult<()> {
        let query = self.selector.describe();
        let deadline = Instant::now() + self.app.action_timeout();
        while Instant::now() < deadline {
            let count = self.app.query_widgets(&self.selector, true)?.len();
            if count == expected {
                return Ok(());
            }
            thread::sleep(self.app.poll_interval());
        }
        Err(TestError::new(format!(
            "timed out waiting for selector `{query}` to match {expected} visible widgets"
        )))
    }

    pub fn assert_text(self, expected: impl AsRef<str>) -> Self {
        if let Err(err) = self.try_assert_text(expected) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_assert_text(&self, expected: impl AsRef<str>) -> TestResult<()> {
        let expected = expected.as_ref();
        let widget = self.resolve_unique_readable()?;
        match widget.text.as_deref() {
            Some(actual) if actual == expected => Ok(()),
            Some(actual) => Err(TestError::new(format!(
                "selector `{}` expected text `{expected}`, found `{actual}`",
                self.selector.describe()
            ))),
            None => Err(TestError::new(format!(
                "selector `{}` does not expose text state",
                self.selector.describe()
            ))),
        }
    }

    pub fn wait_text(self, expected: impl AsRef<str>) -> Self {
        if let Err(err) = self.try_wait_text(expected) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_wait_text(&self, expected: impl AsRef<str>) -> TestResult<()> {
        self.wait_for_state("text", expected.as_ref(), |widget| widget.text.as_deref())
    }

    pub fn assert_value(self, expected: impl AsRef<str>) -> Self {
        if let Err(err) = self.try_assert_value(expected) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_assert_value(&self, expected: impl AsRef<str>) -> TestResult<()> {
        let expected = expected.as_ref();
        let widget = self.resolve_unique_readable()?;
        match widget.value.as_deref() {
            Some(actual) if actual == expected => Ok(()),
            Some(actual) => Err(TestError::new(format!(
                "selector `{}` expected value `{expected}`, found `{actual}`",
                self.selector.describe()
            ))),
            None => Err(TestError::new(format!(
                "selector `{}` does not expose value state",
                self.selector.describe()
            ))),
        }
    }

    pub fn wait_value(self, expected: impl AsRef<str>) -> Self {
        if let Err(err) = self.try_wait_value(expected) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_wait_value(&self, expected: impl AsRef<str>) -> TestResult<()> {
        self.wait_for_state("value", expected.as_ref(), |widget| widget.value.as_deref())
    }

    pub fn assert_checked(self, expected: bool) -> Self {
        if let Err(err) = self.try_assert_checked(expected) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_assert_checked(&self, expected: bool) -> TestResult<()> {
        let widget = self.resolve_unique_readable()?;
        match widget.checked {
            Some(actual) if actual == expected => Ok(()),
            Some(actual) => Err(TestError::new(format!(
                "selector `{}` expected checked state `{expected}`, found `{actual}`",
                self.selector.describe()
            ))),
            None => Err(TestError::new(format!(
                "selector `{}` does not expose checked state",
                self.selector.describe()
            ))),
        }
    }

    pub fn wait_checked(self, expected: bool) -> Self {
        if let Err(err) = self.try_wait_checked(expected) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_wait_checked(&self, expected: bool) -> TestResult<()> {
        self.wait_for_bool_state("checked", expected, |widget| widget.checked)
    }

    pub fn assert_enabled(self, expected: bool) -> Self {
        if let Err(err) = self.try_assert_enabled(expected) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_assert_enabled(&self, expected: bool) -> TestResult<()> {
        let widget = self.resolve_unique_readable()?;
        if widget.enabled == expected {
            Ok(())
        } else {
            Err(TestError::new(format!(
                "selector `{}` expected enabled state `{expected}`, found `{}`",
                self.selector.describe(), widget.enabled
            )))
        }
    }

    pub fn wait_enabled(self, expected: bool) -> Self {
        if let Err(err) = self.try_wait_enabled(expected) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_wait_enabled(&self, expected: bool) -> TestResult<()> {
        self.wait_for_bool_state("enabled", expected, |widget| Some(widget.enabled))
    }

    pub fn click(self) -> Self {
        if let Err(err) = self.try_click() {
            panic_for_error(err);
        }
        self
    }

    pub fn try_click(&self) -> TestResult<()> {
        let target = self.resolve_unique_visible()?;
        self.app.try_click_center(&target)
    }

    pub fn type_text(self, text: impl AsRef<str>) -> Self {
        if let Err(err) = self.try_type_text(text) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_type_text(&self, text: impl AsRef<str>) -> TestResult<()> {
        self.try_click()?;
        self.app.try_type_text(text)
    }

    pub fn clear(self) -> Self {
        if let Err(err) = self.try_clear() {
            panic_for_error(err);
        }
        self
    }

    pub fn try_clear(&self) -> TestResult<()> {
        let widget = self.resolve_unique_visible()?;
        if widget.value.is_none() {
            return Err(TestError::new(format!(
                "selector `{}` is not a text input",
                self.selector.describe()
            )));
        }
        self.app.try_click_center(&widget)?;
        self.app
            .try_press_key_with_modifiers(KeyCode::KeyA, primary_shortcut_modifiers())?;
        self.app.try_press_key(KeyCode::Backspace)
    }

    pub fn fill(self, text: impl AsRef<str>) -> Self {
        if let Err(err) = self.try_fill(text) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_fill(&self, text: impl AsRef<str>) -> TestResult<()> {
        self.try_clear()?;
        self.app.try_type_text(text)
    }

    pub fn press_key(self, key_code: KeyCode) -> Self {
        if let Err(err) = self.try_press_key(key_code) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_press_key(&self, key_code: KeyCode) -> TestResult<()> {
        self.try_click()?;
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
        self.try_click()?;
        self.app.try_press_key_with_modifiers(key_code, modifiers)
    }

    pub fn scroll(self, sx: f64, sy: f64) -> Self {
        if let Err(err) = self.try_scroll(sx, sy) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_scroll(&self, sx: f64, sy: f64) -> TestResult<()> {
        let target = self.resolve_unique_visible()?;
        self.app.try_scroll_center(&target, sx, sy)
    }

    pub fn drag_by(self, dx: f64, dy: f64) -> Self {
        if let Err(err) = self.try_drag_by(dx, dy) {
            panic_for_error(err);
        }
        self
    }

    pub fn try_drag_by(&self, dx: f64, dy: f64) -> TestResult<()> {
        let target = self.resolve_unique_visible()?;
        self.app.try_drag_from(&target, dx, dy)
    }

    pub fn snapshot(&self) -> WidgetSnapshot {
        match self.try_snapshot() {
            Ok(widget) => widget,
            Err(err) => panic_for_error(err),
        }
    }

    pub fn try_snapshot(&self) -> TestResult<WidgetSnapshot> {
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

    fn wait_for_state<F>(&self, field_name: &str, expected: &str, accessor: F) -> TestResult<()>
    where
        F: Fn(&WidgetSnapshot) -> Option<&str>,
    {
        let deadline = Instant::now() + self.app.action_timeout();
        let mut last_seen = None::<String>;
        while Instant::now() < deadline {
            let widget = match self.resolve_unique_readable() {
                Ok(widget) => widget,
                Err(err) if selector_resolution_error(err.message()) => {
                    thread::sleep(self.app.poll_interval());
                    continue;
                }
                Err(err) => return Err(err),
            };
            match accessor(&widget) {
                Some(actual) if actual == expected => return Ok(()),
                Some(actual) => {
                    last_seen = Some(actual.to_string());
                    thread::sleep(self.app.poll_interval());
                }
                None => {
                    thread::sleep(self.app.poll_interval());
                }
            }
        }
        let detail = last_seen
            .map(|value| format!(" last seen `{value}`"))
            .unwrap_or_default();
        Err(TestError::new(format!(
            "timed out waiting for selector `{}` {} to equal `{expected}`{}",
            self.selector.describe(),
            field_name,
            detail
        )))
    }

    fn wait_for_bool_state<F>(
        &self,
        field_name: &str,
        expected: bool,
        accessor: F,
    ) -> TestResult<()>
    where
        F: Fn(&WidgetSnapshot) -> Option<bool>,
    {
        let deadline = Instant::now() + self.app.action_timeout();
        let mut last_seen = None::<bool>;
        while Instant::now() < deadline {
            let widget = match self.resolve_unique_readable() {
                Ok(widget) => widget,
                Err(err) if selector_resolution_error(err.message()) => {
                    thread::sleep(self.app.poll_interval());
                    continue;
                }
                Err(err) => return Err(err),
            };
            match accessor(&widget) {
                Some(actual) if actual == expected => return Ok(()),
                Some(actual) => {
                    last_seen = Some(actual);
                    thread::sleep(self.app.poll_interval());
                }
                None => {
                    thread::sleep(self.app.poll_interval());
                }
            }
        }
        let detail = last_seen
            .map(|value| format!(" last seen `{value}`"))
            .unwrap_or_default();
        Err(TestError::new(format!(
            "timed out waiting for selector `{}` {} to equal `{expected}`{}",
            self.selector.describe(),
            field_name,
            detail
        )))
    }

    fn resolve_unique_visible(&self) -> TestResult<WidgetSnapshot> {
        let query = self.selector.describe();
        let matches = self.app.query_widgets(&self.selector, true)?;
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
    fn resolve_unique_readable(&self) -> TestResult<WidgetSnapshot> {
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
        matches: Vec<WidgetSnapshot>,
        empty_message: &str,
    ) -> TestResult<WidgetSnapshot> {
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
    // sensitive assertions flaky. A suite whose app is cheap enough can opt out.
    let _guard = (!parallel_tests_enabled()).then(|| {
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
            capture_failure_artifacts(&app, err.message());
            app.pause_before_shutdown();
            app.shutdown();
            Err(err)
        }
        Err(payload) => {
            let err = TestError::from_panic_payload(payload);
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

    match app.try_widget_snapshot() {
        Ok(snapshot) => {
            let _ = fs::write(
                artifact_dir.join("widget-snapshot.json"),
                snapshot.serialize_json(),
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
        StudioToApp::WidgetTreeDump(_) => "WidgetTreeDump",
        StudioToApp::WidgetQuery(_) => "WidgetQuery",
        StudioToApp::WidgetSnapshot(_) => "WidgetSnapshot",
        StudioToApp::KeepAlive => "KeepAlive",
        StudioToApp::LiveChange { .. } => "LiveChange",
        StudioToApp::Swapchain(_) => "Swapchain",
        StudioToApp::WindowGeomChange { .. } => "WindowGeomChange",
        StudioToApp::Tick => "Tick",
        StudioToApp::MouseDown(_) => "MouseDown",
        StudioToApp::MouseUp(_) => "MouseUp",
        StudioToApp::MouseMove(_) => "MouseMove",
        StudioToApp::TweakRay(_) => "TweakRay",
        StudioToApp::KeyDown(_) => "KeyDown",
        StudioToApp::KeyUp(_) => "KeyUp",
        StudioToApp::TextInput(_) => "TextInput",
        StudioToApp::TextCopy => "TextCopy",
        StudioToApp::TextCut => "TextCut",
        StudioToApp::Scroll(_) => "Scroll",
        StudioToApp::GameInput(_) => "GameInput",
        StudioToApp::Custom(_) => "Custom",
        StudioToApp::None => "None",
        StudioToApp::Kill => "Kill",
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

fn selector_resolution_error(message: &str) -> bool {
    message.contains("matched no visible widgets") || message.contains("matched multiple widgets")
}

fn primary_window_scope(widgets: &[WidgetSnapshot]) -> (String, usize) {
    if let Some(widget) = widgets
        .iter()
        .filter(|widget| !widget.window_id.is_empty())
        .min_by(|left, right| snapshot_sort_key(left).cmp(&snapshot_sort_key(right)))
    {
        return (widget.window_id.clone(), widget.window_index);
    }
    (String::new(), 0)
}

fn snapshot_is_visible(widget: &WidgetSnapshot) -> bool {
    widget.visible && widget.width > 0 && widget.height > 0
}

fn snapshot_sort_key(widget: &WidgetSnapshot) -> (usize, i64, i64, String, String) {
    (
        widget.window_index,
        widget.y,
        widget.x,
        widget.id.clone(),
        widget.widget_type.clone(),
    )
}

fn snapshot_center(widget: &WidgetSnapshot) -> (i64, i64) {
    (widget.x + widget.width / 2, widget.y + widget.height / 2)
}

fn snapshot_center_f64(widget: &WidgetSnapshot) -> (f64, f64) {
    let (x, y) = snapshot_center(widget);
    (x as f64, y as f64)
}

fn snapshot_summary(widget: &WidgetSnapshot) -> String {
    let mut fields = Vec::new();
    fields.push(format!(
        "{} {} @{} {} {} {} {}",
        widget.id,
        widget.widget_type,
        widget.window_id,
        widget.x,
        widget.y,
        widget.width,
        widget.height
    ));
    if let Some(text) = &widget.text {
        fields.push(format!("text={text:?}"));
    }
    if let Some(value) = &widget.value {
        fields.push(format!("value={value:?}"));
    }
    if let Some(checked) = widget.checked {
        fields.push(format!("checked={checked}"));
    }
    if let Some(selected) = &widget.selected {
        fields.push(format!("selected={selected:?}"));
    }
    fields.join(" ")
}

fn parallel_tests_enabled() -> bool {
    env_truthy("MAKEPAD_TEST_PARALLEL")
}

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

fn primary_shortcut_modifiers() -> KeyModifiers {
    #[cfg(target_vendor = "apple")]
    {
        KeyModifiers {
            logo: true,
            ..Default::default()
        }
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        KeyModifiers {
            control: true,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        button_index, env_duration_ms, primary_window_scope, sanitize_path_component,
        snapshot_is_visible, snapshot_sort_key, visible_mode_enabled, TestError, TestResult,
        WidgetMatch,
    };
    use crate::{Selector, TestConfig};
    use makepad_studio_protocol::{MouseButton, WidgetSnapshot};
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

    fn snapshot(id: &str) -> WidgetSnapshot {
        WidgetSnapshot {
            id: id.to_string(),
            widget_type: "Button".to_string(),
            window_id: "main_window".to_string(),
            window_index: 0,
            visible: true,
            enabled: true,
            x: 10,
            y: 20,
            width: 30,
            height: 40,
            text: Some(id.to_string()),
            value: None,
            checked: None,
            selected: None,
        }
    }

    #[test]
    fn widget_match_parse_accepts_widget_rects() {
        let parsed = WidgetMatch::parse("12 input_singleline TextInput 10 20 30 40").unwrap();
        assert_eq!(parsed.id, "input_singleline");
        assert_eq!(parsed.widget_type, "TextInput");
        assert_eq!(parsed.center(), (25, 40));
    }

    #[test]
    fn widget_match_parse_accepts_dock_rects() {
        let parsed = WidgetMatch::parse("DT math_tab DockTab 10 20 30 40").unwrap();
        assert_eq!(parsed.id, "math_tab");
        assert_eq!(parsed.widget_type, "DockTab");
    }

    #[test]
    fn config_uses_expected_artifact_dir_and_hidden_default() {
        let _guard = ENV_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_visible = std::env::var("MAKEPAD_TEST_VISIBLE").ok();
        std::env::remove_var("MAKEPAD_TEST_VISIBLE");
        let config =
            TestConfig::current_package("/tmp/example", "makepad-example", "ui::test").unwrap();
        restore_env_var("MAKEPAD_TEST_VISIBLE", old_visible);
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
    fn headless_dpi_env_scales_screenshots() {
        let mut config =
            TestConfig::current_package("/tmp/example", "makepad-example", "ui::test").unwrap();
        config
            .env
            .insert("MAKEPAD_HEADLESS_DPI".to_string(), "1".to_string());
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
    fn result_alias_accepts_test_error() {
        let result: TestResult<()> = Err(TestError::new("boom"));
        assert!(result.is_err());
    }

    #[test]
    fn selector_queries_remain_publicly_compatible() {
        assert_eq!(Selector::id("foo").as_query(), "id:foo");
    }

    #[test]
    fn window_scope_defaults_to_primary_window() {
        let widgets = vec![
            WidgetSnapshot {
                window_id: "panel_window".to_string(),
                window_index: 1,
                ..snapshot("secondary")
            },
            snapshot("primary"),
        ];
        assert_eq!(
            primary_window_scope(&widgets),
            ("main_window".to_string(), 0)
        );
    }

    #[test]
    fn snapshot_visibility_requires_geometry() {
        let mut widget = snapshot("hidden");
        widget.width = 0;
        assert!(!snapshot_is_visible(&widget));
    }

    #[test]
    fn snapshot_sort_prefers_window_then_position() {
        let mut left = snapshot("left");
        let mut right = snapshot("right");
        left.x = 10;
        right.x = 20;
        assert!(snapshot_sort_key(&left) < snapshot_sort_key(&right));
    }

    #[test]
    fn button_bits_map_to_remote_indices() {
        assert_eq!(button_index(MouseButton::PRIMARY.bits()), 0);
        assert_eq!(button_index(0b100), 2);
        assert_eq!(button_index(0), 0);
    }
}

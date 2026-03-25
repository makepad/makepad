use crate::error::{IntoTestResult, TestError, TestResult};
use crate::selector::Selector;
use crate::studio_remote::StudioRemoteClient;
use makepad_micro_serde::{SerBin, SerJson};
use makepad_studio_hub::{HubConfig, HubConnection, MountConfig, StudioHub};
use makepad_studio_protocol::hub_protocol::{ClientToHub, HubToClient, LogEntry, QueryId};
use makepad_studio_protocol::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, RemoteKeyModifiers, RemoteMouseDown,
    RemoteMouseMove, RemoteMouseUp, RemoteScroll, StudioToApp, StudioToAppVec, WidgetSnapshot,
};
use std::cell::RefCell;
use std::cmp;
use std::collections::HashMap;
use std::fmt::Write;
use std::fs;
use std::net::{Ipv4Addr, SocketAddr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(600);
const ACTION_TIMEOUT: Duration = Duration::from_secs(10);
const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(20);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const STARTUP_RETRIES: usize = 2;
const STARTUP_RETRY_DELAY: Duration = Duration::from_millis(250);
const DRAG_STEPS: usize = 6;
const PUMP_TICKS: usize = 3;
const RECENT_LOG_LINES: usize = 200;
const DEFAULT_STUDIO_ADDR: &str = "127.0.0.1:8001";
const DEFAULT_STUDIO_MOUNT: &str = "makepad";

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
    pub mount_name: String,
    pub manifest_dir: PathBuf,
    pub test_name: String,
    pub artifacts_dir: PathBuf,
    pub listen_address: SocketAddr,
    pub env: HashMap<String, String>,
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
        if !visible_mode_enabled() {
            env.insert("MAKEPAD".to_string(), "headless".to_string());
        }
        env.insert("RUST_BACKTRACE".to_string(), "1".to_string());
        env.insert(
            "CARGO_TARGET_DIR".to_string(),
            manifest_dir.join("target").to_string_lossy().to_string(),
        );

        Ok(Self {
            mount_name: package_name.clone(),
            package_name,
            manifest_dir,
            test_name,
            artifacts_dir,
            listen_address: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            env,
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
}

enum TestConnection {
    InProcess(HubConnection),
    Remote(StudioRemoteClient),
}

impl TestConnection {
    fn send(&mut self, msg: ClientToHub) -> TestResult<QueryId> {
        match self {
            Self::InProcess(connection) => Ok(connection.send(msg)),
            Self::Remote(connection) => connection.send(msg),
        }
    }

    fn recv_timeout(&self, timeout: Duration) -> Option<HubToClient> {
        match self {
            Self::InProcess(connection) => connection.recv_timeout(timeout),
            Self::Remote(connection) => connection.recv_timeout(timeout),
        }
    }
}

struct TestAppInner {
    config: TestConfig,
    connection: TestConnection,
    build_id: QueryId,
    build_stopped: Option<Option<i32>>,
}

impl TestAppInner {
    fn observe_message(&mut self, msg: &HubToClient) {
        if let HubToClient::BuildStopped {
            build_id,
            exit_code,
        } = msg
        {
            if *build_id == self.build_id {
                self.build_stopped = Some(*exit_code);
            }
        }
    }
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

        let mut last_error = None;
        for attempt in 0..STARTUP_RETRIES {
            match Self::start_once(config.clone()) {
                Ok(app) => return Ok(app),
                Err(err) if attempt + 1 < STARTUP_RETRIES && startup_error_is_retryable(&err) => {
                    last_error = Some(err);
                    thread::sleep(STARTUP_RETRY_DELAY);
                }
                Err(err) => return Err(err),
            }
        }

        Err(last_error.unwrap_or_else(|| TestError::new("failed to start test app")))
    }

    fn start_once(config: TestConfig) -> TestResult<Self> {
        let (connection, build_id) = if visible_mode_enabled() {
            start_visible_app(&config)?
        } else {
            start_headless_app(&config)?
        };

        if config.startup_pause > Duration::ZERO {
            thread::sleep(config.startup_pause);
        }

        Ok(Self {
            inner: Rc::new(RefCell::new(TestAppInner {
                config,
                connection,
                build_id,
                build_stopped: None,
            })),
        })
    }

    pub fn locator(&self, selector: Selector) -> Locator {
        Locator {
            app: self.clone(),
            selector,
        }
    }

    pub fn type_text(&self, text: impl AsRef<str>) {
        if let Err(err) = self.try_type_text(text) {
            panic_for_error(err);
        }
    }

    pub fn try_type_text(&self, text: impl AsRef<str>) -> TestResult<()> {
        let text = text.as_ref().to_string();
        let build_id = self.build_id();
        self.send_no_wait(ClientToHub::TypeText { build_id, text })?;
        self.pace_after_action();
        Ok(())
    }

    pub fn press_return(&self) {
        if let Err(err) = self.try_press_return() {
            panic_for_error(err);
        }
    }

    pub fn try_press_return(&self) -> TestResult<()> {
        let build_id = self.build_id();
        self.send_no_wait(ClientToHub::Return {
            build_id,
            auto_dump: Some(false),
        })?;
        self.pace_after_action();
        Ok(())
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
        let event = KeyEvent {
            key_code,
            is_repeat: false,
            modifiers,
            time: now_seconds(),
        };
        self.try_forward(vec![StudioToApp::KeyDown(event), StudioToApp::KeyUp(event)])?;
        self.pace_after_action();
        Ok(())
    }

    pub fn screenshot(&self) -> PathBuf {
        match self.try_screenshot() {
            Ok(path) => path,
            Err(err) => panic_for_error(err),
        }
    }

    pub fn try_screenshot(&self) -> TestResult<PathBuf> {
        self.ensure_running()?;
        let build_id = self.build_id();
        let query_id = self.send(ClientToHub::Screenshot {
            build_id,
            kind_id: Some(0),
        })?;
        self.wait_for_reply(SCREENSHOT_TIMEOUT, move |msg| match msg {
            HubToClient::Screenshot {
                query_id: id, path, ..
            } if id == query_id => Some(Ok(PathBuf::from(path))),
            _ => None,
        })
    }

    pub fn widget_dump(&self) -> String {
        match self.try_widget_dump() {
            Ok(dump) => dump,
            Err(err) => panic_for_error(err),
        }
    }

    pub fn try_widget_dump(&self) -> TestResult<String> {
        self.ensure_running()?;
        self.try_pump_ui()?;
        let build_id = self.build_id();
        let query_id = self.send(ClientToHub::WidgetTreeDump { build_id })?;
        self.wait_for_reply(self.action_timeout(), move |msg| match msg {
            HubToClient::WidgetTreeDump {
                query_id: id, dump, ..
            } if id == query_id => Some(Ok(dump)),
            _ => None,
        })
    }

    pub fn widget_snapshot(&self) -> Vec<WidgetSnapshot> {
        match self.try_widget_snapshot() {
            Ok(widgets) => widgets,
            Err(err) => panic_for_error(err),
        }
    }

    pub fn try_widget_snapshot(&self) -> TestResult<Vec<WidgetSnapshot>> {
        self.ensure_running()?;
        self.try_pump_ui()?;
        let build_id = self.build_id();
        let query_id = self.send(ClientToHub::WidgetSnapshot { build_id })?;
        self.wait_for_reply(self.action_timeout(), move |msg| match msg {
            HubToClient::WidgetSnapshot {
                query_id: id,
                widgets,
                ..
            } if id == query_id => Some(Ok(widgets)),
            _ => None,
        })
    }

    pub fn wait_for_log_contains(&self, needle: &str) {
        if let Err(err) = self.try_wait_for_log_contains(needle) {
            panic_for_error(err);
        }
    }

    pub fn try_wait_for_log_contains(&self, needle: &str) -> TestResult<()> {
        let deadline = Instant::now() + self.action_timeout();
        while Instant::now() < deadline {
            let entries = self.query_logs_once(Some(needle.to_string()))?;
            if entries
                .iter()
                .any(|(_, entry)| entry.message.contains(needle))
            {
                return Ok(());
            }
            thread::sleep(self.poll_interval());
        }
        Err(TestError::new(format!(
            "timed out waiting for log containing `{needle}`"
        )))
    }

    pub fn forward(&self, msgs: Vec<StudioToApp>) {
        if let Err(err) = self.try_forward(msgs) {
            panic_for_error(err);
        }
    }

    pub fn try_forward(&self, msgs: Vec<StudioToApp>) -> TestResult<()> {
        let build_id = self.build_id();
        self.send_no_wait(ClientToHub::ForwardToApp {
            build_id,
            msg_bin: StudioToAppVec(msgs).serialize_bin(),
        })
    }

    fn try_click_center(&self, target: &WidgetSnapshot) -> TestResult<()> {
        self.ensure_running()?;
        let (x, y) = snapshot_center(target);
        let build_id = self.build_id();
        self.send_no_wait(ClientToHub::Click { build_id, x, y })?;
        self.pace_after_action();
        Ok(())
    }

    fn try_scroll_center(&self, target: &WidgetSnapshot, sx: f64, sy: f64) -> TestResult<()> {
        let (x, y) = snapshot_center_f64(target);
        self.try_forward(vec![StudioToApp::Scroll(RemoteScroll {
            time: now_seconds(),
            sx,
            sy,
            x,
            y,
            is_mouse: true,
            modifiers: RemoteKeyModifiers::default(),
        })])?;
        self.pace_after_action();
        Ok(())
    }

    fn try_drag_from(&self, target: &WidgetSnapshot, dx: f64, dy: f64) -> TestResult<()> {
        let (start_x, start_y) = snapshot_center_f64(target);
        let button_raw_bits = MouseButton::PRIMARY.bits();
        let mut msgs = Vec::with_capacity(DRAG_STEPS + 2);
        msgs.push(StudioToApp::MouseDown(RemoteMouseDown {
            button_raw_bits,
            x: start_x,
            y: start_y,
            time: now_seconds(),
            modifiers: RemoteKeyModifiers::default(),
        }));
        for step in 1..=DRAG_STEPS {
            let progress = step as f64 / DRAG_STEPS as f64;
            msgs.push(StudioToApp::MouseMove(RemoteMouseMove {
                time: now_seconds() + progress * 0.01,
                x: start_x + dx * progress,
                y: start_y + dy * progress,
                modifiers: RemoteKeyModifiers::default(),
            }));
        }
        msgs.push(StudioToApp::MouseUp(RemoteMouseUp {
            time: now_seconds() + 0.02,
            button_raw_bits,
            x: start_x + dx,
            y: start_y + dy,
            modifiers: RemoteKeyModifiers::default(),
        }));
        self.try_forward(msgs)?;
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

    fn query_logs_once(&self, pattern: Option<String>) -> TestResult<Vec<(usize, LogEntry)>> {
        let query_id = self.send_unchecked(ClientToHub::QueryLogs {
            build_id: Some(self.build_id()),
            level: None,
            source: None,
            file: None,
            pattern,
            is_regex: Some(false),
            since_index: None,
            live: Some(false),
        })?;
        self.wait_for_reply(self.action_timeout(), move |msg| match msg {
            HubToClient::QueryLogResults {
                query_id: id,
                entries,
                done: _,
            } if id == query_id => Some(Ok(entries)),
            _ => None,
        })
    }

    fn collect_logs_text(&self) -> TestResult<String> {
        let mut entries = self.query_logs_once(None)?;
        if entries.len() > RECENT_LOG_LINES {
            let split = entries.len() - RECENT_LOG_LINES;
            entries = entries.split_off(split);
        }
        let mut out = String::new();
        for (index, entry) in entries {
            let _ = writeln!(
                &mut out,
                "[{index}] {:?} {:?}: {}",
                entry.source, entry.level, entry.message
            );
        }
        Ok(out)
    }

    fn send(&self, msg: ClientToHub) -> TestResult<QueryId> {
        self.ensure_running()?;
        self.send_unchecked(msg)
    }

    fn send_unchecked(&self, msg: ClientToHub) -> TestResult<QueryId> {
        let mut inner = self.inner.borrow_mut();
        inner.connection.send(msg)
    }

    fn send_no_wait(&self, msg: ClientToHub) -> TestResult<()> {
        let _ = self.send(msg)?;
        Ok(())
    }

    fn try_pump_ui(&self) -> TestResult<()> {
        self.try_forward((0..PUMP_TICKS).map(|_| StudioToApp::Tick).collect())
    }

    fn wait_for_reply<T, F>(&self, timeout: Duration, mut matcher: F) -> TestResult<T>
    where
        F: FnMut(HubToClient) -> Option<TestResult<T>>,
    {
        let deadline = Instant::now() + timeout;
        loop {
            self.ensure_running()?;
            if Instant::now() >= deadline {
                return Err(TestError::new("timed out waiting for hub response"));
            }
            let slice = cmp::min(
                self.poll_interval(),
                deadline.saturating_duration_since(Instant::now()),
            );
            let Some(msg) = self.recv_timeout(slice) else {
                continue;
            };
            if let HubToClient::Error { message } = &msg {
                return Err(TestError::new(message.clone()));
            }
            if let Some(result) = matcher(msg) {
                return result;
            }
        }
    }

    fn recv_timeout(&self, timeout: Duration) -> Option<HubToClient> {
        let msg = {
            let inner = self.inner.borrow();
            inner.connection.recv_timeout(timeout)
        };
        if let Some(ref msg) = msg {
            self.inner.borrow_mut().observe_message(msg);
        }
        msg
    }

    fn build_id(&self) -> QueryId {
        self.inner.borrow().build_id
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
        let inner = self.inner.borrow();
        if let Some(exit_code) = inner.build_stopped {
            return Err(match exit_code {
                Some(code) => TestError::new(format!(
                    "app build {} exited unexpectedly with code {code}",
                    inner.build_id.0
                )),
                None => TestError::new(format!(
                    "app build {} exited unexpectedly",
                    inner.build_id.0
                )),
            });
        }
        Ok(())
    }

    fn shutdown(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        {
            let mut inner = self.inner.borrow_mut();
            if inner.build_stopped.is_some() {
                return;
            }
            let build_id = inner.build_id;
            let _ = inner.connection.send(ClientToHub::ClearBuild { build_id });
        }

        loop {
            if Instant::now() >= deadline {
                return;
            }
            {
                let inner = self.inner.borrow();
                if inner.build_stopped.is_some() {
                    return;
                }
            }
            let slice = cmp::min(
                POLL_INTERVAL,
                deadline.saturating_duration_since(Instant::now()),
            );
            let msg = {
                let inner = self.inner.borrow();
                inner.connection.recv_timeout(slice)
            };
            let Some(msg) = msg else {
                continue;
            };
            let mut inner = self.inner.borrow_mut();
            inner.observe_message(&msg);
        }
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
        let widget = self.resolve_unique_visible()?;
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
        let widget = self.resolve_unique_visible()?;
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
        let widget = self.resolve_unique_visible()?;
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
        let widget = self.resolve_unique_visible()?;
        if widget.enabled == expected {
            return Ok(());
        }
        Err(TestError::new(format!(
            "selector `{}` expected enabled state `{expected}`, found `{}`",
            self.selector.describe(),
            widget.enabled
        )))
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
            let widget = match self.resolve_unique_visible() {
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
            let widget = match self.resolve_unique_visible() {
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
        match matches.as_slice() {
            [] => Err(TestError::new(format!(
                "selector `{query}` matched no visible widgets"
            ))),
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
    let test_lock = TEST_MUTEX.get_or_init(|| Mutex::new(()));
    let _guard = test_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

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

fn start_headless_app(config: &TestConfig) -> TestResult<(TestConnection, QueryId)> {
    let mut connection = TestConnection::InProcess(
        StudioHub::start_in_process(HubConfig {
            listen_address: config.listen_address,
            mounts: vec![MountConfig {
                name: config.mount_name.clone(),
                path: config.manifest_dir.clone(),
            }],
            enable_in_process_gateway: true,
            ..Default::default()
        })
        .map_err(TestError::new)?,
    );

    let _ = connection.send(ClientToHub::Run {
        mount: config.mount_name.clone(),
        process: config.package_name.clone(),
        args: Vec::new(),
        standalone: None,
        env: Some(config.env.clone()),
        buildbox: None,
    })?;

    let build_id = wait_for_run_ready(
        &connection,
        &config.mount_name,
        &config.package_name,
        config.startup_timeout,
    )?;

    Ok((connection, build_id))
}

fn start_visible_app(config: &TestConfig) -> TestResult<(TestConnection, QueryId)> {
    let studio_addr = studio_addr_from_env();
    let mount = studio_mount_from_env();
    let mut connection = TestConnection::Remote(StudioRemoteClient::connect(&studio_addr)?);

    clear_existing_visible_builds(
        &mut connection,
        &mount,
        &config.package_name,
        config.startup_timeout,
    )?;

    let _ = connection.send(ClientToHub::Run {
        mount: mount.clone(),
        process: config.package_name.clone(),
        args: Vec::new(),
        standalone: None,
        env: Some(config.env.clone()),
        buildbox: None,
    })?;

    let build_id = wait_for_run_ready(
        &connection,
        &mount,
        &config.package_name,
        config.startup_timeout,
    )?;

    Ok((connection, build_id))
}

fn clear_existing_visible_builds(
    connection: &mut TestConnection,
    mount: &str,
    package: &str,
    timeout: Duration,
) -> TestResult<()> {
    let _ = connection.send(ClientToHub::ListBuilds)?;
    let builds = wait_for_builds(connection, timeout)?;
    for build in builds
        .into_iter()
        .filter(|build| build.mount == mount && build.package == package)
    {
        let _ = connection.send(ClientToHub::ClearBuild {
            build_id: build.build_id,
        })?;
    }
    Ok(())
}

fn wait_for_builds(
    connection: &TestConnection,
    timeout: Duration,
) -> TestResult<Vec<makepad_studio_protocol::hub_protocol::BuildInfo>> {
    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            return Err(TestError::new("timed out waiting for studio build list"));
        }
        let slice = cmp::min(
            POLL_INTERVAL,
            deadline.saturating_duration_since(Instant::now()),
        );
        let Some(msg) = connection.recv_timeout(slice) else {
            continue;
        };
        match msg {
            HubToClient::Builds { builds } => return Ok(builds),
            HubToClient::Error { message } => return Err(TestError::new(message)),
            _ => {}
        }
    }
}

fn wait_for_run_ready(
    connection: &TestConnection,
    mount: &str,
    package: &str,
    timeout: Duration,
) -> TestResult<QueryId> {
    let deadline = Instant::now() + timeout;
    let mut build_started = None;
    let mut app_started = None;

    loop {
        if let (Some(build_id), Some(app_build_id)) = (build_started, app_started) {
            if build_id == app_build_id {
                return Ok(build_id);
            }
        }
        if Instant::now() >= deadline {
            return Err(TestError::new(format!(
                "timed out waiting for `{package}` to start"
            )));
        }
        let slice = cmp::min(
            POLL_INTERVAL,
            deadline.saturating_duration_since(Instant::now()),
        );
        let Some(msg) = connection.recv_timeout(slice) else {
            continue;
        };
        match msg {
            HubToClient::BuildStarted {
                build_id,
                mount: msg_mount,
                package: msg_package,
            } if msg_mount == mount && msg_package == package => {
                build_started = Some(build_id);
                if app_started == Some(build_id) {
                    return Ok(build_id);
                }
            }
            HubToClient::AppStarted { build_id } => {
                app_started = Some(build_id);
                if build_started == Some(build_id) {
                    return Ok(build_id);
                }
            }
            HubToClient::BuildStopped {
                build_id,
                exit_code,
            } => {
                let detail = match exit_code {
                    Some(code) => {
                        format!("build {build_id:?} exited with code {code} before startup")
                    }
                    None => format!("build {build_id:?} exited before startup"),
                };
                return Err(TestError::new(detail));
            }
            HubToClient::Error { message } => return Err(TestError::new(message)),
            _ => {}
        }
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

fn startup_error_is_retryable(err: &TestError) -> bool {
    err.message().contains("before startup")
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

fn visible_mode_enabled() -> bool {
    env_truthy("MAKEPAD_TEST_VISIBLE")
}

fn studio_addr_from_env() -> String {
    std::env::var("MAKEPAD_TEST_STUDIO")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_STUDIO_ADDR.to_string())
}

fn studio_mount_from_env() -> String {
    std::env::var("MAKEPAD_TEST_STUDIO_MOUNT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_STUDIO_MOUNT.to_string())
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

fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
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
        env_duration_ms, primary_window_scope, sanitize_path_component, snapshot_is_visible,
        snapshot_sort_key, studio_addr_from_env, studio_mount_from_env, visible_mode_enabled,
        TestError, TestResult, WidgetMatch,
    };
    use crate::{Selector, TestConfig};
    use makepad_studio_protocol::WidgetSnapshot;
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
    fn config_uses_expected_artifact_dir() {
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
        assert_eq!(config.env.get("MAKEPAD"), Some(&"headless".to_string()));
    }

    #[test]
    fn visible_mode_omits_headless_env() {
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

        assert!(!config.env.contains_key("MAKEPAD"));
    }

    #[test]
    fn visible_mode_uses_expected_studio_defaults() {
        let _guard = ENV_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_studio = std::env::var("MAKEPAD_TEST_STUDIO").ok();
        let old_mount = std::env::var("MAKEPAD_TEST_STUDIO_MOUNT").ok();
        std::env::remove_var("MAKEPAD_TEST_STUDIO");
        std::env::remove_var("MAKEPAD_TEST_STUDIO_MOUNT");

        assert_eq!(studio_addr_from_env(), "127.0.0.1:8001");
        assert_eq!(studio_mount_from_env(), "makepad");
        restore_env_var("MAKEPAD_TEST_STUDIO", old_studio);
        restore_env_var("MAKEPAD_TEST_STUDIO_MOUNT", old_mount);
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
}

use crate::error::{IntoTestResult, TestError, TestResult};
use crate::selector::Selector;
use makepad_micro_serde::SerBin;
use makepad_studio_hub::{HubConfig, HubConnection, MountConfig, StudioHub};
use makepad_studio_protocol::hub_protocol::{ClientToHub, HubToClient, LogEntry, QueryId};
use makepad_studio_protocol::{StudioToApp, StudioToAppVec};
use std::cell::RefCell;
use std::cmp;
use std::collections::HashMap;
use std::fs;
use std::net::{Ipv4Addr, SocketAddr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(600);
const ACTION_TIMEOUT: Duration = Duration::from_secs(5);
const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(20);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

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
            .join("makepad-ui-tests")
            .join(sanitize_path_component(&package_name))
            .join(sanitize_path_component(&test_name));

        let mut env = HashMap::new();
        env.insert("MAKEPAD".to_string(), "headless".to_string());
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

struct TestAppInner {
    config: TestConfig,
    connection: HubConnection,
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
        fs::create_dir_all(&config.artifacts_dir)?;

        let mut connection = StudioHub::start_in_process(HubConfig {
            listen_address: config.listen_address,
            mounts: vec![MountConfig {
                name: config.mount_name.clone(),
                path: config.manifest_dir.clone(),
            }],
            enable_in_process_gateway: true,
            ..Default::default()
        })
        .map_err(TestError::new)?;

        let _query_id = connection.send(ClientToHub::Run {
            mount: config.mount_name.clone(),
            process: config.package_name.clone(),
            args: Vec::new(),
            standalone: None,
            env: Some(config.env.clone()),
            buildbox: None,
        });

        let build_id = wait_for_run_ready(
            &connection,
            &config.mount_name,
            &config.package_name,
            config.startup_timeout,
        )?;

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
        self.send_no_wait(ClientToHub::TypeText { build_id, text })
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
        })
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
        let build_id = self.build_id();
        let query_id = self.send(ClientToHub::WidgetTreeDump { build_id })?;
        self.wait_for_reply(self.action_timeout(), move |msg| match msg {
            HubToClient::WidgetTreeDump {
                query_id: id, dump, ..
            } if id == query_id => Some(Ok(dump)),
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

    fn try_click_center(&self, target: &WidgetMatch) -> TestResult<()> {
        self.ensure_running()?;
        let (x, y) = target.center();
        let build_id = self.build_id();
        self.send_no_wait(ClientToHub::Click { build_id, x, y })
    }

    fn query_visible_widgets(&self, selector: &Selector) -> TestResult<Vec<WidgetMatch>> {
        self.ensure_running()?;
        let build_id = self.build_id();
        let query = selector.as_query();
        let query_id = self.send(ClientToHub::WidgetQuery {
            build_id,
            query: query.clone(),
        })?;
        self.wait_for_reply(self.action_timeout(), move |msg| match msg {
            HubToClient::WidgetQuery {
                query_id: id,
                rects,
                ..
            } if id == query_id => Some(Ok(rects
                .into_iter()
                .filter_map(|line| WidgetMatch::parse(&line))
                .collect())),
            _ => None,
        })
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
        let entries = self.query_logs_once(None)?;
        let mut out = String::new();
        for (index, entry) in entries {
            out.push_str(&format!(
                "[{index}] {:?} {:?}: {}\n",
                entry.source, entry.level, entry.message
            ));
        }
        Ok(out)
    }

    fn send(&self, msg: ClientToHub) -> TestResult<QueryId> {
        self.ensure_running()?;
        self.send_unchecked(msg)
    }

    fn send_unchecked(&self, msg: ClientToHub) -> TestResult<QueryId> {
        let mut inner = self.inner.borrow_mut();
        Ok(inner.connection.send(msg))
    }

    fn send_no_wait(&self, msg: ClientToHub) -> TestResult<()> {
        let _ = self.send(msg)?;
        Ok(())
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
        let mut inner = self.inner.borrow_mut();
        let build_id = inner.build_id;
        let _ = inner.connection.send(ClientToHub::ClearBuild { build_id });
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
        let query = self.selector.as_query();
        let deadline = Instant::now() + self.app.action_timeout();
        while Instant::now() < deadline {
            if !self.app.query_visible_widgets(&self.selector)?.is_empty() {
                return Ok(());
            }
            thread::sleep(self.app.poll_interval());
        }
        Err(TestError::new(format!(
            "timed out waiting for selector `{query}` to become visible"
        )))
    }

    pub fn click(self) -> Self {
        if let Err(err) = self.try_click() {
            panic_for_error(err);
        }
        self
    }

    pub fn try_click(&self) -> TestResult<()> {
        let target = self.resolve_unique()?;
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

    fn resolve_unique(&self) -> TestResult<WidgetMatch> {
        let query = self.selector.as_query();
        let matches = self.app.query_visible_widgets(&self.selector)?;
        match matches.as_slice() {
            [] => Err(TestError::new(format!(
                "selector `{query}` matched no visible widgets"
            ))),
            [single] => Ok(single.clone()),
            _ => Err(TestError::new(format!(
                "selector `{query}` matched multiple widgets:\n{}",
                matches
                    .iter()
                    .map(|item| item.raw.as_str())
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
            app.shutdown();
            Ok(())
        }
        Ok(Err(err)) => {
            capture_failure_artifacts(&app, err.message());
            app.shutdown();
            Err(err)
        }
        Err(payload) => {
            let err = TestError::from_panic_payload(payload);
            capture_failure_artifacts(&app, err.message());
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

fn wait_for_run_ready(
    connection: &HubConnection,
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

#[cfg(test)]
mod tests {
    use super::{sanitize_path_component, TestError, TestResult, WidgetMatch};
    use crate::{Selector, TestConfig};
    use std::path::PathBuf;

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
        let config =
            TestConfig::current_package("/tmp/example", "makepad-example", "ui::test").unwrap();
        assert_eq!(
            config.artifacts_dir,
            PathBuf::from("/tmp/example")
                .join("target")
                .join("makepad-ui-tests")
                .join("makepad-example")
                .join("ui__test")
        );
        assert_eq!(config.env.get("MAKEPAD"), Some(&"headless".to_string()));
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
}

use crate::error::TestResult;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

pub(super) const STARTUP_TIMEOUT: Duration = Duration::from_secs(600);
pub(super) const ACTION_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(50);
pub(super) const SPLASH_RUNNABLE: &str = "makepad.splash";

#[derive(Clone, Debug)]
pub(super) struct SplashLaunchTarget {
    pub(super) root_package: String,
    pub(super) visible_run_item: String,
    pub(super) headless_run_item: String,
    pub(super) child_package: String,
}

#[derive(Clone, Debug)]
pub(super) enum TestLaunch {
    CurrentPackage,
    SplashRunItem(SplashLaunchTarget),
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WidgetMatch {
    pub raw: String,
    pub id: String,
    pub widget_type: String,
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

#[cfg(test)]
impl WidgetMatch {
    pub(super) fn parse(line: &str) -> Option<Self> {
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

    pub(super) fn center(&self) -> (i64, i64) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }
}

#[derive(Clone, Debug)]
pub struct TestConfig {
    pub package_name: String,
    pub mount_name: String,
    pub manifest_dir: PathBuf,
    pub mount_root: PathBuf,
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
    pub(super) launch: TestLaunch,
}

impl TestConfig {
    fn new_for_launch(
        manifest_dir: impl Into<PathBuf>,
        mount_root: impl Into<PathBuf>,
        package_name: impl Into<String>,
        test_name: impl Into<String>,
        launch: TestLaunch,
    ) -> TestResult<Self> {
        let manifest_dir = manifest_dir.into();
        let mount_root = mount_root.into();
        let package_name = package_name.into();
        let test_name = test_name.into();
        let artifacts_dir = manifest_dir
            .join("target")
            .join("makepad_test")
            .join(sanitize_path_component(&package_name))
            .join(sanitize_path_component(&test_name));

        let mut env = HashMap::new();
        if !super::visible_mode_enabled() {
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
            mount_root,
            test_name,
            artifacts_dir,
            listen_address: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            env,
            startup_timeout: STARTUP_TIMEOUT,
            action_timeout: ACTION_TIMEOUT,
            poll_interval: POLL_INTERVAL,
            startup_pause: super::env_duration_ms("MAKEPAD_TEST_STARTUP_DELAY_MS"),
            action_delay: super::env_duration_ms("MAKEPAD_TEST_ACTION_DELAY_MS"),
            keep_open: super::env_duration_ms("MAKEPAD_TEST_KEEP_OPEN_MS"),
            launch,
        })
    }

    pub fn new(
        manifest_dir: impl Into<PathBuf>,
        package_name: impl Into<String>,
        test_name: impl Into<String>,
    ) -> TestResult<Self> {
        let manifest_dir = manifest_dir.into();
        Self::new_for_launch(
            manifest_dir.clone(),
            manifest_dir,
            package_name,
            test_name,
            TestLaunch::CurrentPackage,
        )
    }

    pub fn current_package(
        manifest_dir: impl Into<PathBuf>,
        package_name: impl Into<String>,
        test_name: impl Into<String>,
    ) -> TestResult<Self> {
        Self::new(manifest_dir, package_name, test_name)
    }

    pub fn splash_run_item(
        mount_root: impl Into<PathBuf>,
        manifest_dir: impl Into<PathBuf>,
        package_name: impl Into<String>,
        test_name: impl Into<String>,
        visible_run_item: impl Into<String>,
        headless_run_item: impl Into<String>,
    ) -> TestResult<Self> {
        let manifest_dir = manifest_dir.into();
        let mount_root = mount_root.into();
        let package_name = package_name.into();
        Self::new_for_launch(
            manifest_dir,
            mount_root,
            package_name.clone(),
            test_name,
            TestLaunch::SplashRunItem(SplashLaunchTarget {
                root_package: SPLASH_RUNNABLE.to_string(),
                visible_run_item: visible_run_item.into(),
                headless_run_item: headless_run_item.into(),
                child_package: package_name,
            }),
        )
    }

    pub fn with_artifacts_dir(mut self, artifacts_dir: impl Into<PathBuf>) -> Self {
        self.artifacts_dir = artifacts_dir.into();
        self
    }
}

pub(crate) fn sanitize_path_component(value: &str) -> String {
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

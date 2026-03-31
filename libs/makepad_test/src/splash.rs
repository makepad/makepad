use crate::report::{
    write_case_outputs, write_suite_outputs, CaseReport, StepEvidence, SuiteReport, TraceStep,
};
use crate::runtime::{
    capture_failure_artifacts_to, run_with_config, sanitize_path_component, StepScreenshotPolicy,
};
use crate::selector::SelectorOptions;
use crate::{Selector, TestApp, TestConfig, TestError, TestResult};
use makepad_script_std::makepad_network::{NetworkConfig, NetworkRuntime};
use makepad_script_std::makepad_script::*;
use makepad_script_std::{script_mod as script_std_mod, with_vm_and_async, ScriptStd};
use makepad_studio_protocol::hub_protocol::LogEntry;
use makepad_studio_protocol::{KeyCode, KeyModifiers, WidgetSnapshot};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
enum SplashLaunch {
    CurrentPackage,
    SplashRunItem {
        visible_run_item: String,
        headless_run_item: String,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SessionMode {
    #[default]
    Isolated,
    Shared,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SplashSuiteOptions {
    launch: Option<SplashLaunch>,
    session_mode: SessionMode,
    step_screenshot_policy: Option<StepScreenshotPolicy>,
    startup_timeout: Option<Duration>,
    action_timeout: Option<Duration>,
    poll_interval: Option<Duration>,
    startup_delay: Option<Duration>,
    action_delay: Option<Duration>,
    keep_open: Option<Duration>,
}

#[derive(Clone)]
struct SplashCase {
    name: String,
    function: ScriptObjectRef,
}

struct SplashSuiteHost {
    suite_path: PathBuf,
    options: Option<SplashSuiteOptions>,
    cases: HashMap<String, SplashCase>,
    case_order: Vec<String>,
    current_app: Option<TestApp>,
    last_error_message: Option<String>,
    current_case: Option<RunningCase>,
}

impl SplashSuiteHost {
    fn new(suite_path: PathBuf) -> Self {
        Self {
            suite_path,
            options: None,
            cases: HashMap::new(),
            case_order: Vec::new(),
            current_app: None,
            last_error_message: None,
            current_case: None,
        }
    }

    fn register_options(&mut self, options: SplashSuiteOptions) -> TestResult<()> {
        if self.options.is_some() {
            return Err(TestError::new(format!(
                "`test.configure(...)` was called more than once in {}",
                self.suite_path.display()
            )));
        }
        self.options = Some(options);
        Ok(())
    }

    fn register_case(&mut self, name: String, function: ScriptObjectRef) -> TestResult<()> {
        if self.cases.contains_key(&name) {
            return Err(TestError::new(format!(
                "duplicate Splash test case `{name}` in {}",
                self.suite_path.display()
            )));
        }
        self.cases.insert(
            name.clone(),
            SplashCase { name: name.clone(), function },
        );
        self.case_order.push(name);
        Ok(())
    }

    fn case(&self, name: &str) -> TestResult<SplashCase> {
        self.cases.get(name).cloned().ok_or_else(|| {
            TestError::new(format!(
                "Splash test case `{name}` was not registered in {}",
                self.suite_path.display()
            ))
        })
    }

    fn clear_last_error_message(&mut self) {
        self.last_error_message = None;
    }

    fn set_last_error_message(&mut self, message: String) {
        self.last_error_message = Some(message);
    }

    fn take_last_error_message(&mut self) -> Option<String> {
        self.last_error_message.take()
    }

    fn begin_case(&mut self, name: &str, artifact_dir: PathBuf, started_at_ms: u64) {
        self.current_case = Some(RunningCase {
            name: name.to_string(),
            artifact_dir,
            started_at_ms,
            start_instant: Instant::now(),
            steps: Vec::new(),
            frame_paths: Vec::new(),
            next_step_index: 1,
        });
    }

    fn finish_case(&mut self, status: &str, failure_message: Option<String>) -> FinishedCase {
        let case = self.current_case.take().unwrap_or_else(|| RunningCase {
            name: "unknown".to_string(),
            artifact_dir: PathBuf::new(),
            started_at_ms: 0,
            start_instant: Instant::now(),
            steps: Vec::new(),
            frame_paths: Vec::new(),
            next_step_index: 1,
        });
        FinishedCase {
            report: CaseReport {
                case_name: case.name,
                status: status.to_string(),
                started_at_ms: case.started_at_ms,
                finished_at_ms: case.started_at_ms + duration_ms(case.start_instant.elapsed()),
                duration_ms: duration_ms(case.start_instant.elapsed()),
                artifact_dir: case.artifact_dir.to_string_lossy().to_string(),
                failure_message,
                session_apng_path: None,
                generated_case_path: None,
                steps: case.steps,
            },
            frame_paths: case.frame_paths,
        }
    }

    fn begin_step(
        &mut self,
        app: &TestApp,
        kind: &str,
        detail: String,
        selector: Option<&Selector>,
    ) -> PendingTraceStep {
        let case = self
            .current_case
            .as_mut()
            .expect("begin_step called without an active Splash case");
        let index = case.next_step_index;
        case.next_step_index += 1;
        PendingTraceStep {
            index,
            kind: kind.to_string(),
            detail,
            started_at_ms: duration_ms(case.start_instant.elapsed()),
            started_at: Instant::now(),
            evidence: StepEvidence {
                selector_query: selector.map(Selector::describe),
                before_widgets: selector
                    .and_then(|selector| app.try_query_widgets(selector, false).ok()),
                after_widgets: None,
                screenshot_path: None,
                log_excerpt: None,
                widget_dump_excerpt: None,
            },
        }
    }

    fn finish_step<T>(
        &mut self,
        app: &TestApp,
        selector: Option<&Selector>,
        pending: PendingTraceStep,
        result: TestResult<T>,
        extra_evidence: StepEvidence,
    ) -> TestResult<T> {
        let case = self
            .current_case
            .as_mut()
            .expect("finish_step called without an active Splash case");
        let PendingTraceStep {
            index,
            kind,
            detail,
            started_at_ms,
            started_at,
            mut evidence,
        } = pending;
        evidence = merge_evidence(evidence, extra_evidence);
        if selector.is_some() && evidence.after_widgets.is_none() {
            evidence.after_widgets =
                selector.and_then(|selector| app.try_query_widgets(selector, false).ok());
        }
        let finished_at_ms = duration_ms(case.start_instant.elapsed());
        let step_screenshot_policy = app.step_screenshot_policy();
        let step = match result {
            Ok(value) => {
                if let Some(path) = frame_path_for_step(
                    app,
                    case,
                    index,
                    &kind,
                    &mut evidence,
                    step_screenshot_policy,
                    false,
                ) {
                    case.frame_paths.push(path);
                }
                case.steps.push(TraceStep {
                    index,
                    kind,
                    detail,
                    started_at_ms,
                    finished_at_ms,
                    duration_ms: duration_ms(started_at.elapsed()),
                    status: "passed".to_string(),
                    error_message: None,
                    evidence,
                });
                return Ok(value);
            }
            Err(err) => {
                if evidence.log_excerpt.is_none() {
                    evidence.log_excerpt = app.try_collect_logs_excerpt(None, 40).ok();
                }
                if evidence.widget_dump_excerpt.is_none() {
                    evidence.widget_dump_excerpt = app
                        .try_widget_dump()
                        .ok()
                        .map(|dump| truncate_text(&dump, 2000));
                }
                if let Some(path) = frame_path_for_step(
                    app,
                    case,
                    index,
                    &kind,
                    &mut evidence,
                    step_screenshot_policy,
                    true,
                ) {
                    case.frame_paths.push(path);
                }
                TraceStep {
                    index,
                    kind,
                    detail,
                    started_at_ms,
                    finished_at_ms,
                    duration_ms: duration_ms(started_at.elapsed()),
                    status: "failed".to_string(),
                    error_message: Some(err.message().to_string()),
                    evidence,
                }
            }
        };
        let error = step
            .error_message
            .clone()
            .unwrap_or_else(|| "step failed".to_string());
        case.steps.push(step);
        Err(TestError::new(error))
    }
}

struct RunningCase {
    name: String,
    artifact_dir: PathBuf,
    started_at_ms: u64,
    start_instant: Instant,
    steps: Vec<TraceStep>,
    frame_paths: Vec<PathBuf>,
    next_step_index: usize,
}

struct PendingTraceStep {
    index: usize,
    kind: String,
    detail: String,
    started_at_ms: u64,
    started_at: Instant,
    evidence: StepEvidence,
}

struct CaseRunOutcome {
    report: CaseReport,
    frame_paths: Vec<PathBuf>,
    error: Option<TestError>,
}

struct FinishedCase {
    report: CaseReport,
    frame_paths: Vec<PathBuf>,
}

struct CapturedStepFrame {
    apng_path: PathBuf,
    report_screenshot_path: Option<String>,
}

pub struct SplashSuiteRunner {
    manifest_dir: PathBuf,
    host: SplashSuiteHost,
    std: ScriptStd,
    script_vm: Option<Box<ScriptVmBase>>,
}

impl SplashSuiteRunner {
    pub fn load(
        manifest_dir: impl Into<PathBuf>,
        suite_path: impl AsRef<Path>,
    ) -> TestResult<Self> {
        let manifest_dir = manifest_dir.into();
        let suite_path = resolve_suite_path(&manifest_dir, suite_path.as_ref());
        let source = fs::read_to_string(&suite_path).map_err(|err| {
            TestError::new(format!(
                "failed to read Splash suite {}: {err}",
                suite_path.display()
            ))
        })?;
        Self::load_from_source(manifest_dir, suite_path, source)
    }

    fn load_from_source(
        manifest_dir: PathBuf,
        suite_path: PathBuf,
        source: String,
    ) -> TestResult<Self> {
        let runtime = Arc::new(NetworkRuntime::new(NetworkConfig::default()));
        let mut host = SplashSuiteHost::new(suite_path.clone());
        let mut std = ScriptStd::with_network_runtime(runtime);
        let mut script_vm = Some(Box::new(ScriptVmBase::new()));
        let suite_file_name = suite_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("ui.splash");
        let script_mod = ScriptMod {
            cargo_manifest_path: manifest_dir.to_string_lossy().to_string(),
            module_path: format!("makepad_test::{suite_file_name}"),
            file: suite_path.to_string_lossy().to_string(),
            line: 1,
            column: 1,
            code: normalize_script_source(&source),
            values: Vec::new(),
        };

        host.clear_last_error_message();
        let result = with_vm_and_async(&mut host, &mut std, &mut script_vm, |vm| {
            script_std_mod(vm);
            install_test_script_module(vm);
            vm.eval(script_mod)
        });

        if result.is_err() {
            let message = host.take_last_error_message().unwrap_or_else(|| {
                with_vm_and_async(&mut host, &mut std, &mut script_vm, |vm| {
                    script_value_to_string(vm, result)
                })
            });
            return Err(TestError::new(format!(
                "failed to evaluate Splash suite {}: {}",
                suite_path.display(),
                message
            )));
        }

        Ok(Self {
            manifest_dir,
            host,
            std,
            script_vm,
        })
    }

    pub fn test_config(&self, package_name: &str, test_name: &str) -> TestResult<TestConfig> {
        let options = self.host.options.clone().unwrap_or_default();
        let launch = options.launch.unwrap_or(SplashLaunch::CurrentPackage);

        let mut config = match launch {
            SplashLaunch::CurrentPackage => {
                TestConfig::current_package(&self.manifest_dir, package_name, test_name)?
            }
            SplashLaunch::SplashRunItem {
                visible_run_item,
                headless_run_item,
            } => {
                let mount_root = discover_splash_mount_root(&self.manifest_dir)?;
                TestConfig::splash_run_item(
                    mount_root,
                    &self.manifest_dir,
                    package_name,
                    test_name,
                    visible_run_item,
                    headless_run_item,
                )?
            }
        };

        if let Some(value) = options.startup_timeout {
            config.startup_timeout = value;
        }
        if let Some(value) = options.action_timeout {
            config.action_timeout = value;
        }
        if let Some(value) = options.poll_interval {
            config.poll_interval = value;
        }
        if let Some(value) = options.startup_delay {
            config.startup_pause = value;
        }
        if let Some(value) = options.action_delay {
            config.action_delay = value;
        }
        if let Some(value) = options.keep_open {
            config.keep_open = value;
        }
        if let Some(value) = options.step_screenshot_policy {
            config.step_screenshot_policy = value;
        }

        Ok(config)
    }

    fn run_case_with_report(
        &mut self,
        case_name: &str,
        app: TestApp,
        artifact_dir: PathBuf,
        suite_started_at_ms: u64,
    ) -> CaseRunOutcome {
        let case = match self.host.case(case_name) {
            Ok(case) => case,
            Err(err) => {
                let report = CaseReport {
                    case_name: case_name.to_string(),
                    status: "failed".to_string(),
                    started_at_ms: suite_started_at_ms,
                    finished_at_ms: suite_started_at_ms,
                    duration_ms: 0,
                    artifact_dir: artifact_dir.to_string_lossy().to_string(),
                    failure_message: Some(err.message().to_string()),
                    session_apng_path: None,
                    generated_case_path: None,
                    steps: Vec::new(),
                };
                return CaseRunOutcome {
                    report,
                    frame_paths: Vec::new(),
                    error: Some(err),
                };
            }
        };
        self.host.clear_last_error_message();
        self.host
            .begin_case(&case.name, artifact_dir, suite_started_at_ms);
        self.host.current_app = Some(app);
        let result = with_vm_and_async(&mut self.host, &mut self.std, &mut self.script_vm, |vm| {
            vm.call(case.function.clone().into(), &[])
        });
        self.host.current_app = None;

        if result.is_err() {
            let message = self.host.take_last_error_message().unwrap_or_else(|| {
                with_vm_and_async(&mut self.host, &mut self.std, &mut self.script_vm, |vm| {
                    script_value_to_string(vm, result)
                })
            });
            let error = TestError::new(format!(
                "Splash test case `{}` failed: {}",
                case.name, message
            ));
            let finished = self
                .host
                .finish_case("failed", Some(error.message().to_string()));
            return CaseRunOutcome {
                report: finished.report,
                frame_paths: finished.frame_paths,
                error: Some(error),
            };
        }
        let finished = self.host.finish_case("passed", None);
        CaseRunOutcome {
            report: finished.report,
            frame_paths: finished.frame_paths,
            error: None,
        }
    }

    #[cfg(test)]
    fn options(&self) -> Option<&SplashSuiteOptions> {
        self.host.options.as_ref()
    }

    fn case_names(&self) -> Vec<String> {
        self.host.case_order.clone()
    }

    fn session_mode(&self) -> SessionMode {
        self.host
            .options
            .as_ref()
            .map(|options| options.session_mode)
            .unwrap_or_default()
    }
}

pub fn run_splash_suite(
    package_name: &str,
    manifest_dir: &str,
    module_path: &str,
    suite_path: &str,
) -> TestResult<()> {
    let mut runner = SplashSuiteRunner::load(manifest_dir, suite_path)?;
    let case_names = runner.case_names();
    if case_names.is_empty() {
        return Err(TestError::new(format!(
            "Splash suite `{}` did not register any test cases",
            resolve_suite_path(Path::new(manifest_dir), Path::new(suite_path)).display()
        )));
    }

    let suite_test_name = if module_path.is_empty() {
        "splash_suite".to_string()
    } else {
        format!("{module_path}::splash_suite")
    };
    let suite_dir = splash_suite_dir(manifest_dir, package_name, &suite_test_name);
    if suite_dir.exists() {
        let _ = fs::remove_dir_all(&suite_dir);
    }
    fs::create_dir_all(suite_dir.join("cases"))?;
    let session_mode = runner.session_mode();
    let session_mode_name = match session_mode {
        SessionMode::Isolated => "isolated",
        SessionMode::Shared => "shared",
    };
    let wall = Instant::now();
    let suite_start = Instant::now();
    let mut case_reports = Vec::new();
    let mut suite_failure = None;
    let result = match session_mode {
        SessionMode::Isolated => run_isolated_splash_suite(
            &mut runner,
            package_name,
            &suite_test_name,
            &suite_dir,
            &case_names,
            &mut case_reports,
            &mut suite_failure,
            wall,
        ),
        SessionMode::Shared => run_shared_splash_suite(
            &mut runner,
            package_name,
            &suite_test_name,
            &suite_dir,
            &case_names,
            &mut case_reports,
            &mut suite_failure,
            wall,
        ),
    };
    let suite_report = SuiteReport {
        suite_id: sanitize_path_component(&suite_test_name),
        session_mode: session_mode_name.to_string(),
        started_at_ms: 0,
        finished_at_ms: duration_ms(suite_start.elapsed()),
        duration_ms: duration_ms(suite_start.elapsed()),
        status: if suite_failure.is_some() {
            "failed".to_string()
        } else {
            "passed".to_string()
        },
        failure_message: suite_failure.clone(),
        generated_case_path: None,
        cases: case_reports,
    };
    write_suite_outputs(&suite_dir, &suite_report)?;
    eprintln!(
        "[makepad_test] splash: total {:.2}s (startup + case bodies + teardown — explains cargo test wall time vs case sum)",
        wall.elapsed().as_secs_f64()
    );
    result
}

fn startup_failure_case_outcome(
    case_name: &str,
    artifact_dir: PathBuf,
    started_at_ms: u64,
    duration: Duration,
    error: TestError,
) -> CaseRunOutcome {
    let duration_ms = duration_ms(duration);
    let failure_message = error.message().to_string();
    CaseRunOutcome {
        report: CaseReport {
            case_name: case_name.to_string(),
            status: "failed".to_string(),
            started_at_ms,
            finished_at_ms: started_at_ms + duration_ms,
            duration_ms,
            artifact_dir: artifact_dir.to_string_lossy().to_string(),
            failure_message: Some(failure_message),
            session_apng_path: None,
            generated_case_path: None,
            steps: Vec::new(),
        },
        frame_paths: Vec::new(),
        error: Some(error),
    }
}

fn run_isolated_splash_suite(
    runner: &mut SplashSuiteRunner,
    package_name: &str,
    suite_test_name: &str,
    suite_dir: &Path,
    case_names: &[String],
    case_reports: &mut Vec<CaseReport>,
    suite_failure: &mut Option<String>,
    wall: Instant,
) -> TestResult<()> {
    let total = case_names.len();
    for (index, case_name) in case_names.iter().enumerate() {
        let case_dir = suite_dir
            .join("cases")
            .join(sanitize_path_component(case_name));
        let config = runner
            .test_config(package_name, suite_test_name)?
            .with_artifacts_dir(case_dir.clone());
        eprintln!(
            "[makepad_test] splash case {}/{}: {} …",
            index + 1,
            total,
            case_name
        );
        let case_start = Instant::now();
        let suite_started_at_ms = duration_ms(wall.elapsed());
        let mut outcome = None;
        let result = run_with_config(config, |app| {
            outcome = Some(runner.run_case_with_report(
                case_name,
                app,
                case_dir.clone(),
                suite_started_at_ms,
            ));
            if let Some(err) = outcome.as_ref().and_then(|outcome| outcome.error.clone()) {
                Err(err)
            } else {
                Ok(())
            }
        });
        let synthesized_startup_failure = outcome.is_none();
        let mut outcome = match outcome {
            Some(outcome) => outcome,
            None => {
                let error = match &result {
                    Err(err) => err.clone(),
                    Ok(()) => TestError::new(format!(
                        "Splash isolated case `{case_name}` finished without recording an outcome"
                    )),
                };
                startup_failure_case_outcome(
                    case_name,
                    case_dir.clone(),
                    suite_started_at_ms,
                    case_start.elapsed(),
                    error,
                )
            }
        };
        if synthesized_startup_failure {
            fs::create_dir_all(&case_dir)?;
            if let Some(message) = &outcome.report.failure_message {
                fs::write(case_dir.join("failure.txt"), message)?;
            }
        }
        write_case_outputs(&case_dir, &mut outcome.report, &outcome.frame_paths)?;
        case_reports.push(outcome.report);
        match result {
            Ok(()) => {
                eprintln!(
                    "[makepad_test] splash case {}/{}: {} ok ({:.2}s)",
                    index + 1,
                    total,
                    case_name,
                    case_start.elapsed().as_secs_f64()
                );
            }
            Err(err) => {
                eprintln!(
                    "[makepad_test] splash case {}/{}: {} FAILED after {:.2}s — {}",
                    index + 1,
                    total,
                    case_name,
                    case_start.elapsed().as_secs_f64(),
                    err.message()
                );
                *suite_failure = Some(err.message().to_string());
                return Err(err);
            }
        }
    }
    Ok(())
}

fn run_shared_splash_suite(
    runner: &mut SplashSuiteRunner,
    package_name: &str,
    suite_test_name: &str,
    suite_dir: &Path,
    case_names: &[String],
    case_reports: &mut Vec<CaseReport>,
    suite_failure: &mut Option<String>,
    wall: Instant,
) -> TestResult<()> {
    let config = runner
        .test_config(package_name, suite_test_name)?
        .with_artifacts_dir(suite_dir.to_path_buf());
    run_with_config(config, |app| -> TestResult<()> {
        eprintln!(
            "[makepad_test] splash: app ready after {:.2}s (hub + build + launch; not part of case timings below)",
            wall.elapsed().as_secs_f64()
        );
        let total = case_names.len();
        let session_start = Instant::now();
        for (index, case_name) in case_names.iter().enumerate() {
            let case_dir = suite_dir
                .join("cases")
                .join(sanitize_path_component(case_name));
            eprintln!(
                "[makepad_test] splash case {}/{}: {} …",
                index + 1,
                total,
                case_name
            );
            let case_start = Instant::now();
            let mut outcome = runner.run_case_with_report(
                case_name,
                app.clone(),
                case_dir.clone(),
                duration_ms(wall.elapsed()),
            );
            if let Some(err) = outcome.error.clone() {
                capture_failure_artifacts_to(&app, &case_dir, err.message());
            }
            write_case_outputs(&case_dir, &mut outcome.report, &outcome.frame_paths)?;
            case_reports.push(outcome.report);
            if let Some(err) = outcome.error {
                eprintln!(
                    "[makepad_test] splash case {}/{}: {} FAILED after {:.2}s — {}",
                    index + 1,
                    total,
                    case_name,
                    case_start.elapsed().as_secs_f64(),
                    err.message()
                );
                *suite_failure = Some(err.message().to_string());
                return Err(err);
            }
            eprintln!(
                "[makepad_test] splash case {}/{}: {} ok ({:.2}s)",
                index + 1,
                total,
                case_name,
                case_start.elapsed().as_secs_f64()
            );
        }
        eprintln!(
            "[makepad_test] splash suite: {} cases ran in {:.2}s (Splash `test.case` bodies only)",
            total,
            session_start.elapsed().as_secs_f64()
        );
        Ok(())
    })
}

fn splash_suite_dir(manifest_dir: &str, package_name: &str, suite_test_name: &str) -> PathBuf {
    Path::new(manifest_dir)
        .join("target")
        .join("makepad_test")
        .join(sanitize_path_component(package_name))
        .join(sanitize_path_component(suite_test_name))
}

fn require_current_app(vm: &mut ScriptVm, method: &str) -> TestResult<TestApp> {
    current_app(vm).ok_or_else(|| {
        TestError::new(format!(
            "{method} can only be used while a Splash test case is running"
        ))
    })
}

fn trace_call<T, F, G>(
    vm: &mut ScriptVm,
    method: &str,
    kind: &str,
    detail: String,
    selector: Option<Selector>,
    call: F,
    extra_evidence: G,
) -> TestResult<T>
where
    F: FnOnce(TestApp) -> TestResult<T>,
    G: FnOnce(&TestApp, &T) -> StepEvidence,
{
    let app = require_current_app(vm, method)?;
    let pending = {
        let host = vm.host.downcast_mut::<SplashSuiteHost>().unwrap();
        host.begin_step(&app, kind, detail, selector.as_ref())
    };
    let result = call(app.clone());
    let success_evidence = result
        .as_ref()
        .ok()
        .map(|value| extra_evidence(&app, value))
        .unwrap_or_default();
    let host = vm.host.downcast_mut::<SplashSuiteHost>().unwrap();
    host.finish_step(&app, selector.as_ref(), pending, result, success_evidence)
}

fn trace_action<F>(
    vm: &mut ScriptVm,
    method: &str,
    kind: &str,
    detail: String,
    selector: Option<Selector>,
    call: F,
) -> ScriptValue
where
    F: FnOnce(TestApp) -> TestResult<()>,
{
    match trace_call(vm, method, kind, detail, selector, call, |_app, _value| {
        StepEvidence::default()
    }) {
        Ok(()) => NIL,
        Err(err) => host_script_error(vm, err.message()),
    }
}

fn frame_path_for_step(
    app: &TestApp,
    case: &RunningCase,
    index: usize,
    kind: &str,
    evidence: &mut StepEvidence,
    step_screenshot_policy: StepScreenshotPolicy,
    failed: bool,
) -> Option<PathBuf> {
    if let Some(path) = &evidence.screenshot_path {
        return Some(PathBuf::from(path));
    }
    let retain_screenshot = match step_screenshot_policy {
        StepScreenshotPolicy::All => true,
        StepScreenshotPolicy::Failures => failed,
        StepScreenshotPolicy::None => false,
    };
    let captured =
        capture_auto_step_frame(app, &case.artifact_dir, index, kind, retain_screenshot)?;
    if evidence.screenshot_path.is_none() {
        evidence.screenshot_path = captured.report_screenshot_path;
    }
    Some(captured.apng_path)
}

fn capture_auto_step_frame(
    app: &TestApp,
    artifact_dir: &Path,
    index: usize,
    kind: &str,
    retain_screenshot: bool,
) -> Option<CapturedStepFrame> {
    let parent_dir = if retain_screenshot {
        artifact_dir.join("steps")
    } else {
        artifact_dir.join(".frames")
    };
    let _ = fs::create_dir_all(&parent_dir);
    let screenshot_path = parent_dir.join(format!(
        "{:03}-{}.png",
        index,
        sanitize_path_component(kind)
    ));
    if app.try_copy_screenshot_to(&screenshot_path).is_ok() {
        return Some(CapturedStepFrame {
            apng_path: screenshot_path.clone(),
            report_screenshot_path: retain_screenshot
                .then(|| screenshot_path.to_string_lossy().to_string()),
        });
    }
    None
}

fn merge_evidence(mut base: StepEvidence, extra: StepEvidence) -> StepEvidence {
    if extra.selector_query.is_some() {
        base.selector_query = extra.selector_query;
    }
    if extra.before_widgets.is_some() {
        base.before_widgets = extra.before_widgets;
    }
    if extra.after_widgets.is_some() {
        base.after_widgets = extra.after_widgets;
    }
    if extra.screenshot_path.is_some() {
        base.screenshot_path = extra.screenshot_path;
    }
    if extra.log_excerpt.is_some() {
        base.log_excerpt = extra.log_excerpt;
    }
    if extra.widget_dump_excerpt.is_some() {
        base.widget_dump_excerpt = extra.widget_dump_excerpt;
    }
    base
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis() as u64
}

fn truncate_text(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let truncated: String = input.chars().take(max_chars).collect();
    format!("{truncated}\n…")
}

fn format_logs(logs: &[(usize, LogEntry)]) -> String {
    let mut out = String::new();
    for (index, entry) in logs {
        out.push_str(&format!(
            "[{index}] {:?} {:?}: {}\n",
            entry.source, entry.level, entry.message
        ));
    }
    out
}

fn install_test_script_module(vm: &mut ScriptVm) {
    let test = vm.new_module(id!(test));

    vm.add_method(
        test,
        id_lut!(configure),
        script_args_def!(opts = NIL),
        |vm, args| {
            let opts = script_value!(vm, args.opts);
            let options = match parse_suite_options(vm, opts) {
                Ok(options) => options,
                Err(err) => return err,
            };
            let host = vm.host.downcast_mut::<SplashSuiteHost>().unwrap();
            if let Err(err) = host.register_options(options) {
                return host_script_error(vm, err.message());
            }
            NIL
        },
    );

    vm.add_method(
        test,
        id_lut!(case),
        script_args_def!(name = NIL, body = NIL),
        |vm, args| {
            let name = match script_value_to_checked_string(
                vm,
                script_value!(vm, args.name),
                "test.case name",
            ) {
                Ok(name) => name,
                Err(err) => return err,
            };
            if name.trim().is_empty() {
                return script_err_unexpected!(
                    vm.trap(),
                    "test.case requires a non-empty case name"
                );
            }
            let body = script_value!(vm, args.body);
            let Some(function) = body.as_object() else {
                return script_err_type_mismatch!(vm.trap(), "test.case body must be a function");
            };
            if !vm.bx.heap.is_fn(function) {
                return script_err_type_mismatch!(vm.trap(), "test.case body must be a function");
            }
            let host = vm.host.downcast_mut::<SplashSuiteHost>().unwrap();
            if let Err(err) = host.register_case(name, vm.bx.heap.new_object_ref(function)) {
                return host_script_error(vm, err.message());
            }
            NIL
        },
    );

    vm.add_method(
        test,
        id_lut!(fail),
        script_args_def!(message = NIL),
        |vm, args| {
            let message = script_value!(vm, args.message);
            let message = script_value_to_string(vm, message);
            trace_action(
                vm,
                "test.fail",
                "fail",
                message.clone(),
                None,
                move |_app| Err(TestError::new(message)),
            )
        },
    );

    vm.add_method(
        test,
        id_lut!(click),
        script_args_def!(selector = NIL),
        |vm, args| {
            let selector =
                match parse_selector(vm, script_value!(vm, args.selector), "test.click selector") {
                    Ok(selector) => selector,
                    Err(err) => return err,
                };
            let detail = selector.describe();
            let call_selector = selector.clone();
            trace_action(
                vm,
                "test.click",
                "click",
                detail,
                Some(selector),
                move |app| app.locator(call_selector).try_click(),
            )
        },
    );

    vm.add_method(
        test,
        id_lut!(hover),
        script_args_def!(selector = NIL),
        |vm, args| {
            let selector =
                match parse_selector(vm, script_value!(vm, args.selector), "test.hover selector") {
                    Ok(selector) => selector,
                    Err(err) => return err,
                };
            let detail = selector.describe();
            let call_selector = selector.clone();
            trace_action(
                vm,
                "test.hover",
                "hover",
                detail,
                Some(selector),
                move |app| app.locator(call_selector).try_hover(),
            )
        },
    );

    vm.add_method(
        test,
        id_lut!(fill),
        script_args_def!(selector = NIL, text = NIL),
        |vm, args| {
            let selector =
                match parse_selector(vm, script_value!(vm, args.selector), "test.fill selector") {
                    Ok(selector) => selector,
                    Err(err) => return err,
                };
            let text = match script_value_to_checked_string(
                vm,
                script_value!(vm, args.text),
                "test.fill text",
            ) {
                Ok(text) => text,
                Err(err) => return err,
            };
            let detail = format!("{} {:?}", selector.describe(), text);
            let call_selector = selector.clone();
            trace_action(
                vm,
                "test.fill",
                "fill",
                detail,
                Some(selector),
                move |app| app.locator(call_selector).try_fill(text),
            )
        },
    );

    vm.add_method(
        test,
        id_lut!(clear),
        script_args_def!(selector = NIL),
        |vm, args| {
            let selector =
                match parse_selector(vm, script_value!(vm, args.selector), "test.clear selector") {
                    Ok(selector) => selector,
                    Err(err) => return err,
                };
            let detail = selector.describe();
            let call_selector = selector.clone();
            trace_action(
                vm,
                "test.clear",
                "clear",
                detail,
                Some(selector),
                move |app| app.locator(call_selector).try_clear(),
            )
        },
    );

    vm.add_method(
        test,
        id_lut!(type_text),
        script_args_def!(text = NIL),
        |vm, args| {
            let text = match script_value_to_checked_string(
                vm,
                script_value!(vm, args.text),
                "test.type_text text",
            ) {
                Ok(text) => text,
                Err(err) => return err,
            };
            let detail = format!("{text:?}");
            trace_action(
                vm,
                "test.type_text",
                "type_text",
                detail,
                None,
                move |app| app.try_type_text(text),
            )
        },
    );

    vm.add_method(
        test,
        id_lut!(press_return),
        script_args_def!(),
        |vm, _args| {
            trace_action(
                vm,
                "test.press_return",
                "press_return",
                String::new(),
                None,
                |app| app.try_press_return(),
            )
        },
    );

    vm.add_method(
        test,
        id_lut!(press_key),
        script_args_def!(key = NIL),
        |vm, args| {
            let (key_code, modifiers) =
                match parse_key_press(vm, script_value!(vm, args.key), "test.press_key key") {
                    Ok(value) => value,
                    Err(err) => return err,
                };
            let detail = format!("{:?} {:?}", key_code, modifiers);
            trace_action(
                vm,
                "test.press_key",
                "press_key",
                detail,
                None,
                move |app| app.try_press_key_with_modifiers(key_code, modifiers),
            )
        },
    );

    vm.add_method(
        test,
        id_lut!(scroll),
        script_args_def!(selector = NIL, sx = 0.0, sy = 0.0),
        |vm, args| {
            let selector = match parse_selector(
                vm,
                script_value!(vm, args.selector),
                "test.scroll selector",
            ) {
                Ok(selector) => selector,
                Err(err) => return err,
            };
            let sx = match parse_f64(vm, script_value!(vm, args.sx), "test.scroll sx") {
                Ok(value) => value,
                Err(err) => return err,
            };
            let sy = match parse_f64(vm, script_value!(vm, args.sy), "test.scroll sy") {
                Ok(value) => value,
                Err(err) => return err,
            };
            let detail = format!("{} ({sx}, {sy})", selector.describe());
            let call_selector = selector.clone();
            trace_action(
                vm,
                "test.scroll",
                "scroll",
                detail,
                Some(selector),
                move |app| app.locator(call_selector).try_scroll(sx, sy),
            )
        },
    );

    vm.add_method(
        test,
        id_lut!(drag),
        script_args_def!(selector = NIL, dx = 0.0, dy = 0.0),
        |vm, args| {
            let selector =
                match parse_selector(vm, script_value!(vm, args.selector), "test.drag selector") {
                    Ok(selector) => selector,
                    Err(err) => return err,
                };
            let dx = match parse_f64(vm, script_value!(vm, args.dx), "test.drag dx") {
                Ok(value) => value,
                Err(err) => return err,
            };
            let dy = match parse_f64(vm, script_value!(vm, args.dy), "test.drag dy") {
                Ok(value) => value,
                Err(err) => return err,
            };
            let detail = format!("{} ({dx}, {dy})", selector.describe());
            let call_selector = selector.clone();
            trace_action(
                vm,
                "test.drag",
                "drag",
                detail,
                Some(selector),
                move |app| app.locator(call_selector).try_drag_by(dx, dy),
            )
        },
    );

    install_visibility_method(vm, test, id_lut!(wait_visible), |app, selector| {
        app.locator(selector).try_wait_visible()
    });
    install_visibility_method(vm, test, id_lut!(wait_hidden), |app, selector| {
        app.locator(selector).try_wait_hidden()
    });
    install_string_method(vm, test, id_lut!(wait_text), |app, selector, expected| {
        app.locator(selector).try_wait_text(expected)
    });
    install_string_method(vm, test, id_lut!(wait_value), |app, selector, expected| {
        app.locator(selector).try_wait_value(expected)
    });
    install_bool_method(
        vm,
        test,
        id_lut!(wait_checked),
        |app, selector, expected| app.locator(selector).try_wait_checked(expected),
    );
    install_bool_method(
        vm,
        test,
        id_lut!(wait_enabled),
        |app, selector, expected| app.locator(selector).try_wait_enabled(expected),
    );
    install_expect_method(vm, test, id_lut!(expect_text), |app, selector, expected| {
        app.locator(selector).try_assert_text(expected)
    });
    install_expect_method(
        vm,
        test,
        id_lut!(expect_value),
        |app, selector, expected| app.locator(selector).try_assert_value(expected),
    );
    install_bool_method(
        vm,
        test,
        id_lut!(expect_checked),
        |app, selector, expected| app.locator(selector).try_assert_checked(expected),
    );
    install_bool_method(
        vm,
        test,
        id_lut!(expect_enabled),
        |app, selector, expected| app.locator(selector).try_assert_enabled(expected),
    );

    vm.add_method(
        test,
        id_lut!(wait_count),
        script_args_def!(selector = NIL, count = 0.0),
        |vm, args| {
            let selector = match parse_selector(
                vm,
                script_value!(vm, args.selector),
                "test.wait_count selector",
            ) {
                Ok(selector) => selector,
                Err(err) => return err,
            };
            let count =
                match parse_usize(vm, script_value!(vm, args.count), "test.wait_count count") {
                    Ok(value) => value,
                    Err(err) => return err,
                };
            let detail = format!("{} count={count}", selector.describe());
            let call_selector = selector.clone();
            trace_action(
                vm,
                "test.wait_count",
                "wait_count",
                detail,
                Some(selector),
                move |app| app.locator(call_selector).try_wait_count(count),
            )
        },
    );

    vm.add_method(
        test,
        id_lut!(snapshot),
        script_args_def!(selector = NIL),
        |vm, args| {
            let selector = match parse_selector(
                vm,
                script_value!(vm, args.selector),
                "test.snapshot selector",
            ) {
                Ok(selector) => selector,
                Err(err) => return err,
            };
            let detail = selector.describe();
            let call_selector = selector.clone();
            match trace_call(
                vm,
                "test.snapshot",
                "snapshot",
                detail,
                Some(selector),
                move |app| app.locator(call_selector).try_snapshot(),
                |_app, snapshot| StepEvidence {
                    after_widgets: Some(vec![snapshot.clone()]),
                    ..StepEvidence::default()
                },
            ) {
                Ok(snapshot) => widget_snapshot_to_value(vm, &snapshot),
                Err(err) => host_script_error(vm, err.message()),
            }
        },
    );

    vm.add_method(
        test,
        id_lut!(snapshots),
        script_args_def!(selector = NIL),
        |vm, args| {
            let selector_value = script_value!(vm, args.selector);
            let result = if selector_value.is_nil() {
                trace_call(
                    vm,
                    "test.snapshots",
                    "snapshots",
                    "all widgets".to_string(),
                    None,
                    |app| app.try_widget_snapshot(),
                    |_app, widgets| StepEvidence {
                        after_widgets: Some(widgets.clone()),
                        ..StepEvidence::default()
                    },
                )
            } else {
                let selector = match parse_selector(vm, selector_value, "test.snapshots selector") {
                    Ok(selector) => selector,
                    Err(err) => return err,
                };
                let detail = selector.describe();
                let call_selector = selector.clone();
                trace_call(
                    vm,
                    "test.snapshots",
                    "snapshots",
                    detail,
                    Some(selector),
                    move |app| app.try_query_widgets(&call_selector, false),
                    |_app, widgets| StepEvidence {
                        after_widgets: Some(widgets.clone()),
                        ..StepEvidence::default()
                    },
                )
            };
            match result {
                Ok(widgets) => widgets_to_value(vm, &widgets),
                Err(err) => host_script_error(vm, err.message()),
            }
        },
    );

    vm.add_method(
        test,
        id_lut!(widget_dump),
        script_args_def!(),
        |vm, _args| match trace_call(
            vm,
            "test.widget_dump",
            "widget_dump",
            String::new(),
            None,
            |app| app.try_widget_dump(),
            |_app, dump| StepEvidence {
                widget_dump_excerpt: Some(truncate_text(dump, 2000)),
                ..StepEvidence::default()
            },
        ) {
            Ok(dump) => vm.new_string_with(|_vm, out| out.push_str(&dump)),
            Err(err) => host_script_error(vm, err.message()),
        },
    );

    vm.add_method(
        test,
        id_lut!(screenshot),
        script_args_def!(name = NIL),
        |vm, args| {
            let case_artifact_dir = current_case_artifact_dir(vm);
            let name_value = script_value!(vm, args.name);
            let screenshot_name = if name_value.is_nil() {
                None
            } else {
                match script_value_to_checked_string(vm, name_value, "test.screenshot name") {
                    Ok(name) => Some(name),
                    Err(err) => return err,
                }
            };
            let detail = screenshot_name
                .clone()
                .unwrap_or_else(|| "screenshot".to_string());
            match trace_call(
                vm,
                "test.screenshot",
                "screenshot",
                detail,
                None,
                move |app| {
                    let screenshot_path = app.try_screenshot()?;
                    let output_path = if let Some(name) = screenshot_name {
                        let artifact_dir = case_artifact_dir
                            .clone()
                            .unwrap_or_else(|| app.artifacts_dir());
                        fs::create_dir_all(&artifact_dir).map_err(|err| {
                            TestError::new(format!(
                                "failed to create screenshot directory {}: {}",
                                artifact_dir.display(),
                                err
                            ))
                        })?;
                        let output_path =
                            artifact_dir.join(format!("{}.png", sanitize_path_component(&name)));
                        fs::copy(&screenshot_path, &output_path).map_err(|err| {
                            TestError::new(format!(
                                "failed to copy screenshot to {}: {}",
                                output_path.display(),
                                err
                            ))
                        })?;
                        output_path
                    } else {
                        screenshot_path
                    };
                    Ok(output_path)
                },
                |_app, path| StepEvidence {
                    screenshot_path: Some(path.to_string_lossy().to_string()),
                    ..StepEvidence::default()
                },
            ) {
                Ok(path) => vm.new_string_with(|_vm, out| out.push_str(&path.to_string_lossy())),
                Err(err) => host_script_error(vm, err.message()),
            }
        },
    );

    vm.add_method(
        test,
        id_lut!(logs),
        script_args_def!(pattern = NIL),
        |vm, args| {
            let pattern = if script_value!(vm, args.pattern).is_nil() {
                None
            } else {
                match script_value_to_checked_string(
                    vm,
                    script_value!(vm, args.pattern),
                    "test.logs pattern",
                ) {
                    Ok(pattern) => Some(pattern),
                    Err(err) => return err,
                }
            };
            let detail = pattern.clone().unwrap_or_else(|| "all logs".to_string());
            let logs = match trace_call(
                vm,
                "test.logs",
                "logs",
                detail,
                None,
                move |app| app.try_query_logs(pattern),
                |_app, logs| StepEvidence {
                    log_excerpt: Some(format_logs(logs)),
                    ..StepEvidence::default()
                },
            ) {
                Ok(logs) => logs,
                Err(err) => return host_script_error(vm, err.message()),
            };
            logs_to_value(vm, &logs)
        },
    );

    vm.add_method(
        test,
        id_lut!(wait_log),
        script_args_def!(pattern = NIL),
        |vm, args| {
            let pattern = match script_value_to_checked_string(
                vm,
                script_value!(vm, args.pattern),
                "test.wait_log pattern",
            ) {
                Ok(pattern) => pattern,
                Err(err) => return err,
            };
            trace_action(
                vm,
                "test.wait_log",
                "wait_log",
                pattern.clone(),
                None,
                move |app| app.try_wait_for_log_contains(&pattern),
            )
        },
    );
}

fn install_visibility_method<F>(vm: &mut ScriptVm, test: ScriptObject, method: LiveId, f: F)
where
    F: Fn(TestApp, Selector) -> TestResult<()> + 'static,
{
    vm.add_method(
        test,
        method,
        script_args_def!(selector = NIL),
        move |vm, args| {
            let selector = match parse_selector(vm, script_value!(vm, args.selector), "selector") {
                Ok(selector) => selector,
                Err(err) => return err,
            };
            let detail = selector.describe();
            let call_selector = selector.clone();
            trace_action(
                vm,
                "test wait/assert",
                "selector_wait",
                detail,
                Some(selector),
                |app| f(app, call_selector.clone()),
            )
        },
    );
}

fn install_string_method<F>(vm: &mut ScriptVm, test: ScriptObject, method: LiveId, f: F)
where
    F: Fn(TestApp, Selector, String) -> TestResult<()> + 'static,
{
    vm.add_method(
        test,
        method,
        script_args_def!(selector = NIL, expected = NIL),
        move |vm, args| {
            let selector = match parse_selector(vm, script_value!(vm, args.selector), "selector") {
                Ok(selector) => selector,
                Err(err) => return err,
            };
            let expected = match script_value_to_checked_string(
                vm,
                script_value!(vm, args.expected),
                "expected",
            ) {
                Ok(expected) => expected,
                Err(err) => return err,
            };
            let detail = format!("{} {:?}", selector.describe(), expected);
            let call_selector = selector.clone();
            trace_action(
                vm,
                "test string assert",
                "string_assert",
                detail,
                Some(selector),
                |app| f(app, call_selector.clone(), expected.clone()),
            )
        },
    );
}

fn install_expect_method<F>(vm: &mut ScriptVm, test: ScriptObject, method: LiveId, f: F)
where
    F: Fn(TestApp, Selector, String) -> TestResult<()> + 'static,
{
    vm.add_method(
        test,
        method,
        script_args_def!(selector = NIL, expected = NIL),
        move |vm, args| {
            let selector = match parse_selector(vm, script_value!(vm, args.selector), "selector") {
                Ok(selector) => selector,
                Err(err) => return err,
            };
            let expected = match script_value_to_checked_string(
                vm,
                script_value!(vm, args.expected),
                "expected",
            ) {
                Ok(expected) => expected,
                Err(err) => return err,
            };
            let detail = format!("{} {:?}", selector.describe(), expected);
            let call_selector = selector.clone();
            trace_action(vm, "test assert", "expect", detail, Some(selector), |app| {
                f(app, call_selector.clone(), expected.clone())
            })
        },
    );
}

fn install_bool_method<F>(vm: &mut ScriptVm, test: ScriptObject, method: LiveId, f: F)
where
    F: Fn(TestApp, Selector, bool) -> TestResult<()> + 'static,
{
    vm.add_method(
        test,
        method,
        script_args_def!(selector = NIL, expected = NIL),
        move |vm, args| {
            let selector = match parse_selector(vm, script_value!(vm, args.selector), "selector") {
                Ok(selector) => selector,
                Err(err) => return err,
            };
            let expected = match parse_bool(vm, script_value!(vm, args.expected), "expected") {
                Ok(expected) => expected,
                Err(err) => return err,
            };
            let detail = format!("{} {expected}", selector.describe());
            let call_selector = selector.clone();
            trace_action(
                vm,
                "test bool assert",
                "bool_assert",
                detail,
                Some(selector),
                |app| f(app, call_selector.clone(), expected),
            )
        },
    );
}

fn current_app(vm: &mut ScriptVm) -> Option<TestApp> {
    vm.host
        .downcast_mut::<SplashSuiteHost>()
        .unwrap()
        .current_app
        .clone()
}

fn current_case_artifact_dir(vm: &mut ScriptVm) -> Option<PathBuf> {
    vm.host
        .downcast_mut::<SplashSuiteHost>()
        .unwrap()
        .current_case
        .as_ref()
        .map(|case| case.artifact_dir.clone())
}

fn host_script_error(vm: &mut ScriptVm, message: impl Into<String>) -> ScriptValue {
    let message = message.into();
    {
        let host = vm.host.downcast_mut::<SplashSuiteHost>().unwrap();
        host.set_last_error_message(message.clone());
    }
    vm.bail(&message);
    NIL
}

fn parse_suite_options(
    vm: &mut ScriptVm,
    value: ScriptValue,
) -> Result<SplashSuiteOptions, ScriptValue> {
    let Some(object) = value.as_object() else {
        return Err(script_err_type_mismatch!(
            vm.trap(),
            "test.configure expects an options object"
        ));
    };

    let launch_name = optional_string_field(vm, object, id!(launch), "test.configure launch")?;
    let visible_run_item = optional_string_field(
        vm,
        object,
        id!(visible_run_item),
        "test.configure visible_run_item",
    )?;
    let session_mode =
        match optional_string_field(vm, object, id!(session_mode), "test.configure session_mode")?
            .as_deref()
            .unwrap_or("isolated")
        {
            "isolated" => SessionMode::Isolated,
            "shared" => SessionMode::Shared,
            other => {
                return Err(host_script_error(
                    vm,
                    format!("unknown test.configure session_mode `{}`", other),
                ))
            }
        };
    let step_screenshot_policy = match optional_string_field(
        vm,
        object,
        id!(step_screenshots),
        "test.configure step_screenshots",
    )? {
        Some(value) => match StepScreenshotPolicy::from_str(&value) {
            Some(policy) => Some(policy),
            None => {
                return Err(host_script_error(
                    vm,
                    format!("unknown test.configure step_screenshots `{}`", value),
                ))
            }
        },
        None => None,
    };
    let headless_run_item = optional_string_field(
        vm,
        object,
        id!(headless_run_item),
        "test.configure headless_run_item",
    )?;

    let launch = match launch_name.as_deref().unwrap_or("current_package") {
        "current_package" => {
            if visible_run_item.is_some() || headless_run_item.is_some() {
                return Err(host_script_error(
                    vm,
                    "test.configure current_package launch does not accept visible/headless run items",
                ));
            }
            Some(SplashLaunch::CurrentPackage)
        }
        "splash_run_item" => {
            let Some(visible_run_item) = visible_run_item else {
                return Err(host_script_error(
                    vm,
                    "test.configure splash_run_item launch requires visible_run_item",
                ));
            };
            let Some(headless_run_item) = headless_run_item else {
                return Err(host_script_error(
                    vm,
                    "test.configure splash_run_item launch requires headless_run_item",
                ));
            };
            Some(SplashLaunch::SplashRunItem {
                visible_run_item,
                headless_run_item,
            })
        }
        other => {
            return Err(host_script_error(
                vm,
                format!("unknown test.configure launch `{}`", other),
            ))
        }
    };

    Ok(SplashSuiteOptions {
        launch,
        session_mode,
        step_screenshot_policy,
        startup_timeout: optional_duration_field(
            vm,
            object,
            id!(startup_timeout_ms),
            "test.configure startup_timeout_ms",
        )?,
        action_timeout: optional_duration_field(
            vm,
            object,
            id!(action_timeout_ms),
            "test.configure action_timeout_ms",
        )?,
        poll_interval: optional_duration_field(
            vm,
            object,
            id!(poll_interval_ms),
            "test.configure poll_interval_ms",
        )?,
        startup_delay: optional_duration_field(
            vm,
            object,
            id!(startup_delay_ms),
            "test.configure startup_delay_ms",
        )?,
        action_delay: optional_duration_field(
            vm,
            object,
            id!(action_delay_ms),
            "test.configure action_delay_ms",
        )?,
        keep_open: optional_duration_field(
            vm,
            object,
            id!(keep_open_ms),
            "test.configure keep_open_ms",
        )?,
    })
}

fn parse_selector(
    vm: &mut ScriptVm,
    value: ScriptValue,
    what: &str,
) -> Result<Selector, ScriptValue> {
    if value.is_string_like() {
        return Ok(Selector::raw(script_value_to_string(vm, value)));
    }
    let Some(object) = value.as_object() else {
        return Err(script_err_type_mismatch!(
            vm.trap(),
            "{} expects a selector object or raw string",
            what
        ));
    };

    let options = SelectorOptions {
        id: optional_string_field(vm, object, id!(id), &format!("{what}.id"))?,
        widget_type: optional_string_field(
            vm,
            object,
            id!(widget_type),
            &format!("{what}.widget_type"),
        )?,
        raw: optional_string_field(vm, object, id!(raw), &format!("{what}.raw"))?,
        text_exact: optional_string_field(
            vm,
            object,
            id!(text_exact),
            &format!("{what}.text_exact"),
        )?,
        text_contains: optional_string_field(
            vm,
            object,
            id!(text_contains),
            &format!("{what}.text_contains"),
        )?,
        nth: optional_usize_field(vm, object, id!(nth), &format!("{what}.nth"))?,
        window: optional_string_field(vm, object, id!(window), &format!("{what}.window"))?,
        window_index: optional_usize_field(
            vm,
            object,
            id!(window_index),
            &format!("{what}.window_index"),
        )?,
        any_window: optional_bool_field(
            vm,
            object,
            id!(any_window),
            &format!("{what}.any_window"),
        )?
        .unwrap_or(false),
    };

    if options.any_window && (options.window.is_some() || options.window_index.is_some()) {
        return Err(script_err_unexpected!(
            vm.trap(),
            "{} cannot set any_window together with window or window_index",
            what
        ));
    }

    Ok(Selector::from_options(options))
}

fn parse_key_press(
    vm: &mut ScriptVm,
    value: ScriptValue,
    what: &str,
) -> Result<(KeyCode, KeyModifiers), ScriptValue> {
    if value.is_string_like() {
        let key_name = script_value_to_string(vm, value);
        let key_code = parse_key_code_name(&key_name).ok_or_else(|| {
            script_err_unexpected!(vm.trap(), "{} has unknown key `{}`", what, key_name)
        })?;
        return Ok((key_code, KeyModifiers::default()));
    }

    let Some(object) = value.as_object() else {
        return Err(script_err_type_mismatch!(
            vm.trap(),
            "{} expects a key name string or key options object",
            what
        ));
    };

    let key_name = match optional_string_field(vm, object, id!(key), &format!("{what}.key"))? {
        Some(key_name) => key_name,
        None => optional_string_field(vm, object, id!(key_code), &format!("{what}.key_code"))?
            .ok_or_else(|| {
                script_err_unexpected!(vm.trap(), "{} requires `key` or `key_code`", what)
            })?,
    };
    let key_code = parse_key_code_name(&key_name).ok_or_else(|| {
        script_err_unexpected!(vm.trap(), "{} has unknown key `{}`", what, key_name)
    })?;
    let modifiers = KeyModifiers {
        shift: optional_bool_field(vm, object, id!(shift), &format!("{what}.shift"))?
            .unwrap_or(false),
        control: optional_bool_field(vm, object, id!(control), &format!("{what}.control"))?
            .unwrap_or(false),
        alt: optional_bool_field(vm, object, id!(alt), &format!("{what}.alt"))?.unwrap_or(false),
        logo: optional_bool_field(vm, object, id!(logo), &format!("{what}.logo"))?.unwrap_or(false),
    };
    Ok((key_code, modifiers))
}

fn parse_key_code_name(name: &str) -> Option<KeyCode> {
    let normalized: String = name
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| {
            ch.to_ascii_lowercase()
                .to_string()
                .chars()
                .collect::<Vec<_>>()
        })
        .collect();
    if normalized.len() == 1 {
        let ch = normalized.chars().next()?;
        if ch.is_ascii_lowercase() {
            return match ch {
                'a' => Some(KeyCode::KeyA),
                'b' => Some(KeyCode::KeyB),
                'c' => Some(KeyCode::KeyC),
                'd' => Some(KeyCode::KeyD),
                'e' => Some(KeyCode::KeyE),
                'f' => Some(KeyCode::KeyF),
                'g' => Some(KeyCode::KeyG),
                'h' => Some(KeyCode::KeyH),
                'i' => Some(KeyCode::KeyI),
                'j' => Some(KeyCode::KeyJ),
                'k' => Some(KeyCode::KeyK),
                'l' => Some(KeyCode::KeyL),
                'm' => Some(KeyCode::KeyM),
                'n' => Some(KeyCode::KeyN),
                'o' => Some(KeyCode::KeyO),
                'p' => Some(KeyCode::KeyP),
                'q' => Some(KeyCode::KeyQ),
                'r' => Some(KeyCode::KeyR),
                's' => Some(KeyCode::KeyS),
                't' => Some(KeyCode::KeyT),
                'u' => Some(KeyCode::KeyU),
                'v' => Some(KeyCode::KeyV),
                'w' => Some(KeyCode::KeyW),
                'x' => Some(KeyCode::KeyX),
                'y' => Some(KeyCode::KeyY),
                'z' => Some(KeyCode::KeyZ),
                _ => None,
            };
        }
        if ch.is_ascii_digit() {
            return match ch {
                '0' => Some(KeyCode::Key0),
                '1' => Some(KeyCode::Key1),
                '2' => Some(KeyCode::Key2),
                '3' => Some(KeyCode::Key3),
                '4' => Some(KeyCode::Key4),
                '5' => Some(KeyCode::Key5),
                '6' => Some(KeyCode::Key6),
                '7' => Some(KeyCode::Key7),
                '8' => Some(KeyCode::Key8),
                '9' => Some(KeyCode::Key9),
                _ => None,
            };
        }
    }

    match normalized.as_str() {
        "escape" | "esc" => Some(KeyCode::Escape),
        "backspace" => Some(KeyCode::Backspace),
        "tab" => Some(KeyCode::Tab),
        "enter" | "return" | "returnkey" => Some(KeyCode::ReturnKey),
        "space" | "spacebar" => Some(KeyCode::Space),
        "delete" | "del" => Some(KeyCode::Delete),
        "insert" | "ins" => Some(KeyCode::Insert),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "pageup" => Some(KeyCode::PageUp),
        "pagedown" => Some(KeyCode::PageDown),
        "arrowup" | "up" => Some(KeyCode::ArrowUp),
        "arrowdown" | "down" => Some(KeyCode::ArrowDown),
        "arrowleft" | "left" => Some(KeyCode::ArrowLeft),
        "arrowright" | "right" => Some(KeyCode::ArrowRight),
        "minus" => Some(KeyCode::Minus),
        "equals" | "equal" => Some(KeyCode::Equals),
        "semicolon" => Some(KeyCode::Semicolon),
        "quote" => Some(KeyCode::Quote),
        "comma" => Some(KeyCode::Comma),
        "period" | "dot" => Some(KeyCode::Period),
        "slash" => Some(KeyCode::Slash),
        "backslash" => Some(KeyCode::Backslash),
        "backtick" | "grave" => Some(KeyCode::Backtick),
        "lbracket" | "leftbracket" => Some(KeyCode::LBracket),
        "rbracket" | "rightbracket" => Some(KeyCode::RBracket),
        "f1" => Some(KeyCode::F1),
        "f2" => Some(KeyCode::F2),
        "f3" => Some(KeyCode::F3),
        "f4" => Some(KeyCode::F4),
        "f5" => Some(KeyCode::F5),
        "f6" => Some(KeyCode::F6),
        "f7" => Some(KeyCode::F7),
        "f8" => Some(KeyCode::F8),
        "f9" => Some(KeyCode::F9),
        "f10" => Some(KeyCode::F10),
        "f11" => Some(KeyCode::F11),
        "f12" => Some(KeyCode::F12),
        _ => None,
    }
}

fn parse_bool(vm: &mut ScriptVm, value: ScriptValue, what: &str) -> Result<bool, ScriptValue> {
    if let Some(value) = value.as_bool() {
        return Ok(value);
    }
    if let Some(value) = value.as_number() {
        return Ok(value != 0.0);
    }
    Err(script_err_type_mismatch!(
        vm.trap(),
        "{} expects a bool",
        what
    ))
}

fn parse_f64(vm: &mut ScriptVm, value: ScriptValue, what: &str) -> Result<f64, ScriptValue> {
    value
        .as_number()
        .ok_or_else(|| script_err_type_mismatch!(vm.trap(), "{} expects a number", what))
}

fn parse_usize(vm: &mut ScriptVm, value: ScriptValue, what: &str) -> Result<usize, ScriptValue> {
    let number = parse_f64(vm, value, what)?;
    if !number.is_finite() || number < 0.0 || number.fract() != 0.0 {
        return Err(script_err_type_mismatch!(
            vm.trap(),
            "{} expects a non-negative integer",
            what
        ));
    }
    Ok(number as usize)
}

fn optional_string_field(
    vm: &mut ScriptVm,
    object: ScriptObject,
    field: LiveId,
    what: &str,
) -> Result<Option<String>, ScriptValue> {
    let value = vm.bx.heap.value(object, field.into(), NoTrap);
    if value.is_nil() || value.is_err() {
        return Ok(None);
    }
    Ok(Some(script_value_to_checked_string(vm, value, what)?))
}

fn optional_bool_field(
    vm: &mut ScriptVm,
    object: ScriptObject,
    field: LiveId,
    what: &str,
) -> Result<Option<bool>, ScriptValue> {
    let value = vm.bx.heap.value(object, field.into(), NoTrap);
    if value.is_nil() || value.is_err() {
        return Ok(None);
    }
    Ok(Some(parse_bool(vm, value, what)?))
}

fn optional_usize_field(
    vm: &mut ScriptVm,
    object: ScriptObject,
    field: LiveId,
    what: &str,
) -> Result<Option<usize>, ScriptValue> {
    let value = vm.bx.heap.value(object, field.into(), NoTrap);
    if value.is_nil() || value.is_err() {
        return Ok(None);
    }
    Ok(Some(parse_usize(vm, value, what)?))
}

fn optional_duration_field(
    vm: &mut ScriptVm,
    object: ScriptObject,
    field: LiveId,
    what: &str,
) -> Result<Option<Duration>, ScriptValue> {
    let value = vm.bx.heap.value(object, field.into(), NoTrap);
    if value.is_nil() || value.is_err() {
        return Ok(None);
    }
    let millis = parse_f64(vm, value, what)?;
    if !millis.is_finite() || millis < 0.0 {
        return Err(script_err_type_mismatch!(
            vm.trap(),
            "{} expects a non-negative number",
            what
        ));
    }
    Ok(Some(Duration::from_millis(millis as u64)))
}

fn widget_snapshot_to_value(vm: &mut ScriptVm, widget: &WidgetSnapshot) -> ScriptValue {
    let object = vm.bx.heap.new_object();
    set_string_field(vm, object, id!(id), &widget.id);
    set_string_field(vm, object, id!(widget_type), &widget.widget_type);
    set_string_field(vm, object, id!(window_id), &widget.window_id);
    vm.bx.heap.set_value_def(
        object,
        id!(window_index).into(),
        (widget.window_index as f64).into(),
    );
    vm.bx
        .heap
        .set_value_def(object, id!(visible).into(), widget.visible.into());
    vm.bx
        .heap
        .set_value_def(object, id!(enabled).into(), widget.enabled.into());
    vm.bx
        .heap
        .set_value_def(object, id!(x).into(), (widget.x as f64).into());
    vm.bx
        .heap
        .set_value_def(object, id!(y).into(), (widget.y as f64).into());
    vm.bx
        .heap
        .set_value_def(object, id!(width).into(), (widget.width as f64).into());
    vm.bx
        .heap
        .set_value_def(object, id!(height).into(), (widget.height as f64).into());
    set_optional_string_field(vm, object, id!(text), widget.text.as_deref());
    set_optional_string_field(vm, object, id!(value), widget.value.as_deref());
    if let Some(checked) = widget.checked {
        vm.bx
            .heap
            .set_value_def(object, id!(checked).into(), checked.into());
    } else {
        vm.bx.heap.set_value_def(object, id!(checked).into(), NIL);
    }
    set_optional_string_field(vm, object, id!(selected), widget.selected.as_deref());
    object.into()
}

fn widgets_to_value(vm: &mut ScriptVm, widgets: &[WidgetSnapshot]) -> ScriptValue {
    let array = vm.bx.heap.new_array();
    for widget in widgets {
        let value = widget_snapshot_to_value(vm, widget);
        vm.bx.heap.array_push_unchecked(array, value);
    }
    array.into()
}

fn logs_to_value(vm: &mut ScriptVm, logs: &[(usize, LogEntry)]) -> ScriptValue {
    let array = vm.bx.heap.new_array();
    for (_index, entry) in logs {
        let object = vm.bx.heap.new_object();
        vm.bx
            .heap
            .set_value_def(object, id!(index).into(), (entry.index as f64).into());
        vm.bx
            .heap
            .set_value_def(object, id!(timestamp).into(), entry.timestamp.into());
        if let Some(build_id) = entry.build_id {
            vm.bx
                .heap
                .set_value_def(object, id!(build_id).into(), (build_id.0 as f64).into());
        } else {
            vm.bx.heap.set_value_def(object, id!(build_id).into(), NIL);
        }
        set_string_field(vm, object, id!(level), &format!("{:?}", entry.level));
        set_string_field(vm, object, id!(source), &format!("{:?}", entry.source));
        set_string_field(vm, object, id!(message), &entry.message);
        set_optional_string_field(vm, object, id!(file_name), entry.file_name.as_deref());
        if let Some(line) = entry.line {
            vm.bx
                .heap
                .set_value_def(object, id!(line).into(), (line as f64).into());
        } else {
            vm.bx.heap.set_value_def(object, id!(line).into(), NIL);
        }
        if let Some(column) = entry.column {
            vm.bx
                .heap
                .set_value_def(object, id!(column).into(), (column as f64).into());
        } else {
            vm.bx.heap.set_value_def(object, id!(column).into(), NIL);
        }
        vm.bx.heap.array_push_unchecked(array, object.into());
    }
    array.into()
}

fn set_string_field(vm: &mut ScriptVm, object: ScriptObject, field: LiveId, value: &str) {
    let value = vm.new_string_with(|_vm, out| out.push_str(value));
    vm.bx.heap.set_value_def(object, field.into(), value);
}

fn set_optional_string_field(
    vm: &mut ScriptVm,
    object: ScriptObject,
    field: LiveId,
    value: Option<&str>,
) {
    if let Some(value) = value {
        set_string_field(vm, object, field, value);
    } else {
        vm.bx.heap.set_value_def(object, field.into(), NIL);
    }
}

fn discover_splash_mount_root(manifest_dir: &Path) -> TestResult<PathBuf> {
    for ancestor in manifest_dir.ancestors() {
        if ancestor.join("makepad.splash").is_file() {
            return Ok(ancestor.to_path_buf());
        }
    }
    Err(TestError::new(format!(
        "failed to find makepad.splash above {}",
        manifest_dir.display()
    )))
}

fn resolve_suite_path(manifest_dir: &Path, suite_path: &Path) -> PathBuf {
    if suite_path.is_absolute() {
        suite_path.to_path_buf()
    } else {
        manifest_dir.join(suite_path)
    }
}

fn normalize_script_source(source: &str) -> String {
    let mut normalized = source.to_string();
    if !normalized.trim_end().ends_with(';') {
        normalized.push(';');
    }
    normalized
}

fn script_value_to_string(vm: &mut ScriptVm, value: ScriptValue) -> String {
    if let Some(line) = vm.string_with(value, |_vm, s| s.to_string()) {
        return line;
    }
    vm.bx.heap.temp_string_with(|heap, temp| {
        heap.cast_to_string(value, temp);
        temp.clone()
    })
}

fn script_value_to_checked_string(
    vm: &mut ScriptVm,
    value: ScriptValue,
    what: &str,
) -> Result<String, ScriptValue> {
    if value.is_err() {
        let rendered = script_value_to_string(vm, value);
        return Err(script_err_unexpected!(
            vm.trap(),
            "{} resolved to script error {}",
            what,
            rendered
        ));
    }
    Ok(script_value_to_string(vm, value))
}

#[cfg(test)]
mod tests {
    use super::{
        discover_splash_mount_root, SessionMode, SplashLaunch, SplashSuiteRunner,
        StepScreenshotPolicy,
    };
    use crate::{TestConfig, TestError};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("makepad_test_{prefix}_{unique}"));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn loads_current_package_suite_without_explicit_config() {
        let runner = SplashSuiteRunner::load_from_source(
            PathBuf::from("/tmp/example"),
            PathBuf::from("/tmp/example/tests/ui.splash"),
            "use mod.test\ntest.case(\"smoke\", || {})\n".to_string(),
        )
        .unwrap();

        assert_eq!(runner.case_names(), vec!["smoke".to_string()]);
        assert!(runner.options().is_none());
    }

    #[test]
    fn preserves_case_declaration_order() {
        let runner = SplashSuiteRunner::load_from_source(
            PathBuf::from("/tmp/example"),
            PathBuf::from("/tmp/example/tests/ui.splash"),
            "use mod.test\ntest.case(\"z_last\", || {})\ntest.case(\"a_first\", || {})\n"
                .to_string(),
        )
        .unwrap();

        assert_eq!(
            runner.case_names(),
            vec!["z_last".to_string(), "a_first".to_string()]
        );
    }

    #[test]
    fn rejects_duplicate_case_names() {
        let err = SplashSuiteRunner::load_from_source(
            PathBuf::from("/tmp/example"),
            PathBuf::from("/tmp/example/tests/ui.splash"),
            "use mod.test\ntest.case(\"smoke\", || {})\ntest.case(\"smoke\", || {})\n".to_string(),
        )
        .err()
        .unwrap();

        assert!(err.message().contains("duplicate Splash test case"));
    }

    #[test]
    fn parses_splash_run_item_options() {
        let runner = SplashSuiteRunner::load_from_source(
            PathBuf::from("/tmp/example"),
            PathBuf::from("/tmp/example/tests/ui.splash"),
            "use mod.test\ntest.configure({launch:\"splash_run_item\" visible_run_item:\"visible\" headless_run_item:\"headless\" action_timeout_ms:250})\ntest.case(\"smoke\", || {})\n".to_string(),
        )
        .unwrap();

        let options = runner.options().unwrap();
        assert_eq!(
            options.launch,
            Some(SplashLaunch::SplashRunItem {
                visible_run_item: "visible".to_string(),
                headless_run_item: "headless".to_string(),
            })
        );
        assert_eq!(options.action_timeout, Some(Duration::from_millis(250)));
    }

    #[test]
    fn defaults_to_isolated_session_mode() {
        let runner = SplashSuiteRunner::load_from_source(
            PathBuf::from("/tmp/example"),
            PathBuf::from("/tmp/example/tests/ui.splash"),
            "use mod.test\ntest.case(\"smoke\", || {})\n".to_string(),
        )
        .unwrap();

        assert_eq!(runner.session_mode(), SessionMode::Isolated);
    }

    #[test]
    fn parses_shared_session_mode() {
        let runner = SplashSuiteRunner::load_from_source(
            PathBuf::from("/tmp/example"),
            PathBuf::from("/tmp/example/tests/ui.splash"),
            "use mod.test\ntest.configure({session_mode:\"shared\"})\ntest.case(\"smoke\", || {})\n".to_string(),
        )
        .unwrap();

        assert_eq!(runner.session_mode(), SessionMode::Shared);
    }

    #[test]
    fn defaults_step_screenshot_policy_to_failures() {
        let runner = SplashSuiteRunner::load_from_source(
            PathBuf::from("/tmp/example"),
            PathBuf::from("/tmp/example/tests/ui.splash"),
            "use mod.test\ntest.case(\"smoke\", || {})\n".to_string(),
        )
        .unwrap();

        assert_eq!(
            runner
                .test_config("makepad-example", "ui::test")
                .unwrap()
                .step_screenshot_policy,
            StepScreenshotPolicy::Failures
        );
    }

    #[test]
    fn parses_step_screenshot_policy_from_config() {
        let all_runner = SplashSuiteRunner::load_from_source(
            PathBuf::from("/tmp/example"),
            PathBuf::from("/tmp/example/tests/ui.splash"),
            "use mod.test\ntest.configure({step_screenshots:\"all\"})\ntest.case(\"smoke\", || {})\n".to_string(),
        )
        .unwrap();
        assert_eq!(
            all_runner
                .test_config("makepad-example", "ui::test")
                .unwrap()
                .step_screenshot_policy,
            StepScreenshotPolicy::All
        );

        let none_runner = SplashSuiteRunner::load_from_source(
            PathBuf::from("/tmp/example"),
            PathBuf::from("/tmp/example/tests/ui.splash"),
            "use mod.test\ntest.configure({step_screenshots:\"none\"})\ntest.case(\"smoke\", || {})\n".to_string(),
        )
        .unwrap();
        assert_eq!(
            none_runner
                .test_config("makepad-example", "ui::test")
                .unwrap()
                .step_screenshot_policy,
            StepScreenshotPolicy::None
        );
    }

    #[test]
    fn rejects_invalid_session_mode() {
        let err = SplashSuiteRunner::load_from_source(
            PathBuf::from("/tmp/example"),
            PathBuf::from("/tmp/example/tests/ui.splash"),
            "use mod.test\ntest.configure({session_mode:\"broken\"})\n".to_string(),
        )
        .err()
        .unwrap();

        assert!(err.message().contains("session_mode"));
    }

    #[test]
    fn rejects_invalid_step_screenshot_policy() {
        let err = SplashSuiteRunner::load_from_source(
            PathBuf::from("/tmp/example"),
            PathBuf::from("/tmp/example/tests/ui.splash"),
            "use mod.test\ntest.configure({step_screenshots:\"broken\"})\n".to_string(),
        )
        .err()
        .unwrap();

        assert!(err.message().contains("step_screenshots"));
    }

    #[test]
    fn rejects_invalid_launch_configuration() {
        let err = SplashSuiteRunner::load_from_source(
            PathBuf::from("/tmp/example"),
            PathBuf::from("/tmp/example/tests/ui.splash"),
            "use mod.test\ntest.configure({launch:\"splash_run_item\" visible_run_item:\"visible\"})\n".to_string(),
        )
        .err()
        .unwrap();

        assert!(err.message().contains("headless_run_item"));
    }

    #[test]
    fn selector_config_shapes_artifact_dir_by_case_name() {
        let config =
            TestConfig::current_package("/tmp/example", "makepad-example", "ui::splash_case_name")
                .unwrap();
        assert_eq!(
            config.artifacts_dir,
            PathBuf::from("/tmp/example")
                .join("target")
                .join("makepad_test")
                .join("makepad-example")
                .join("ui__splash_case_name")
        );
    }

    #[test]
    fn startup_failure_case_outcome_preserves_original_error() {
        let outcome = super::startup_failure_case_outcome(
            "smoke",
            PathBuf::from("/tmp/example/cases/smoke"),
            25,
            Duration::from_millis(10),
            TestError::new("bind failed"),
        );

        assert_eq!(outcome.report.case_name, "smoke");
        assert_eq!(outcome.report.failure_message.as_deref(), Some("bind failed"));
        assert_eq!(outcome.error.as_ref().map(TestError::message), Some("bind failed"));
    }

    #[test]
    fn discovers_mount_root_from_makepad_splash() {
        let root = temp_dir("discover_mount_root");
        let nested = root.join("examples/splash");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join("makepad.splash"), "use mod.hub\n").unwrap();
        assert_eq!(discover_splash_mount_root(&nested).unwrap(), root);
    }

    #[test]
    fn missing_case_reports_error_without_launching_app() {
        let manifest_dir = temp_dir("missing_case").join("example");
        fs::create_dir_all(manifest_dir.join("tests")).unwrap();
        fs::write(
            manifest_dir.join("tests/ui.splash"),
            "use mod.test\ntest.case(\"exists\", || {})\n",
        )
        .unwrap();

        let runner = SplashSuiteRunner::load(manifest_dir.clone(), "tests/ui.splash").unwrap();
        let err = runner.host.case("missing").err().unwrap();

        assert!(err.message().contains("was not registered"));
    }
}

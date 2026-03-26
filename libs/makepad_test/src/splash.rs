use crate::runtime::{run_with_config, sanitize_path_component};
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SplashSuiteOptions {
    launch: Option<SplashLaunch>,
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
    current_app: Option<TestApp>,
    last_error_message: Option<String>,
}

impl SplashSuiteHost {
    fn new(suite_path: PathBuf) -> Self {
        Self {
            suite_path,
            options: None,
            cases: HashMap::new(),
            current_app: None,
            last_error_message: None,
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
            SplashCase {
                name,
                function,
            },
        );
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
}

pub struct SplashSuiteRunner {
    manifest_dir: PathBuf,
    host: SplashSuiteHost,
    std: ScriptStd,
    script_vm: Option<Box<ScriptVmBase>>,
}

impl SplashSuiteRunner {
    pub fn load(manifest_dir: impl Into<PathBuf>, suite_path: impl AsRef<Path>) -> TestResult<Self> {
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

    pub fn test_config(
        &self,
        package_name: &str,
        test_name: &str,
    ) -> TestResult<TestConfig> {
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

        Ok(config)
    }

    pub fn run_case(&mut self, case_name: &str, app: TestApp) -> TestResult<()> {
        let case = self.host.case(case_name)?;
        self.host.clear_last_error_message();
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
            return Err(TestError::new(format!(
                "Splash test case `{}` failed: {}",
                case.name, message
            )));
        }
        Ok(())
    }

    #[cfg(test)]
    fn options(&self) -> Option<&SplashSuiteOptions> {
        self.host.options.as_ref()
    }

    fn case_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.host.cases.keys().cloned().collect();
        names.sort();
        names
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
    let config = runner.test_config(package_name, &suite_test_name)?;
    let wall = Instant::now();
    let result = run_with_config(config, |app| -> TestResult<()> {
        eprintln!(
            "[makepad_test] splash: app ready after {:.2}s (hub + build + launch; not part of case timings below)",
            wall.elapsed().as_secs_f64()
        );
        let session_start = Instant::now();
        let total = case_names.len();
        for (index, case_name) in case_names.iter().enumerate() {
            eprintln!(
                "[makepad_test] splash case {}/{}: {} …",
                index + 1,
                total,
                case_name
            );
            let case_start = Instant::now();
            match runner.run_case(case_name, app.clone()) {
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
                    return Err(err);
                }
            }
        }
        eprintln!(
            "[makepad_test] splash suite: {} cases ran in {:.2}s (Splash `test.case` bodies only)",
            total,
            session_start.elapsed().as_secs_f64()
        );
        Ok(())
    });
    eprintln!(
        "[makepad_test] splash: total {:.2}s (startup + case bodies + teardown — explains cargo test wall time vs case sum)",
        wall.elapsed().as_secs_f64()
    );
    result
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
            let name = match script_value_to_checked_string(vm, script_value!(vm, args.name), "test.case name")
            {
                Ok(name) => name,
                Err(err) => return err,
            };
            if name.trim().is_empty() {
                return script_err_unexpected!(vm.trap(), "test.case requires a non-empty case name");
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
            host_script_error(vm, message)
        },
    );

    vm.add_method(
        test,
        id_lut!(click),
        script_args_def!(selector = NIL),
        |vm, args| {
            let selector = match parse_selector(vm, script_value!(vm, args.selector), "test.click selector") {
                Ok(selector) => selector,
                Err(err) => return err,
            };
            let Some(app) = current_app(vm) else {
                return missing_app_error(vm, "test.click");
            };
            if let Err(err) = app.locator(selector).try_click() {
                return host_script_error(vm, err.message());
            }
            NIL
        },
    );

    vm.add_method(
        test,
        id_lut!(fill),
        script_args_def!(selector = NIL, text = NIL),
        |vm, args| {
            let selector = match parse_selector(vm, script_value!(vm, args.selector), "test.fill selector") {
                Ok(selector) => selector,
                Err(err) => return err,
            };
            let text = match script_value_to_checked_string(vm, script_value!(vm, args.text), "test.fill text") {
                Ok(text) => text,
                Err(err) => return err,
            };
            let Some(app) = current_app(vm) else {
                return missing_app_error(vm, "test.fill");
            };
            if let Err(err) = app.locator(selector).try_fill(text) {
                return host_script_error(vm, err.message());
            }
            NIL
        },
    );

    vm.add_method(
        test,
        id_lut!(clear),
        script_args_def!(selector = NIL),
        |vm, args| {
            let selector = match parse_selector(vm, script_value!(vm, args.selector), "test.clear selector") {
                Ok(selector) => selector,
                Err(err) => return err,
            };
            let Some(app) = current_app(vm) else {
                return missing_app_error(vm, "test.clear");
            };
            if let Err(err) = app.locator(selector).try_clear() {
                return host_script_error(vm, err.message());
            }
            NIL
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
            let Some(app) = current_app(vm) else {
                return missing_app_error(vm, "test.type_text");
            };
            if let Err(err) = app.try_type_text(text) {
                return host_script_error(vm, err.message());
            }
            NIL
        },
    );

    vm.add_method(
        test,
        id_lut!(press_return),
        script_args_def!(),
        |vm, _args| {
            let Some(app) = current_app(vm) else {
                return missing_app_error(vm, "test.press_return");
            };
            if let Err(err) = app.try_press_return() {
                return host_script_error(vm, err.message());
            }
            NIL
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
            let Some(app) = current_app(vm) else {
                return missing_app_error(vm, "test.press_key");
            };
            if let Err(err) = app.try_press_key_with_modifiers(key_code, modifiers) {
                return host_script_error(vm, err.message());
            }
            NIL
        },
    );

    vm.add_method(
        test,
        id_lut!(scroll),
        script_args_def!(selector = NIL, sx = 0.0, sy = 0.0),
        |vm, args| {
            let selector = match parse_selector(vm, script_value!(vm, args.selector), "test.scroll selector") {
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
            let Some(app) = current_app(vm) else {
                return missing_app_error(vm, "test.scroll");
            };
            if let Err(err) = app.locator(selector).try_scroll(sx, sy) {
                return host_script_error(vm, err.message());
            }
            NIL
        },
    );

    vm.add_method(
        test,
        id_lut!(drag),
        script_args_def!(selector = NIL, dx = 0.0, dy = 0.0),
        |vm, args| {
            let selector = match parse_selector(vm, script_value!(vm, args.selector), "test.drag selector") {
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
            let Some(app) = current_app(vm) else {
                return missing_app_error(vm, "test.drag");
            };
            if let Err(err) = app.locator(selector).try_drag_by(dx, dy) {
                return host_script_error(vm, err.message());
            }
            NIL
        },
    );

    install_visibility_method(
        vm,
        test,
        id_lut!(wait_visible),
        |app, selector| app.locator(selector).try_wait_visible(),
    );
    install_visibility_method(
        vm,
        test,
        id_lut!(wait_hidden),
        |app, selector| app.locator(selector).try_wait_hidden(),
    );
    install_string_method(
        vm,
        test,
        id_lut!(wait_text),
        |app, selector, expected| app.locator(selector).try_wait_text(expected),
    );
    install_string_method(
        vm,
        test,
        id_lut!(wait_value),
        |app, selector, expected| app.locator(selector).try_wait_value(expected),
    );
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
    install_expect_method(
        vm,
        test,
        id_lut!(expect_text),
        |app, selector, expected| app.locator(selector).try_assert_text(expected),
    );
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
            let selector = match parse_selector(vm, script_value!(vm, args.selector), "test.wait_count selector") {
                Ok(selector) => selector,
                Err(err) => return err,
            };
            let count = match parse_usize(vm, script_value!(vm, args.count), "test.wait_count count") {
                Ok(value) => value,
                Err(err) => return err,
            };
            let Some(app) = current_app(vm) else {
                return missing_app_error(vm, "test.wait_count");
            };
            if let Err(err) = app.locator(selector).try_wait_count(count) {
                return host_script_error(vm, err.message());
            }
            NIL
        },
    );

    vm.add_method(
        test,
        id_lut!(snapshot),
        script_args_def!(selector = NIL),
        |vm, args| {
            let selector = match parse_selector(vm, script_value!(vm, args.selector), "test.snapshot selector") {
                Ok(selector) => selector,
                Err(err) => return err,
            };
            let Some(app) = current_app(vm) else {
                return missing_app_error(vm, "test.snapshot");
            };
            match app.locator(selector).try_snapshot() {
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
            let Some(app) = current_app(vm) else {
                return missing_app_error(vm, "test.snapshots");
            };
            let widgets = if selector_value.is_nil() {
                match app.try_widget_snapshot() {
                    Ok(widgets) => widgets,
                    Err(err) => return host_script_error(vm, err.message()),
                }
            } else {
                let selector = match parse_selector(vm, selector_value, "test.snapshots selector") {
                    Ok(selector) => selector,
                    Err(err) => return err,
                };
                match app.try_query_widgets(&selector, false) {
                    Ok(widgets) => widgets,
                    Err(err) => return host_script_error(vm, err.message()),
                }
            };
            widgets_to_value(vm, &widgets)
        },
    );

    vm.add_method(
        test,
        id_lut!(widget_dump),
        script_args_def!(),
        |vm, _args| {
            let Some(app) = current_app(vm) else {
                return missing_app_error(vm, "test.widget_dump");
            };
            match app.try_widget_dump() {
                Ok(dump) => vm.new_string_with(|_vm, out| out.push_str(&dump)),
                Err(err) => host_script_error(vm, err.message()),
            }
        },
    );

    vm.add_method(
        test,
        id_lut!(screenshot),
        script_args_def!(name = NIL),
        |vm, args| {
            let name_value = script_value!(vm, args.name);
            let Some(app) = current_app(vm) else {
                return missing_app_error(vm, "test.screenshot");
            };
            let screenshot_path = match app.try_screenshot() {
                Ok(path) => path,
                Err(err) => return host_script_error(vm, err.message()),
            };
            let output_path = if name_value.is_nil() {
                screenshot_path
            } else {
                let name = match script_value_to_checked_string(vm, name_value, "test.screenshot name") {
                    Ok(name) => name,
                    Err(err) => return err,
                };
                let output_path = app
                    .artifacts_dir()
                    .join(format!("{}.png", sanitize_path_component(&name)));
                if let Err(err) = fs::copy(&screenshot_path, &output_path) {
                    return host_script_error(
                        vm,
                        format!(
                            "failed to copy screenshot to {}: {}",
                            output_path.display(),
                            err
                        ),
                    );
                }
                output_path
            };
            vm.new_string_with(|_vm, out| out.push_str(&output_path.to_string_lossy()))
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
                match script_value_to_checked_string(vm, script_value!(vm, args.pattern), "test.logs pattern")
                {
                    Ok(pattern) => Some(pattern),
                    Err(err) => return err,
                }
            };
            let Some(app) = current_app(vm) else {
                return missing_app_error(vm, "test.logs");
            };
            let logs = match app.try_query_logs(pattern) {
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
            let pattern =
                match script_value_to_checked_string(vm, script_value!(vm, args.pattern), "test.wait_log pattern")
                {
                    Ok(pattern) => pattern,
                    Err(err) => return err,
                };
            let Some(app) = current_app(vm) else {
                return missing_app_error(vm, "test.wait_log");
            };
            if let Err(err) = app.try_wait_for_log_contains(&pattern) {
                return host_script_error(vm, err.message());
            }
            NIL
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
            let selector =
                match parse_selector(vm, script_value!(vm, args.selector), "selector") {
                    Ok(selector) => selector,
                    Err(err) => return err,
                };
            let Some(app) = current_app(vm) else {
                return missing_app_error(vm, "test wait/assert");
            };
            if let Err(err) = f(app, selector) {
                return host_script_error(vm, err.message());
            }
            NIL
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
            let selector =
                match parse_selector(vm, script_value!(vm, args.selector), "selector") {
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
            let Some(app) = current_app(vm) else {
                return missing_app_error(vm, "test string assert");
            };
            if let Err(err) = f(app, selector, expected) {
                return host_script_error(vm, err.message());
            }
            NIL
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
            let selector =
                match parse_selector(vm, script_value!(vm, args.selector), "selector") {
                    Ok(selector) => selector,
                    Err(err) => return err,
                };
            let expected = match script_value_to_checked_string(vm, script_value!(vm, args.expected), "expected")
            {
                Ok(expected) => expected,
                Err(err) => return err,
            };
            let Some(app) = current_app(vm) else {
                return missing_app_error(vm, "test assert");
            };
            if let Err(err) = f(app, selector, expected) {
                return host_script_error(vm, err.message());
            }
            NIL
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
            let selector =
                match parse_selector(vm, script_value!(vm, args.selector), "selector") {
                    Ok(selector) => selector,
                    Err(err) => return err,
                };
            let expected = match parse_bool(vm, script_value!(vm, args.expected), "expected") {
                Ok(expected) => expected,
                Err(err) => return err,
            };
            let Some(app) = current_app(vm) else {
                return missing_app_error(vm, "test bool assert");
            };
            if let Err(err) = f(app, selector, expected) {
                return host_script_error(vm, err.message());
            }
            NIL
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

fn missing_app_error(vm: &mut ScriptVm, method: &str) -> ScriptValue {
    host_script_error(
        vm,
        format!("{method} can only be used while a Splash test case is running"),
    )
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

fn parse_suite_options(vm: &mut ScriptVm, value: ScriptValue) -> Result<SplashSuiteOptions, ScriptValue> {
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

fn parse_selector(vm: &mut ScriptVm, value: ScriptValue, what: &str) -> Result<Selector, ScriptValue> {
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
        widget_type: optional_string_field(vm, object, id!(widget_type), &format!("{what}.widget_type"))?,
        raw: optional_string_field(vm, object, id!(raw), &format!("{what}.raw"))?,
        text_exact: optional_string_field(vm, object, id!(text_exact), &format!("{what}.text_exact"))?,
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
        any_window: optional_bool_field(vm, object, id!(any_window), &format!("{what}.any_window"))?
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
        shift: optional_bool_field(vm, object, id!(shift), &format!("{what}.shift"))?.unwrap_or(false),
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
        .flat_map(|ch| ch.to_ascii_lowercase().to_string().chars().collect::<Vec<_>>())
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
    value.as_number().ok_or_else(|| {
        script_err_type_mismatch!(vm.trap(), "{} expects a number", what)
    })
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
    vm.bx
        .heap
        .set_value_def(object, id!(window_index).into(), (widget.window_index as f64).into());
    vm.bx
        .heap
        .set_value_def(object, id!(visible).into(), widget.visible.into());
    vm.bx
        .heap
        .set_value_def(object, id!(enabled).into(), widget.enabled.into());
    vm.bx.heap.set_value_def(object, id!(x).into(), (widget.x as f64).into());
    vm.bx.heap.set_value_def(object, id!(y).into(), (widget.y as f64).into());
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
            vm.bx.heap.set_value_def(object, id!(line).into(), (line as f64).into());
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
    use super::{discover_splash_mount_root, SplashSuiteRunner, SplashLaunch};
    use crate::TestConfig;
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
        let config = TestConfig::current_package(
            "/tmp/example",
            "makepad-example",
            "ui::splash_case_name",
        )
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
        let err = runner
            .host
            .case("missing")
            .err()
            .unwrap();

        assert!(err.message().contains("was not registered"));
    }
}

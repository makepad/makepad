use crate::dispatch::HubEvent;
use makepad_micro_serde::DeJson;
use makepad_studio_protocol::hub_protocol::{BuildInfo, QueryId};
use makepad_studio_protocol::AppToStudio;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn studio_hub_debug_enabled() -> bool {
    env::var_os("MAKEPAD_STUDIO_HUB_DEBUG").is_some()
}

#[cfg(windows)]
mod process_group {
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::process::{Child, Command};

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(lp_job_attributes: *mut u8, lp_name: *const u16) -> *mut u8;
        fn AssignProcessToJobObject(h_job: *mut u8, h_process: *mut u8) -> i32;
        fn TerminateJobObject(h_job: *mut u8, exit_code: u32) -> i32;
        fn CloseHandle(h_object: *mut u8) -> i32;
    }

    pub struct JobHandle(*mut u8);

    unsafe impl Send for JobHandle {}

    impl JobHandle {
        pub fn new() -> io::Result<Self> {
            unsafe {
                let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
                if job.is_null() {
                    return Err(io::Error::last_os_error());
                }
                Ok(JobHandle(job))
            }
        }

        pub fn assign(&mut self, child: &Child) -> io::Result<()> {
            unsafe {
                let process = child.as_raw_handle() as *mut u8;
                if AssignProcessToJobObject(self.0, process) == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            }
        }

        pub fn terminate(&self) -> io::Result<()> {
            unsafe {
                if TerminateJobObject(self.0, 1) == 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        }
    }

    impl Drop for JobHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    pub fn configure_command(_cmd: &mut Command) {}
}

#[cfg(unix)]
mod process_group {
    use std::io;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};

    pub struct JobHandle(u32);

    impl JobHandle {
        pub fn new() -> io::Result<Self> {
            Ok(Self(0))
        }

        pub fn assign(&mut self, child: &Child) -> io::Result<()> {
            self.0 = child.id();
            Ok(())
        }

        pub fn terminate(&self) -> io::Result<()> {
            if self.0 == 0 {
                return Ok(());
            }
            unsafe {
                if kill(-(self.0 as i32), SIGKILL) == -1 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        }
    }

    pub fn configure_command(cmd: &mut Command) {
        unsafe {
            cmd.pre_exec(|| {
                if setpgid(0, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    const SIGKILL: i32 = 9;

    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
        fn setpgid(pid: i32, pgid: i32) -> i32;
    }
}

#[cfg(not(any(unix, windows)))]
mod process_group {
    use std::io;
    use std::process::{Child, Command};

    pub struct JobHandle;

    impl JobHandle {
        pub fn new() -> io::Result<Self> {
            Ok(Self)
        }

        pub fn assign(&mut self, _child: &Child) -> io::Result<()> {
            Ok(())
        }

        pub fn terminate(&self) -> io::Result<()> {
            Ok(())
        }
    }

    pub fn configure_command(_cmd: &mut Command) {}
}

struct RunningBuild {
    info: BuildInfo,
    child: RunningChild,
}

struct RunningChild {
    child: Arc<Mutex<Child>>,
    job: process_group::JobHandle,
}

impl RunningChild {
    fn new(child: Child) -> io::Result<Self> {
        let mut job = process_group::JobHandle::new()?;
        job.assign(&child)?;
        Ok(Self {
            child: Arc::new(Mutex::new(child)),
            job,
        })
    }

    fn child(&self) -> Arc<Mutex<Child>> {
        Arc::clone(&self.child)
    }

    fn terminate(&self) -> Result<(), String> {
        match self.job.terminate() {
            Ok(()) => Ok(()),
            Err(group_err) => {
                let mut child = self
                    .child
                    .lock()
                    .map_err(|_| "build process lock poisoned".to_string())?;
                match child.kill() {
                    Ok(()) => Ok(()),
                    Err(kill_err) if kill_err.kind() == io::ErrorKind::InvalidInput => Ok(()),
                    Err(kill_err) => Err(format!(
                        "failed to stop process group ({group_err}); fallback kill failed: {kill_err}"
                    )),
                }
            }
        }
    }

    fn send_stdin(&self, text: &str) -> Result<(), String> {
        if studio_hub_debug_enabled() {
            eprintln!(
                "studio hub debug: child stdin write {}",
                text.trim_end_matches(&['\r', '\n'][..])
            );
        }
        let mut child = self
            .child
            .lock()
            .map_err(|_| "build process lock poisoned".to_string())?;
        let Some(stdin) = child.stdin.as_mut() else {
            return Err("build process stdin is not available".to_string());
        };
        stdin
            .write_all(text.as_bytes())
            .map_err(|err| format!("failed to write build stdin: {err}"))?;
        stdin
            .flush()
            .map_err(|err| format!("failed to flush build stdin: {err}"))?;
        Ok(())
    }
}

fn normalize_studio_host(base: Option<&str>) -> String {
    let Some(base) = base.map(str::trim).filter(|base| !base.is_empty()) else {
        return String::new();
    };
    let normalized = base.trim_end_matches('/');
    let without_scheme = normalized
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(normalized);
    let host_port = without_scheme
        .split_once(['/', '?', '#'])
        .map(|(host_port, _)| host_port)
        .unwrap_or(without_scheme)
        .trim();
    host_port.to_string()
}

fn studio_query_value(studio: &str, key: &str) -> Option<String> {
    let query = studio.split_once('?')?.1;
    for pair in query.split('&') {
        let (pair_key, pair_value) = pair.split_once('=').unwrap_or((pair, ""));
        if pair_key == key {
            let pair_value = pair_value.trim();
            if !pair_value.is_empty() {
                return Some(pair_value.to_string());
            }
        }
    }
    None
}

fn extract_studio_build_id(studio: &str) -> Option<String> {
    studio_query_value(studio, "build").or_else(|| {
        let studio = studio.trim().trim_end_matches('/');
        let without_scheme = studio
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(studio);
        let path = without_scheme
            .split_once('/')
            .map(|(_, path)| path)
            .unwrap_or("");
        let rest = path.strip_prefix("app/")?;
        let build_id = rest.split('/').next()?.trim();
        (!build_id.is_empty()).then(|| build_id.to_string())
    })
}

fn extract_studio_crate_name(studio: &str) -> Option<String> {
    studio_query_value(studio, "crate")
}

#[derive(Default)]
pub struct BuildManager {
    builds: HashMap<QueryId, RunningBuild>,
}

impl BuildManager {
    pub fn start_command_run(
        &mut self,
        build_id: QueryId,
        mount: String,
        package: String,
        cwd: &Path,
        program: String,
        args: Vec<String>,
        env: HashMap<String, String>,
        inject_studio_env: bool,
        studio_addr: Option<String>,
        event_tx: Sender<HubEvent>,
    ) -> Result<BuildInfo, String> {
        if self.builds.contains_key(&build_id) {
            return Err(format!("build already exists: {}", build_id.0));
        }

        let mut command = Command::new(&program);
        process_group::configure_command(&mut command);
        command.args(&args);
        command.current_dir(cwd);
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        let mut child_env = env;
        child_env
            .entry("RUST_BACKTRACE".to_string())
            .or_insert_with(|| "1".to_string());
        child_env
            .entry("MAKEPAD".to_string())
            .or_insert_with(|| "lines".to_string());

        let resolved_studio_host = child_env
            .get("STUDIO_HOST")
            .map(String::as_str)
            .map(|host| normalize_studio_host(Some(host)))
            .filter(|host| !host.is_empty())
            .or_else(|| {
                child_env.get("STUDIO").and_then(|studio| {
                    let host = normalize_studio_host(Some(studio));
                    (!host.is_empty()).then_some(host)
                })
            })
            .or_else(|| {
                inject_studio_env
                    .then_some(studio_addr.as_deref())
                    .flatten()
                    .map(|host| normalize_studio_host(Some(host)))
                    .filter(|host| !host.is_empty())
            });
        if let Some(studio_host) = resolved_studio_host {
            child_env.insert("STUDIO_HOST".to_string(), studio_host);
            let studio_build = child_env
                .get("STUDIO_BUILD")
                .cloned()
                .filter(|build| !build.trim().is_empty())
                .or_else(|| {
                    child_env
                        .get("STUDIO")
                        .and_then(|studio| extract_studio_build_id(studio))
                })
                .unwrap_or_else(|| build_id.0.to_string());
            child_env.insert("STUDIO_BUILD".to_string(), studio_build);
            let studio_crate = child_env
                .get("STUDIO_CRATE")
                .cloned()
                .filter(|crate_name| !crate_name.trim().is_empty())
                .or_else(|| {
                    child_env
                        .get("STUDIO")
                        .and_then(|studio| extract_studio_crate_name(studio))
                })
                .unwrap_or_else(|| package.clone());
            child_env.insert("STUDIO_CRATE".to_string(), studio_crate);
            child_env.remove("STUDIO");
        }
        command.envs(child_env.iter());

        let mut child = command.spawn().map_err(|err| {
            format!(
                "failed to spawn {} in {}: {}",
                program,
                cwd.to_string_lossy(),
                err
            )
        })?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let child = RunningChild::new(child).map_err(|err| {
            format!(
                "failed to configure process group for {} in {}: {}",
                program,
                cwd.to_string_lossy(),
                err
            )
        })?;

        if let Some(stdout) = stdout {
            spawn_reader(build_id, false, stdout, event_tx.clone());
        }
        if let Some(stderr) = stderr {
            spawn_reader(build_id, true, stderr, event_tx.clone());
        }
        spawn_waiter(build_id, child.child(), event_tx);

        let info = BuildInfo {
            build_id,
            mount,
            package,
            active: true,
        };
        self.builds.insert(
            build_id,
            RunningBuild {
                info: info.clone(),
                child,
            },
        );
        Ok(info)
    }

    pub fn start_cargo_run(
        &mut self,
        build_id: QueryId,
        mount: String,
        cwd: &Path,
        args: Vec<String>,
        env: HashMap<String, String>,
        studio_addr: Option<String>,
        event_tx: Sender<HubEvent>,
    ) -> Result<BuildInfo, String> {
        let package = parse_package_name(&args).unwrap_or_else(|| "unknown".to_string());
        #[cfg(unix)]
        if should_use_direct_stdio_run(&args, &env) {
            if let Some(script) = build_direct_stdio_run_script(cwd, &args, &env) {
                return self.start_command_run(
                    build_id,
                    mount,
                    package,
                    cwd,
                    "/bin/sh".to_string(),
                    vec!["-lc".to_string(), script],
                    env,
                    false,
                    studio_addr,
                    event_tx,
                );
            }
        }
        self.start_command_run(
            build_id,
            mount,
            package,
            cwd,
            "cargo".to_string(),
            args,
            env,
            true,
            studio_addr,
            event_tx,
        )
    }

    pub fn stop_build(&mut self, build_id: QueryId) -> Result<(), String> {
        let Some(build) = self.builds.get(&build_id) else {
            return Err(format!("unknown build: {}", build_id.0));
        };
        if let Err(err) = build.child.terminate() {
            return Err(format!("failed to stop build {}: {}", build_id.0, err));
        }
        Ok(())
    }

    pub fn mark_exited(&mut self, build_id: QueryId, exit_code: Option<i32>) -> Option<BuildInfo> {
        let mut info = self.builds.remove(&build_id)?.info;
        info.active = false;
        let _ = exit_code;
        Some(info)
    }

    pub fn send_stdin(&self, build_id: QueryId, text: &str) -> Result<(), String> {
        let Some(build) = self.builds.get(&build_id) else {
            return Err(format!("unknown build: {}", build_id.0));
        };
        build.child.send_stdin(text)
    }

    pub fn list_builds(&self) -> Vec<BuildInfo> {
        let mut builds: Vec<BuildInfo> = self.builds.values().map(|b| b.info.clone()).collect();
        builds.sort_by_key(|b| b.build_id.0);
        builds
    }

    pub fn package_for_build(&self, build_id: QueryId) -> Option<&str> {
        self.builds
            .get(&build_id)
            .map(|build| build.info.package.as_str())
    }
}

fn spawn_reader<R: Read + Send + 'static>(
    build_id: QueryId,
    is_stderr: bool,
    reader: R,
    event_tx: std::sync::mpsc::Sender<HubEvent>,
) {
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    let line = line.trim_end_matches(&['\r', '\n'][..]).to_string();
                    if !is_stderr {
                        if let Ok(msg) = AppToStudio::deserialize_json(&line) {
                            if studio_hub_debug_enabled() {
                                eprintln!("studio hub debug: child stdout app msg {}", line);
                            }
                            let _ = event_tx.send(HubEvent::ProcessAppMessage { build_id, msg });
                            continue;
                        }
                    }
                    if studio_hub_debug_enabled() {
                        eprintln!(
                            "studio hub debug: child {} {}",
                            if is_stderr { "stderr" } else { "stdout" },
                            line
                        );
                    }
                    let _ = event_tx.send(HubEvent::ProcessOutput {
                        build_id,
                        is_stderr,
                        line,
                    });
                }
                Err(_) => break,
            }
        }
    });
}

fn spawn_waiter(
    build_id: QueryId,
    child: Arc<Mutex<Child>>,
    event_tx: std::sync::mpsc::Sender<HubEvent>,
) {
    thread::spawn(move || loop {
        let exited = {
            let mut child = match child.lock() {
                Ok(child) => child,
                Err(_) => return,
            };
            match child.try_wait() {
                Ok(Some(status)) => Some(status.code()),
                Ok(None) => None,
                Err(_) => Some(None),
            }
        };

        if let Some(exit_code) = exited {
            let _ = event_tx.send(HubEvent::ProcessExited {
                build_id,
                exit_code,
            });
            break;
        }

        thread::sleep(Duration::from_millis(30));
    });
}

#[derive(Debug, Default, PartialEq, Eq)]
struct CargoManifestTargets {
    package_name: Option<String>,
    bin_names: Vec<String>,
}

impl CargoManifestTargets {
    fn default_binary_name(&self) -> Option<String> {
        match self.bin_names.as_slice() {
            [] => self.package_name.clone(),
            [single] => Some(single.clone()),
            _ => self
                .package_name
                .as_ref()
                .filter(|package_name| self.bin_names.iter().any(|bin| bin == *package_name))
                .cloned(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CargoManifestSection {
    Other,
    Package,
    Bin,
}

fn parse_package_name(args: &[String]) -> Option<String> {
    parse_package_arg(args).or_else(|| parse_bin_arg(args))
}

fn parse_package_arg(args: &[String]) -> Option<String> {
    parse_cargo_flag_value(args, &["-p", "--package"])
}

fn parse_bin_arg(args: &[String]) -> Option<String> {
    parse_cargo_flag_value(args, &["--bin"])
}

fn parse_cargo_flag_value(args: &[String], flags: &[&str]) -> Option<String> {
    let mut i = 0usize;
    while i < args.len() {
        let arg = args[i].as_str();
        if flags.contains(&arg) && i + 1 < args.len() {
            return Some(args[i + 1].clone());
        }
        for flag in flags {
            let prefix = format!("{flag}=");
            if arg.starts_with(&prefix) {
                return arg.split_once('=').map(|(_, value)| value.to_string());
            }
        }
        i += 1;
    }
    None
}

fn parse_manifest_targets(manifest: &str) -> CargoManifestTargets {
    let mut targets = CargoManifestTargets::default();
    let mut section = CargoManifestSection::Other;

    for raw_line in manifest.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = match line {
                "[package]" => CargoManifestSection::Package,
                "[[bin]]" => CargoManifestSection::Bin,
                _ => CargoManifestSection::Other,
            };
            continue;
        }
        let Some(name) = parse_manifest_string_value(line, "name") else {
            continue;
        };
        match section {
            CargoManifestSection::Package => {
                if targets.package_name.is_none() {
                    targets.package_name = Some(name);
                }
            }
            CargoManifestSection::Bin => targets.bin_names.push(name),
            CargoManifestSection::Other => {}
        }
    }

    targets
}

fn parse_manifest_string_value(line: &str, key: &str) -> Option<String> {
    let (lhs, rhs) = line.split_once('=')?;
    if lhs.trim() != key {
        return None;
    }
    let rhs = rhs.trim();
    let value = rhs.strip_prefix('"')?;
    let end = value.find('"')?;
    Some(value[..end].to_string())
}

fn read_manifest_targets(cwd: &Path) -> Option<CargoManifestTargets> {
    let manifest_path = cwd.join("Cargo.toml");
    let manifest = fs::read_to_string(manifest_path).ok()?;
    Some(parse_manifest_targets(&manifest))
}

fn resolve_direct_stdio_binary_name(cwd: &Path, args: &[String]) -> Option<String> {
    if let Some(bin) = parse_bin_arg(args) {
        return Some(bin);
    }

    let targets = read_manifest_targets(cwd)?;
    if let Some(package) = parse_package_arg(args) {
        if targets.package_name.as_deref() != Some(package.as_str()) {
            return None;
        }
    }
    targets.default_binary_name()
}

fn should_use_direct_stdio_run(args: &[String], env: &HashMap<String, String>) -> bool {
    env.get("MAKEPAD").is_some_and(|value| value == "headless")
        && args.first().is_some_and(|arg| arg == "run")
        && args.iter().any(|arg| arg == "--stdin-loop")
}

#[cfg(unix)]
fn build_direct_stdio_run_script(
    cwd: &Path,
    args: &[String],
    env: &HashMap<String, String>,
) -> Option<String> {
    let app_args_index = args.iter().position(|arg| arg == "--")?;
    let cargo_run_args = args.get(..app_args_index)?;
    if cargo_run_args.first().map(String::as_str) != Some("run") {
        return None;
    }

    let executable_name = resolve_direct_stdio_binary_name(cwd, args)?;
    let app_args = &args[app_args_index + 1..];
    let binary_path = cargo_target_dir(cwd, env)
        .join("release")
        .join(executable_name);
    let mut script = String::from("cargo");
    for arg in cargo_run_args {
        script.push(' ');
        if arg == "run" {
            script.push_str("build");
        } else {
            script.push_str(&shell_escape(arg));
        }
    }
    script.push_str(" && exec ");
    script.push_str(&shell_escape(binary_path.to_string_lossy().as_ref()));
    for arg in app_args {
        script.push(' ');
        script.push_str(&shell_escape(arg));
    }
    Some(script)
}

#[cfg(unix)]
fn cargo_target_dir(cwd: &Path, env: &HashMap<String, String>) -> PathBuf {
    env.get("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.join("target"))
}

#[cfg(unix)]
fn shell_escape(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_manifest_targets_reads_single_bin_name() {
        let targets = parse_manifest_targets(
            r#"
                [package]
                name = "makepad-widgets-test"

                [[bin]]
                name = "widget_tree_test"
                path = "src/main.rs"
            "#,
        );

        assert_eq!(
            targets,
            CargoManifestTargets {
                package_name: Some("makepad-widgets-test".to_string()),
                bin_names: vec!["widget_tree_test".to_string()],
            }
        );
        assert_eq!(
            targets.default_binary_name(),
            Some("widget_tree_test".to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn build_direct_stdio_run_script_uses_resolved_bin_name() {
        let dir = crate::test_support::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            r#"
                [package]
                name = "makepad-widgets-test"
                version = "0.1.0"
                edition = "2021"

                [[bin]]
                name = "widget_tree_test"
                path = "src/main.rs"
            "#,
        )
        .unwrap();

        let args = vec![
            "run".to_string(),
            "-p".to_string(),
            "makepad-widgets-test".to_string(),
            "--release".to_string(),
            "--message-format=json".to_string(),
            "--".to_string(),
            "--message-format=json".to_string(),
            "--stdin-loop".to_string(),
        ];

        let script = build_direct_stdio_run_script(dir.path(), &args, &HashMap::new()).unwrap();

        assert!(script.contains("cargo build '-p' 'makepad-widgets-test'"));
        assert!(script.contains("/release/widget_tree_test"));
    }
}

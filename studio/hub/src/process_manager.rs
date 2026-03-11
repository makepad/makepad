use crate::dispatch::HubEvent;
use makepad_script_std::makepad_network::{NetworkConfig, NetworkRuntime};
use makepad_script_std::makepad_script::*;
use makepad_script_std::{
    pump, pump_network_runtime, script_mod as script_std_mod, with_vm_and_async, ScriptStd,
};
use makepad_studio_protocol::hub_protocol::{BuildInfo, QueryId};
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub const MAKEPAD_SPLASH_RUNNABLE: &str = "makepad.splash";

enum RunningBuildHandle {
    Child(Arc<Mutex<Child>>),
    Script(Arc<AtomicBool>),
}

struct RunningBuild {
    info: BuildInfo,
    handle: RunningBuildHandle,
}

struct ScriptBuildHost {
    build_id: QueryId,
    event_tx: std::sync::mpsc::Sender<HubEvent>,
    stop: Arc<AtomicBool>,
}

impl ScriptBuildHost {
    fn emit_output(&self, line: String, is_stderr: bool) {
        let _ = self.event_tx.send(HubEvent::ProcessOutput {
            build_id: self.build_id,
            is_stderr,
            line,
        });
    }

    fn emit_exit(&self, exit_code: Option<i32>) {
        let _ = self.event_tx.send(HubEvent::ProcessExited {
            build_id: self.build_id,
            exit_code,
        });
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

fn install_hub_script_stdio(vm: &mut ScriptVm) {
    fn script_value_to_string(vm: &mut ScriptVm, value: ScriptValue) -> String {
        if let Some(line) = vm.string_with(value, |_vm, s| s.to_string()) {
            return line;
        }
        vm.bx.heap.temp_string_with(|heap, temp| {
            heap.cast_to_string(value, temp);
            temp.clone()
        })
    }

    let std = vm.module(id!(std));

    vm.add_method(std, id_lut!(print), script_args_def!(what = NIL), |vm, args| {
        let what = script_value!(vm, args.what);
        let line = script_value_to_string(vm, what);
        vm.host
            .downcast_mut::<ScriptBuildHost>()
            .unwrap()
            .emit_output(line.clone(), false);
        print!("{line}");
        let _ = io::stdout().flush();
        NIL
    });

    vm.add_method(std, id_lut!(println), script_args_def!(what = NIL), |vm, args| {
        let what = script_value!(vm, args.what);
        let line = script_value_to_string(vm, what);
        vm.host
            .downcast_mut::<ScriptBuildHost>()
            .unwrap()
            .emit_output(line.clone(), false);
        println!("{line}");
        NIL
    });
}

fn has_pending_script_work(std: &ScriptStd) -> bool {
    if !std.data.child_processes.is_empty()
        || !std.data.web_sockets.is_empty()
        || !std.data.http_requests.is_empty()
        || !std.data.http_servers.is_empty()
        || !std.data.socket_streams.borrow().is_empty()
        || !std.data.tasks.pending_resumes.is_empty()
    {
        return true;
    }

    std.data.tasks.tasks.borrow().iter().any(|task| {
        task.start_task.is_some()
            || !task.recv_pause.is_empty()
            || !task.send_pause.is_empty()
            || !task.ended
    })
}

fn normalize_script_source(source: &str) -> String {
    let mut normalized = source.to_string();
    if !normalized.trim_end().ends_with(';') {
        normalized.push(';');
    }
    normalized
}

fn run_script_build(
    build_id: QueryId,
    cwd: &Path,
    splash_path: &Path,
    stop: Arc<AtomicBool>,
    event_tx: std::sync::mpsc::Sender<HubEvent>,
) {
    let source = match fs::read_to_string(splash_path) {
        Ok(source) => source,
        Err(err) => {
            let host = ScriptBuildHost {
                build_id,
                event_tx,
                stop,
            };
            host.emit_output(
                format!("failed to read {}: {}", splash_path.to_string_lossy(), err),
                true,
            );
            host.emit_exit(Some(1));
            return;
        }
    };
    let code = normalize_script_source(&source);

    let mut host = ScriptBuildHost {
        build_id,
        event_tx,
        stop,
    };
    let runtime = Arc::new(NetworkRuntime::new(NetworkConfig::default()));
    let mut std = ScriptStd::with_network_runtime(runtime);
    let mut script_vm = Some(Box::new(ScriptVmBase::new()));
    let script_mod = ScriptMod {
        cargo_manifest_path: cwd.to_string_lossy().to_string(),
        module_path: MAKEPAD_SPLASH_RUNNABLE.to_string(),
        file: splash_path.to_string_lossy().to_string(),
        line: 1,
        column: 1,
        code,
        values: Vec::new(),
    };

    let result = with_vm_and_async(&mut host, &mut std, &mut script_vm, |vm| {
        script_std_mod(vm);
        install_hub_script_stdio(vm);
        vm.eval(script_mod)
    });

    if result.is_err() {
        host.emit_output(
            format!("script failed while evaluating {}", splash_path.to_string_lossy()),
            true,
        );
        host.emit_exit(Some(1));
        return;
    }

    loop {
        if host.stopped() {
            host.emit_exit(None);
            return;
        }

        pump(&mut host, &mut std, &mut script_vm);
        let _ = pump_network_runtime(&mut host, &mut std, &mut script_vm);

        if !has_pending_script_work(&std) {
            host.emit_exit(Some(0));
            return;
        }

        thread::sleep(Duration::from_millis(16));
    }
}

#[derive(Default)]
pub struct ProcessManager {
    builds: HashMap<QueryId, RunningBuild>,
}

impl ProcessManager {
    pub fn start_cargo_run(
        &mut self,
        build_id: QueryId,
        mount: String,
        cwd: &Path,
        args: Vec<String>,
        env: HashMap<String, String>,
        studio_addr: Option<String>,
        event_tx: std::sync::mpsc::Sender<HubEvent>,
    ) -> Result<BuildInfo, String> {
        if self.builds.contains_key(&build_id) {
            return Err(format!("build already exists: {}", build_id.0));
        }

        let package = parse_package_name(&args).unwrap_or_else(|| "unknown".to_string());
        let mut command = Command::new("cargo");
        command.args(&args);
        command.current_dir(cwd);
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        let mut child_env = env;
        child_env.insert("RUST_BACKTRACE".to_string(), "1".to_string());
        child_env.insert("MAKEPAD".to_string(), "lines".to_string());

        if let Some(studio_addr) = studio_addr.as_deref().map(str::trim) {
            if !studio_addr.is_empty() {
                child_env.insert("STUDIO".to_string(), studio_addr.to_string());
                child_env.insert("STUDIO_BUILD_ID".to_string(), build_id.0.to_string());
            }
        } else if child_env
            .get("STUDIO")
            .is_some_and(|studio| !studio.trim().is_empty())
        {
            child_env.insert("STUDIO_BUILD_ID".to_string(), build_id.0.to_string());
        }
        command.envs(child_env.iter());

        let mut child = command.spawn().map_err(|err| {
            format!(
                "failed to spawn cargo in {}: {}",
                cwd.to_string_lossy(),
                err
            )
        })?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(child));

        if let Some(stdout) = stdout {
            spawn_reader(build_id, false, stdout, event_tx.clone());
        }
        if let Some(stderr) = stderr {
            spawn_reader(build_id, true, stderr, event_tx.clone());
        }
        spawn_waiter(build_id, Arc::clone(&child), event_tx);

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
                handle: RunningBuildHandle::Child(child),
            },
        );
        Ok(info)
    }

    pub fn start_script_run(
        &mut self,
        build_id: QueryId,
        mount: String,
        cwd: &Path,
        event_tx: std::sync::mpsc::Sender<HubEvent>,
    ) -> Result<BuildInfo, String> {
        if self.builds.contains_key(&build_id) {
            return Err(format!("build already exists: {}", build_id.0));
        }

        let splash_path = cwd.join(MAKEPAD_SPLASH_RUNNABLE);
        if !splash_path.is_file() {
            return Err(format!(
                "missing {} in {}",
                MAKEPAD_SPLASH_RUNNABLE,
                cwd.to_string_lossy()
            ));
        }

        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread_cwd = cwd.to_path_buf();
        let thread_splash = splash_path.clone();
        thread::spawn(move || {
            run_script_build(build_id, &thread_cwd, &thread_splash, thread_stop, event_tx);
        });

        let info = BuildInfo {
            build_id,
            mount,
            package: MAKEPAD_SPLASH_RUNNABLE.to_string(),
            active: true,
        };
        self.builds.insert(
            build_id,
            RunningBuild {
                info: info.clone(),
                handle: RunningBuildHandle::Script(stop),
            },
        );
        Ok(info)
    }

    pub fn stop_build(&mut self, build_id: QueryId) -> Result<(), String> {
        let Some(build) = self.builds.get(&build_id) else {
            return Err(format!("unknown build: {}", build_id.0));
        };

        match &build.handle {
            RunningBuildHandle::Child(child) => {
                let mut child = child
                    .lock()
                    .map_err(|_| "build process lock poisoned".to_string())?;
                if let Err(err) = child.kill() {
                    if err.kind() != std::io::ErrorKind::InvalidInput {
                        return Err(format!("failed to stop build {}: {}", build_id.0, err));
                    }
                }
            }
            RunningBuildHandle::Script(stop) => {
                stop.store(true, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    pub fn mark_exited(&mut self, build_id: QueryId, exit_code: Option<i32>) -> Option<BuildInfo> {
        let mut info = self.builds.remove(&build_id)?.info;
        info.active = false;
        let _ = exit_code;
        Some(info)
    }

    pub fn list_builds(&self) -> Vec<BuildInfo> {
        let mut builds: Vec<BuildInfo> = self.builds.values().map(|b| b.info.clone()).collect();
        builds.sort_by_key(|b| b.build_id.0);
        builds
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

fn parse_package_name(args: &[String]) -> Option<String> {
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "-p" | "--package" if i + 1 < args.len() => return Some(args[i + 1].clone()),
            "--bin" if i + 1 < args.len() => return Some(args[i + 1].clone()),
            arg if arg.starts_with("--package=") => {
                return arg.split_once('=').map(|(_, value)| value.to_string());
            }
            arg if arg.starts_with("--bin=") => {
                return arg.split_once('=').map(|(_, value)| value.to_string());
            }
            _ => {}
        }
        i += 1;
    }
    None
}

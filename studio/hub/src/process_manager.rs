use crate::dispatch::HubEvent;
use makepad_script_std::makepad_network::{NetworkConfig, NetworkRuntime};
use makepad_script_std::makepad_script::*;
use makepad_script_std::{
    pump, pump_network_runtime, script_mod as script_std_mod, with_vm_and_async, ScriptStd,
};
use makepad_studio_protocol::hub_protocol::{BuildInfo, QueryId, RunItem};
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub const MAKEPAD_SPLASH_RUNNABLE: &str = "makepad.splash";

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

enum RunningBuildHandle {
    Child(RunningChild),
    Script(Arc<ScriptRunControl>),
}

struct RunningBuild {
    info: BuildInfo,
    handle: RunningBuildHandle,
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
}

enum ScriptRunCommand {
    RunItem { name: String },
}

struct ScriptRunControl {
    stop: Arc<AtomicBool>,
    command_tx: Sender<ScriptRunCommand>,
}

struct RegisteredRunItem {
    info: RunItem,
    item: ScriptObjectRef,
}

struct ScriptBuildHost {
    build_id: QueryId,
    mount: String,
    cwd: PathBuf,
    studio_addr: Option<String>,
    event_tx: Sender<HubEvent>,
    stop: Arc<AtomicBool>,
    command_rx: Receiver<ScriptRunCommand>,
    run_items: HashMap<String, RegisteredRunItem>,
    current_run_item_name: Option<String>,
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

    fn emit_run_items(&self, items: Vec<RunItem>) {
        let _ = self.event_tx.send(HubEvent::RunItemsUpdated {
            mount: self.mount.clone(),
            items,
        });
    }

    fn emit_run_request(&self, program: String, args: Vec<String>, env: HashMap<String, String>) {
        let _ = self.event_tx.send(HubEvent::ScriptRunRequest {
            mount: self.mount.clone(),
            cwd: self.cwd.clone(),
            program,
            args,
            env,
            package: self.current_run_item_name.clone(),
        });
    }

    fn has_registered_run_items(&self) -> bool {
        !self.run_items.is_empty()
    }
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

fn script_value_to_bool(value: ScriptValue) -> Option<bool> {
    value
        .as_bool()
        .or_else(|| value.as_number().map(|number| number != 0.0))
}

fn script_value_to_string_array(
    vm: &mut ScriptVm,
    value: ScriptValue,
    what: &str,
) -> Result<Vec<String>, ScriptValue> {
    let Some(array) = value.as_array() else {
        return Err(script_err_type_mismatch!(
            vm.trap(),
            "{} expects an array of strings",
            what
        ));
    };
    let len = vm.bx.heap.array_len(array);
    let mut out = Vec::with_capacity(len);
    for index in 0..len {
        let Some(value) = vm.bx.heap.array_storage(array).index(index) else {
            continue;
        };
        let item_what = format!("{}[{}]", what, index);
        out.push(
            match script_value_to_checked_string(vm, value, &item_what) {
                Ok(value) => value,
                Err(err) => return Err(err),
            },
        );
    }
    Ok(out)
}

fn script_value_to_string_map(
    vm: &mut ScriptVm,
    value: ScriptValue,
    what: &str,
) -> Result<HashMap<String, String>, ScriptValue> {
    if value.is_nil() {
        return Ok(HashMap::new());
    }
    let Some(object) = value.as_object() else {
        return Err(script_err_type_mismatch!(
            vm.trap(),
            "{} expects an object map",
            what
        ));
    };

    let mut pairs = Vec::new();
    vm.proto_map_iter_mut_with(object, &mut |_vm, map| {
        for (key, value) in map.iter() {
            pairs.push((*key, value.value));
        }
    });

    let mut out = HashMap::with_capacity(pairs.len());
    for (key, value) in pairs {
        let key = match script_value_to_checked_string(vm, key, &format!("{} key", what)) {
            Ok(key) => key,
            Err(err) => return Err(err),
        };
        let value = match script_value_to_checked_string(vm, value, &format!("{}[{}]", what, key)) {
            Ok(value) => value,
            Err(err) => return Err(err),
        };
        out.insert(key, value);
    }
    Ok(out)
}

fn parse_registered_run_item(
    vm: &mut ScriptVm,
    value: ScriptValue,
) -> Result<RegisteredRunItem, ScriptValue> {
    let Some(item) = value.as_object() else {
        return Err(script_err_type_mismatch!(
            vm.trap(),
            "hub.set_run_items expects an array of objects"
        ));
    };

    let name = match script_value_to_checked_string(
        vm,
        vm.bx.heap.value(item, id!(name).into(), vm.trap()),
        "hub.set_run_items item.name",
    ) {
        Ok(name) => name,
        Err(err) => return Err(err),
    };
    if name.trim().is_empty() {
        return Err(script_err_unexpected!(
            vm.trap(),
            "hub.set_run_items requires non-empty item names"
        ));
    }

    let in_studio = {
        let value = vm.bx.heap.value(item, id!(in_studio).into(), vm.trap());
        let Some(in_studio) = script_value_to_bool(value) else {
            return Err(script_err_type_mismatch!(
                vm.trap(),
                "hub.set_run_items item.in_studio must be a bool"
            ));
        };
        in_studio
    };

    let on_run = vm.bx.heap.value(item, id!(on_run).into(), vm.trap());
    let Some(on_run_obj) = on_run.as_object() else {
        return Err(script_err_type_mismatch!(
            vm.trap(),
            "hub.set_run_items item.on_run must be a function"
        ));
    };
    if !vm.bx.heap.is_fn(on_run_obj) {
        return Err(script_err_type_mismatch!(
            vm.trap(),
            "hub.set_run_items item.on_run must be a function"
        ));
    }

    Ok(RegisteredRunItem {
        info: RunItem { name, in_studio },
        item: vm.bx.heap.new_object_ref(item),
    })
}

fn install_hub_script_stdio(vm: &mut ScriptVm) {
    let std = vm.module(id!(std));

    vm.add_method(
        std,
        id_lut!(print),
        script_args_def!(what = NIL),
        |vm, args| {
            let what = script_value!(vm, args.what);
            let line = script_value_to_string(vm, what);
            vm.host
                .downcast_mut::<ScriptBuildHost>()
                .unwrap()
                .emit_output(line.clone(), false);
            print!("{line}");
            let _ = io::stdout().flush();
            NIL
        },
    );

    vm.add_method(
        std,
        id_lut!(println),
        script_args_def!(what = NIL),
        |vm, args| {
            let what = script_value!(vm, args.what);
            let line = script_value_to_string(vm, what);
            vm.host
                .downcast_mut::<ScriptBuildHost>()
                .unwrap()
                .emit_output(line.clone(), false);
            println!("{line}");
            NIL
        },
    );
}

fn install_hub_script_module(vm: &mut ScriptVm) {
    let hub = vm.new_module(id!(hub));
    let studio_ip = vm
        .host
        .downcast_ref::<ScriptBuildHost>()
        .and_then(|host| host.studio_addr.clone())
        .unwrap_or_default();
    let studio_ip = vm.new_string_with(|_vm, out| out.push_str(&studio_ip));
    vm.bx
        .heap
        .set_value_def(hub, id!(studio_ip).into(), studio_ip.into());

    vm.add_method(
        hub,
        id_lut!(run),
        script_args_def!(env = NIL, cmd = NIL, args = NIL),
        |vm, args| {
            let env = script_value!(vm, args.env);
            let cmd = script_value!(vm, args.cmd);
            let args = script_value!(vm, args.args);

            let env = match script_value_to_string_map(vm, env, "hub.run") {
                Ok(env) => env,
                Err(err) => return err,
            };
            let program = match script_value_to_checked_string(vm, cmd, "hub.run cmd") {
                Ok(program) => program,
                Err(err) => return err,
            };
            if program.trim().is_empty() {
                return script_err_unexpected!(vm.trap(), "hub.run requires a command");
            }
            let args = match script_value_to_string_array(vm, args, "hub.run") {
                Ok(args) => args,
                Err(err) => return err,
            };

            vm.host
                .downcast_mut::<ScriptBuildHost>()
                .unwrap()
                .emit_run_request(program, args, env);
            NIL
        },
    );

    vm.add_method(
        hub,
        id_lut!(set_run_items),
        script_args_def!(items = NIL),
        |vm, args| {
            let items = script_value!(vm, args.items);
            let Some(array) = items.as_array() else {
                return script_err_type_mismatch!(vm.trap(), "hub.set_run_items expects an array");
            };

            let len = vm.bx.heap.array_len(array);
            let mut registered = Vec::with_capacity(len);
            for index in 0..len {
                let Some(value) = vm.bx.heap.array_storage(array).index(index) else {
                    continue;
                };
                let item = match parse_registered_run_item(vm, value) {
                    Ok(item) => item,
                    Err(err) => return err,
                };
                if registered
                    .iter()
                    .any(|existing: &RegisteredRunItem| existing.info.name == item.info.name)
                {
                    return script_err_unexpected!(
                        vm.trap(),
                        "duplicate run item name {:?}",
                        item.info.name
                    );
                }
                registered.push(item);
            }

            let host = vm.host.downcast_mut::<ScriptBuildHost>().unwrap();
            host.run_items.clear();
            let mut infos = Vec::with_capacity(registered.len());
            for item in registered {
                infos.push(item.info.clone());
                host.run_items.insert(item.info.name.clone(), item);
            }
            host.emit_run_items(infos);
            NIL
        },
    );
}

fn run_pending_script_commands(
    host: &mut ScriptBuildHost,
    std: &mut ScriptStd,
    script_vm: &mut Option<Box<ScriptVmBase>>,
) {
    loop {
        let Ok(command) = host.command_rx.try_recv() else {
            break;
        };
        match command {
            ScriptRunCommand::RunItem { name } => {
                let Some(item) = host.run_items.get(&name).map(|item| item.item.clone()) else {
                    host.emit_output(format!("unknown run item {:?}", name), true);
                    continue;
                };
                let item_object = item.as_object();
                let on_run = with_vm_and_async(host, std, script_vm, |vm| {
                    vm.bx.heap.value(item_object, id!(on_run).into(), vm.trap())
                });
                let Some(on_run_object) = on_run.as_object() else {
                    host.emit_output(
                        format!("run item {:?} is missing an on_run function", name),
                        true,
                    );
                    continue;
                };
                host.current_run_item_name = Some(name.clone());
                let result = with_vm_and_async(host, std, script_vm, |vm| {
                    vm.call_with_me(on_run_object.into(), &[], item_object.into())
                });
                host.current_run_item_name = None;
                if result.is_err() {
                    let err = with_vm_and_async(host, std, script_vm, |vm| {
                        script_value_to_string(vm, result)
                    });
                    host.emit_output(format!("run item {:?} failed: {}", name, err), true);
                }
            }
        }
    }
}

fn has_pending_script_work(host: &ScriptBuildHost, std: &ScriptStd) -> bool {
    if !std.data.child_processes.is_empty()
        || !std.data.web_sockets.is_empty()
        || !std.data.http_requests.is_empty()
        || !std.data.http_servers.is_empty()
        || !std.data.socket_streams.borrow().is_empty()
        || !std.data.tasks.pending_resumes.is_empty()
    {
        return true;
    }

    host.has_registered_run_items()
        || std.data.tasks.tasks.borrow().iter().any(|task| {
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
    mount: String,
    cwd: &Path,
    splash_path: &Path,
    studio_addr: Option<String>,
    stop: Arc<AtomicBool>,
    command_rx: Receiver<ScriptRunCommand>,
    event_tx: Sender<HubEvent>,
) {
    let source = match fs::read_to_string(splash_path) {
        Ok(source) => source,
        Err(err) => {
            let host = ScriptBuildHost {
                build_id,
                mount,
                cwd: cwd.to_path_buf(),
                studio_addr,
                event_tx,
                stop,
                command_rx,
                run_items: HashMap::new(),
                current_run_item_name: None,
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
        mount,
        cwd: cwd.to_path_buf(),
        studio_addr,
        event_tx,
        stop,
        command_rx,
        run_items: HashMap::new(),
        current_run_item_name: None,
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
        install_hub_script_module(vm);
        vm.eval(script_mod)
    });

    if result.is_err() {
        let err = with_vm_and_async(&mut host, &mut std, &mut script_vm, |vm| {
            script_value_to_string(vm, result)
        });
        host.emit_output(
            format!(
                "script failed while evaluating {}: {}",
                splash_path.to_string_lossy(),
                err
            ),
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

        run_pending_script_commands(&mut host, &mut std, &mut script_vm);
        pump(&mut host, &mut std, &mut script_vm);
        let _ = pump_network_runtime(&mut host, &mut std, &mut script_vm);

        if !has_pending_script_work(&host, &std) {
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

        if inject_studio_env {
            if let Some(studio_addr) = studio_addr.as_deref().map(str::trim) {
                if !studio_addr.is_empty() {
                    child_env
                        .entry("STUDIO".to_string())
                        .or_insert_with(|| studio_addr.to_string());
                }
            }
        }

        if child_env
            .get("STUDIO")
            .is_some_and(|studio| !studio.trim().is_empty())
        {
            child_env.insert("STUDIO_BUILD_ID".to_string(), build_id.0.to_string());
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
                handle: RunningBuildHandle::Child(child),
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

    pub fn start_script_run(
        &mut self,
        build_id: QueryId,
        mount: String,
        cwd: &Path,
        studio_addr: Option<String>,
        event_tx: Sender<HubEvent>,
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
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        let control = Arc::new(ScriptRunControl {
            stop: Arc::clone(&stop),
            command_tx,
        });
        let thread_control = Arc::clone(&control);
        let thread_cwd = cwd.to_path_buf();
        let thread_splash = splash_path.clone();
        let thread_mount = mount.clone();
        let thread_studio_addr = studio_addr;
        thread::spawn(move || {
            run_script_build(
                build_id,
                thread_mount,
                &thread_cwd,
                &thread_splash,
                thread_studio_addr,
                Arc::clone(&thread_control.stop),
                command_rx,
                event_tx,
            );
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
                handle: RunningBuildHandle::Script(control),
            },
        );
        Ok(info)
    }

    pub fn invoke_script_run_item(&self, mount: &str, name: &str) -> Result<(), String> {
        let Some(build) = self.builds.values().find(|build| {
            build.info.active
                && build.info.mount == mount
                && build.info.package == MAKEPAD_SPLASH_RUNNABLE
        }) else {
            return Err(format!(
                "{} is not running for mount {}",
                MAKEPAD_SPLASH_RUNNABLE, mount
            ));
        };
        let RunningBuildHandle::Script(control) = &build.handle else {
            return Err(format!(
                "{} is not script-controlled for mount {}",
                MAKEPAD_SPLASH_RUNNABLE, mount
            ));
        };
        control
            .command_tx
            .send(ScriptRunCommand::RunItem {
                name: name.to_string(),
            })
            .map_err(|_| format!("failed to send run item {:?} to splash", name))
    }

    pub fn stop_build(&mut self, build_id: QueryId) -> Result<(), String> {
        let Some(build) = self.builds.get(&build_id) else {
            return Err(format!("unknown build: {}", build_id.0));
        };

        match &build.handle {
            RunningBuildHandle::Child(child) => {
                if let Err(err) = child.terminate() {
                    return Err(format!("failed to stop build {}: {}", build_id.0, err));
                }
            }
            RunningBuildHandle::Script(control) => {
                control.stop.store(true, Ordering::Relaxed);
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

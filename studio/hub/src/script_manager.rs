use crate::dispatch::HubEvent;
use makepad_script_std::makepad_network::{NetworkConfig, NetworkRuntime};
use makepad_script_std::makepad_script::*;
use makepad_script_std::{
    pump, pump_network_runtime, script_mod as script_std_mod, with_vm_and_async, ScriptStd,
};
use makepad_studio_protocol::hub_protocol::{QueryId, RunItem};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub const MAKEPAD_SPLASH_RUNNABLE: &str = "makepad.splash";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScriptId(pub u64);

enum ScriptCommand {
    RunItem {
        name: String,
        child_build_id: QueryId,
    },
}

struct ScriptControl {
    stop: Arc<AtomicBool>,
    command_tx: Sender<ScriptCommand>,
}

struct RunningScript {
    mount: String,
    control: Arc<ScriptControl>,
}

struct RegisteredRunItem {
    info: RunItem,
    item: ScriptObjectRef,
}

struct ScriptHost {
    script_id: ScriptId,
    mount: String,
    cwd: PathBuf,
    studio_local_addr: Option<String>,
    studio_ext_addr: Option<String>,
    event_tx: Sender<HubEvent>,
    stop: Arc<AtomicBool>,
    command_rx: Receiver<ScriptCommand>,
    run_items: HashMap<String, RegisteredRunItem>,
    current_run_item_name: Option<String>,
    current_child_build_id: Option<QueryId>,
}

impl ScriptHost {
    fn emit_output(&self, line: String, is_stderr: bool) {
        let _ = self.event_tx.send(HubEvent::ScriptOutput {
            script_id: self.script_id,
            mount: self.mount.clone(),
            is_stderr,
            line,
        });
    }

    fn emit_exit(&self, exit_code: Option<i32>) {
        let _ = self.event_tx.send(HubEvent::ScriptExited {
            script_id: self.script_id,
            mount: self.mount.clone(),
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
            child_build_id: self.current_child_build_id,
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

#[derive(Default)]
pub struct ScriptManager {
    next_script_id: u64,
    scripts: HashMap<ScriptId, RunningScript>,
    script_by_mount: HashMap<String, ScriptId>,
}

impl ScriptManager {
    pub fn start_script(
        &mut self,
        mount: String,
        cwd: &Path,
        studio_local_addr: Option<String>,
        studio_ext_addr: Option<String>,
        event_tx: Sender<HubEvent>,
    ) -> Result<ScriptId, String> {
        if let Some(script_id) = self.script_by_mount.get(&mount) {
            return Err(format!(
                "{} is already running for mount {} as script {}",
                MAKEPAD_SPLASH_RUNNABLE, mount, script_id.0
            ));
        }

        let splash_path = cwd.join(MAKEPAD_SPLASH_RUNNABLE);
        if !splash_path.is_file() {
            return Err(format!(
                "missing {} in {}",
                MAKEPAD_SPLASH_RUNNABLE,
                cwd.to_string_lossy()
            ));
        }

        let script_id = self.alloc_script_id();
        let stop = Arc::new(AtomicBool::new(false));
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        let control = Arc::new(ScriptControl {
            stop: Arc::clone(&stop),
            command_tx,
        });
        let thread_control = Arc::clone(&control);
        let thread_cwd = cwd.to_path_buf();
        let thread_splash = splash_path.clone();
        let thread_mount = mount.clone();
        thread::spawn(move || {
            run_script_build(
                script_id,
                thread_mount,
                &thread_cwd,
                &thread_splash,
                studio_local_addr,
                studio_ext_addr,
                Arc::clone(&thread_control.stop),
                command_rx,
                event_tx,
            );
        });

        self.script_by_mount.insert(mount.clone(), script_id);
        self.scripts
            .insert(script_id, RunningScript { mount, control });
        Ok(script_id)
    }

    pub fn invoke_script_run_item(
        &self,
        mount: &str,
        name: &str,
        child_build_id: QueryId,
    ) -> Result<(), String> {
        let Some(script_id) = self.script_by_mount.get(mount).copied() else {
            return Err(format!(
                "{} is not running for mount {}",
                MAKEPAD_SPLASH_RUNNABLE, mount
            ));
        };
        let Some(script) = self.scripts.get(&script_id) else {
            return Err(format!(
                "script {} for mount {} is missing",
                script_id.0, mount
            ));
        };
        script
            .control
            .command_tx
            .send(ScriptCommand::RunItem {
                name: name.to_string(),
                child_build_id,
            })
            .map_err(|_| format!("failed to send run item {:?} to splash", name))
    }

    pub fn stop_script_for_mount(&mut self, mount: &str) -> Result<ScriptId, String> {
        let Some(script_id) = self.script_by_mount.get(mount).copied() else {
            return Err(format!(
                "{} is not running for mount {}",
                MAKEPAD_SPLASH_RUNNABLE, mount
            ));
        };
        self.stop_script(script_id)?;
        Ok(script_id)
    }

    pub fn stop_script(&mut self, script_id: ScriptId) -> Result<(), String> {
        let Some(script) = self.scripts.get(&script_id) else {
            return Err(format!("unknown script: {}", script_id.0));
        };
        script.control.stop.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub fn mark_exited(&mut self, script_id: ScriptId, exit_code: Option<i32>) -> Option<String> {
        let script = self.scripts.remove(&script_id)?;
        self.script_by_mount.remove(&script.mount);
        let _ = exit_code;
        Some(script.mount)
    }

    pub fn is_running_for_mount(&self, mount: &str) -> bool {
        self.script_by_mount.contains_key(mount)
    }

    fn alloc_script_id(&mut self) -> ScriptId {
        self.next_script_id = self.next_script_id.wrapping_add(1);
        if self.next_script_id == 0 {
            self.next_script_id = 1;
        }
        ScriptId(self.next_script_id)
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

fn script_value_to_query_id(
    vm: &mut ScriptVm,
    value: ScriptValue,
    what: &str,
) -> Result<Option<QueryId>, ScriptValue> {
    if value.is_nil() {
        return Ok(None);
    }
    if let Some(number) = value.as_number() {
        if number.is_finite() && number >= 0.0 && number.fract() == 0.0 {
            return Ok(Some(QueryId(number as u64)));
        }
    }
    let rendered = script_value_to_string(vm, value);
    Err(script_err_type_mismatch!(
        vm.trap(),
        "{} expects a build id number or nil, got {}",
        what,
        rendered
    ))
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

fn studio_url_for_app(base: Option<&str>, build_id: Option<QueryId>) -> String {
    let normalized = normalize_studio_host(base);
    if normalized.is_empty() {
        return String::new();
    }
    let Some(build_id) = build_id else {
        return normalized;
    };
    format!("{normalized}/app?build={}", build_id.0)
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
        id_lut!(log),
        script_args_def!(what = NIL),
        |vm, args| {
            let what = script_value!(vm, args.what);
            let line = script_value_to_string(vm, what);
            vm.host
                .downcast_mut::<ScriptHost>()
                .unwrap()
                .emit_output(line, false);
            NIL
        },
    );

    vm.add_method(
        std,
        id_lut!(print),
        script_args_def!(what = NIL),
        |vm, args| {
            let what = script_value!(vm, args.what);
            let line = script_value_to_string(vm, what);
            vm.host
                .downcast_mut::<ScriptHost>()
                .unwrap()
                .emit_output(line, false);
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
                .downcast_mut::<ScriptHost>()
                .unwrap()
                .emit_output(line, false);
            NIL
        },
    );
}

fn install_hub_script_module(vm: &mut ScriptVm) {
    let hub = vm.new_module(id!(hub));
    let studio_ip = vm
        .host
        .downcast_ref::<ScriptHost>()
        .and_then(|host| host.studio_local_addr.clone())
        .unwrap_or_default();
    let studio_ip = vm.new_string_with(|_vm, out| out.push_str(&studio_ip));
    vm.bx
        .heap
        .set_value_def(hub, id!(studio_ip).into(), studio_ip.into());

    vm.add_method(
        hub,
        id_lut!(studio_local),
        script_args_def!(build_id = NIL),
        |vm, args| {
            let build_id = script_value!(vm, args.build_id);
            let build_id = match script_value_to_query_id(vm, build_id, "hub.studio_local build_id")
            {
                Ok(Some(build_id)) => Some(build_id),
                Ok(None) => vm
                    .host
                    .downcast_ref::<ScriptHost>()
                    .and_then(|host| host.current_child_build_id),
                Err(err) => return err,
            };
            let url = vm
                .host
                .downcast_ref::<ScriptHost>()
                .map(|host| studio_url_for_app(host.studio_local_addr.as_deref(), build_id))
                .unwrap_or_default();
            vm.new_string_with(|_vm, out| out.push_str(&url)).into()
        },
    );

    vm.add_method(
        hub,
        id_lut!(studio_local_host),
        script_args_def!(),
        |vm, _args| {
            let host = vm
                .host
                .downcast_ref::<ScriptHost>()
                .map(|host| normalize_studio_host(host.studio_local_addr.as_deref()))
                .unwrap_or_default();
            vm.new_string_with(|_vm, out| out.push_str(&host)).into()
        },
    );

    vm.add_method(
        hub,
        id_lut!(studio_ext),
        script_args_def!(build_id = NIL),
        |vm, args| {
            let build_id = script_value!(vm, args.build_id);
            let build_id = match script_value_to_query_id(vm, build_id, "hub.studio_ext build_id") {
                Ok(Some(build_id)) => Some(build_id),
                Ok(None) => vm
                    .host
                    .downcast_ref::<ScriptHost>()
                    .and_then(|host| host.current_child_build_id),
                Err(err) => return err,
            };
            let url = vm
                .host
                .downcast_ref::<ScriptHost>()
                .map(|host| studio_url_for_app(host.studio_ext_addr.as_deref(), build_id))
                .unwrap_or_default();
            vm.new_string_with(|_vm, out| out.push_str(&url)).into()
        },
    );

    vm.add_method(
        hub,
        id_lut!(studio_ext_host),
        script_args_def!(),
        |vm, _args| {
            let host = vm
                .host
                .downcast_ref::<ScriptHost>()
                .map(|host| normalize_studio_host(host.studio_ext_addr.as_deref()))
                .unwrap_or_default();
            vm.new_string_with(|_vm, out| out.push_str(&host)).into()
        },
    );

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
                .downcast_mut::<ScriptHost>()
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

            let host = vm.host.downcast_mut::<ScriptHost>().unwrap();
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
    host: &mut ScriptHost,
    std: &mut ScriptStd,
    script_vm: &mut Option<Box<ScriptVmBase>>,
) {
    loop {
        let Ok(command) = host.command_rx.try_recv() else {
            break;
        };
        match command {
            ScriptCommand::RunItem {
                name,
                child_build_id,
            } => {
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
                host.current_child_build_id = Some(child_build_id);
                let build_id_arg = (child_build_id.0 as f64).into();
                let result = with_vm_and_async(host, std, script_vm, |vm| {
                    vm.call_with_self(on_run_object.into(), &[build_id_arg], item_object.into())
                });
                host.current_run_item_name = None;
                host.current_child_build_id = None;
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

fn has_pending_script_work(host: &ScriptHost, std: &ScriptStd) -> bool {
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
    script_id: ScriptId,
    mount: String,
    cwd: &Path,
    splash_path: &Path,
    studio_local_addr: Option<String>,
    studio_ext_addr: Option<String>,
    stop: Arc<AtomicBool>,
    command_rx: Receiver<ScriptCommand>,
    event_tx: Sender<HubEvent>,
) {
    let source = match fs::read_to_string(splash_path) {
        Ok(source) => source,
        Err(err) => {
            let host = ScriptHost {
                script_id,
                mount,
                cwd: cwd.to_path_buf(),
                studio_local_addr,
                studio_ext_addr,
                event_tx,
                stop,
                command_rx,
                run_items: HashMap::new(),
                current_run_item_name: None,
                current_child_build_id: None,
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

    let mut host = ScriptHost {
        script_id,
        mount,
        cwd: cwd.to_path_buf(),
        studio_local_addr,
        studio_ext_addr,
        event_tx,
        stop,
        command_rx,
        run_items: HashMap::new(),
        current_run_item_name: None,
        current_child_build_id: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_url_for_app_appends_app_path_to_base_addr() {
        assert_eq!(
            studio_url_for_app(Some("127.0.0.1:8001"), Some(QueryId(7))),
            "127.0.0.1:8001/app?build=7"
        );
    }

    #[test]
    fn studio_url_for_app_normalizes_existing_app_url() {
        assert_eq!(
            studio_url_for_app(Some("http://127.0.0.1:8001/app/5"), Some(QueryId(9))),
            "127.0.0.1:8001/app?build=9"
        );
    }
}

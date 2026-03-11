pub use makepad_network;
pub use makepad_script;

pub mod data;
pub mod fs;
pub mod net;
pub mod run;
pub mod task;
pub mod vm;

pub use data::*;
pub use net::*;
pub use run::*;
pub use task::*;
pub use vm::*;

use makepad_script::*;
use std::any::Any;

pub fn script_mod(vm: &mut ScriptVm) {
    crate::fs::script_mod(vm);
    crate::run::script_mod(vm);
    crate::task::script_mod(vm);
    crate::net::script_mod(vm);
}

pub fn pump<H: Any>(host: &mut H, std: &mut ScriptStd, script_vm: &mut Option<Box<ScriptVmBase>>) {
    crate::run::handle_script_child_processes(host, std, script_vm);
    crate::net::handle_script_socket_streams(host, std, script_vm);
    crate::net::handle_script_http_servers(host, std, script_vm);
    crate::task::handle_script_tasks(host, std, script_vm);
}

pub fn pump_network_runtime<H: Any>(
    host: &mut H,
    std: &mut ScriptStd,
    script_vm: &mut Option<Box<ScriptVmBase>>,
) -> Vec<makepad_network::NetworkResponse> {
    let responses = crate::net::drain_network_runtime(std);
    if !responses.is_empty() {
        crate::net::handle_script_network_events(host, std, script_vm, &responses);
        crate::task::handle_script_tasks(host, std, script_vm);
    }
    responses
}

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

pub fn script_mod(vm: &mut ScriptVm) {
    crate::fs::script_mod(vm);
    crate::run::script_mod(vm);
    crate::task::script_mod(vm);
    crate::net::script_mod(vm);
}

pub fn pump(host: &mut dyn ScriptHost) {
    crate::run::handle_script_child_processes(host);
    crate::net::handle_script_socket_streams(host);
    crate::net::handle_script_http_servers(host);
    crate::task::handle_script_tasks(host);
}

pub fn pump_network_runtime(host: &mut dyn ScriptHost) -> Vec<makepad_network::NetworkResponse> {
    let responses = crate::net::drain_network_runtime(
        host.script_std().downcast_mut::<ScriptStd>().unwrap(),
    );
    if !responses.is_empty() {
        crate::net::handle_script_network_events(host, &responses);
        crate::task::handle_script_tasks(host);
    }
    responses
}

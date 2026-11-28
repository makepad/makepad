use crate::*;
use makepad_script::*;

pub mod cx;
pub mod draw;
pub mod event;
pub mod reload;
pub mod res;
pub mod script;
pub mod std;
pub mod timer;
pub mod vm;
pub use self::std::{fs, net, run};
pub use makepad_script_std::HostGrants;

pub fn script_mod(vm: &mut ScriptVm) {
    register_platform(vm, true);
}

/// Platform registration for guest heaps. Their host owns the resource loader.
pub fn script_mod_isolate(vm: &mut ScriptVm) {
    register_platform(vm, false);
}

fn register_platform(vm: &mut ScriptVm, host: bool) {
    cx::register(vm, host);
    if host {
        makepad_script_std::script_mod(vm);
    } else {
        makepad_script_std::script_mod_isolate(vm);
    }
    crate::script::timer::script_mod(vm);
    if host {
        res::script_mod(vm);
    }
    crate::script::draw::script_mod(vm);
    crate::script::event::script_mod(vm);
}

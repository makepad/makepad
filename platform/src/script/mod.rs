use crate::*;
use makepad_script::*;

pub mod draw;
pub mod event;
pub mod fs;
pub mod helper;
pub mod cx;
pub mod net;
pub mod res;
pub mod run;
pub mod script;
pub mod std;
pub mod task;
pub mod timer;
pub mod vm;

pub fn script_mod(vm: &mut ScriptVm) {
    crate::script::cx::script_mod(vm);
    crate::script::net::script_mod(vm);
    crate::script::fs::script_mod(vm);
    crate::script::run::script_mod(vm);
    crate::script::timer::script_mod(vm);
    crate::script::task::script_mod(vm);
    crate::script::res::script_mod(vm);
    crate::script::draw::script_mod(vm);
    crate::script::event::script_mod(vm);
}

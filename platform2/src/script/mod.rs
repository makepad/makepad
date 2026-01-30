use crate::*;
use makepad_script::*;

pub mod net;
pub mod draw; 
pub mod vm;
pub mod fs;
pub mod run;
pub mod std;
pub mod script;
pub mod timer;
pub mod task;
pub mod res;

pub fn script_mod(vm:&mut ScriptVm){
    crate::script::net::script_mod(vm);
    crate::script::fs::script_mod(vm);
    crate::script::run::script_mod(vm);
    crate::script::timer::script_mod(vm);
    crate::script::task::script_mod(vm);
    crate::script::res::script_mod(vm);
    crate::script::draw::script_mod(vm);
}
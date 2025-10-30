use crate::*;
use makepad_script::*;

pub mod net;
pub mod vm;
pub mod fs;
pub mod script;

pub fn define_script_modules(vm:&mut ScriptVm){
    crate::script::net::define_net_module(vm);
    crate::script::fs::define_fs_module(vm);
}
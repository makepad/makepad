use crate::*;
use makepad_script::*;

pub mod net;
pub mod vm;
pub mod fs;

pub fn define_script_modules(vm:&mut ScriptVm){
    self::script::net::define_net_module(vm);
    self::script::fs::define_fs_module(vm);
}
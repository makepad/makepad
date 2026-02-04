use crate::*;
use makepad_script::*;

pub mod net;
pub mod vm;
pub mod fs;
pub mod run;
pub mod std;
pub mod script;
pub mod timer;
pub mod task;

pub fn define_script_modules(vm:&mut ScriptVm){
    crate::script::net::define_net_module(vm);
    crate::script::fs::define_fs_module(vm);
    crate::script::run::define_run_module(vm);
    crate::script::timer::extend_std_module_with_timer(vm);
    crate::script::task::extend_std_module_with_task(vm);
    crate::script::std::extend_std_module_with_println(vm);
}
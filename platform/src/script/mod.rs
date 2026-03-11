use crate::*;
use makepad_script::*;

pub mod draw;
pub mod event;
pub mod cx;
pub mod res;
pub mod script;
pub mod std;
pub mod timer;
pub mod vm;
pub use self::std::{fs, run};

pub fn script_mod(vm: &mut ScriptVm) {
    crate::script::cx::script_mod(vm);
    makepad_script_std::script_mod(vm);
    crate::script::timer::script_mod(vm);
    crate::script::res::script_mod(vm);
    crate::script::draw::script_mod(vm);
    crate::script::event::script_mod(vm);
}

use makepad_platform2::*;
mod fonts;

pub fn script_mod(vm:&mut ScriptVm){
    fonts::script_mod(vm);    
}

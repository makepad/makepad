use makepad_platform::*;
mod fonts;

pub fn script_mod(vm:&mut ScriptVm){
    fonts::script_mod(vm);    
}

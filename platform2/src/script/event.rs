use crate::event::KeyCode;
use crate::*;

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    let draw = vm.module(id!(draw));
    set_script_value_to_api!(vm, draw.KeyCode);

    NIL
}

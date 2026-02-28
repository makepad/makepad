use crate::cx::OsType;
use crate::script::vm::*;
use crate::*;
use makepad_script::*;

pub fn script_mod(vm: &mut ScriptVm) {
    let cx = vm.new_module(id_lut!(cx));

    set_script_value_to_api!(vm, cx.OsType);

    vm.add_method(cx, id_lut!(quit), script_args_def!(), |vm, _args| {
        vm.cx_mut().quit();
        NIL
    });

    vm.add_method(cx, id_lut!(os_type), script_args_def!(), |vm, _args| {
        let os_type = vm.cx().os_type().clone();
        os_type.script_to_value(vm)
    });
}

use crate::script::vm::*;
use crate::*;
use makepad_script::id;
use makepad_script::*;

pub fn script_mod(vm: &mut ScriptVm) {
    let helper = vm.new_module(id!(helper));

    vm.add_method(
        helper,
        id_lut!(startup),
        script_args_def!(value = NIL),
        move |vm, args| {
            let value = script_value!(vm, args.value);
            let cx = vm.host.cx_mut();
            cx.load_all_script_resources();
            let modules = vm.bx.heap.modules;
            vm.bx.heap.set_static(modules.into());
            value
        },
    );
}

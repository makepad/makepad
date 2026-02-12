use crate::*;
use makepad_script::*;
use makepad_script::id;

/// Extends the std module with a `println` function that properly prints objects.
/// This uses the debug string formatting to show object structure.
pub fn extend_std_module_with_println(vm: &mut ScriptVm) {
    let std = vm.module(id!(std));
    
    // Override/add println that properly formats and prints objects
    vm.add_method(std, id_lut!(println), script_args_def!(what = NIL), |vm, args| {
        let what = script_value!(vm, args.what);
        vm.heap().println(what);
        NIL
    });
}

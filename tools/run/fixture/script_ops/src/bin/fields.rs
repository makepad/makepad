//! A handle's fields from JIT-compiled code, with makepad-script bound to the loader's rustc
//! build: ScriptVmBase stays opaque (rustc's HashMap/Vec fields), but its plain fields are
//! reached at rustc's offsets (compute_program's driver sets `bail_on_uncaught_error` and
//! `allow_debug_output`, which was refused: "a field of ScriptVmBase is read from JIT-compiled code"),
//! and a handle field is a place the crate's ops take. rustc's ops read back what JIT code wrote.
//! Prints what rustc's build prints (provided_script.sh compares the two).
use makepad_script::vm_ctl::{ScriptCtlReq, STAT_KEEP_NUMBERS};
use makepad_script::*;

#[derive(Script, ScriptHook)]
struct Named {
    #[live(2.5)]
    a: f64,
}

fn main() {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let vm = &mut ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.bx.bail_on_uncaught_error = true;
    vm.bx.allow_debug_output = false;
    vm.bx.keep_numbers = true;
    let keep = vm.bx.vm_stat(&mut ScriptCtlReq::new(STAT_KEEP_NUMBERS));
    Named::script_proto(vm);
    // `vm.bx.heap`: a handle field as a place, the heap's ops on it
    let named = vm.bx.heap.type_name_by_id(Named::script_type_id_static()).is_some();
    println!("fields {} {} {} keep {} heap {}", vm.bx.bail_on_uncaught_error, vm.bx.allow_debug_output, vm.bx.keep_numbers, keep, named);
}

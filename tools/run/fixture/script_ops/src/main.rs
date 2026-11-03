//! makepad-script's pub API from JIT-compiled code, with the crate bound to the loader's rustc
//! build (`makepad run` binds it; a test harness binds nothing): an unmarked pub fn of the crate
//! that reads rustc's tables reaches them through the crate's ops. Prints what rustc's build
//! prints (provided_script.sh compares the two).
use makepad_script::*;

#[derive(Script, ScriptHook)]
struct Named {
    #[live(1.5)]
    a: f64,
}

fn main() {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let vm = &mut ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    Named::script_proto(vm);
    let n = Named::script_new(vm);
    // ScriptHeap::type_name_by_id: unmarked, its tables are rustc's HashMap/Vec (it was
    // refused: "a field of ScriptHeap is read from JIT-compiled code")
    let named = vm.heap().type_name_by_id(Named::script_type_id_static());
    let missing = vm.heap().type_name_by_id(ScriptTypeId::of_path("relax_provided_script_ops::Missing"));
    println!("type_name_by_id {} {} {} a {}", named.is_some(), named == Some(id!(Named)), missing.is_some(), n.a);
}

//! A panic in a `with_vm` closure with makepad-script-std bound to the loader's rustc build: the
//! VM entry ops are rustc's code, the closure and its trampoline JIT code. The trampoline boxes
//! the panic on the caller's side and `ScriptVmCall::resume_if_panicked` (unmarked, compiled
//! here) frees that box and resumes it; the JIT refused it ("calls `core::boxed::::from_raw` in
//! JIT-compiled code"), so every JIT app reaching with_vm failed to start. The VM is back on the
//! host after each panic and the next entry runs script. Prints what rustc's build prints
//! (provided_script.sh compares the two).
use makepad_script_std::makepad_script::{script, *};
use makepad_script_std::{try_with_vm, with_vm, with_vm_and_async_with, with_vm_with, ScriptStd, ScriptVmCall};
use std::panic::{catch_unwind, AssertUnwindSafe};

fn message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default()
}

fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    let mut host = ScriptVmHost::new((), ScriptStd::new());
    host.script_vm = Some(Box::new(ScriptVmBase::new()));
    let key = with_vm(&mut host.host_context(), |vm| vm.heap_key());

    // the op returns, the caller's side resumes the closure's own panic
    let mut boom = |_vm: &mut ScriptVm| panic!("closure panicked under with_vm");
    let mut call = ScriptVmCall::new(&mut boom);
    let returned = catch_unwind(AssertUnwindSafe(|| with_vm_with(&mut host.host_context(), false, &mut call)));
    let back = host.script_vm.is_some();
    let resumed = catch_unwind(AssertUnwindSafe(|| call.resume_if_panicked())).err().map(message);
    drop(call);

    // the async entry says the closure panicked; a second resume has nothing left
    let mut boom = |_vm: &mut ScriptVm| panic!("under with_vm_and_async");
    let mut call = ScriptVmCall::new(&mut boom);
    let async_returned = catch_unwind(AssertUnwindSafe(|| with_vm_and_async_with(&mut host.host_context(), &mut call)));
    let async_resumed = catch_unwind(AssertUnwindSafe(|| call.resume_if_panicked())).is_err();
    call.resume_if_panicked();
    drop(call);

    // the shells resume the panic, and the next entry still runs script on the same heap
    let shell = catch_unwind(AssertUnwindSafe(|| with_vm(&mut host.host_context(), |_vm| -> u32 { panic!("a native panicked") }))).err().map(message);
    let tried = catch_unwind(AssertUnwindSafe(|| try_with_vm(&mut host.host_context(), |_vm| -> u32 { panic!("under try_with_vm") }))).is_err();
    let value = with_vm(&mut host.host_context(), |vm| vm.eval(script! { 1 + 2 }).as_f64());
    let same = with_vm(&mut host.host_context(), |vm| vm.heap_key()) == key;
    println!(
        "vm_call returned {:?} back {} resumed {:?} async {:?} {} shell {:?} try {} value {:?} same heap {} vm {}",
        returned.ok(),
        back,
        resumed,
        async_returned.ok(),
        async_resumed,
        shell,
        tried,
        value,
        same,
        host.script_vm.is_some()
    );
}

//! A panic in a `with_vm` closure never unwinds out of the VM entry ops:
//! the closure's trampoline catches it on the caller's side, the op puts
//! the VM back on the host and returns, and the caller resumes the panic.
//! Under makepad run those ops are compiled code and the closure JIT code;
//! a JIT panic cannot unwind through a compiled frame, so a panic that had
//! to cross one left the VM behind (every later entry read as re-entrant).

use makepad_script_std::makepad_script::{script, *};
use makepad_script_std::{try_with_vm, with_vm, with_vm_and_async_with, with_vm_with, ScriptStd, ScriptVmCall};
use std::panic::{catch_unwind, AssertUnwindSafe};

fn host() -> ScriptVmHost<(), ScriptStd> {
    let mut host = ScriptVmHost::new((), ScriptStd::new());
    host.script_vm = Some(Box::new(ScriptVmBase::new()));
    host
}

fn heap_key(host: &mut ScriptVmHost<(), ScriptStd>) -> usize {
    with_vm(&mut host.host_context(), |vm| vm.heap_key())
}

fn message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default()
}

#[test]
fn a_closure_panic_returns_from_the_op_with_the_vm_back() {
    let mut host = host();
    let key = heap_key(&mut host);

    // The op itself returns: nothing unwinds through it.
    let mut ran = false;
    let mut boom = |_vm: &mut ScriptVm| {
        ran = true;
        panic!("closure panicked under with_vm")
    };
    let mut call = ScriptVmCall::new(&mut boom);
    let returned = catch_unwind(AssertUnwindSafe(|| with_vm_with(&mut host.host_context(), false, &mut call)));
    assert!(matches!(returned, Ok(true)), "the op returned instead of unwinding");
    assert!(host.script_vm.is_some(), "the VM is back on the host before the panic goes on");
    // The caller's side resumes the closure's own panic.
    let resumed = catch_unwind(AssertUnwindSafe(|| call.resume_if_panicked()));
    assert_eq!(message(resumed.expect_err("the panic goes on")), "closure panicked under with_vm");
    drop(call);
    assert!(ran);
    assert_eq!(heap_key(&mut host), key, "the same heap");

    // The async entry skips its script tasks and says the closure panicked.
    let mut boom = |_vm: &mut ScriptVm| panic!("under with_vm_and_async");
    let mut call = ScriptVmCall::new(&mut boom);
    let returned = catch_unwind(AssertUnwindSafe(|| with_vm_and_async_with(&mut host.host_context(), &mut call)));
    assert!(matches!(returned, Ok(true)));
    assert!(host.script_vm.is_some());
    assert!(catch_unwind(AssertUnwindSafe(|| call.resume_if_panicked())).is_err());
    // Resumed once: a second resume has nothing left.
    call.resume_if_panicked();
    drop(call);
    assert_eq!(heap_key(&mut host), key);
}

#[test]
fn the_shells_resume_the_panic_and_the_next_entry_works() {
    let mut host = host();
    let key = heap_key(&mut host);
    let caught = catch_unwind(AssertUnwindSafe(|| with_vm(&mut host.host_context(), |_vm| -> u32 { panic!("a native panicked") })));
    assert_eq!(message(caught.expect_err("with_vm resumes the panic")), "a native panicked");
    assert!(host.script_vm.is_some());
    let caught = catch_unwind(AssertUnwindSafe(|| try_with_vm(&mut host.host_context(), |_vm| -> u32 { panic!("under try_with_vm") })));
    assert!(caught.is_err());
    assert!(host.script_vm.is_some());
    // The VM still runs script, and a quiet closure's result comes back.
    let value = with_vm(&mut host.host_context(), |vm| vm.eval(script! { 1 + 2 }).as_f64());
    assert_eq!(value, Some(3.0));
    assert_eq!(heap_key(&mut host), key);
}

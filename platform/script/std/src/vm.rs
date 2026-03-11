use crate::{task, ScriptStd};
use makepad_script::*;
use std::any::Any;

pub trait ScriptVmStdExt {
    fn std_ref<T: Any>(&mut self) -> &T;
    fn std_mut<T: Any>(&mut self) -> &mut T;
}

impl<'a> ScriptVmStdExt for ScriptVm<'a> {
    fn std_ref<T: Any>(&mut self) -> &T {
        self.std.downcast_ref().unwrap()
    }

    fn std_mut<T: Any>(&mut self) -> &mut T {
        self.std.downcast_mut().unwrap()
    }
}

impl ScriptVmStdExt for &mut dyn Any {
    fn std_ref<T: Any>(&mut self) -> &T {
        self.downcast_ref().unwrap()
    }

    fn std_mut<T: Any>(&mut self) -> &mut T {
        self.downcast_mut().unwrap()
    }
}

pub fn with_vm_and_async<H: Any, F: FnOnce(&mut ScriptVm) -> R, R>(
    host: &mut H,
    std: &mut ScriptStd,
    script_vm: &mut Option<Box<ScriptVmBase>>,
    f: F,
) -> R {
    let mut bx = script_vm
        .take()
        .expect("Script VM swapped off, make sure to call with_vm if you want this to work");
    bx.threads.set_current_to_first_unpaused_thread();

    let (out, bx) = {
        let mut vm = ScriptVm { host, std, bx };
        let out = f(&mut vm);
        (out, vm.bx)
    };
    *script_vm = Some(bx);
    task::handle_script_tasks(host, std, script_vm);
    out
}

pub fn with_vm<H: Any, F: FnOnce(&mut ScriptVm) -> R, R>(
    host: &mut H,
    std: &mut ScriptStd,
    script_vm: &mut Option<Box<ScriptVmBase>>,
    f: F,
) -> R {
    let mut bx = script_vm
        .take()
        .expect("Script VM swapped off, make sure to call with_vm if you want this to work");
    bx.threads.set_current_to_first_unpaused_thread();

    let (out, bx) = {
        let mut vm = ScriptVm { host, std, bx };
        let out = f(&mut vm);
        vm.drain_errors();
        (out, vm.bx)
    };
    *script_vm = Some(bx);
    out
}

pub fn with_vm_thread<H: Any, F: FnOnce(&mut ScriptVm) -> R, R>(
    host: &mut H,
    std: &mut ScriptStd,
    script_vm: &mut Option<Box<ScriptVmBase>>,
    thread_id: ScriptThreadId,
    f: F,
) -> R {
    let mut bx = script_vm
        .take()
        .expect("Script VM swapped off, make sure to call with_vm if you want this to work");
    bx.threads.set_current_thread_id(thread_id);

    let (out, bx) = {
        let mut vm = ScriptVm { host, std, bx };
        let out = f(&mut vm);
        (out, vm.bx)
    };
    *script_vm = Some(bx);
    out
}

pub fn eval<H: Any>(
    host: &mut H,
    std: &mut ScriptStd,
    script_vm: &mut Option<Box<ScriptVmBase>>,
    script_mod: ScriptMod,
) -> ScriptValue {
    with_vm_and_async(host, std, script_vm, |vm| vm.eval(script_mod))
}

use crate::{task, ScriptStd};
use makepad_script::*;
use std::any::Any;
use std::cell::Cell;
use std::panic::Location;

thread_local! {
    /// Source location of the `with_vm`/`eval` entry that currently holds the
    /// script VM (`None` when the VM is available). Set when the VM is taken and
    /// restored when it's released, so a re-entrant entry can name the holder.
    /// Only read on the panic path, so the bookkeeping is off the hot path's
    /// critical observation.
    static VM_HELD_AT: Cell<Option<&'static Location<'static>>> = const { Cell::new(None) };
}

/// RAII guard, created by the `Cx::with_vm` family right before they take the
/// VM, that (a) turns a re-entrant entry into an actionable panic naming both
/// the current holder and the re-entrant caller (readable even when the native
/// backtrace is unsymbolicated, e.g. iOS release builds), and (b) records this
/// entry as the new holder for the duration of the call.
///
/// The canonical re-entrancy bug is holding a raw `vm.cx_mut()` borrow (which
/// leaves `Cx::script_vm` swapped off) and then reaching `cx.with_vm`; the fix
/// is `vm.with_cx_mut(|cx| ...)`, which parks the VM back onto `Cx` first.
pub struct VmHolderGuard {
    prev: Option<&'static Location<'static>>,
}

impl VmHolderGuard {
    /// `vm_available` is `Cx::script_vm.is_some()` evaluated at the call site.
    /// When it's `false` the VM is already held (re-entrancy) and we panic with
    /// a diagnostic instead of letting the later `take().expect(...)` abort with
    /// the opaque "swapped off" message.
    #[track_caller]
    pub fn enter(vm_available: bool, caller: &'static Location<'static>) -> VmHolderGuard {
        if !vm_available {
            let held = VM_HELD_AT.with(|c| c.get());
            panic!(
                "re-entrant script VM access at {caller}: the VM is already held by \
                 {} (it's `take()`n for the whole duration of a `with_vm`/`eval` closure). \
                 You're most likely inside a raw `vm.cx_mut()` borrow or another `with_vm`; \
                 use `vm.with_cx_mut(|cx| ...)` so the VM is parked back onto `Cx` first.",
                held.map(|l| l.to_string()).unwrap_or_else(|| "<unknown>".to_string()),
            );
        }
        let prev = VM_HELD_AT.with(|c| c.replace(Some(caller)));
        VmHolderGuard { prev }
    }
}

impl Drop for VmHolderGuard {
    fn drop(&mut self) {
        VM_HELD_AT.with(|c| c.set(self.prev));
    }
}

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
        .expect(
            "re-entrant script VM access: the VM is already `take()`n (swapped off) by an \
             enclosing `with_vm`/`eval`. You're most likely inside a raw `vm.cx_mut()` borrow; \
             use `vm.with_cx_mut(|cx| ...)` so the VM is parked back onto `Cx` first.",
        );
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
        .expect(
            "re-entrant script VM access: the VM is already `take()`n (swapped off) by an \
             enclosing `with_vm`/`eval`. You're most likely inside a raw `vm.cx_mut()` borrow; \
             use `vm.with_cx_mut(|cx| ...)` so the VM is parked back onto `Cx` first.",
        );
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

/// Like [`with_vm`], but returns `None` instead of panicking when the VM is
/// already held (swapped off) by an enclosing `with_vm`/`eval`. Use this for
/// call sites that can correctly degrade — e.g. defer to a later frame — when
/// invoked re-entrantly, rather than aborting.
pub fn try_with_vm<H: Any, F: FnOnce(&mut ScriptVm) -> R, R>(
    host: &mut H,
    std: &mut ScriptStd,
    script_vm: &mut Option<Box<ScriptVmBase>>,
    f: F,
) -> Option<R> {
    let mut bx = script_vm.take()?;
    bx.threads.set_current_to_first_unpaused_thread();

    let (out, bx) = {
        let mut vm = ScriptVm { host, std, bx };
        let out = f(&mut vm);
        vm.drain_errors();
        (out, vm.bx)
    };
    *script_vm = Some(bx);
    Some(out)
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
        .expect(
            "re-entrant script VM access: the VM is already `take()`n (swapped off) by an \
             enclosing `with_vm`/`eval`. You're most likely inside a raw `vm.cx_mut()` borrow; \
             use `vm.with_cx_mut(|cx| ...)` so the VM is parked back onto `Cx` first.",
        );
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

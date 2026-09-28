use crate::task;
use makepad_script::*;
use std::any::Any;
use std::cell::Cell;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe, Location};

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
        self.host.script_std().downcast_ref().unwrap()
    }

    fn std_mut<T: Any>(&mut self) -> &mut T {
        self.host.script_std().downcast_mut().unwrap()
    }
}

const TAKEN_MSG: &str = "re-entrant script VM access: the VM is already `take()`n (swapped off) by an \
             enclosing `with_vm`/`eval`. You're most likely inside a raw `vm.cx_mut()` borrow; \
             use `vm.with_cx_mut(|cx| ...)` so the VM is parked back onto `Cx` first.";

/// Run `f` on `bx` as the host's VM and put `bx` back on the host whether
/// `f` returns or unwinds.
///
/// A drop guard cannot do this: the `ScriptVm` owns the host borrow AND
/// the base for the whole call, so nothing else can hold either to restore
/// them. The unwind is caught instead, the VM parked, and the panic resumed
/// as it was (`resume_unwind` runs no hook a second time). A catcher higher
/// up — a platform's event catcher, a host isolating a guest — then finds
/// the host exactly as it was before the call, where letting the frame go
/// dropped the whole heap and left every later entry re-entrant.
///
/// The slot is never overwritten on the way out of a panic: `with_cx_mut`
/// parks the real VM on the host itself while its closure runs, and the
/// `ScriptVm` then holds a placeholder — if that closure is where the
/// unwind began and the placeholder came back here, the slot already has
/// the VM and the placeholder is dropped.
///
/// Not generic: the `with_vm` family's shells below move their closure and
/// its result through `Option` slots and share these bodies.
#[inline(never)]
fn run_parked(host: &mut dyn ScriptHost, bx: Box<ScriptVmBase>, f: &mut dyn FnMut(&mut ScriptVm)) {
    let mut vm = ScriptVm { host, bx };
    let out = catch_unwind(AssertUnwindSafe(|| f(&mut vm)));
    let ScriptVm { host, bx } = vm;
    match out {
        Ok(()) => {
            *host.script_vm_slot() = Some(bx);
        }
        Err(payload) => {
            let slot = host.script_vm_slot();
            if slot.is_none() {
                *slot = Some(bx);
            }
            resume_unwind(payload)
        }
    }
}

#[inline(never)]
pub fn with_vm_and_async_dyn(host: &mut dyn ScriptHost, f: &mut dyn FnMut(&mut ScriptVm)) {
    let mut bx = host.script_vm_slot().take().expect(TAKEN_MSG);
    bx.threads.set_current_to_first_unpaused_thread();
    run_parked(host, bx, f);
    task::handle_script_tasks(host);
}

/// `false` when the VM is already held (only when `try_` is set; otherwise
/// that panics with `TAKEN_MSG`).
#[inline(never)]
pub fn with_vm_dyn(host: &mut dyn ScriptHost, try_: bool, f: &mut dyn FnMut(&mut ScriptVm)) -> bool {
    let mut bx = match host.script_vm_slot().take() {
        Some(bx) => bx,
        None if try_ => return false,
        None => panic!("{}", TAKEN_MSG),
    };
    bx.threads.set_current_to_first_unpaused_thread();
    run_parked(host, bx, &mut |vm| {
        f(vm);
        vm.drain_errors();
    });
    true
}

#[inline(never)]
pub fn with_vm_thread_dyn(
    host: &mut dyn ScriptHost,
    thread_id: ScriptThreadId,
    f: &mut dyn FnMut(&mut ScriptVm),
) {
    let mut bx = host.script_vm_slot().take().expect(TAKEN_MSG);
    bx.threads.set_current_thread_id(thread_id);
    run_parked(host, bx, f)
}

pub fn with_vm_and_async<F: FnOnce(&mut ScriptVm) -> R, R>(
    host: &mut dyn ScriptHost,
    f: F,
) -> R {
    let mut f = Some(f);
    let mut out = None;
    with_vm_and_async_dyn(host, &mut |vm| out = Some((f.take().unwrap())(vm)));
    out.unwrap()
}

pub fn with_vm<F: FnOnce(&mut ScriptVm) -> R, R>(host: &mut dyn ScriptHost, f: F) -> R {
    let mut f = Some(f);
    let mut out = None;
    with_vm_dyn(host, false, &mut |vm| out = Some((f.take().unwrap())(vm)));
    out.unwrap()
}

/// Like [`with_vm`], but returns `None` instead of panicking when the VM is
/// already held (swapped off) by an enclosing `with_vm`/`eval`. Use this for
/// call sites that can correctly degrade — e.g. defer to a later frame — when
/// invoked re-entrantly, rather than aborting.
pub fn try_with_vm<F: FnOnce(&mut ScriptVm) -> R, R>(
    host: &mut dyn ScriptHost,
    f: F,
) -> Option<R> {
    let mut f = Some(f);
    let mut out = None;
    if !with_vm_dyn(host, true, &mut |vm| out = Some((f.take().unwrap())(vm))) {
        return None;
    }
    out
}

pub fn with_vm_thread<F: FnOnce(&mut ScriptVm) -> R, R>(
    host: &mut dyn ScriptHost,
    thread_id: ScriptThreadId,
    f: F,
) -> R {
    let mut f = Some(f);
    let mut out = None;
    with_vm_thread_dyn(host, thread_id, &mut |vm| out = Some((f.take().unwrap())(vm)));
    out.unwrap()
}

pub fn eval(host: &mut dyn ScriptHost, script_mod: ScriptMod) -> ScriptValue {
    with_vm_and_async(host, |vm| vm.eval(script_mod))
}

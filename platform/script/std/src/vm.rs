use makepad_relax_api::relax_api;
use crate::task;
use makepad_script::*;
use std::cell::Cell;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe, Location};

thread_local! {
    /// Source location of the `with_vm`/`eval` entry that currently holds the
    /// script VM (line zero when the VM is available). Set when the VM is taken and
    /// restored when it's released, so a re-entrant entry can name the holder.
    /// Only read on the panic path, so the bookkeeping is off the hot path's
    /// critical observation.
    static VM_HELD_AT: Cell<VmCaller> = const { Cell::new(VmCaller { file: "", line: 0, column: 0 }) };
}

#[derive(Clone, Copy)]
#[repr(C)]
struct VmCaller {
    file: &'static str,
    line: u32,
    column: u32,
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
#[relax_api(plain)]
#[repr(C)]
pub struct VmHolderGuard {
    prev: VmCaller,
}

#[relax_api]
impl VmHolderGuard {
    /// `vm_available` is `Cx::script_vm.is_some()` evaluated at the call site.
    /// When it's `false` the VM is already held (re-entrancy) and we panic with
    /// a diagnostic instead of letting the later `take().expect(...)` abort with
    /// the opaque "swapped off" message.
    #[track_caller]
    #[inline(always)]
    pub fn enter(vm_available: bool, caller: &'static Location<'static>) -> VmHolderGuard {
        Self::enter_at(vm_available, caller.file(), caller.line(), caller.column())
    }

    /// C-shaped entry: Location and thread-local storage stay on their owning
    /// side of a provided-crate boundary. The source filename is static, so
    /// recording an entry needs no allocation or caller-owned Rust value.
    #[relax_api]
    pub fn enter_at(
        vm_available: bool,
        file: &'static str,
        line: u32,
        column: u32,
    ) -> VmHolderGuard {
        if !vm_available {
            let held = VM_HELD_AT.with(|c| c.get());
            panic!(
                "re-entrant script VM access at {file}:{line}:{column}: the VM is already held by \
                 {} (it's `take()`n for the whole duration of a `with_vm`/`eval` closure). \
                 You're most likely inside a raw `vm.cx_mut()` borrow or another `with_vm`; \
                 use `vm.with_cx_mut(|cx| ...)` so the VM is parked back onto `Cx` first.",
                if held.line == 0 { "<unknown>".to_string() } else {
                    format!("{}:{}:{}", held.file, held.line, held.column)
                },
            );
        }
        let prev = VM_HELD_AT.with(|c| c.replace(VmCaller { file, line, column }));
        VmHolderGuard { prev }
    }

    /// Restore the previous holder inside the crate that owns the TLS slot.
    #[relax_api]
    pub(crate) fn restore(&mut self) {
        VM_HELD_AT.with(|c| c.set(self.prev));
    }
}

impl Drop for VmHolderGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

pub trait ScriptVmStdExt {
    fn std_ref<T: ScriptHostCapability>(&mut self) -> &T;
    fn std_mut<T: ScriptHostCapability>(&mut self) -> &mut T;
}

impl<'a> ScriptVmStdExt for ScriptVm<'a> {
    fn std_ref<T: ScriptHostCapability>(&mut self) -> &T {
        self.host.std_ref().unwrap()
    }

    fn std_mut<T: ScriptHostCapability>(&mut self) -> &mut T {
        self.host.std_mut().unwrap()
    }
}

const TAKEN_MSG: &str = "re-entrant script VM access: the VM is already `take()`n (swapped off) by an \
             enclosing `with_vm`/`eval`. You're most likely inside a raw `vm.cx_mut()` borrow; \
             use `vm.with_cx_mut(|cx| ...)` so the VM is parked back onto `Cx` first.";

/// Borrowed caller-owned closure; its trampoline stays with the caller.
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptVmCall<'a> {
    context: *mut u8,
    call: unsafe extern "C-unwind" fn(*mut u8, &mut ScriptVm),
    borrow: std::marker::PhantomData<&'a mut u8>,
}
impl<'a> ScriptVmCall<'a> {
    #[inline]
    pub fn new<F: FnMut(&mut ScriptVm)>(f: &'a mut F) -> Self {
        unsafe extern "C-unwind" fn call<F: FnMut(&mut ScriptVm)>(p: *mut u8, vm: &mut ScriptVm) {
            (&mut *p.cast::<F>())(vm);
        }
        Self {
            context: f as *mut F as *mut u8,
            call: call::<F>,
            borrow: std::marker::PhantomData,
        }
    }
    #[inline]
    fn invoke(&mut self, vm: &mut ScriptVm) {
        unsafe { (self.call)(self.context, vm) }
    }
}

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
#[inline]
fn run_parked(
    host: &mut ScriptHostContext,
    bx: Box<ScriptVmBase>,
    f: &mut ScriptVmCall,
    drain: bool,
) {
    let mut vm = ScriptVm {
        host: host.reborrow(),
        bx,
    };
    let out = catch_unwind(AssertUnwindSafe(|| {
        f.invoke(&mut vm);
        if drain {
            vm.drain_errors();
        }
    }));
    let ScriptVm { mut host, bx } = vm;
    match out {
        Ok(()) => {
            host.restore_vm(bx);
        }
        Err(payload) => {
            if host.vm_base().is_none() {
                host.restore_vm(bx);
            }
            resume_unwind(payload)
        }
    }
}

#[inline]
#[relax_api]
pub fn with_vm_and_async_with(host: &mut ScriptHostContext, f: &mut ScriptVmCall) {
    let mut bx = host.take_vm().expect(TAKEN_MSG);
    bx.select_first_unpaused_thread();
    run_parked(host, bx, f, false);
    task::handle_script_tasks(host);
}

/// False only when try_ is set and the VM is already held.
#[inline]
#[relax_api]
pub fn with_vm_with(host: &mut ScriptHostContext, try_: bool, f: &mut ScriptVmCall) -> bool {
    let mut bx = match host.take_vm() {
        Some(bx) => bx,
        None if try_ => return false,
        None => panic!("{}", TAKEN_MSG),
    };
    bx.select_first_unpaused_thread();
    run_parked(host, bx, f, true);
    true
}

#[inline]
#[relax_api]
pub fn with_vm_thread_with(
    host: &mut ScriptHostContext,
    thread_id: ScriptThreadId,
    f: &mut ScriptVmCall,
) {
    let mut bx = host.take_vm().expect(TAKEN_MSG);
    bx.select_thread(thread_id);
    run_parked(host, bx, f, false)
}

#[inline]
pub fn with_vm_and_async<F: FnOnce(&mut ScriptVm) -> R, R>(
    host: &mut ScriptHostContext,
    f: F,
) -> R {
    let mut f = Some(f);
    let mut out = None;
    with_vm_and_async_with(
        host,
        &mut ScriptVmCall::new(&mut |vm: &mut ScriptVm| out = Some((f.take().unwrap())(vm))),
    );
    out.unwrap()
}
#[inline]
pub fn with_vm<F: FnOnce(&mut ScriptVm) -> R, R>(host: &mut ScriptHostContext, f: F) -> R {
    try_with_vm_inner(host, false, f).unwrap()
}
#[inline]
fn try_with_vm_inner<F: FnOnce(&mut ScriptVm) -> R, R>(
    host: &mut ScriptHostContext,
    try_: bool,
    f: F,
) -> Option<R> {
    let mut f = Some(f);
    let mut out = None;
    with_vm_with(
        host,
        try_,
        &mut ScriptVmCall::new(&mut |vm: &mut ScriptVm| out = Some((f.take().unwrap())(vm))),
    );
    out
}
#[inline]
pub fn try_with_vm<F: FnOnce(&mut ScriptVm) -> R, R>(
    host: &mut ScriptHostContext,
    f: F,
) -> Option<R> {
    try_with_vm_inner(host, true, f)
}
#[inline]
pub fn with_vm_thread<F: FnOnce(&mut ScriptVm) -> R, R>(
    host: &mut ScriptHostContext,
    thread_id: ScriptThreadId,
    f: F,
) -> R {
    let mut f = Some(f);
    let mut out = None;
    with_vm_thread_with(
        host,
        thread_id,
        &mut ScriptVmCall::new(&mut |vm: &mut ScriptVm| out = Some((f.take().unwrap())(vm))),
    );
    out.unwrap()
}
#[relax_api]
pub fn eval(host: &mut ScriptHostContext, script_mod: &ScriptModView) -> ScriptValue {
    with_vm_and_async(host, |vm| {
        vm.eval_with_source_view(script_mod, ScriptObject::ZERO)
    })
}

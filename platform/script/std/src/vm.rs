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
///
/// A panic in the closure never unwinds out of `call`: the trampoline
/// catches it on the caller's side and hands it back as a pointer, which
/// [`ScriptVmCall::resume_if_panicked`] resumes there once the VM is back
/// on the host. Under makepad run the ops taking a `ScriptVmCall` are this
/// provided crate's compiled code while the closure is JIT code: a JIT
/// panic cannot unwind through a compiled frame, it leaves to the JIT's
/// nearest catch above it, past `run_parked`, and the VM was lost with the
/// frame that held it (every later entry then read as re-entrant).
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptVmCall<'a> {
    context: *mut u8,
    /// Runs the closure; null when it returned, else its panic (a
    /// `Box<Box<dyn Any + Send>>` of the caller's side).
    call: unsafe extern "C-unwind" fn(*mut u8, &mut ScriptVm) -> *mut u8,
    /// The panic the last `invoke` caught, until `resume_if_panicked`.
    panic: *mut u8,
    borrow: std::marker::PhantomData<&'a mut u8>,
}
impl<'a> ScriptVmCall<'a> {
    #[inline]
    pub fn new<F: FnMut(&mut ScriptVm)>(f: &'a mut F) -> Self {
        unsafe extern "C-unwind" fn call<F: FnMut(&mut ScriptVm)>(p: *mut u8, vm: &mut ScriptVm) -> *mut u8 {
            match catch_unwind(AssertUnwindSafe(|| (&mut *p.cast::<F>())(vm))) {
                Ok(()) => std::ptr::null_mut(),
                Err(payload) => Box::into_raw(Box::new(payload)).cast(),
            }
        }
        Self {
            context: f as *mut F as *mut u8,
            call: call::<F>,
            panic: std::ptr::null_mut(),
            borrow: std::marker::PhantomData,
        }
    }
    /// Runs the closure; true when it panicked (the panic waits in `self`).
    #[inline]
    fn invoke(&mut self, vm: &mut ScriptVm) -> bool {
        self.panic = unsafe { (self.call)(self.context, vm) };
        !self.panic.is_null()
    }
    /// Continues the closure's panic, if it had one, on the caller's side
    /// (`resume_unwind`: the panic hook does not run a second time).
    #[inline]
    pub fn resume_if_panicked(&mut self) {
        if !self.panic.is_null() {
            let payload = unsafe { Box::from_raw(self.panic.cast::<Box<dyn std::any::Any + Send>>()) };
            self.panic = std::ptr::null_mut();
            resume_unwind(*payload)
        }
    }
}

/// Run `f` on `bx` as the host's VM and put `bx` back on the host whether
/// `f` returns or panics; true when `f` panicked (its panic waits in `f`
/// for the caller, [`ScriptVmCall::resume_if_panicked`]).
///
/// `f` catches its own panic (see [`ScriptVmCall`]). What can still unwind
/// here is this crate's own code (`drain_errors`): that unwind is caught,
/// the VM parked, and the panic resumed as it was. A drop guard cannot do
/// this: the `ScriptVm` owns the host borrow AND the base for the whole
/// call, so nothing else can hold either to restore them.
///
/// The slot is never overwritten on the way out of a panic: `with_cx_mut`
/// parks the real VM on the host itself while its closure runs, and the
/// `ScriptVm` then holds a placeholder — if that closure is where the
/// panic began and the placeholder came back here, the slot already has
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
) -> bool {
    let mut vm = ScriptVm {
        host: host.reborrow(),
        bx,
    };
    let out = catch_unwind(AssertUnwindSafe(|| {
        let panicked = f.invoke(&mut vm);
        if drain && !panicked {
            vm.drain_errors();
        }
        panicked
    }));
    let ScriptVm { mut host, bx } = vm;
    if matches!(out, Ok(false)) || host.vm_base().is_none() {
        host.restore_vm(bx);
    }
    match out {
        Ok(panicked) => panicked,
        Err(payload) => resume_unwind(payload),
    }
}

/// True when `f` panicked: its panic waits in `f`, and the script tasks
/// were not run.
#[inline]
#[relax_api]
pub fn with_vm_and_async_with(host: &mut ScriptHostContext, f: &mut ScriptVmCall) -> bool {
    let mut bx = host.take_vm().expect(TAKEN_MSG);
    bx.select_first_unpaused_thread();
    if run_parked(host, bx, f, false) {
        return true;
    }
    task::handle_script_tasks(host);
    false
}

/// False only when try_ is set and the VM is already held. A panic of `f`
/// waits in `f` (`resume_if_panicked`).
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

/// A panic of `f` waits in `f` (`resume_if_panicked`).
#[inline]
#[relax_api]
pub fn with_vm_thread_with(
    host: &mut ScriptHostContext,
    thread_id: ScriptThreadId,
    f: &mut ScriptVmCall,
) {
    let mut bx = host.take_vm().expect(TAKEN_MSG);
    bx.select_thread(thread_id);
    run_parked(host, bx, f, false);
}

#[inline]
pub fn with_vm_and_async<F: FnOnce(&mut ScriptVm) -> R, R>(
    host: &mut ScriptHostContext,
    f: F,
) -> R {
    let mut f = Some(f);
    let mut out = None;
    let mut call = |vm: &mut ScriptVm| out = Some((f.take().unwrap())(vm));
    let mut call = ScriptVmCall::new(&mut call);
    with_vm_and_async_with(host, &mut call);
    call.resume_if_panicked();
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
    let mut call = |vm: &mut ScriptVm| out = Some((f.take().unwrap())(vm));
    let mut call = ScriptVmCall::new(&mut call);
    with_vm_with(host, try_, &mut call);
    call.resume_if_panicked();
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
    let mut call = |vm: &mut ScriptVm| out = Some((f.take().unwrap())(vm));
    let mut call = ScriptVmCall::new(&mut call);
    with_vm_thread_with(host, thread_id, &mut call);
    call.resume_if_panicked();
    out.unwrap()
}
#[relax_api]
pub fn eval(host: &mut ScriptHostContext, script_mod: &ScriptModView) -> ScriptValue {
    with_vm_and_async(host, |vm| {
        vm.eval_with_source_view(script_mod, ScriptObject::ZERO)
    })
}

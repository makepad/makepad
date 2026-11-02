//! Relax runtime hooks used by std (declared in core::intrinsics_rt). The rustc shim
//! (check_os, built with --cfg relax_shim) uses check_os/src/rt_shim.rs instead.

#[cfg(relax_shim)]
#[path = "../../check_os/src/rt_shim.rs"]
mod imp;

#[cfg(not(relax_shim))]
mod imp {
use core::ffi::c_char;

/// Runs `f(data)`; true if it panicked (live mode: the panic was reported and the stack
/// reset to this frame; AOT: panics abort, so this returns false or never returns).
pub fn catch_panic(f: fn(*mut u8), data: *mut u8) -> bool {
    unsafe { core::intrinsics_rt::catch_panic(f, data) }
}

/// argc/argv as the process received them (the runtime records them at boot).
pub fn args() -> (usize, *const *const c_char) {
    unsafe { core::intrinsics_rt::args() }
}

/// True while the panic hook or unwinding drops run, until catch_unwind or thread start.
pub fn panicking() -> bool {
    #[cfg(not(relax_web))]
    if crate::panic::panic_depth_step(0) != 0 {
        return true;
    }
    unsafe { core::intrinsics_rt::panicking() }
}

/// The next panic continues one already reported (resume_unwind).
pub fn rethrow() {
    unsafe { core::intrinsics_rt::rethrow() }
}

/// The std hook trampoline (std::panic), called by `core_hook` below.
static mut STD_HOOK: Option<fn(&str, &core::panic::Location<'_>)> = None;

/// What core::panicking calls before the runtime's own report/abort: records the payload
/// catch_unwind / JoinHandle::join hand out, then runs std's hook if one was set.
fn core_hook(info: &core::panic::PanicInfo<'_>) {
    report_panic(info.message(), info.location());
}

fn report_panic(message: core::fmt::Arguments<'_>, location: Option<&core::panic::Location<'_>>) {
    #[cfg(not(relax_web))]
    {
    crate::panic::record_payload(message);
    if crate::panic::take_resuming() {
        return;
    }
    }
    let hook = unsafe { STD_HOOK };
    if let (Some(h), Some(loc)) = (hook, location) {
        let msg = alloc::fmt::format(message);
        h(&msg, loc);
    }
}

#[cfg(not(relax_web))]
type RuntimePanicHook = extern "C" fn(*const u8, usize, *const u8, usize, u32, u32, bool);

#[cfg(not(relax_web))]
extern "C" {
    fn relax_set_panic_hook(hook: RuntimePanicHook);
}

/// Static message bytes are interned by the loader; formatted bytes live through this call.
#[cfg(not(relax_web))]
extern "C" fn runtime_panic_hook(msg: *const u8, len: usize, file: *const u8, file_len: usize, line: u32, col: u32, is_static: bool) {
    let depth = crate::panic::panic_depth_step(1);
    if depth > 2 {
        unsafe { core::intrinsics_rt::abort() };
    }
    if depth == 1 {
        let file = unsafe { core::slice::from_raw_parts(file, file_len) };
        let loc = core::panic::Location::internal_constructor(file, line, col);
        if is_static {
            let msg: &'static str = unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(msg, len)) };
            let pieces = [msg];
            report_panic(core::fmt::Arguments::new_const(&pieces), Some(&loc));
        } else {
            let msg = unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(msg, len)) };
            report_panic(format_args!("{}", msg), Some(&loc));
        }
    }
    crate::panic::panic_depth_step(-1);
}

/// Installs std::panic's trampoline (called with the formatted message and location).
pub fn set_panic_trampoline(f: fn(&str, &core::panic::Location<'_>)) {
    unsafe {
        STD_HOOK = Some(f);
    }
    #[cfg(not(relax_web))]
    unsafe { relax_set_panic_hook(runtime_panic_hook) };
    core::panicking::set_depth_fn(crate::panic::panic_depth_step);
    core::panicking::set_hook_fn(Some(core_hook));
}

/// Makes core::panicking call `core_hook` (payload recording) even without a std hook, and
/// count panic nesting per thread.
pub fn install_core_hook() {
    #[cfg(not(relax_web))]
    unsafe { relax_set_panic_hook(runtime_panic_hook) };
    core::panicking::set_depth_fn(crate::panic::panic_depth_step);
    if core::panicking::hook_fn().is_none() {
        core::panicking::set_hook_fn(Some(core_hook));
    }
}
}

pub use self::imp::{args, catch_panic, install_core_hook, panicking, rethrow, set_panic_trampoline};

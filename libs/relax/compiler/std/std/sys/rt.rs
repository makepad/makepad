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

/// Whether this thread is unwinding a panic: true while the frames' drops run on the way to
/// the catch_unwind (or thread start) that catches it (MutexGuard poisons the lock then).
pub fn panicking() -> bool {
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
    crate::panic::record_payload(info.message());
    if crate::panic::take_resuming() {
        return;
    }
    let hook = unsafe { STD_HOOK };
    if let (Some(h), Some(loc)) = (hook, info.location()) {
        let msg = alloc::fmt::format(info.message());
        h(&msg, loc);
    }
}

/// Installs std::panic's trampoline (called with the formatted message and location).
pub fn set_panic_trampoline(f: fn(&str, &core::panic::Location<'_>)) {
    unsafe {
        STD_HOOK = Some(f);
    }
    core::panicking::set_depth_fn(crate::panic::panic_depth_step);
    core::panicking::set_hook_fn(Some(core_hook));
}

/// Makes core::panicking call `core_hook` (payload recording) even without a std hook, and
/// count panic nesting per thread.
pub fn install_core_hook() {
    core::panicking::set_depth_fn(crate::panic::panic_depth_step);
    if core::panicking::hook_fn().is_none() {
        core::panicking::set_hook_fn(Some(core_hook));
    }
}
}

pub use self::imp::{args, catch_panic, install_core_hook, panicking, rethrow, set_panic_trampoline};

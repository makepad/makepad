//! HotRust runtime hooks used by std (declared in core::intrinsics_rt). The rustc shim
//! (check_os, built with --cfg hotrust_shim) uses check_os/src/rt_shim.rs instead.

#[cfg(hotrust_shim)]
#[path = "../../check_os/src/rt_shim.rs"]
mod imp;

#[cfg(not(hotrust_shim))]
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

/// Whether this thread is unwinding a panic. HotRust never unwinds through frames (live mode
/// resets the stack to the recovery point), so no Drop ever observes a panic in progress.
pub fn panicking() -> bool {
    false
}

/// The std hook trampoline (std::panic), called by `core_hook` below.
static mut STD_HOOK: Option<fn(&str, &core::panic::Location<'_>)> = None;

/// What core::panicking calls before the runtime's own report/abort.
fn core_hook(info: &core::panic::PanicInfo<'_>) {
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
    core::panicking::set_hook_fn(Some(core_hook));
}
}

pub use self::imp::{args, catch_panic, panicking, set_panic_trampoline};

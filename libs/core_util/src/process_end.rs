//! How a process ends, natively and under the JIT alike: the exit work its own code registered
//! ([`at_exit`]), then the flushes of Rust's and C's standard streams, then the end of the process
//! without the C library's exit handlers ([`end`]).
//!
//! Those handlers include the ones every native library registered as it loaded, and a GPU
//! driver's teardown at exit can wait for good: NVIDIA's Vulkan driver joined one of its threads
//! that never ended, about 1 in 150 test runs, and the process never ended. The kernel releases
//! what the process held as its files close, as for a killed process. The main thread's pthread
//! key destructors (thread locals) do not run either, as at any exit.

/// The exit work registered with [`at_exit`], in the order it was registered: [`end`] runs it.
#[cfg(all(unix, not(relax)))]
static AT_EXIT: std::sync::Mutex<Vec<extern "C" fn()>> = std::sync::Mutex::new(Vec::new());

/// Registers `f` as the process's exit work. [`end`] runs it, last registered first; it also goes to
/// the C library's `atexit`, for a process that ends through the C library's exit instead (main
/// returning, `std::process::exit`), which runs it in the same order among the handlers of the
/// libraries loaded since. A process runs it once: it ends one way or the other. Under the JIT
/// (`cfg(relax)`) a program's `atexit` is the JIT's, which the JIT's exit runs.
pub fn at_exit(f: extern "C" fn()) {
    #[cfg(all(unix, not(relax)))]
    AT_EXIT.lock().unwrap_or_else(|e| e.into_inner()).push(f);
    #[cfg(not(target_arch = "wasm32"))]
    {
        extern "C" {
            fn atexit(f: extern "C" fn()) -> i32;
        }
        unsafe { atexit(f) };
    }
    #[cfg(target_arch = "wasm32")]
    let _ = f;
}

/// Ends the process with `code`: its exit work ([`at_exit`]), the flushes of Rust's and C's
/// standard streams, then `_exit` (Windows: the C runtime's exit work with `_cexit`, then
/// `TerminateProcess`, so no DLL's detach runs). Under the JIT (`cfg(relax)`) this is the
/// program's `std::process::exit`: the JIT's exit runs the program's exit work, then the host's,
/// and ends the host's process here.
pub fn end(code: i32) -> ! {
    imp::end(code)
}

#[cfg(relax)]
mod imp {
    pub fn end(code: i32) -> ! {
        std::process::exit(code)
    }
}

#[cfg(all(unix, not(relax)))]
mod imp {
    pub fn end(code: i32) -> ! {
        extern "C" {
            fn fflush(stream: *mut core::ffi::c_void) -> i32;
            fn _exit(code: i32) -> !;
        }
        // one a handler registers runs too
        while let Some(f) = {
            let mut list = super::AT_EXIT.lock().unwrap_or_else(|e| e.into_inner());
            list.pop()
        } {
            f();
        }
        super::flush_std();
        unsafe {
            fflush(std::ptr::null_mut());
            _exit(code)
        }
    }
}

#[cfg(all(windows, not(relax)))]
mod imp {
    pub fn end(code: i32) -> ! {
        extern "C" {
            fn _cexit();
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcess() -> isize;
            fn TerminateProcess(process: isize, code: u32) -> i32;
        }
        unsafe { _cexit() };
        super::flush_std();
        unsafe { TerminateProcess(GetCurrentProcess(), code as u32) };
        std::process::exit(code)
    }
}

#[cfg(not(any(unix, windows, relax)))]
mod imp {
    pub fn end(code: i32) -> ! {
        std::process::exit(code)
    }
}

#[cfg(all(any(unix, windows), not(relax)))]
fn flush_std() {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
}

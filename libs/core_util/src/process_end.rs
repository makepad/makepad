//! How a process ends, natively and under the JIT alike.
//!
//! - [`at_exit`] registers the process's own exit work. It runs once, last registered first,
//!   however the process ends: through the C library's exit (one `atexit` handler runs the list)
//!   or through [`end_without_c_handlers`] (which runs it itself).
//! - [`end`] is the normal end. On the process's main thread it is the C library's exit: every
//!   handler registered with `atexit` runs (ours, the native libraries', a profile or sanitizer
//!   runtime's), the C streams are flushed and the main thread's thread locals are destroyed. On
//!   any other thread, and on Android, it is [`end_without_c_handlers`]: the libraries' handlers
//!   are not safe to run while the process's other threads still run (a GPU driver's teardown
//!   under a thread that is inside the driver).
//! - [`end_without_c_handlers`] runs our exit work, flushes Rust's and C's standard streams, and
//!   writes the profile runtime's counts (an instrumented build, `cfg(makepad_profile_runtime)`),
//!   then calls `_exit` (Windows: the C runtime's exit work with `_cexit`, then `TerminateProcess`,
//!   so no DLL's detach runs). The JIT's exit ends this way. NVIDIA's Vulkan driver joined, at exit,
//!   one of its threads that never ended, and the JIT's process hung for good: 49 of 717 test
//!   processes under GPU load, against 0 of 715 for a native build of the same tests.
//!
//! Under the JIT (`cfg(relax)`), a program's `atexit` and `exit` are the JIT's. The JIT's exit runs
//! the program's exit work, then the host's, and ends the host's process with
//! [`end_without_c_handlers`].

/// The exit work registered with [`at_exit`], in the order it was registered.
#[cfg(all(any(unix, windows), not(relax)))]
static AT_EXIT: std::sync::Mutex<Vec<extern "C" fn()>> = std::sync::Mutex::new(Vec::new());

/// [`run_at_exit`] is on the C library's `atexit` list (once, at the first [`at_exit`]).
#[cfg(all(any(unix, windows), not(relax)))]
static HOOKED: std::sync::Once = std::sync::Once::new();

/// Registers `f` as the process's exit work. It runs once, last registered first, however the
/// process ends: through the C library's exit or through [`end_without_c_handlers`]. Under the JIT
/// (`cfg(relax)`), a program's `atexit` is the JIT's, which the JIT's exit runs.
pub fn at_exit(f: extern "C" fn()) {
    #[cfg(all(any(unix, windows), not(relax)))]
    {
        AT_EXIT.lock().unwrap_or_else(|e| e.into_inner()).push(f);
        HOOKED.call_once(|| {
            extern "C" {
                fn atexit(f: extern "C" fn()) -> i32;
            }
            unsafe { atexit(run_at_exit) };
        });
    }
    #[cfg(all(relax, not(target_arch = "wasm32")))]
    {
        extern "C" {
            fn atexit(f: extern "C" fn()) -> i32;
        }
        unsafe { atexit(f) };
    }
    #[cfg(not(any(unix, windows, all(relax, not(target_arch = "wasm32")))))]
    let _ = f;
}

/// Runs the registered exit work, last registered first. Each handler is taken off the list before
/// it runs, so it runs once even if both ends reach this. A handler that registers more work has
/// that run too.
#[cfg(all(any(unix, windows), not(relax)))]
extern "C" fn run_at_exit() {
    while let Some(f) = {
        let mut list = AT_EXIT.lock().unwrap_or_else(|e| e.into_inner());
        list.pop()
    } {
        f();
    }
}

/// Ends the process with `code`, the normal way. On the main thread: the C library's exit, with
/// every `atexit` handler. On another thread, and on Android: [`end_without_c_handlers`]. Under
/// the JIT (`cfg(relax)`): the program's `std::process::exit`, which the JIT's exit takes.
pub fn end(code: i32) -> ! {
    imp::end(code)
}

/// Ends the process with `code` without the C library's exit handlers: the exit work registered
/// with [`at_exit`], the flushes of Rust's and C's standard streams, the profile runtime's write in
/// an instrumented build, then `_exit` (Windows: `_cexit`, then `TerminateProcess`).
pub fn end_without_c_handlers(code: i32) -> ! {
    imp::end_without_c_handlers(code)
}

#[cfg(relax)]
mod imp {
    pub fn end(code: i32) -> ! {
        std::process::exit(code)
    }

    pub fn end_without_c_handlers(code: i32) -> ! {
        std::process::exit(code)
    }
}

#[cfg(all(any(unix, windows), not(relax)))]
mod imp {
    pub fn end(code: i32) -> ! {
        // (Rust names the thread that ran `main` "main"; on Android, the Java thread that runs the
        // app has no Rust main)
        if cfg!(target_os = "android") || std::thread::current().name() != Some("main") {
            end_without_c_handlers(code)
        }
        super::flush_std();
        std::process::exit(code)
    }

    #[cfg(unix)]
    pub fn end_without_c_handlers(code: i32) -> ! {
        extern "C" {
            fn fflush(stream: *mut core::ffi::c_void) -> i32;
            fn _exit(code: i32) -> !;
        }
        super::run_at_exit();
        super::flush_std();
        // an instrumented build (PGO training, coverage): the profile runtime writes its counts at
        // the C library's exit, which does not run here
        #[cfg(makepad_profile_runtime)]
        {
            extern "C" {
                fn __llvm_profile_write_file() -> i32;
            }
            unsafe { __llvm_profile_write_file() };
        }
        unsafe {
            fflush(std::ptr::null_mut());
            _exit(code)
        }
    }

    #[cfg(windows)]
    pub fn end_without_c_handlers(code: i32) -> ! {
        extern "C" {
            fn _cexit();
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcess() -> isize;
            fn TerminateProcess(process: isize, code: u32) -> i32;
        }
        super::run_at_exit();
        // the C runtime's exit work: its atexit handlers (a profile runtime's write among them)
        // and its stream flushes
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

    pub fn end_without_c_handlers(code: i32) -> ! {
        std::process::exit(code)
    }
}

#[cfg(all(any(unix, windows), not(relax)))]
fn flush_std() {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
}

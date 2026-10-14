use {
    crate::{log, thread::SignalToUI},
    std::sync::atomic::{AtomicBool, Ordering},
};

static INSTALLED: AtomicBool = AtomicBool::new(false);
static REQUESTED: AtomicBool = AtomicBool::new(false);
/// A termination was asked for at some point (never cleared).
static SEEN: AtomicBool = AtomicBool::new(false);

pub(crate) fn install() {
    if INSTALLED.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Err(err) = native::install() {
        log!("Failed to install termination signal handler: {err}");
    }
}

fn request() {
    SEEN.store(true, Ordering::Release);
    REQUESTED.store(true, Ordering::Release);
    relax_quit_requested();
    SignalToUI::set_internal_signal();
}

/// Optional loader notification (like relax_first_present in cx.rs): under Relax, the disk
/// cache is written now, while the app shuts down, so a harness or a user that stops the app
/// with SIGTERM/Ctrl+C and kills it soon after still leaves a warm cache behind. A rustc build
/// has no such symbol. Called on the termination worker thread, not in signal context.
#[cfg(any(unix, windows))]
fn relax_quit_requested() {
    let hook = crate::cx::relax_host_symbol(c"relax_quit_requested");
    if !hook.is_null() {
        let hook = unsafe { std::mem::transmute::<*mut std::ffi::c_void, extern "C" fn()>(hook) };
        hook();
    }
}

#[cfg(not(any(unix, windows)))]
fn relax_quit_requested() {}

/// Whether SIGTERM/SIGINT/SIGHUP (or the console's close) ever arrived.
pub(crate) fn seen() -> bool {
    SEEN.load(Ordering::Acquire)
}

pub(crate) fn take_requested() -> bool {
    REQUESTED.swap(false, Ordering::AcqRel)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod native {
    use super::*;
    use std::{
        ffi::{c_int, c_void},
        io::{self, Read},
        os::{fd::AsRawFd, unix::net::UnixStream},
        sync::{atomic::AtomicI32, OnceLock},
    };

    #[cfg(target_os = "linux")]
    use std::sync::atomic::AtomicU64;

    // Both endpoints live for the process lifetime, including on partial setup
    // failure. An in-flight signal must never write to a closed/reused fd.
    static WAKE: OnceLock<(UnixStream, UnixStream)> = OnceLock::new();
    static WRITE_FD: AtomicI32 = AtomicI32::new(-1);
    /// CLOCK_MONOTONIC ns of the first signal (0: none yet). A second signal forces the exit,
    /// unless it comes within SAME_REQUEST_NS: that is one request delivered twice -- GNU
    /// timeout (and so every x64run/CI wrapper) sends SIGTERM to its child and then to its
    /// process group, and the child's _exit then cut off Relax's quit write of the cache.
    #[cfg(target_os = "linux")]
    static FIRST_SIGNAL_NS: AtomicU64 = AtomicU64::new(0);
    #[cfg(target_os = "linux")]
    const SAME_REQUEST_NS: u64 = 250_000_000;
    #[cfg(target_os = "linux")]
    #[repr(C)]
    struct Timespec {
        sec: i64,
        nsec: i64,
    }
    const SIGNALS: [c_int; 3] = [2, 15, 1]; // SIGINT, SIGTERM, SIGHUP

    extern "C" {
        // Bind the BSD signal() library symbol directly: glibc/musl and Darwin
        // keep the handler installed, mask its signal and use SA_RESTART. The
        // Linux System V header redirect (__sysv_signal) is deliberately absent.
        fn signal(number: c_int, handler: usize) -> usize;
        fn write(fd: c_int, bytes: *const c_void, len: usize) -> isize;
        #[cfg_attr(target_os = "macos", link_name = "__error")]
        #[cfg_attr(target_os = "linux", link_name = "__errno_location")]
        fn errno_location() -> *mut c_int;
        #[cfg(target_os = "linux")]
        fn _exit(status: c_int) -> !;
        #[cfg(target_os = "linux")]
        fn clock_gettime(clock: c_int, ts: *mut Timespec) -> c_int;
    }

    extern "C" fn handle(_: c_int) {
        // Only lock-free atomics and async-signal-safe C calls here. In
        // particular, the UI waker may lock or enter AppKit, so a worker runs it.
        #[cfg(target_os = "linux")]
        {
            let mut ts = Timespec { sec: 0, nsec: 0 };
            unsafe { clock_gettime(1, &mut ts) }; // CLOCK_MONOTONIC; async-signal-safe
            let now = (ts.sec as u64).wrapping_mul(1_000_000_000).wrapping_add(ts.nsec as u64).max(1);
            if let Err(first) = FIRST_SIGNAL_NS.compare_exchange(0, now, Ordering::AcqRel, Ordering::Acquire) {
                if now.saturating_sub(first) >= SAME_REQUEST_NS {
                    unsafe { _exit(130) };
                }
            }
        }
        unsafe {
            let errno = errno_location();
            let saved_errno = *errno;
            let byte = 1u8;
            // A full socket already contains a wake. Retry only EINTR; never
            // wait for space, allocate, log, or disturb the interrupted errno.
            while write(
                WRITE_FD.load(Ordering::Acquire),
                (&byte as *const u8).cast(),
                1,
            ) < 0
            {
                if *errno != 4 {
                    // EINTR on both supported OSes
                    break;
                }
            }
            *errno = saved_errno;
        }
    }

    /// Nothing drains the pipe once the worker is gone, so hand the three
    /// signals back to the kernel rather than leave the process unkillable.
    fn restore_defaults() {
        const SIG_DFL: usize = 0;
        for number in SIGNALS {
            unsafe { signal(number, SIG_DFL) };
        }
    }

    pub(super) fn install() -> io::Result<()> {
        // std creates close-on-exec sockets, so hosted apps cannot keep these
        // descriptors alive. Only the signal handler's endpoint is nonblocking.
        let (reader, writer) = UnixStream::pair()?;
        writer.set_nonblocking(true)?;
        WAKE.set((reader, writer))
            .expect("termination worker installed once");
        let (reader, writer) = WAKE.get().unwrap();
        std::thread::Builder::new()
            .name("makepad-termination".into())
            .spawn(move || {
                let mut reader = reader;
                let mut bytes = [0u8; 64];
                loop {
                    match reader.read(&mut bytes) {
                        Ok(0) => {
                            restore_defaults();
                            return;
                        }
                        Ok(_) => request(),
                        Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                        Err(err) => {
                            log!("Termination signal worker failed: {err}");
                            restore_defaults();
                            return;
                        }
                    }
                }
            })?;
        WRITE_FD.store(writer.as_raw_fd(), Ordering::Release);
        // Resolve the handler's C entry points before it can interrupt a loader.
        unsafe {
            let _ = errno_location();
            write(writer.as_raw_fd(), std::ptr::null(), 0);
        }
        let mut previous = [0usize; 3];
        for (index, number) in SIGNALS.into_iter().enumerate() {
            let old = unsafe { signal(number, handle as *const () as usize) };
            if old == usize::MAX {
                let err = io::Error::last_os_error();
                for prior in (0..index).rev() {
                    unsafe { signal(SIGNALS[prior], previous[prior]) };
                }
                return Err(err);
            }
            previous[index] = old;
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod native {
    use super::*;
    use std::io;

    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }

    unsafe extern "system" fn handle(event: u32) -> i32 {
        match event {
            // Ctrl-C, Ctrl-Break, console close, logoff and shutdown.
            0 | 1 | 2 | 5 | 6 => {
                // Windows invokes this on its own thread, not an interrupted
                // thread's signal stack, so the regular UI waker is safe here.
                request();
                1
            }
            _ => 0,
        }
    }

    pub(super) fn install() -> io::Result<()> {
        if unsafe { SetConsoleCtrlHandler(Some(handle), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_request_is_seen_for_good_and_taken_once() {
        assert!(!super::seen());
        super::request();
        assert!(super::seen());
        assert!(super::take_requested());
        assert!(!super::take_requested());
        assert!(super::seen(), "taking the request does not forget it");
    }
}

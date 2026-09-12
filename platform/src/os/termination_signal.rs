use {
    crate::{log, thread::SignalToUI},
    std::sync::atomic::{AtomicBool, Ordering},
};

static INSTALLED: AtomicBool = AtomicBool::new(false);
static REQUESTED: AtomicBool = AtomicBool::new(false);

pub(crate) fn install() {
    if INSTALLED.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Err(err) = native::install() {
        log!("Failed to install termination signal handler: {err}");
    }
}

fn request() {
    REQUESTED.store(true, Ordering::Release);
    SignalToUI::set_ui_signal();
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

    // Both endpoints live for the process lifetime, including on partial setup
    // failure. An in-flight signal must never write to a closed/reused fd.
    static WAKE: OnceLock<(UnixStream, UnixStream)> = OnceLock::new();
    static WRITE_FD: AtomicI32 = AtomicI32::new(-1);
    #[cfg(target_os = "linux")]
    static SIGNAL_SEEN: AtomicBool = AtomicBool::new(false);
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
    }

    extern "C" fn handle(_: c_int) {
        // Only lock-free atomics and async-signal-safe C calls here. In
        // particular, the UI waker may lock or enter AppKit, so a worker runs it.
        #[cfg(target_os = "linux")]
        if SIGNAL_SEEN.swap(true, Ordering::AcqRel) {
            unsafe { _exit(130) };
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
                        Ok(0) => return,
                        Ok(_) => request(),
                        Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                        Err(err) => {
                            log!("Termination signal worker failed: {err}");
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

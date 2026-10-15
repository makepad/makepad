//! SIGINT / SIGTERM / SIGHUP for the `makepad run` host (Linux, macOS).
//!
//! The app runs as a HAL domain (domain >= 1) of this host, and makepad-platform installs its
//! termination handler only in an unhosted Cx (platform termination_signal.rs, restart::hosted),
//! so under the host nobody handled the signal: the kernel's default action ended the process
//! at once, before Relax's quit write of the disk cache. A harness (the merge gate, the mini
//! rows) that stops an app with SIGTERM right after its first present then left a manifest that
//! names code entries never written, or none: the next launch came up cold.
//!
//! Here, as the platform handler does for an unhosted app: the handler only writes a byte; a
//! worker starts Relax's quit write (relax_compiler::run::quit_requested) and ends the run the
//! way closing its window does (the host loop sees QUIT: the app domain stops with QUIT, the
//! window closes, the exit write runs). A second signal later than 250 ms forces the exit (one
//! request delivered twice, as GNU timeout sends it to the child and its group, is one).

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod native {
    use std::ffi::{c_int, c_void};
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
    use std::sync::OnceLock;

    static WAKE: OnceLock<(UnixStream, UnixStream)> = OnceLock::new();
    static WRITE_FD: AtomicI32 = AtomicI32::new(-1);
    static FIRST_NS: AtomicU64 = AtomicU64::new(0);
    const SAME_REQUEST_NS: u64 = 250_000_000;
    const SIGNALS: [c_int; 3] = [2, 15, 1]; // SIGINT, SIGTERM, SIGHUP

    #[repr(C)]
    struct Timespec {
        sec: i64,
        nsec: i64,
    }
    extern "C" {
        fn signal(number: c_int, handler: usize) -> usize;
        fn write(fd: c_int, bytes: *const c_void, len: usize) -> isize;
        fn _exit(status: c_int) -> !;
        fn clock_gettime(clock: c_int, ts: *mut Timespec) -> c_int;
        #[cfg_attr(target_os = "macos", link_name = "__error")]
        #[cfg_attr(target_os = "linux", link_name = "__errno_location")]
        fn errno_location() -> *mut c_int;
    }
    #[cfg(target_os = "linux")]
    const CLOCK_MONOTONIC: c_int = 1;
    #[cfg(target_os = "macos")]
    const CLOCK_MONOTONIC: c_int = 6;

    extern "C" fn handle(_: c_int) {
        // async-signal-safe calls and lock-free atomics only
        let mut ts = Timespec { sec: 0, nsec: 0 };
        unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) };
        let now = (ts.sec as u64).wrapping_mul(1_000_000_000).wrapping_add(ts.nsec as u64).max(1);
        if let Err(first) = FIRST_NS.compare_exchange(0, now, Ordering::AcqRel, Ordering::Acquire) {
            if now.saturating_sub(first) >= SAME_REQUEST_NS {
                unsafe { _exit(130) };
            }
            return;
        }
        unsafe {
            let errno = errno_location();
            let saved = *errno;
            let byte = 1u8;
            while write(WRITE_FD.load(Ordering::Acquire), (&byte as *const u8).cast(), 1) < 0 && *errno == 4 {}
            *errno = saved;
        }
    }

    /// Installs the handlers; `on_quit` runs once, on a worker thread, at the first signal.
    pub fn install(on_quit: impl FnOnce() + Send + 'static) {
        let Ok(pair) = UnixStream::pair() else { return };
        if pair.1.set_nonblocking(true).is_err() { return; }
        let Ok(mut reader) = pair.0.try_clone() else { return };
        WRITE_FD.store(pair.1.as_raw_fd(), Ordering::Release);
        if WAKE.set(pair).is_err() { return; }
        let spawned = std::thread::Builder::new().name("run-quit-signal".into()).spawn(move || {
            let mut b = [0u8; 1];
            loop {
                match reader.read(&mut b) {
                    Ok(1) => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    _ => return,
                }
            }
            on_quit();
        });
        if spawned.is_err() { return; }
        for n in SIGNALS {
            unsafe { signal(n, handle as extern "C" fn(c_int) as usize) };
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use native::install;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn install(_on_quit: impl FnOnce() + Send + 'static) {}

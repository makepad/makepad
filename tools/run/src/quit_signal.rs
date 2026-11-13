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
//! request delivered twice, as GNU timeout sends it to the child and its group, is one); the
//! endpoint's socket and discovery file go first (the exit's own cleanup is cut off).
//!
//! The first signal is said in the log with its sender (SA_SIGINFO's pid, and that process's and
//! its parents' names while they still run): a run that ended this way used to leave
//! only its app's "the last window closed" (Scope on the Mac, 11-06), as if the person had
//! closed it.

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
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
    /// the first signal and the pid that sent it (0: the kernel's, or not said)
    static SIGNAL: AtomicI32 = AtomicI32::new(0);
    static SENDER: AtomicI32 = AtomicI32::new(0);
    const SAME_REQUEST_NS: u64 = 250_000_000;
    const SIGNALS: [c_int; 3] = [2, 15, 1]; // SIGINT, SIGTERM, SIGHUP

    #[repr(C)]
    struct Timespec {
        sec: i64,
        nsec: i64,
    }
    /// `struct sigaction` and where `siginfo_t` holds `si_pid`
    #[cfg(target_vendor = "apple")]
    #[repr(C)]
    struct SigAction {
        handler: usize,
        mask: u32,
        flags: c_int,
    }
    #[cfg(target_vendor = "apple")]
    const SA_SIGINFO: c_int = 0x40;
    #[cfg(target_vendor = "apple")]
    const SA_RESTART: c_int = 0x2;
    #[cfg(target_vendor = "apple")]
    const SI_PID_AT: usize = 12;
    #[cfg(target_os = "linux")]
    #[repr(C)]
    struct SigAction {
        handler: usize,
        mask: [u64; 16],
        flags: c_int,
        pad: c_int,
        restorer: usize,
    }
    #[cfg(target_os = "linux")]
    const SA_SIGINFO: c_int = 4;
    #[cfg(target_os = "linux")]
    const SA_RESTART: c_int = 0x1000_0000;
    #[cfg(target_os = "linux")]
    const SI_PID_AT: usize = 16;

    extern "C" {
        fn sigaction(number: c_int, action: *const SigAction, old: *mut SigAction) -> c_int;
        fn write(fd: c_int, bytes: *const c_void, len: usize) -> isize;
        fn _exit(status: c_int) -> !;
        fn clock_gettime(clock: c_int, ts: *mut Timespec) -> c_int;
        #[cfg_attr(target_vendor = "apple", link_name = "__error")]
        #[cfg_attr(target_os = "linux", link_name = "__errno_location")]
        fn errno_location() -> *mut c_int;
    }
    #[cfg(target_os = "linux")]
    const CLOCK_MONOTONIC: c_int = 1;
    #[cfg(target_vendor = "apple")]
    const CLOCK_MONOTONIC: c_int = 6;

    extern "C" fn handle(number: c_int, info: *const u8, _: *mut c_void) {
        // async-signal-safe calls and lock-free atomics only
        let mut ts = Timespec { sec: 0, nsec: 0 };
        unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) };
        let now = (ts.sec as u64).wrapping_mul(1_000_000_000).wrapping_add(ts.nsec as u64).max(1);
        if let Err(first) = FIRST_NS.compare_exchange(0, now, Ordering::AcqRel, Ordering::Acquire) {
            if now.saturating_sub(first) >= SAME_REQUEST_NS {
                const FORCED: &[u8] = b"relax: a second quit signal: the process exits now\n";
                unsafe { write(2, FORCED.as_ptr().cast(), FORCED.len()) };
                // the exit's cleanup does not run past _exit: the endpoint's files go here
                makepad_remote::carrier::unix::withdraw_in_signal();
                unsafe { _exit(130) };
            }
            return;
        }
        // SAFETY: SA_SIGINFO: the kernel's siginfo_t for this signal (si_pid is the sender's)
        let sender = if info.is_null() { 0 } else { unsafe { info.add(SI_PID_AT).cast::<i32>().read_unaligned() } };
        SENDER.store(sender, Ordering::Release);
        SIGNAL.store(number, Ordering::Release);
        unsafe {
            let errno = errno_location();
            let saved = *errno;
            let byte = 1u8;
            while write(WRITE_FD.load(Ordering::Acquire), (&byte as *const u8).cast(), 1) < 0 && *errno == 4 {}
            *errno = saved;
        }
    }

    /// Installs the handlers; `on_quit` runs once, on a worker thread, at the first signal, with
    /// what it was and who sent it.
    pub fn install(on_quit: impl FnOnce(String) + Send + 'static) {
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
            on_quit(said(SIGNAL.load(Ordering::Acquire), SENDER.load(Ordering::Acquire)));
        });
        if spawned.is_err() { return; }
        let action = SigAction {
            handler: handle as extern "C" fn(c_int, *const u8, *mut c_void) as usize,
            flags: SA_SIGINFO | SA_RESTART,
            ..unsafe { std::mem::zeroed() }
        };
        for n in SIGNALS {
            unsafe { sigaction(n, &action, std::ptr::null_mut()) };
        }
    }

    /// `SIGTERM from pid 812 kill, from 811 zsh, from 790 node`: the signal and who sent it, as
    /// far as the processes still run.
    fn said(number: c_int, sender: i32) -> String {
        let name = match number {
            1 => "SIGHUP",
            2 => "SIGINT",
            15 => "SIGTERM",
            _ => "a quit signal",
        };
        match u32::try_from(sender) {
            Ok(pid) if pid > 0 => format!("{name} from {}", makepad_remote::procs::said(pid)),
            _ => format!("{name} (no sender pid)"),
        }
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub use native::install;

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
pub fn install(_on_quit: impl FnOnce(String) + Send + 'static) {}

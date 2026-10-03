//! macOS (arm64 first; x86_64 has the same layouts with the 64-bit-inode ABI) specifics.

use core::ffi::{c_char, c_int, c_void};
use core::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use core::sync::atomic::{AtomicU32, AtomicU8, AtomicUsize};
use core::time::Duration;

pub type clockid_t = u32;
pub const CLOCK_REALTIME: clockid_t = 0;
/// What real std's Instant uses on Apple targets (does not advance while asleep, like
/// mach_absolute_time).
pub const CLOCK_INSTANT: clockid_t = 8; // CLOCK_UPTIME_RAW
pub const CLOCK_MONOTONIC: clockid_t = 6;

pub const SC_NPROCESSORS_ONLN: c_int = 58;
pub const SC_PAGESIZE: c_int = 29;

pub const O_NONBLOCK: c_int = 0x4;
pub const O_APPEND: c_int = 0x8;
pub const O_NOFOLLOW: c_int = 0x100;
pub const O_CREAT: c_int = 0x200;
pub const O_TRUNC: c_int = 0x400;
pub const O_EXCL: c_int = 0x800;
pub const O_DIRECTORY: c_int = 0x100000;
pub const O_CLOEXEC: c_int = 0x1000000;

pub const LOCK_SH: c_int = 1;
pub const LOCK_EX: c_int = 2;
pub const LOCK_NB: c_int = 4;
pub const LOCK_UN: c_int = 8;

pub const EPERM: i32 = 1;
pub const ENFILE: i32 = 23;
pub const EMFILE: i32 = 24;
pub const EOPNOTSUPP: i32 = 102;
pub const ENOENT: i32 = 2;
pub const ESRCH: i32 = 3;
pub const EINTR: i32 = 4;
pub const EIO: i32 = 5;
pub const E2BIG: i32 = 7;
pub const EBADF: i32 = 9;
pub const ECHILD: i32 = 10;
pub const EDEADLK: i32 = 11;
pub const ENOMEM: i32 = 12;
pub const EACCES: i32 = 13;
pub const EBUSY: i32 = 16;
pub const EEXIST: i32 = 17;
pub const EXDEV: i32 = 18;
pub const ENOTDIR: i32 = 20;
pub const EISDIR: i32 = 21;
pub const EINVAL: i32 = 22;
pub const ETXTBSY: i32 = 26;
pub const EFBIG: i32 = 27;
pub const ENOSPC: i32 = 28;
pub const ESPIPE: i32 = 29;
pub const EROFS: i32 = 30;
pub const EMLINK: i32 = 31;
pub const EPIPE: i32 = 32;
pub const ERANGE: i32 = 34;
pub const EAGAIN: i32 = 35;
pub const EWOULDBLOCK: i32 = 35;
pub const EINPROGRESS: i32 = 36;
pub const EALREADY: i32 = 37;
pub const ENOTSOCK: i32 = 38;
pub const EADDRINUSE: i32 = 48;
pub const EADDRNOTAVAIL: i32 = 49;
pub const ENETDOWN: i32 = 50;
pub const ENETUNREACH: i32 = 51;
pub const ECONNABORTED: i32 = 53;
pub const ECONNRESET: i32 = 54;
pub const ENOTCONN: i32 = 57;
pub const ETIMEDOUT: i32 = 60;
pub const ECONNREFUSED: i32 = 61;
pub const ELOOP: i32 = 62;
pub const ENAMETOOLONG: i32 = 63;
pub const EHOSTUNREACH: i32 = 65;
pub const ENOTEMPTY: i32 = 66;
pub const EDQUOT: i32 = 69;
pub const ESTALE: i32 = 70;
pub const ENOSYS: i32 = 78;
pub const ENOTSUP: i32 = 45;

/// struct stat (64-bit inode layout, 144 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct stat_t {
    pub st_dev: i32,
    pub st_mode: u16,
    pub st_nlink: u16,
    pub st_ino: u64,
    pub st_uid: u32,
    pub st_gid: u32,
    pub st_rdev: i32,
    pub st_atime: i64,
    pub st_atime_nsec: i64,
    pub st_mtime: i64,
    pub st_mtime_nsec: i64,
    pub st_ctime: i64,
    pub st_ctime_nsec: i64,
    pub st_birthtime: i64,
    pub st_birthtime_nsec: i64,
    pub st_size: i64,
    pub st_blocks: i64,
    pub st_blksize: i32,
    pub st_flags: u32,
    pub st_gen: u32,
    pub st_lspare: i32,
    pub st_qspare: [i64; 2],
}

impl stat_t {
    pub fn zeroed() -> stat_t {
        stat_t {
            st_dev: 0, st_mode: 0, st_nlink: 0, st_ino: 0, st_uid: 0, st_gid: 0, st_rdev: 0,
            st_atime: 0, st_atime_nsec: 0, st_mtime: 0, st_mtime_nsec: 0, st_ctime: 0,
            st_ctime_nsec: 0, st_birthtime: 0, st_birthtime_nsec: 0, st_size: 0, st_blocks: 0,
            st_blksize: 0, st_flags: 0, st_gen: 0, st_lspare: 0, st_qspare: [0; 2],
        }
    }
    pub fn mode(&self) -> u32 {
        self.st_mode as u32
    }
    pub fn nlink(&self) -> u64 {
        self.st_nlink as u64
    }
    pub fn dev(&self) -> u64 {
        self.st_dev as u32 as u64
    }
    pub fn rdev(&self) -> u64 {
        self.st_rdev as u32 as u64
    }
    pub fn blksize(&self) -> u64 {
        self.st_blksize as u64
    }
    pub fn created(&self) -> Option<(i64, i64)> {
        Some((self.st_birthtime, self.st_birthtime_nsec))
    }
}

/// struct dirent (64-bit inode layout).
#[repr(C)]
pub struct dirent_t {
    pub d_ino: u64,
    pub d_seekoff: u64,
    pub d_reclen: u16,
    pub d_namlen: u16,
    pub d_type: u8,
    pub d_name: [c_char; 1024],
}

extern "C" {
    fn __error() -> *mut c_int;
    pub fn strerror_r(errnum: c_int, buf: *mut c_char, len: usize) -> c_int;
    pub fn stat(path: *const c_char, buf: *mut stat_t) -> c_int;
    pub fn lstat(path: *const c_char, buf: *mut stat_t) -> c_int;
    pub fn fstat(fd: c_int, buf: *mut stat_t) -> c_int;
    pub fn readdir(dir: *mut c_void) -> *mut dirent_t;
    fn pthread_setname_np(name: *const c_char) -> c_int;
    fn _NSGetEnviron() -> *mut *const *const c_char;
    fn _NSGetExecutablePath(buf: *mut c_char, size: *mut u32) -> c_int;
    fn _NSGetArgc() -> *mut c_int;
    fn _NSGetArgv() -> *mut *const *const c_char;
    fn pthread_threadid_np(thread: usize, id: *mut u64) -> c_int;
    fn arc4random_buf(buf: *mut c_void, n: usize);
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    // Darwin's futex since macOS 10.12 (libSystem, every macOS version Rapid supports).
    fn __ulock_wait(operation: u32, addr: *mut c_void, value: u64, timeout_us: u32) -> c_int;
    fn __ulock_wake(operation: u32, addr: *mut c_void, wake_value: u64) -> c_int;
}

const OS_CLOCK_MACH_ABSOLUTE_TIME: u32 = 32;

pub fn errno_location() -> *mut c_int {
    unsafe { __error() }
}

pub fn environ() -> *const *const c_char {
    unsafe { *_NSGetEnviron() }
}

pub fn args() -> (usize, *const *const c_char) {
    unsafe { (*_NSGetArgc() as usize, *_NSGetArgv()) }
}

pub fn executable_path(buf: &mut [u8]) -> Option<usize> {
    let mut size = buf.len() as u32;
    let r = unsafe { _NSGetExecutablePath(buf.as_mut_ptr() as *mut c_char, &mut size as *mut u32) };
    if r != 0 {
        return None;
    }
    let mut n = 0;
    while n < buf.len() && buf[n] != 0 {
        n += 1;
    }
    Some(n)
}

pub fn set_thread_name(name: *const c_char) {
    unsafe {
        pthread_setname_np(name);
    }
}

pub fn fill_random(buf: &mut [u8]) {
    unsafe { arc4random_buf(buf.as_mut_ptr() as *mut c_void, buf.len()) }
}

// ---- futex
// <os/os_sync_wait_on_address.h> (macOS 14.4+) is used when the running macOS has it; it is
// looked up with dlsym (no link-time dependency, so the binary still loads on older macOS).
// Otherwise __ulock_wait/__ulock_wake. The choice is made once and cached;
// RAPID_FORCE_ULOCK=1 forces the fallback (testing only).

type OsSyncWait = unsafe extern "C" fn(*mut c_void, u64, usize, u32) -> c_int;
type OsSyncWaitTimeout = unsafe extern "C" fn(*mut c_void, u64, usize, u32, u32, u64) -> c_int;
type OsSyncWake = unsafe extern "C" fn(*mut c_void, usize, u32) -> c_int;

const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
const UL_COMPARE_AND_WAIT: u32 = 1;
const ULF_WAKE_ALL: u32 = 0x100;

const BACKEND_UNKNOWN: u8 = 0;
const BACKEND_OS_SYNC: u8 = 1;
const BACKEND_ULOCK: u8 = 2;

static BACKEND: AtomicU8 = AtomicU8::new(BACKEND_UNKNOWN);
static OS_SYNC_WAIT: AtomicUsize = AtomicUsize::new(0);
static OS_SYNC_WAIT_TIMEOUT: AtomicUsize = AtomicUsize::new(0);
static OS_SYNC_WAKE_ANY: AtomicUsize = AtomicUsize::new(0);
static OS_SYNC_WAKE_ALL: AtomicUsize = AtomicUsize::new(0);

fn lookup(name: &[u8]) -> usize {
    unsafe { dlsym(RTLD_DEFAULT, name.as_ptr() as *const c_char) as usize }
}

fn force_ulock() -> bool {
    unsafe {
        let v = super::getenv(b"RAPID_FORCE_ULOCK\0".as_ptr() as *const c_char);
        !v.is_null() && *v == b'1' as c_char && *v.add(1) == 0
    }
}

#[cold]
fn resolve_backend() -> u8 {
    // Racing first callers resolve the same answer; the pointers are published before the
    // backend (Release), readers load the backend with Acquire.
    let mut backend = BACKEND_ULOCK;
    if !force_ulock() {
        let w = lookup(b"os_sync_wait_on_address\0");
        let wt = lookup(b"os_sync_wait_on_address_with_timeout\0");
        let wa = lookup(b"os_sync_wake_by_address_any\0");
        let wl = lookup(b"os_sync_wake_by_address_all\0");
        if w != 0 && wt != 0 && wa != 0 && wl != 0 {
            OS_SYNC_WAIT.store(w, Relaxed);
            OS_SYNC_WAIT_TIMEOUT.store(wt, Relaxed);
            OS_SYNC_WAKE_ANY.store(wa, Relaxed);
            OS_SYNC_WAKE_ALL.store(wl, Relaxed);
            backend = BACKEND_OS_SYNC;
        }
    }
    BACKEND.store(backend, Release);
    backend
}

fn backend() -> u8 {
    let b = BACKEND.load(Acquire);
    if b != BACKEND_UNKNOWN {
        b
    } else {
        resolve_backend()
    }
}

/// Which futex the process uses: "os_sync" or "ulock" (for tests).
pub fn futex_backend() -> &'static str {
    if backend() == BACKEND_OS_SYNC {
        "os_sync"
    } else {
        "ulock"
    }
}

/// One wait call; Err(errno) on failure.
fn wait_once(addr: *mut c_void, expected: u32, timeout_ns: Option<u64>) -> Result<(), i32> {
    let r = unsafe {
        if backend() == BACKEND_OS_SYNC {
            match timeout_ns {
                Some(ns) => {
                    let f: OsSyncWaitTimeout = core::mem::transmute(OS_SYNC_WAIT_TIMEOUT.load(Relaxed));
                    f(addr, expected as u64, 4, 0, OS_CLOCK_MACH_ABSOLUTE_TIME, ns)
                }
                None => {
                    let f: OsSyncWait = core::mem::transmute(OS_SYNC_WAIT.load(Relaxed));
                    f(addr, expected as u64, 4, 0)
                }
            }
        } else {
            // microseconds, 0 = forever: round up, clamp (the caller re-waits until its deadline)
            let us = match timeout_ns {
                Some(ns) => {
                    let us = (ns + 999) / 1000;
                    if us > u32::MAX as u64 {
                        u32::MAX
                    } else if us == 0 {
                        1
                    } else {
                        us as u32
                    }
                }
                None => 0,
            };
            __ulock_wait(UL_COMPARE_AND_WAIT, addr, expected as u64, us)
        }
    };
    if r >= 0 {
        Ok(())
    } else {
        Err(super::errno())
    }
}

/// Waits while `*futex == expected`. Returns false only on timeout.
pub fn futex_wait(futex: &AtomicU32, expected: u32, timeout: Option<Duration>) -> bool {
    let addr = futex.as_ptr() as *mut c_void;
    let deadline = match timeout {
        Some(d) => match super::time::now(CLOCK_MONOTONIC).checked_add(d) {
            Some(t) => Some(t),
            None => None, // overflow: wait forever
        },
        None => None,
    };
    loop {
        if futex.load(Relaxed) != expected {
            return true;
        }
        let timeout_ns = match deadline {
            Some(t) => {
                let left = match t.sub_timespec(&super::time::now(CLOCK_MONOTONIC)) {
                    Ok(d) => d,
                    Err(_) => return false,
                };
                let ns = left.as_secs().saturating_mul(1_000_000_000).saturating_add(left.subsec_nanos() as u64);
                if ns == 0 {
                    return false;
                }
                Some(ns)
            }
            None => None,
        };
        match wait_once(addr, expected, timeout_ns) {
            Ok(()) => return true,
            Err(e) => {
                if e == ETIMEDOUT {
                    // the ulock path may have woken early because of the u32 clamp
                    match deadline {
                        Some(t) => {
                            if super::time::now(CLOCK_MONOTONIC) >= t {
                                return false;
                            }
                        }
                        None => {}
                    }
                    continue;
                }
                if e == EINTR {
                    continue;
                }
                return true;
            }
        }
    }
}

/// Wakes one waiter; true if one was woken.
pub fn futex_wake(futex: &AtomicU32) -> bool {
    let addr = futex.as_ptr() as *mut c_void;
    unsafe {
        if backend() == BACKEND_OS_SYNC {
            let f: OsSyncWake = core::mem::transmute(OS_SYNC_WAKE_ANY.load(Relaxed));
            f(addr, 4, 0) == 0
        } else {
            __ulock_wake(UL_COMPARE_AND_WAIT, addr, 0) == 0
        }
    }
}

pub fn futex_wake_all(futex: &AtomicU32) {
    let addr = futex.as_ptr() as *mut c_void;
    unsafe {
        if backend() == BACKEND_OS_SYNC {
            let f: OsSyncWake = core::mem::transmute(OS_SYNC_WAKE_ALL.load(Relaxed));
            f(addr, 4, 0);
        } else {
            __ulock_wake(UL_COMPARE_AND_WAIT | ULF_WAKE_ALL, addr, 0);
        }
    }
}

/// The OS thread id real std prints in panic messages.
pub fn thread_os_id() -> u64 {
    let mut id = 0u64;
    unsafe {
        pthread_threadid_np(0, &mut id as *mut u64);
    }
    id
}

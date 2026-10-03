//! Linux (x86_64 glibc first; aarch64 differs only in the syscall number and stat layout,
//! not handled yet) specifics.

use core::ffi::{c_char, c_int, c_long, c_void};
use core::sync::atomic::AtomicU32;
use core::time::Duration;

pub type clockid_t = c_int;
pub const CLOCK_REALTIME: clockid_t = 0;
pub const CLOCK_MONOTONIC: clockid_t = 1;
/// What real std's Instant uses on Linux.
pub const CLOCK_INSTANT: clockid_t = 1;

pub const SC_NPROCESSORS_ONLN: c_int = 84;
pub const SC_PAGESIZE: c_int = 30;

pub const O_CREAT: c_int = 0x40;
pub const O_EXCL: c_int = 0x80;
pub const O_TRUNC: c_int = 0x200;
pub const O_APPEND: c_int = 0x400;
pub const O_NONBLOCK: c_int = 0x800;
pub const O_DIRECTORY: c_int = 0x10000;
pub const O_NOFOLLOW: c_int = 0x20000;
pub const O_CLOEXEC: c_int = 0x80000;

pub const LOCK_SH: c_int = 1;
pub const LOCK_EX: c_int = 2;
pub const LOCK_NB: c_int = 4;
pub const LOCK_UN: c_int = 8;

pub const EPERM: i32 = 1;
pub const ENFILE: i32 = 23;
pub const EMFILE: i32 = 24;
pub const EOPNOTSUPP: i32 = 95;
pub const ENOENT: i32 = 2;
pub const ESRCH: i32 = 3;
pub const EINTR: i32 = 4;
pub const EIO: i32 = 5;
pub const E2BIG: i32 = 7;
pub const EBADF: i32 = 9;
pub const ECHILD: i32 = 10;
pub const EAGAIN: i32 = 11;
pub const EWOULDBLOCK: i32 = 11;
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
pub const EDEADLK: i32 = 35;
pub const ENAMETOOLONG: i32 = 36;
pub const ENOSYS: i32 = 38;
pub const ENOTEMPTY: i32 = 39;
pub const ELOOP: i32 = 40;
pub const ENOTSOCK: i32 = 88;
pub const ENOTSUP: i32 = 95;
pub const EADDRINUSE: i32 = 98;
pub const EADDRNOTAVAIL: i32 = 99;
pub const ENETDOWN: i32 = 100;
pub const ENETUNREACH: i32 = 101;
pub const ECONNABORTED: i32 = 103;
pub const ECONNRESET: i32 = 104;
pub const ENOTCONN: i32 = 107;
pub const ETIMEDOUT: i32 = 110;
pub const ECONNREFUSED: i32 = 111;
pub const EHOSTUNREACH: i32 = 113;
pub const EALREADY: i32 = 114;
pub const EINPROGRESS: i32 = 115;
pub const ESTALE: i32 = 116;
pub const EDQUOT: i32 = 122;

/// struct stat, x86_64 (144 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct stat_t {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_nlink: u64,
    pub st_mode: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub pad0: i32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i64,
    pub st_blocks: i64,
    pub st_atime: i64,
    pub st_atime_nsec: i64,
    pub st_mtime: i64,
    pub st_mtime_nsec: i64,
    pub st_ctime: i64,
    pub st_ctime_nsec: i64,
    pub unused: [i64; 3],
}

impl stat_t {
    pub fn zeroed() -> stat_t {
        stat_t {
            st_dev: 0, st_ino: 0, st_nlink: 0, st_mode: 0, st_uid: 0, st_gid: 0, pad0: 0,
            st_rdev: 0, st_size: 0, st_blksize: 0, st_blocks: 0, st_atime: 0, st_atime_nsec: 0,
            st_mtime: 0, st_mtime_nsec: 0, st_ctime: 0, st_ctime_nsec: 0, unused: [0; 3],
        }
    }
    pub fn mode(&self) -> u32 {
        self.st_mode
    }
    pub fn nlink(&self) -> u64 {
        self.st_nlink
    }
    pub fn dev(&self) -> u64 {
        self.st_dev
    }
    pub fn rdev(&self) -> u64 {
        self.st_rdev
    }
    pub fn blksize(&self) -> u64 {
        self.st_blksize as u64
    }
    /// Birth time needs statx; real std reports it as unsupported without statx.
    pub fn created(&self) -> Option<(i64, i64)> {
        None
    }
}

#[repr(C)]
pub struct dirent_t {
    pub d_ino: u64,
    pub d_off: i64,
    pub d_reclen: u16,
    pub d_type: u8,
    pub d_name: [c_char; 256],
}

extern "C" {
    fn __errno_location() -> *mut c_int;
    fn __xpg_strerror_r(errnum: c_int, buf: *mut c_char, len: usize) -> c_int;
    pub fn stat(path: *const c_char, buf: *mut stat_t) -> c_int;
    pub fn lstat(path: *const c_char, buf: *mut stat_t) -> c_int;
    pub fn fstat(fd: c_int, buf: *mut stat_t) -> c_int;
    pub fn readdir(dir: *mut c_void) -> *mut dirent_t;
    fn pthread_setname_np(thread: usize, name: *const c_char) -> c_int;
    fn pthread_self() -> usize;
    fn syscall(num: c_long, ...) -> c_long;
    fn gettid() -> i32;
    fn getrandom(buf: *mut c_void, len: usize, flags: u32) -> isize;
    fn readlink(path: *const c_char, buf: *mut c_char, size: usize) -> isize;
}

const SYS_FUTEX: c_long = 202;
const FUTEX_WAIT_BITSET: c_int = 9;
const FUTEX_WAKE: c_int = 1;
const FUTEX_PRIVATE_FLAG: c_int = 128;

/// XSI strerror_r (glibc's plain strerror_r is the GNU variant).
pub unsafe fn strerror_r(errnum: c_int, buf: *mut c_char, len: usize) -> c_int {
    __xpg_strerror_r(errnum, buf, len)
}

pub fn errno_location() -> *mut c_int {
    unsafe { __errno_location() }
}

mod c {
    extern "C" {
        pub static environ: *const *const core::ffi::c_char;
    }
}

pub fn environ() -> *const *const c_char {
    unsafe { c::environ }
}

/// glibc passes argc/argv to .init_array functions; Rapid's runtime records them instead.
pub fn args() -> (usize, *const *const c_char) {
    super::rt::args()
}

pub fn executable_path(buf: &mut [u8]) -> Option<usize> {
    let n = unsafe { readlink(b"/proc/self/exe\0".as_ptr() as *const c_char, buf.as_mut_ptr() as *mut c_char, buf.len()) };
    if n < 0 || n as usize >= buf.len() {
        None
    } else {
        Some(n as usize)
    }
}

pub fn set_thread_name(name: *const c_char) {
    unsafe {
        pthread_setname_np(pthread_self(), name);
    }
}

pub fn fill_random(buf: &mut [u8]) {
    let mut off = 0;
    while off < buf.len() {
        let r = unsafe { getrandom(buf.as_mut_ptr().add(off) as *mut c_void, buf.len() - off, 0) };
        if r > 0 {
            off += r as usize;
        } else if super::errno() != EINTR {
            panic!("getrandom failed");
        }
    }
}

pub fn futex_wait(futex: &AtomicU32, expected: u32, timeout: Option<Duration>) -> bool {
    // absolute CLOCK_MONOTONIC deadline; overflow = wait forever
    let deadline = match timeout {
        Some(d) => super::time::now(CLOCK_MONOTONIC).checked_add(d),
        None => None,
    };
    loop {
        if futex.load(core::sync::atomic::Ordering::Relaxed) != expected {
            return true;
        }
        let ts = match deadline {
            Some(t) => super::timespec { tv_sec: t.secs, tv_nsec: t.nanos as c_long },
            None => super::timespec { tv_sec: 0, tv_nsec: 0 },
        };
        let tsp = match deadline {
            Some(_) => &ts as *const super::timespec,
            None => core::ptr::null(),
        };
        let r = unsafe {
            syscall(SYS_FUTEX, futex.as_ptr(), FUTEX_WAIT_BITSET | FUTEX_PRIVATE_FLAG, expected, tsp, core::ptr::null::<u32>(), !0u32)
        };
        if r < 0 {
            let e = super::errno();
            if e == ETIMEDOUT {
                return false;
            }
            if e == EINTR {
                continue;
            }
        }
        return true;
    }
}

pub fn futex_wake(futex: &AtomicU32) -> bool {
    unsafe { syscall(SYS_FUTEX, futex.as_ptr(), FUTEX_WAKE | FUTEX_PRIVATE_FLAG, 1) > 0 }
}

pub fn futex_wake_all(futex: &AtomicU32) {
    unsafe {
        syscall(SYS_FUTEX, futex.as_ptr(), FUTEX_WAKE | FUTEX_PRIVATE_FLAG, i32::MAX);
    }
}

/// The OS thread id real std prints in panic messages.
pub fn thread_os_id() -> u64 {
    unsafe { gettid() as u64 }
}

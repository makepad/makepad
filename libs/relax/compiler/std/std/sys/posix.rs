//! The POSIX layer (macOS libSystem / Linux glibc): `extern "C"` declarations and thin
//! wrappers, plus the portable sys API the rest of std uses (threads, thread-local keys,
//! stdio) in its POSIX form. OS-specific constants and layouts are in macos.rs / linux.rs.

use core::ffi::{c_char, c_int, c_long, c_void};

pub type ssize_t = isize;
pub type size_t = usize;
pub type off_t = i64;
pub type mode_t = u32;
pub type pid_t = i32;
pub type pthread_t = usize;
pub type pthread_key_t = usize;
pub type uid_t = u32;
pub type gid_t = u32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct timespec {
    pub tv_sec: i64,
    pub tv_nsec: c_long,
}

/// Opaque pthread_attr_t storage (macOS 64 bytes, glibc 56 bytes).
#[repr(C)]
pub struct pthread_attr_t {
    pub storage: [u64; 8],
}

pub const SEEK_SET: c_int = 0;
pub const SEEK_CUR: c_int = 1;
pub const SEEK_END: c_int = 2;
pub const O_RDONLY: c_int = 0;
pub const O_WRONLY: c_int = 1;
pub const O_RDWR: c_int = 2;
pub const F_GETFD: c_int = 1;
pub const F_SETFD: c_int = 2;
pub const F_GETFL: c_int = 3;
pub const F_SETFL: c_int = 4;
pub const FD_CLOEXEC: c_int = 1;
pub const S_IFMT: u32 = 0o170000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFLNK: u32 = 0o120000;
pub const S_IFIFO: u32 = 0o010000;
pub const S_IFSOCK: u32 = 0o140000;
pub const S_IFCHR: u32 = 0o020000;
pub const S_IFBLK: u32 = 0o060000;
pub const WNOHANG: c_int = 1;
pub const SIGKILL: c_int = 9;
pub const R_OK: c_int = 4;
pub const W_OK: c_int = 2;
pub const X_OK: c_int = 1;
pub const F_OK: c_int = 0;

extern "C" {
    // io
    pub fn read(fd: c_int, buf: *mut c_void, count: size_t) -> ssize_t;
    pub fn write(fd: c_int, buf: *const c_void, count: size_t) -> ssize_t;
    pub fn pread(fd: c_int, buf: *mut c_void, count: size_t, offset: off_t) -> ssize_t;
    pub fn pwrite(fd: c_int, buf: *const c_void, count: size_t, offset: off_t) -> ssize_t;
    pub fn close(fd: c_int) -> c_int;
    pub fn open(path: *const c_char, flags: c_int, ...) -> c_int;
    pub fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    pub fn lseek(fd: c_int, offset: off_t, whence: c_int) -> off_t;
    pub fn fsync(fd: c_int) -> c_int;
    pub fn ftruncate(fd: c_int, len: off_t) -> c_int;
    pub fn dup(fd: c_int) -> c_int;
    pub fn dup2(fd: c_int, fd2: c_int) -> c_int;
    pub fn pipe(fds: *mut c_int) -> c_int;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn pipe2(fds: *mut c_int, flags: c_int) -> c_int;
    pub fn isatty(fd: c_int) -> c_int;
    pub fn flock(fd: c_int, op: c_int) -> c_int;
    // fs
    pub fn mkdir(path: *const c_char, mode: mode_t) -> c_int;
    pub fn rmdir(path: *const c_char) -> c_int;
    pub fn unlink(path: *const c_char) -> c_int;
    pub fn rename(old: *const c_char, new: *const c_char) -> c_int;
    pub fn symlink(target: *const c_char, link: *const c_char) -> c_int;
    pub fn link(old: *const c_char, new: *const c_char) -> c_int;
    pub fn readlink(path: *const c_char, buf: *mut c_char, size: size_t) -> ssize_t;
    pub fn realpath(path: *const c_char, resolved: *mut c_char) -> *mut c_char;
    pub fn chmod(path: *const c_char, mode: mode_t) -> c_int;
    pub fn fchmod(fd: c_int, mode: mode_t) -> c_int;
    pub fn access(path: *const c_char, mode: c_int) -> c_int;
    pub fn opendir(path: *const c_char) -> *mut c_void;
    pub fn closedir(dir: *mut c_void) -> c_int;
    pub fn getcwd(buf: *mut c_char, size: size_t) -> *mut c_char;
    pub fn chdir(path: *const c_char) -> c_int;
    // env / process
    pub fn getenv(name: *const c_char) -> *mut c_char;
    pub fn setenv(name: *const c_char, value: *const c_char, overwrite: c_int) -> c_int;
    pub fn unsetenv(name: *const c_char) -> c_int;
    pub fn getpid() -> pid_t;
    pub fn getppid() -> pid_t;
    pub fn getuid() -> uid_t;
    pub fn exit(code: c_int) -> !;
    pub fn _exit(code: c_int) -> !;
    pub fn abort() -> !;
    pub fn kill(pid: pid_t, sig: c_int) -> c_int;
    pub fn waitpid(pid: pid_t, status: *mut c_int, options: c_int) -> pid_t;
    pub fn sysconf(name: c_int) -> c_long;
    // memory
    pub fn malloc(size: size_t) -> *mut c_void;
    pub fn free(p: *mut c_void);
    pub fn strlen(s: *const c_char) -> size_t;
    // threads
    pub fn pthread_create(native: *mut pthread_t, attr: *const pthread_attr_t, f: extern "C" fn(*mut c_void) -> *mut c_void, arg: *mut c_void) -> c_int;
    pub fn pthread_join(native: pthread_t, value: *mut *mut c_void) -> c_int;
    pub fn pthread_detach(native: pthread_t) -> c_int;
    pub fn pthread_self() -> pthread_t;
    pub fn pthread_attr_init(attr: *mut pthread_attr_t) -> c_int;
    pub fn pthread_attr_destroy(attr: *mut pthread_attr_t) -> c_int;
    pub fn pthread_attr_setstacksize(attr: *mut pthread_attr_t, size: size_t) -> c_int;
    pub fn pthread_key_create(key: *mut pthread_key_t, dtor: extern "C" fn(*mut c_void)) -> c_int;
    pub fn pthread_key_delete(key: pthread_key_t) -> c_int;
    pub fn pthread_getspecific(key: pthread_key_t) -> *mut c_void;
    pub fn pthread_setspecific(key: pthread_key_t, value: *const c_void) -> c_int;
    pub fn sched_yield() -> c_int;
    pub fn nanosleep(req: *const timespec, rem: *mut timespec) -> c_int;
    pub fn clock_gettime(clock: super::os::clockid_t, tp: *mut timespec) -> c_int;
}

pub fn errno() -> i32 {
    unsafe { *super::os::errno_location() }
}

pub fn set_errno(v: i32) {
    unsafe { *super::os::errno_location() = v }
}

/// `-1` (or any negative return) -> Err(errno).
pub fn cvt(r: c_int) -> Result<c_int, i32> {
    if r < 0 {
        Err(errno())
    } else {
        Ok(r)
    }
}

pub fn cvt_isize(r: isize) -> Result<isize, i32> {
    if r < 0 {
        Err(errno())
    } else {
        Ok(r)
    }
}

/// Retries `f` while it fails with EINTR.
pub fn cvt_r<F: FnMut() -> c_int>(mut f: F) -> Result<c_int, i32> {
    loop {
        let r = f();
        if r >= 0 {
            return Ok(r);
        }
        let e = errno();
        if e != super::os::EINTR {
            return Err(e);
        }
    }
}

/// The text of an OS error code (strerror_r), as real std prints it.
pub fn error_string(code: i32) -> alloc::string::String {
    let mut buf = [0u8; 128];
    unsafe {
        if super::os::strerror_r(code, buf.as_mut_ptr() as *mut c_char, buf.len()) < 0 {
            panic!("strerror_r failure");
        }
        let n = strlen(buf.as_ptr() as *const c_char);
        alloc::string::String::from_utf8_lossy(&buf[..n]).into_owned()
    }
}

pub fn thread_yield() {
    unsafe {
        sched_yield();
    }
}

pub fn available_parallelism() -> usize {
    let n = unsafe { sysconf(super::os::SC_NPROCESSORS_ONLN) };
    if n < 1 {
        1
    } else {
        n as usize
    }
}

// ---- the portable sys API (windows.rs has the same functions)

/// The calling thread's id (nonzero, unique among live threads).
pub fn current_thread() -> usize {
    unsafe { pthread_self() }
}

/// An owned thread: joining consumes it; dropping it detaches.
pub struct NativeThread(pthread_t);

struct StartArg {
    f: fn(*mut u8),
    arg: *mut u8,
    /// the starting thread's domain static block (relax domain.rs): the new thread runs in it
    domain: u64,
    token: u64,
}

extern "C" {
    fn __relax_domain_base() -> u64;
    fn __relax_domain_enter(base: u64, token: u64);
    fn __relax_domain_reserve(base: u64) -> u64;
    fn __relax_domain_failed(base: u64, token: u64);
}

extern "C" fn posix_thread_start(p: *mut c_void) -> *mut c_void {
    // first, before anything that may name a static
    unsafe { __relax_domain_enter((*(p as *const StartArg)).domain, (*(p as *const StartArg)).token) };
    let a = unsafe { alloc::boxed::Box::from_raw(p as *mut StartArg) };
    (a.f)(a.arg);
    core::ptr::null_mut()
}

/// Starts a thread running `f(arg)` with a stack of at least `stack_size` bytes; Err(os error).
pub fn spawn_thread(stack_size: usize, f: fn(*mut u8), arg: *mut u8) -> Result<NativeThread, i32> {
    let domain = unsafe { __relax_domain_base() };
    let token = unsafe { __relax_domain_reserve(domain) };
    let start = alloc::boxed::Box::into_raw(alloc::boxed::Box::new(StartArg { f, arg, domain, token })) as *mut c_void;
    let mut native: pthread_t = 0;
    let mut attr = pthread_attr_t { storage: [0; 8] };
    unsafe {
        let r = pthread_attr_init(&mut attr as *mut pthread_attr_t);
        if r != 0 {
            drop(alloc::boxed::Box::from_raw(start as *mut StartArg));
            __relax_domain_failed(domain, token);
            return Err(r);
        }
        let page = 16 * 1024;
        let size = (stack_size + page - 1) / page * page;
        pthread_attr_setstacksize(&mut attr as *mut pthread_attr_t, size);
        let r = pthread_create(&mut native as *mut pthread_t, &attr as *const pthread_attr_t, posix_thread_start, start);
        pthread_attr_destroy(&mut attr as *mut pthread_attr_t);
        if r != 0 {
            drop(alloc::boxed::Box::from_raw(start as *mut StartArg));
            __relax_domain_failed(domain, token);
            return Err(r);
        }
    }
    Ok(NativeThread(native))
}

pub fn join_thread(t: NativeThread) {
    // pthread_join consumes the native resource, so Drop must not detach it afterwards.
    let t = core::mem::ManuallyDrop::new(t);
    // makepad inspect's wait registry: this thread waits for thread `t` (kind 7: join)
    extern "C" {
        fn __relax_wait_enter(kind: u32, addr: usize, holder: usize);
        fn __relax_wait_leave();
    }
    unsafe {
        __relax_wait_enter(7, 0, t.0 as usize);
        pthread_join(t.0, core::ptr::null_mut());
        __relax_wait_leave();
    }
}

impl Drop for NativeThread {
    fn drop(&mut self) {
        unsafe {
            pthread_detach(self.0);
        }
    }
}

/// A thread-local storage key whose destructor runs with the thread's value at thread exit.
pub type Key = u32;
extern "C" {
    fn __relax_key_create(dtor: extern "C" fn(*mut c_void), late: u32) -> u32;
    fn __relax_key_delete(key: u32);
    fn __relax_key_get(key: u32) -> *mut c_void;
    fn __relax_key_set(key: u32, value: *const c_void);
}
/// `late`: the value type has no drop glue; its slot stays readable through every destructor
/// round and is freed (by `dtor`) after them.
pub fn key_create(dtor: extern "C" fn(*mut c_void), late: bool) -> Option<Key> { Some(unsafe { __relax_key_create(dtor, late as u32) }) }
pub fn key_delete(key: Key) { unsafe { __relax_key_delete(key); } }
pub fn key_get(key: Key) -> *mut c_void { unsafe { __relax_key_get(key) } }
pub fn key_set(key: Key, value: *const c_void) { unsafe { __relax_key_set(key, value); } }

/// Writes to standard stream `fd` (1 stdout, 2 stderr); a closed stream swallows the bytes, as
/// real std's (EBADF -> Ok(len)).
pub fn stdio_write(fd: i32, buf: &[u8]) -> Result<usize, i32> {
    let len = core::cmp::min(buf.len(), isize::MAX as usize);
    let r = unsafe { write(fd, buf.as_ptr() as *const c_void, len) };
    if r < 0 {
        let e = errno();
        if e == super::os::EBADF {
            return Ok(buf.len());
        }
        return Err(e);
    }
    Ok(r as usize)
}

/// Reads standard input; a closed stream reads as end of file (EBADF -> Ok(0)).
pub fn stdio_read(fd: i32, buf: &mut [u8]) -> Result<usize, i32> {
    let len = core::cmp::min(buf.len(), isize::MAX as usize);
    let r = unsafe { read(fd, buf.as_mut_ptr() as *mut c_void, len) };
    if r < 0 {
        let e = errno();
        if e == super::os::EBADF {
            return Ok(0);
        }
        return Err(e);
    }
    Ok(r as usize)
}

pub fn stdio_is_terminal(fd: i32) -> bool {
    unsafe { isatty(fd) == 1 }
}

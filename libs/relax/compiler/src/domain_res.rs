//! What a run holds of the OS, so that its end gives it back in whatever state the run died:
//! files, sockets, pipes, directories, mappings, signal handlers, thread-local keys, fork handlers
//! (Windows: handles, sockets, views, reservations, wait objects). The program reaches these
//! through the interposers here (jit.rs `lookup_interposer`), which see only the calls that create
//! or give back a resource: a read or a write never comes here.
//!
//! An fd number can be given back by native code this table never hears of: its entry names what
//! it was opened as, and one that is something else by now is dropped, not closed.
//!
//! A process a run started ends with it, so that restarts do not pile up a second hub or helper
//! each. A process meant to outlive the run says so the way the OS has for it: its own session
//! on unix (`setsid` before exec), DETACHED_PROCESS on Windows. It only goes down the tree as far
//! as the child's process group (unix) or job (Windows): a grandchild in neither stays.
//!
//! The process's own domain (0) is not recorded: it has no end before the process's.
use std::sync::Mutex;

/// What a run's end gave back (`domain::RunEnd`). The same fields on every platform; a pipe, a
/// directory and any other kernel object count as files.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Released {
    pub files: u32,
    pub sockets: u32,
    pub mappings: u32,
    pub mapped_bytes: u64,
    /// signal handlers, thread-local keys and fork handlers: they would have called its code
    pub callbacks: u32,
    /// processes it started and left running, ended with it
    pub children_ended: u32,
    /// processes it started to outlive it (`detached`): left alone
    pub children_detached: u32,
    /// processes it started that a kill did not end yet: tried again at the next end
    pub children_left: u32,
}

impl Released {
    /// The part of the run's ledger line, empty when there was nothing.
    pub fn line(&self) -> String {
        let mut l = String::new();
        if self.files != 0 || self.sockets != 0 {
            l += &format!("; closed {} files, {} sockets", self.files, self.sockets);
        }
        if self.mappings != 0 {
            l += &format!("; unmapped {} ({} KB)", self.mappings, self.mapped_bytes / 1024);
        }
        if self.callbacks != 0 {
            l += &format!("; {} callbacks taken back", self.callbacks);
        }
        if self.children_ended != 0 {
            l += &format!("; ended {} child processes", self.children_ended);
        }
        if self.children_detached != 0 {
            l += &format!("; {} detached processes stay", self.children_detached);
        }
        if self.children_left != 0 {
            l += &format!("; {} child processes not gone yet", self.children_left);
        }
        l
    }
}

#[cfg(unix)]
fn pages(len: usize) -> usize {
    let p = crate::os::mem::page_size() as usize;
    (len + p - 1) & !(p - 1)
}

/// The domain the calling thread's resources belong to.
fn owner() -> Option<u64> {
    let base = crate::domain::current();
    (base != 0 && base != crate::domain::default_base()).then_some(base)
}

/// An atfork child: the table's lock may be held for good by a thread that did not come along.
static FORKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

thread_local! {
    /// In the table on this thread: a signal handler's `close` (async-signal-safe, so a program
    /// may call it there) must not wait for the lock it interrupted.
    static IN_TABLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f` on the table, unless this is a place that may not wait for it (nothing is recorded
/// then: a stale entry is dropped at the end, a missing one is the one thing lost).
fn with_table<R: Default>(f: impl FnOnce(&mut Vec<Owned>) -> R) -> R {
    if FORKED.load(std::sync::atomic::Ordering::Relaxed) || IN_TABLE.with(|t| t.replace(true)) {
        return R::default();
    }
    let r = crate::jit::host_boundary("run resource table", || {
        let _kept = crate::zone_alloc::kept();
        f(&mut OWNED.lock().unwrap_or_else(|e| e.into_inner()))
    });
    IN_TABLE.with(|t| t.set(false));
    r
}

fn note(res: Res) {
    if let Some(domain) = owner() {
        with_table(|t| t.push(Owned { domain, res }));
    }
}

struct Owned {
    domain: u64,
    res: Res,
}

static OWNED: Mutex<Vec<Owned>> = Mutex::new(Vec::new());

/// The handlers a run registered with `pthread_atfork`: only its own `fork` runs them (nothing
/// is left with libc to call its code after its end).
#[cfg(unix)]
type ForkFn = Option<unsafe extern "C" fn()>;
#[cfg(unix)]
static ATFORK: Mutex<Vec<(u64, ForkFn, ForkFn, ForkFn)>> = Mutex::new(Vec::new());

#[cfg(unix)]
use unix::Res;
#[cfg(unix)]
pub(crate) use unix::*;
#[cfg(unix)]
mod unix {
    use super::*;
    use std::ffi::{c_char, c_int, c_uint, c_void};
    use std::os::fd::FromRawFd;

    #[derive(Clone, Copy)]
    pub(super) enum Res {
        Fd { fd: c_int, dev: u64, ino: u64, socket: bool },
        Dir(usize),
        Map { addr: usize, len: usize },
        Signal { sig: c_int, old: [u64; 32] },
        Key(KeyT),
        Child { pid: i32, told: bool, detached: bool },
    }

    /// pthread_key_t
    #[cfg(target_vendor = "apple")]
    pub type KeyT = usize;
    #[cfg(not(target_vendor = "apple"))]
    pub type KeyT = u32;

    /// What number `fd` is now.
    fn identity(fd: c_int) -> Option<(u64, u64, bool)> {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let file = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd) });
        let m = file.metadata().ok()?;
        Some((m.dev(), m.ino(), m.file_type().is_socket()))
    }

    fn note_fd(fd: c_int) -> c_int {
        // (stdin, stdout and stderr are the process's, whatever the program put there)
        if fd > 2 {
            if let Some((dev, ino, socket)) = identity(fd) {
                note(Res::Fd { fd, dev, ino, socket });
            }
        }
        fd
    }

    fn forget_fd(fd: c_int) {
        with_table(|t| {
            if let Some(i) = t.iter().rposition(|o| matches!(o.res, Res::Fd { fd: f, .. } if f == fd)) {
                t.swap_remove(i);
            }
        });
    }

    /// The range is no longer the run's: what is left of a mapping around it stays.
    fn forget_range(addr: usize, len: usize) {
        let end = addr + len;
        with_table(|t| {
            let mut i = 0;
            while i < t.len() {
                let domain = t[i].domain;
                if let Res::Map { addr: a, len: l } = t[i].res {
                    let e = a + l;
                    if a < end && addr < e {
                        t.swap_remove(i);
                        if a < addr {
                            t.push(Owned { domain, res: Res::Map { addr: a, len: addr - a } });
                        }
                        if end < e {
                            t.push(Owned { domain, res: Res::Map { addr: end, len: e - end } });
                        }
                        continue;
                    }
                }
                i += 1;
            }
        });
    }

    mod sys {
        use super::*;
        extern "C" {
            pub fn open(path: *const c_char, flags: c_int, ...) -> c_int;
            pub fn openat(dir: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
            pub fn creat(path: *const c_char, mode: c_uint) -> c_int;
            pub fn socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int;
            pub fn socketpair(domain: c_int, ty: c_int, protocol: c_int, fds: *mut c_int) -> c_int;
            pub fn accept(fd: c_int, addr: *mut c_void, len: *mut c_uint) -> c_int;
            pub fn pipe(fds: *mut c_int) -> c_int;
            pub fn dup(fd: c_int) -> c_int;
            pub fn dup2(from: c_int, to: c_int) -> c_int;
            pub fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
            pub fn mkstemp(template: *mut c_char) -> c_int;
            pub fn close(fd: c_int) -> c_int;
            pub fn opendir(path: *const c_char) -> *mut c_void;
            pub fn fdopendir(fd: c_int) -> *mut c_void;
            pub fn closedir(dir: *mut c_void) -> c_int;
            pub fn mmap(addr: *mut u8, len: usize, prot: c_int, flags: c_int, fd: c_int, off: i64) -> *mut u8;
            pub fn munmap(addr: *mut u8, len: usize) -> c_int;
            pub fn signal(sig: c_int, handler: usize) -> usize;
            pub fn sigaction(sig: c_int, act: *const crate::os::fault::SigAction, old: *mut crate::os::fault::SigAction) -> c_int;
            pub fn pthread_key_create(key: *mut usize, dtor: Option<extern "C" fn(*mut u8)>) -> c_int;
            pub fn pthread_key_delete(key: KeyT) -> c_int;
            pub fn posix_spawn(pid: *mut i32, path: *const c_char, actions: *const c_void, attr: *const c_void, argv: *const usize, envp: *const usize) -> c_int;
            pub fn posix_spawnp(pid: *mut i32, path: *const c_char, actions: *const c_void, attr: *const c_void, argv: *const usize, envp: *const usize) -> c_int;
            pub fn waitpid(pid: i32, status: *mut c_int, options: c_int) -> i32;
            pub fn kill(pid: i32, sig: c_int) -> c_int;
            pub fn getpgid(pid: i32) -> i32;
            pub fn getsid(pid: i32) -> i32;
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        extern "C" {
            pub fn accept4(fd: c_int, addr: *mut c_void, len: *mut c_uint, flags: c_int) -> c_int;
            pub fn pipe2(fds: *mut c_int, flags: c_int) -> c_int;
            pub fn dup3(from: c_int, to: c_int, flags: c_int) -> c_int;
            pub fn eventfd(init: c_uint, flags: c_int) -> c_int;
            pub fn inotify_init1(flags: c_int) -> c_int;
            pub fn epoll_create1(flags: c_int) -> c_int;
            pub fn timerfd_create(clock: c_int, flags: c_int) -> c_int;
            pub fn memfd_create(name: *const c_char, flags: c_uint) -> c_int;
            pub fn mremap(old: *mut c_void, old_len: usize, new_len: usize, flags: c_int, ...) -> *mut c_void;
            pub fn syscall(n: std::ffi::c_long, ...) -> std::ffi::c_long;
        }
        #[cfg(target_vendor = "apple")]
        extern "C" {
            pub fn kqueue() -> c_int;
        }
    }

    // The variadic ones (open, openat, fcntl): the next argument is in the next register, except
    // on Apple's arm64, where variadic arguments go on the stack, past the eight argument
    // registers.
    #[cfg(not(all(target_vendor = "apple", target_arch = "aarch64")))]
    pub extern "C" fn open(path: *const c_char, flags: c_int, mode: c_uint) -> c_int {
        note_fd(unsafe { sys::open(path, flags, mode) })
    }
    #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
    pub extern "C" fn open(path: *const c_char, flags: c_int, _2: u64, _3: u64, _4: u64, _5: u64, _6: u64, _7: u64, mode: c_uint) -> c_int {
        note_fd(unsafe { sys::open(path, flags, mode) })
    }
    #[cfg(not(all(target_vendor = "apple", target_arch = "aarch64")))]
    pub extern "C" fn openat(dir: c_int, path: *const c_char, flags: c_int, mode: c_uint) -> c_int {
        note_fd(unsafe { sys::openat(dir, path, flags, mode) })
    }
    #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
    pub extern "C" fn openat(dir: c_int, path: *const c_char, flags: c_int, _3: u64, _4: u64, _5: u64, _6: u64, _7: u64, mode: c_uint) -> c_int {
        note_fd(unsafe { sys::openat(dir, path, flags, mode) })
    }
    fn fcntl_inner(fd: c_int, cmd: c_int, arg: usize) -> c_int {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        const DUP_CLOEXEC: c_int = 1030;
        #[cfg(target_vendor = "apple")]
        const DUP_CLOEXEC: c_int = 67;
        const F_DUPFD: c_int = 0;
        let r = unsafe { sys::fcntl(fd, cmd, arg) };
        if (cmd == F_DUPFD || cmd == DUP_CLOEXEC) && r >= 0 {
            note_fd(r);
        }
        r
    }
    #[cfg(not(all(target_vendor = "apple", target_arch = "aarch64")))]
    pub extern "C" fn fcntl(fd: c_int, cmd: c_int, arg: usize) -> c_int {
        fcntl_inner(fd, cmd, arg)
    }
    #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
    pub extern "C" fn fcntl(fd: c_int, cmd: c_int, _2: u64, _3: u64, _4: u64, _5: u64, _6: u64, _7: u64, arg: usize) -> c_int {
        fcntl_inner(fd, cmd, arg)
    }

    pub extern "C" fn creat(path: *const c_char, mode: c_uint) -> c_int {
        note_fd(unsafe { sys::creat(path, mode) })
    }
    pub extern "C" fn socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int {
        note_fd(unsafe { sys::socket(domain, ty, protocol) })
    }
    fn note_two(r: c_int, fds: *mut c_int) -> c_int {
        if r == 0 {
            unsafe {
                note_fd(*fds);
                note_fd(*fds.add(1));
            }
        }
        r
    }
    pub extern "C" fn socketpair(domain: c_int, ty: c_int, protocol: c_int, fds: *mut c_int) -> c_int {
        note_two(unsafe { sys::socketpair(domain, ty, protocol, fds) }, fds)
    }
    pub extern "C" fn accept(fd: c_int, addr: *mut c_void, len: *mut c_uint) -> c_int {
        note_fd(unsafe { sys::accept(fd, addr, len) })
    }
    pub extern "C" fn pipe(fds: *mut c_int) -> c_int {
        note_two(unsafe { sys::pipe(fds) }, fds)
    }
    pub extern "C" fn dup(fd: c_int) -> c_int {
        note_fd(unsafe { sys::dup(fd) })
    }
    pub extern "C" fn dup2(from: c_int, to: c_int) -> c_int {
        forget_fd(to);
        let r = unsafe { sys::dup2(from, to) };
        if r >= 0 { note_fd(r) } else { r }
    }
    pub extern "C" fn mkstemp(template: *mut c_char) -> c_int {
        note_fd(unsafe { sys::mkstemp(template) })
    }
    pub extern "C" fn close(fd: c_int) -> c_int {
        // (first: once it is closed, another thread can get this number)
        forget_fd(fd);
        unsafe { sys::close(fd) }
    }
    pub extern "C" fn opendir(path: *const c_char) -> *mut c_void {
        let d = unsafe { sys::opendir(path) };
        if !d.is_null() {
            note(Res::Dir(d as usize));
        }
        d
    }
    pub extern "C" fn fdopendir(fd: c_int) -> *mut c_void {
        let d = unsafe { sys::fdopendir(fd) };
        if !d.is_null() {
            forget_fd(fd);
            note(Res::Dir(d as usize));
        }
        d
    }
    pub extern "C" fn closedir(d: *mut c_void) -> c_int {
        with_table(|t| {
            if let Some(i) = t.iter().rposition(|o| matches!(o.res, Res::Dir(x) if x == d as usize)) {
                t.swap_remove(i);
            }
        });
        unsafe { sys::closedir(d) }
    }
    pub extern "C" fn mmap(addr: *mut u8, len: usize, prot: c_int, flags: c_int, fd: c_int, off: i64) -> *mut u8 {
        let p = unsafe { sys::mmap(addr, len, prot, flags, fd, off) };
        if p as isize != -1 {
            let len = pages(len);
            // (MAP_FIXED over what it had)
            forget_range(p as usize, len);
            note(Res::Map { addr: p as usize, len });
        }
        p
    }
    pub extern "C" fn munmap(addr: *mut u8, len: usize) -> c_int {
        forget_range(addr as usize, pages(len));
        unsafe { sys::munmap(addr, len) }
    }
    fn save_handler(sig: c_int) {
        let Some(domain) = owner() else { return };
        let mut old = [0u64; 32];
        with_table(|t| {
            if !t.iter().any(|o| o.domain == domain && matches!(o.res, Res::Signal { sig: s, .. } if s == sig))
                && unsafe { sys::sigaction(sig, std::ptr::null(), old.as_mut_ptr() as *mut _) } == 0
            {
                t.push(Owned { domain, res: Res::Signal { sig, old } });
            }
        });
    }
    pub extern "C" fn signal(sig: c_int, handler: usize) -> usize {
        save_handler(sig);
        unsafe { sys::signal(sig, handler) }
    }
    pub extern "C" fn sigaction(sig: c_int, act: *const crate::os::fault::SigAction, old: *mut crate::os::fault::SigAction) -> c_int {
        if !act.is_null() {
            save_handler(sig);
        }
        unsafe { sys::sigaction(sig, act, old) }
    }
    pub extern "C" fn pthread_key_create(key: *mut KeyT, dtor: Option<extern "C" fn(*mut u8)>) -> c_int {
        let mut k = 0usize;
        let r = unsafe { sys::pthread_key_create(&mut k, dtor) };
        if r == 0 {
            unsafe { *key = k as KeyT };
            note(Res::Key(k as KeyT));
        }
        r
    }
    pub extern "C" fn pthread_key_delete(key: KeyT) -> c_int {
        with_table(|t| {
            if let Some(i) = t.iter().rposition(|o| matches!(o.res, Res::Key(k) if k == key)) {
                t.swap_remove(i);
            }
        });
        unsafe { sys::pthread_key_delete(key) }
    }
    pub(crate) fn note_child(pid: i32) {
        if pid > 0 {
            note(Res::Child { pid, told: false, detached: false });
        }
    }
    pub extern "C" fn posix_spawn(pid: *mut i32, path: *const c_char, actions: *const c_void, attr: *const c_void, argv: *const usize, envp: *const usize) -> c_int {
        let r = unsafe { sys::posix_spawn(pid, path, actions, attr, argv, envp) };
        if r == 0 && !pid.is_null() {
            note_child(unsafe { *pid });
        }
        r
    }
    pub extern "C" fn posix_spawnp(pid: *mut i32, path: *const c_char, actions: *const c_void, attr: *const c_void, argv: *const usize, envp: *const usize) -> c_int {
        let r = unsafe { sys::posix_spawnp(pid, path, actions, attr, argv, envp) };
        if r == 0 && !pid.is_null() {
            note_child(unsafe { *pid });
        }
        r
    }
    pub extern "C" fn waitpid(pid: i32, status: *mut c_int, options: c_int) -> i32 {
        let r = unsafe { sys::waitpid(pid, status, options) };
        if r > 0 {
            with_table(|t| {
                if let Some(i) = t.iter().rposition(|o| matches!(o.res, Res::Child { pid: c, .. } if c == r)) {
                    t.swap_remove(i);
                }
            });
        }
        r
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    mod linux {
        use super::*;
        use std::ffi::c_long;
        pub extern "C" fn accept4(fd: c_int, addr: *mut c_void, len: *mut c_uint, flags: c_int) -> c_int {
            note_fd(unsafe { sys::accept4(fd, addr, len, flags) })
        }
        pub extern "C" fn pipe2(fds: *mut c_int, flags: c_int) -> c_int {
            note_two(unsafe { sys::pipe2(fds, flags) }, fds)
        }
        pub extern "C" fn dup3(from: c_int, to: c_int, flags: c_int) -> c_int {
            forget_fd(to);
            let r = unsafe { sys::dup3(from, to, flags) };
            if r >= 0 { note_fd(r) } else { r }
        }
        pub extern "C" fn eventfd(init: c_uint, flags: c_int) -> c_int {
            note_fd(unsafe { sys::eventfd(init, flags) })
        }
        pub extern "C" fn inotify_init1(flags: c_int) -> c_int {
            note_fd(unsafe { sys::inotify_init1(flags) })
        }
        pub extern "C" fn epoll_create1(flags: c_int) -> c_int {
            note_fd(unsafe { sys::epoll_create1(flags) })
        }
        pub extern "C" fn timerfd_create(clock: c_int, flags: c_int) -> c_int {
            note_fd(unsafe { sys::timerfd_create(clock, flags) })
        }
        pub extern "C" fn memfd_create(name: *const c_char, flags: c_uint) -> c_int {
            note_fd(unsafe { sys::memfd_create(name, flags) })
        }
        pub extern "C" fn mremap(old: *mut c_void, old_len: usize, new_len: usize, flags: c_int, at: usize) -> *mut c_void {
            let p = unsafe { sys::mremap(old, old_len, new_len, flags, at) };
            if p as isize != -1 {
                forget_range(old as usize, pages(old_len));
                forget_range(p as usize, pages(new_len));
                note(Res::Map { addr: p as usize, len: pages(new_len) });
            }
            p
        }
        /// The fds that come from a number, not a name (a thread's perf counters). Everything else
        /// goes straight through: most of what passes here is a futex.
        pub extern "C" fn syscall(n: c_long, a: usize, b: usize, c: usize, d: usize, e: usize, f: usize) -> c_long {
            #[cfg(target_arch = "x86_64")]
            const SYS_PERF_EVENT_OPEN: c_long = 298;
            #[cfg(target_arch = "aarch64")]
            const SYS_PERF_EVENT_OPEN: c_long = 241;
            let r = unsafe { sys::syscall(n, a, b, c, d, e, f) };
            if n == SYS_PERF_EVENT_OPEN && r >= 0 {
                note_fd(r as c_int);
            }
            r
        }
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    use linux::*;
    #[cfg(target_vendor = "apple")]
    pub extern "C" fn kqueue() -> c_int {
        note_fd(unsafe { sys::kqueue() })
    }

    /// The run's address for a C symbol it binds (jit.rs `lookup_interposer`).
    pub(crate) fn interposer(link: &str) -> Option<u64> {
        macro_rules! at {
            ($f:expr) => { Some($f as *const () as usize as u64) };
        }
        match link {
            "open" | "open64" => at!(open),
            "openat" | "openat64" => at!(openat),
            "creat" | "creat64" => at!(creat),
            "socket" => at!(socket),
            "socketpair" => at!(socketpair),
            "accept" => at!(accept),
            "pipe" => at!(pipe),
            "dup" => at!(dup),
            "dup2" => at!(dup2),
            "fcntl" | "fcntl64" => at!(fcntl),
            "mkstemp" | "mkstemp64" => at!(mkstemp),
            "close" => at!(close),
            "opendir" => at!(opendir),
            "fdopendir" => at!(fdopendir),
            "closedir" => at!(closedir),
            "mmap" | "mmap64" => at!(mmap),
            "munmap" => at!(munmap),
            "signal" => at!(signal),
            "sigaction" => at!(sigaction),
            "pthread_key_create" => at!(pthread_key_create),
            "pthread_key_delete" => at!(pthread_key_delete),
            "posix_spawn" => at!(posix_spawn),
            "posix_spawnp" => at!(posix_spawnp),
            "waitpid" => at!(waitpid),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "accept4" => at!(accept4),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "pipe2" => at!(pipe2),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "dup3" => at!(dup3),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "eventfd" => at!(eventfd),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "inotify_init1" => at!(inotify_init1),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "epoll_create1" => at!(epoll_create1),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "timerfd_create" => at!(timerfd_create),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "memfd_create" => at!(memfd_create),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "mremap" => at!(mremap),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "syscall" => at!(syscall),
            #[cfg(target_vendor = "apple")]
            "kqueue" => at!(kqueue),
            _ => None,
        }
    }

    /// The program's `pthread_atfork`, kept here (`ATFORK`): libc would call the handlers after the
    /// run's end, and on threads not in its domain.
    pub extern "C" fn pthread_atfork(prepare: ForkFn, parent: ForkFn, child: ForkFn) -> c_int {
        let domain = crate::domain::current();
        let _kept = crate::zone_alloc::kept();
        ATFORK.lock().unwrap_or_else(|e| e.into_inner()).push((domain, prepare, parent, child));
        0
    }

    /// The handlers of the forking thread's domain, for `fork_child`/`fork_parent` after the fork
    /// (no lock there). `prepare` runs now, last registered first.
    pub(crate) fn atfork_prepare() -> Vec<(ForkFn, ForkFn)> {
        let domain = crate::domain::current();
        let list: Vec<_> = {
            let _kept = crate::zone_alloc::kept();
            ATFORK.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|h| h.0 == domain).copied().collect()
        };
        for h in list.iter().rev() {
            if let Some(f) = h.1 {
                unsafe { f() };
            }
        }
        list.into_iter().map(|h| (h.2, h.3)).collect()
    }
    /// After the fork, in the parent, registration order.
    pub(crate) fn atfork_parent(list: &[(ForkFn, ForkFn)]) {
        for h in list {
            if let Some(f) = h.0 {
                unsafe { f() };
            }
        }
    }
    /// In any forked child, first thing: nothing here may wait for a lock any more.
    pub(crate) fn fork_child() {
        FORKED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    /// After the fork, in the child.
    pub(crate) fn atfork_child(list: &[(ForkFn, ForkFn)]) {
        for h in list {
            if let Some(f) = h.1 {
                unsafe { f() };
            }
        }
    }

    pub(super) fn give_back_callback(res: Res) -> bool {
        match res {
            Res::Signal { sig, old } => {
                unsafe { sys::sigaction(sig, old.as_ptr() as *const _, std::ptr::null_mut()) };
                true
            }
            // (a thread leaving the run finds no destructor to call)
            Res::Key(k) => {
                unsafe { sys::pthread_key_delete(k) };
                true
            }
            _ => false,
        }
    }

    pub(super) fn take_callbacks(bases: &[u64]) -> u32 {
        let mut g = ATFORK.lock().unwrap_or_else(|e| e.into_inner());
        let n = g.len();
        g.retain(|h| !bases.contains(&h.0));
        (n - g.len()) as u32
    }

    /// Gives one resource back, a child's end aside (`end_children`).
    pub(super) fn give_back(res: Res, out: &mut Released) {
        match res {
            Res::Fd { fd, dev, ino, socket } => {
                if identity(fd).is_some_and(|i| i.0 == dev && i.1 == ino) {
                    unsafe { sys::close(fd) };
                    if socket { out.sockets += 1 } else { out.files += 1 }
                }
            }
            Res::Dir(d) => {
                unsafe { sys::closedir(d as *mut c_void) };
                out.files += 1;
            }
            Res::Map { addr, len } => {
                unsafe { sys::munmap(addr as *mut u8, len) };
                out.mappings += 1;
                out.mapped_bytes += len as u64;
            }
            Res::Signal { .. } | Res::Key(_) | Res::Child { .. } => {}
        }
    }

    const WNOHANG: c_int = 1;
    const SIGTERM: c_int = 15;
    const SIGKILL: c_int = 9;

    /// A signal to the child and the group it leads (what it started, in that case).
    fn signal_child(pid: i32, sig: c_int) {
        let target = if unsafe { sys::getpgid(pid) } == pid { -pid } else { pid };
        unsafe { sys::kill(target, sig) };
    }

    /// Ends the processes in `list`, and gives back what remains to be looked at again.
    pub(super) fn end_children(list: Vec<Res>, out: &mut Released) -> Vec<Res> {
        let own_session = unsafe { sys::getsid(0) };
        let mut left = Vec::new();
        let mut ending = Vec::new();
        for res in list {
            let Res::Child { pid, told, detached } = res else { continue };
            if unsafe { sys::waitpid(pid, std::ptr::null_mut(), WNOHANG) } != 0 {
                continue;
            }
            if detached {
                left.push(res);
            } else if unsafe { sys::getsid(pid) } != own_session {
                out.children_detached += 1;
                left.push(Res::Child { pid, told, detached: true });
            } else {
                signal_child(pid, if told { SIGKILL } else { SIGTERM });
                ending.push(pid);
            }
        }
        let wait = |ending: &mut Vec<i32>, out: &mut Released, grace: std::time::Duration| {
            let start = std::time::Instant::now();
            while !ending.is_empty() && start.elapsed() < grace {
                ending.retain(|pid| {
                    let gone = unsafe { sys::waitpid(*pid, std::ptr::null_mut(), WNOHANG) } != 0;
                    out.children_ended += gone as u32;
                    !gone
                });
                if !ending.is_empty() {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
        };
        wait(&mut ending, out, CHILD_GRACE);
        for pid in &ending {
            signal_child(*pid, SIGKILL);
        }
        wait(&mut ending, out, CHILD_KILL_GRACE);
        out.children_left += ending.len() as u32;
        left.extend(ending.into_iter().map(|pid| Res::Child { pid, told: true, detached: false }));
        left
    }

    pub(super) fn is_child(res: &Res) -> bool {
        matches!(res, Res::Child { .. })
    }
}

#[cfg(windows)]
use windows::Res;
#[cfg(windows)]
pub(crate) use windows::*;
#[cfg(windows)]
mod windows {
    use super::*;
    use crate::os::win32 as os;
    use crate::os::win32::{HANDLE, ProcessInformation, SecurityAttributes};

    const INVALID: HANDLE = -1;
    const WAIT_OBJECT_0: u32 = 0;
    const DUPLICATE_SAME_ACCESS: u32 = 2;
    const DETACHED_PROCESS: u32 = 0x8;
    const MEM_RESERVE: u32 = 0x2000;
    const MEM_RELEASE: u32 = 0x8000;

    #[derive(Clone, Copy)]
    pub(super) enum Res {
        Handle(HANDLE),
        Find(HANDLE),
        Socket(usize),
        Memory { addr: usize, len: usize },
        View { addr: usize, len: usize },
        Child { process: HANDLE, detached: bool },
    }

    /// A handle: NULL means it failed.
    fn note_handle(h: HANDLE) -> HANDLE {
        if h != 0 && h != INVALID {
            note(Res::Handle(h));
        }
        h
    }
    fn note_socket(s: usize) -> usize {
        if s != usize::MAX {
            note(Res::Socket(s));
        }
        s
    }
    fn forget(matches: impl Fn(&Res) -> bool) {
        with_table(|t| {
            if let Some(i) = t.iter().rposition(|o| matches(&o.res)) {
                t.swap_remove(i);
            }
        });
    }

    extern "system" fn create_file(name: *const u16, access: u32, share: u32, sa: *mut u8, disposition: u32, flags: u32, template: HANDLE) -> HANDLE {
        note_handle(unsafe { os::CreateFileW(name, access, share, sa, disposition, flags, template) })
    }
    extern "system" fn create_file_mapping(file: HANDLE, sa: *const SecurityAttributes, protect: u32, hi: u32, lo: u32, name: *const u16) -> HANDLE {
        note_handle(unsafe { os::CreateFileMappingW(file, sa, protect, hi, lo, name) })
    }
    extern "system" fn create_event(sa: *const u8, manual: i32, initial: i32, name: *const u16) -> HANDLE {
        note_handle(unsafe { os::CreateEventW(sa, manual, initial, name) })
    }
    extern "system" fn create_event_ex(sa: *const u8, name: *const u16, flags: u32, access: u32) -> HANDLE {
        note_handle(unsafe { os::CreateEventExW(sa, name, flags, access) })
    }
    extern "system" fn create_mutex(sa: *const u8, initial: i32, name: *const u16) -> HANDLE {
        note_handle(unsafe { os::CreateMutexW(sa, initial, name) })
    }
    extern "system" fn create_semaphore(sa: *const u8, initial: i32, max: i32, name: *const u16) -> HANDLE {
        note_handle(unsafe { os::CreateSemaphoreW(sa, initial, max, name) })
    }
    extern "system" fn create_waitable_timer_ex(sa: *const u8, name: *const u16, flags: u32, access: u32) -> HANDLE {
        note_handle(unsafe { os::CreateWaitableTimerExW(sa, name, flags, access) })
    }
    extern "system" fn create_io_completion_port(file: HANDLE, existing: HANDLE, key: usize, threads: u32) -> HANDLE {
        let h = unsafe { os::CreateIoCompletionPort(file, existing, key, threads) };
        // (with a port already there, the same handle comes back)
        if existing == 0 { note_handle(h) } else { h }
    }
    extern "system" fn create_pipe(read: *mut HANDLE, write: *mut HANDLE, sa: *const u8, size: u32) -> i32 {
        let ok = unsafe { os::CreatePipe(read, write, sa, size) };
        if ok != 0 {
            unsafe {
                note_handle(*read);
                note_handle(*write);
            }
        }
        ok
    }
    extern "system" fn create_named_pipe(name: *const u16, mode: u32, pipe_mode: u32, instances: u32, out: u32, inp: u32, timeout: u32, sa: *const u8) -> HANDLE {
        note_handle(unsafe { os::CreateNamedPipeW(name, mode, pipe_mode, instances, out, inp, timeout, sa) })
    }
    extern "system" fn create_job(sa: *const u8, name: *const u16) -> HANDLE {
        note_handle(unsafe { os::CreateJobObjectW(sa, name) })
    }
    extern "system" fn open_process(access: u32, inherit: i32, pid: u32) -> HANDLE {
        note_handle(unsafe { os::OpenProcess(access, inherit, pid) })
    }
    extern "system" fn open_thread(access: u32, inherit: i32, id: u32) -> HANDLE {
        note_handle(unsafe { os::OpenThread(access, inherit, id) })
    }
    extern "system" fn duplicate_handle(sp: HANDLE, s: HANDLE, tp: HANDLE, t: *mut HANDLE, access: u32, inherit: i32, options: u32) -> i32 {
        let ok = unsafe { os::DuplicateHandle(sp, s, tp, t, access, inherit, options) };
        if ok != 0 && !t.is_null() && tp == unsafe { os::GetCurrentProcess() } {
            note_handle(unsafe { *t });
        }
        ok
    }
    extern "system" fn close_handle(h: HANDLE) -> i32 {
        // (first: once it is closed, another thread can get this number)
        forget(|r| matches!(r, Res::Handle(x) if *x == h));
        unsafe { os::CloseHandle(h) }
    }
    extern "system" fn create_process(app: *const u16, cmd: *mut u16, pa: *const u8, ta: *const u8, inherit: i32, flags: u32, env: *mut u8, cwd: *const u16, si: *mut u8, pi: *mut ProcessInformation) -> i32 {
        let ok = unsafe { os::CreateProcessW(app, cmd, pa, ta, inherit, flags, env, cwd, si, pi) };
        if ok != 0 {
            let pi = unsafe { &*pi };
            note_handle(pi.process);
            note_handle(pi.thread);
            // (the run's own handle may be closed long before the process ends)
            let me = unsafe { os::GetCurrentProcess() };
            let mut process = 0;
            if unsafe { os::DuplicateHandle(me, pi.process, me, &mut process, 0, 0, DUPLICATE_SAME_ACCESS) } != 0 {
                forget_ended_children();
                note(Res::Child { process, detached: flags & DETACHED_PROCESS != 0 });
            }
        }
        ok
    }
    /// Those that are over stop holding their process's record.
    fn forget_ended_children() {
        with_table(|t| {
            t.retain(|o| match o.res {
                Res::Child { process, .. } if unsafe { os::WaitForSingleObject(process, 0) } == WAIT_OBJECT_0 => {
                    unsafe { os::CloseHandle(process) };
                    false
                }
                _ => true,
            })
        });
    }
    extern "system" fn find_first_ex(name: *const u16, level: i32, data: *mut u8, op: i32, filter: *const u8, flags: u32) -> HANDLE {
        let h = unsafe { os::FindFirstFileExW(name, level, data, op, filter, flags) };
        if h != INVALID {
            note(Res::Find(h));
        }
        h
    }
    extern "system" fn find_first(name: *const u16, data: *mut u8) -> HANDLE {
        let h = unsafe { os::FindFirstFileW(name, data) };
        if h != INVALID {
            note(Res::Find(h));
        }
        h
    }
    extern "system" fn find_close(h: HANDLE) -> i32 {
        forget(|r| matches!(r, Res::Find(x) if *x == h));
        unsafe { os::FindClose(h) }
    }
    extern "system" fn socket(af: i32, ty: i32, protocol: i32) -> usize {
        note_socket(unsafe { os::socket(af, ty, protocol) })
    }
    extern "system" fn wsa_socket(af: i32, ty: i32, protocol: i32, info: *const u8, group: u32, flags: u32) -> usize {
        note_socket(unsafe { os::WSASocketW(af, ty, protocol, info, group, flags) })
    }
    extern "system" fn accept(s: usize, addr: *mut u8, len: *mut i32) -> usize {
        note_socket(unsafe { os::accept(s, addr, len) })
    }
    extern "system" fn closesocket(s: usize) -> i32 {
        forget(|r| matches!(r, Res::Socket(x) if *x == s));
        unsafe { os::closesocket(s) }
    }
    extern "system" fn virtual_alloc(addr: *mut u8, len: usize, kind: u32, protect: u32) -> *mut u8 {
        let p = unsafe { os::VirtualAlloc(addr, len, kind, protect) };
        // (a commit in what it reserved is not another one)
        if !p.is_null() && kind & MEM_RESERVE != 0 {
            note(Res::Memory { addr: p as usize, len });
        }
        p
    }
    extern "system" fn virtual_free(addr: *mut u8, len: usize, kind: u32) -> i32 {
        if kind & MEM_RELEASE != 0 {
            forget(|r| matches!(r, Res::Memory { addr: a, .. } if *a == addr as usize));
        }
        unsafe { os::VirtualFree(addr, len, kind) }
    }
    fn note_view(p: *mut u8, len: usize) -> *mut u8 {
        if !p.is_null() {
            note(Res::View { addr: p as usize, len });
        }
        p
    }
    extern "system" fn map_view(h: HANDLE, access: u32, hi: u32, lo: u32, size: usize) -> *mut u8 {
        note_view(unsafe { os::MapViewOfFile(h, access, hi, lo, size) }, size)
    }
    extern "system" fn map_view_ex(h: HANDLE, access: u32, hi: u32, lo: u32, size: usize, base: *mut u8) -> *mut u8 {
        note_view(unsafe { os::MapViewOfFileEx(h, access, hi, lo, size, base) }, size)
    }
    extern "system" fn unmap_view(addr: *mut u8) -> i32 {
        forget(|r| matches!(r, Res::View { addr: a, .. } if *a == addr as usize));
        unsafe { os::UnmapViewOfFile(addr) }
    }

    /// The run's address for a C symbol it binds (jit.rs `lookup_interposer`).
    pub(crate) fn interposer(link: &str) -> Option<u64> {
        macro_rules! at {
            ($f:expr) => { Some($f as *const () as usize as u64) };
        }
        match link {
            "CreateFileW" => at!(create_file),
            "CreateFileMappingW" => at!(create_file_mapping),
            "CreateEventW" => at!(create_event),
            "CreateEventExW" => at!(create_event_ex),
            "CreateMutexW" => at!(create_mutex),
            "CreateSemaphoreW" => at!(create_semaphore),
            "CreateWaitableTimerExW" => at!(create_waitable_timer_ex),
            "CreateIoCompletionPort" => at!(create_io_completion_port),
            "CreatePipe" => at!(create_pipe),
            "CreateNamedPipeW" => at!(create_named_pipe),
            "CreateJobObjectW" => at!(create_job),
            "OpenProcess" => at!(open_process),
            "OpenThread" => at!(open_thread),
            "DuplicateHandle" => at!(duplicate_handle),
            "CloseHandle" => at!(close_handle),
            "CreateProcessW" => at!(create_process),
            "FindFirstFileExW" => at!(find_first_ex),
            "FindFirstFileW" => at!(find_first),
            "FindClose" => at!(find_close),
            "socket" => at!(socket),
            "WSASocketW" => at!(wsa_socket),
            "accept" => at!(accept),
            "closesocket" => at!(closesocket),
            "VirtualAlloc" => at!(virtual_alloc),
            "VirtualFree" => at!(virtual_free),
            "MapViewOfFile" => at!(map_view),
            "MapViewOfFileEx" => at!(map_view_ex),
            "UnmapViewOfFile" => at!(unmap_view),
            _ => None,
        }
    }

    pub(super) fn give_back_callback(_res: Res) -> bool {
        false
    }

    pub(super) fn take_callbacks(_bases: &[u64]) -> u32 {
        0
    }

    /// Gives one resource back, a child's end aside (`end_children`).
    pub(super) fn give_back(res: Res, out: &mut Released) {
        unsafe {
            match res {
                Res::Handle(h) => {
                    os::CloseHandle(h);
                    out.files += 1;
                }
                Res::Find(h) => {
                    os::FindClose(h);
                    out.files += 1;
                }
                Res::Socket(s) => {
                    os::closesocket(s);
                    out.sockets += 1;
                }
                Res::Memory { addr, len } => {
                    os::VirtualFree(addr as *mut u8, 0, MEM_RELEASE);
                    out.mappings += 1;
                    out.mapped_bytes += len as u64;
                }
                Res::View { addr, len } => {
                    os::UnmapViewOfFile(addr as *mut u8);
                    out.mappings += 1;
                    out.mapped_bytes += len as u64;
                }
                Res::Child { .. } => {}
            }
        }
    }

    /// Ends the processes in `list`, and gives back what remains to be looked at again. There is
    /// no asking here: a process is stopped. (Everything in a job the run made went when the
    /// job's handle closed.)
    pub(super) fn end_children(list: Vec<Res>, out: &mut Released) -> Vec<Res> {
        let mut ending = Vec::new();
        for res in list {
            let Res::Child { process, detached } = res else { continue };
            if detached {
                unsafe { os::CloseHandle(process) };
                out.children_detached += 1;
            } else if unsafe { os::WaitForSingleObject(process, 0) } == WAIT_OBJECT_0 {
                unsafe { os::CloseHandle(process) };
            } else {
                unsafe { os::TerminateProcess(process, 1) };
                ending.push(process);
            }
        }
        let start = std::time::Instant::now();
        while !ending.is_empty() && start.elapsed() < CHILD_GRACE + CHILD_KILL_GRACE {
            ending.retain(|h| {
                let gone = unsafe { os::WaitForSingleObject(*h, 0) } == WAIT_OBJECT_0;
                if gone {
                    unsafe { os::CloseHandle(*h) };
                    out.children_ended += 1;
                }
                !gone
            });
            if !ending.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        out.children_left += ending.len() as u32;
        ending.into_iter().map(|process| Res::Child { process, detached: false }).collect()
    }

    pub(super) fn is_child(res: &Res) -> bool {
        matches!(res, Res::Child { .. })
    }
}

/// The domain of what outlived its run and is looked at again at the next end (a child: its exit
/// is reaped there). No run has this base.
const OUTLIVED: u64 = 0;

/// How long a run's children get to end by themselves, before they are made to.
const CHILD_GRACE: std::time::Duration = std::time::Duration::from_millis(500);
/// And how long the next run waits for those, before it starts anyway.
const CHILD_KILL_GRACE: std::time::Duration = std::time::Duration::from_millis(100);

/// Before a run's code ends: what would call it from threads not its own. Returns how many.
pub(crate) fn take_back_callbacks(bases: &[u64]) -> u32 {
    let mut n = take_callbacks(bases);
    crate::os::proc::forget_program_atexit(bases);
    with_table(|t| {
        // (last taken first: a handler saved twice over goes back to what it was before the run)
        for i in (0..t.len()).rev() {
            if bases.contains(&t[i].domain) && give_back_callback(t[i].res) {
                t.remove(i);
                n += 1;
            }
        }
    });
    n
}

/// After a run's threads have left: everything of its still open. Its children may take a moment
/// (`CHILD_GRACE`): nothing waits for this table meanwhile.
pub(crate) fn give_back_run(bases: &[u64]) -> Released {
    let mut out = Released::default();
    let mut children = Vec::new();
    with_table(|t| {
        let mut i = 0;
        while i < t.len() {
            if bases.contains(&t[i].domain) || t[i].domain == OUTLIVED {
                let o = t.swap_remove(i);
                if is_child(&o.res) {
                    children.push(o.res);
                } else {
                    give_back(o.res, &mut out);
                }
            } else {
                i += 1;
            }
        }
    });
    let left = end_children(children, &mut out);
    with_table(|t| t.extend(left.into_iter().map(|res| Owned { domain: OUTLIVED, res })));
    out
}

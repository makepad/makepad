//! A confined build script's run: under a Host whose compile-time policy is Sandboxed, a script
//! whose closure holds a registry or git package runs its program unit whole as twins, in one
//! arena of the `build-script` domain, entered as the macro world's bridge is (pmhost.rs
//! `call_confined`). The script reaches nothing of the host but this domain's entries:
//! - the environment its parent gave (progenv `given_value`), as arena copies;
//! - its exit handlers, recorded in the arena and called in it (`call_in_arena`);
//! - `exit`, which runs them and ends the run as an exit does.
//!
//! The domain's table has no form that keeps a host function (`entry` returns a replacement or
//! nothing), so no host function's result reaches the script unless an entry made it: every
//! pointer an entry hands back lies in the arena (`ret_arena`), and every pointer it is given is
//! read inside the arena's window. A host address masked into the window would read the arena's
//! bytes at that offset (a flaky crash, garbage): an entry's mistake gives the script NULL.
//!
//! A fault of the script's code ends the child (its masked accesses stay in the window: the
//! process is the boundary), said as the script's (`fault_note`); a panic ends it with 101 after
//! its exit handlers ran (stdout's tail).

use super::*;
use crate::program::DefId;

/// The build-script domain: no fuel (a script's time is the person's to end), the clean-up of
/// the masks always (its twins never tier up).
pub const SCRIPT_DOMAIN: Domain = Domain { name: "build-script", allow: script_allow, fuel: false, clean_up: true };

/// The script arena's module number (the macro world's is u32::MAX).
const SCRIPT_MODULE: u32 = u32::MAX - 1;

/// The confined script's arena (its window base; 0: no confined script in this process).
static ARENA_BASE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The domain's entries as `Allow` sees them: every one a replacement (`entry`).
fn script_allow(name: &str) -> Option<Option<(&'static str, u64)>> {
    entry(name).map(Some)
}

/// The domain's entries: a C name and its replacement. The type has no kept form.
fn entry(name: &str) -> Option<(&'static str, u64)> {
    macro_rules! e {
        ($f:ident) => {
            (stringify!($f), $f as *const () as u64)
        };
    }
    Some(match name {
        "getenv" => e!(sc_getenv),
        "relax_env_snapshot" => e!(sc_env_snapshot),
        "relax_host_getenv" => e!(sc_host_getenv),
        "relax_pm_getenv" => e!(sc_pm_getenv),
        "atexit" => e!(sc_atexit),
        "exit" => e!(sc_exit),
        #[cfg(unix)]
        _ => return files::entry(name),
        #[cfg(not(unix))]
        _ => return None,
    })
}

/// The folders a confined script reaches, as its parent gives them (absolute paths).
#[derive(Default)]
pub struct Roots {
    /// folders it reads (its package's source)
    pub read: Vec<Vec<u8>>,
    /// folders it reads and writes (OUT_DIR)
    pub write: Vec<Vec<u8>>,
    /// folders inside those it never reaches (the run's target folder, Relax's cache)
    pub excluded: Vec<Vec<u8>>,
    /// store folders of Relax's VFS it reads (a git package's source; virtual paths)
    pub store: Vec<Vec<u8>>,
    /// another spelling of a folder of `read`: (spelling, folder) (a folder mount's virtual path
    /// of its disk folder)
    pub alias: Vec<(Vec<u8>, Vec<u8>)>,
    /// its current folder at the start (the manifest folder, as cargo starts a script)
    pub cwd: Vec<u8>,
}

/// A confined script's folders from what its parent named (relax-base resolve.rs `ScriptFolders`)
/// and OUT_DIR, the one it writes (its environment's): a store mount's path is a store folder, a
/// folder mount's path its disk folder under both spellings, any other path a disk folder.
pub fn roots_of(f: &relax_base::resolve::ScriptFolders) -> Result<Roots, String> {
    let out = std::env::var_os("OUT_DIR").filter(|o| !o.is_empty()).ok_or("its parent gave it no OUT_DIR")?;
    let bytes = |s: &str| s.as_bytes().to_vec();
    let mut r = Roots { write: vec![out.into_encoded_bytes()], ..Default::default() };
    for p in &f.read {
        if crate::vfs::in_store(p) {
            r.store.push(bytes(p));
            continue;
        }
        let disk = crate::vfs::on_disk(p).into_owned();
        if disk != *p {
            r.alias.push((bytes(p), bytes(&disk)));
        }
        r.read.push(bytes(&disk));
    }
    r.excluded = f.excluded.iter().map(|p| bytes(&crate::vfs::on_disk(p))).collect();
    r.cwd = bytes(&crate::vfs::on_disk(&f.cwd));
    Ok(r)
}

/// What the script asked for and was refused (broker.rs `withhold`), said when it failed or under
/// RELAX_VERBOSE: a script that probes and takes its fallback says nothing.
fn say_withheld(code: i32) {
    #[cfg(unix)]
    {
        if code == 0 && std::env::var_os("RELAX_VERBOSE").is_none() {
            return;
        }
        let list = files::BROKER.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|b| b.withheld()).unwrap_or_default();
        if !list.is_empty() {
            eprintln!("relax: the confined build script was refused: {}", list.join(", "));
        }
    }
    #[cfg(not(unix))]
    let _ = code;
}

const ENAMETOOLONG: i32 = if cfg!(target_vendor = "apple") { 63 } else { 36 };

/// A C string of the script's (`p`), copied out of its window: at most 4096 bytes before the NUL
/// (ENAMETOOLONG), never past the window (EFAULT).
fn c_str(p: u64) -> Result<Vec<u8>, i32> {
    // (an unmapped page of the arena faults as the script's own read would: contained)
    let _safe = crate::jit::FaultSafe::enter();
    let pm = m(p);
    if pm == 0 {
        return Err(EFAULT);
    }
    let mut out = Vec::new();
    loop {
        if !fits(pm, out.len() as u64 + 1) {
            return Err(EFAULT);
        }
        let b = unsafe { *((pm + out.len() as u64) as *const u8) };
        if b == 0 {
            return Ok(out);
        }
        out.push(b);
        if out.len() >= 4096 {
            return Err(ENAMETOOLONG);
        }
    }
}

/// Writes `v` through the script's out pointer `p` (false: out of its window).
fn put_u64(p: u64, v: u64) -> bool {
    let _safe = crate::jit::FaultSafe::enter();
    let pm = arena_ptr(p, 8);
    if pm == 0 {
        return false;
    }
    unsafe { std::ptr::write_unaligned(pm as *mut u64, v) };
    true
}

// ------------------------------------------------------------------ environment

/// (arena base, name, its value's NUL-ended arena copy): made at the name's first ask, kept for
/// the arena's life (C's `getenv` contract: a script's own FFI may hold two results)
static INTERNED: Mutex<Vec<(u64, Vec<u8>, u64)>> = Mutex::new(Vec::new());

fn interned(name: &[u8], value: &[u8]) -> u64 {
    let Some(a) = cur_arena() else { return 0 };
    let mut g = INTERNED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&(_, _, p)) = g.iter().find(|(b, n, _)| *b == a.base && n == name) {
        return ret_arena(p);
    }
    let p = sb_malloc(value.len() as u64 + 1);
    if p == 0 {
        return 0;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(value.as_ptr(), p as *mut u8, value.len());
        *((p + value.len() as u64) as *mut u8) = 0;
    }
    g.push((a.base, name.to_vec(), p));
    ret_arena(p)
}

/// `getenv`: the names the script's parent gave, nothing of this process's environment.
extern "C" fn sc_getenv(name: u64) -> u64 {
    let Ok(n) = c_str(name) else { return 0 };
    match relax_base::progenv::given_value(&n) {
        Some(v) => interned(&n, &v),
        None => 0,
    }
}

/// std's `vars_os`: the given set, encoded (std copies it under its lock).
extern "C" fn sc_env_snapshot(out: u64, out_len: u64) {
    let b = relax_base::progenv::given_wire();
    let p = ret_arena(give(&b));
    if put_u64(out, p) {
        put_u64(out_len, if p == 0 { 0 } else { b.len() as u64 });
    }
}

/// std's plumbing reads: none of the host's values reach the script. RELAX_WORKSPACE, std's
/// virtual current folder, is the script's own when its manifest folder is virtual.
extern "C" fn sc_host_getenv(name: u64) -> u64 {
    #[cfg(unix)]
    if c_str(name).is_ok_and(|n| n == b"RELAX_WORKSPACE") {
        let v = files::BROKER.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|b| b.virtual_cwd.clone());
        if let Some(v) = v {
            return interned(b"RELAX_WORKSPACE", v.as_bytes());
        }
    }
    let _ = name;
    0
}

/// The proc-macro environment call: not set (a script reads its environment through `getenv`).
extern "C" fn sc_pm_getenv(_key: u64, _len: u64, _out: u64, _out_len: u64) -> i32 {
    1
}

// ------------------------------------------------------------------ exit

/// (arena base, a handler's fn value as the script gave it): `atexit`'s list, last first
static EXITS: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());
/// the handlers ran (a second `exit`, from inside one, goes straight to the end)
static EXITED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn sc_atexit(f: u64) -> i32 {
    let Some(a) = cur_arena() else { return -1 };
    EXITS.lock().unwrap_or_else(|e| e.into_inner()).push((a.base, f));
    0
}

/// Calls the current arena's exit handlers, last registered first, until none is left (one a
/// handler registers runs too); a handler that panics is reported and the next one runs.
fn run_exit_handlers() {
    let Some(a) = cur_arena() else { return };
    EXITED.store(true, std::sync::atomic::Ordering::SeqCst);
    loop {
        let next = {
            let mut g = EXITS.lock().unwrap_or_else(|e| e.into_inner());
            let i = g.iter().rposition(|(b, _)| *b == a.base);
            i.map(|i| g.remove(i).1)
        };
        let Some(f) = next else { break };
        call_in_arena(f, 0);
    }
}

extern "C" fn sc_exit(code: i32) {
    if !EXITED.load(std::sync::atomic::Ordering::SeqCst) {
        run_exit_handlers();
    }
    end(code)
}

/// The run's end as an exit ends it (jit.rs `recorded_exit`).
fn end(code: i32) -> ! {
    say_withheld(code);
    #[cfg(unix)]
    if let Some(f) = crate::jit::loader_symbol(b"exit") {
        let f: extern "C" fn(i32) -> ! = unsafe { std::mem::transmute(f) };
        f(code)
    }
    std::process::exit(code)
}

/// A run's end in place: sandbox.rs `reset_statics` empties the arena table, so no confined
/// script is left in this process, and nothing it was given or did (its arena, its environment's
/// copies, its exit handlers, its broker with the folders it reaches and its open fds) may reach
/// the next run.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&ARENA_BASE, std::sync::atomic::AtomicU64::new(0));
    reset(&INTERNED, Mutex::new(Vec::new()));
    reset(&EXITS, Mutex::new(Vec::new()));
    reset(&EXITED, std::sync::atomic::AtomicBool::new(false));
    #[cfg(unix)]
    files::reset_statics();
}

// ------------------------------------------------------------------ files

/// The file entries: paths go to the broker's walk (broker.rs), fds are the broker's table.
#[cfg(unix)]
mod files {
    use super::super::broker::{c, last_errno, Broker, Fd, Opened};
    use super::*;
    use std::ffi::{c_int, c_void};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

    /// The broker of this process's confined script (one per child).
    pub(super) static BROKER: Mutex<Option<Broker>> = Mutex::new(None);

    /// (script.rs `reset_statics`): the broker goes, its fds closed.
    pub(super) unsafe fn reset_statics() {
        relax_session::reset_static(&BROKER, Mutex::new(None));
    }

    pub(super) fn entry(name: &str) -> Option<(&'static str, u64)> {
        macro_rules! e {
            ($f:ident) => {
                (stringify!($f), $f as *const () as u64)
            };
        }
        Some(match name {
            "open" => e!(sc_open),
            "close" => e!(sc_close),
            "read" => e!(sc_read),
            "write" => e!(sc_write),
            "pread" => e!(sc_pread),
            "pwrite" => e!(sc_pwrite),
            "lseek" => e!(sc_lseek),
            "fstat" => e!(sc_fstat),
            "stat" => e!(sc_stat),
            "lstat" => e!(sc_lstat),
            "fsync" => e!(sc_fsync),
            "ftruncate" => e!(sc_ftruncate),
            "fchmod" => e!(sc_fchmod),
            "flock" => e!(sc_flock),
            "isatty" => e!(sc_isatty),
            "fcntl" => e!(sc_fcntl),
            "chmod" => e!(sc_chmod),
            "mkdir" => e!(sc_mkdir),
            "rmdir" => e!(sc_rmdir),
            "unlink" => e!(sc_unlink),
            "rename" => e!(sc_rename),
            "readlink" => e!(sc_readlink),
            "realpath" => e!(sc_realpath),
            "getcwd" => e!(sc_getcwd),
            "chdir" => e!(sc_chdir),
            "opendir" => e!(sc_opendir),
            "readdir" => e!(sc_readdir),
            "closedir" => e!(sc_closedir),
            "pipe" => e!(sc_pipe),
            "pipe2" => e!(sc_pipe2),
            "poll" => e!(sc_poll),
            "relax_anon_fd" => e!(sc_anon_fd),
            "relax_tool_spawn" => e!(sc_tool_spawn),
            "relax_tool_wait" => e!(sc_tool_wait),
            "relax_tool_release" => e!(sc_tool_release),
            "relax_vfs_stat" => e!(sc_vfs_stat),
            "relax_vfs_read" => e!(sc_vfs_read),
            "relax_vfs_list" => e!(sc_vfs_list),
            "relax_vfs_release" => e!(sc_vfs_release),
            "relax_vfs_edit" => e!(sc_vfs_edit),
            "relax_vfs_mounts" => e!(sc_vfs_mounts),
            "relax_vfs_note" => e!(sc_vfs_note),
            _ => return None,
        })
    }

    extern "C" {
        #[link_name = "read"]
        fn host_read(fd: c_int, buf: *mut c_void, n: usize) -> isize;
        #[link_name = "write"]
        fn host_write(fd: c_int, buf: *const c_void, n: usize) -> isize;
        #[link_name = "pread"]
        fn host_pread(fd: c_int, buf: *mut c_void, n: usize, off: i64) -> isize;
        #[link_name = "pwrite"]
        fn host_pwrite(fd: c_int, buf: *const u8, n: usize, off: i64) -> isize;
        #[link_name = "lseek"]
        fn host_lseek(fd: c_int, off: i64, whence: c_int) -> i64;
        #[cfg_attr(all(target_vendor = "apple", target_arch = "x86_64"), link_name = "fstat$INODE64")]
        #[cfg_attr(not(all(target_vendor = "apple", target_arch = "x86_64")), link_name = "fstat")]
        fn host_fstat(fd: c_int, buf: *mut u8) -> c_int;
        #[link_name = "fsync"]
        fn host_fsync(fd: c_int) -> c_int;
        #[link_name = "ftruncate"]
        fn host_ftruncate(fd: c_int, len: i64) -> c_int;
        #[link_name = "fchmod"]
        fn host_fchmod(fd: c_int, mode: ModeT) -> c_int;
        #[link_name = "flock"]
        fn host_flock(fd: c_int, op: c_int) -> c_int;
        #[link_name = "isatty"]
        fn host_isatty(fd: c_int) -> c_int;
        #[link_name = "fcntl"]
        fn host_fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
        #[link_name = "pipe"]
        fn host_pipe(fds: *mut c_int) -> c_int;
        fn poll(fds: *mut PollFd, n: NfdsT, timeout: c_int) -> c_int;
        #[cfg_attr(all(target_vendor = "apple", target_arch = "x86_64"), link_name = "readdir$INODE64")]
        #[cfg_attr(not(all(target_vendor = "apple", target_arch = "x86_64")), link_name = "readdir")]
        fn host_readdir(dir: *mut c_void) -> *const u8;
        #[link_name = "closedir"]
        fn host_closedir(dir: *mut c_void) -> c_int;
    }
    /// struct pollfd, as reload_watch.rs and tui.rs declare it.
    #[repr(C)]
    struct PollFd {
        fd: c_int,
        events: i16,
        revents: i16,
    }
    // nfds_t: glibc's unsigned long, bionic's and Apple's unsigned int (as tui.rs declares poll)
    #[cfg(target_os = "linux")]
    type NfdsT = u64;
    #[cfg(not(target_os = "linux"))]
    type NfdsT = u32;
    #[cfg(target_vendor = "apple")]
    type ModeT = u16;
    #[cfg(not(target_vendor = "apple"))]
    type ModeT = u32;

    /// An entry's failure: the script's errno, -1.
    fn fail(e: i32) -> i64 {
        set_errno(e)
    }

    /// The broker's answer, or EACCES when this process has none (fail closed).
    fn with<T>(f: impl FnOnce(&mut Broker) -> Result<T, i32>) -> Result<T, i32> {
        match BROKER.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            Some(b) => f(b),
            None => Err(c::EACCES),
        }
    }

    /// What one of the script's fds is on the host.
    enum On {
        Null,
        Std(RawFd),
        File(RawFd, bool),
    }

    fn on(fd: i32) -> Result<On, i32> {
        with(|b| {
            Ok(match b.fd(fd).ok_or(c::EBADF)? {
                Fd::Null => On::Null,
                Fd::Std(h) => On::Std(*h),
                Fd::File { fd, write } => On::File(fd.as_raw_fd(), *write),
                Fd::Pipe(fd) => On::File(fd.as_raw_fd(), false),
            })
        })
    }

    /// `r` as an entry's result: a host failure is the script's errno.
    fn host(r: i64) -> i64 {
        if r < 0 {
            fail(last_errno())
        } else {
            r
        }
    }

    /// A buffer of the script's: its arena address, or EFAULT.
    fn buf(p: u64, n: u64) -> Result<u64, i32> {
        if n > WINDOW {
            return Err(EFAULT);
        }
        match arena_ptr(p, n) {
            0 => Err(EFAULT),
            a => Ok(a),
        }
    }

    fn path_then(p: u64, f: impl FnOnce(&mut Broker, &[u8]) -> Result<i64, i32>) -> i64 {
        let r = c_str(p).and_then(|p| with(|b| f(b, &p)));
        r.unwrap_or_else(fail)
    }

    fn open_in(path: u64, flags: i32, mode: u32) -> i32 {
        path_then(path, |b, p| {
            let fd = match b.open(p, flags, mode)? {
                Opened::Null => Fd::Null,
                Opened::File { fd, write } => Fd::File { fd, write },
            };
            Ok(b.add_fd(fd, 0) as i64)
        }) as i32
    }

    // the variadic ones (open, fcntl): the next argument is in the next register, except on
    // Apple's arm64, where variadic arguments go on the stack past the eight argument registers
    #[cfg(not(all(target_vendor = "apple", target_arch = "aarch64")))]
    extern "C" fn sc_open(path: u64, flags: i32, mode: u32) -> i32 {
        open_in(path, flags, mode)
    }
    #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
    extern "C" fn sc_open(path: u64, flags: i32, _2: u64, _3: u64, _4: u64, _5: u64, _6: u64, _7: u64, mode: u32) -> i32 {
        open_in(path, flags, mode)
    }

    extern "C" fn sc_close(fd: i32) -> i32 {
        // (a Std fd's entry goes; the child's own stdout stays open)
        match with(|b| b.take_fd(fd).ok_or(c::EBADF)) {
            Ok(_) => 0,
            Err(e) => fail(e) as i32,
        }
    }

    extern "C" fn sc_read(fd: i32, p: u64, n: u64) -> i64 {
        let r = on(fd).and_then(|o| {
            Ok(match o {
                On::Null => 0,
                On::Std(_) => return Err(c::EBADF),
                On::File(h, _) => host(unsafe { host_read(h, buf(p, n)? as *mut c_void, n as usize) } as i64),
            })
        });
        r.unwrap_or_else(fail)
    }

    extern "C" fn sc_write(fd: i32, p: u64, n: u64) -> i64 {
        let r = on(fd).and_then(|o| {
            Ok(match o {
                On::Null => n as i64,
                On::Std(h) | On::File(h, _) => host(unsafe { host_write(h, buf(p, n)? as *const c_void, n as usize) } as i64),
            })
        });
        r.unwrap_or_else(fail)
    }

    extern "C" fn sc_pread(fd: i32, p: u64, n: u64, off: i64) -> i64 {
        let r = on(fd).and_then(|o| match o {
            On::File(h, _) => Ok(host(unsafe { host_pread(h, buf(p, n)? as *mut c_void, n as usize, off) } as i64)),
            _ => Err(c::ESPIPE),
        });
        r.unwrap_or_else(fail)
    }

    extern "C" fn sc_pwrite(fd: i32, p: u64, n: u64, off: i64) -> i64 {
        let r = on(fd).and_then(|o| match o {
            On::File(h, _) => Ok(host(unsafe { host_pwrite(h, buf(p, n)? as *const u8, n as usize, off) } as i64)),
            _ => Err(c::ESPIPE),
        });
        r.unwrap_or_else(fail)
    }

    extern "C" fn sc_lseek(fd: i32, off: i64, whence: i32) -> i64 {
        let r = on(fd).and_then(|o| match o {
            On::Null => Ok(0),
            On::Std(_) => Err(c::ESPIPE),
            On::File(h, _) => Ok(host(unsafe { host_lseek(h, off, whence) })),
        });
        r.unwrap_or_else(fail)
    }

    /// Writes a stat into the script's `struct stat` at `out`.
    fn put_stat(st: &[u8], out: u64) -> Result<i64, i32> {
        let a = buf(out, st.len() as u64)?;
        unsafe { std::ptr::copy_nonoverlapping(st.as_ptr(), a as *mut u8, st.len()) };
        Ok(0)
    }

    extern "C" fn sc_fstat(fd: i32, out: u64) -> i32 {
        let r = on(fd).and_then(|o| {
            let h = match o {
                On::Null => with(|b| Ok(b.null.as_raw_fd()))?,
                On::Std(h) | On::File(h, _) => h,
            };
            let mut st = [0u8; 160];
            if unsafe { host_fstat(h, st.as_mut_ptr()) } != 0 {
                return Err(last_errno());
            }
            put_stat(&st[..c::STAT_SIZE], out)
        });
        r.unwrap_or_else(fail) as i32
    }

    extern "C" fn sc_stat(path: u64, out: u64) -> i32 {
        path_then(path, |b, p| put_stat(b.stat(p, true)?.bytes(), out)) as i32
    }

    extern "C" fn sc_lstat(path: u64, out: u64) -> i32 {
        path_then(path, |b, p| put_stat(b.stat(p, false)?.bytes(), out)) as i32
    }

    /// An fd call on a file the walk opened (`write`: under a write root, else EPERM).
    fn file_call(fd: i32, write: bool, f: impl FnOnce(RawFd) -> c_int) -> i32 {
        let r = on(fd).and_then(|o| match o {
            On::File(h, w) if w || !write => Ok(host(f(h) as i64)),
            On::File(..) => Err(c::EPERM),
            _ => Err(c::EINVAL),
        });
        r.unwrap_or_else(fail) as i32
    }

    extern "C" fn sc_fsync(fd: i32) -> i32 {
        file_call(fd, false, |h| unsafe { host_fsync(h) })
    }

    extern "C" fn sc_ftruncate(fd: i32, len: i64) -> i32 {
        file_call(fd, true, |h| unsafe { host_ftruncate(h, len) })
    }

    extern "C" fn sc_fchmod(fd: i32, mode: u32) -> i32 {
        file_call(fd, true, |h| unsafe { host_fchmod(h, (mode & 0o7777) as ModeT) })
    }

    extern "C" fn sc_flock(fd: i32, op: i32) -> i32 {
        file_call(fd, false, |h| unsafe { host_flock(h, op) })
    }

    extern "C" fn sc_isatty(fd: i32) -> i32 {
        match on(fd) {
            Ok(On::Std(h)) => unsafe { host_isatty(h) },
            Ok(_) => {
                fail(c::ENOTTY);
                0
            }
            Err(e) => {
                fail(e);
                0
            }
        }
    }

    fn fcntl_in(fd: i32, cmd: i32, arg: u64) -> i32 {
        let r = on(fd).and_then(|o| match cmd {
            c::F_GETFD => Ok(c::FD_CLOEXEC as i64),
            c::F_SETFD => Ok(0),
            c::F_GETFL => match o {
                On::Null => Ok(c::O_RDWR as i64),
                On::Std(h) | On::File(h, _) => Ok(host(unsafe { host_fcntl(h, c::F_GETFL) } as i64)),
            },
            c::F_SETFL => match o {
                On::File(h, _) => Ok(host(unsafe { host_fcntl(h, c::F_SETFL, (arg as c_int) & (c::O_NONBLOCK | c::O_APPEND)) } as i64)),
                _ => Err(c::EINVAL),
            },
            c::F_DUPFD | c::F_DUPFD_CLOEXEC => {
                let copy = match o {
                    On::Null => Fd::Null,
                    On::Std(h) => Fd::Std(h),
                    On::File(h, write) => {
                        let d = unsafe { host_fcntl(h, c::F_DUPFD_CLOEXEC, 0 as c_int) };
                        if d < 0 {
                            return Err(last_errno());
                        }
                        Fd::File { fd: unsafe { OwnedFd::from_raw_fd(d) }, write }
                    }
                };
                with(|b| Ok(b.add_fd(copy, (arg as u32).min(i32::MAX as u32) as usize) as i64))
            }
            _ => Err(c::EINVAL),
        });
        r.unwrap_or_else(fail) as i32
    }
    #[cfg(not(all(target_vendor = "apple", target_arch = "aarch64")))]
    extern "C" fn sc_fcntl(fd: i32, cmd: i32, arg: u64) -> i32 {
        fcntl_in(fd, cmd, arg)
    }
    #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
    extern "C" fn sc_fcntl(fd: i32, cmd: i32, _2: u64, _3: u64, _4: u64, _5: u64, _6: u64, _7: u64, arg: u64) -> i32 {
        fcntl_in(fd, cmd, arg)
    }

    extern "C" fn sc_chmod(path: u64, mode: u32) -> i32 {
        path_then(path, |b, p| b.chmod(p, mode).map(|()| 0)) as i32
    }

    extern "C" fn sc_mkdir(path: u64, mode: u32) -> i32 {
        path_then(path, |b, p| b.mkdir(p, mode).map(|()| 0)) as i32
    }

    extern "C" fn sc_rmdir(path: u64) -> i32 {
        path_then(path, |b, p| b.remove(p, true).map(|()| 0)) as i32
    }

    extern "C" fn sc_unlink(path: u64) -> i32 {
        path_then(path, |b, p| b.remove(p, false).map(|()| 0)) as i32
    }

    extern "C" fn sc_rename(from: u64, to: u64) -> i32 {
        let r = c_str(from).and_then(|a| c_str(to).and_then(|t| with(|b| b.rename(&a, &t))));
        match r {
            Ok(()) => 0,
            Err(e) => fail(e) as i32,
        }
    }

    extern "C" fn sc_readlink(path: u64, out: u64, size: u64) -> i64 {
        path_then(path, |b, p| {
            let t = b.readlink(p)?;
            let n = t.len().min(size as usize);
            let a = buf(out, n as u64)?;
            unsafe { std::ptr::copy_nonoverlapping(t.as_ptr(), a as *mut u8, n) };
            Ok(n as i64)
        })
    }

    /// `s` and a NUL into the script's buffer `out` of `size` bytes, or a new heap block when
    /// `out` is NULL (the script frees it): its address.
    fn put_path(s: &[u8], out: u64, size: u64, too_long: i32) -> Result<u64, i32> {
        let n = s.len() as u64 + 1;
        let a = if out == 0 {
            match sb_malloc(n) {
                0 => return Err(ENOMEM),
                a => a,
            }
        } else {
            if n > size {
                return Err(too_long);
            }
            buf(out, n)?
        };
        unsafe {
            std::ptr::copy_nonoverlapping(s.as_ptr(), a as *mut u8, s.len());
            *((a + s.len() as u64) as *mut u8) = 0;
        }
        Ok(ret_arena(a))
    }

    const ENOMEM: i32 = 12;
    #[cfg(target_vendor = "apple")]
    const PATH_MAX: u64 = 1024;
    #[cfg(not(target_vendor = "apple"))]
    const PATH_MAX: u64 = 4096;

    extern "C" fn sc_realpath(path: u64, out: u64) -> u64 {
        let r = c_str(path).and_then(|p| with(|b| b.realpath(&p))).and_then(|s| put_path(&s, out, PATH_MAX, c::ENAMETOOLONG));
        r.unwrap_or_else(|e| {
            fail(e);
            0
        })
    }

    extern "C" fn sc_getcwd(out: u64, size: u64) -> u64 {
        let r = with(|b| Ok(b.cwd())).and_then(|s| put_path(&s, out, size, c::ERANGE));
        r.unwrap_or_else(|e| {
            fail(e);
            0
        })
    }

    extern "C" fn sc_chdir(path: u64) -> i32 {
        path_then(path, |b, p| b.chdir(p).map(|()| 0)) as i32
    }

    // ---- folder listings: a handle from 1 (never a `DIR*`), the entry written into a block of
    // the script's heap that opendir made and closedir frees, overwritten per entry (C's contract)

    /// struct dirent: its size, and where d_type and d_name lie (d_ino at 0, d_reclen at 16)
    #[cfg(target_vendor = "apple")]
    const DIRENT: (usize, usize, usize) = (1048, 20, 21);
    #[cfg(not(target_vendor = "apple"))]
    const DIRENT: (usize, usize, usize) = (280, 18, 19);

    extern "C" fn sc_opendir(path: u64) -> u64 {
        let r = c_str(path).and_then(|p| {
            with(|b| {
                let d = b.opendir(&p)?;
                let block = sb_malloc(DIRENT.0 as u64);
                if block == 0 {
                    unsafe { host_closedir(d as *mut c_void) };
                    return Err(ENOMEM);
                }
                Ok(b.add_dir(d, block))
            })
        });
        r.unwrap_or_else(|e| {
            fail(e);
            0
        })
    }

    extern "C" fn sc_readdir(h: u64) -> u64 {
        let Some((d, block)) = with(|b| b.dir(h).ok_or(c::EBADF)).map_err(fail).ok() else { return 0 };
        // (the end of the listing leaves errno as it was)
        let e = unsafe { host_readdir(d as *mut c_void) };
        if e.is_null() {
            return 0;
        }
        let (size, ty, name) = DIRENT;
        let n = unsafe { std::ffi::CStr::from_ptr(e.add(name) as *const std::ffi::c_char) }.to_bytes();
        let n = &n[..n.len().min(size - name - 1)];
        let out = unsafe { std::slice::from_raw_parts_mut(block as *mut u8, size) };
        out.fill(0);
        // a constant non-zero inode: the script learns nothing of the host's numbering
        out[0..8].copy_from_slice(&1u64.to_ne_bytes());
        out[16..18].copy_from_slice(&(size as u16).to_ne_bytes());
        #[cfg(target_vendor = "apple")]
        out[18..20].copy_from_slice(&(n.len() as u16).to_ne_bytes());
        out[ty] = unsafe { *e.add(ty) };
        out[name..name + n.len()].copy_from_slice(n);
        ret_arena(block)
    }

    extern "C" fn sc_closedir(h: u64) -> i32 {
        match with(|b| b.take_dir(h).ok_or(c::EBADF)) {
            Ok((d, block)) => {
                sb_free(block);
                unsafe { host_closedir(d as *mut c_void) }
            }
            Err(e) => fail(e) as i32,
        }
    }

    // ---- pipes: real host pipes with both ends in the table (a `$RUSTC` answer writes into one)

    fn pipe_in(fds: u64, flags: i32) -> i32 {
        let r = (|| {
            if flags & !(c::O_CLOEXEC | c::O_NONBLOCK) != 0 {
                return Err(c::EINVAL);
            }
            let out = buf(fds, 8)?;
            let mut p = [0 as c_int; 2];
            if unsafe { host_pipe(p.as_mut_ptr()) } != 0 {
                return Err(last_errno());
            }
            let ends = p.map(|fd| unsafe { OwnedFd::from_raw_fd(fd) });
            for e in &ends {
                // (close-on-exec always: the host's processes never get a script's pipe)
                if unsafe { host_fcntl(e.as_raw_fd(), c::F_SETFD, c::FD_CLOEXEC) } != 0 || (flags & c::O_NONBLOCK != 0 && unsafe { host_fcntl(e.as_raw_fd(), c::F_SETFL, c::O_NONBLOCK) } != 0) {
                    return Err(last_errno());
                }
            }
            let [r, w] = ends;
            let (r, w) = with(|b| Ok((b.add_fd(Fd::Pipe(r), 0), b.add_fd(Fd::Pipe(w), 0))))?;
            unsafe { std::ptr::copy_nonoverlapping([r, w].as_ptr() as *const u8, out as *mut u8, 8) };
            Ok(0)
        })();
        r.unwrap_or_else(fail) as i32
    }

    extern "C" fn sc_pipe(fds: u64) -> i32 {
        pipe_in(fds, 0)
    }

    extern "C" fn sc_pipe2(fds: u64, flags: i32) -> i32 {
        pipe_in(fds, flags)
    }

    /// poll over the script's fds (struct pollfd: fd, events, revents): an fd it does not have
    /// is POLLNVAL, as the kernel says of a closed one.
    extern "C" fn sc_poll(fds: u64, n: NfdsT, timeout: i32) -> i32 {
        const POLLNVAL: i16 = 0x20;
        let r = (|| {
            let n = n as u64;
            let a = buf(fds, n.checked_mul(8).ok_or(EFAULT)?)?;
            let theirs = unsafe { std::slice::from_raw_parts_mut(a as *mut [u8; 8], n as usize) };
            let mut host_fds: Vec<PollFd> = Vec::with_capacity(theirs.len());
            let mut bad = 0;
            for p in theirs.iter() {
                let fd = i32::from_ne_bytes(p[0..4].try_into().unwrap());
                let h = if fd < 0 {
                    -1
                } else {
                    match on(fd) {
                        Ok(On::Null) => with(|b| Ok(b.null.as_raw_fd()))?,
                        Ok(On::Std(h) | On::File(h, _)) => h,
                        Err(_) => {
                            bad += 1;
                            -1
                        }
                    }
                };
                host_fds.push(PollFd { fd: h, events: i16::from_ne_bytes([p[4], p[5]]), revents: 0 });
            }
            let r = unsafe { poll(host_fds.as_mut_ptr(), n as NfdsT, if bad > 0 { 0 } else { timeout }) };
            if r < 0 {
                return Err(last_errno());
            }
            for (p, q) in theirs.iter_mut().zip(host_fds.iter()) {
                let fd = i32::from_ne_bytes(p[0..4].try_into().unwrap());
                let revents = if fd >= 0 && q.fd < 0 { POLLNVAL } else { q.revents };
                p[6..8].copy_from_slice(&revents.to_ne_bytes());
            }
            Ok(r as i64 + bad)
        })();
        r.unwrap_or_else(fail) as i32
    }

    // ---- a virtual file's private copy: an anonymous file the host makes (jit.rs relax_anon_fd)

    extern "C" fn sc_anon_fd() -> i32 {
        let fd = crate::jit::relax_anon_fd();
        if fd < 0 {
            return fail(last_errno()) as i32;
        }
        with(|b| Ok(b.add_fd(Fd::File { fd: unsafe { OwnedFd::from_raw_fd(fd) }, write: true }, 0))).unwrap_or_else(|e| fail(e) as i32)
    }

    // ---- `$RUSTC` questions (tool.rs): only an option-led question of the `rustc` the script was
    // given, answered here; any other spawn is refused (std's NotFound) and nothing starts

    /// A child's end as a host file: the script's fd `fd` (stdin's -1 is the null device, never
    /// the host's stdin; stdout's and stderr's the child's own).
    fn child_end(fd: i64, slot: i32) -> Option<std::fs::File> {
        let h = match fd {
            -1 if slot == 0 => with(|b| Ok(b.null.as_raw_fd())).ok()?,
            -1 => slot,
            fd => match on(i32::try_from(fd).ok()?).ok()? {
                On::Null => with(|b| Ok(b.null.as_raw_fd())).ok()?,
                On::Std(h) | On::File(h, _) => h,
            },
        };
        let owned = unsafe { std::os::fd::BorrowedFd::borrow_raw(h) }.try_clone_to_owned().ok()?;
        Some(std::fs::File::from(owned))
    }

    extern "C" fn sc_tool_spawn(argv: u64, len: u64, stdin: i64, stdout: i64, stderr: i64, plain: i32) -> u64 {
        use crate::tool::TOOL_REFUSED;
        let Ok(a) = buf(argv, len) else { return TOOL_REFUSED };
        let bytes = {
            let _safe = crate::jit::FaultSafe::enter();
            unsafe { std::slice::from_raw_parts(a as *const u8, len as usize) }.to_vec()
        };
        let words = crate::tool::words(&bytes);
        // (said in the withheld report: the program it named)
        let refuse = || {
            let program = words.as_ref().and_then(|w| w.first()).map_or(&b"a program"[..], |p| p.as_slice());
            let _ = with(|b| Ok(b.withhold("run", program, 0)));
            TOOL_REFUSED
        };
        if plain == 0 {
            return refuse();
        }
        let Some(rustc) = relax_base::progenv::given_value(b"RUSTC") else { return refuse() };
        let Some(args) = words.as_deref().and_then(|w| crate::tool::confined_question(w, &rustc)) else { return refuse() };
        let (Some(i), Some(o), Some(e)) = (child_end(stdin, 0), child_end(stdout, 1), child_end(stderr, 2)) else { return TOOL_REFUSED };
        // (0 would tell std to start a process: an answer that cannot start is refused)
        match crate::tool::start(args, i, o, e) {
            0 => TOOL_REFUSED,
            h => h,
        }
    }

    extern "C" fn sc_tool_wait(h: u64, block: i32) -> i64 {
        crate::tool::relax_tool_wait(h, block)
    }

    extern "C" fn sc_tool_release(h: u64) {
        crate::tool::relax_tool_release(h)
    }

    // ---- the VFS: the script's store folders only, as arena copies; any other path is "not
    // virtual" (0), and std goes on to the walk

    fn store_arg(p: u64, n: u64) -> Option<String> {
        let a = buf(p, n).ok()?;
        let bytes = {
            let _safe = crate::jit::FaultSafe::enter();
            unsafe { std::slice::from_raw_parts(a as *const u8, n as usize) }.to_vec()
        };
        with(|b| Ok(b.store_path(&bytes))).ok().flatten()
    }

    extern "C" fn sc_vfs_stat(p: u64, n: u64, size: u64, mtime_ns: u64) -> i32 {
        let Some(path) = store_arg(p, n) else { return 0 };
        match crate::vfs::store_kind(&path) {
            None => 0,
            Some(crate::vfs::StoreKind::File(len, t)) => {
                if !(put_u64(size, len) && put_u64(mtime_ns, t)) {
                    return -1;
                }
                1
            }
            Some(crate::vfs::StoreKind::Dir) => 2,
            Some(crate::vfs::StoreKind::Missing) => -1,
        }
    }

    /// Hands `data` to the script (`give`) through its out pointers: a constant id, 0 when it
    /// cannot (std then reads NotFound).
    fn hand(data: &[u8], out: u64, out_len: u64) -> u64 {
        let q = ret_arena(give(data));
        if q == 0 || !(put_u64(out, q) && put_u64(out_len, data.len() as u64)) {
            return 0;
        }
        1
    }

    extern "C" fn sc_vfs_read(p: u64, n: u64, out: u64, out_len: u64) -> u64 {
        match store_arg(p, n).and_then(|path| crate::vfs::store_read(&path)) {
            Some(Ok(data)) => hand(&data, out, out_len),
            _ => 0,
        }
    }

    extern "C" fn sc_vfs_list(p: u64, n: u64, out: u64, out_len: u64) -> u64 {
        match store_arg(p, n).and_then(|path| crate::vfs::store_list(&path)) {
            Some(Ok(v)) => hand(v.iter().map(|(name, d)| format!("{} {name}\n", if *d { 'd' } else { 'f' })).collect::<String>().as_bytes(), out, out_len),
            _ => 0,
        }
    }

    /// (the broker's copy went with `give`: nothing of the host's is held)
    extern "C" fn sc_vfs_release(_id: u64) {}

    /// A write into a virtual folder: none (a confined script's virtual files are read-only).
    extern "C" fn sc_vfs_edit(_p: u64, _n: u64, _mode: u32, _out: u64, _out_len: u64) -> u64 {
        0
    }

    /// The mount table a child would inherit: none (the script starts no process).
    extern "C" fn sc_vfs_mounts(_out: u64, _out_len: u64) -> u64 {
        0
    }

    /// What the program looked at, for a live session: nothing of a confined script's is kept.
    extern "C" fn sc_vfs_note(_p: u64, _n: u64, _kind: u32) {}
}

// ------------------------------------------------------------------ the run

/// Runs the program of a confined build script (`main_def` of crate `root`) as twins in the
/// script's arena, reaching the folders of `roots` only; its exit code.
pub unsafe fn run_main(up: *mut Unit, root: u32, main_def: DefId, roots: &Roots) -> i32 {
    let package = relax_base::progenv::given_value(b"CARGO_PKG_NAME").map_or_else(|| "a package".to_string(), |v| String::from_utf8_lossy(&v).into_owned());
    let name = format!("the confined build script of {package}");
    if cfg!(windows) {
        eprintln!("relax: {name} cannot run here: confined build scripts need arena windows, which Windows does not have yet (it runs under bare `relax`)");
        return 1;
    }
    // a constructor table would run before main as the platform's start code runs it: the
    // confined run calls none
    match crate::sections::constructor_paths(up, root) {
        Ok(v) if v.is_empty() => {}
        Ok(v) => {
            eprintln!("relax: {name} links constructors ({}), which a confined run does not call: it runs under bare `relax`", v.join(", "));
            return 1;
        }
        Err(e) => {
            eprintln!("relax: {e}");
            return 1;
        }
    }
    #[cfg(unix)]
    match super::broker::Broker::new(roots) {
        Ok(b) => *files::BROKER.lock().unwrap_or_else(|e| e.into_inner()) = Some(b),
        Err(e) => {
            eprintln!("relax: {name} cannot run confined: {e}");
            return 1;
        }
    }
    #[cfg(not(unix))]
    let _ = roots;
    let setup = confine_unit(up, SCRIPT_DOMAIN, &[]).and_then(|()| new_arena(up, SCRIPT_MODULE, &name, (HEAP_END - HEAP_BASE) as u64, 0));
    let arena = match setup {
        Ok(a) => a,
        Err(e) => {
            eprintln!("relax: {name} cannot run confined: {e}");
            return 1;
        }
    };
    // main's twin, or std's start of a main that returns a value with main's twin as its fn value
    let target = crate::owner::call(crate::owner::Class::Program, || -> Result<(u64, u64), String> {
        let u = &mut *up;
        let main_id = u.fn_id(FnKey::Inst(main_def, Vec::new()));
        let (t, arg) = match crate::run::lang_start_slot(u, main_def)? {
            None => (twin(up, main_id), 0),
            Some(start) => (twin(up, start), fn_value(up, main_id)?),
        };
        Ok(((*up).entry(t), arg))
    });
    let (entry, arg) = match target {
        Ok(t) => t,
        Err(e) => {
            eprintln!("relax: {e}");
            return 1;
        }
    };
    let _program = crate::os::program_thread();
    let depth = entry_depth();
    let saved = save();
    if enter_arena(&arena).is_err() {
        eprintln!("relax: {name}: its arena cannot be entered");
        return 1;
    }
    ARENA_BASE.store(arena.base, std::sync::atomic::Ordering::SeqCst);
    let code = match crate::jit::call_jit(up, entry, arg) {
        Ok(_) => 0,
        Err(p) => {
            // (the frames it unwound are gone: the arena is entered again for its exit handlers)
            entry_recover(depth);
            restore(saved);
            eprintln!("relax: main {}: {}", p.kind, p.message);
            if enter_arena(&arena).is_err() {
                return 101;
            }
            101
        }
    };
    run_exit_handlers();
    leave();
    say_withheld(code);
    code
}

/// A fault while a confined script runs: (the script's, what to say). The script's when it hit
/// its arena's window or guards from its own code; anything else is Relax's.
pub(crate) fn fault_note(u: &Unit, info: &crate::jit::PanicInfo) -> Option<(bool, String)> {
    let base = ARENA_BASE.load(std::sync::atomic::Ordering::SeqCst);
    if base == 0 {
        return None;
    }
    let in_window = info.fault_addr.wrapping_sub(base - GUARD) < WINDOW + 2 * GUARD;
    let twin_pc = info.frames.first().is_some_and(|pc| {
        let slot = u.code.read().unwrap_or_else(|e| e.into_inner()).slot_at(*pc);
        slot.is_some_and(|s| (s as usize) < u.fns.len() && matches!(u.fns[s as usize].key, FnKey::Sandbox(_)))
    });
    Some(if in_window && twin_pc {
        let off = info.fault_addr.wrapping_sub(base);
        (true, format!("relax: the confined build script faulted inside its arena ({} at {}): it failed, as a script that crashes fails", info.kind, describe_window_offset(off)))
    } else {
        (false, format!("relax: a fault outside the confined build script's arena ({} at {:#x}): a Relax bug, not the script's (report follows)", info.kind, info.fault_addr))
    })
}

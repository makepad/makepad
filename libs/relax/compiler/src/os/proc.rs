//! Process-level services: foreign symbols and libraries, power state, the process's own
//! arguments, private cache files.
//!
//! - `find(name)`: a symbol in any image loaded into the process (`name` NUL-terminated);
//!   `find_system(name)`: the C/math libraries the host may not have loaded (Linux libm/libc).
//! - `load_library(name, framework)`: a program's `#[link(name = ..)]` library (false: not found);
//!   `load_library_versioned(name)`: Linux's lib<name>.so.N where lib<name>.so is a linker script.
//! - `lookup(handle, name)`: the system's own lookup in a library handle (the program's dlsym).
//! - `exit(code)`: ends the process once Relax is done with the program (see the Windows half).
//! - `on_battery()`.
//! - `symbol_at(pc)`: the nearest exported symbol of the image holding `pc` (dladdr's view).
//! - `args_bytes()`: the process arguments as bytes (`set_args`: an embedder names them).
//! - `create_private(path)`: a new file only this user can read (O_EXCL); `ensure_private_dir`:
//!   a directory that is safe to load code from (owned by this user, not writable by others).

pub use imp::*;

/// The status an embedded run (the Android host) ended with when it failed; 0: it has not.
pub static EMBEDDED_FAILURE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
/// Whether a host owns this process (it named its library): a failed run then must not exit().
pub static EMBEDDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

static ARGS: std::sync::OnceLock<Vec<Vec<u8>>> = std::sync::OnceLock::new();

/// The arguments Relax runs under, when the process's own are not what it was started with: a
/// host library started from a loader (Android: bionic hands a cdylib's std no argv, the APK's
/// executable passes its own).
pub fn set_args(args: Vec<Vec<u8>>) {
    // process state: rebuilt in the kept heap (the settle swap destroys the compiler arena's generation)
    let args = crate::zone_alloc::kept_with(|| args.clone());
    let _ = ARGS.set(args);
}

/// The process arguments as bytes (`set_args` first, else the process's).
pub fn args_bytes() -> Vec<Vec<u8>> {
    match ARGS.get() {
        Some(a) => a.clone(),
        None => os_args_bytes(),
    }
}

/// Keeps the calling thread's last OS error (errno / GetLastError) across runtime work done in
/// the middle of program code (lazy compiles, tier-up glue, thread-local block allocation).
pub struct KeepLastError(i32);

impl KeepLastError {
    pub fn new() -> KeepLastError {
        KeepLastError(last_error())
    }
}

impl Drop for KeepLastError {
    fn drop(&mut self) {
        set_last_error(self.0);
    }
}

/// The exported symbol nearest at or below `pc` in the loaded image that contains it: (name,
/// symbol address, image path). Unix: dladdr. Windows: the module containing `pc`
/// (GetModuleHandleExW from an address) and the nearest entry of its PE export table, which is
/// what dladdr reports for a shared object (exported names only).
pub fn symbol_at(pc: u64) -> Option<(String, u64, String)> {
    symbol_at_imp(pc)
}

#[cfg(unix)]
fn symbol_at_imp(pc: u64) -> Option<(String, u64, String)> {
    #[repr(C)]
    struct DlInfo {
        fname: *const std::ffi::c_char,
        fbase: *mut u8,
        sname: *const std::ffi::c_char,
        saddr: *mut u8,
    }
    extern "C" {
        fn dladdr(addr: *const u8, info: *mut DlInfo) -> i32;
    }
    let mut info = DlInfo { fname: std::ptr::null(), fbase: std::ptr::null_mut(), sname: std::ptr::null(), saddr: std::ptr::null_mut() };
    if unsafe { dladdr(pc as *const u8, &mut info) } == 0 || info.sname.is_null() {
        return None;
    }
    let name = unsafe { std::ffi::CStr::from_ptr(info.sname) }.to_string_lossy().into_owned();
    let file = if info.fname.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(info.fname) }.to_string_lossy().into_owned() };
    Some((name, info.saddr as u64, file))
}

#[cfg(windows)]
fn symbol_at_imp(pc: u64) -> Option<(String, u64, String)> {
    use crate::os::win32::*;
    // FROM_ADDRESS | UNCHANGED_REFCOUNT
    let mut module: HANDLE = 0;
    if unsafe { GetModuleHandleExW(0x4 | 0x2, pc as *const u16, &mut module) } == 0 || module == 0 {
        return None;
    }
    let base = module as usize as *const u8;
    let mut path = [0u16; 1024];
    let n = unsafe { GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) } as usize;
    let file = String::from_utf16_lossy(&path[..n.min(path.len())]);
    // SAFETY: a loaded module's headers and export directory are mapped readable for its life.
    unsafe {
        let rd32 = |off: usize| std::ptr::read_unaligned(base.add(off) as *const u32) as usize;
        let rd16 = |off: usize| std::ptr::read_unaligned(base.add(off) as *const u16) as usize;
        if rd16(0) != 0x5a4d {
            return None;
        }
        let pe = rd32(0x3c);
        if rd32(pe) != 0x4550 || rd16(pe + 24) != 0x20b {
            return None; // not PE32+
        }
        // optional header (PE32+) at pe+24; data directory 0 (exports) at +112
        let (exp, exp_size) = (rd32(pe + 24 + 112), rd32(pe + 24 + 116));
        if exp == 0 {
            return None;
        }
        let n_funcs = rd32(exp + 20);
        let n_names = rd32(exp + 24);
        let (funcs, names, ords) = (rd32(exp + 28), rd32(exp + 32), rd32(exp + 36));
        let rva = (pc as usize).wrapping_sub(base as usize);
        let mut best: Option<(usize, usize)> = None; // (function rva, name index)
        for i in 0..n_names {
            let ord = rd16(ords + 2 * i);
            if ord >= n_funcs {
                continue;
            }
            let f = rd32(funcs + 4 * ord);
            // forwarders point into the export directory, not at code
            if f > rva || (f >= exp && f < exp + exp_size) {
                continue;
            }
            if best.is_none_or(|(b, _)| f > b) {
                best = Some((f, i));
            }
        }
        let (f, i) = best?;
        let name = std::ffi::CStr::from_ptr(base.add(rd32(names + 4 * i)) as *const std::ffi::c_char).to_string_lossy().into_owned();
        Some((name, base as u64 + f as u64, file))
    }
}

#[cfg(unix)]
mod unix {
    use std::path::Path;

    extern "C" {
        pub fn dlopen(name: *const u8, flags: i32) -> *mut u8;
        pub fn dlsym(handle: *mut u8, name: *const u8) -> *mut u8;
    }

    pub unsafe fn lookup(handle: *mut u8, name: *const u8) -> *mut u8 {
        dlsym(handle, name)
    }

    /// exit(): the main thread's pthread key destructors do not run (the program's thread locals
    /// are left as they are, as in a rustc build).
    pub fn exit(code: i32) -> ! {
        // An embedded run (the Android host: its library handle is named) failed: exit() would run the
        // process's atexit/static destructors on this thread while the host's UI thread still locks
        // the mutexes they destroy (FORTIFY abort, no message). The host owns the process: the failure
        // is already reported through its status, so this thread just stays out of the way.
        if code != 0 && super::EMBEDDED.load(std::sync::atomic::Ordering::Acquire) {
            use std::io::Write;
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            super::EMBEDDED_FAILURE.store(code, std::sync::atomic::Ordering::Release);
            loop { std::thread::park(); }
        }
        std::process::exit(code)
    }

    pub fn last_error() -> i32 {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    }

    pub fn set_last_error(e: i32) {
        extern "C" {
            #[cfg(target_os = "linux")]
            fn __errno_location() -> *mut i32;
            #[cfg(target_os = "android")]
            #[link_name = "__errno"]
            fn __errno_location() -> *mut i32;
            #[cfg(target_os = "macos")]
            #[link_name = "__error"]
            fn __errno_location() -> *mut i32;
        }
        unsafe {
            *__errno_location() = e;
        }
    }

    pub fn os_args_bytes() -> Vec<Vec<u8>> {
        use std::os::unix::ffi::OsStringExt;
        std::env::args_os().map(|a| a.into_vec()).collect()
    }

    pub fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
    }

    pub fn ensure_private_dir(dir: &Path) -> Result<(), String> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        if !dir.exists() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| format!("cannot create {}: {}", dir.display(), e))?;
        }
        let m = std::fs::metadata(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        if !m.is_dir() {
            return Err(format!("{} is not a directory", dir.display()));
        }
        extern "C" {
            fn getuid() -> u32;
        }
        if m.uid() != unsafe { getuid() } {
            return Err(format!("{} belongs to another user", dir.display()));
        }
        if m.permissions().mode() & 0o022 != 0 {
            return Err(format!("{} is writable by group or others", dir.display()));
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod imp {
    pub use super::unix::{os_args_bytes, create_private, ensure_private_dir, exit, last_error, lookup, set_last_error};
    use super::unix::{dlopen, dlsym};

    pub unsafe fn find(name: *const u8) -> *mut u8 {
        dlsym(std::ptr::null_mut(), name)
    }

    /// The C math library is not necessarily loaded into the host.
    pub unsafe fn find_system(name: *const u8) -> *mut u8 {
        for lib in ["libm.so.6\0", "libc.so.6\0"] {
            let h = dlopen(lib.as_ptr(), 2);
            if !h.is_null() {
                let p = dlsym(h, name);
                if !p.is_null() {
                    return p;
                }
            }
        }
        std::ptr::null_mut()
    }

    pub fn load_library(name: &str, framework: bool) -> bool {
        let path = if framework { format!("/System/Library/Frameworks/{}.framework/{}\0", name, name) } else { format!("lib{}.so\0", name) };
        // RTLD_NOW | RTLD_GLOBAL
        unsafe { !dlopen(path.as_ptr(), 2 | 0x100).is_null() }
    }

    /// glibc's lib<name>.so is often a linker script (libm.so): the versioned file instead.
    pub fn load_library_versioned(name: &str) -> bool {
        match versioned_so(name) {
            Some(v) => unsafe { !dlopen(format!("{}\0", v).as_ptr(), 2 | 0x100).is_null() },
            None => false,
        }
    }

    fn versioned_so(name: &str) -> Option<String> {
        let prefix = format!("lib{}.so.", name);
        let mut best: Option<String> = None;
        for dir in ["/usr/lib", "/usr/lib64", "/lib/x86_64-linux-gnu", "/usr/lib/x86_64-linux-gnu", "/lib64"] {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let f = e.file_name().to_string_lossy().into_owned();
                    if f.starts_with(&prefix) && best.as_ref().map_or(true, |b| f.len() < b.len()) {
                        best = Some(f);
                    }
                }
            }
            if best.is_some() {
                break;
            }
        }
        best
    }


    /// A battery that is discharging and no mains supply online (/sys/class/power_supply).
    pub fn on_battery() -> bool {
        let rd = |p: std::path::PathBuf| std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default();
        let mut mains_online = false;
        let mut discharging = false;
        if let Ok(dir) = std::fs::read_dir("/sys/class/power_supply") {
            for e in dir.flatten() {
                let p = e.path();
                match rd(p.join("type")).as_str() {
                    "Mains" | "USB" => mains_online |= rd(p.join("online")) == "1",
                    "Battery" => discharging |= rd(p.join("status")) == "Discharging",
                    _ => {}
                }
            }
        }
        discharging && !mains_online
    }
}

#[cfg(target_os = "android")]
mod imp {
    pub use super::unix::{os_args_bytes, create_private, ensure_private_dir, exit, last_error, set_last_error};
    use super::unix::{dlopen, dlsym};

    /// The library Relax itself is (libmakepad.so): System.loadLibrary loads it into a scope of its
    /// own, so the process-wide lookup below never sees its exported symbols (the C functions the
    /// compiled program calls by name: Relax std's file system, the runtime's hooks).
    static HOST_LIBRARY: std::sync::atomic::AtomicPtr<u8> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

    /// Names the loaded host library's handle (dlopen of its soname with RTLD_NOLOAD).
    pub fn set_host_library(handle: *mut u8) {
        HOST_LIBRARY.store(handle, std::sync::atomic::Ordering::Release);
        super::EMBEDDED.store(!handle.is_null(), std::sync::atomic::Ordering::Release);
    }

    /// Handles of the libraries `load_library` opened.
    static LOADED: relax_session::sync::Mutex<Vec<usize>> = relax_session::sync::Mutex::new(Vec::new());

    /// The stub pool of the host library: `count` stubs of 16 bytes from `base`, each jumping to
    /// the address in its word of `table` (android_host/src/trampolines.rs).
    struct Pool {
        base: u64,
        table: *mut u64,
        count: usize,
        used: std::collections::HashMap<u64, u64>,
    }
    unsafe impl Send for Pool {}
    static POOL: relax_session::sync::Mutex<Option<Pool>> = relax_session::sync::Mutex::new(None);

    pub fn set_trampolines(base: u64, table: *mut u64, count: usize) {
        // process state: kept heap (the settle swap destroys the compiler zone)
        let _k = crate::zone_alloc::kept();
        *POOL.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pool { base, table, count, used: std::collections::HashMap::new() });
    }

    /// An address in the host library that jumps to `target`, one per target. System libraries
    /// (libaaudio ...) check an indirect call's target against the libraries they know (CFI) and
    /// crash on JIT memory: C code is given these instead of a thunk of the JIT.
    pub fn trampoline(target: u64) -> u64 {
        let mut guard = POOL.lock().unwrap_or_else(|e| e.into_inner());
        let Some(pool) = guard.as_mut() else { return target };
        if let Some(&a) = pool.used.get(&target) {
            return a;
        }
        // called while compiling (c_callable_hook): the map is process state, so its growth goes
        // to the kept heap -- in the compiler zone it was gone after the settle swap and the
        // first new C callback (a touch handler on the phone) faulted
        let _k = crate::zone_alloc::kept();
        let i = pool.used.len();
        assert!(i < pool.count, "the host library's {} C-callable stubs are all in use", pool.count);
        unsafe { *pool.table.add(i) = target };
        let a = pool.base + 16 * i as u64;
        pool.used.insert(target, a);
        a
    }

    /// dlsym for the program: RTLD_DEFAULT (a null handle) does not see the host library from inside an
    /// app's linker namespace, so the host library and the libraries the program loaded are asked too
    /// (the program's `dlsym(RTLD_DEFAULT, "makepad_run_first_present")` finds the host's hooks).
    pub unsafe fn lookup(handle: *mut u8, name: *const u8) -> *mut u8 {
        let p = dlsym(handle, name);
        if p.is_null() && handle.is_null() { find(name) } else { p }
    }

    // NDK r28 dlfcn.h:169 RTLD_DEFAULT = 0, :191 RTLD_NOW = 2,
    // :204 RTLD_GLOBAL = 0x100 on LP64.
    pub unsafe fn find(name: *const u8) -> *mut u8 {
        let p = dlsym(std::ptr::null_mut(), name);
        if !p.is_null() {
            return p;
        }
        let host = HOST_LIBRARY.load(std::sync::atomic::Ordering::Acquire);
        if !host.is_null() {
            let p = dlsym(host, name);
            if !p.is_null() {
                return p;
            }
        }
        // RTLD_GLOBAL does not put a library into the lookup of RTLD_DEFAULT from inside an app's
        // linker namespace: ask the libraries the program's #[link] blocks loaded
        for &h in LOADED.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            let p = dlsym(h as *mut u8, name);
            if !p.is_null() {
                return p;
            }
        }
        std::ptr::null_mut()
    }

    /// The C math library is not necessarily loaded into the host.
    pub unsafe fn find_system(name: *const u8) -> *mut u8 {
        for lib in ["libc.so\0", "libm.so\0", "libdl.so\0"] {
            let h = dlopen(lib.as_ptr(), 2);
            if !h.is_null() {
                let p = dlsym(h, name);
                if !p.is_null() {
                    return p;
                }
            }
        }
        std::ptr::null_mut()
    }

    pub fn load_library(name: &str, framework: bool) -> bool {
        let path = if framework { format!("/System/Library/Frameworks/{}.framework/{}\0", name, name) } else { format!("lib{}.so\0", name) };
        // RTLD_NOW | RTLD_GLOBAL
        unsafe {
            let h = dlopen(path.as_ptr(), 2 | 0x100);
            if !h.is_null() {
                let _k = crate::zone_alloc::kept();
                let mut l = LOADED.lock().unwrap_or_else(|e| e.into_inner());
                if !l.contains(&(h as usize)) {
                    l.push(h as usize);
                }
                return true;
            }
            extern "C" {
                fn dlerror() -> *const std::ffi::c_char;
            }
            let e = dlerror();
            if !e.is_null() {
                eprintln!("makepad: cannot load {}: {}", path.trim_end_matches('\0'), std::ffi::CStr::from_ptr(e).to_string_lossy());
            }
            false
        }
    }

    /// Bionic public libraries use plain sonames; never scan desktop library paths.
    pub fn load_library_versioned(_name: &str) -> bool { false }

    /// A battery that is discharging and no mains supply online (/sys/class/power_supply).
    pub fn on_battery() -> bool {
        let rd = |p: std::path::PathBuf| std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default();
        let mut mains_online = false;
        let mut discharging = false;
        if let Ok(dir) = std::fs::read_dir("/sys/class/power_supply") {
            for e in dir.flatten() {
                let p = e.path();
                match rd(p.join("type")).as_str() {
                    "Mains" | "USB" => mains_online |= rd(p.join("online")) == "1",
                    "Battery" => discharging |= rd(p.join("status")) == "Discharging",
                    _ => {}
                }
            }
        }
        discharging && !mains_online
    }
}

#[cfg(target_os = "macos")]
mod imp {
    pub use super::unix::{os_args_bytes, create_private, ensure_private_dir, exit, last_error, lookup, set_last_error};
    use super::unix::{dlopen, dlsym};

    /// Symbol in any loaded image (RTLD_DEFAULT). libm lives in libSystem, always loaded.
    pub unsafe fn find(name: *const u8) -> *mut u8 {
        dlsym(-2isize as *mut u8, name)
    }

    pub unsafe fn find_system(_name: *const u8) -> *mut u8 {
        std::ptr::null_mut()
    }

    pub fn load_library(name: &str, framework: bool) -> bool {
        let path = if framework { format!("/System/Library/Frameworks/{}.framework/{}\0", name, name) } else { format!("lib{}.dylib\0", name) };
        // RTLD_NOW | RTLD_GLOBAL
        unsafe { !dlopen(path.as_ptr(), 2 | 8).is_null() }
    }

    pub fn load_library_versioned(_name: &str) -> bool {
        false
    }

    pub fn on_battery() -> bool {
        #[link(name = "IOKit", kind = "framework")]
        extern "C" {
            /// IOKit/ps/IOPowerSources.h: kIOPSTimeRemainingUnlimited (-2.0) while on AC power,
            /// kIOPSTimeRemainingUnknown (-1.0) or seconds left while on battery
            fn IOPSGetTimeRemainingEstimate() -> f64;
        }
        let t = unsafe { IOPSGetTimeRemainingEstimate() };
        t != -2.0
    }
}

#[cfg(windows)]
mod imp {
    use crate::os::win32::*;
    use std::path::Path;

    /// A symbol in the Universal C Runtime first (the C library a rustc build of the program uses
    /// on Windows: math results match it bit for bit), then in any module loaded into the process:
    /// GetProcAddress over the module list, the counterpart of dlsym's default namespace.
    pub unsafe fn find(name: *const u8) -> *mut u8 {
        static UCRT: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(-1);
        let mut u = UCRT.load(std::sync::atomic::Ordering::Relaxed);
        if u == -1 {
            u = LoadLibraryA(b"ucrtbase.dll\0".as_ptr());
            UCRT.store(u, std::sync::atomic::Ordering::Relaxed);
        }
        if u != 0 {
            let p = GetProcAddress(u, name);
            if !p.is_null() {
                return p;
            }
        }
        let mut mods = [0 as HANDLE; 1024];
        let mut needed = 0u32;
        if K32EnumProcessModules(GetCurrentProcess(), mods.as_mut_ptr(), std::mem::size_of_val(&mods) as u32, &mut needed) == 0 {
            return std::ptr::null_mut();
        }
        let n = (needed as usize / std::mem::size_of::<HANDLE>()).min(mods.len());
        for m in &mods[..n] {
            let p = GetProcAddress(*m, name);
            if !p.is_null() {
                return p;
            }
        }
        std::ptr::null_mut()
    }

    /// The C runtime and math functions where no loaded module exports them yet.
    pub unsafe fn find_system(name: *const u8) -> *mut u8 {
        for lib in ["msvcrt.dll\0", "kernelbase.dll\0", "ntdll.dll\0"] {
            let h = LoadLibraryA(lib.as_ptr());
            if h != 0 {
                let p = GetProcAddress(h, name);
                if !p.is_null() {
                    return p;
                }
            }
        }
        std::ptr::null_mut()
    }

    /// `#[link(name = "user32")]` / raw-dylib `name = "DINPUT8"`: the DLL of that name.
    pub fn load_library(name: &str, _framework: bool) -> bool {
        let file = if name.to_ascii_lowercase().ends_with(".dll") { format!("{}\0", name) } else { format!("{}.dll\0", name) };
        unsafe { LoadLibraryA(file.as_ptr()) != 0 }
    }

    pub fn load_library_versioned(_name: &str) -> bool {
        false
    }

    pub unsafe fn lookup(handle: *mut u8, name: *const u8) -> *mut u8 {
        GetProcAddress(handle as HANDLE, name)
    }

    /// ExitProcess would run the fiber-local destructors of the exiting thread, and the program's
    /// thread locals (FlsAlloc callbacks in JIT code) would run after Relax dropped the unit:
    /// flush our output and end the process without them, as unix's exit() does not run the main
    /// thread's key destructors either.
    pub fn exit(code: i32) -> ! {
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        unsafe {
            TerminateProcess(GetCurrentProcess(), code as u32);
        }
        std::process::exit(code)
    }

    pub fn last_error() -> i32 {
        unsafe { GetLastError() as i32 }
    }

    pub fn set_last_error(e: i32) {
        unsafe { SetLastError(e as u32) }
    }

    pub fn on_battery() -> bool {
        let mut st = SystemPowerStatus::default();
        unsafe { GetSystemPowerStatus(&mut st) != 0 && st.ac_line_status == 0 }
    }

    /// The arguments as UTF-8 (WTF-16 decoded lossily: the program's C argv view).
    pub fn os_args_bytes() -> Vec<Vec<u8>> {
        std::env::args_os().map(|a| a.to_string_lossy().into_owned().into_bytes()).collect()
    }

    /// Cache files are created inside folders ensure_private_dir gave a protected owner-only
    /// DACL (the workspace's <target>/relax, dirs.rs); they inherit it.
    pub fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
        std::fs::OpenOptions::new().write(true).create_new(true).open(path)
    }

    pub fn ensure_private_dir(dir: &Path) -> Result<(), String> {
        private_dir::ensure(dir)
    }

    mod private_dir {
        use super::*;
        use std::os::windows::ffi::OsStrExt;
        use std::path::{Component, PathBuf, Prefix};
        use std::ptr::{null_mut, read_unaligned};

        const OWNER_SECURITY_INFORMATION: u32 = 1;
        const DACL_SECURITY_INFORMATION: u32 = 4;
        const PROTECTED_DACL_SECURITY_INFORMATION: u32 = 0x8000_0000;
        const SE_FILE_OBJECT: u32 = 1;
        // FILE_WRITE_DATA/FILE_ADD_FILE, FILE_APPEND_DATA/FILE_ADD_SUBDIRECTORY,
        // FILE_DELETE_CHILD, DELETE, WRITE_DAC, WRITE_OWNER, GENERIC_WRITE, GENERIC_ALL.
        const WRITE_RIGHTS: u32 = 0x2 | 0x4 | 0x40 | 0x10000 | 0x40000 | 0x80000 | 0x4000_0000 | 0x1000_0000;

        struct Token(HANDLE);
        impl Drop for Token {
            fn drop(&mut self) { unsafe { CloseHandle(self.0); } }
        }
        struct Descriptor(*mut u8);
        impl Drop for Descriptor {
            fn drop(&mut self) { unsafe { LocalFree(self.0); } }
        }

        fn wide(path: &Path) -> std::io::Result<Vec<u16>> {
            let mut s: Vec<u16> = path.as_os_str().encode_wide().collect();
            if s.contains(&0) {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "NUL in directory path"));
            }
            s.push(0);
            Ok(s)
        }

        // Resolve the existing ancestor as well as an existing directory, so junctions cannot
        // hide a shared location even before the final directory has been created.
        fn resolved(path: &Path) -> std::io::Result<PathBuf> {
            match std::fs::canonicalize(path) {
                Ok(p) => Ok(p),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).ok_or(e)?;
                    Ok(resolved(parent)?.join(path.file_name().ok_or_else(||
                        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid directory path"))?))
                }
                Err(e) => Err(e),
            }
        }

        // Accept only drive paths; UNC, verbatim UNC and device namespaces fail closed.
        fn local_path(path: &Path) -> std::io::Result<PathBuf> {
            if !path.is_absolute() {
                return Err(std::io::Error::other("directory has no absolute local path"));
            }
            let mut parts = path.components();
            let drive = match parts.next() {
                Some(Component::Prefix(p)) => match p.kind() {
                    Prefix::Disk(d) | Prefix::VerbatimDisk(d) => d,
                    _ => return Err(std::io::Error::other("shared or network directory")),
                },
                _ => return Err(std::io::Error::other("directory has no local drive")),
            };
            let root = [drive as u16, b':' as u16, b'\\' as u16, 0];
            if !matches!(unsafe { GetDriveTypeW(root.as_ptr()) }, 2 | 3 | 5 | 6) {
                return Err(std::io::Error::other("shared or network directory"));
            }
            let mut result = PathBuf::from(format!("{}:\\", drive as char));
            for part in parts {
                match part {
                    Component::Normal(p) => result.push(p),
                    Component::ParentDir => { result.pop(); }
                    Component::RootDir | Component::CurDir => {}
                    _ => return Err(std::io::Error::other("invalid directory path")),
                }
            }
            Ok(result)
        }

        fn under(path: &Path, root: &Path) -> std::io::Result<bool> {
            let path = wide(path)?;
            let root = wide(root)?;
            let n = root.len() - 1;
            Ok(path.len() >= root.len()
                && unsafe { CompareStringOrdinal(path.as_ptr(), n as i32, root.as_ptr(), n as i32, 1) } == 2
                && (path[n] == 0 || path[n] == b'\\' as u16 || root[n - 1] == b'\\' as u16))
        }

        fn check_location(path: &Path) -> std::io::Result<PathBuf> {
            let lexical = local_path(path)?;
            let target = local_path(&resolved(path)?)?;
            for name in ["ProgramFiles", "ProgramFiles(x86)", "ProgramData", "PUBLIC"] {
                if let Some(root) = std::env::var_os(name).filter(|s| !s.is_empty()) {
                    let root = PathBuf::from(root);
                    let plain = local_path(&root)?;
                    let actual = local_path(&resolved(&root)?)?;
                    if under(&lexical, &plain)? || under(&target, &plain)? || under(&target, &actual)? {
                        return Err(std::io::Error::other("shared or network directory"));
                    }
                }
            }
            Ok(target)
        }

        pub(super) fn ensure(dir: &Path) -> Result<(), String> {
            let error = |e| format!("{}: {}", dir.display(), e);
            let absolute = if dir.is_absolute() { dir.to_path_buf() } else {
                std::env::current_dir().map_err(error)?.join(dir)
            };
            let target = check_location(&absolute).map_err(error)?;
            unsafe { ensure_acl(&target, dir) }
        }

        unsafe fn ensure_acl(target: &Path, display: &Path) -> Result<(), String> {
            let error = || format!("{}: {}", display.display(), std::io::Error::last_os_error());
            let writable = || format!("{} is writable by group or others", display.display());
            let mut token = Token(0);
            if OpenProcessToken(GetCurrentProcess(), 0x8, &mut token.0) == 0 { return Err(error()); } // TOKEN_QUERY
            let mut needed = 0;
            GetTokenInformation(token.0, 1, null_mut(), 0, &mut needed); // TokenUser
            if needed == 0 { return Err(error()); }
            // TOKEN_USER starts with SID_AND_ATTRIBUTES. Keep its backing store pointer-aligned.
            let mut user_info = vec![0usize; (needed as usize + std::mem::size_of::<usize>() - 1) / std::mem::size_of::<usize>()];
            if GetTokenInformation(token.0, 1, user_info.as_mut_ptr().cast(), needed, &mut needed) == 0 { return Err(error()); }
            let user = *(user_info.as_ptr() as *const *mut u8);
            // SID revision, subauthority count, six-byte authority, little-endian subauthorities.
            let system = [0x0000_0101u32, 0x0500_0000, 18];
            let admins = [0x0000_0201u32, 0x0500_0000, 32, 544];
            let creator = [0x0000_0101u32, 0x0300_0000, 0];
            let system = system.as_ptr().cast::<u8>();
            let admins = admins.as_ptr().cast::<u8>();
            let creator = creator.as_ptr().cast::<u8>();
            let trusted = |sid: *const u8| EqualSid(sid, user) != 0 || EqualSid(sid, system) != 0
                || EqualSid(sid, admins) != 0 || EqualSid(sid, creator) != 0;

            if !target.exists() {
                // Supply the private DACL at creation too: never expose an inherited shared ACL
                // between CreateDirectoryW and SetNamedSecurityInfoW. Children inherit these ACEs.
                let mut acl = [0u32; 256];
                let acl_size = std::mem::size_of_val(&acl) as u32;
                let acl = acl.as_mut_ptr().cast::<u8>();
                if InitializeAcl(acl, acl_size, 2) == 0 { return Err(error()); }
                for sid in [user as *const u8, system, admins] {
                    if AddAccessAllowedAceEx(acl, 2, 3, 0x001f_01ff, sid) == 0 { return Err(error()); }
                }
                // Absolute SECURITY_DESCRIPTOR fits in 40 bytes on Win64 (20 on Win32).
                let mut sd = [0usize; 5];
                let sd = sd.as_mut_ptr().cast::<u8>();
                if InitializeSecurityDescriptor(sd, 1) == 0
                    || SetSecurityDescriptorDacl(sd, 1, acl, 0) == 0
                    || SetSecurityDescriptorControl(sd, 0x1000, 0x1000) == 0 { return Err(error()); }
                let attributes = SecurityAttributes { length: std::mem::size_of::<SecurityAttributes>() as u32, descriptor: sd, inherit: 0 };
                let mut missing = Vec::new();
                let mut ancestor = target;
                while !ancestor.exists() {
                    missing.push(ancestor);
                    ancestor = ancestor.parent().ok_or_else(error)?;
                }
                for path in missing.into_iter().rev() {
                    let mut name = wide(path).map_err(|e| format!("{}: {}", display.display(), e))?;
                    if CreateDirectoryW(name.as_ptr(), &attributes) == 0 {
                        // Do not rewrite an existing directory's ACL after a creation race.
                        return Err(format!("cannot create {}: {}", display.display(), std::io::Error::last_os_error()));
                    }
                    let status = SetNamedSecurityInfoW(name.as_mut_ptr(), SE_FILE_OBJECT,
                        DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                        null_mut(), null_mut(), acl, null_mut());
                    if status != 0 { return Err(format!("cannot create {}: {}", display.display(), std::io::Error::from_raw_os_error(status as i32))); }
                }
            }
            let metadata = std::fs::metadata(target).map_err(|e| format!("{}: {}", display.display(), e))?;
            if !metadata.is_dir() { return Err(format!("{} is not a directory", display.display())); }
            check_location(target).map_err(|e| format!("{}: {}", display.display(), e))?;
            let name = wide(target).map_err(|e| format!("{}: {}", display.display(), e))?;
            let (mut owner, mut acl) = (null_mut(), null_mut());
            let mut sd = Descriptor(null_mut());
            let status = GetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION,
                &mut owner, null_mut(), &mut acl, null_mut(), &mut sd.0);
            if status != 0 { return Err(format!("{}: {}", display.display(), std::io::Error::from_raw_os_error(status as i32))); }
            if owner.is_null() || IsValidSid(owner) == 0 || (EqualSid(owner, user) == 0 && EqualSid(owner, admins) == 0) {
                return Err(format!("{} belongs to another user", display.display()));
            }
            if acl.is_null() || IsValidAcl(acl) == 0 { return Err(writable()); }
            // ACL header: revision, reserved byte, size, AceCount, reserved word.
            let count = read_unaligned(acl.add(4).cast::<u16>()) as u32;
            for index in 0..count {
                let mut ace = null_mut();
                if GetAce(acl, index, &mut ace) == 0 { return Err(error()); }
                let size = read_unaligned(ace.add(2).cast::<u16>()) as usize;
                // Include callback and object allow ACEs; their conditions may grant access.
                let offset = match *ace {
                    0 | 9 => 8, // ACCESS_ALLOWED[_CALLBACK]_ACE
                    5 | 11 => { // ACCESS_ALLOWED[_CALLBACK]_OBJECT_ACE
                        if size < 12 { return Err(writable()); }
                        let flags = read_unaligned(ace.add(8).cast::<u32>());
                        12 + if flags & 1 != 0 { 16 } else { 0 } + if flags & 2 != 0 { 16 } else { 0 }
                    }
                    4 => return Err(writable()), // Unsupported compound allow ACE: fail closed.
                    _ => continue,
                };
                if size < offset + 8 { return Err(writable()); }
                let mask = read_unaligned(ace.add(4).cast::<u32>());
                let sid = ace.add(offset);
                if size < offset + 8 + 4 * (*sid.add(1) as usize) || IsValidSid(sid) == 0 { return Err(writable()); }
                if mask & WRITE_RIGHTS != 0 && !trusted(sid) { return Err(writable()); }
            }
            Ok(())
        }
    }
}

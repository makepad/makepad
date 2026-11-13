//! Process-level services: foreign symbols and libraries, power state, the process's own
//! arguments, private cache files.
//!
//! - `find(name)`: a symbol in any image loaded into the process (`name` NUL-terminated; on
//!   Linux and macOS a miss asks a native bundle's address accessor for a symbol its link keeps
//!   hidden);
//!   `find_system(name)`: the C/math libraries the host may not have loaded (Linux libm/libc).
//! - `load_library(name, framework)`: a program's `#[link(name = ..)]` library (false: not found);
//!   `load_library_versioned(name)`: Linux's lib<name>.so.N where lib<name>.so is a linker script.
//! - `lookup(handle, name)`: the system's own lookup in a library handle (the program's dlsym).
//! - `exit(code)`: ends the process once Relax is done with the program (see the Windows half).
//! - `on_battery()`.
//! - `symbol_at(pc)`: the nearest exported symbol of the image holding `pc` (dladdr's view).
//! - `binary_id()`: the identity the linker gave the image holding this code.
//! - `args_bytes()`: the process arguments as bytes (`set_args`: an embedder names them).
//! - `create_private(path)`: a new file only this user can read (O_EXCL); `ensure_private_dir`:
//!   a directory that is safe to load code from (owned by this user, not writable by others);
//!   its error for another user's ends in `OTHER_USERS`.

pub use imp::*;

/// How `ensure_private_dir`'s error for a directory another user owns ends.
pub const OTHER_USERS: &str = "belongs to another user";

/// `name` (a C symbol) wherever this platform's loader finds one for the process: the default
/// search of every loaded image (`find`), then the system libraries (`find_system`). Null: none.
pub unsafe fn find_any(name: *const u8) -> *mut u8 {
    let p = find(name);
    if p.is_null() { find_system(name) } else { p }
}

/// Resolve ordinary exports first. Linux and macOS native bundles also expose address
/// accessors for archive symbols that the shared link keeps hidden.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub unsafe fn find(name: *const u8) -> *mut u8 {
    let address = imp::find(name);
    if address.is_null() {
        let name = std::ffi::CStr::from_ptr(name.cast()).to_bytes();
        let accessor = std::ffi::CString::new(relax_front::native::address_name(name)).unwrap();
        let entry = imp::find(accessor.as_ptr().cast());
        if !entry.is_null() {
            let get: unsafe extern "C" fn() -> *mut u8 = std::mem::transmute(entry);
            return get();
        }
    }
    address
}

/// The status an embedded run (the Android host) ended with when it failed; 0: it has not.
pub static EMBEDDED_FAILURE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
/// Whether a host owns this process (it named its library): a failed run then must not exit().
pub static EMBEDDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

static ARGS: std::sync::OnceLock<Vec<Vec<u8>>> = std::sync::OnceLock::new();

/// The arguments Relax runs under, when the process's own are not what it was started with: a
/// host library started from a loader (Android: bionic hands a cdylib's std no argv, the APK's
/// executable passes its own).
pub fn set_args(args: Vec<Vec<u8>>) {
    // process state: rebuilt in the kept heap
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

/// The identity the linker gave the image holding this code (the `relax` or `makepad` executable,
/// or the library the compiler is linked into), as hex: the Mach-O LC_UUID, the ELF GNU build-id
/// note, the PE CodeView signature (GUID and age); an image without one: a hash of the executable
/// file. It names the binary that was built, whatever the build scripts did: a stamp a build
/// script computes from a list of source files stays as it was whenever cargo reuses the
/// script's output, and two different binaries then print one id. Read from the image in memory,
/// once (the file hash reads the executable).
pub fn binary_id() -> &'static str {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| {
        let here = binary_id as fn() -> &'static str as usize;
        if let Some(id) = link_id(here).filter(|id| id.iter().any(|&b| b != 0)) {
            return id.iter().map(|b| format!("{b:02x}")).collect();
        }
        match std::env::current_exe().and_then(std::fs::read) {
            Ok(bytes) => format!("file-{}", &relax_base::sha256::sha256_hex(&bytes)[..32]),
            Err(_) => "unknown".to_string(),
        }
    })
}

/// The linker's id of the image holding `addr` (binary_id).
#[cfg(target_vendor = "apple")]
fn link_id(addr: usize) -> Option<Vec<u8>> {
    let mut info = DlInfo::default();
    if unsafe { dladdr(addr as *const u8, &mut info) } == 0 || info.fbase.is_null() {
        return None;
    }
    // SAFETY: dladdr's base is the image's mach_header_64, mapped with its load commands.
    unsafe {
        let h = info.fbase as *const u8;
        let rd = |p: *const u8| std::ptr::read_unaligned(p as *const u32);
        if rd(h) != 0xfeed_facf {
            return None;
        }
        let mut c = h.add(32);
        for _ in 0..rd(h.add(16)) {
            // LC_UUID
            if rd(c) == 0x1b {
                return Some(std::slice::from_raw_parts(c.add(8), 16).to_vec());
            }
            c = c.add(rd(c.add(4)) as usize);
        }
    }
    None
}

#[cfg(all(unix, not(target_vendor = "apple")))]
fn link_id(addr: usize) -> Option<Vec<u8>> {
    let mut info = DlInfo::default();
    if unsafe { dladdr(addr as *const u8, &mut info) } == 0 || info.fbase.is_null() {
        return None;
    }
    // SAFETY: dladdr's base is where the image's first segment is mapped: the ELF header and its
    // program headers; PT_NOTE segments are loaded.
    unsafe {
        let h = info.fbase as *const u8;
        let r32 = |p: *const u8| std::ptr::read_unaligned(p as *const u32);
        let r64 = |p: *const u8| std::ptr::read_unaligned(p as *const u64);
        if std::slice::from_raw_parts(h, 5) != b"\x7fELF\x02" {
            return None;
        }
        let (phoff, phentsize, phnum) = (r64(h.add(32)) as usize, std::ptr::read_unaligned(h.add(54) as *const u16) as usize, std::ptr::read_unaligned(h.add(56) as *const u16) as usize);
        let ph = |i: usize| h.add(phoff + i * phentsize);
        // the load bias: the base less the address of the segment that maps the file's start
        let start = (0..phnum).map(ph).find(|&p| r32(p) == 1 && r64(p.add(8)) == 0).map(|p| r64(p.add(16)))?;
        let bias = (h as u64).wrapping_sub(start);
        let align = |n: u32| (n as usize + 3) & !3;
        for p in (0..phnum).map(ph).filter(|&p| r32(p) == 4) {
            let mut n = bias.wrapping_add(r64(p.add(16))) as *const u8;
            let end = n.add(r64(p.add(32)) as usize);
            while n.add(12) <= end {
                let (namesz, descsz, kind) = (r32(n), r32(n.add(4)), r32(n.add(8)));
                let desc = n.add(12 + align(namesz));
                // NT_GNU_BUILD_ID
                if kind == 3 && namesz == 4 && std::slice::from_raw_parts(n.add(12), 4) == b"GNU\0" {
                    return Some(std::slice::from_raw_parts(desc, descsz as usize).to_vec());
                }
                n = desc.add(align(descsz));
            }
        }
    }
    None
}

#[cfg(windows)]
fn link_id(addr: usize) -> Option<Vec<u8>> {
    use crate::os::win32::*;
    // FROM_ADDRESS | UNCHANGED_REFCOUNT
    let mut module: HANDLE = 0;
    if unsafe { GetModuleHandleExW(0x4 | 0x2, addr as *const u16, &mut module) } == 0 || module == 0 {
        return None;
    }
    let base = module as usize as *const u8;
    // SAFETY: a loaded module's headers and debug directory are mapped readable for its life.
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
        // optional header (PE32+) at pe+24; data directory 6 (debug) at +112+48; 28-byte entries
        let (dir, size) = (rd32(pe + 24 + 160), rd32(pe + 24 + 164));
        for e in (0..size / 28).map(|k| dir + k * 28) {
            // IMAGE_DEBUG_TYPE_CODEVIEW, an RSDS record: GUID and age
            let data = rd32(e + 20);
            if rd32(e + 12) == 2 && data != 0 && rd32(data) == 0x5344_5352 {
                return Some(std::slice::from_raw_parts(base.add(data + 4), 20).to_vec());
            }
        }
    }
    None
}

/// dladdr's answer (`Dl_info`). The process's one declaration of dladdr: the profiler and the
/// executable-region scan use it too (two declarations with their own structs clash where
/// `c_char` is unsigned, arm64 Linux and Android).
#[cfg(unix)]
#[repr(C)]
pub(crate) struct DlInfo {
    pub fname: *const std::ffi::c_char,
    pub fbase: *mut u8,
    pub sname: *const std::ffi::c_char,
    pub saddr: *mut u8,
}

#[cfg(unix)]
impl Default for DlInfo {
    fn default() -> Self {
        Self { fname: std::ptr::null(), fbase: std::ptr::null_mut(), sname: std::ptr::null(), saddr: std::ptr::null_mut() }
    }
}

#[cfg(unix)]
extern "C" {
    pub(crate) fn dladdr(addr: *const u8, info: *mut DlInfo) -> i32;
}

/// From a fault handler (no allocation): the image holding `a` (its file name's last part and
/// load base) and the nearest symbol dladdr knows (empty when none, with its address). Unix:
/// dladdr, which the handler calls only on the way to the end of the process. Windows: None.
pub fn image_at_raw(a: u64) -> Option<(&'static [u8], u64, &'static [u8], u64)> {
    #[cfg(unix)]
    {
        let mut info = DlInfo::default();
        if unsafe { dladdr(a as *const u8, &mut info) } == 0 || info.fname.is_null() {
            return None;
        }
        // SAFETY: dladdr's strings belong to the loaded image (they live as long as it stays loaded)
        let file: &'static [u8] = unsafe { std::ffi::CStr::from_ptr(info.fname) }.to_bytes();
        let base = file.iter().rposition(|&c| c == b'/').map_or(file, |k| &file[k + 1..]);
        let sym: &'static [u8] = if info.sname.is_null() { b"" } else { unsafe { std::ffi::CStr::from_ptr(info.sname) }.to_bytes() };
        Some((base, info.fbase as u64, sym, info.saddr as u64))
    }
    #[cfg(not(unix))]
    {
        let _ = a;
        None
    }
}

#[cfg(unix)]
fn symbol_at_imp(pc: u64) -> Option<(String, u64, String)> {
    let mut info = DlInfo::default();
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

/// The program's atexit handlers, in the order it registered them.
static PROGRAM_ATEXIT: std::sync::Mutex<Vec<(u64, extern "C" fn())>> = std::sync::Mutex::new(Vec::new());

/// The program's `atexit`: [`exit`] runs the handler, while JIT code can still compile.
pub extern "C" fn program_atexit(f: extern "C" fn()) -> i32 {
    PROGRAM_ATEXIT.lock().unwrap_or_else(|e| e.into_inner()).push((crate::domain::current(), f));
    0
}

/// A run ended: its handlers go with it, not run (its end does not depend on its cleanup).
pub fn forget_program_atexit(bases: &[u64]) {
    PROGRAM_ATEXIT.lock().unwrap_or_else(|e| e.into_inner()).retain(|h| !bases.contains(&h.0));
}

/// Runs (and empties) the program's atexit handlers, last registered first; one a handler
/// registers runs too. exit() runs them, and so does the exit of the run's app domain, which
/// ends the process from the host (domain.rs exit_current).
pub fn run_program_atexit() {
    while let Some((_, f)) = { let mut g = PROGRAM_ATEXIT.lock().unwrap_or_else(|e| e.into_inner()); g.pop() } {
        f();
    }
}

#[cfg(unix)]
mod unix {
    use std::path::Path;

    extern "C" {
        pub fn dlopen(name: *const u8, flags: i32) -> *mut u8;
        pub fn dlsym(handle: *mut u8, name: *const u8) -> *mut u8;
    }

    /// The library's handle, or the loader's diagnostic.
    pub fn load_path(path: &str, flags: i32) -> Result<usize, String> {
        let path = std::ffi::CString::new(path).map_err(|e| e.to_string())?;
        unsafe {
            let h = dlopen(path.as_ptr().cast(), flags);
            if !h.is_null() {
                return Ok(h as usize);
            }
            extern "C" {
                fn dlerror() -> *const std::ffi::c_char;
            }
            let error = dlerror();
            Err(if error.is_null() {
                "dlopen failed without a loader diagnostic".into()
            } else {
                std::ffi::CStr::from_ptr(error).to_string_lossy().into_owned()
            })
        }
    }

    /// Gives back a handle load_path took: the library unloads once nothing else holds it.
    pub fn unload(handle: usize) {
        extern "C" {
            fn dlclose(handle: *mut u8) -> i32;
        }
        unsafe {
            dlclose(handle as *mut u8);
        }
    }

    /// The address of `name` (a C symbol) in the library `handle` (load_path) and what it loaded.
    pub fn symbol_in(handle: usize, name: &str) -> usize {
        let Ok(name) = std::ffi::CString::new(name) else { return 0 };
        unsafe { dlsym(handle as *mut u8, name.as_ptr().cast()) as usize }
    }

    /// The image (its load base) holding `addr`, and that image's file (dladdr).
    pub fn image_of(addr: usize) -> Option<(usize, std::path::PathBuf)> {
        let mut info = super::DlInfo::default();
        if unsafe { super::dladdr(addr as *const u8, &mut info) } == 0 || info.fname.is_null() {
            return None;
        }
        let file = unsafe { std::ffi::CStr::from_ptr(info.fname) }.to_string_lossy().into_owned();
        Some((info.fbase as usize, std::path::PathBuf::from(file)))
    }

    // (Android's own `lookup` also asks the host library and the program's libraries)
    #[cfg(not(target_os = "android"))]
    pub unsafe fn lookup(handle: *mut u8, name: *const u8) -> *mut u8 {
        dlsym(handle, name)
    }

    /// exit(): the program's atexit handlers, then the end every process takes
    /// (`makepad_core_util::process_end::end`, a native app's too): the host's own exit work, the
    /// flushes, `_exit`. The C library's exit handlers do not run, so neither do those a native
    /// library registered: a GPU driver's teardown at exit joined one of its threads that never
    /// ended (NVIDIA's Vulkan driver at a test process's exit, about 1 in 150 runs and more under
    /// GPU load, the process hung for good). The main thread's pthread key destructors do not run
    /// either (the program's thread locals are left as they are, as in a rustc build).
    pub fn exit(code: i32) -> ! {
        super::run_program_atexit();
        crate::selfexe::remove();
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
        makepad_core_util::process_end::end(code)
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
            #[cfg(target_vendor = "apple")]
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
            return Err(format!("{} {}", dir.display(), crate::os::proc::OTHER_USERS));
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

    #[cfg(target_env = "gnu")]
    extern "C" {
        fn dlvsym(handle: *mut u8, name: *const u8, version: *const u8) -> *mut u8;
    }

    pub unsafe fn find(name: *const u8) -> *mut u8 {
        // `name@VERSION` (a `#[link_name]` the way an assembler or linker spells a versioned reference):
        // the symbol of that version
        #[cfg(target_env = "gnu")]
        {
            let s = std::ffi::CStr::from_ptr(name as *const _).to_bytes();
            if let Some(at) = s.iter().position(|&b| b == b'@') {
                if s.get(at + 1) != Some(&b'@') {
                    let (base, ver) = (&s[..at], &s[at + 1..]);
                    let mut b = base.to_vec();
                    b.push(0);
                    let mut v = ver.to_vec();
                    v.push(0);
                    return dlvsym(std::ptr::null_mut(), b.as_ptr(), v.as_ptr());
                }
            }
        }
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

    /// A native bundle must resolve every import before any program code starts. Its handle.
    pub fn load_path(path: &str) -> Result<usize, String> {
        super::unix::load_path(path, 2 | 0x100)
    }
    pub use super::unix::{image_of, symbol_in, unload};

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
        // process state: kept heap
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
        // to the kept heap
        let _k = crate::zone_alloc::kept();
        let i = pool.used.len();
        assert!(i < pool.count, "the host library's {} C-callable stubs are all in use", pool.count);
        unsafe { *pool.table.add(i) = target };
        let a = pool.base + 16 * i as u64;
        pool.used.insert(target, a);
        // one stub per target, never reused: say how fast they go (a game used up 4096)
        if (i + 1).is_power_of_two() && i >= 1023 { eprintln!("relax: {} of {} C-callable stubs in use", i + 1, pool.count); }
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

    /// A shared library by absolute path (--allow-c's native library), its symbols global. Its handle.
    pub fn load_path(path: &str) -> Result<usize, String> {
        super::unix::load_path(path, 2 | 0x100)
    }
    pub use super::unix::{image_of, symbol_in, unload};

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
                eprintln!("relax: cannot load {}: {}", path.trim_end_matches('\0'), std::ffi::CStr::from_ptr(e).to_string_lossy());
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

#[cfg(target_vendor = "apple")]
mod imp {
    pub use super::unix::{os_args_bytes, create_private, ensure_private_dir, exit, last_error, lookup, set_last_error};
    use super::unix::{dlopen, dlsym};

    /// Symbol in any loaded image (RTLD_DEFAULT). libm lives in libSystem, always loaded.
    pub unsafe fn find(name: *const u8) -> *mut u8 {
        dlsym(-2isize as *mut u8, name)
    }

    /// libSystem's (libc, libm): always loaded, so `find` sees them too.
    pub unsafe fn find_system(name: *const u8) -> *mut u8 {
        // RTLD_NOW; from the dyld shared cache, not a file on disk
        let h = dlopen("/usr/lib/libSystem.B.dylib\0".as_ptr(), 2);
        if h.is_null() { std::ptr::null_mut() } else { dlsym(h, name) }
    }

    /// A shared library by absolute path (--allow-c's native library), its symbols global. Its handle.
    pub fn load_path(path: &str) -> Result<usize, String> {
        super::unix::load_path(path, 2 | 8)
    }
    pub use super::unix::{image_of, symbol_in, unload};

    pub fn load_library(name: &str, framework: bool) -> bool {
        let path = if framework { format!("/System/Library/Frameworks/{}.framework/{}\0", name, name) } else { format!("lib{}.dylib\0", name) };
        // RTLD_NOW | RTLD_GLOBAL
        unsafe { !dlopen(path.as_ptr(), 2 | 8).is_null() }
    }

    pub fn load_library_versioned(_name: &str) -> bool {
        false
    }

    /// iOS and tvOS: a phone or tablet runs on its battery (IOKit's power sources are macOS's)
    #[cfg(not(target_os = "macos"))]
    pub fn on_battery() -> bool {
        true
    }

    #[cfg(target_os = "macos")]
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

    /// --allow-c's native library is not supported on Windows.
    pub fn load_path(_path: &str) -> Result<usize, String> {
        Err("--allow-c native libraries are not supported on Windows".into())
    }

    pub fn symbol_in(_handle: usize, _name: &str) -> usize {
        0
    }

    /// load_path takes no handle here.
    pub fn unload(_handle: usize) {}

    pub fn image_of(_addr: usize) -> Option<(usize, std::path::PathBuf)> {
        None
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

    /// What unix's exit() does: the program's atexit handlers, then the process's (`_cexit`: the
    /// run's exit work, the control endpoint's cleanup). A run that returned from main or quit
    /// through its app domain did neither: no final cache, and a discovery file left behind, which
    /// the hub reads as a crash (11-04). Then the process ends without ExitProcess
    /// (`makepad_core_util::process_end::end`), which would run the fiber-local destructors of the
    /// exiting thread: the program's thread locals (FlsAlloc callbacks in JIT code) would run after
    /// Relax dropped the unit, and unix's exit() does not run the main thread's key destructors
    /// either.
    pub fn exit(code: i32) -> ! {
        super::run_program_atexit();
        crate::selfexe::remove();
        makepad_core_util::process_end::end(code)
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

        /// `DOMAIN\name` of a SID, else its `S-1-...` form.
        unsafe fn account(sid: *const u8) -> String {
            let (mut name, mut domain) = ([0u16; 256], [0u16; 256]);
            let (mut n, mut d, mut kind) = (256u32, 256u32, 0u32);
            if LookupAccountSidW(null_mut(), sid, name.as_mut_ptr(), &mut n, domain.as_mut_ptr(), &mut d, &mut kind) != 0 {
                let name = String::from_utf16_lossy(&name[..n as usize]);
                let domain = String::from_utf16_lossy(&domain[..d as usize]);
                return if domain.is_empty() { name } else { format!("{}\\{}", domain, name) };
            }
            // revision, subauthority count, six-byte big-endian authority, little-endian subauthorities
            let count = *sid.add(1) as usize;
            let authority = (0..6).fold(0u64, |a, i| a << 8 | *sid.add(2 + i) as u64);
            let mut text = format!("S-{}-{}", *sid, authority);
            for i in 0..count {
                text += &format!("-{}", read_unaligned(sid.add(8 + 4 * i).cast::<u32>()));
            }
            text
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
                return Err(format!("{} {}", display.display(), crate::os::proc::OTHER_USERS));
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
                if mask & WRITE_RIGHTS != 0 && !trusted(sid) {
                    // name the principal: the fix is that account's grant, not this check
                    return Err(format!("{} is writable by {} (a JIT cache may be writable only by you, SYSTEM and Administrators)",
                        display.display(), account(sid)));
                }
            }
            Ok(())
        }
    }
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&EMBEDDED_FAILURE, std::sync::atomic::AtomicI32::new(0));
    reset(&PROGRAM_ATEXIT, std::sync::Mutex::new(Vec::new()));
}

//! Minimal CEF subprocess entry, also compiled directly into Relax. Keep this
//! module std-only: it must run before the loader, Cx or allocator setup.
//! The library stays loaded until process exit (Chromium installs allocator hooks).

#[cfg(unix)]
use std::ffi::CString;
use std::ffi::{c_char, c_int, c_void};

/// ABI layout implemented by makepad-cef, also negotiated by the browser process.
pub(crate) const CEF_API_VERSION: c_int = 13800;

/// Some(exit code) when this process is a CEF subprocess; None otherwise.
/// Call this first in main and exit immediately on Some, before initializing
/// allocators, Cx, worker threads or a JIT. A helper setup error returns Some(1).
pub fn run_helper_if_subprocess() -> Option<i32> {
    if !std::env::args_os()
        .skip(1)
        .any(|a| a.to_string_lossy().starts_with("--type="))
    {
        return None;
    }
    Some(match library_path().and_then(|lib| run(lib.as_os_str())) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("makepad: CEF subprocess: {e}");
            1
        }
    })
}

fn library_path() -> Result<std::path::PathBuf, String> {
    #[cfg(target_os = "macos")]
    {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = exe
            .parent()
            .ok_or("CEF helper has no executable directory")?;
        Ok(dir.join("../../../Chromium Embedded Framework.framework/Chromium Embedded Framework"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::var_os("MAKEPAD_CEF_LIBRARY")
            .filter(|path| !path.is_empty())
            .map(std::path::PathBuf::from)
            .ok_or_else(|| "MAKEPAD_CEF_LIBRARY is missing for the CEF subprocess".into())
    }
}

type ApiHash = unsafe extern "C" fn(c_int, c_int) -> *const c_char;

#[cfg(unix)]
fn run(lib: &std::ffi::OsStr) -> Result<i32, String> {
    use std::os::unix::ffi::OsStrExt;
    #[repr(C)]
    struct MainArgs {
        argc: c_int,
        argv: *mut *mut c_char,
    }
    type ExecuteProcess = unsafe extern "C" fn(*const MainArgs, *mut c_void, *mut c_void) -> c_int;
    #[cfg_attr(target_os = "linux", link(name = "dl"))]
    extern "C" {
        fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn dlerror() -> *const c_char;
    }
    #[cfg(target_os = "macos")]
    let flags = 0x1 | 0x4 | 0x100; // RTLD_LAZY | RTLD_LOCAL | RTLD_FIRST
    #[cfg(not(target_os = "macos"))]
    let flags = 2; // RTLD_NOW (RTLD_LOCAL is zero on Linux)
    let path = CString::new(lib.as_bytes()).map_err(|e| e.to_string())?;
    unsafe {
        let h = dlopen(path.as_ptr(), flags);
        if h.is_null() {
            let e = dlerror();
            return Err(format!(
                "dlopen {}: {}",
                lib.to_string_lossy(),
                if e.is_null() {
                    "?".into()
                } else {
                    std::ffi::CStr::from_ptr(e).to_string_lossy()
                }
            ));
        }
        let hash = dlsym(h, c"cef_api_hash".as_ptr());
        let exec = dlsym(h, c"cef_execute_process".as_ptr());
        if hash.is_null() || exec.is_null() {
            return Err("libcef lacks cef_api_hash / cef_execute_process".into());
        }
        let hash: ApiHash = std::mem::transmute(hash);
        if hash(CEF_API_VERSION, 0).is_null() {
            return Err("cef_api_hash returned null".into());
        }
        // argv as this process got it (CEF parses its switches from it)
        let args: Vec<CString> = std::env::args_os()
            .map(|a| CString::new(a.as_bytes()))
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        let mut ptrs: Vec<*mut c_char> = args.iter().map(|a| a.as_ptr() as *mut c_char).collect();
        ptrs.push(std::ptr::null_mut());
        let main_args = MainArgs {
            argc: args.len() as c_int,
            argv: ptrs.as_mut_ptr(),
        };
        let exec: ExecuteProcess = std::mem::transmute(exec);
        Ok(exec(&main_args, std::ptr::null_mut(), std::ptr::null_mut()))
    }
}

#[cfg(windows)]
fn run(lib: &std::ffi::OsStr) -> Result<i32, String> {
    use std::os::windows::ffi::OsStrExt;
    // Windows CEF reads its switches from GetCommandLineW itself; it takes the module instance
    #[repr(C)]
    struct MainArgs {
        instance: *mut c_void,
    }
    type ExecuteProcess = unsafe extern "C" fn(*const MainArgs, *mut c_void, *mut c_void) -> c_int;
    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        fn GetLastError() -> u32;
    }
    let wide: Vec<u16> = lib.encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        // libcef.dll's own folder first for its dependents (chrome_elf.dll sits beside it): a plain
        // LoadLibraryW searches the exe's folder -- the loader's -- and fails with error 126
        // (LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS)
        let h = LoadLibraryExW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            0x0000_0100 | 0x0000_1000,
        );
        if h.is_null() {
            return Err(format!(
                "LoadLibraryEx {}: error {}",
                lib.to_string_lossy(),
                GetLastError()
            ));
        }
        let hash = GetProcAddress(h, c"cef_api_hash".as_ptr());
        let exec = GetProcAddress(h, c"cef_execute_process".as_ptr());
        if hash.is_null() || exec.is_null() {
            return Err("libcef lacks cef_api_hash / cef_execute_process".into());
        }
        let hash: ApiHash = std::mem::transmute(hash);
        if hash(CEF_API_VERSION, 0).is_null() {
            return Err("cef_api_hash returned null".into());
        }
        let main_args = MainArgs {
            instance: GetModuleHandleW(std::ptr::null()),
        };
        let exec: ExecuteProcess = std::mem::transmute(exec);
        Ok(exec(&main_args, std::ptr::null_mut(), std::ptr::null_mut()))
    }
}

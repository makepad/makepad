//! Relax's executable on Android. An app process is `app_process64`, which cannot run Relax's build
//! scripts (Relax runs itself again for each: front/src/unitgraph.rs); this PIE sits in the APK's
//! native-library directory, the one place an app may exec from, next to libmakepad.so. It loads
//! that library and hands it its arguments (bionic gives a shared object's std no argv).

use std::ffi::{c_char, c_int, c_void, CStr, CString};

extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

const RTLD_NOW: c_int = 2;
const RTLD_GLOBAL: c_int = 0x100;

fn dl_error() -> String {
    let e = unsafe { dlerror() };
    if e.is_null() {
        "unknown error".to_string()
    } else {
        unsafe { CStr::from_ptr(e) }.to_string_lossy().into_owned()
    }
}

fn main() {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("makepad_exe: cannot find itself: {e}");
            std::process::exit(1);
        }
    };
    let library = exe.with_file_name("libmakepad.so");
    let path = CString::new(library.to_string_lossy().as_bytes()).expect("library path");
    let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW | RTLD_GLOBAL) };
    if handle.is_null() {
        eprintln!("makepad_exe: cannot load {}: {}", library.display(), dl_error());
        std::process::exit(1);
    }
    let entry = unsafe { dlsym(handle, c"makepad_cli_main".as_ptr()) };
    if entry.is_null() {
        eprintln!("makepad_exe: {} has no makepad_cli_main: {}", library.display(), dl_error());
        std::process::exit(1);
    }
    let entry: extern "C" fn(c_int, *const *const c_char) = unsafe { std::mem::transmute(entry) };
    // std's argument vector is not available to this library's std; pass it as C does
    let args: Vec<CString> = std::env::args_os().map(|a| {
        use std::os::unix::ffi::OsStrExt;
        CString::new(a.as_bytes()).expect("argument without NUL")
    }).collect();
    let mut argv: Vec<*const c_char> = args.iter().map(|a| a.as_ptr()).collect();
    argv.push(std::ptr::null());
    entry(args.len() as c_int, argv.as_ptr());
}

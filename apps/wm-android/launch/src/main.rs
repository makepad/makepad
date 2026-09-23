//! Run one hosted app in this process: `dlopen` its library (the APK's, or
//! one built on the phone) and call its `makepad_hosted_main`, the
//! `app_main!` export for a hosted child (platform/src/os/linux/android/
//! android_hosted.rs). An app's library may live where the phone forbids
//! exec but allows mapping code; this executable lives where exec is allowed.

use std::ffi::{c_char, c_int, c_void, CStr, CString};

extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

const RTLD_NOW: c_int = 2;

fn last_dl_error() -> String {
    let err = unsafe { dlerror() };
    if err.is_null() {
        "unknown error".to_string()
    } else {
        unsafe { CStr::from_ptr(err) }.to_string_lossy().into_owned()
    }
}

fn main() {
    let Some(library) = std::env::args().nth(1) else {
        eprintln!("usage: makepad-launch <app library> [args]");
        std::process::exit(2);
    };
    // The app reads its mode from the environment: its library's own copy
    // of std never saw this process's arguments.
    std::env::set_var("MAKEPAD_STDIN_LOOP", "1");
    let path = CString::new(library.clone()).expect("library path");
    let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW) };
    if handle.is_null() {
        eprintln!("makepad-launch: cannot load {library}: {}", last_dl_error());
        std::process::exit(1);
    }
    let symbol = unsafe { dlsym(handle, c"makepad_hosted_main".as_ptr()) };
    if symbol.is_null() {
        eprintln!("makepad-launch: {library} has no makepad_hosted_main: {}", last_dl_error());
        std::process::exit(1);
    }
    let entry: extern "C" fn() = unsafe { std::mem::transmute(symbol) };
    entry();
}

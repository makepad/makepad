//! rustc stand-in for std/std/sys/rt.rs.

use core::ffi::c_char;

pub fn catch_panic(f: fn(*mut u8), data: *mut u8) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(data))).is_err()
}

/// argv rebuilt from real std (leaked once): the shim has no runtime that records it.
pub fn args() -> (usize, *const *const c_char) {
    static ARGV: std::sync::OnceLock<(usize, usize)> = std::sync::OnceLock::new();
    let (n, p) = *ARGV.get_or_init(|| {
        let mut ptrs: Vec<*const c_char> = Vec::new();
        for a in std::env::args_os() {
            let c = std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(a.as_os_str())).unwrap();
            ptrs.push(c.into_raw() as *const c_char);
        }
        ptrs.push(core::ptr::null());
        let n = ptrs.len() - 1;
        (n, Box::leak(ptrs.into_boxed_slice()).as_ptr() as usize)
    });
    (n, p as *const *const c_char)
}

pub fn panicking() -> bool {
    std::thread::panicking()
}

pub fn set_panic_trampoline(f: fn(&str, &core::panic::Location<'_>)) {
    std::panic::set_hook(std::boxed::Box::new(move |info| {
        let msg = match info.payload_as_str() {
            Some(s) => s,
            None => "Box<dyn Any>",
        };
        if let Some(loc) = info.location() {
            f(msg, loc);
        }
    }));
}

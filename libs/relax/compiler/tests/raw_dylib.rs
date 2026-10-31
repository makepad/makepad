// `#[link(name = "<x>.dll", kind = "raw-dylib", modifiers = "+verbatim")]` blocks made by a macro,
// as windows-link's `link!` (what windows-sys 0.61 and libloading declare every import with):
// Relax binds the imports to the named DLL's exports (os::proc::load_library + find). On other
// systems the same macro gives a plain extern block (libc's getpid).
#[cfg(windows)]
macro_rules! link {
    ($library:literal $abi:literal $($link_name:literal)? fn $($function:tt)*) => (
        #[link(name = $library, kind = "raw-dylib", modifiers = "+verbatim")]
        extern $abi {
            $(#[link_name=$link_name])?
            pub fn $($function)*;
        }
    )
}

#[cfg(windows)]
mod sys {
    link!("kernel32.dll" "system" fn GetCurrentProcessId() -> u32);
    link!("kernel32.dll" "system" "GetTickCount64" fn ticks() -> u64);
    link!("user32.dll" "system" fn GetSystemMetrics(index: i32) -> i32);
    pub fn pid() -> u32 {
        unsafe { GetCurrentProcessId() }
    }
    pub fn other() -> bool {
        // SM_CMONITORS >= 0 on any session; GetTickCount64 > 0 after boot
        unsafe { ticks() > 0 && GetSystemMetrics(80) >= 0 }
    }
}

#[cfg(not(windows))]
mod sys {
    extern "C" {
        fn getpid() -> i32;
    }
    pub fn pid() -> u32 {
        unsafe { getpid() as u32 }
    }
    pub fn other() -> bool {
        true
    }
}

#[test]
fn raw_dylib_imports_bind() {
    assert_eq!(sys::pid(), std::process::id());
    assert!(sys::other());
}

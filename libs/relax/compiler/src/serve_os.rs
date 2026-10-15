//! OS services for the check server. No entropy derived from clocks or process IDs.
use std::sync::atomic::{AtomicBool, Ordering};
pub static STOP: AtomicBool = AtomicBool::new(false);

pub fn token() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    #[cfg(target_os = "macos")]
    {
        extern "C" { fn getentropy(p: *mut u8, n: usize) -> i32; }
        if unsafe { getentropy(bytes.as_mut_ptr(), bytes.len()) } != 0 { return Err(std::io::Error::last_os_error()); }
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        extern "C" { fn getrandom(p: *mut u8, n: usize, flags: u32) -> isize; }
        let mut at = 0;
        while at < bytes.len() {
            let n = unsafe { getrandom(bytes.as_mut_ptr().add(at), bytes.len() - at, 0) };
            if n < 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() == std::io::ErrorKind::Interrupted { continue; }
                return Err(e);
            }
            if n == 0 { return Err(std::io::Error::other("OS entropy returned no bytes")); }
            at += n as usize;
        }
    }
    #[cfg(windows)]
    {
        if unsafe { relax_session::win32::ProcessPrng(bytes.as_mut_ptr(), bytes.len()) } == 0 { return Err(std::io::Error::last_os_error()); }
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 { return false; }
    #[cfg(unix)]
    {
        extern "C" { fn kill(pid: i32, sig: i32) -> i32; }
        unsafe { kill(pid as i32, 0) == 0 }
    }
    #[cfg(windows)]
    {
        use relax_session::win32::{CloseHandle, GetExitCodeProcess, OpenProcess};
        unsafe {
            let h = OpenProcess(0x1000, 0, pid);
            if h == 0 { return false; }
            let mut code = 0;
            let ok = GetExitCodeProcess(h, &mut code) != 0 && code == 259;
            CloseHandle(h); ok
        }
    }
}

pub fn signals() {
    STOP.store(false, Ordering::Relaxed);
    #[cfg(unix)]
    unsafe {
        extern "C" fn stop(_: i32) { STOP.store(true, Ordering::Relaxed); }
        extern "C" { fn signal(sig: i32, handler: usize) -> usize; }
        signal(2, stop as *const () as usize);
        signal(15, stop as *const () as usize);
    }
    #[cfg(windows)]
    unsafe {
        extern "system" fn stop(kind: u32) -> i32 {
            if matches!(kind, 0 | 1 | 2 | 5 | 6) { STOP.store(true, Ordering::Relaxed); 1 } else { 0 }
        }
        relax_session::win32::SetConsoleCtrlHandler(Some(stop), 1);
    }
}

pub fn detach(cmd: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe { cmd.pre_exec(|| {
            extern "C" { fn setsid() -> i32; }
            if setsid() < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
        }); }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x00000008 | 0x00000200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    }
}

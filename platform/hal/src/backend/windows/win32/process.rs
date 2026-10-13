//! Other processes and the shell: bringing a launched app's window to the front, opening URLs.
//! Moved from os/windows/win32_window.rs (`activate_process`); URL opening is new (the old
//! backend logged it as unimplemented).

use makepad_windows_sys::core::{BOOL, PCWSTR};
use makepad_windows_sys::Win32::Foundation::{HWND, LPARAM};
use makepad_windows_sys::Win32::UI::Shell::ShellExecuteW;
use makepad_windows_sys::Win32::UI::WindowsAndMessaging::*;

struct Find {
    pid: u32,
    found: HWND,
}

unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let find = &mut *(lparam.0 as *mut Find);
    let mut owner_pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut owner_pid));
    // the main window: visible and unowned (tool windows and popups have an owner)
    let unowned = GetWindow(hwnd, GW_OWNER).0.is_null();
    if owner_pid == find.pid && IsWindowVisible(hwnd).as_bool() && unowned {
        find.found = hwnd;
        return BOOL(0);
    }
    BOOL(1)
}

/// The first visible unowned top-level window of `pid` comes to the front. The caller must be
/// allowed to set the foreground (the active launcher is); it hands that right to `pid`.
pub fn activate_process(pid: u32) -> bool {
    if std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_some() {
        return false;
    }
    let mut find = Find { pid, found: HWND(std::ptr::null_mut()) };
    unsafe {
        let _ = AllowSetForegroundWindow(pid);
        let _ = EnumWindows(Some(visit), LPARAM(&mut find as *mut Find as isize));
        if find.found.0.is_null() {
            return false;
        }
        if IsIconic(find.found).as_bool() {
            let _ = ShowWindow(find.found, SW_RESTORE);
        }
        SetForegroundWindow(find.found).as_bool()
    }
}

/// Opens `url` with its registered handler (the default browser for http/https).
pub fn open_url(url: &str) -> bool {
    let wide: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    // ShellExecuteW answers a value above 32 on success
    let r = unsafe { ShellExecuteW(None, Some(PCWSTR(verb.as_ptr())), PCWSTR(wide.as_ptr()), None, None, SW_SHOWNORMAL) };
    r.0 as isize > 32
}

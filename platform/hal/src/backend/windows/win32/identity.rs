//! The Windows identity a launcher gave this process (crate::app_id: MAKEPAD_APP_ID, set by
//! `makepad app <id>`): the AppUserModelID `Makepad.<id>` -- the one the app's Start menu
//! shortcut carries -- for the process, and per top-level window the relaunch properties the
//! taskbar pins (command `"<makepad.exe>" app <id>`, the app's title, the icon the runner made).
//! Without one, Windows groups the windows under the executable as before.

use makepad_windows_sys::core::{GUID, PWSTR};
use makepad_windows_sys::Win32::Foundation::{HWND, PROPERTYKEY};
use makepad_windows_sys::Win32::Storage::EnhancedStorage::*;
use makepad_windows_sys::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0};
use makepad_windows_sys::Win32::System::Variant::{VARENUM, VT_LPWSTR};
use makepad_windows_sys::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use makepad_windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `Makepad.<id>`
fn aumid(id: &str) -> String {
    format!("Makepad.{id}")
}

/// Before the first window: the process's AppUserModelID.
pub(super) fn process_started() {
    let Some(id) = crate::app_id() else { return };
    let w = wide(&aumid(&id));
    if let Err(e) = unsafe { SetCurrentProcessExplicitAppUserModelID(makepad_windows_sys::core::PCWSTR(w.as_ptr())) } {
        eprintln!("[hal-win32] SetCurrentProcessExplicitAppUserModelID failed ({e:?})");
    }
}

fn store(hwnd: HWND) -> Option<IPropertyStore> {
    let raw = unsafe { SHGetPropertyStoreForWindow(hwnd, &IPropertyStore::IID as *const GUID) }.ok()?;
    unsafe { IPropertyStore::from_raw(raw) }.ok()
}

fn set(store: &IPropertyStore, key: &PROPERTYKEY, value: Option<&str>) {
    // SetValue copies the string: the buffer only lives for the call
    let w = value.map(wide);
    let (vt, p) = match &w {
        Some(w) => (VT_LPWSTR, PWSTR(w.as_ptr() as *mut u16)),
        None => (VARENUM(0), PWSTR(std::ptr::null_mut())),
    };
    let v = PROPVARIANT { Anonymous: PROPVARIANT_0 { Anonymous: PROPVARIANT_0_0 { vt, wReserved1: 0, wReserved2: 0, wReserved3: 0, Anonymous: PROPVARIANT_0_0_0 { pwszVal: p } } } };
    if let Err(e) = unsafe { store.SetValue(key, &v) } {
        eprintln!("[hal-win32] window property store SetValue failed ({e:?})");
    }
}

/// A new top-level window: the app's id and relaunch properties (before it is shown; the
/// relaunch command and display name only count together, both with the id).
pub(super) fn window_created(hwnd: HWND) {
    let Some(id) = crate::app_id() else { return };
    let Some(store) = store(hwnd) else { return };
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let title = std::env::var("MAKEPAD_APP_TITLE").ok().filter(|t| !t.is_empty()).unwrap_or_else(|| id.clone());
    set(&store, &PKEY_AppUserModel_ID, Some(&aumid(&id)));
    set(&store, &PKEY_AppUserModel_RelaunchCommand, Some(&format!("\"{exe}\" app {id}")));
    set(&store, &PKEY_AppUserModel_RelaunchDisplayNameResource, Some(&title));
    if let Some(icon) = std::env::var("MAKEPAD_APP_ICON").ok().filter(|i| !i.is_empty()) {
        set(&store, &PKEY_AppUserModel_RelaunchIconResource, Some(&icon));
    }
}

/// WM_DESTROY: the shell wants a window's properties cleared before it goes.
pub(super) fn window_destroyed(hwnd: HWND) {
    if crate::app_id().is_none() {
        return;
    }
    let Some(store) = store(hwnd) else { return };
    for key in [&PKEY_AppUserModel_RelaunchIconResource, &PKEY_AppUserModel_RelaunchDisplayNameResource, &PKEY_AppUserModel_RelaunchCommand, &PKEY_AppUserModel_ID] {
        set(&store, key, None);
    }
}

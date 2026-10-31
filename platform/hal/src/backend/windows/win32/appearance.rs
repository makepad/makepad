//! The light or dark look of what Windows draws for the app: menus through uxtheme's undocumented
//! ordinals (there is no public API), the window frame through DWM.

use makepad_windows_sys::core::{BOOL, PCSTR, PCWSTR};
use makepad_windows_sys::Win32::Foundation::HWND;
use makepad_windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
use makepad_windows_sys::system_web_view::Win32::System::LibraryLoader::LOAD_LIBRARY_SEARCH_DEFAULT_DIRS;
use makepad_windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryExW};
use crate::types::native_appearance;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use std::{ffi::c_void, mem};

#[repr(C)]
struct OsVersionInfo {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    csd_version: [u16; 128],
}

#[repr(i32)]
enum PreferredAppMode {
    AllowDark = 1,
    ForceDark = 2,
    ForceLight = 3,
}

type RtlGetVersion = unsafe extern "system" fn(*mut OsVersionInfo) -> i32;
type SetPreferredAppMode = unsafe extern "system" fn(PreferredAppMode);
type AllowDarkModeForWindow = unsafe extern "system" fn(HWND, BOOL);
type NoArguments = unsafe extern "system" fn();

struct Functions {
    /// DWMWA_USE_IMMERSIVE_DARK_MODE, which was 19 before build 19041.
    immersive_dark_mode: Option<DWMWINDOWATTRIBUTE>,
    set_preferred_app_mode: Option<SetPreferredAppMode>,
    allow_dark_mode_for_window: Option<AllowDarkModeForWindow>,
    refresh_immersive_color_policy_state: Option<NoArguments>,
    flush_menu_themes: Option<NoArguments>,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn os_build() -> u32 {
    unsafe {
        let Ok(ntdll) = GetModuleHandleW(Some(PCWSTR(wide("ntdll.dll").as_ptr()))) else { return 0 };
        let Some(get_version) = GetProcAddress(ntdll, PCSTR(c"RtlGetVersion".as_ptr().cast())) else { return 0 };
        let get_version = mem::transmute::<_, RtlGetVersion>(get_version);
        let mut info = OsVersionInfo { size: mem::size_of::<OsVersionInfo>() as u32, ..mem::zeroed() };
        get_version(&mut info);
        info.build
    }
}

fn functions() -> &'static Functions {
    static FUNCTIONS: OnceLock<Functions> = OnceLock::new();
    FUNCTIONS.get_or_init(|| unsafe {
        let build = os_build();
        // Before build 18362, ordinal 135 took a bool, and older builds may have unrelated code at these ordinals.
        let uxtheme = (build >= 18362)
            .then(|| LoadLibraryExW(PCWSTR(wide("uxtheme.dll").as_ptr()), None, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS).ok())
            .flatten();
        let ordinal = |ordinal: u16| uxtheme.and_then(|uxtheme| GetProcAddress(uxtheme, PCSTR(ordinal as usize as *const u8)));
        Functions {
            immersive_dark_mode: match build {
                19041.. => Some(DWMWINDOWATTRIBUTE(20)),
                17763.. => Some(DWMWINDOWATTRIBUTE(19)),
                _ => None,
            },
            set_preferred_app_mode: ordinal(135).map(|f| mem::transmute::<_, SetPreferredAppMode>(f)),
            allow_dark_mode_for_window: ordinal(133).map(|f| mem::transmute::<_, AllowDarkModeForWindow>(f)),
            refresh_immersive_color_policy_state: ordinal(104).map(|f| mem::transmute::<_, NoArguments>(f)),
            flush_menu_themes: ordinal(136).map(|f| mem::transmute::<_, NoArguments>(f)),
        }
    })
}

/// What the app asked for, `native_appearance::SYSTEM` until it asks.
static APPEARANCE: AtomicU32 = AtomicU32::new(native_appearance::SYSTEM);

/// The app's appearance from now on, for its menus. The caller re-themes its windows
/// (`apply_to_window`, then `WM_THEMECHANGED` for an open window menu).
pub(super) fn set_app_appearance(value: u32) {
    APPEARANCE.store(value, Ordering::Release);
    let functions = functions();
    let mode = match value {
        native_appearance::LIGHT => PreferredAppMode::ForceLight,
        native_appearance::DARK => PreferredAppMode::ForceDark,
        _ => PreferredAppMode::AllowDark,
    };
    unsafe {
        if let Some(set_preferred_app_mode) = functions.set_preferred_app_mode { set_preferred_app_mode(mode); }
        if let Some(refresh) = functions.refresh_immersive_color_policy_state { refresh(); }
        if let Some(flush) = functions.flush_menu_themes { flush(); }
    }
}

/// A window frame's look: the app's appearance, or the system theme while it follows that.
pub(super) fn apply_to_window(hwnd: HWND, system_dark: bool) {
    let value = APPEARANCE.load(Ordering::Acquire);
    let functions = functions();
    let dark = match value {
        native_appearance::LIGHT => false,
        native_appearance::DARK => true,
        _ => system_dark,
    };
    unsafe {
        if let Some(allow_dark_mode_for_window) = functions.allow_dark_mode_for_window {
            allow_dark_mode_for_window(hwnd, BOOL((value != native_appearance::LIGHT) as i32));
        }
        if let Some(attribute) = functions.immersive_dark_mode {
            let dark = BOOL(dark as i32);
            let _ = DwmSetWindowAttribute(hwnd, attribute, &dark as *const _ as *const c_void, mem::size_of_val(&dark) as u32);
        }
    }
}

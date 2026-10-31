use {
    crate::{
        display_context::NativeAppearance,
        os::windows::win32_app::with_win32_app,
        windows::{
            core::{s, BOOL, PCSTR},
            Win32::{
                Foundation::HWND,
                Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE},
                System::LibraryLoader::{GetProcAddress, LoadLibraryA},
                UI::WindowsAndMessaging::SendMessageW,
            },
        },
    },
    std::{ffi::c_void, mem},
};

// The vendored `windows` bindings lack this constant.
const WM_THEMECHANGED: u32 = 0x031A;

#[repr(i32)]
enum PreferredAppMode {
    AllowDark = 1,
    ForceDark = 2,
    ForceLight = 3,
}

type SetPreferredAppMode = unsafe extern "system" fn(mode: PreferredAppMode);
type AllowDarkModeForWindow = unsafe extern "system" fn(hwnd: HWND, allow: BOOL);
type RefreshImmersiveColorPolicyState = unsafe extern "system" fn();
type FlushMenuThemes = unsafe extern "system" fn();

#[repr(C)]
struct OsVersionInfo {
    os_version_info_size: u32,
    major_version: u32,
    minor_version: u32,
    build_number: u32,
    platform_id: u32,
    csd_version: [u16; 128],
}

#[derive(Clone, Copy)]
pub struct AppearanceFunctions {
    immersive_dark_mode_attribute: Option<DWMWINDOWATTRIBUTE>,
    set_preferred_app_mode: Option<SetPreferredAppMode>,
    allow_dark_mode_for_window: Option<AllowDarkModeForWindow>,
    refresh_immersive_color_policy_state: Option<RefreshImmersiveColorPolicyState>,
    flush_menu_themes: Option<FlushMenuThemes>,
}

impl AppearanceFunctions {
    fn load() -> AppearanceFunctions {
        windows_core::link!("ntdll.dll" "system" fn RtlGetVersion(info: *mut OsVersionInfo) -> i32);
        unsafe {
            let mut info = OsVersionInfo {
                os_version_info_size: mem::size_of::<OsVersionInfo>() as u32,
                ..mem::zeroed()
            };
            RtlGetVersion(&mut info);
            let build = info.build_number;

            // Before build 18362, ordinal 135 took a bool, and older builds
            // may have unrelated code at these ordinals.
            let uxtheme = (build >= 18362)
                .then(|| LoadLibraryA(s!("uxtheme.dll")).ok())
                .flatten();
            // Undocumented uxtheme functions that are only exported by ordinal.
            let get_ordinal = |ordinal: u16| {
                let name = PCSTR(ordinal as usize as *const u8);
                uxtheme.and_then(|uxtheme| GetProcAddress(uxtheme, name))
            };
            AppearanceFunctions {
                // DWMWA_USE_IMMERSIVE_DARK_MODE, which was 19 before build 19041.
                immersive_dark_mode_attribute: match build {
                    19041.. => Some(DWMWINDOWATTRIBUTE(20)),
                    17763.. => Some(DWMWINDOWATTRIBUTE(19)),
                    _ => None,
                },
                set_preferred_app_mode: get_ordinal(135)
                    .map(|f| mem::transmute::<_, SetPreferredAppMode>(f)),
                allow_dark_mode_for_window: get_ordinal(133)
                    .map(|f| mem::transmute::<_, AllowDarkModeForWindow>(f)),
                refresh_immersive_color_policy_state: get_ordinal(104)
                    .map(|f| mem::transmute::<_, RefreshImmersiveColorPolicyState>(f)),
                flush_menu_themes: get_ordinal(136)
                    .map(|f| mem::transmute::<_, FlushMenuThemes>(f)),
            }
        }
    }

    fn get_or_load() -> AppearanceFunctions {
        with_win32_app(|app| *app.appearance_functions.get_or_insert_with(Self::load))
    }

    fn apply_to_window(&self, hwnd: HWND, appearance: NativeAppearance) {
        unsafe {
            if let Some(allow_dark_mode_for_window) = self.allow_dark_mode_for_window {
                allow_dark_mode_for_window(hwnd, BOOL::from(appearance != NativeAppearance::Light));
            }
            if let Some(attribute) = self.immersive_dark_mode_attribute {
                let use_dark_mode = BOOL::from(appearance == NativeAppearance::Dark);
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    attribute,
                    &use_dark_mode as *const _ as *const c_void,
                    mem::size_of_val(&use_dark_mode) as u32,
                );
            }
        }
    }
}

/// Makes the app's menus and the DWM-drawn parts of these windows light or dark.
///
/// Don't call this inside `with_win32_app`, since `WM_THEMECHANGED` re-enters the window proc.
pub fn set_native_appearance(appearance: NativeAppearance, hwnds: impl Iterator<Item = HWND>) {
    let functions = AppearanceFunctions::get_or_load();
    let mode = match appearance {
        NativeAppearance::System => PreferredAppMode::AllowDark,
        NativeAppearance::Light => PreferredAppMode::ForceLight,
        NativeAppearance::Dark => PreferredAppMode::ForceDark,
    };
    unsafe {
        if let Some(set_preferred_app_mode) = functions.set_preferred_app_mode {
            set_preferred_app_mode(mode);
        }
        if let Some(refresh_immersive_color_policy_state) =
            functions.refresh_immersive_color_policy_state
        {
            refresh_immersive_color_policy_state();
        }
        if let Some(flush_menu_themes) = functions.flush_menu_themes {
            flush_menu_themes();
        }
        for hwnd in hwnds {
            functions.apply_to_window(hwnd, appearance);
            // This re-themes the window menu if it's open.
            SendMessageW(hwnd, WM_THEMECHANGED, None, None);
        }
    }
}

/// Makes the DWM-drawn parts of a new window light or dark before it's first shown.
pub fn apply_native_appearance_to_window(hwnd: HWND, appearance: NativeAppearance) {
    AppearanceFunctions::get_or_load().apply_to_window(hwnd, appearance);
}

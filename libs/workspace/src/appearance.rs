//! The standalone UI theme choice. Hosted under the WM the theme arrives
//! through `StudioToApp::Custom` and `Window` installs it; nothing here runs
//! then. Standalone, the choice is persisted in `Settings` and installed
//! before the widgets register so every `theme.*` role comes from the chosen
//! family.

use crate::state::Settings;
use makepad_widgets_core::desktop_style::{self, DesktopStyle, SheetEntry, StyleSheet};
use makepad_widgets_core::*;

/// Picker rows: index 0 follows the host OS, then the sheets of the
/// catalogue a picker offers (`desktop_style::picks`) in order.
/// The labels are made from that same list rather than typed
/// out a second time: a typed copy fell behind the themes, and a row that
/// said macOS picked another sheet.
pub const FOLLOW_HOST: &str = "Follow host OS";

/// What the picker shows, row for row with `sheet_at`.
pub fn picker_labels() -> Vec<String> {
    std::iter::once(FOLLOW_HOST.to_string())
        .chain(picker_sheets().map(|entry| entry.label.to_string()))
        .collect()
}

/// A resolved appearance: one sheet and whether its dark variant is on.
#[derive(Clone, Copy, Debug)]
pub struct ThemeChoice {
    /// The sheet as a picker offers it: never another's dark variant.
    pub sheet: &'static SheetEntry,
    pub dark: bool,
}

impl PartialEq for ThemeChoice {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.sheet, other.sheet) && self.dark == other.dark
    }
}
impl Eq for ThemeChoice {}

impl ThemeChoice {
    /// Follow both the host family and its reported light/dark appearance.
    /// The saved dark flag belongs to an explicit theme selection; it is only
    /// a fallback when this platform cannot report its appearance.
    pub fn from_settings(settings: &Settings, host: DesktopStyle, host_dark: Option<bool>) -> Self {
        let selected = settings
            .theme
            .as_deref()
            .and_then(desktop_style::find)
            .map(|entry| entry.light());
        let sheet = selected.unwrap_or_else(|| host.sheet(false));
        let dark = if selected.is_none() {
            host_dark.unwrap_or(settings.dark)
        } else {
            settings.dark
        };
        Self {
            sheet,
            dark: dark && sheet.dark.is_some(),
        }
    }

    pub fn sheet(self) -> StyleSheet {
        StyleSheet::load(self.entry())
    }

    /// The catalogue entry this choice installs, in its appearance.
    pub fn entry(self) -> &'static SheetEntry {
        self.sheet.with_appearance(self.dark)
    }

    /// The theme name this choice installs (`macos-dark`, `omarchy`, ...).
    pub fn name(self) -> String {
        self.entry().id.to_string()
    }
}

/// Picker index for a settings value (0 = follow host).
/// A saved theme absent from the picker also displays the follow-host row.
pub fn picker_index(settings: &Settings) -> usize {
    settings
        .theme
        .as_deref()
        .and_then(desktop_style::find)
        .and_then(|entry| picker_sheets().position(|s| std::ptr::eq(s, entry.light())))
        .map(|i| i + 1)
        .unwrap_or(0)
}

/// Settings sheet for a picker index (`None` = follow host).
pub fn sheet_at(index: usize) -> Option<&'static SheetEntry> {
    index
        .checked_sub(1)
        .and_then(|i| picker_sheets().nth(i))
}

fn picker_sheets() -> impl Iterator<Item = &'static SheetEntry> {
    desktop_style::picks().into_iter()
}

/// The family a standalone Studio follows on this machine.
pub fn host_family(cx: &Cx) -> DesktopStyle {
    match cx.os_type() {
        OsType::Macos => DesktopStyle::Macos,
        OsType::Windows => DesktopStyle::Windows,
        OsType::Ios(_) => DesktopStyle::Ios,
        OsType::Android(_) => DesktopStyle::Android,
        _ => DesktopStyle::Omarchy,
    }
}

/// Ask the host (the platform: AppKit on the UI thread, or the window system's last appearance), without
/// subprocesses or a worker lock.
/// Windows reads the per-user application theme; other hosts keep the manual fallback.
pub fn host_dark() -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        return makepad_widgets_core::makepad_platform::host_dark();
    }
    #[cfg(target_os = "windows")]
    unsafe {
        use std::ffi::c_void;
        #[link(name = "advapi32")]
        extern "system" {
            fn RegGetValueW(key: *mut c_void, subkey: *const u16, value: *const u16,
                flags: u32, kind: *mut u32, data: *mut c_void, size: *mut u32) -> i32;
        }
        let key: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize".encode_utf16().chain(Some(0)).collect();
        let name: Vec<u16> = "AppsUseLightTheme".encode_utf16().chain(Some(0)).collect();
        let mut value = 1u32;
        let mut size = 4u32;
        let result = RegGetValueW((0x80000001u32 as i32 as isize) as *mut c_void,
            key.as_ptr(), name.as_ptr(), 0x10, std::ptr::null_mut(),
            (&mut value as *mut u32).cast(), &mut size);
        return (result == 0 && size == 4 && value <= 1).then_some(value == 0);
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    None
}

/// Human label for a theme name, for the status strip.
pub fn describe(name: &str) -> String {
    desktop_style::find(name).map_or_else(|| name.to_string(), |entry| entry.label.to_string())
}

/// Standalone first start: install the persisted (or host) theme into
/// this isolate if none is present yet. Hosted processes and reloads leave
/// the installed sheet alone — the WM's theme wins, and a reload re-runs
/// this with the picker's sheet already in place.
pub fn install_initial(vm: &mut ScriptVm, settings: &Settings) {
    if vm.cx().in_makepad_studio() {
        return;
    }
    if desktop_style::current_name(vm).is_some() {
        return;
    }
    let host = host_family(vm.cx());
    let choice = ThemeChoice::from_settings(settings, host, host_dark());
    desktop_style::install(vm, choice.sheet());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_match_the_picker() {
        let labels = picker_labels();
        assert_eq!(labels[0], FOLLOW_HOST);
        assert_eq!(labels.len(), picker_sheets().count() + 1);
        for (i, entry) in picker_sheets().enumerate() {
            assert_eq!(labels[i + 1], entry.label);
            assert!(sheet_at(i + 1).is_some_and(|s| std::ptr::eq(s, entry)));
        }
        assert!(sheet_at(0).is_none());
        assert!(sheet_at(99).is_none());
    }

    #[test]
    fn settings_round_trip_through_the_picker() {
        for (i, entry) in picker_sheets().enumerate() {
            let settings = Settings { theme: Some(entry.id.into()), ..Default::default() };
            assert_eq!(picker_index(&settings), i + 1);
            assert!(sheet_at(picker_index(&settings)).is_some_and(|s| std::ptr::eq(s, entry)));
        }
        let unknown = Settings { theme: Some("blackorange".into()), ..Default::default() };
        assert_eq!(picker_index(&unknown), 0);
        assert_eq!(ThemeChoice::from_settings(&unknown, DesktopStyle::Macos, None).sheet.id, "macos");
        let black_orange = Settings { theme: Some("black-orange".into()), ..Default::default() };
        let row = picker_index(&black_orange);
        assert!(row > 0);
        assert_eq!(sheet_at(row).unwrap().id, "black-orange");
        assert_eq!(ThemeChoice::from_settings(&black_orange, DesktopStyle::Macos, None).sheet.id, "black-orange");

        let s = Settings { theme: Some("nextstep".into()), dark: true, ..Default::default() };
        let row = picker_sheets().position(|entry| entry.id == "nextstep").map(|i| i + 1);
        assert_eq!(Some(picker_index(&s)), row);
        let c = ThemeChoice::from_settings(&s, DesktopStyle::Macos, Some(true));
        assert_eq!(c.sheet.id, "nextstep");
        assert!(!c.dark, "a sheet without a dark variant stays light");
        assert_eq!(c.name(), "nextstep");

        let follow = Settings { theme: None, dark: true, ..Default::default() };
        assert_eq!(picker_index(&follow), 0);
        let c = ThemeChoice::from_settings(&follow, DesktopStyle::Macos, Some(true));
        assert_eq!(c, ThemeChoice { sheet: DesktopStyle::Macos.sheet(false), dark: true });
        assert_eq!(c.name(), "macos-dark");
        assert_eq!(describe("macos-dark"), "macOS dark");
        assert_eq!(describe("windows-2000"), "Windows 2000");

        let junk = Settings { theme: Some("beos".into()), dark: false, ..Default::default() };
        assert_eq!(picker_index(&junk), 0);
        assert_eq!(ThemeChoice::from_settings(&junk, DesktopStyle::Omarchy, None).sheet.id, "omarchy");
    }

    #[test]
    fn follow_host_tracks_appearance_despite_opposite_saved_preference() {
        for saved_dark in [false, true] {
            let settings = Settings { theme: None, dark: saved_dark, ..Default::default() };
            for host_dark in [true, false, true] {
                let choice = ThemeChoice::from_settings(&settings, DesktopStyle::Macos, Some(host_dark));
                assert_eq!(choice.sheet.id, "macos");
                assert_eq!(choice.dark, host_dark);
                assert_eq!(settings.dark, saved_dark, "following does not overwrite the manual preference");
            }
            assert_eq!(ThemeChoice::from_settings(&settings, DesktopStyle::Macos, None).dark, saved_dark);
        }
    }

    #[test]
    fn explicit_themes_keep_manual_appearance_when_host_changes() {
        for entry in desktop_style::picks() {
            for dark in [false, true] {
                let settings = Settings { theme: Some(entry.id.into()), dark, ..Default::default() };
                for host_dark in [Some(true), Some(false), None] {
                    let choice = ThemeChoice::from_settings(&settings, DesktopStyle::Macos, host_dark);
                    assert!(std::ptr::eq(choice.sheet, entry));
                    assert_eq!(choice.dark, dark && entry.dark.is_some());
                }
            }
        }
    }
}

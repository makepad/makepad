//! The standalone OS-style choice. Hosted under the WM the stylesheet arrives
//! through `StudioToApp::Custom` and `Window` installs it; nothing here runs
//! then. Standalone, the choice is persisted in `Settings` and installed
//! before the widgets register so every `theme.*` role comes from the chosen
//! family.

use crate::state::Settings;
use makepad_widgets::desktop_style::{self, DesktopStyle, StyleSheet};
use makepad_widgets::*;

/// Picker rows: index 0 follows the host OS, then `DesktopStyle::ALL` in
/// order. The Settings panel's `DropDown` labels are spelled out in Splash
/// (so a stylesheet reapply keeps them); `labels_match_the_picker` pins the
/// two lists together.
pub const FOLLOW_HOST: &str = "Follow host OS";
pub const PICKER_LABELS: [&str; 8] = [
    FOLLOW_HOST,
    "Omarchy",
    "macOS",
    "Windows",
    "Windows 2000",
    "NeXTSTEP",
    "iOS",
    "Android",
];

/// A resolved appearance: one family and whether its dark variant is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StyleChoice {
    pub family: DesktopStyle,
    pub dark: bool,
}

impl StyleChoice {
    /// Follow both the host family and its reported light/dark appearance.
    /// The saved dark flag belongs to an explicit style selection; it is only
    /// a fallback when this platform cannot report its appearance.
    pub fn from_settings(settings: &Settings, host: DesktopStyle, host_dark: Option<bool>) -> Self {
        let selected = settings
            .style
            .as_deref()
            .and_then(DesktopStyle::parse);
        let family = selected.unwrap_or(host);
        let dark = if selected.is_none() {
            host_dark.unwrap_or(settings.dark)
        } else {
            settings.dark
        };
        Self {
            family,
            dark: dark && family.supports_dark(),
        }
    }

    pub fn sheet(self) -> StyleSheet {
        StyleSheet::load_with_appearance(self.family, self.dark)
    }

    /// The stylesheet name this choice installs (`macos-dark`, `omarchy`, ...).
    pub fn name(self) -> String {
        self.sheet_name()
    }

    fn sheet_name(self) -> String {
        if self.dark {
            format!("{}-dark", self.family.id())
        } else {
            self.family.id().to_string()
        }
    }
}

/// Picker index for a settings value (0 = follow host).
pub fn picker_index(settings: &Settings) -> usize {
    settings
        .style
        .as_deref()
        .and_then(DesktopStyle::parse)
        .and_then(|f| DesktopStyle::ALL.iter().position(|s| *s == f))
        .map(|i| i + 1)
        .unwrap_or(0)
}

/// Settings family for a picker index (`None` = follow host).
pub fn family_at(index: usize) -> Option<DesktopStyle> {
    index
        .checked_sub(1)
        .and_then(|i| DesktopStyle::ALL.get(i).copied())
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

/// Query AppKit on the UI thread, without subprocesses or a worker lock.
/// Matching Aqua/DarkAqua also handles the accessibility contrast variants.
/// Other platforms retain their existing manual appearance fallback.
pub fn host_dark() -> Option<bool> {
    #[cfg(target_os = "macos")]
    unsafe {
        use makepad_widgets::makepad_platform::os::apple::apple_sys::*;
        extern "C" {
            static NSAppearanceNameAqua: ObjcId;
            static NSAppearanceNameDarkAqua: ObjcId;
        }
        // Library/style tests can evaluate Splash off the main thread. They
        // must not create an AppKit application there.
        let main_thread: bool = msg_send![class!(NSThread), isMainThread];
        if !main_thread {
            return None;
        }
        let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
        let app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
        let appearance: ObjcId = msg_send![app, effectiveAppearance];
        let names = [NSAppearanceNameAqua, NSAppearanceNameDarkAqua];
        let choices: ObjcId = msg_send![class!(NSArray), arrayWithObjects: names.as_ptr() count: names.len()];
        let best: ObjcId = msg_send![appearance, bestMatchFromAppearancesWithNames: choices];
        let result = if best.is_null() {
            None
        } else {
            Some(msg_send![best, isEqualToString: NSAppearanceNameDarkAqua])
        };
        let () = msg_send![pool, drain];
        return result;
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// Human label for a stylesheet name, for the status strip.
pub fn describe(name: &str) -> String {
    let dark = name.ends_with("-dark");
    match DesktopStyle::parse(name) {
        Some(family) if dark => format!("{} (dark)", family.label()),
        Some(family) => family.label().to_string(),
        None => name.to_string(),
    }
}

/// Standalone first start: install the persisted (or host) stylesheet into
/// this isolate if none is present yet. Hosted processes and reloads leave
/// the installed sheet alone — the WM's style wins, and a reload re-runs
/// this with the picker's sheet already in place.
pub fn install_initial(vm: &mut ScriptVm, settings: &Settings) {
    if vm.cx().in_makepad_studio() {
        return;
    }
    if desktop_style::current_name(vm).is_some() {
        return;
    }
    let host = host_family(vm.cx());
    let choice = StyleChoice::from_settings(settings, host, host_dark());
    desktop_style::install(vm, choice.sheet());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_match_the_picker() {
        assert_eq!(PICKER_LABELS[0], FOLLOW_HOST);
        for (i, style) in DesktopStyle::ALL.iter().enumerate() {
            assert_eq!(PICKER_LABELS[i + 1], style.label());
            assert_eq!(family_at(i + 1), Some(*style));
        }
        assert_eq!(family_at(0), None);
        assert_eq!(family_at(99), None);
    }

    #[test]
    fn settings_round_trip_through_the_picker() {
        let s = Settings { style: Some("nextstep".into()), dark: true };
        assert_eq!(picker_index(&s), 5);
        let c = StyleChoice::from_settings(&s, DesktopStyle::Macos, Some(true));
        assert_eq!(c.family, DesktopStyle::NextStep);
        assert!(!c.dark, "a family without a dark variant stays light");
        assert_eq!(c.name(), "nextstep");

        let follow = Settings { style: None, dark: true };
        assert_eq!(picker_index(&follow), 0);
        let c = StyleChoice::from_settings(&follow, DesktopStyle::Macos, Some(true));
        assert_eq!(c, StyleChoice { family: DesktopStyle::Macos, dark: true });
        assert_eq!(c.name(), "macos-dark");
        assert_eq!(describe("macos-dark"), "macOS (dark)");
        assert_eq!(describe("windows-2000"), "Windows 2000");

        let junk = Settings { style: Some("beos".into()), dark: false };
        assert_eq!(picker_index(&junk), 0);
        assert_eq!(StyleChoice::from_settings(&junk, DesktopStyle::Omarchy, None).family, DesktopStyle::Omarchy);
    }

    #[test]
    fn follow_host_tracks_appearance_despite_opposite_saved_preference() {
        for saved_dark in [false, true] {
            let settings = Settings { style: None, dark: saved_dark };
            for host_dark in [true, false, true] {
                let choice = StyleChoice::from_settings(&settings, DesktopStyle::Macos, Some(host_dark));
                assert_eq!(choice.family, DesktopStyle::Macos);
                assert_eq!(choice.dark, host_dark);
                assert_eq!(settings.dark, saved_dark, "following does not overwrite the manual preference");
            }
            assert_eq!(StyleChoice::from_settings(&settings, DesktopStyle::Macos, None).dark, saved_dark);
        }
    }

    #[test]
    fn explicit_styles_keep_manual_appearance_when_host_changes() {
        for family in DesktopStyle::ALL {
            for dark in [false, true] {
                let settings = Settings { style: Some(family.id().into()), dark };
                for host_dark in [Some(true), Some(false), None] {
                    let choice = StyleChoice::from_settings(&settings, DesktopStyle::Macos, host_dark);
                    assert_eq!(choice.family, family);
                    assert_eq!(choice.dark, dark && family.supports_dark());
                }
            }
        }
    }
}

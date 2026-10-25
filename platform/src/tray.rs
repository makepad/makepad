//! The system tray (macOS status item, Windows notification area): an icon with a tooltip and a
//! menu that lives outside the app's windows. `Cx::set_tray` creates or updates one in place,
//! `Cx::remove_tray` removes it; a chosen item arrives as `Event::TrayMenu`, a primary click
//! (Windows) as `Event::TrayClick`. While a tray exists, closing the last window does not end
//! the app: it keeps running from the tray until it quits.

use crate::{cx::Cx, cx_api::CxOsOp, makepad_live_id::LiveId};

#[derive(Clone, Debug, PartialEq)]
pub struct TrayIcon {
    /// The app's id for this tray: events name it, `set_tray` with the same id updates it.
    pub id: LiveId,
    pub image: TrayImage,
    pub tooltip: String,
    pub menu: Vec<TrayMenuItem>,
}

/// A monochrome glyph: one alpha byte per pixel, rows top-down. macOS shows it as a template
/// image (it follows the light / dark menu bar); Windows draws it white or black from the
/// taskbar's theme. 36x36 suits a @2x menu bar, 32x32 the Windows tray.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrayImage {
    pub width: u32,
    pub height: u32,
    pub alpha: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TrayMenuItem {
    /// `checked: Some(on)` shows a check mark state.
    Item { id: LiveId, label: String, enabled: bool, checked: Option<bool> },
    Line,
}

impl TrayMenuItem {
    pub fn item(id: LiveId, label: impl Into<String>) -> Self {
        Self::Item { id, label: label.into(), enabled: true, checked: None }
    }

    pub fn check(id: LiveId, label: impl Into<String>, checked: bool) -> Self {
        Self::Item { id, label: label.into(), enabled: true, checked: Some(checked) }
    }
}

/// A tray menu item was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrayMenuEvent {
    pub tray: LiveId,
    pub item: LiveId,
}

/// A primary click on a tray icon, where the system gives one (Windows left click; macOS opens
/// the menu instead).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrayClickEvent {
    pub tray: LiveId,
}

impl Cx {
    /// Creates tray `tray.id`, or updates it in place. Where the desktop has no tray yet (Linux,
    /// until our WM hosts trays; mobile; web) the shell drops it and `has_tray` stays false, so
    /// the app still ends with its last window.
    pub fn set_tray(&mut self, tray: TrayIcon) {
        if self.script_data.std.host_io_only() {
            return;
        }
        if cfg!(any(target_os = "macos", target_os = "windows")) && !self.trays.contains(&tray.id) {
            self.trays.push(tray.id);
        }
        self.platform_ops.push_back(CxOsOp::SetTray(tray));
    }

    pub fn remove_tray(&mut self, id: LiveId) {
        self.trays.retain(|t| *t != id);
        self.platform_ops.push_back(CxOsOp::RemoveTray(id));
    }

    /// Whether the app has a tray the desktop shows (it then outlives its last window).
    pub fn has_tray(&self) -> bool {
        !self.trays.is_empty()
    }
}

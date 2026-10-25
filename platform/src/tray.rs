//! The system tray (macOS status item, Windows notification area, the makepad WM's tray area on
//! Linux): an icon with a tooltip and a menu that lives outside the app's windows.
//! `Cx::set_tray` creates or updates one in place, `Cx::remove_tray` removes it; a chosen item
//! arrives as `Event::TrayMenu`, a primary click (Windows) as `Event::TrayClick`. While a tray
//! exists, closing the last window does not end the app: it keeps running from the tray until it
//! quits.
//!
//! On the makepad WM the tray travels as a hosted relay (`ChildRelay::SetTray`, from the HAL's
//! WM shell or a hosted child): the WM draws the glyph in its bar's tray area tinted with its
//! shell tokens, shows the tooltip, and opens the menu on a primary click (as macOS does; it
//! sends no `TrayClick`). A tray app outlives a WM restart: its windows and trays come back on
//! the next WM.

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
    /// Creates tray `tray.id`, or updates it in place. Where the desktop has no tray (Linux
    /// outside the makepad WM; mobile; web) the shell drops it and `has_tray` stays false, so the
    /// app still ends with its last window.
    pub fn set_tray(&mut self, tray: TrayIcon) {
        if self.script_data.std.host_io_only() {
            return;
        }
        let shown = cfg!(any(target_os = "macos", target_os = "windows"))
            || (cfg!(target_os = "linux") && crate::app_main::hosted_by_window_manager());
        if shown && !self.trays.contains(&tray.id) {
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

/// The tray as a hosted relay carries it to the host (`ChildRelay::SetTray`).
pub(crate) fn relay_tray(tray: &TrayIcon) -> crate::studio::RelayTray {
    use crate::studio::RelayTrayItem;
    let items = tray
        .menu
        .iter()
        .map(|item| match item {
            TrayMenuItem::Item { id, label, enabled, checked } => {
                RelayTrayItem { id: id.0, label: label.clone(), enabled: *enabled, checked: *checked, line: false }
            }
            TrayMenuItem::Line => RelayTrayItem { line: true, ..Default::default() },
        })
        .collect();
    let pixels = tray.image.width as usize * tray.image.height as usize;
    let (width, height) = if tray.image.alpha.len() >= pixels { (tray.image.width, tray.image.height) } else { (0, 0) };
    crate::studio::RelayTray {
        tray: tray.id.0,
        width,
        height,
        alpha: tray.image.alpha[..width as usize * height as usize].to_vec(),
        tooltip: tray.tooltip.clone(),
        items,
    }
}

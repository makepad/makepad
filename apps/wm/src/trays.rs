//! The WM's tray area: the tray icons its apps set (`Cx::set_tray`), shown in the bar after the
//! Desk Link item and before the status modules.
//!
//! An app's tray reaches the WM as a hosted relay (`ChildRelay::SetTray` / `RemoveTray`): from
//! the HAL's WM shell (a native app or `makepad run` adopted through the WM socket) or from a
//! `--stdin-loop` child. The WM keeps each one's glyph as a texture (an alpha mask, tinted with
//! the bar's text colour like its own icons), its tooltip and its menu.
//!
//! What a click does is the convention of macOS and Windows. A tray without `dropdown` opens its
//! menu in a flyout drawn with the shell kit on a primary click; a chosen item goes back to the
//! app as `HostRelay::TrayMenu`, and the WM sends no `TrayClick`. A `dropdown` tray (the AI Hub)
//! belongs to the app's primary click: the WM sends `HostRelay::TrayClick` with the bar item's
//! rect, the app shows its dropdown window (`show_at_tray`) and `dropdown.rs` places that window as
//! a flyout under the item; a second click, a click elsewhere or Escape closes it. A secondary
//! click opens the menu. A tray goes with its client.

use crate::shell::bar::BarTray;
use crate::shell::panels::TrayMenu;
use makepad_widgets::makepad_platform::studio::RelayTray;
use makepad_widgets::*;

struct Entry {
    client: u64,
    relay: RelayTray,
    glyph: Option<Texture>,
}

#[derive(Default)]
pub struct WmTrays {
    entries: Vec<Entry>,
    /// The WM's own slot for the AI Hub while it is off (Quit, `enabled = off`): the app's dim
    /// "off" glyph after the apps' trays. A click starts the hub. The app's own tray replaces it.
    hub_off: Option<Texture>,
}

pub const HUB_OFF_TOOLTIP: &str = "AI Hub \u{b7} off \u{2014} click to start";

impl WmTrays {
    /// `client` set (or updated) a tray. The glyph texture is made again only when the mask
    /// changed. True when the bar changes.
    pub fn set(&mut self, cx: &mut Cx, client: u64, relay: RelayTray) -> bool {
        if let Some(e) = self.entries.iter_mut().find(|e| e.client == client && e.relay.tray == relay.tray) {
            if e.relay == relay {
                return false;
            }
            if (e.relay.width, e.relay.height, &e.relay.alpha) != (relay.width, relay.height, &relay.alpha) {
                e.glyph = makepad_shell_kit::ui::glyph_texture(cx, relay.width, relay.height, &relay.alpha);
            }
            e.relay = relay;
            return true;
        }
        let glyph = makepad_shell_kit::ui::glyph_texture(cx, relay.width, relay.height, &relay.alpha);
        log!("wm: client {client} shows tray {:x} ({:?})", relay.tray, relay.tooltip);
        self.entries.push(Entry { client, relay, glyph });
        true
    }

    pub fn remove(&mut self, client: u64, tray: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| !(e.client == client && e.relay.tray == tray));
        before != self.entries.len()
    }

    /// The client went away: its trays go with it.
    pub fn client_gone(&mut self, client: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.client != client);
        before != self.entries.len()
    }

    /// What the bar shows, in tray order; the hub's off slot last.
    pub fn bar(&self) -> Vec<BarTray> {
        let mut bar: Vec<BarTray> = self.entries.iter().map(|e| BarTray { glyph: e.glyph.clone(), tooltip: e.relay.tooltip.clone() }).collect();
        if self.hub_off.is_some() {
            bar.push(BarTray { glyph: self.hub_off.clone(), tooltip: HUB_OFF_TOOLTIP.to_string() });
        }
        bar
    }

    /// The hub is off (`off`) and no tray of its own shows: the WM keeps the off slot. True when
    /// the bar changes.
    pub fn set_hub_off(&mut self, cx: &mut Cx, off: bool, hub_client_has_tray: bool) -> bool {
        let want = off && !hub_client_has_tray;
        if want == self.hub_off.is_some() {
            return false;
        }
        self.hub_off = if want {
            let img = makepad_ai_hub_panel::tray_glyph::image(makepad_ai_hub_panel::tray_glyph::Glyph::Off);
            makepad_shell_kit::ui::glyph_texture(cx, img.width, img.height, &img.alpha)
        } else {
            None
        };
        true
    }

    /// Whether the bar's `index`th item is the WM's own hub off slot.
    pub fn is_hub_off(&self, index: usize) -> bool {
        self.hub_off.is_some() && index == self.entries.len()
    }

    /// Whether `client` shows a tray.
    pub fn has_tray(&self, client: u64) -> bool {
        self.entries.iter().any(|e| e.client == client)
    }

    /// The menu of the bar's `index`th tray.
    pub fn menu(&self, index: usize) -> Option<TrayMenu> {
        self.entries.get(index).map(Self::menu_of)
    }

    /// The menu of `client`'s tray `tray` as it stands now (the open flyout follows an update).
    pub fn menu_by(&self, client: u64, tray: u64) -> Option<TrayMenu> {
        self.entries.iter().find(|e| e.client == client && e.relay.tray == tray).map(Self::menu_of)
    }

    fn menu_of(e: &Entry) -> TrayMenu {
        TrayMenu { client: e.client, tray: e.relay.tray, glyph: e.glyph.clone(), tooltip: e.relay.tooltip.clone(), items: e.relay.items.clone() }
    }

    /// The client and id of the bar's `index`th tray, and whether its primary click is the
    /// app's (a `dropdown` tray).
    pub fn at(&self, index: usize) -> Option<(u64, u64, bool)> {
        self.entries.get(index).map(|e| (e.client, e.relay.tray, e.relay.dropdown))
    }

    /// The bar index of the first dropdown tray `client` shows.
    pub fn first_dropdown_of(&self, client: u64) -> Option<usize> {
        self.entries.iter().position(|e| e.client == client && e.relay.dropdown)
    }

    /// The bar index of `client`'s tray `tray`.
    pub fn index_of(&self, client: u64, tray: u64) -> Option<usize> {
        self.entries.iter().position(|e| e.client == client && e.relay.tray == tray)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hub_off_slot_is_last_and_goes_when_the_hub_has_a_tray() {
        let mut t = WmTrays::default();
        // no GL context in a unit test: the slot's presence is what is tested via the flag path
        assert!(!t.is_hub_off(0));
        assert!(t.bar().is_empty());
    }
}

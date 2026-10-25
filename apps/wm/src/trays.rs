//! The WM's tray area: the tray icons its apps set (`Cx::set_tray`), shown in the bar after the
//! Desk Link item and before the status modules.
//!
//! An app's tray reaches the WM as a hosted relay (`ChildRelay::SetTray` / `RemoveTray`): from
//! the HAL's WM shell (a native app or `makepad run` adopted through the WM socket) or from a
//! `--stdin-loop` child. The WM keeps each one's glyph as a texture (an alpha mask, tinted with
//! the bar's text colour like its own icons), its tooltip and its menu. A primary click opens
//! the menu in a flyout drawn with the shell kit (as macOS opens a status item's menu); a chosen
//! item goes back to the app as `HostRelay::TrayMenu`. The WM sends no `TrayClick`: an app puts
//! its primary action in the menu ("Open AI Hub"). A tray goes with its client.

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
}

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

    /// What the bar shows, in tray order.
    pub fn bar(&self) -> Vec<BarTray> {
        self.entries.iter().map(|e| BarTray { glyph: e.glyph.clone(), tooltip: e.relay.tooltip.clone() }).collect()
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

    /// The bar index of `client`'s tray `tray`.
    pub fn index_of(&self, client: u64, tray: u64) -> Option<usize> {
        self.entries.iter().position(|e| e.client == client && e.relay.tray == tray)
    }
}

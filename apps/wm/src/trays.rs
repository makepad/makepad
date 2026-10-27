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

/// A service the WM supervises whose tray it keeps while the service is off (its Quit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    AiHub,
    Devhub,
}

impl Service {
    fn tooltip(self) -> &'static str {
        match self {
            Service::AiHub => "AI Hub \u{b7} off \u{2014} click to start",
            Service::Devhub => "devhub \u{b7} off \u{2014} click to start",
        }
    }

    /// The service's own dim "off" glyph.
    fn glyph(self, cx: &mut Cx) -> Option<Texture> {
        match self {
            Service::AiHub => {
                let img = makepad_ai_hub_panel::tray_glyph::image(makepad_ai_hub_panel::tray_glyph::Glyph::Off);
                makepad_shell_kit::ui::glyph_texture(cx, img.width, img.height, &img.alpha)
            }
            Service::Devhub => {
                // the app's Off glyph (hollow pupil) at half strength: dim like the AI Hub's
                use makepad_devhub::tray_glyph::{alpha, Glyph, SIZE};
                let dim: Vec<u8> = alpha(Glyph::Off).iter().map(|a| a / 2).collect();
                makepad_shell_kit::ui::glyph_texture(cx, SIZE, SIZE, &dim)
            }
        }
    }
}

#[derive(Default)]
pub struct WmTrays {
    entries: Vec<Entry>,
    /// The WM's own slots for its services while they are off (Quit, `enabled = off`): each
    /// one's dim "off" glyph after the apps' trays, AI Hub first. A click starts the service; its
    /// own tray replaces the slot.
    off: Vec<(Service, Option<Texture>)>,
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

    /// What the bar shows, in tray order; the services' off slots last.
    pub fn bar(&self) -> Vec<BarTray> {
        let mut bar: Vec<BarTray> = self.entries.iter().map(|e| BarTray { glyph: e.glyph.clone(), tooltip: e.relay.tooltip.clone() }).collect();
        bar.extend(self.off.iter().map(|(service, glyph)| BarTray { glyph: glyph.clone(), tooltip: service.tooltip().to_string() }));
        bar
    }

    /// `service` is off (`off`) and no tray of its own shows: the WM keeps its off slot. True
    /// when the bar changes.
    pub fn set_off(&mut self, cx: &mut Cx, service: Service, off: bool, service_has_tray: bool) -> bool {
        let want = off && !service_has_tray;
        if want == self.off.iter().any(|(s, _)| *s == service) {
            return false;
        }
        if want {
            let glyph = service.glyph(cx);
            self.off.push((service, glyph));
            self.off.sort_by_key(|(s, _)| *s as u8);
        } else {
            self.off.retain(|(s, _)| *s != service);
        }
        true
    }

    /// The service whose off slot is the bar's `index`th item.
    pub fn off_at(&self, index: usize) -> Option<Service> {
        index.checked_sub(self.entries.len()).and_then(|i| self.off.get(i)).map(|(s, _)| *s)
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
    fn the_off_slots_are_last_in_service_order() {
        let mut t = WmTrays::default();
        assert_eq!(t.off_at(0), None);
        assert!(t.bar().is_empty());
        // no GL context in a unit test: the slots without their textures
        t.off.push((Service::Devhub, None));
        t.off.push((Service::AiHub, None));
        t.off.sort_by_key(|(s, _)| *s as u8);
        assert_eq!(t.off_at(0), Some(Service::AiHub));
        assert_eq!(t.off_at(1), Some(Service::Devhub));
        assert_eq!(t.off_at(2), None);
        assert_eq!(t.bar()[1].tooltip, "devhub \u{b7} off \u{2014} click to start");
    }
}

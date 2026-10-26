//! A tray app's dropdown on the WM: the AI Hub's panel as a flyout under its tray item.
//!
//! A `dropdown` tray (`TrayIcon::dropdown`, `RelayTray::dropdown`) belongs to its app's primary
//! click. The press on the bar item sends the app `HostRelay::TrayClick` with the item's rect;
//! the app answers by showing its dropdown window (`WindowHandle::show_at_tray`), which the HAL
//! WM shell announces as `HostedDropdown { op: SHOW }` with the size it wants, and the WM
//! places it: the app's client (a supervised service, `ClientSlot::service`, which has no tile
//! and is in no layout) joins the layout of the item's screen as a flyout float, borderless
//! (`WmState::dropdown_client`), centred under the item and clamped into the screen like the
//! bar's own flyouts (`ShellPanel::flyout_rect`), above every window on every workspace, never
//! a tile, never in the dock, never the keyboard's target.
//!
//! It closes, and the app hears `HostedDropdown { op: DISMISS }` (its `Event::PopupDismissed`),
//! when the person presses anywhere outside it, presses Escape, presses the bar item again (the
//! WM answers that press itself: no second `TrayClick`), or another window takes the focus. The
//! app hiding the window itself (`HostedDropdown { op: HIDE }`) closes it as well. A second
//! window of a service is not a dropdown's business: a service has the dropdown only.

use super::*;
use makepad_widgets::makepad_platform::studio::{dropdown_op, HostRelay, HostedDropdown};

/// The primary click the WM sent the app, until the app shows its dropdown (or the next click).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pending {
    client: ClientId,
    tray: u64,
    seg: usize,
    anchor: Rect,
}

/// The dropdown on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Open {
    pub client: ClientId,
    pub window_id: usize,
    pub tray: u64,
    /// The bar segment (screen) of the item it hangs under.
    pub seg: usize,
    /// The bar item's rect.
    pub anchor: Rect,
    /// The flyout's rect.
    pub card: Rect,
}

#[derive(Default)]
pub(crate) struct Dropdowns {
    pending: Option<Pending>,
    pub open: Option<Open>,
}

impl App {
    /// A primary press on the bar's `index`th tray item (segment `seg`). A tray whose primary
    /// click is its menu opens it; a dropdown tray toggles the app's dropdown.
    pub(super) fn tray_press(&mut self, cx: &mut Cx, seg: usize, index: usize) {
        if let Some(service) = self.trays.off_at(index) {
            return self.start_service_from_tray(cx, service);
        }
        let Some((client, tray, dropdown)) = self.trays.at(index) else { return };
        log!("wm: primary press on tray item {index} (client {client}, tray {tray:x}, dropdown {dropdown})");
        if !dropdown {
            self.toggle_shell_panel(cx, seg, BarModule::Tray(index));
            return;
        }
        // The press on the item of the open dropdown closes it (and is not an open).
        if self.dropdown.open.is_some_and(|o| o.client == client && o.tray == tray) {
            self.dismiss_dropdown(cx, "its tray item pressed again");
            return;
        }
        self.dismiss_dropdown(cx, "another tray item pressed");
        self.close_shell_panel(cx);
        let anchor = self.tray_item_rect(cx, seg, index);
        let target = self.surface_target(self.bar_screen(seg));
        let screen = match target {
            Some(screen) => screen,
            None => self.ui.widget(cx, ids!(shell_panel)).borrow::<shell::panels::ShellPanel>().map(|p| p.flyout_rect(anchor, None, 0.0, 0.0).1).unwrap_or_default(),
        };
        self.dropdown.pending = Some(Pending { client, tray, seg, anchor });
        let rect = |r: Rect| [r.pos.x, r.pos.y, r.size.x, r.size.y];
        self.send_relays(vec![(client, HostRelay::TrayClick { tray, anchor: rect(anchor), screen: rect(screen) })]);
    }

    /// A secondary press on the bar's `index`th tray item: its menu.
    pub(super) fn tray_menu_press(&mut self, cx: &mut Cx, seg: usize, index: usize) {
        if let Some(service) = self.trays.off_at(index) {
            return self.start_service_from_tray(cx, service);
        }
        if self.trays.at(index).is_none() {
            return;
        }
        self.dismiss_dropdown(cx, "its menu opened");
        self.toggle_shell_panel(cx, seg, BarModule::Tray(index));
    }

    /// A service's off slot was pressed: `enabled = on` and start it (the launcher's start); the
    /// slot goes and the app's own tray takes its place once it runs.
    fn start_service_from_tray(&mut self, cx: &mut Cx, service: crate::trays::Service) {
        self.dismiss_dropdown(cx, "a service's off slot pressed");
        self.close_shell_panel(cx);
        self.launch_app(cx, match service {
            crate::trays::Service::AiHub => crate::clients::HUB_SERVICE_APP,
            crate::trays::Service::Devhub => crate::clients::DEVHUB_SERVICE_APP,
        });
    }

    fn tray_item_rect(&self, cx: &mut Cx, seg: usize, index: usize) -> Rect {
        let bar = self.ui.widget(cx, ids!(shell_bar));
        let borrowed = bar.borrow::<shell::bar::ShellBar>();
        borrowed.as_ref().and_then(|b| b.module_rect(seg, BarModule::Tray(index))).unwrap_or_default()
    }

    fn close_shell_panel(&mut self, cx: &mut Cx) {
        let closed = match self.ui.widget(cx, ids!(shell_panel)).borrow_mut::<shell::panels::ShellPanel>() {
            Some(mut p) if p.open.is_some() => {
                p.close(cx);
                true
            }
            _ => false,
        };
        if closed {
            self.shell_panel_open = None;
            self.update_bar(cx);
        }
    }

    /// What the app said of its dropdown window (`HostedDropdown`, from its HAL WM shell).
    pub(super) fn dropdown_message(&mut self, cx: &mut Cx, client: ClientId, d: HostedDropdown) {
        match d.op {
            dropdown_op::SHOW => self.dropdown_show(cx, client, d),
            dropdown_op::HIDE => self.dropdown_end(cx, client),
            _ => {}
        }
    }

    fn dropdown_show(&mut self, cx: &mut Cx, client: ClientId, d: HostedDropdown) {
        // Where it hangs: under the item the person pressed; again while open (a new size),
        // under the same item; shown on the app's own account, under its first dropdown tray.
        let (tray, seg, anchor) = match (self.dropdown.open, self.dropdown.pending) {
            (Some(o), _) if o.client == client => (o.tray, o.seg, o.anchor),
            (_, Some(p)) if p.client == client => (p.tray, p.seg, p.anchor),
            _ => {
                let Some(index) = self.trays.first_dropdown_of(client) else {
                    log!("wm: client {client} shows a dropdown but has no dropdown tray");
                    return;
                };
                let seg = self.active_bar_segment();
                (self.trays.at(index).map_or(d.tray, |t| t.1), seg, self.tray_item_rect(cx, seg, index))
            }
        };
        self.dropdown.pending = None;
        let target = self.surface_target(self.bar_screen(seg));
        let Some((card, _)) = self.ui.widget(cx, ids!(shell_panel)).borrow::<shell::panels::ShellPanel>().map(|p| p.flyout_rect(anchor, target, d.w, d.h)) else { return };
        let screen_index = self.bar_screen(seg);
        let rect = LRect::new(card.pos.x, card.pos.y, card.size.x, card.size.y);
        let state = self.state_mut();
        if let Some(old) = state.dropdown_client.filter(|old| *old != client) {
            state.layout_of_mut(old).remove(old);
        }
        let ws = state.layout_at(screen_index).active;
        state.layout_of_mut(client).remove(client);
        state.layout_at_mut(screen_index).add_flyout_float(client, rect, ws);
        state.dropdown_client = Some(client);
        self.dropdown.open = Some(Open { client, window_id: d.window_id, tray, seg, anchor, card });
        self.desk(cx).borrow_mut::<WmDesk>().map(|mut desk| desk.with_tile(cx, client, |_cx, v| v.set_takes_key_focus(false)));
        log!("wm: client {client}'s dropdown {:.0}x{:.0} under its tray item at {:.0},{:.0}", card.size.x, card.size.y, card.pos.x, card.pos.y);
        self.redraw_all(cx);
    }

    /// The app hid its dropdown, or its client went: the flyout leaves.
    pub(super) fn dropdown_end(&mut self, cx: &mut Cx, client: ClientId) {
        self.dropdown.pending = self.dropdown.pending.filter(|p| p.client != client);
        if self.dropdown.open.is_none_or(|o| o.client != client) {
            return;
        }
        self.dropdown.open = None;
        let state = self.state_mut();
        state.dropdown_client = None;
        state.layout_of_mut(client).remove(client);
        self.redraw_all(cx);
    }

    /// The person turned away from the open dropdown: it leaves and the app is told.
    pub(super) fn dismiss_dropdown(&mut self, cx: &mut Cx, why: &str) {
        self.dropdown.pending = None;
        let Some(open) = self.dropdown.open else { return };
        log!("wm: client {}'s dropdown closed ({why})", open.client);
        self.dropdown_end(cx, open.client);
        let sender = self.state_mut().clients.get(&open.client).and_then(|slot| slot.sender.clone());
        if let Some(sender) = sender {
            let dismiss = HostedDropdown::new(open.window_id, dropdown_op::DISMISS);
            send_to_app(&sender, vec![StudioToApp::Custom(dismiss.to_json())]);
        }
    }

    /// Escape and a press outside close the open dropdown. True for the event it took (Escape);
    /// a press goes on to what it hit, as on every desktop's menus.
    pub(super) fn dropdown_event(&mut self, cx: &mut Cx, event: &Event) -> bool {
        let Some(open) = self.dropdown.open else { return false };
        match event {
            Event::KeyDown(e) if e.key_code == KeyCode::Escape => {
                self.dismiss_dropdown(cx, "Escape");
                true
            }
            // The bar item itself toggles (`tray_press`).
            Event::MouseDown(e) if !open.card.contains(e.abs) && !open.anchor.contains(e.abs) => {
                self.dismiss_dropdown(cx, "a press elsewhere");
                false
            }
            _ => false,
        }
    }

    /// Another window took the focus: the dropdown has lost the person's attention.
    pub(super) fn dropdown_focus_moved(&mut self, cx: &mut Cx, to: ClientId) {
        if self.dropdown.open.is_some_and(|o| o.client != to) {
            self.dismiss_dropdown(cx, "another window took the focus");
        }
    }
}

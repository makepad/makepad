//! Desktop style orchestration. Application state stays in the existing clients;
//! shell presentation and the compositor's frozen framebuffer change together.
use crate::desk::ChromeHit;
use crate::desktop::{DesktopShelf, DesktopStyle, ShelfHit};
use crate::*;

/// Match the browser's initial page palette, including light Omarchy themes
/// and classic desktops that have no dark variant.
pub(super) fn browser_appearance(style: DesktopStyle, dark: bool, omarchy_source: &str) -> bool {
    if style == DesktopStyle::Omarchy {
        return scan_theme_color(omarchy_source, "background")
            .map(|c| 0.2126*c.x + 0.7152*c.y + 0.0722*c.z < 0.5)
            .unwrap_or(true);
    }
    style.supports_dark() && dark
}

/// How long a shelf called out from the dock stays out when the pointer is not on it (s).
const SHELF_CALL_HOLD: f64 = 2.0;

impl App {
    /// The family's own sheet.
    pub(super) fn set_desktop_style(&mut self, cx: &mut Cx, style: DesktopStyle) {
        self.set_desktop_sheet(cx, style.sheet(false));
    }
    /// A sheet of the catalogue, in the appearance the toggle has, laid out
    /// as its family.
    pub(super) fn set_desktop_sheet(&mut self, cx: &mut Cx, entry: &'static desktop_style::SheetEntry) {
        let style = entry.family;
        let previous = self.state_mut().style.target;
        let changes_size = previous.mobile() != style.mobile() || (style.mobile() && previous != style);
        self.close_shell_menu(cx);
        if let Some(mut scene) = self
            .ui
            .widget(cx, ids!(scene))
            .borrow_mut::<scene::WmScene>()
        {
            if changes_size { scene.cut(cx); } else { scene.transition(cx); }
        }
        self.drag = None;
        self.div_drag = None;
        self.state_mut().snap.clear();
        cx.stop_timer(self.snap_hover_timer);
        let area = self.desk_area(cx);
        // Every screen's layout switches presentation, each in its own area.
        let screen_count = self.state_mut().screens.screens.len();
        let areas: Vec<LRect> = (0..screen_count).map(|i| self.screen_area(cx, i)).collect();
        let dark = self.state_mut().style.dark;
        let sheet = crate::clients::with_app_artwork(desktop_style::StyleSheet::load(entry.light().with_appearance(dark)));
        if let Some(mut desk) = self.desk(cx).borrow_mut::<WmDesk>() {desk.set_startup_style(cx, &sheet);}
        let sheet_name = sheet.name.clone();
        let sheet_ground = sheet.theme_color("color_bg_app");
        app_icon::install(cx, style, &sheet.icons);
        // Tiling and the shelves exclude each other: parked windows come back full size
        // while the shelves still say where they are shown, then the shelves go off. The
        // user's shelf setting (`warp.mode`) is kept for the next floating style.
        if !style.floating() && previous.floating() {
            self.unpark_all(cx);
        }
        let state = self.state_mut();
        state.dragging.clear();
        state.style.select(style);
        state.warp.tiling = !style.floating();
        if changes_size { state.style.step(1.0); }
        let shelf_reserve = state.shelf_reserve();
        for (i, s) in state.screens.screens.iter_mut().enumerate() {
            s.layout.desktop.enabled = style.floating();
            s.layout.desktop.shelf_reserve = shelf_reserve;
            let a = areas.get(i).copied().unwrap_or(area);
            for c in s.layout.all_clients() {
                s.layout.ensure_desktop(c, a);
            }
        }
        // Send the complete stylesheet, so already-running and warm applications
        // receive exactly the version selected in the WM, even across hosts.
        let json = sheet.to_json();
        for slot in state.clients.values().filter(|slot| slot.ready) {
            if let Some(sender) = &slot.sender {
                send_to_app(sender, vec![StudioToApp::Custom(json.clone())]);
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let clients: Vec<_> = state.clients.iter().filter(|(_, slot)| slot.ready).map(|(id, _)| *id).collect();
            for client in clients { self.domains.send_message(client, json.clone()); }
        }
        self.module_host.apply_style(cx, &sheet);
        self.stylesheet = Some(sheet);
        // New child processes pick the style before their widget definitions load.
        host::set_child_env("MAKEPAD_WIDGET_STYLE", std::ffi::OsStr::new(&sheet_name));
        let omarchy_source = if style == DesktopStyle::Omarchy {
            theme::load_theme_source(&self.state_mut().theme_name)
        } else { String::new() };
        self.refresh_warm_browser_appearance(cx, browser_appearance(style, dark, &omarchy_source));
        self.style_clock.stop();
        self.style_clock.arm(cx);
        if let Some(mut menu) = self
            .ui
            .widget(cx, ids!(shell_menu))
            .borrow_mut::<ShellMenu>()
        {
            menu.desktop_style = style;
            menu.dark = dark;
        }
        // Wallpaper is part of the framebuffer crossfade. Omarchy retains the
        // selected wallpaper; the other desktop identities have their own ground.
        self.ui.widget(cx, ids!(bg_image)).set_visible(
            cx,
            entry.light().id == DesktopStyle::Omarchy.id()
                && !theme::theme_backgrounds(&self.state_mut().theme_name).is_empty(),
        );
        // A family's own sheet has the family's desktop behind it; any other
        // sheet's desktop is its own window ground (`theme.color_bg_app`), a
        // shade darker at the foot, so a tiled app sits on what it draws.
        let (top, bottom) = match style {
            _ if entry.light().id != style.id() => {
                let ground = sheet_ground.unwrap_or(0x101315ff);
                let [r, g, b] = [ground >> 24, ground >> 16, ground >> 8].map(|c| (c & 0xff) as u8);
                let shade = |c: u8| (c as f32 * 0.9) as u8;
                (shell::rgb(r, g, b), shell::rgb(shade(r), shade(g), shade(b)))
            }
            DesktopStyle::Omarchy => (shell::rgb(16, 19, 21), shell::rgb(24, 30, 34)),
            // Near-black grounds, so the desktop behind it is the same flat
            // dark the style itself draws.
            DesktopStyle::BlackOrange => (shell::rgb(20, 23, 28), shell::rgb(16, 19, 23)),
            DesktopStyle::Macos if dark => (shell::rgb(12, 15, 36), shell::rgb(65, 36, 69)),
            DesktopStyle::Macos => (shell::rgb(39, 43, 87), shell::rgb(171, 109, 131)),
            DesktopStyle::Windows if dark => (shell::rgb(10, 19, 34), shell::rgb(21, 49, 72)),
            DesktopStyle::Windows => (shell::rgb(10, 45, 108), shell::rgb(24, 137, 210)),
            DesktopStyle::Windows2000 => (shell::rgb(0, 128, 128), shell::rgb(0, 128, 128)),
            DesktopStyle::NextStep => (shell::rgb(85, 85, 85), shell::rgb(85, 85, 85)),
            DesktopStyle::Ios => (shell::rgb(38, 78, 137), shell::rgb(159, 207, 227)),
            DesktopStyle::Android => (shell::rgb(50, 46, 73), shell::rgb(158, 156, 204)),
        };
        let mut bg = self.ui.widget(cx, ids!(bg_fill));
        script_apply_eval!(cx,bg,{draw_bg +: {color_top: #(top) color_bottom: #(bottom)}});
        self.update_bar(cx);
        self.configure_phone_mode(cx,previous,style);
        self.redraw_all(cx);
        log!(
            "wm: desktop style {} applied to {} clients and {} modules",
            sheet_name,
            self.state_mut().clients.len(),
            self.module_host.len()
        );
    }
    pub(super) fn send_desktop_style(&mut self, client: ClientId) {
        let Some(json) = self.stylesheet.as_ref().map(|s| s.to_json()) else {
            return;
        };
        // with the dark/light it stands for (`HostedAppearance`), for a process client
        let appearance = self.warm_pool.browser_appearance().map(|dark| makepad_studio_protocol::HostedAppearance { dark }.to_json());
        #[cfg(not(target_arch = "wasm32"))]
        if self.domains.owns(client) {
            self.domains.send_message(client, json);
            return;
        }
        if let Some(sender) = self
            .state_mut()
            .clients
            .get(&client)
            .filter(|s| s.ready)
            .and_then(|s| s.sender.as_ref())
        {
            let mut msgs = vec![StudioToApp::Custom(json)];
            msgs.extend(appearance.map(StudioToApp::Custom));
            send_to_app(sender, msgs);
        }
    }
    pub(super) fn refresh_warm_browser_appearance(&mut self, cx: &mut Cx, dark: bool) {
        // browsers and other portal clients follow (Chromium: SettingChanged color-scheme)
        #[cfg(target_os = "linux")]
        if let Some(portal) = &self.portal {
            portal.set_color_scheme(if dark { crate::portal::PREFER_DARK } else { crate::portal::PREFER_LIGHT });
        }
        for client in self.warm_pool.set_browser_appearance(dark) {
            log!("wm: retiring warm browser {} for {} appearance", client, if dark {"dark"} else {"light"});
            self.request_close(cx, client);
        }
        // Running apps hear it too (`HostedAppearance`): what draws without a stylesheet, such
        // as `makepad run`'s load screen, follows it.
        let json = makepad_studio_protocol::HostedAppearance { dark }.to_json();
        let state = self.state_mut();
        for slot in state.clients.values().filter(|slot| slot.ready) {
            if let Some(sender) = &slot.sender {
                send_to_app(sender, vec![StudioToApp::Custom(json.clone())]);
            }
        }
        // Normal top-up runs after reaping; no new process overlaps the old
        // profile lock, and pages the user has opened stay in their process.
    }
    pub(super) fn toggle_desktop_appearance(&mut self, cx: &mut Cx) {
        self.state_mut().style.dark = !self.state_mut().style.dark;
        let entry = self.current_desktop_sheet();
        self.set_desktop_sheet(cx, entry);
        self.persist_desktop_look(cx);
    }
    /// Remember the sheet and appearance the desktop wears now.
    pub(super) fn persist_desktop_look(&mut self, cx: &mut Cx) {
        let id = self.current_desktop_sheet().id;
        let dark = self.state_mut().style.dark;
        host::persist_desktop_look(cx, id, dark);
    }
    /// The sheet the desktop wears, as a picker offers it (its light one).
    pub(super) fn current_desktop_sheet(&mut self) -> &'static desktop_style::SheetEntry {
        let family = self.state_mut().style.target;
        self.stylesheet
            .as_ref()
            .and_then(|sheet| desktop_style::find(&sheet.name))
            .map(|entry| entry.light())
            .unwrap_or_else(|| family.sheet(false))
    }
    /// Switch the dock's auto-hide, and remember it.
    pub(super) fn set_dock_autohide(&mut self, cx: &mut Cx, on: bool) {
        let now = host::now();
        self.state_mut().dock_autohide.set_enabled(on, now);
        host::persist_dock_autohide(cx, on);
        self.apply_shelf_reserve(cx);
        self.dock_autohide_refresh(cx);
        self.redraw_all(cx);
    }
    /// The dock's icon of an app whose window is a shelf item: its shelf
    /// slides out if it auto-hides, and the item blinks a ring around itself.
    fn call_out_shelf_item(&mut self, cx: &mut Cx, client: ClientId) {
        let side = self.desk(cx).borrow::<crate::desk::WmDesk>().and_then(|d| d.warp_pose(client)).map(|p| p.side).filter(|s| *s != 0);
        let Some(side) = side else { return };
        let now = host::now();
        let screen = self.state_mut().screens.screen_of(client).unwrap_or(0);
        self.state_mut().shelf_autohide.display(screen, now)[(side > 0) as usize].reveal(now, SHELF_CALL_HOLD);
        if let Some(mut d) = self.desk(cx).borrow_mut::<crate::desk::WmDesk>() {
            d.warp.pulse_start(client);
        }
        self.shelf_autohide_refresh(cx);
        self.redraw_all(cx);
    }
    /// Every screen's maximised windows follow the dock's reserve.
    fn apply_shelf_reserve(&mut self, cx: &mut Cx) {
        let area = self.desk_area(cx);
        let state = self.state_mut();
        let reserve = state.shelf_reserve();
        for s in state.screens.screens.iter_mut() {
            s.layout.desktop.shelf_reserve = reserve;
            for c in s.layout.all_clients() {
                s.layout.ensure_desktop(c, area);
            }
        }
    }
    /// The pointer and the dock's timer, before anything else sees the event.
    pub(super) fn dock_autohide_event(&mut self, cx: &mut Cx, event: &Event) {
        if self.state.is_none() {
            return;
        }
        // A delay armed before any pointer event (the dock of a restart).
        if self.dock_autohide_timer.is_empty() {
            if let Some(at) = self.state_mut().dock_autohide.deadline() {
                self.dock_autohide_timer = cx.start_timeout((at - host::now()).max(0.0));
            }
        }
        match event {
            Event::MouseMove(e) => {
                self.dock_pointer = e.abs;
                self.dock_autohide_refresh(cx);
            }
            Event::MouseUp(_) => self.dock_autohide_refresh(cx),
            Event::Timer(t) if self.dock_autohide_timer.is_timer(t).is_some() => {
                self.dock_autohide_timer = Timer::empty();
                self.state_mut().dock_autohide.fire(host::now());
                self.dock_autohide_refresh(cx);
                self.redraw_all(cx);
            }
            _ => {}
        }
    }
    /// Tell the dock's auto-hide where the pointer is and whether a menu or
    /// a drag holds it, then wake for its next deadline.
    pub(super) fn dock_autohide_refresh(&mut self, cx: &mut Cx) {
        if self.state.is_none() || !self.state_mut().dock_autohides() {
            return;
        }
        let Some(full) = self.ui.widget(cx, ids!(desktop_shelf)).borrow::<DesktopShelf>().map(|s| s.screen_rect()) else { return };
        // Not drawn yet: there is no display edge to be at.
        if full.size.x <= 0.0 || full.size.y <= 0.0 {
            return;
        }
        let menu_open = self.ui.widget(cx, ids!(shell_menu)).borrow::<ShellMenu>().is_some_and(|m| m.is_open());
        let busy = menu_open || self.drag.is_some() || self.div_drag.is_some();
        let p = self.dock_pointer;
        let now = host::now();
        let state = self.state_mut();
        let zone = crate::desktop::dock_zone(state, full);
        let was = state.dock_autohide.deadline();
        let over = zone.over(state.dock_autohide.is_out(), p.x, p.y);
        state.dock_autohide.update(now, over, busy);
        let due = state.dock_autohide.deadline();
        if due != was || due.is_none() != self.dock_autohide_timer.is_empty() {
            cx.stop_timer(self.dock_autohide_timer);
            self.dock_autohide_timer = match due {
                Some(at) => cx.start_timeout((at - now).max(0.0)),
                None => Timer::empty(),
            };
        }
    }
    /// Switch one shelf's auto-hide (`side` 0 left, 1 right), and remember it.
    pub(super) fn set_shelf_autohide(&mut self, cx: &mut Cx, side: usize, on: bool) {
        let now = host::now();
        let state = self.state_mut();
        state.shelf_autohide.set_enabled(side, on, now);
        let sides = [state.shelf_autohide.enabled(0), state.shelf_autohide.enabled(1)];
        host::persist_shelf_autohide(cx, sides);
        self.shelf_autohide_refresh(cx);
        self.redraw_all(cx);
    }
    /// The pointer and the shelves' timer, before anything else sees the event.
    pub(super) fn shelf_autohide_event(&mut self, cx: &mut Cx, event: &Event) {
        if self.state.is_none() {
            return;
        }
        if self.shelf_autohide_timer.is_empty() {
            if let Some(at) = self.shelf_deadline() {
                self.shelf_autohide_timer = cx.start_timeout((at - host::now()).max(0.0));
            }
        }
        match event {
            Event::MouseMove(e) => {
                self.shelf_pointer = e.abs;
                self.shelf_autohide_refresh(cx);
            }
            Event::MouseUp(_) => self.shelf_autohide_refresh(cx),
            Event::Timer(t) if self.shelf_autohide_timer.is_timer(t).is_some() => {
                self.shelf_autohide_timer = Timer::empty();
                let now = host::now();
                self.state_mut().shelf_autohide.fire(now);
                self.shelf_autohide_refresh(cx);
                self.redraw_all(cx);
            }
            _ => {}
        }
    }
    fn shelf_deadline(&mut self) -> Option<f64> {
        self.state_mut().shelf_autohide.deadline()
    }
    /// Tell each auto-hiding shelf where the pointer is and whether a menu
    /// or a drag holds it, then wake for the next deadline.
    pub(super) fn shelf_autohide_refresh(&mut self, cx: &mut Cx) {
        if self.state.is_none() || !self.state_mut().shelf_autohide.any_enabled() {
            return;
        }
        let p = self.shelf_pointer;
        let pointer_screen = self.state_mut().screen_under(p.x, p.y);
        let menu_open = self.ui.widget(cx, ids!(shell_menu)).borrow::<ShellMenu>().is_some_and(|m| m.is_open());
        // A window being moved calls the shelf of the zone it is over and
        // keeps it out while it hovers there; other drags and menus hold it.
        let moving_window = self.drag.as_ref().is_some_and(|d| d.floating && !d.resize && d.armed);
        let busy = menu_open || self.div_drag.is_some() || (self.drag.is_some() && !moving_window);
        let now = host::now();
        let was = self.shelf_deadline();
        for screen in 0..self.state_mut().screens.live_count() {
            let area = self.screen_area(cx, screen);
            let reach = self.desk(cx).borrow::<crate::desk::WmDesk>().map_or([0.0; 2], |d| d.warp.reach_at((area.x + area.w * 0.5, area.y + area.h * 0.5)));
            let held = self.desk(cx).borrow::<crate::desk::WmDesk>().and_then(|d| d.warp.unfold.filter(|u| u.open() && u.screen == area).map(|u| u.side));
            let state = self.state_mut();
            for (i, side) in [-1i8, 1].into_iter().enumerate() {
                let m = &mut state.shelf_autohide.display(screen, now)[i];
                let over = screen == pointer_screen && crate::spacewarp::shelf::autohide_over(side, m.is_out(), reach[i], area, (p.x, p.y));
                // A shelf with an unfolded window stays out until it is folded.
                m.update(now, over || held == Some(side), busy);
            }
        }
        let due = self.shelf_deadline();
        if due != was || due.is_none() != self.shelf_autohide_timer.is_empty() {
            cx.stop_timer(self.shelf_autohide_timer);
            self.shelf_autohide_timer = match due {
                Some(at) => cx.start_timeout((at - now).max(0.0)),
                None => Timer::empty(),
            };
        }
    }
    pub(super) fn open_style_menu(&mut self, cx: &mut Cx) {
        {
            let state = self.state_mut();
            let (autohide, dark) = (state.dock_autohide.enabled, state.style.target.supports_dark().then_some(state.style.dark));
            let (warp, shelves) = (state.warp.mode == spacewarp::Mode::Warp, [state.shelf_autohide.enabled(0), state.shelf_autohide.enabled(1)]);
            if let Some(mut menu) = self.ui.widget(cx, ids!(shell_menu)).borrow_mut::<ShellMenu>() {
                menu.model.warp_shelves = warp;
                menu.model.shelf_autohide = shelves;
                menu.model.dock_autohide = autohide;
                menu.model.dark_choice = dark;
                menu.model.natural_scroll = host::load_natural_scroll();
                menu.model.tap_to_click = host::load_tap_to_click();
            }
        }
        let anchor = if self.state_mut().style.target.mobile() {
            self.ui.widget(cx, ids!(phone_controls)).borrow::<crate::mobile_surface::PhoneSurface>()
                .and_then(|bar| bar.hit_rect(&crate::mobile::PhoneHit::Style))
        } else {
            self.ui.widget(cx, ids!(shell_bar)).borrow::<crate::shell::bar::ShellBar>()
                .and_then(|bar| bar.module_rect(self.bar_press_segment, BarModule::Style))
        };
        self.open_shell_menu(cx, "desktop", MenuSkin::Menu);
        // Under the switch it was pressed in: that segment's screen.
        self.menu_screen = self.bar_screen(self.bar_press_segment);
        let active = format!("desktop.{}", self.current_desktop_sheet().id);
        if let Some(mut menu) = self.ui.widget(cx, ids!(shell_menu)).borrow_mut::<ShellMenu>() {
            menu.anchor = anchor;
            if let Some(index) = menu.model.rows.iter().position(|row| row.target == active) {
                menu.model.sel = index;
            }
        }
    }
    pub(super) fn toggle_launcher(&mut self, cx: &mut Cx) {
        let open = self
            .ui
            .widget(cx, ids!(shell_menu))
            .borrow::<ShellMenu>()
            .is_some_and(|m| m.is_open());
        if open {
            self.close_shell_menu(cx);
        } else {
            let path = match self.state_mut().style.target {
                DesktopStyle::Omarchy => "",
                DesktopStyle::Windows2000 => "start",
                DesktopStyle::NextStep => "workspace",
                _ => "apps",
            };
            self.open_shell_menu(cx, path, MenuSkin::Launcher);
        }
    }
    pub(super) fn minimize_desktop(&mut self, cx: &mut Cx, client: ClientId) {
        let layout = self.state_mut().layout_of_mut(client);
        if let Some(w) = layout.desktop.get_mut(client) {
            w.minimized = true;
        }
        let next = layout
            .desktop
            .windows
            .iter()
            .rev()
            .find(|w| !w.minimized && layout.workspace_of(w.client) == Some(layout.active))
            .map(|w| w.client);
        layout.workspaces[layout.active].focus = next;
        if let Some(next) = next {
            self.focus_client(cx, next);
        }
        self.update_bar(cx);
        self.redraw_all(cx);
    }
    pub(super) fn apply_snap(&mut self, cx: &mut Cx, client: ClientId, zone: crate::snap::Zone) {
        if let Some(w)=self.state_mut().layout_of_mut(client).desktop.get_mut(client) {
            w.maximized=zone==crate::snap::Zone::Maximize;
            w.snap=(!w.maximized).then_some(zone);
            w.minimized=false;
        }
        self.state_mut().snap.clear();
        self.focus_client(cx,client);
        self.redraw_all(cx);
        log!("wm: snapped client {} to {:?}",client,zone);
    }
    pub(super) fn snap_event(&mut self, cx: &mut Cx, event: &Event) -> bool {
        if self.state_mut().style.target!=DesktopStyle::Windows || self.drag.is_some() || self.div_drag.is_some() {return false;}
        if let Some(client)=self.state_mut().snap.picker.as_ref().map(|p|p.client) {
            let state=self.state_mut();
            let layout=state.layout_of(client);
            if layout.workspace_of(client)!=Some(layout.active) || layout.desktop.minimized(client) {
                state.snap.clear();cx.stop_timer(self.snap_hover_timer);self.redraw_all(cx);
            }
        }
        if self.snap_hover_timer.is_event(event).is_some() {
            if let Some((client,anchor))=self.state_mut().snap.hovered {
                let area=crate::spacewarp::band_of(self.desk_area(cx));
                self.state_mut().snap.picker=Some(crate::snap::Picker::new(client,area,Some(anchor)));
                self.state_mut().snap.area=Some(area);
                self.redraw_all(cx);
            }
            return false;
        }
        if matches!(event,Event::KeyDown(e) if e.key_code==KeyCode::Escape) && self.state_mut().snap.picker.is_some() {
            self.state_mut().snap.clear();cx.stop_timer(self.snap_hover_timer);self.redraw_all(cx);return true;
        }
        let p=match event {Event::MouseMove(e)=>e.abs,Event::MouseDown(e)=>e.abs,Event::MouseUp(e)=>e.abs,_=>return false};
        if matches!(event,Event::MouseDown(_)) {cx.stop_timer(self.snap_hover_timer);}
        let picker=self.state_mut().snap.picker.clone();
        if let Some(picker)=picker {
            if picker.bounds.contains(p) {
                let hit=picker.hit(p);
                if self.state_mut().snap.preview!=hit {self.state_mut().snap.preview=hit;self.redraw_all(cx);}
                if matches!(event,Event::MouseDown(e) if e.button.contains(MouseButton::PRIMARY)) {
                    if let Some(zone)=hit {self.apply_snap(cx,picker.client,zone);}
                }
                return true;
            }
            let on_anchor=self.state_mut().snap.hovered.is_some_and(|(client,r)|client==picker.client && r.contains(p));
            if (!picker.near(p) && !on_anchor) || matches!(event,Event::MouseDown(_)) {
                self.state_mut().snap.clear();self.redraw_all(cx);
            }
        }
        let hovered=self.desk(cx).borrow::<WmDesk>().and_then(|desk| {
            let (client,hit)=desk.chrome_hit(p)?;
            (hit==ChromeHit::Maximize).then(|| desk.chrome_button_rect(client,hit).map(|r|(client,r))).flatten()
        });
        if self.state_mut().snap.hovered!=hovered {
            cx.stop_timer(self.snap_hover_timer);
            self.state_mut().snap.hovered=hovered;
            if hovered.is_some() && matches!(event,Event::MouseMove(_)) {self.snap_hover_timer=cx.start_timeout(0.45);}
        }
        false
    }
    pub(super) fn activate_shelf(&mut self, cx: &mut Cx, hit: ShelfHit) {
        match hit {
            ShelfHit::Launcher => self.toggle_launcher(cx),
            ShelfHit::App(app) => {
                let existing = {
                    let s = self.state_mut();
                    // The app's window on any screen, the active one first.
                    let active = s.screens.active;
                    std::iter::once(active)
                        .chain((0..s.screens.live_count()).filter(|i| *i != active))
                        .find_map(|i| {
                            s.layout_at(i)
                                .desktop
                                .windows
                                .iter()
                                .rev()
                                .find(|w| s.clients.get(&w.client).is_some_and(|c| c.app == app))
                                .map(|w| w.client)
                        })
                };
                if let Some(c) = existing {
                    self.call_out_shelf_item(cx, c);
                    self.restore_desktop(cx, c);
                } else {
                    self.launch_app(cx, &app);
                }
            }
            ShelfHit::More(shown) => self.open_dock_overflow(cx, shown),
            ShelfHit::Window(c) => {
                if self.state_mut().layout().focused_client() == Some(c)
                    && !self.state_mut().layout_of(c).desktop.minimized(c)
                {
                    self.minimize_desktop(cx, c);
                } else {
                    self.restore_desktop(cx, c);
                }
            }
            ShelfHit::ShowDesktop => {
                let layout = self.state_mut().layout_mut();
                let clients = layout.clients_on(layout.active);
                let hide = clients.iter().any(|c| !layout.desktop.minimized(*c));
                for c in clients {
                    if let Some(w) = layout.desktop.get_mut(c) {
                        w.minimized = hide;
                    }
                }
                self.redraw_all(cx);
            }
        }
    }
    fn restore_desktop(&mut self, cx: &mut Cx, client: ClientId) {
        if let Some(w) = self.state_mut().layout_of_mut(client).desktop.get_mut(client) {
            w.minimized = false;
        }
        self.state_mut().layout_of_mut(client).raise_float(client);
        self.focus_client(cx, client);
        self.redraw_all(cx);
    }
    pub(super) fn desktop_key(&mut self, cx: &mut Cx, e: &KeyEvent) -> bool {
        let style = self.state_mut().style.target;
        if !style.floating() {
            return false;
        }
        if e.key_code == KeyCode::Escape && e.modifiers.control {
            self.toggle_launcher(cx);
            return true;
        }
        if e.key_code == KeyCode::F4 && e.modifiers.alt {
            if let Some(c) = self.state_mut().layout().focused_client() {
                self.request_close(cx, c);
            }
            return true;
        }
        if !super_chord(&e.modifiers) {
            return false;
        }
        if e.key_code == KeyCode::KeyD && style != DesktopStyle::Macos {
            self.activate_shelf(cx, ShelfHit::ShowDesktop);
            return true;
        }
        let Some(client) = self.state_mut().layout().focused_client() else {
            return false;
        };
        if e.key_code == KeyCode::KeyM {
            self.minimize_desktop(cx, client);
            return true;
        }
        if style == DesktopStyle::Windows {
            match e.key_code {
                KeyCode::KeyZ => {
                    let area=crate::spacewarp::band_of(self.desk_area(cx));
                    let anchor=self.desk(cx).borrow::<WmDesk>().and_then(|d|d.chrome_button_rect(client,ChromeHit::Maximize));
                    self.state_mut().snap.picker=Some(crate::snap::Picker::new(client,area,anchor));
                    self.state_mut().snap.area=Some(area);
                    self.redraw_all(cx);
                }
                KeyCode::ArrowUp => self.maximise_toggle(cx, client),
                KeyCode::ArrowDown => {
                    if self
                        .state_mut()
                        .layout()
                        .desktop
                        .get(client)
                        .is_some_and(|w| w.maximized)
                    {
                        self.maximise_toggle(cx, client);
                    } else {
                        self.minimize_desktop(cx, client);
                    }
                }
                KeyCode::ArrowLeft | KeyCode::ArrowRight => {
                    self.apply_snap(cx,client,if e.key_code==KeyCode::ArrowRight {crate::snap::Zone::Right}else{crate::snap::Zone::Left});
                }
                _ => return false,
            }
            return true;
        }
        false
    }
    /// Called before child input dispatch; coordinates are from the last drawn
    /// frame, including the interpolated title bar and shelf rectangles.
    pub(super) fn desktop_pointer(&mut self, cx: &mut Cx, event: &Event, physical: Option<Vec2d>) -> bool {
        if self.drag.is_some() || self.div_drag.is_some() {
            return false;
        }
        let p = match event {
            Event::MouseDown(e) => e.abs,
            Event::MouseMove(e) => e.abs,
            Event::MouseUp(e) => e.abs,
            Event::Scroll(e) => e.abs,
            _ => return false,
        };
        let shelf = self.ui.widget(cx, ids!(desktop_shelf));
        // Window input may have passed through the VR desktop camera;
        // the shell's shelf keeps its ordinary screen coordinates.
        let shelf_point = physical.unwrap_or(p);
        let (hit, inside) = if let Some(mut s) = shelf.borrow_mut::<DesktopShelf>() {
            s.hover_at(cx, shelf_point);
            (s.hit(shelf_point), s.contains(shelf_point))
        } else {
            (None, false)
        };
        let chrome_hit = self.desk(cx).borrow_mut::<WmDesk>()
            .and_then(|mut d| d.chrome_pointer(cx, (!inside).then_some(p)));
        if let Event::MouseUp(e) = event {
            if e.button.contains(MouseButton::PRIMARY) {
                let pressed = self.desk(cx).borrow_mut::<WmDesk>()
                    .and_then(|mut d| d.release_chrome(cx));
                if let Some((client, control)) = pressed {
                    if chrome_hit == Some((client, control)) {
                        match control {
                            ChromeHit::Close => self.request_close(cx, client),
                            ChromeHit::Minimize => self.minimize_desktop(cx, client),
                            ChromeHit::Maximize => self.fullscreen_toggle(cx, client),
                            _ => {}
                        }
                    }
                    return true;
                }
            }
        }
        if inside {
            if let Event::MouseDown(e) = event {
                if e.button.contains(MouseButton::PRIMARY) {
                    if let Some(hit) = hit {
                        self.activate_shelf(cx, hit);
                    }
                }
            }
            return true;
        }
        if self.warp_shelf_press(cx, event) {
            return true;
        }
        // A skin's controls work on every style; its title and edges are the
        // floating styles' business.
        let skin_control = self.state_mut().skin.active()
            && chrome_hit.is_some_and(|(_, h)| matches!(h, ChromeHit::Close | ChromeHit::Minimize | ChromeHit::Maximize));
        if !self.state_mut().style.target.floating() && !skin_control {
            return false;
        }
        if self.state_mut().style.target == DesktopStyle::NextStep {
            if let Event::MouseDown(e) = event {
                let (on_desktop, over_window) = self.desk(cx).borrow::<WmDesk>()
                    .map(|d| (d.desk_rect.contains(p), d.window_at(p).is_some()))
                    .unwrap_or((false, false));
                // The bar floating over the desk's top is not the desktop.
                let on_desktop = on_desktop && !self.bar_floats_over(cx, p);
                if e.button.contains(MouseButton::SECONDARY)
                    && on_desktop
                    && (chrome_hit.is_some() || !over_window)
                {
                    if let Some(mut menu) = self.ui.widget(cx, ids!(shell_menu)).borrow_mut::<ShellMenu>() {
                        menu.open_next_popup(cx, p);
                    }
                    self.redraw_all(cx);
                    return true;
                }
            }
        }
        let hit = chrome_hit;
        if let Event::MouseMove(_) = event {
            if let Some((_, hit)) = hit {
                cx.set_cursor(match hit {
                    ChromeHit::Resize(0, _) => MouseCursor::NsResize,
                    ChromeHit::Resize(_, 0) => MouseCursor::EwResize,
                    ChromeHit::Resize(x, y) if x == y => MouseCursor::NwseResize,
                    ChromeHit::Resize(_, _) => MouseCursor::NeswResize,
                    _ => MouseCursor::Default,
                });
            }
        }
        if let Event::MouseDown(e) = event {
            if e.button.contains(MouseButton::PRIMARY) {
                if let Some((client, hit)) = hit {
                    match hit {
                        ChromeHit::Close | ChromeHit::Minimize | ChromeHit::Maximize => {
                            if let Some(mut desk) = self.desk(cx).borrow_mut::<WmDesk>() {
                                desk.press_chrome(cx, (client, hit));
                            }
                        }
                        ChromeHit::Title => {
                            // A parked window's title: drag moves it, a click
                            // brings it to the centre (never a double click).
                            let parked = self.warp_is_parked(cx, client);
                            let double = !parked && self.title_press.is_some_and(|(c, time, pos)| {
                                c == client && e.time - time < 0.35 && (pos - p).length() < 5.0
                            });
                            self.title_press = if parked { None } else { Some((client, e.time, p)) };
                            if double {
                                self.maximise_toggle(cx, client);
                            } else {
                                self.begin_drag_on(cx, client, p, false, false);
                            }
                        }
                        ChromeHit::Resize(x, y) => {
                            self.begin_drag_on(cx, client, p, true, false);
                            if let Some(d) = &mut self.drag {
                                d.resize_x = x != 0;
                                d.resize_y = y != 0;
                                d.grab_left = x < 0;
                                d.grab_top = y < 0;
                            }
                        }
                    }
                    return true;
                }
            }
        }
        hit.is_some()
    }
}

#[cfg(test)]
mod appearance_tests {
    use super::*;
    #[test]
    fn warm_browser_appearance_matches_classic_and_omarchy_palettes() {
        for style in [DesktopStyle::Windows2000,DesktopStyle::NextStep] {
            assert!(!browser_appearance(style,true,""));
        }
        for style in [DesktopStyle::Macos,DesktopStyle::Windows,DesktopStyle::Ios,DesktopStyle::Android] {
            assert!(browser_appearance(style,true,""));
            assert!(!browser_appearance(style,false,""));
        }
        assert!(browser_appearance(DesktopStyle::Omarchy,false,"background: #121212"));
        assert!(!browser_appearance(DesktopStyle::Omarchy,true,"background: #eeeeee"));
    }
}

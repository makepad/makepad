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

impl App {
    pub(super) fn set_desktop_style(&mut self, cx: &mut Cx, style: DesktopStyle) {
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
        let dark = self.state_mut().style.dark;
        let sheet = desktop_style::StyleSheet::load_with_appearance(style, dark);
        if let Some(mut desk) = self.desk(cx).borrow_mut::<WmDesk>() {desk.set_startup_style(cx, &sheet);}
        let sheet_name = sheet.name.clone();
        app_icon::install(cx, style, &sheet.icons);
        let state = self.state_mut();
        state.dragging.clear();
        state.style.select(style);
        if changes_size { state.style.step(1.0); }
        state.layout.desktop.enabled = style.floating();
        for c in state.layout.all_clients() {
            state.layout.desktop.ensure(c, area);
        }
        // Send the complete stylesheet, so already-running and warm applications
        // receive exactly the version selected in the WM, even across hosts.
        let json = sheet.to_json();
        for slot in state.clients.values().filter(|slot| slot.ready) {
            if let Some(sender) = &slot.sender {
                send_to_app(sender, vec![StudioToApp::Custom(json.clone())]);
            }
        }
        self.module_host.apply_style(cx, &sheet);
        self.stylesheet = Some(sheet);
        // New child processes pick the style before their widget definitions load.
        host::set_child_env("MAKEPAD_WIDGET_STYLE", std::ffi::OsStr::new(&sheet_name));
        let omarchy_source = if style == DesktopStyle::Omarchy {
            theme::load_theme_source(&self.state_mut().theme_name)
        } else { String::new() };
        self.refresh_warm_browser_appearance(cx, browser_appearance(style, dark, &omarchy_source));
        self.style_time = 0.0;
        self.style_frame = cx.new_next_frame();
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
            style == DesktopStyle::Omarchy
                && !theme::theme_backgrounds(&self.state_mut().theme_name).is_empty(),
        );
        let (top, bottom) = match style {
            DesktopStyle::Omarchy => (shell::rgb(16, 19, 21), shell::rgb(24, 30, 34)),
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
        if let Some(sender) = self
            .state_mut()
            .clients
            .get(&client)
            .filter(|s| s.ready)
            .and_then(|s| s.sender.as_ref())
        {
            send_to_app(sender, vec![StudioToApp::Custom(json)]);
        }
    }
    pub(super) fn refresh_warm_browser_appearance(&mut self, cx: &mut Cx, dark: bool) {
        for client in self.warm_pool.set_browser_appearance(dark) {
            log!("wm: retiring warm browser {} for {} appearance", client, if dark {"dark"} else {"light"});
            self.request_close(cx, client);
        }
        // Normal top-up runs after reaping; no new process overlaps the old
        // profile lock, and pages the user has opened stay in their process.
    }
    pub(super) fn toggle_desktop_appearance(&mut self, cx: &mut Cx) {
        self.state_mut().style.dark = !self.state_mut().style.dark;
        let style = self.state_mut().style.target;
        self.set_desktop_style(cx, style);
    }
    pub(super) fn open_style_menu(&mut self, cx: &mut Cx) {
        let anchor = if self.state_mut().style.target.mobile() {
            self.ui.widget(cx, ids!(phone_controls)).borrow::<crate::mobile_surface::PhoneSurface>()
                .and_then(|bar| bar.hit_rect(&crate::mobile::PhoneHit::Style))
        } else {
            self.ui.widget(cx, ids!(shell_bar)).borrow::<crate::shell::bar::ShellBar>()
                .and_then(|bar| bar.module_rect(BarModule::Style))
        };
        self.open_shell_menu(cx, "desktop", MenuSkin::Menu);
        let active = format!("desktop.{}", self.state_mut().style.target.id());
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
    fn minimize_desktop(&mut self, cx: &mut Cx, client: ClientId) {
        let layout = &mut self.state_mut().layout;
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
    fn maximize_desktop(&mut self, cx: &mut Cx, client: ClientId) {
        if let Some(w) = self.state_mut().layout.desktop.get_mut(client) {
            w.maximized = !w.maximized;
            w.minimized = false;
        }
        self.focus_client(cx, client);
        self.redraw_all(cx);
    }
    pub(super) fn apply_snap(&mut self, cx: &mut Cx, client: ClientId, zone: crate::snap::Zone) {
        if let Some(w)=self.state_mut().layout.desktop.get_mut(client) {
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
            if state.layout.workspace_of(client)!=Some(state.layout.active) || state.layout.desktop.minimized(client) {
                state.snap.clear();cx.stop_timer(self.snap_hover_timer);self.redraw_all(cx);
            }
        }
        if self.snap_hover_timer.is_event(event).is_some() {
            if let Some((client,anchor))=self.state_mut().snap.hovered {
                let area=self.desk_area(cx);
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
    fn activate_shelf(&mut self, cx: &mut Cx, hit: ShelfHit) {
        match hit {
            ShelfHit::Launcher => self.toggle_launcher(cx),
            ShelfHit::App(app) => {
                let existing = {
                    let s = self.state_mut();
                    s.layout
                        .desktop
                        .windows
                        .iter()
                        .rev()
                        .find(|w| s.clients.get(&w.client).is_some_and(|c| c.app == app))
                        .map(|w| w.client)
                };
                if let Some(c) = existing {
                    self.restore_desktop(cx, c);
                } else {
                    self.launch_app(cx, &app);
                }
            }
            ShelfHit::Window(c) => {
                if self.state_mut().layout.focused_client() == Some(c)
                    && !self.state_mut().layout.desktop.minimized(c)
                {
                    self.minimize_desktop(cx, c);
                } else {
                    self.restore_desktop(cx, c);
                }
            }
            ShelfHit::ShowDesktop => {
                let layout = &mut self.state_mut().layout;
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
        if let Some(w) = self.state_mut().layout.desktop.get_mut(client) {
            w.minimized = false;
        }
        self.state_mut().layout.raise_float(client);
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
            if let Some(c) = self.state_mut().layout.focused_client() {
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
        let Some(client) = self.state_mut().layout.focused_client() else {
            return false;
        };
        if e.key_code == KeyCode::KeyM {
            self.minimize_desktop(cx, client);
            return true;
        }
        if style == DesktopStyle::Windows {
            match e.key_code {
                KeyCode::KeyZ => {
                    let area=self.desk_area(cx);
                    let anchor=self.desk(cx).borrow::<WmDesk>().and_then(|d|d.chrome_button_rect(client,ChromeHit::Maximize));
                    self.state_mut().snap.picker=Some(crate::snap::Picker::new(client,area,anchor));
                    self.state_mut().snap.area=Some(area);
                    self.redraw_all(cx);
                }
                KeyCode::ArrowUp => self.maximize_desktop(cx, client),
                KeyCode::ArrowDown => {
                    if self
                        .state_mut()
                        .layout
                        .desktop
                        .get(client)
                        .is_some_and(|w| w.maximized)
                    {
                        self.maximize_desktop(cx, client);
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
    pub(super) fn desktop_pointer(&mut self, cx: &mut Cx, event: &Event) -> bool {
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
        let (hit, inside) = if let Some(mut s) = shelf.borrow_mut::<DesktopShelf>() {
            s.hover_at(cx, p);
            (s.hit(p), s.contains(p))
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
                            ChromeHit::Maximize => self.maximize_desktop(cx, client),
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
        if !self.state_mut().style.target.floating() {
            return false;
        }
        if self.state_mut().style.target == DesktopStyle::NextStep {
            if let Event::MouseDown(e) = event {
                let (on_desktop, over_window) = self.desk(cx).borrow::<WmDesk>()
                    .map(|d| (d.desk_rect.contains(p), d.window_at(p).is_some()))
                    .unwrap_or((false, false));
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
                            let double = self.title_press.is_some_and(|(c, time, pos)| {
                                c == client && e.time - time < 0.35 && (pos - p).length() < 5.0
                            });
                            self.title_press = Some((client, e.time, p));
                            if double {
                                self.maximize_desktop(cx, client);
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

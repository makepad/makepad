//! The page area. One widget hosts every tab's CEF browser; the active
//! tab's texture is what gets drawn, input goes to the active browser,
//! background tabs are told they are hidden so they stop painting.
//!
//! Rendering: with `shared_texture_enabled` CEF paints on the GPU into pooled
//! IOSurfaces and `libs/cef` blits each one into a Makepad-owned IOSurface
//! texture (`Cx::create_iosurface_render_texture`) that the quad samples —
//! no CPU readback anywhere. The classic `on_paint` BGRA upload stays as the
//! fallback (`MAKEPAD_CEF_SOFTWARE=1`, or a CEF build without GPU paint).

use crate::tabs::{TabId, TabModel, TabSummary};
use makepad_widgets::browser::Browser as BrowserKeys;
use makepad_widgets::image::DrawImage;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.WebViewBase = #(WebView::register_widget(vm))

    mod.widgets.WebView = set_type_default() do mod.widgets.WebViewBase{
        width: Fill
        height: Fill
        draw_empty +: {
            color: uniform(mod.mpb_theme.darker_background)
            pixel: fn() {
                return self.color
            }
        }
        draw_status +: {
            color: mod.mpb_theme.dark_foreground
            text_style: theme.font_regular{
                font_size: 11
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum WebViewAction {
    #[default]
    None,
    /// Titles / URLs / loading state / favicons / the tab list changed.
    TabsChanged,
}

/// What the chrome shows for the active tab.
#[derive(Clone, Debug, Default)]
pub struct ActiveInfo {
    pub id: Option<TabId>,
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub render_mode: String,
    pub accelerated_frames: u64,
    pub last_blit_micros: u64,
}

#[derive(Script, ScriptHook, Widget)]
pub struct WebView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawImage,
    #[live]
    draw_empty: DrawQuad,
    #[live]
    draw_status: DrawText,
    #[rust]
    tabs: TabModel,
    /// `cef_initialize` runs on the frame after the chrome first drew, so
    /// the window is up before Chromium's processes spawn.
    #[rust]
    cef_ready: bool,
    #[rust]
    cef_init_frame: NextFrame,
    #[rust]
    cef_init_requested: bool,
    #[rust]
    cef_init_error: Option<String>,
    #[rust]
    first_frame_logged: bool,
    #[rust]
    pump_timer: Timer,
    #[rust]
    pressed_buttons: MouseButton,
    #[rust]
    suppress_next_paste_shortcut: bool,
    #[rust]
    pump_started: bool,
}

impl WebView {
    const PUMP_INTERVAL: f64 = 1.0 / 120.0;

    // ------------------------------------------------------------------
    // Tab operations (called by the app from chrome actions / shortcuts)
    // ------------------------------------------------------------------

    pub fn new_tab(&mut self, cx: &mut Cx, url: &str, activate: bool) -> TabId {
        let id = self.tabs.insert(url, activate);
        if activate {
            self.apply_active_visibility();
            self.focus(cx);
        }
        self.notify(cx);
        id
    }

    pub fn close_tab(&mut self, cx: &mut Cx, id: TabId) {
        if let Some((tab, _was_active)) = self.tabs.remove(id) {
            // Dropping the tab closes its browser.
            drop(tab);
        }
        self.apply_active_visibility();
        self.notify(cx);
    }

    pub fn activate(&mut self, cx: &mut Cx, id: TabId) {
        if self.tabs.activate(id) {
            self.apply_active_visibility();
            self.focus(cx);
            self.notify(cx);
        }
    }

    pub fn activate_offset(&mut self, cx: &mut Cx, delta: isize) {
        self.tabs.activate_offset(delta);
        self.apply_active_visibility();
        self.focus(cx);
        self.notify(cx);
    }

    pub fn activate_index(&mut self, cx: &mut Cx, index: usize) {
        self.tabs.activate_index(index);
        self.apply_active_visibility();
        self.focus(cx);
        self.notify(cx);
    }

    pub fn active_id(&self) -> Option<TabId> {
        self.tabs.active_id()
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn summaries(&self) -> Vec<TabSummary> {
        self.tabs.summaries()
    }

    pub fn active_info(&self) -> ActiveInfo {
        let Some(tab) = self.tabs.active() else {
            return ActiveInfo::default();
        };
        let (accelerated_frames, last_blit_micros) = tab
            .browser
            .as_ref()
            .map(|b| {
                let s = b.accelerated_stats();
                (s.frames, s.last_blit_micros)
            })
            .unwrap_or((0, 0));
        ActiveInfo {
            id: Some(tab.id),
            url: tab.url.clone(),
            title: tab.display_title(),
            loading: tab.loading,
            can_go_back: tab.can_go_back,
            can_go_forward: tab.can_go_forward,
            render_mode: format!("{:?}", tab.render_mode),
            accelerated_frames,
            last_blit_micros,
        }
    }

    pub fn navigate(&mut self, cx: &mut Cx, url: &str) {
        if let Some(tab) = self.tabs.active_mut() {
            tab.url = url.to_string();
            tab.title.clear();
            tab.favicon = None;
            tab.loading = true;
            match &mut tab.browser {
                Some(browser) => {
                    if let Err(err) = browser.set_url(url) {
                        log!("navigate failed: {err}");
                    }
                }
                None => tab.initial_url = url.to_string(),
            }
        }
        self.focus(cx);
        self.notify(cx);
    }

    pub fn go_back(&mut self, cx: &mut Cx) {
        if let Some(browser) = self.active_browser() {
            let _ = browser.go_back();
        }
        self.focus(cx);
    }

    pub fn go_forward(&mut self, cx: &mut Cx) {
        if let Some(browser) = self.active_browser() {
            let _ = browser.go_forward();
        }
        self.focus(cx);
    }

    pub fn reload(&mut self, cx: &mut Cx) {
        if let Some(browser) = self.active_browser() {
            let _ = browser.reload();
        }
        self.focus(cx);
    }

    pub fn stop(&mut self, cx: &mut Cx) {
        if let Some(browser) = self.active_browser() {
            let _ = browser.stop_load();
        }
        self.focus(cx);
    }

    pub fn focus(&mut self, cx: &mut Cx) {
        let area = self.draw_bg.area();
        if area.is_valid(cx) {
            cx.set_key_focus(area);
        }
        if let Some(browser) = self.active_browser() {
            let _ = browser.set_focus(true);
        }
    }

    // ------------------------------------------------------------------

    fn notify(&mut self, cx: &mut Cx) {
        cx.widget_action(self.uid, WebViewAction::TabsChanged);
        self.redraw(cx);
    }

    fn active_browser(&mut self) -> Option<&mut makepad_cef::Browser> {
        self.tabs.active_mut().and_then(|t| t.browser.as_mut())
    }

    /// Background tabs stop painting; the active one is shown.
    fn apply_active_visibility(&mut self) {
        let active = self.tabs.active;
        for (i, tab) in self.tabs.tabs.iter_mut().enumerate() {
            if let Some(browser) = &mut tab.browser {
                let _ = browser.set_hidden(i != active);
            }
        }
    }

    fn ensure_active_browser(&mut self, cx: &mut Cx, width: usize, height: usize, dpi: f32) {
        if !self.cef_ready {
            return;
        }
        let active = self.tabs.active;
        let Some(tab) = self.tabs.tabs.get_mut(active) else {
            return;
        };
        if tab.browser.is_none() && tab.init_error.is_none() {
            match makepad_cef::Browser::new(&tab.initial_url, width, height, dpi) {
                Ok(browser) => {
                    tab.browser = Some(browser);
                }
                Err(err) => {
                    let message = err.to_string();
                    log!("CEF browser creation failed: {message}");
                    tab.init_error = Some(message);
                }
            }
        }
        let Some(browser) = &mut tab.browser else {
            return;
        };
        let _ = browser.set_hidden(false);
        if let Err(err) = browser.resize(width, height, dpi) {
            log!("CEF resize failed: {err}");
        }

        // GPU path: hand the browser a Makepad-owned IOSurface to copy into.
        #[cfg(target_os = "macos")]
        if browser.is_accelerated()
            && (tab.accel_target_size != Some((width, height)) || tab.texture.is_none())
        {
            let (texture, iosurface, _id) = cx.create_iosurface_render_texture(width, height);
            match browser.set_accelerated_target(iosurface, width, height) {
                Ok(()) => {
                    tab.texture = Some(texture);
                    tab.accel_target_size = Some((width, height));
                }
                Err(err) => {
                    log!("accelerated target failed, software frames only: {err}");
                    tab.accel_target_size = None;
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = cx;
    }

    fn apply_software_frame(cx: &mut Cx, tab: &mut crate::tabs::Tab, frame: makepad_cef::Frame) {
        match &tab.texture {
            Some(texture)
                if tab.accel_target_size.is_none()
                    && texture.get_format(cx).vec_width_height()
                        == Some((frame.width, frame.height)) =>
            {
                texture.set_data_u32(cx, frame.width, frame.height, frame.pixels);
            }
            _ => {
                tab.accel_target_size = None;
                tab.texture = Some(Texture::new_with_format(
                    cx,
                    TextureFormat::VecBGRAu8_32 {
                        data: Some(frame.pixels),
                        width: frame.width,
                        height: frame.height,
                        updated: TextureUpdated::Full,
                    },
                ));
            }
        }
    }

    fn favicon_texture(cx: &mut Cx, frame: makepad_cef::Frame) -> Texture {
        Texture::new_with_format(
            cx,
            TextureFormat::VecBGRAu8_32 {
                data: Some(frame.pixels),
                width: frame.width,
                height: frame.height,
                updated: TextureUpdated::Full,
            },
        )
    }

    /// One pump: run CEF's message loop, collect frames and navigation
    /// changes from every tab, open queued popups as tabs.
    fn pump(&mut self, cx: &mut Cx) {
        makepad_cef::do_message_loop_work();
        let active = self.tabs.active;
        let mut changed = false;
        let mut redraw = false;
        let mut popups = Vec::new();
        for (i, tab) in self.tabs.tabs.iter_mut().enumerate() {
            let Some(browser) = &mut tab.browser else {
                continue;
            };
            let mut latest = None;
            while let Some(frame) = browser.take_frame() {
                latest = Some(frame);
            }
            let counter = browser.accelerated_frame_counter();
            if counter != tab.accel_frame_counter {
                tab.accel_frame_counter = counter;
                if i == active {
                    redraw = true;
                }
                if !self.first_frame_logged {
                    self.first_frame_logged = true;
                    log!("mpbrowser: first page frame at {} ms", crate::uptime_ms());
                }
            }
            let generation = browser.nav_generation();
            if generation != tab.nav_generation {
                tab.nav_generation = generation;
                tab.title = browser.title();
                let url = browser.url();
                if !url.is_empty() {
                    tab.url = url;
                }
                tab.loading = browser.is_loading();
                tab.can_go_back = browser.can_go_back();
                tab.can_go_forward = browser.can_go_forward();
                if let Some(favicon) = browser.take_favicon() {
                    tab.favicon = Some(Self::favicon_texture(cx, favicon));
                }
                popups.extend(browser.take_popup_requests());
                changed = true;
            }
            let mode = browser.render_mode();
            if mode != tab.render_mode {
                tab.render_mode = mode;
                log!("tab {:?} render mode: {:?}", tab.id, mode);
                changed = true;
            }
            if let Some(frame) = latest {
                Self::apply_software_frame(cx, tab, frame);
                if i == active {
                    redraw = true;
                }
            }
        }
        for url in popups {
            self.tabs.insert(&url, true);
            self.apply_active_visibility();
            changed = true;
            redraw = true;
        }
        if changed {
            cx.widget_action(self.uid, WebViewAction::TabsChanged);
        }
        if redraw {
            self.redraw(cx);
        }
    }

    // ------------------------------------------------------------------
    // Input routing (same mapping as the stock Browser widget)
    // ------------------------------------------------------------------

    fn browser_rect(&self, cx: &mut Cx) -> Option<Rect> {
        let area = self.draw_bg.area();
        if area.is_valid(cx) {
            Some(area.rect(cx))
        } else {
            None
        }
    }

    /// CEF takes mouse coordinates in view points (it applies the device
    /// scale factor itself).
    fn cef_position(&self, cx: &mut Cx, abs: Vec2d) -> Option<(i32, i32)> {
        let rect = self.browser_rect(cx)?;
        let local = abs - rect.pos;
        Some((local.x.round() as i32, local.y.round() as i32))
    }

    fn send_mouse_move(&mut self, cx: &mut Cx, abs: Vec2d, modifiers: KeyModifiers, leave: bool) {
        let Some((x, y)) = self.cef_position(cx, abs) else {
            return;
        };
        let m = BrowserKeys::cef_modifiers(modifiers, self.pressed_buttons);
        if let Some(browser) = self.active_browser() {
            let _ = browser.send_mouse_move(x, y, m, leave);
        }
    }

    fn send_mouse_click(
        &mut self,
        cx: &mut Cx,
        abs: Vec2d,
        modifiers: KeyModifiers,
        button: Option<MouseButton>,
        mouse_up: bool,
        click_count: i32,
    ) {
        let Some((x, y)) = self.cef_position(cx, abs) else {
            return;
        };
        let m = BrowserKeys::cef_modifiers(modifiers, self.pressed_buttons);
        let b = BrowserKeys::cef_mouse_button(button);
        if let Some(browser) = self.active_browser() {
            let _ = browser.send_mouse_click(x, y, m, b, mouse_up, click_count.max(1));
        }
    }

    fn send_mouse_wheel(&mut self, cx: &mut Cx, abs: Vec2d, modifiers: KeyModifiers, delta: Vec2d) {
        let Some((x, y)) = self.cef_position(cx, abs) else {
            return;
        };
        let m = BrowserKeys::cef_modifiers(modifiers, self.pressed_buttons)
            | makepad_cef::EVENTFLAG_PRECISION_SCROLLING_DELTA;
        if let Some(browser) = self.active_browser() {
            let _ = browser.send_mouse_wheel(x, y, m, delta.x.round() as i32, delta.y.round() as i32);
        }
    }

    fn send_key(&mut self, key_event: &KeyEvent, event_type: i32) {
        let modifiers = BrowserKeys::key_event_modifiers(key_event);
        let windows_key_code = BrowserKeys::windows_key_code(key_event.key_code);
        let character = if key_event.modifiers.control
            || key_event.modifiers.alt
            || key_event.modifiers.logo
        {
            0
        } else {
            BrowserKeys::key_char(key_event.key_code, key_event.modifiers.shift)
                .map(|ch| ch as u16)
                .unwrap_or(0)
        };
        let send_char = event_type == makepad_cef::KEY_EVENT_KEYDOWN
            && character != 0
            && !key_event.modifiers.control
            && !key_event.modifiers.alt
            && !key_event.modifiers.logo
            && BrowserKeys::sends_char_on_keydown(key_event.key_code);
        if let Some(browser) = self.active_browser() {
            let _ = browser.send_key_event(
                event_type,
                modifiers,
                windows_key_code,
                windows_key_code,
                character,
                character,
                false,
            );
            if send_char {
                let _ = browser.send_key_event(
                    makepad_cef::KEY_EVENT_CHAR,
                    modifiers,
                    windows_key_code,
                    windows_key_code,
                    character,
                    character,
                    false,
                );
            }
        }
    }

    fn update_ime_spot(&self, cx: &mut Cx, pos: Vec2d) {
        let area = self.draw_bg.area();
        if area.is_valid(cx) {
            cx.show_text_ime(area, pos);
        }
    }
}

impl Widget for WebView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::Startup = event {
            if !self.pump_started {
                self.pump_started = true;
                self.pump_timer = cx.start_interval(Self::PUMP_INTERVAL);
            }
        }
        if let Event::Shutdown = event {
            self.tabs.tabs.clear();
            return;
        }
        if !self.cef_ready && self.cef_init_frame.is_event(event).is_some() {
            // The chrome has been drawn: bring Chromium up now.
            match makepad_cef::initialize() {
                Ok(()) => {
                    self.cef_ready = true;
                    log!(
                        "mpbrowser: CEF {} initialized at {} ms",
                        makepad_cef::CEF_VERSION,
                        crate::uptime_ms()
                    );
                }
                Err(err) => {
                    let message = err.to_string();
                    log!("CEF initialize failed: {message}");
                    self.cef_init_error = Some(message);
                }
            }
            self.redraw(cx);
        }
        if self.pump_timer.is_event(event).is_some() && self.cef_ready {
            self.pump(cx);
        }

        match event.hits_with_capture_overload(cx, self.draw_bg.area(), true) {
            Hit::KeyFocus(_) => {
                if let Some(browser) = self.active_browser() {
                    let _ = browser.set_focus(true);
                }
                if let Some(rect) = self.browser_rect(cx) {
                    self.update_ime_spot(cx, rect.pos);
                }
            }
            Hit::KeyFocusLost(_) => {
                if let Some(browser) = self.active_browser() {
                    let _ = browser.set_focus(false);
                }
                cx.hide_text_ime();
                self.suppress_next_paste_shortcut = false;
            }
            Hit::FingerDown(fe) => {
                let button = fe.mouse_button().unwrap_or(MouseButton::PRIMARY);
                self.pressed_buttons.insert(button);
                cx.set_key_focus(self.draw_bg.area());
                if let Some(browser) = self.active_browser() {
                    let _ = browser.set_focus(true);
                }
                self.update_ime_spot(cx, fe.abs);
                self.send_mouse_move(cx, fe.abs, fe.modifiers, false);
                self.send_mouse_click(
                    cx,
                    fe.abs,
                    fe.modifiers,
                    Some(button),
                    false,
                    fe.tap_count as i32,
                );
            }
            Hit::FingerMove(fe) => {
                self.send_mouse_move(cx, fe.abs, fe.modifiers, false);
            }
            Hit::FingerUp(fe) => {
                let button = fe.mouse_button().unwrap_or(MouseButton::PRIMARY);
                self.send_mouse_move(cx, fe.abs, fe.modifiers, false);
                self.send_mouse_click(
                    cx,
                    fe.abs,
                    fe.modifiers,
                    Some(button),
                    true,
                    fe.tap_count as i32,
                );
                self.pressed_buttons.remove(button);
            }
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                self.send_mouse_move(cx, fe.abs, fe.modifiers, false);
            }
            Hit::FingerHoverOut(fe) => {
                self.send_mouse_move(cx, fe.abs, fe.modifiers, true);
            }
            Hit::FingerScroll(fe) => {
                self.send_mouse_wheel(cx, fe.abs, fe.modifiers, fe.scroll);
            }
            Hit::KeyDown(key_event) => {
                // Browser-level shortcuts (Cmd+T/W/L/R/[ ]) are the app's;
                // they never reach the page.
                if key_event.modifiers.logo && crate::is_app_shortcut(&key_event) {
                    return;
                }
                if self.suppress_next_paste_shortcut
                    && key_event.key_code == KeyCode::KeyV
                    && key_event.modifiers.is_primary()
                {
                    self.suppress_next_paste_shortcut = false;
                } else {
                    self.send_key(&key_event, makepad_cef::KEY_EVENT_KEYDOWN);
                }
            }
            Hit::KeyUp(key_event) => {
                if key_event.modifiers.logo && crate::is_app_shortcut(&key_event) {
                    return;
                }
                self.send_key(&key_event, makepad_cef::KEY_EVENT_KEYUP);
            }
            Hit::TextInput(text_event) => {
                let ime_pos = self
                    .browser_rect(cx)
                    .map(|rect| rect.pos)
                    .unwrap_or_default();
                self.update_ime_spot(cx, ime_pos);
                if text_event.was_paste {
                    self.suppress_next_paste_shortcut = true;
                }
                let modifiers = BrowserKeys::cef_modifiers(cx.keyboard.modifiers(), MouseButton::empty());
                let char_data = BrowserKeys::char_event_data(&text_event.input);
                if let Some(browser) = self.active_browser() {
                    if text_event.was_paste || text_event.replace_last || char_data.is_none() {
                        let _ = browser.ime_commit_text(&text_event.input);
                    } else if let Some((windows_key_code, character)) = char_data {
                        let _ = browser.send_key_event(
                            makepad_cef::KEY_EVENT_CHAR,
                            modifiers,
                            windows_key_code,
                            windows_key_code,
                            character,
                            character,
                            false,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.peek_walk_turtle(walk);
        let dpi = cx.current_dpi_factor() as f32;
        let width = (rect.size.x.max(1.0) * dpi as f64).round().max(1.0) as usize;
        let height = (rect.size.y.max(1.0) * dpi as f64).round().max(1.0) as usize;

        self.ensure_active_browser(cx, width, height, dpi);

        let texture = self.tabs.active().and_then(|t| t.texture.clone());
        // Themed ground under the page, so a tab that has not painted yet
        // (or a transparent page) never shows black.
        self.draw_empty.draw_abs(cx, rect);
        if !self.cef_ready {
            let status = match &self.cef_init_error {
                Some(err) => format!("browser engine failed to start: {err}"),
                None => "starting browser engine…".to_string(),
            };
            self.draw_status
                .draw_abs(cx, rect.pos + dvec2(16.0, 14.0), &status);
            if !self.cef_init_requested && self.cef_init_error.is_none() {
                self.cef_init_requested = true;
                self.cef_init_frame = cx.new_next_frame();
            }
        }
        match texture {
            Some(texture) => {
                self.draw_bg.draw_vars.set_texture(0, &texture);
                self.draw_bg.opacity = 1.0;
            }
            None => {
                self.draw_bg.draw_vars.empty_texture(0);
                self.draw_bg.opacity = 0.0;
            }
        }
        self.draw_bg.draw_walk(cx, walk);
        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        DrawStep::done()
    }
}

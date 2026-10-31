//! The page area. One widget shows every tab's page; the active tab's page
//! is what gets drawn and gets the input, background tabs are told they are
//! hidden so they stop painting.
//!
//! The pages live in the browser host (libs/browser_view): Chromium runs in
//! that process, never in this one. Each tab is one page of it, a hosted
//! window whose frames arrive as a shared GPU texture the quad samples
//! directly; input goes to it the hosted-app way. Without a browser host
//! there are no pages and the area shows its ground.

use crate::tabs::{TabId, TabModel, TabSummary};
use makepad_browser_view::{BrowserViews, ViewOp};
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
            color: uniform(mod.theme.color_bg_container)
            pixel: fn() {
                return self.color
            }
        }
        draw_status +: {
            color: mod.theme.color_text_disabled
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
}

/// The profile the pages keep cookies and logins in: the one this app's
/// own Chromium kept before the pages moved to the browser host.
const PROFILE: &str = "makepad-app-browser";

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
    #[rust]
    views: Option<BrowserViews>,
    #[rust]
    first_frame_logged: bool,
    #[rust]
    page_appearance: Option<(bool, u32)>,
}

impl WebView {
    fn sync_page_appearance(&mut self, cx: &mut Cx) {
        let palette = crate::palette();
        let wanted = (palette.is_dark(), palette.page_background_argb());
        if self.page_appearance == Some(wanted) {
            return;
        }
        self.page_appearance = Some(wanted);
        let Some(views) = &mut self.views else { return };
        for tab in &mut self.tabs.tabs {
            let Some(view) = tab.view else { continue };
            views.send(view, ViewOp::Appearance { dark: wanted.0, background: wanted.1 });
            if crate::theme::is_new_tab_url(&tab.url) {
                let url = palette.new_tab_url();
                views.send(view, ViewOp::Navigate { url: url.clone() });
                tab.url = url;
            }
        }
        log!("browser: page color scheme {}", if wanted.0 { "dark" } else { "light" });
        self.redraw(cx);
    }

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
            if let (Some(view), Some(views)) = (tab.view, &mut self.views) {
                views.close(view);
                views.service(cx);
            }
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

    /// Title, URL and active marker for the browser's AI `tabs` tool.
    pub fn ai_tabs(&self) -> Vec<(String, String, bool)> {
        self.tabs
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| (tab.display_title(), tab.url.clone(), index == self.tabs.active))
            .collect()
    }

    pub fn active_info(&self) -> ActiveInfo {
        let Some(tab) = self.tabs.active() else {
            return ActiveInfo::default();
        };
        ActiveInfo {
            id: Some(tab.id),
            url: tab.url.clone(),
            title: tab.display_title(),
            loading: tab.loading,
            can_go_back: tab.can_go_back,
            can_go_forward: tab.can_go_forward,
        }
    }

    pub fn navigate(&mut self, cx: &mut Cx, url: &str) {
        if let Some(tab) = self.tabs.active_mut() {
            tab.url = url.to_string();
            tab.title.clear();
            tab.favicon = None;
            tab.loading = true;
            match (tab.view, &mut self.views) {
                (Some(view), Some(views)) => views.send(view, ViewOp::Navigate { url: url.to_string() }),
                _ => tab.initial_url = url.to_string(),
            }
        }
        self.focus(cx);
        self.notify(cx);
    }

    fn send_active(&mut self, op: ViewOp) {
        let Some(view) = self.tabs.active().and_then(|tab| tab.view) else { return };
        if let Some(views) = &mut self.views {
            views.send(view, op);
        }
    }

    pub fn go_back(&mut self, cx: &mut Cx) {
        self.send_active(ViewOp::Back);
        self.focus(cx);
    }

    pub fn go_forward(&mut self, cx: &mut Cx) {
        self.send_active(ViewOp::Forward);
        self.focus(cx);
    }

    pub fn reload(&mut self, cx: &mut Cx) {
        self.send_active(ViewOp::Reload);
        self.focus(cx);
    }

    pub fn stop(&mut self, cx: &mut Cx) {
        self.send_active(ViewOp::Stop);
        self.focus(cx);
    }

    pub fn focus(&mut self, cx: &mut Cx) {
        let area = self.draw_bg.area();
        if area.is_valid(cx) {
            cx.set_key_focus(area);
        }
        self.send_active(ViewOp::Focus { on: true });
    }

    // ------------------------------------------------------------------

    fn notify(&mut self, cx: &mut Cx) {
        cx.widget_action(self.uid, WebViewAction::TabsChanged);
        self.redraw(cx);
    }

    /// Background tabs stop painting; the active one is shown.
    fn apply_active_visibility(&mut self) {
        let active = self.tabs.active;
        let Some(views) = &mut self.views else { return };
        for (i, tab) in self.tabs.tabs.iter().enumerate() {
            if let Some(view) = tab.view {
                views.send(view, ViewOp::Hidden { on: i != active });
            }
        }
    }

    /// The active tab's page exists and is `size` points big.
    fn ensure_active_page(&mut self, cx: &mut Cx, size: Vec2d, dpi: f64) {
        let (dark, background) = (crate::palette().is_dark(), crate::palette().page_background_argb());
        let active = self.tabs.active;
        let views = self.views.get_or_insert_with(|| BrowserViews::new(PROFILE));
        let Some(tab) = self.tabs.tabs.get_mut(active) else { return };
        if tab.view.is_none() && tab.init_error.is_none() {
            match views.open(&tab.initial_url, dark, background) {
                Some(view) => tab.view = Some(view),
                // No browser host: no page, quietly (browser_view said why once).
                None => tab.init_error = Some("no browser engine".to_string()),
            }
        }
        if let Some(view) = tab.view {
            views.set_size(view, size, dpi);
        }
        views.service(cx);
    }

    /// The pages' news: states, popups, errors, frames.
    fn sync_tabs(&mut self, cx: &mut Cx, event: &Event) {
        let Some(views) = &mut self.views else { return };
        if !views.handle_event(cx, event) {
            return;
        }
        let active = self.tabs.active;
        let mut changed = false;
        let mut popups = Vec::new();
        for (i, tab) in self.tabs.tabs.iter_mut().enumerate() {
            let Some(view) = tab.view else { continue };
            popups.extend(views.take_popups(view));
            let Some(state) = views.state(view) else { continue };
            if let Some(error) = &state.error {
                if tab.init_error.is_none() {
                    tab.init_error = Some(error.clone());
                    changed = true;
                }
            }
            if !state.url.is_empty() && (tab.url != state.url || tab.title != state.title || tab.loading != state.loading
                || tab.can_go_back != state.can_go_back || tab.can_go_forward != state.can_go_forward)
            {
                tab.url = state.url.clone();
                tab.title = state.title.clone();
                tab.loading = state.loading;
                tab.can_go_back = state.can_go_back;
                tab.can_go_forward = state.can_go_forward;
                changed = true;
            }
            if i == active && !self.first_frame_logged && views.frame(view).is_some() {
                self.first_frame_logged = true;
                log!("browser: first page frame at {} ms", crate::uptime_ms());
            }
        }
        for url in popups {
            self.tabs.insert(&url, true);
            changed = true;
        }
        if changed {
            self.apply_active_visibility();
            cx.widget_action(self.uid, WebViewAction::TabsChanged);
        }
        self.redraw(cx);
    }
}

impl Widget for WebView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.sync_page_appearance(cx);
        if let Event::Shutdown = event {
            if let Some(views) = &mut self.views {
                views.shutdown(cx);
            }
            self.tabs.tabs.clear();
            return;
        }
        self.sync_tabs(cx, event);

        let Some(view) = self.tabs.active().and_then(|tab| tab.view) else { return };
        let area = self.draw_bg.area();
        let hit = event.hits_with_capture_overload(cx, area, true);
        match &hit {
            Hit::KeyFocus(_) => self.send_active(ViewOp::Focus { on: true }),
            Hit::KeyFocusLost(_) => self.send_active(ViewOp::Focus { on: false }),
            // Browser-level shortcuts (Cmd+T/W/L/R/[ ]) are the app's; they
            // never reach the page.
            Hit::KeyDown(key) | Hit::KeyUp(key) if key.modifiers.logo && crate::is_app_shortcut(key) => return,
            Hit::FingerDown(_) => cx.set_key_focus(area),
            _ => {}
        }
        let rect = area.is_valid(cx).then(|| area.rect(cx));
        let Some(views) = &mut self.views else { return };
        views.forward_hit(cx, view, &hit, |abs| rect.map(|rect| abs - rect.pos));
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.peek_walk_turtle(walk);
        let dpi = cx.current_dpi_factor();
        self.ensure_active_page(cx, rect.size, dpi);

        // Themed ground under the page, so a tab that has not painted yet
        // (or a transparent page) never shows black.
        self.draw_empty.draw_abs(cx, rect);
        let shown = self.tabs.active().and_then(|tab| tab.view).and_then(|view| self.views.as_ref()?.frame(view));
        match shown {
            Some(frame) if frame.info.width > 0 && frame.info.height > 0 => {
                // The frame's share of the slot, stretched over the current
                // rect: mid-drag the last good frame at a slightly older
                // size, never a blank margin.
                self.draw_bg.image_scale = vec2(frame.info.width as f32 / frame.alloc[0] as f32, frame.info.height as f32 / frame.alloc[1] as f32);
                self.draw_bg.draw_vars.set_texture(0, &frame.texture);
                self.draw_bg.opacity = 1.0;
            }
            _ => {
                self.draw_bg.image_scale = vec2(1.0, 1.0);
                self.draw_bg.draw_vars.empty_texture(0);
                self.draw_bg.opacity = 0.0;
            }
        }
        self.draw_bg.draw_walk(cx, walk);
        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        DrawStep::done()
    }
}

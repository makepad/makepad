//! One web-view API for OS-composited content and the engine host's textures.

use crate::{image::DrawImage, makepad_derive_widget::*, makepad_draw::*, textured::TexturedWebView, widget::*};
use crate::makepad_platform::native_layer::NativeLayerOperation;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
struct WebViewAttachments {
    redraw: u64,
    passes: HashMap<DrawPassId, HashSet<DrawListId>>,
}

pub(crate) fn area_is_drawn(cx: &mut Cx, area: Area) -> bool {
    if !area.is_valid(cx) { return false; }
    let clip = area.clipped_rect(cx);
    if clip.size.x <= 0.0 || clip.size.y <= 0.0 { return false; }
    let Some(pass) = area.draw_list_id().and_then(|list| cx.draw_lists[list].draw_pass_id) else { return false };
    if cx.pass_attachment_is_stale(pass) { return false; }
    let redraw = cx.redraw_id;
    let cache = cx.global::<WebViewAttachments>();
    if cache.redraw != redraw {
        cache.redraw = redraw;
        cache.passes.clear();
    }
    if !cache.passes.contains_key(&pass) {
        let attached = cx.attached_draw_lists(pass);
        cx.global::<WebViewAttachments>().passes.insert(pass, attached);
    }
    area.is_attached(cx, &cx.get_global_ref::<WebViewAttachments>().unwrap().passes[&pass])
}

script_mod! {
    use mod.prelude.widgets_internal.*

    set_type_default() do #(DrawWebViewBg::script_shader(vm)){
        ..mod.draw.DrawColor
        border_radius: 0.0
        opacity: 1.0
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, self.border_radius)
            sdf.fill(vec4(self.color.xyz, self.color.w * self.opacity))
            return sdf.result
        }
    }

    mod.widgets.WebViewBase = #(WebView::register_widget(vm))
    mod.widgets.WebView = set_type_default() do mod.widgets.WebViewBase{
        width: Fill
        height: Fill
        draw_bg +: {color: theme.color_bg_app}
        // The frame ring (and HAL texture aliases) already contain premultiplied
        // pixels. Keep their RGB intact; only coverage and opacity scale them.
        draw_image +: {
            pixel: fn() {
                let picture = self.get_color() * self.opacity
                if self.border_radius <= 0.0 { return picture }
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, self.border_radius)
                sdf.fill_keep_premul(picture)
                return sdf.result
            }
        }
        draw_placeholder +: {
            color: theme.color_label_outer
            text_style: theme.font_regular{font_size: 11}
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawWebViewBg {
    #[deref]
    draw_super: DrawColor,
    #[live]
    border_radius: f32,
    #[live(1.0)]
    opacity: f32,
}

/// Both realizations publish the same content and focus events.
#[derive(Clone, Debug, Default)]
pub enum WebViewAction {
    #[default]
    None,
    Realized(WebViewMode),
    Title(String),
    Url(String),
    Loading(bool),
    Nav { can_back: bool, can_forward: bool },
    FocusIn,
    FocusOut,
    Cursor(u32),
    Crashed(String),
    Closed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WebViewMode {
    #[default]
    Pending,
    Composited,
    Textured,
    Unsupported,
}

#[derive(Clone, Debug, Default)]
pub struct WebViewState {
    pub mode: WebViewMode,
    pub title: String,
    pub url: String,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub error: Option<String>,
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
    draw_bg: DrawWebViewBg,
    #[live]
    draw_image: DrawImage,
    #[live]
    draw_placeholder: DrawText,
    #[live]
    url: ArcStringMut,
    #[live]
    border_radius: f64,
    #[live(1.0)]
    opacity: f64,
    #[live]
    input_passthrough: bool,
    /// Use the capture/effects-compatible engine-host texture even when the
    /// OS supports compositing. Once textured, a page stays so until closed.
    #[live]
    prefer_textured: bool,
    #[visible]
    #[live(true)]
    visible: bool,
    #[rust]
    last_url: String,
    #[rust]
    layer: Option<NativeLayer>,
    #[rust]
    textured: TexturedWebView,
    #[rust]
    fallback: bool,
    #[rust]
    closed: bool,
    #[rust]
    focused: bool,
    #[rust]
    pending: Vec<NativeLayerOperation>,
    #[rust]
    state: WebViewState,
}

impl WebView {
    fn use_textured(&mut self, cx: &mut Cx) {
        self.fallback = true;
        if let Some(layer) = self.layer.take() {
            cx.native_layer(layer.id()).close();
        }
        self.textured.open(cx, self.url.as_ref());
        if !self.textured.is_open() {
            self.content_event(cx, NativeLayerEventKind::Realized(NativeLayerMode::Unsupported));
        }
        self.last_url = self.url.as_ref().to_owned();
        self.pending.retain(|op| !matches!(op, NativeLayerOperation::Navigate { .. }));
        if self.focused {
            self.textured.send(&NativeLayerOperation::Focus);
        }
    }

    fn sync_layer(&mut self, cx: &mut Cx) {
        if self.closed { return; }
        if !self.fallback && (self.prefer_textured || self.layer.as_ref().is_some_and(|layer| matches!(layer.mode(cx), NativeLayerMode::Unsupported))) {
            self.use_textured(cx);
        }
        if !self.fallback && self.layer.is_none() {
            self.layer = Some(cx.native_layer_new(NativeContent::Web { url: self.url.as_ref().to_owned() }));
            self.last_url = self.url.as_ref().to_owned();
        }
        if self.last_url != self.url.as_ref() {
            self.send(cx, NativeLayerOperation::Navigate { url: self.url.as_ref().to_owned(), replace: false });
            self.last_url = self.url.as_ref().to_owned();
        }
        let ready = self.textured.is_open() || self.layer.as_ref().is_some_and(|layer| !matches!(layer.mode(cx), NativeLayerMode::Pending));
        if ready {
            for op in std::mem::take(&mut self.pending) { self.send(cx, op); }
        }
    }

    fn send(&mut self, cx: &mut Cx, op: NativeLayerOperation) {
        if self.textured.is_open() {
            self.textured.send(&op);
        } else if let Some(layer) = &self.layer {
            if matches!(layer.mode(cx), NativeLayerMode::Pending | NativeLayerMode::Unsupported) {
                self.pending.push(op);
                return;
            }
            let mut layer = cx.native_layer(layer.id());
            match op {
                NativeLayerOperation::Navigate { url, replace } => layer.navigate(&url, replace),
                NativeLayerOperation::History(delta) => layer.history(delta),
                NativeLayerOperation::Reload => layer.reload(),
                NativeLayerOperation::Stop => layer.stop(),
                NativeLayerOperation::Focus => layer.focus(),
                NativeLayerOperation::Blur => layer.blur(),
                _ => {}
            }
        } else if !self.closed && !self.fallback {
            self.pending.push(op);
        }
    }

    fn content_event(&mut self, cx: &mut Cx, kind: NativeLayerEventKind) {
        match &kind {
            NativeLayerEventKind::Title(title) => self.state.title.clone_from(title),
            NativeLayerEventKind::Url(url) => {
                self.state.url.clone_from(url);
                self.url.set(url);
                self.last_url.clone_from(url);
            }
            NativeLayerEventKind::Loading(loading) => self.state.loading = *loading,
            NativeLayerEventKind::Nav { can_back, can_forward } => {
                self.state.can_go_back = *can_back;
                self.state.can_go_forward = *can_forward;
            }
            NativeLayerEventKind::Crashed(error) => self.state.error = Some(error.clone()),
            NativeLayerEventKind::Closed => {
                self.closed = true;
                self.state.loading = false;
            }
            NativeLayerEventKind::FocusIn => {
                self.focused = true;
                cx.set_key_focus(self.draw_bg.area());
            }
            NativeLayerEventKind::FocusOut => {
                self.focused = false;
                if cx.has_key_focus(self.draw_bg.area()) { cx.set_key_focus(Area::Empty); }
            }
            _ => {}
        }
        let action = match kind {
            NativeLayerEventKind::Realized(mode) => {
                self.state.mode = match mode {
                    NativeLayerMode::Pending => WebViewMode::Pending,
                    NativeLayerMode::Composited => WebViewMode::Composited,
                    NativeLayerMode::Textured(_) => WebViewMode::Textured,
                    NativeLayerMode::Unsupported => WebViewMode::Unsupported,
                };
                WebViewAction::Realized(self.state.mode)
            }
            NativeLayerEventKind::Title(title) => WebViewAction::Title(title),
            NativeLayerEventKind::Url(url) => WebViewAction::Url(url),
            NativeLayerEventKind::Loading(loading) => WebViewAction::Loading(loading),
            NativeLayerEventKind::Nav { can_back, can_forward } => WebViewAction::Nav { can_back, can_forward },
            NativeLayerEventKind::FocusIn => WebViewAction::FocusIn,
            NativeLayerEventKind::FocusOut => WebViewAction::FocusOut,
            NativeLayerEventKind::Cursor(cursor) => WebViewAction::Cursor(cursor),
            NativeLayerEventKind::Crashed(error) => WebViewAction::Crashed(error),
            NativeLayerEventKind::Closed => WebViewAction::Closed,
        };
        cx.widget_action(self.uid, action);
    }

    fn set_url_internal(&mut self, cx: &mut Cx, url: &str) {
        if self.closed {
            self.closed = false;
            self.fallback = false;
        }
        self.url.set(url);
        self.state.error = None;
        self.last_url = url.to_owned();
        if self.layer.is_some() || self.textured.is_open() {
            self.send(cx, NativeLayerOperation::Navigate { url: url.to_owned(), replace: false });
        }
        self.redraw(cx);
    }

    fn set_visible_internal(&mut self, cx: &mut Cx, visible: bool) {
        if self.visible != visible {
            self.visible = visible;
            self.textured.set_hidden(!visible);
            if !visible { self.set_focus(cx, false); }
            self.redraw(cx);
        }
    }

    fn set_focus(&mut self, cx: &mut Cx, focus: bool) {
        let was_focused = self.focused;
        self.send(cx, if focus { NativeLayerOperation::Focus } else { NativeLayerOperation::Blur });
        if self.focused != focus {
            self.content_event(cx, if focus { NativeLayerEventKind::FocusIn } else { NativeLayerEventKind::FocusOut });
        }
        if self.textured.is_open() {
            if focus { self.show_ime(cx); } else if was_focused { cx.hide_text_ime(); }
        }
    }

    fn show_ime(&self, cx: &mut Cx) {
        let area = self.draw_bg.area();
        if area.is_valid(cx) && cx.has_key_focus(area) {
            // The current engine host exposes committed text input but no
            // caret rectangle. Anchor to the widget until it reports one.
            cx.show_text_ime_with_config(area, Rect { pos: dvec2(0.0, 0.0), size: area.rect(cx).size }, TextInputConfig::default());
        }
    }

    fn close(&mut self, cx: &mut Cx) {
        self.set_focus(cx, false);
        self.textured.close(cx);
        if let Some(layer) = self.layer.take() { cx.native_layer(layer.id()).close(); }
        self.pending.clear();
        self.closed = true;
        self.content_event(cx, NativeLayerEventKind::Closed);
        self.redraw(cx);
    }
}

impl Widget for WebView {
    fn semantics(&self, _cx: &Cx, node: &mut a11y_tree::Node) {
        node.role = a11y_tree::Role::Document;
        makepad_platform::a11y::set_string(&mut node.class, "WebView");
        makepad_platform::a11y::set_string(
            &mut node.name,
            if self.state.title.is_empty() { self.url.as_ref() } else { &self.state.title },
        );
        node.states.set(a11y_tree::States::BUSY, self.state.loading);
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::Shutdown = event {
            self.close(cx);
            return;
        }
        if let Event::NativeLayer(event) = event {
            if self.layer.as_ref().is_some_and(|layer| layer.id() == event.layer) {
                if matches!(event.kind, NativeLayerEventKind::Realized(NativeLayerMode::Unsupported)) {
                    self.use_textured(cx);
                } else {
                    self.content_event(cx, event.kind.clone());
                    if matches!(event.kind, NativeLayerEventKind::Closed) {
                        self.layer = None;
                    }
                }
                self.redraw(cx);
            }
        }
        if self.textured.handle_event(cx, event) {
            for kind in self.textured.events(&self.state) { self.content_event(cx, kind); }
            self.redraw(cx);
        }
        if matches!(event, Event::WindowGeomChange(_)) { self.redraw(cx); }
        if self.closed { return; }
        let area = self.draw_bg.area();
        let visible = self.visible && area_is_drawn(cx, area);
        self.textured.set_hidden(!visible);
        // Focus loss must reach the page even after it has been hidden or
        // switched to input passthrough.
        if self.focused && (!visible || self.input_passthrough) {
            self.set_focus(cx, false);
        }
        if !visible || self.input_passthrough { return; }
        let rect = area.rect(cx);
        let radius = self.border_radius.max(0.0).min(rect.size.x.min(rect.size.y) * 0.5);
        let hit = event.hits_with_options_and_test(cx, area, HitOptions::new(), |abs, clip, _| {
            if rect.size.x <= 0.0 || rect.size.y <= 0.0 || !clip.contains(abs) { return false; }
            let p = abs - rect.pos;
            let corner = dvec2(p.x.clamp(radius, rect.size.x - radius), p.y.clamp(radius, rect.size.y - radius));
            (p - corner).length() <= radius
        });
        match &hit {
            Hit::KeyFocus(_) => self.set_focus(cx, true),
            Hit::KeyFocusLost(_) => self.set_focus(cx, false),
            Hit::KeyDown(key) if key.key_code == KeyCode::Escape && self.textured.is_open() => {
                self.set_focus(cx, false);
                return;
            }
            Hit::FingerDown(_) if self.textured.is_open() => {
                cx.set_key_focus(area);
                self.set_focus(cx, true);
            }
            _ => {}
        }
        if self.textured.is_open() && area.is_valid(cx) {
            if let (Hit::FingerScroll(scroll), Event::Scroll(event)) = (&hit, event) {
                let mut scroll = scroll.clone();
                if event.handled_x.replace(true) { scroll.scroll.x = 0.0; }
                if event.handled_y.replace(true) { scroll.scroll.y = 0.0; }
                self.textured.forward_hit(cx, &Hit::FingerScroll(scroll), rect);
            } else {
                self.textured.forward_hit(cx, &hit, rect);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            self.textured.set_hidden(true);
            return DrawStep::done();
        }
        self.sync_layer(cx);
        self.draw_bg.border_radius = self.border_radius as f32;
        self.draw_bg.opacity = self.opacity as f32;
        let rect = self.draw_bg.draw_walk(cx, walk);
        if self.closed { return DrawStep::done(); }
        let style = NativeLayerStyle {
            radius: [self.border_radius; 4], opacity: self.opacity, input_passthrough: self.input_passthrough,
        };
        let mode = self.layer.as_ref().map_or(NativeLayerMode::Unsupported, |layer| layer.draw(cx, rect, style));
        if matches!(mode, NativeLayerMode::Unsupported) && !self.fallback {
            self.use_textured(cx);
        }
        self.textured.set_hidden(false);
        let dpi = cx.current_dpi_factor();
        let frame = if self.fallback {
            self.textured.frame(cx, rect.size, dpi)
        } else if let NativeLayerMode::Textured(texture) = mode {
            Some(crate::textured::WebFrame { texture, scale: vec2(1.0, 1.0) })
        } else { None };
        if let Some(frame) = frame {
            self.draw_image.border_radius = self.border_radius as f32;
            self.draw_image.opacity = self.opacity as f32;
            self.draw_image.image_scale = frame.scale;
            self.draw_image.draw_vars.set_texture(0, &frame.texture);
            self.draw_image.draw_abs(cx, rect);
        } else if self.fallback {
            let text = if self.textured.is_open() && self.state.error.is_none() { "Loading web view…" } else { "Web view not available on this platform" };
            self.draw_placeholder.draw_walk(cx, Walk::fixed(rect.size.x, rect.size.y).with_abs_pos(rect.pos), Align { x: 0.5, y: 0.5 }, text);
        }
        if !self.input_passthrough { cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default()); }
        if self.focused && self.textured.is_open() { self.show_ime(cx); }
        DrawStep::done()
    }
}

impl WebViewRef {
    pub fn navigate(&self, cx: &mut Cx, url: &str) {
        self.set_url(cx, url);
    }
    pub fn set_url(&self, cx: &mut Cx, url: &str) {
        if let Some(mut inner) = self.borrow_mut() { inner.set_url_internal(cx, url); }
    }
    pub fn set_visible(&self, cx: &mut Cx, visible: bool) {
        if let Some(mut inner) = self.borrow_mut() { inner.set_visible_internal(cx, visible); }
    }
    pub fn state(&self) -> WebViewState {
        self.borrow().map(|inner| inner.state.clone()).unwrap_or_default()
    }
    pub fn history(&self, cx: &mut Cx, delta: i32) {
        if let Some(mut inner) = self.borrow_mut() { inner.send(cx, NativeLayerOperation::History(delta)); }
    }
    pub fn reload(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() { inner.send(cx, NativeLayerOperation::Reload); }
    }
    pub fn stop(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() { inner.send(cx, NativeLayerOperation::Stop); }
    }
    pub fn focus(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() { inner.set_focus(cx, true); }
    }
    pub fn blur(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() { inner.set_focus(cx, false); }
    }
    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() { inner.close(cx); }
    }
}

//! A HAL child's lent surface in the ordinary desk. The HAL owns compilation and rendering;
//! this widget only presents the latest frame and forwards input in the child's points.
use crate::own_chrome::ChromePresses;
use crate::tile::TileHost;
use makepad_app_host::ClientId;
use makepad_widgets::*;
#[cfg(not(target_arch = "wasm32"))]
use makepad_hal::{input, Id, Range, RawEvent};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    mod.widgets.DrawDomain = set_type_default() do #(DrawDomain::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex: texture_2d(float)
        fade: 1.0
        crop_origin: vec2(0.0, 0.0)
        crop_span: vec2(1.0, 1.0)
        pixel: fn() {
            return self.tex.sample(self.crop_origin + self.pos * self.crop_span) * self.fade
        }
    }
    mod.widgets.MpDomainViewBase = #(MpDomainView::register_widget(vm))
    mod.widgets.MpDomainView = set_type_default() do mod.widgets.MpDomainViewBase {
        width: Fill height: Fill
        draw_bg +: {color: mod.wm_theme.background}
        draw_app: mod.widgets.DrawDomain{}
        status: Label{text: "starting…" width: Fill align: Align{x: 0.5} draw_text.color: #x8E93A8 draw_text.text_style.font_size: 12}
        title: Label{text: "" width: Fill align: Align{x: 0.5} draw_text.color: #xC9CCDA draw_text.text_style.font_size: 14}
        spinner: LoadingSpinner{width: 44 height: 44}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawDomain {
    #[deref] draw_super: DrawQuad,
    #[live] fade: f32,
    #[live] crop_origin: Vec2f,
    #[live] crop_span: Vec2f,
}

#[derive(Script, ScriptHook, Widget)]
pub struct MpDomainView {
    #[uid] uid: WidgetUid,
    #[source] source: ScriptObjectRef,
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[redraw] #[live] draw_bg: DrawColor,
    #[redraw] #[live] draw_app: DrawDomain,
    #[find] #[redraw] #[live] status: WidgetRef,
    #[rust] started: Option<f64>,
    #[rust] line: String,
    #[find] #[redraw] #[live] title: WidgetRef,
    #[find] #[redraw] #[live] spinner: WidgetRef,
    #[rust] progress: f32,
    #[rust] area: Area,
    #[rust] client: Option<ClientId>,
    #[rust] domain: Option<u32>,
    #[rust] window: Option<LentId>,
    #[rust] texture: Option<Texture>,
    #[rust] surface: Option<LentId>,
    #[rust] ready: bool,
    #[rust] asked: Option<(Vec2d, f64)>,
    #[rust] target: Option<Vec2d>,
    #[rust] child_size: Vec2d,
    #[rust] crop: Option<(Vec2d, Vec2d)>,
    #[rust(1.0f32)] fade: f32,
    #[rust(true)] takes_key_focus: bool,
    #[rust] serial: u64,
    /// The app ended: its card has the desk's title bar (`WmDesk::set_own_chrome`).
    #[rust] desk_chrome: bool,
    #[rust] chrome_press: ChromePresses,
    #[rust] last_pointer: Vec2d,
    #[rust] first_frame: Option<f64>,
    #[rust] early_size: Option<(Vec2d, f64)>,
    /// The desk shows none of this tile (`set_visible(false)`), and whether the domain's current
    /// run was told so: a relaunched domain (`clear_window`) is told again once it has drawn.
    #[rust] hidden: bool,
    #[rust] hidden_told: bool,
}

impl MpDomainView {
    pub fn set_progress(&mut self, p: f32) { self.progress = p; }
    pub fn set_client(&mut self, client: ClientId) { self.client = Some(client); }

    pub fn set_own_chrome(&mut self, own: bool) {
        self.desk_chrome = !own;
        self.chrome_press.clear();
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn take_caption_press(&mut self, cx: &mut Cx, request: u32, response: WindowDragQueryResponse) -> Option<Vec2d> {
        use makepad_studio_protocol::{HostedDragAnswer, drag_answer, drag_double};
        let response = if matches!(response, WindowDragQueryResponse::Caption) { drag_answer::CAPTION } else { drag_answer::CLIENT };
        // A domain's caption click (released before the answer) drags nothing.
        let crate::own_chrome::CaptionPress::Drag(abs) = self.chrome_press.answer(&HostedDragAnswer { id: request, response, double_click: drag_double::MAXIMISE })? else { return None };
        if let (Some(domain), Some(window)) = (self.domain, self.window) {
            cx.hal_forward(domain, window, &RawEvent::mouse_cancel(Id::NONE,
                self.last_pointer.x, self.last_pointer.y, MouseButton::PRIMARY.bits(), 0), &[]);
        }
        Some(abs)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_domain(&mut self, domain: u32) { self.domain = Some(domain); }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn child_window(&mut self, cx: &mut Cx, e: &ChildWindowEvent) {
        self.domain = Some(e.child);
        if self.takes_key_focus && cx.has_key_focus(self.area) { cx.hal_focus_child(e.child); }
        if self.surface != Some(e.texture) { self.ready = false; }
        self.surface = Some(e.texture);
        self.window = Some(e.window);
        self.child_size = dvec2(e.width as f64 / e.dpi, e.height as f64 / e.dpi);
        // the WM chose this child's size before its window existed (a phone tile face): ask for it
        // with the first window instead of letting the child open at its own default size
        if let Some((size, dpi)) = self.early_size.take() {
            if self.asked != Some((size, dpi)) {
                self.asked = Some((size, dpi));
                cx.hal_child_geom(e.child, e.window, size, dpi);
            }
        }
        if let Some(texture) = &self.texture {
            texture.set_hal_lent(cx, e.texture);
        } else {
            self.texture = Some(Texture::from_hal_lent(cx, e.texture));
        }
        self.serial += 1;
        self.redraw(cx);
    }

    /// The child's surface size in points (its latest ChildWindow).
    pub fn child_size(&self) -> Vec2d { self.child_size }

    /// The WM decided the child's viewport (a phone face) outside a draw: ask the child for it now,
    /// or with its first window if it has none yet.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn ask_size(&mut self, cx: &mut Cx, size: Vec2d, dpi: f64) {
        let (Some(domain), Some(window)) = (self.domain, self.window) else {
            self.early_size = Some((size, dpi));
            return;
        };
        if self.asked != Some((size, dpi)) {
            self.asked = Some((size, dpi));
            cx.hal_child_geom(domain, window, size, dpi);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn child_frame(&mut self, cx: &mut Cx, surface: LentId) {
        if self.surface != Some(surface) { return; }
        self.ready = true;
        // a covered tile whose domain came back (relaunch, a new window) holds its frames again
        if self.hidden && !self.hidden_told {
            self.tell_visible(cx, false);
        }
        self.first_frame.get_or_insert_with(crate::host::now);
        self.serial += 1;
        self.redraw(cx);
    }

    /// Sends `HostedVisible` to the domain's window. False while there is none.
    fn tell_visible(&mut self, cx: &mut Cx, visible: bool) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if let (Some(domain), Some(window)) = (self.domain, self.window) {
            let json = makepad_studio_protocol::HostedVisible { visible }.to_json();
            let data = Range { off: 0, len: json.len() as u32 };
            cx.hal_forward(domain, window, &RawEvent::app_message(Id::NONE, data), json.as_bytes());
            self.hidden_told = !visible;
            return true;
        }
        let _ = (cx, visible);
        false
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn clear_window(&mut self, cx: &mut Cx) {
        self.chrome_press.clear();
        self.window = None;
        self.hidden_told = false;
        self.texture = None;
        self.surface = None;
        self.ready = false;
        self.asked = None;
        self.first_frame = None;
        self.serial += 1;
        self.redraw(cx);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn local(&self, abs: Vec2d, rect: Rect) -> Vec2d {
        // The desk may animate a cached-size surface into a smaller tile.
        let size = self.child_size;
        dvec2((abs.x - rect.pos.x) * size.x / rect.size.x,
            (abs.y - rect.pos.y) * size.y / rect.size.y)
    }
}

impl TileHost for MpDomainView {
    fn client(&self) -> Option<ClientId> { self.client }
    fn set_status_line(&mut self, cx: &mut Cx, line: &str) {
        if self.started.is_none() { self.started = Some(Cx::time_now()); }
        self.line = line.to_string();
        self.status.set_text(cx, line);
        self.redraw(cx);
    }
    fn focus_keyboard(&mut self, cx: &mut Cx) -> bool {
        if !self.takes_key_focus { return true; }
        if !self.area.is_valid(cx) { return false; }
        cx.set_key_focus(self.area);
        #[cfg(not(target_arch = "wasm32"))]
        cx.hal_focus_child(if self.window.is_some() { self.domain.unwrap_or(0) } else { 0 });
        true
    }
    fn release_keyboard(&mut self, cx: &mut Cx) {
        if cx.has_key_focus(self.area) {
            cx.set_key_focus(Area::Empty);
            #[cfg(not(target_arch = "wasm32"))]
            cx.hal_focus_child(0);
        }
    }
    fn set_takes_key_focus(&mut self, on: bool) { self.takes_key_focus = on; }
    fn set_remote_cursor(&mut self, _cx: &mut Cx, _cursor: MouseCursor) {}
    fn has_frame(&self) -> bool { self.ready }
    fn arrival_fade(&self) -> f32 {
        self.first_frame.map_or(1.0, |at| {
            let t = ((crate::host::now() - at) / 0.28).clamp(0.0, 1.0) as f32;
            t * t * (3.0 - 2.0 * t)
        })
    }
    fn set_target_size(&mut self, size: Option<Vec2d>) { self.target = size; }
    fn set_close_crop(&mut self, crop: Option<(Vec2d, Vec2d)>) { self.crop = crop; }
    fn set_fade(&mut self, fade: f32) { self.fade = fade; }
    fn content_serial(&self) -> Option<u64> { self.has_frame().then_some(self.serial) }
    /// The same `HostedVisible` a process child gets, as an app message to the domain's window:
    /// a covered domain holds its frames (except for a remote request that needs one).
    fn set_visible(&mut self, cx: &mut Cx, visible: bool) -> bool {
        self.hidden = !visible;
        self.tell_visible(cx, visible)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn modifiers(m: &KeyModifiers) -> u32 {
    (if m.shift { input::MOD_SHIFT } else { 0 }) |
    (if m.control { input::MOD_CONTROL } else { 0 }) |
    (if m.alt { input::MOD_ALT } else { 0 }) |
    (if m.logo { input::MOD_LOGO } else { 0 })
}

impl Widget for MpDomainView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // A capture pass can have a different scale. Children render at the window's scale.
        let dpi = cx.get_current_window_id().map(|w| cx.windows[w].window_geom.dpi_factor)
            .unwrap_or_else(|| cx.current_dpi_factor());
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect().dpi_snap(1.0 / dpi);
        #[cfg(not(target_arch = "wasm32"))]
        if let (Some(domain), Some(window)) = (self.domain, self.window) {
            let size = self.target.unwrap_or(rect.size);
            if size.x > 0.0 && size.y > 0.0 && self.asked != Some((size, dpi)) {
                self.asked = Some((size, dpi));
                cx.hal_child_geom(domain, window, size, dpi);
            }
        }
        if let Some(texture) = self.texture.as_ref().filter(|_| self.ready) {
            self.draw_app.set_texture(0, texture);
            let (origin, span) = self.crop.unwrap_or((dvec2(0.0, 0.0), dvec2(1.0, 1.0)));
            self.draw_app.crop_origin = vec2(origin.x as f32, origin.y as f32);
            self.draw_app.crop_span = vec2(span.x as f32, span.y as f32);
            self.draw_app.fade = self.fade * self.arrival_fade();
            self.draw_app.draw_abs(cx, rect);
            if self.arrival_fade() < 1.0 { self.redraw(cx); }
        } else {
            self.draw_bg.draw_abs(cx, rect);
            let color_keep = self.draw_bg.color;
            // the loading card: spinner centred, the app and what the phone is doing below it in quiet
            // text, a thin bar and the live JIT count (the compile's own counters) under that
            let working = !self.line.starts_with("crashed") && !self.line.starts_with("exited");
            let mut lines = self.line.splitn(2, '\n');
            let (name, what) = (lines.next().unwrap_or(""), lines.next().unwrap_or(""));
            let cx0 = rect.pos.x + rect.size.x * 0.5;
            let cy0 = rect.pos.y + rect.size.y * 0.5;
            if working {
                self.spinner.draw_walk_all(cx, scope, Walk::abs_rect(Rect { pos: dvec2(cx0 - 22.0, cy0 - 60.0), size: dvec2(44.0, 44.0) }));
            }
            let bar = Rect { pos: dvec2(cx0 - rect.size.x * 0.2, cy0 + 8.0), size: dvec2(rect.size.x * 0.4, 2.0) };
            self.draw_bg.color = vec4(1.0, 1.0, 1.0, 0.10);
            self.draw_bg.draw_abs(cx, bar);
            self.draw_bg.color = vec4(1.0, 0.55, 0.12, 0.9);
            self.draw_bg.draw_abs(cx, Rect { pos: bar.pos, size: dvec2(bar.size.x * self.progress.clamp(0.0, 1.0) as f64, bar.size.y) });
            self.draw_bg.color = color_keep;
            let _ = name; self.status.set_text(cx, what);
            self.status.draw_walk_all(cx, scope, Walk::abs_rect(Rect { pos: dvec2(rect.pos.x + 24.0, cy0 + 20.0), size: dvec2((rect.size.x - 48.0).max(1.0), 40.0) }));
            if self.started.is_some() && !self.line.starts_with("crashed") && !self.line.starts_with("exited") { self.redraw(cx); }
        }
        cx.end_turtle_with_area(&mut self.area);
        // a phone raises its keyboard when the app inside asks (its own text input), never for the tile
        if !cfg!(target_os = "android") && self.window.is_some() && cx.has_key_focus(self.area) {
            cx.push_unique_platform_op(CxOsOp::ShowTextIME(self.area,
                Rect { pos: self.last_pointer, size: Vec2d::default() }, TextInputConfig::default()));
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        #[cfg(target_arch = "wasm32")]
        let _ = (cx, event);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(client) = self.client else { return };
            let hit = event.hits(cx, self.area);
            let (Some(domain), Some(window)) = (self.domain, self.window) else {
                if matches!(hit, Hit::FingerDown(_)) {
                    self.focus_keyboard(cx);
                    cx.widget_action(self.uid, crate::run_view::MpRunViewAction::Clicked { client });
                }
                return;
            };
            match hit {
                Hit::KeyFocus(_) => { cx.hal_focus_child(domain); self.redraw(cx); }
                Hit::KeyFocusLost(_) => { cx.hal_focus_child(0); cx.hide_text_ime(); }
                Hit::FingerDown(e) => {
                    if self.takes_key_focus { self.focus_keyboard(cx); }
                    self.last_pointer = self.local(e.abs, e.rect);
                    cx.widget_action(self.uid, crate::run_view::MpRunViewAction::Clicked { client });
                    cx.hal_forward_mouse_down(domain, window, self.local(e.abs, e.rect),
                        e.device.mouse_button().unwrap_or(MouseButton::PRIMARY).bits(), modifiers(&e.modifiers));
                    if !self.desk_chrome && e.device.mouse_button().unwrap_or(MouseButton::PRIMARY) == MouseButton::PRIMARY {
                        let query = self.chrome_press.begin(e.abs, self.last_pointer);
                        cx.hal_query_child_drag(domain, window, query.id, self.last_pointer);
                    }
                }
                Hit::FingerMove(e) => cx.hal_forward_mouse_move(domain, window, self.local(e.abs, e.rect), modifiers(&e.modifiers)),
                Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => cx.hal_forward_mouse_move(domain, window, self.local(e.abs, e.rect), modifiers(&e.modifiers)),
                Hit::FingerUp(e) => {
                    self.chrome_press.end();
                    let p = self.local(e.abs, e.rect);
                    let button = e.device.mouse_button().unwrap_or(MouseButton::PRIMARY).bits();
                    if e.cancelled {
                        cx.hal_forward(domain, window, &RawEvent::mouse_cancel(Id::NONE, p.x, p.y, button, modifiers(&e.modifiers)), &[]);
                    } else { cx.hal_forward_mouse_up(domain, window, p, button, modifiers(&e.modifiers)); }
                }
                Hit::FingerHoverOut(e) => {
                    let p = self.local(e.abs, e.rect);
                    cx.hal_forward(domain, window, &RawEvent::mouse_leave(Id::NONE, p.x, p.y), &[]);
                }
                Hit::FingerScroll(e) => cx.hal_forward_scroll(domain, window, self.local(e.abs, e.rect), e.scroll, e.phase, modifiers(&e.modifiers)),
                Hit::KeyDown(e) => cx.hal_forward(domain, window, &RawEvent::key_down(Id::NONE, e.key_code.to_index(), e.is_repeat, modifiers(&e.modifiers)), &[]),
                Hit::KeyUp(e) => cx.hal_forward(domain, window, &RawEvent::key_up(Id::NONE, e.key_code.to_index(), modifiers(&e.modifiers)), &[]),
                Hit::TextInput(e) => {
                    let data = Range { off: 0, len: e.input.len() as u32 };
                    let raw = if e.replace_last {
                        RawEvent::ime(Id::NONE, input::ime::COMMIT, 0, 1, data)
                    } else {
                        RawEvent::text(Id::NONE, if e.was_paste { input::text::PASTE } else { input::text::TYPED }, data)
                    };
                    cx.hal_forward(domain, window, &raw, e.input.as_bytes());
                }
                _ => {}
            }
        }
    }
}

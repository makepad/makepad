//! A registered Wayland app in an ordinary Makepad draw slot.
use crate::{image::DrawImage, makepad_derive_widget::*, makepad_draw::*, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    mod.widgets.NativeAppViewBase = #(NativeAppView::register_widget(vm))
    mod.widgets.NativeAppView = set_type_default() do mod.widgets.NativeAppViewBase {
        width: Fill height: Fill
        draw_bg +: {color: theme.color_bg_app}
        draw_placeholder +: {color: theme.color_label_outer text_style: theme.font_regular{font_size: 12}}
        draw_image +: {pixel: fn() {return self.get_color()}}
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct NativeAppView {
    #[uid] uid: WidgetUid,
    #[source] source: ScriptObjectRef,
    #[walk] walk: Walk,
    #[redraw] #[live] draw_bg: DrawColor,
    #[live] draw_image: DrawImage,
    #[live] draw_placeholder: DrawText,
    /// A name from the WM's registry, such as wl.foot; never a command line.
    #[live] app: ArcStringMut,
    #[visible] #[live(true)] visible: bool,
    #[rust] layer: Option<NativeLayer>,
    #[rust] last_app: String,
    #[rust] size: (u32, u32),
    #[rust(1.0)] dpi: f64,
    #[rust] focused: bool,
    #[rust] closed: bool,
    #[rust] error: String,
    #[rust] cursor: u32,
}
impl NativeAppView {
    fn focus(&mut self, cx: &mut Cx, on: bool) {
        if self.focused == on { return; }
        self.focused = on;
        if let Some(layer) = &self.layer {
            if on { cx.native_layer(layer.id()).focus(); }
            else { cx.native_layer(layer.id()).blur(); }
        }
    }
    fn close(&mut self, cx: &mut Cx) {
        self.focus(cx, false);
        if let Some(layer) = self.layer.take() { cx.native_layer(layer.id()).close(); }
        self.closed = true;
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn input(&mut self, cx: &mut Cx, hit: &Hit, rect: Rect) {
        use crate::makepad_platform::makepad_hal::{self as hal, RawEvent};
        let Some(layer) = &self.layer else { return };
        let local = |p: Vec2d| (p - rect.pos) * self.dpi;
        let button = |d: &DigitDevice| d.mouse_button().unwrap_or(MouseButton::PRIMARY).bits();
        let mods = |m: &KeyModifiers| (m.shift as u32 * hal::input::MOD_SHIFT) | (m.control as u32 * hal::input::MOD_CONTROL)
            | (m.alt as u32 * hal::input::MOD_ALT) | (m.logo as u32 * hal::input::MOD_LOGO);
        let id = hal::Id::NONE;
        let raw = match hit {
            Hit::FingerDown(e) => { let p = local(e.abs); RawEvent::mouse_down(id, p.x, p.y, button(&e.device), mods(&e.modifiers)) }
            Hit::FingerUp(e) => { let p = local(e.abs); RawEvent::mouse_up(id, p.x, p.y, button(&e.device), mods(&e.modifiers)) }
            Hit::FingerMove(e) => { let p = local(e.abs); RawEvent::mouse_move(id, p.x, p.y, 0.0, 0.0, mods(&e.modifiers)) }
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => { let p = local(e.abs); RawEvent::mouse_move(id, p.x, p.y, 0.0, 0.0, mods(&e.modifiers)) }
            Hit::FingerHoverOut(e) => { let p = local(e.abs); RawEvent::mouse_leave(id, p.x, p.y) }
            Hit::FingerScroll(e) => {
                let p = local(e.abs);
                let precise = e.phase != crate::makepad_platform::event::ScrollPhase::None;
                RawEvent::scroll(id, p.x, p.y, e.scroll.x, e.scroll.y, mods(&e.modifiers), precise, hal::input::scroll::NONE)
            }
            Hit::KeyDown(e) => RawEvent::key_down(id, e.key_code.to_index(), e.is_repeat, mods(&e.modifiers)),
            Hit::KeyUp(e) => RawEvent::key_up(id, e.key_code.to_index(), mods(&e.modifiers)),
            _ => return,
        };
        cx.native_layer(layer.id()).input(&hal::buffers::pack_forward(&raw, &[]));
    }
    #[cfg(target_arch = "wasm32")]
    fn input(&mut self, _: &mut Cx, _: &Hit, _: Rect) {}
}
impl Widget for NativeAppView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _: &mut Scope) {
        if matches!(event, Event::WindowLostFocus(_)) { self.focus(cx, false); }
        if matches!(event, Event::Shutdown) { self.close(cx); return; }
        if let Event::NativeLayer(event) = event {
            if self.layer.as_ref().is_some_and(|l| l.id() == event.layer) {
                match &event.kind {
                    NativeLayerEventKind::FocusIn => { self.focused = true; cx.set_key_focus(self.draw_bg.area()); }
                    NativeLayerEventKind::FocusOut => { self.focused = false; if cx.has_key_focus(self.draw_bg.area()) { cx.set_key_focus(Area::Empty); } }
                    NativeLayerEventKind::Cursor(cursor) => self.cursor = *cursor,
                    NativeLayerEventKind::Crashed(error) => self.error.clone_from(error),
                    NativeLayerEventKind::Closed => self.closed = true,
                    _ => {}
                }
                self.redraw(cx);
            }
        }
        let area = self.draw_bg.area();
        let visible = self.visible && crate::web_view::area_is_drawn(cx, area) && !self.closed;
        if !visible { self.focus(cx, false); return; }
        let rect = area.rect(cx);
        let hit = event.hits(cx, area);
        match &hit {
            Hit::FingerDown(_) => { cx.set_key_focus(area); self.focus(cx, true); }
            Hit::KeyFocus(_) => self.focus(cx, true),
            Hit::KeyFocusLost(_) => self.focus(cx, false),
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => cx.set_cursor(cursor(self.cursor)),
            _ => {}
        }
        if let (Hit::FingerScroll(scroll), Event::Scroll(event)) = (&hit, event) {
            let mut scroll = scroll.clone();
            if event.handled_x.replace(true) { scroll.scroll.x = 0.0; }
            if event.handled_y.replace(true) { scroll.scroll.y = 0.0; }
            self.input(cx, &Hit::FingerScroll(scroll), rect);
        } else { self.input(cx, &hit, rect); }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, _: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible { return DrawStep::done(); }
        if self.last_app != self.app.as_ref() {
            self.close(cx); self.closed = false; self.error.clear(); self.size = (0, 0);
            self.last_app = self.app.as_ref().to_owned();
        }
        if self.layer.is_none() && !self.closed && !self.app.as_ref().is_empty() {
            self.layer = Some(cx.native_layer_new(NativeContent::WaylandApp { app: self.last_app.clone() }));
        }
        let rect = self.draw_bg.draw_walk(cx, walk);
        self.dpi = cx.current_dpi_factor();
        let size = ((rect.size.x * self.dpi).round().max(1.0) as u32, (rect.size.y * self.dpi).round().max(1.0) as u32);
        let mode = if let Some(layer) = &self.layer {
            if self.size != size { cx.native_layer(layer.id()).size(size.0, size.1); self.size = size; }
            layer.draw(cx, rect, NativeLayerStyle::default())
        } else { NativeLayerMode::Unsupported };
        if let NativeLayerMode::Textured(texture) = mode {
            self.draw_image.draw_vars.set_texture(0, &texture);
            self.draw_image.draw_abs(cx, rect);
        } else {
            let text = if !self.error.is_empty() { self.error.as_str() }
                else if self.closed { "Application closed" }
                else if matches!(mode, NativeLayerMode::Pending) { "Starting application…" }
                else { "Only available under the makepad window manager" };
            self.draw_placeholder.draw_walk(cx, Walk::fixed(rect.size.x, rect.size.y).with_abs_pos(rect.pos), Align { x: 0.5, y: 0.5 }, text);
        }
        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        DrawStep::done()
    }
}
impl NativeAppViewRef {
    pub fn close(&self, cx: &mut Cx) { if let Some(mut v) = self.borrow_mut() { v.close(cx); v.redraw(cx); } }
    pub fn set_app(&self, cx: &mut Cx, app: &str) { if let Some(mut v) = self.borrow_mut() { v.app.set(app); v.redraw(cx); } }
}
fn cursor(shape: u32) -> MouseCursor {
    match shape {
        3 => MouseCursor::Help, 4 | 11 | 12 | 33 | 34 => MouseCursor::Hand,
        5 | 6 => MouseCursor::Wait, 7 | 8 => MouseCursor::Crosshair, 9 | 10 => MouseCursor::Text,
        13 | 32 => MouseCursor::Move, 14 | 15 => MouseCursor::NotAllowed, 16 => MouseCursor::Grab, 17 => MouseCursor::Grabbing,
        18 => MouseCursor::EResize, 19 => MouseCursor::NResize, 20 => MouseCursor::NeResize, 21 => MouseCursor::NwResize,
        22 => MouseCursor::SResize, 23 => MouseCursor::SeResize, 24 => MouseCursor::SwResize, 25 => MouseCursor::WResize,
        26 => MouseCursor::EwResize, 27 => MouseCursor::NsResize, 28 => MouseCursor::NeswResize, 29 => MouseCursor::NwseResize,
        30 => MouseCursor::ColResize, 31 => MouseCursor::RowResize, _ => MouseCursor::Default,
    }
}

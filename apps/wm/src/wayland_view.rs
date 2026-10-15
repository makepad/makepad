//! A tile showing one foreign Wayland window (wayland_host.rs): the toplevel, its subsurfaces
//! and its popups as textured quads, with pointer, scroll and key input sent back to the client.

use makepad_app_host::ClientId;
use crate::run_view::MpRunViewAction;
use crate::tile::TileHost;
use crate::wayland_host::{self, Layer, Surf, ViewCmd};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawForeign::script_shader(vm)) {
        ..mod.draw.DrawQuad
        tex: texture_2d(float)
        opaque: 1.0
        uv_pos: vec2(0.0, 0.0)
        uv_size: vec2(1.0, 1.0)
        fade: 1.0
        pixel: fn() {
            let c = self.tex.sample(self.uv_pos + self.pos * self.uv_size)
            // Wayland buffers are premultiplied; an X-format buffer has no alpha at all.
            let a = mix(c.a, 1.0, self.opaque)
            return vec4(c.rgb, a) * self.fade
        }
    }

    mod.widgets.MpWaylandViewBase = #(MpWaylandView::register_widget(vm))

    mod.widgets.MpWaylandView = set_type_default() do mod.widgets.MpWaylandViewBase {
        width: Fill
        height: Fill
        draw_bg +: { color: mod.wm_theme.background }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawForeign {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub opaque: f32,
    #[live]
    pub uv_pos: Vec2f,
    #[live]
    pub uv_size: Vec2f,
    #[live]
    pub fade: f32,
}

#[derive(Script, Widget)]
pub struct MpWaylandView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_fg: DrawForeign,
    #[rust]
    area: Area,
    #[rust]
    client: Option<ClientId>,
    #[rust(true)]
    takes_key_focus: bool,
    #[rust]
    focused: bool,
    #[rust]
    drawn: bool,
    /// None of the window shows (`set_visible`): its frame callbacks wait,
    /// which is how a Wayland client is told to stop animating.
    #[rust]
    hidden: bool,
    #[rust(1.0f32)]
    fade: f32,
    /// The surface the pointer is inside, if any.
    #[rust]
    ptr_surf: Option<Surf>,
    /// The last size sent to the client, and the layout's settled size.
    #[rust]
    sent: (i32, i32, bool),
    #[rust]
    target: Option<Vec2d>,
    /// Popups draw in an overlay list so they can reach past the tile; this is their hit area.
    #[rust]
    overlay: Option<DrawList2d>,
    #[rust]
    popup_area: Area,
    #[rust]
    hovered: bool,
    #[rust]
    last_scale: f64,
    #[rust]
    bounds_sent: (i32, i32, i32, i32),
    /// The last line `MAKEPAD_WM_TRACE_WAYLAND` printed for this tile, so it prints on change only.
    #[rust]
    traced: String,
}

impl ScriptHook for MpWaylandView {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.overlay = Some(DrawList2d::script_new(vm));
    }
}

/// `wp_cursor_shape_v1` shape number to our cursor.
fn cursor_of(shape: u32) -> MouseCursor {
    match shape {
        2 => MouseCursor::Default, 3 => MouseCursor::Help, 4 => MouseCursor::Hand, 5 => MouseCursor::Wait, 6 => MouseCursor::Wait,
        7 => MouseCursor::Crosshair, 8 => MouseCursor::Crosshair, 9 => MouseCursor::Text, 10 => MouseCursor::Text, 11 => MouseCursor::Hand,
        12 => MouseCursor::Hand, 13 => MouseCursor::Move, 14 => MouseCursor::NotAllowed, 15 => MouseCursor::NotAllowed, 16 => MouseCursor::Grab,
        17 => MouseCursor::Grabbing, 18 => MouseCursor::EResize, 19 => MouseCursor::NResize, 20 => MouseCursor::NeResize, 21 => MouseCursor::NwResize,
        22 => MouseCursor::SResize, 23 => MouseCursor::SeResize, 24 => MouseCursor::SwResize, 25 => MouseCursor::WResize, 26 => MouseCursor::EwResize,
        27 => MouseCursor::NsResize, 28 => MouseCursor::NeswResize, 29 => MouseCursor::NwseResize, 30 => MouseCursor::ColResize, 31 => MouseCursor::RowResize,
        32 => MouseCursor::Move, 33 => MouseCursor::Hand, 34 => MouseCursor::Hand,
        _ => MouseCursor::Default,
    }
}

/// Linux input event code for a key (`KEY_*`), for the keys the US layout table has.
pub fn evdev_of(k: KeyCode) -> Option<u16> {
    use KeyCode::*;
    Some(match k {
        Escape => 1, Key1 => 2, Key2 => 3, Key3 => 4, Key4 => 5, Key5 => 6, Key6 => 7, Key7 => 8, Key8 => 9, Key9 => 10, Key0 => 11,
        Minus => 12, Equals => 13, Backspace => 14, Tab => 15,
        KeyQ => 16, KeyW => 17, KeyE => 18, KeyR => 19, KeyT => 20, KeyY => 21, KeyU => 22, KeyI => 23, KeyO => 24, KeyP => 25,
        LBracket => 26, RBracket => 27, ReturnKey => 28, Control => 29,
        KeyA => 30, KeyS => 31, KeyD => 32, KeyF => 33, KeyG => 34, KeyH => 35, KeyJ => 36, KeyK => 37, KeyL => 38,
        Semicolon => 39, Quote => 40, Backtick => 41, Shift => 42, Backslash => 43,
        KeyZ => 44, KeyX => 45, KeyC => 46, KeyV => 47, KeyB => 48, KeyN => 49, KeyM => 50,
        Comma => 51, Period => 52, Slash => 53, NumpadMultiply => 55, Alt => 56, Space => 57, Capslock => 58,
        F1 => 59, F2 => 60, F3 => 61, F4 => 62, F5 => 63, F6 => 64, F7 => 65, F8 => 66, F9 => 67, F10 => 68,
        Numlock => 69, ScrollLock => 70,
        Numpad7 => 71, Numpad8 => 72, Numpad9 => 73, NumpadSubtract => 74, Numpad4 => 75, Numpad5 => 76, Numpad6 => 77,
        NumpadAdd => 78, Numpad1 => 79, Numpad2 => 80, Numpad3 => 81, Numpad0 => 82, NumpadDecimal => 83,
        F11 => 87, F12 => 88, NumpadEnter => 96, NumpadDivide => 98, PrintScreen => 99,
        Home => 102, ArrowUp => 103, PageUp => 104, ArrowLeft => 105, ArrowRight => 106, End => 107, ArrowDown => 108, PageDown => 109,
        Insert => 110, Delete => 111, Pause => 119, Logo => 125,
        _ => return None,
    })
}

fn button_code(device: &DigitDevice) -> u32 {
    let b = device.mouse_button().unwrap_or(MouseButton::PRIMARY);
    if b.contains(MouseButton::SECONDARY) { 0x111 }
    else if b.contains(MouseButton::MIDDLE) { 0x112 }
    else if b.contains(MouseButton::BACK) { 0x113 }
    else if b.contains(MouseButton::FORWARD) { 0x114 }
    else { 0x110 }
}

impl MpWaylandView {
    pub fn set_client(&mut self, client: ClientId) { self.client = Some(client); }

    /// Tells the client the tile size (and activation) when it changed.
    fn configure(&mut self, size: Vec2d) {
        let Some(client) = self.client else { return };
        let (w, h) = (size.x.round() as i32, size.y.round() as i32);
        if w < 1 || h < 1 || self.sent == (w, h, self.focused) { return; }
        self.sent = (w, h, self.focused);
        wayland_host::send(ViewCmd::Configure { client, w, h, activated: self.focused });
    }

    /// The surface under `p` (window coordinates) and the point in its own coordinates.
    fn pick(layers: &[Layer], p: Vec2d) -> Option<(Surf, Vec2d, bool)> {
        for l in layers.iter().rev() {
            let (x, y) = (p.x - l.pos.0 as f64, p.y - l.pos.1 as f64);
            if x >= 0.0 && y >= 0.0 && x < l.size.0 as f64 && y < l.size.1 as f64 {
                return Some((l.surf, dvec2(x, y), l.popup));
            }
        }
        None
    }

    fn pointer_to(&mut self, cx: &Cx, abs: Vec2d, enter: bool) -> Option<bool> {
        let client = self.client?;
        if !self.area.is_valid(cx) { return None; }
        let rect = self.area.rect(cx);
        let p = dvec2(abs.x - rect.pos.x, abs.y - rect.pos.y);
        let layers = wayland_host::layers(client);
        let (surf, local, popup) = Self::pick(&layers, p)?;
        if self.ptr_surf != Some(surf) || enter {
            self.ptr_surf = Some(surf);
            wayland_host::send(ViewCmd::PointerEnter { surf, x: local.x, y: local.y });
        } else {
            wayland_host::send(ViewCmd::PointerMotion { x: local.x, y: local.y });
        }
        Some(popup)
    }
}

impl TileHost for MpWaylandView {
    fn client(&self) -> Option<ClientId> { self.client }
    fn set_visible(&mut self, cx: &mut Cx, visible: bool) -> bool {
        if self.client.is_none() { return false; }
        self.hidden = !visible;
        if visible {
            // The callbacks held back are owed now.
            self.draw_bg.redraw(cx);
        }
        true
    }
    fn set_status_line(&mut self, _cx: &mut Cx, _line: &str) {}
    fn focus_keyboard(&mut self, cx: &mut Cx) -> bool {
        if !self.takes_key_focus { return true; }
        if !self.area.is_valid(cx) { return false; }
        cx.set_key_focus(self.area);
        self.focused = true;
        if let Some(main) = self.client.and_then(wayland_host::main_surface) {
            wayland_host::send(ViewCmd::KeyboardFocus(Some(main)));
        }
        let rect = self.area.rect(cx);
        self.configure(rect.size);
        true
    }
    fn release_keyboard(&mut self, cx: &mut Cx) {
        if self.focused {
            self.focused = false;
            wayland_host::send(ViewCmd::KeyboardFocus(None));
            if cx.has_key_focus(self.area) { cx.set_key_focus(Area::Empty); }
            if self.area.is_valid(cx) { let size = self.area.rect(cx).size; self.configure(size); }
        }
    }
    fn set_takes_key_focus(&mut self, on: bool) { self.takes_key_focus = on; }
    fn set_remote_cursor(&mut self, _cx: &mut Cx, _cursor: MouseCursor) {}
    fn has_frame(&self) -> bool { self.drawn }
    fn arrival_fade(&self) -> f32 { 1.0 }
    fn set_target_size(&mut self, size: Option<Vec2d>) { self.target = size; }
    fn set_close_crop(&mut self, _crop: Option<(Vec2d, Vec2d)>) {}
    fn set_fade(&mut self, fade: f32) { self.fade = fade; }
}

impl MpWaylandView {
    fn on_hit(&mut self, cx: &mut Cx, hit: Hit, from_popup: bool) {
        let Some(client) = self.client else { return };
        match hit {
            Hit::KeyFocus(_) if !from_popup => {
                self.focused = true;
                if let Some(main) = wayland_host::main_surface(client) { wayland_host::send(ViewCmd::KeyboardFocus(Some(main))); }
            }
            Hit::KeyFocusLost(_) if !from_popup => {
                self.focused = false;
                wayland_host::send(ViewCmd::KeyboardFocus(None));
            }
            Hit::FingerHoverIn(e) => {
                self.hovered = true;
                cx.set_cursor(cursor_of(wayland_host::cursor_shape(client)));
                self.pointer_to(cx, e.abs, true);
            }
            Hit::FingerHoverOver(e) => { self.pointer_to(cx, e.abs, false); }
            Hit::FingerMove(e) => { self.pointer_to(cx, e.abs, false); }
            Hit::FingerHoverOut(e) => {
                // Leaving the popups for the tile is the tile's hover; leaving for the desktop ends the pointer.
                let inside_tile = self.area.is_valid(cx) && self.area.rect(cx).contains(e.abs);
                if !from_popup || !inside_tile {
                    self.hovered = false;
                    self.ptr_surf = None;
                    wayland_host::send(ViewCmd::PointerLeave);
                    cx.set_cursor(MouseCursor::Default);
                }
            }
            Hit::FingerDown(e) => {
                let popup = self.pointer_to(cx, e.abs, false);
                if self.takes_key_focus { cx.set_key_focus(self.area); }
                cx.widget_action(self.uid, MpRunViewAction::Clicked { client });
                // A press outside every popup dismisses them, and is not delivered.
                if popup == Some(false) && wayland_host::layers(client).iter().any(|l| l.popup) {
                    wayland_host::send(ViewCmd::DismissPopups(client));
                    return;
                }
                wayland_host::send(ViewCmd::PointerButton { button: button_code(&e.device), pressed: true });
            }
            Hit::FingerUp(e) => {
                wayland_host::send(ViewCmd::PointerButton { button: button_code(&e.device), pressed: false });
            }
            Hit::FingerScroll(e) => {
                self.pointer_to(cx, e.abs, false);
                for (axis, v) in [(0u32, e.scroll.y), (1u32, e.scroll.x)] {
                    if v == 0.0 { continue; }
                    // Every scroll arrives as a mouse device: a wheel is the one with
                    // no gesture phase. Trackpad deltas (and their momentum) go as
                    // smooth continuous axis values, never as wheel notches.
                    if e.device.is_mouse() && e.phase == makepad_widgets::makepad_platform::event::ScrollPhase::None {
                        // A wheel notch arrives as 40 px on the direct (evdev) backend and 60 px on windowed ones.
                        let ticks = ((v / 50.0).round() as i32).clamp(-20, 20);
                        let ticks = if ticks == 0 { v.signum() as i32 } else { ticks };
                        wayland_host::send(ViewCmd::PointerAxis { axis, value: ticks as f64 * 10.0, v120: ticks * 120 });
                    } else {
                        wayland_host::send(ViewCmd::PointerAxis { axis, value: v, v120: 0 });
                    }
                }
            }
            Hit::KeyDown(e) if !e.is_repeat && !from_popup => {
                if let Some(evdev) = evdev_of(e.key_code) { wayland_host::send(ViewCmd::Key { evdev, pressed: true }); }
            }
            Hit::KeyUp(e) if !from_popup => {
                if let Some(evdev) = evdev_of(e.key_code) { wayland_host::send(ViewCmd::Key { evdev, pressed: false }); }
            }
            _ => {}
        }
    }

    fn draw_layer(&mut self, cx: &mut Cx2d, tile: Rect, l: &Layer) {
        let mut dest = Rect { pos: dvec2(tile.pos.x + l.pos.0 as f64, tile.pos.y + l.pos.1 as f64), size: dvec2(l.size.0 as f64, l.size.1 as f64) };
        if dest.size.x <= 0.0 || dest.size.y <= 0.0 { return; }
        // The part of the layer's own crop that is shown: all of it for a popup, the tile's share otherwise.
        let (mut fx, mut fy, mut fw, mut fh) = (0.0f32, 0.0f32, 1.0f32, 1.0f32);
        if !l.popup {
            let (x0, y0) = (dest.pos.x.max(tile.pos.x), dest.pos.y.max(tile.pos.y));
            let (x1, y1) = ((dest.pos.x + dest.size.x).min(tile.pos.x + tile.size.x), (dest.pos.y + dest.size.y).min(tile.pos.y + tile.size.y));
            if x1 <= x0 || y1 <= y0 { return; }
            fx = ((x0 - dest.pos.x) / dest.size.x) as f32;
            fy = ((y0 - dest.pos.y) / dest.size.y) as f32;
            fw = ((x1 - x0) / dest.size.x) as f32;
            fh = ((y1 - y0) / dest.size.y) as f32;
            dest = Rect { pos: dvec2(x0, y0), size: dvec2(x1 - x0, y1 - y0) };
        }
        self.draw_fg.draw_vars.set_texture(0, &l.texture);
        self.draw_fg.opaque = if l.opaque { 1.0 } else { 0.0 };
        self.draw_fg.uv_pos = vec2(l.uv.0 + fx * l.uv.2, l.uv.1 + fy * l.uv.3);
        self.draw_fg.uv_size = vec2(fw * l.uv.2, fh * l.uv.3);
        self.draw_fg.fade = self.fade;
        self.draw_fg.draw_abs(cx, dest);
    }
}

impl Widget for MpWaylandView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.client.is_none() { return; }
        let hit = event.hits(cx, self.area);
        self.on_hit(cx, hit, false);
        if self.popup_area.is_valid(cx) {
            let hit = event.hits(cx, self.popup_area);
            self.on_hit(cx, hit, true);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        self.draw_bg.draw_abs(cx, rect);
        let mut popups: Vec<Layer> = Vec::new();
        if let Some(client) = self.client {
            let dpi = cx.get_current_window_id().map(|w| cx.windows[w].window_geom.dpi_factor).filter(|d| d.is_finite() && *d > 0.0).unwrap_or_else(|| cx.current_dpi_factor());
            if (dpi - self.last_scale).abs() > 1e-6 {
                self.last_scale = dpi;
                wayland_host::send(ViewCmd::Scale(dpi));
            }
            // Popups may use the whole screen: tell the server where it is, in tile coordinates.
            let pass = cx.current_pass_size();
            let bounds = (-rect.pos.x as i32, -rect.pos.y as i32, pass.x as i32, pass.y as i32);
            if bounds != self.bounds_sent {
                self.bounds_sent = bounds;
                wayland_host::send(ViewCmd::PopupBounds { x: bounds.0, y: bounds.1, w: bounds.2, h: bounds.3 });
            }
            self.configure(self.target.unwrap_or(rect.size));
            let layers = wayland_host::layers(client);
            if self.hovered { cx.set_cursor(cursor_of(wayland_host::cursor_shape(client))); }
            let drawn = !layers.is_empty();
            if std::env::var_os("MAKEPAD_WM_TRACE_WAYLAND").is_some() {
                let line = format!(
                    "tile {:?} fade {:.2} dpi {:.2} target {:?} layers [{}]",
                    rect, self.fade, dpi, self.target,
                    layers.iter().map(|l| format!("{}{:?}+{:?}{}", if l.popup { "popup " } else { "" }, l.size, l.pos, if l.opaque { "" } else { " alpha" })).collect::<Vec<_>>().join(", ")
                );
                if line != self.traced {
                    log!("wayland: client {} {}", client, line);
                    self.traced = line;
                }
            }
            for l in layers {
                if l.popup { popups.push(l); } else { self.draw_layer(cx, rect, &l); }
            }
            if drawn {
                self.drawn = true;
                if !self.hidden {
                    wayland_host::frames_presented(client);
                }
            }
        }
        cx.end_turtle_with_area(&mut self.area);
        // Popups in an overlay list, above everything and free of the tile's clip. The list begins
        // every frame, even without popups, or the window drops it.
        if let Some(list) = self.overlay.as_mut() { list.begin_overlay_reuse(cx); }
        let pass = cx.current_pass_size();
        cx.begin_root_turtle(pass, Layout::default());
        if popups.is_empty() {
            self.popup_area = Area::Empty;
        } else {
            let mut union: Option<Rect> = None;
            for l in &popups {
                let r = Rect { pos: dvec2(rect.pos.x + l.pos.0 as f64, rect.pos.y + l.pos.1 as f64), size: dvec2(l.size.0 as f64, l.size.1 as f64) };
                union = Some(match union {
                    None => r,
                    Some(u) => {
                        let (x0, y0) = (u.pos.x.min(r.pos.x), u.pos.y.min(r.pos.y));
                        let (x1, y1) = ((u.pos.x + u.size.x).max(r.pos.x + r.size.x), (u.pos.y + u.size.y).max(r.pos.y + r.size.y));
                        Rect { pos: dvec2(x0, y0), size: dvec2(x1 - x0, y1 - y0) }
                    }
                });
            }
            cx.begin_turtle(Walk::abs_rect(union.unwrap()), Layout::flow_overlay());
            for l in &popups { self.draw_layer(cx, rect, l); }
            cx.end_turtle_with_area(&mut self.popup_area);
        }
        cx.end_pass_sized_turtle();
        if let Some(list) = self.overlay.as_mut() { list.end(cx); }
        DrawStep::done()
    }
}

thread_local! {
    static ICONS: std::cell::RefCell<std::collections::HashMap<String, Option<(Texture, DrawForeign)>>> = Default::default();
}

/// Draws the PNG icon of a listed foreign app (`wl.<id>`, with or without the `apps.` prefix) into
/// `rect`. False when the id is not a foreign app or has no icon, so the caller draws its own.
pub fn draw_foreign_icon(cx: &mut Cx2d, id: &str, rect: Rect, opacity: f32) -> bool {
    let id = id.strip_prefix("apps.").unwrap_or(id);
    if !id.starts_with("wl.") { return false; }
    ICONS.with(|icons| {
        let mut icons = icons.borrow_mut();
        if !icons.contains_key(id) {
            let loaded = wayland_host::icon_file(id).and_then(|f| std::fs::read(f).ok()).and_then(|bytes| {
                let image = ImageBuffer::from_png(&bytes).ok()?;
                let texture = image.into_new_texture(cx);
                let draw = cx.with_vm(|vm| DrawForeign::script_new_with_default(vm));
                Some((texture, draw))
            });
            icons.insert(id.to_string(), loaded);
        }
        let Some(Some((texture, draw))) = icons.get_mut(id) else { return false };
        draw.draw_vars.set_texture(0, texture);
        draw.opaque = 0.0;
        draw.uv_pos = vec2(0.0, 0.0);
        draw.uv_size = vec2(1.0, 1.0);
        draw.fade = opacity;
        draw.draw_abs(cx, rect);
        true
    })
}

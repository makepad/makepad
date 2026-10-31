//! The spaces' gestures on the `App`: slide, jump, make and remove, move a
//! window between spaces, fullscreen in and out, and the strip and cheat
//! sheet when they are up. The data is `spaces/mod.rs`, the motion is
//! `slide.rs` / `strip.rs` over `crate::motion`, the drawing is in `desk.rs`.
use super::strip::{self, Hit};
use crate::desk::WmState;
use crate::layout::LRect;
use crate::App;
use makepad_app_host::ClientId;
use makepad_widgets::*;
use makepad_wm_api::ScreenSpan;

/// The strip's geometry for the display it is on, now.
pub fn strip_layout(state: &WmState) -> Option<strip::Layout> {
    let s = &state.spaces.strip;
    if !s.visible() || s.screen >= state.screens.live_count() {
        return None;
    }
    let screen = &state.screens.screens[s.screen];
    let list = &screen.layout.spaces;
    let desktops = list.desktop_count();
    let closable: Vec<bool> = list.items().iter().map(|sp| !sp.is_desktop() || desktops > 1).collect();
    Some(strip::layout(screen.rect, list.len(), &closable, desktops < super::MAX_DESKTOPS, s.t()))
}

/// Pointer movement under this many px is a click, not a drag.
const CLICK_SLOP: f64 = 6.0;

impl App {
    fn space_screen(&mut self) -> usize {
        self.state_mut().screens.active
    }

    fn space_area(&mut self, cx: &mut Cx, screen: usize) -> (LRect, f64) {
        (self.screen_area(cx, screen), self.state_mut().gap)
    }

    /// Show the space at `idx` on the active display, sliding to it.
    pub(crate) fn spaces_goto(&mut self, cx: &mut Cx, idx: usize) {
        let screen = self.space_screen();
        let state = self.state_mut();
        let cur = state.layout_at(screen).spaces.cur();
        if idx == cur || idx >= state.layout_at(screen).spaces.len() {
            return;
        }
        if crate::desk::spaces_trace() {
            log!("wm: spaces goto screen {} {} -> {}", screen, cur, idx);
        }
        state.spaces.slides.begin(screen, cur, idx);
        state.layout_at_mut(screen).goto_space(idx);
        self.spaces_changed(cx);
    }

    /// The shown space of the active display changed: focus, bar, redraw.
    fn spaces_changed(&mut self, cx: &mut Cx) {
        self.state_mut().spaces.stale = true;
        self.focus_after_layout(cx);
        self.sync_bar_for_fullscreen(cx);
        self.update_bar(cx);
        self.redraw_all(cx);
    }

    /// SUPER+Left / Right: the neighbouring space.
    pub(crate) fn spaces_slide(&mut self, cx: &mut Cx, dir: i32) {
        let screen = self.space_screen();
        // Aim from where a running slide is heading, so repeated presses go on.
        let state = self.state_mut();
        let from = state.spaces.slides.get(screen).map_or(state.layout_at(screen).spaces.cur(), |s| s.to);
        let to = from as i64 + dir as i64;
        if to >= 0 && (to as usize) < state.layout_at(screen).spaces.len() {
            self.spaces_goto(cx, to as usize);
        }
    }

    /// SUPER+1..9: the n-th space in the list (0-based).
    pub(crate) fn spaces_jump(&mut self, cx: &mut Cx, n: usize) {
        self.spaces_goto(cx, n);
    }

    /// A new desktop at the end, shown.
    pub(crate) fn spaces_new(&mut self, cx: &mut Cx) {
        let screen = self.space_screen();
        if let Some(i) = self.state_mut().layout_at_mut(screen).new_desktop() {
            self.spaces_goto(cx, i);
        }
    }

    /// "x" on a thumbnail: a desktop goes (its windows to the previous
    /// desktop), a fullscreen space ends (its window back on its desktop).
    pub(crate) fn spaces_remove(&mut self, cx: &mut Cx, idx: usize) {
        let screen = self.space_screen();
        let (area, gap) = self.space_area(cx, screen);
        let layout = self.state_mut().layout_at_mut(screen);
        let Some(sp) = layout.spaces.get(idx) else { return };
        match sp.fullscreen_client() {
            Some(client) => {
                self.set_client_span(cx, client, None);
            }
            None => {
                self.state_mut().spaces.slides.cancel(screen);
                if self.state_mut().layout_at_mut(screen).remove_desktop(idx, area, gap) {
                    self.spaces_changed(cx);
                }
            }
        }
    }

    /// Move `client` to the desktop at `idx`. False when it cannot.
    pub(crate) fn spaces_move_window(&mut self, cx: &mut Cx, client: ClientId, idx: usize) -> bool {
        let Some(screen) = self.state_mut().screens.screen_of(client) else { return false };
        let (area, gap) = self.space_area(cx, screen);
        let moved = self.state_mut().layout_at_mut(screen).move_client_to_space(client, idx, area, gap);
        if moved {
            self.spaces_changed(cx);
        }
        moved
    }

    /// SUPER+SHIFT+n: move the focused window to space n; follow it or stay.
    pub(crate) fn spaces_move_focused(&mut self, cx: &mut Cx, n: usize, follow: bool) {
        let Some(client) = self.state_mut().layout().focused_client() else { return };
        if self.spaces_move_window(cx, client, n) && follow {
            self.spaces_goto(cx, n);
        }
    }

    /// SUPER+F / F11 / the green button: fullscreen in a space of its own, or
    /// back.
    pub(crate) fn fullscreen_toggle(&mut self, cx: &mut Cx, client: ClientId) {
        let on = !self.state_mut().layout_of(client).is_fullscreen_space(client);
        self.fullscreen_request(cx, client, on);
    }

    /// MAXIMISE (double click on the caption, SUPER+ALT+F): the window fills
    /// the current desktop, bar and dock staying; again restores it.
    pub(crate) fn maximise_toggle(&mut self, cx: &mut Cx, client: ClientId) {
        let on = !self.state_mut().layout_of(client).is_maximised(client);
        self.state_mut().layout_of_mut(client).set_maximised(client, on);
        self.focus_client(cx, client);
        self.redraw_all(cx);
    }

    /// A fullscreen request: the window's own, a menu's, a Wayland client's
    /// `xdg_toplevel.set_fullscreen`.
    pub(crate) fn fullscreen_request(&mut self, cx: &mut Cx, client: ClientId, on: bool) {
        self.set_client_span(cx, client, on.then_some(ScreenSpan::Current));
    }

    /// Windows on show before a fullscreen window grows over them: they stay
    /// under it until it has grown.
    pub(crate) fn spaces_note_underlay(&mut self, cx: &mut Cx, client: ClientId) {
        let Some(screen) = self.state_mut().screens.screen_of(client) else { return };
        let (area, gap) = self.space_area(cx, screen);
        let state = self.state_mut();
        let rects = state.layout_at(screen).rects(area, gap);
        state.spaces.underlay_at = crate::host::now();
        state.spaces.underlay = rects.into_iter().filter(|(c, _)| *c != client).map(|(c, r)| (screen, c, r)).collect();
    }

    pub(crate) fn spaces_strip_toggle(&mut self, cx: &mut Cx) {
        let screen = self.space_screen();
        let state = self.state_mut();
        state.spaces.strip.toggle(screen);
        state.spaces.stale = true;
        self.redraw_all(cx);
    }

    pub(crate) fn spaces_sheet_toggle(&mut self, cx: &mut Cx) {
        let state = self.state_mut();
        state.spaces.sheet = !state.spaces.sheet;
        self.redraw_all(cx);
    }

    /// A window dragged by the title or SUPER and dropped on a thumbnail
    /// moves to that space (and goes back where it was lifted from). True
    /// when it did.
    pub(crate) fn spaces_drop_window(&mut self, cx: &mut Cx, client: ClientId, abs: Vec2d, lifted_from: LRect) -> bool {
        let Some(layout) = self.state.as_ref().and_then(strip_layout) else { return false };
        let Some(Hit::Thumb(i)) = layout.hit((abs.x, abs.y)) else { return false };
        let screen = self.space_screen();
        let state = self.state_mut();
        if state.layout_at(screen).spaces.get(i).is_none_or(|s| !s.is_desktop()) {
            return false;
        }
        if state.layout_at(screen).spaces.cur() == i {
            return false;
        }
        if state.layout_at(screen).is_float(client) {
            state.layout_at_mut(screen).set_float_rect(client, lifted_from);
        }
        self.spaces_move_window(cx, client, i)
    }

    /// Pointer events while the strip is up. True when consumed.
    pub(crate) fn spaces_pointer(&mut self, cx: &mut Cx, event: &Event) -> bool {
        if self.state.is_none() {
            return false;
        }
        if self.state_mut().spaces.sheet {
            if matches!(event, Event::MouseDown(_)) {
                self.state_mut().spaces.sheet = false;
                self.redraw_all(cx);
            }
            return matches!(event, Event::MouseDown(_) | Event::MouseUp(_) | Event::Scroll(_));
        }
        // A window being dragged keeps its pointer; the drop looks at the strip.
        if self.drag.is_some() {
            return false;
        }
        let Some(layout) = self.state.as_ref().and_then(strip_layout) else { return false };
        if !self.state_mut().spaces.strip.open {
            return false;
        }
        let screen = self.state_mut().spaces.strip.screen;
        match event {
            Event::MouseMove(e) => {
                let p = (e.abs.x, e.abs.y);
                let strip = &mut self.state_mut().spaces.strip;
                strip.hover = layout.hit(p);
                let over = strip.hover.is_some();
                let dragging = strip.drag.is_some();
                if let Some(d) = strip.drag.as_mut() {
                    d.at = p;
                    if (p.0 - d.start.0).abs() > CLICK_SLOP || (p.1 - d.start.1).abs() > CLICK_SLOP {
                        d.moved = true;
                    }
                }
                self.redraw_all(cx);
                dragging || over
            }
            Event::MouseDown(e) => {
                let p = (e.abs.x, e.abs.y);
                match layout.hit(p) {
                    Some(Hit::Thumb(i)) => {
                        self.state_mut().spaces.strip.drag = Some(strip::Drag { index: i, start: p, moved: false, at: p });
                        true
                    }
                    Some(Hit::Close(i)) => {
                        self.spaces_remove(cx, i);
                        true
                    }
                    Some(Hit::Plus) => {
                        self.spaces_new(cx);
                        true
                    }
                    Some(Hit::Band) => true,
                    None => false,
                }
            }
            Event::MouseUp(e) => {
                let Some(d) = self.state_mut().spaces.strip.drag.take() else {
                    return layout.hit((e.abs.x, e.abs.y)).is_some();
                };
                if d.moved {
                    let to = layout.drop_index(d.index, e.abs.x);
                    let state = self.state_mut();
                    if state.layout_at_mut(screen).spaces.move_space(d.index, to) {
                        state.spaces.stale = true;
                    }
                } else {
                    self.spaces_goto(cx, d.index);
                    self.state_mut().spaces.strip.hide();
                }
                self.redraw_all(cx);
                true
            }
            Event::Scroll(e) => layout.hit((e.abs.x, e.abs.y)).is_some(),
            _ => false,
        }
    }

    /// Esc closes the strip and the sheet. True when consumed.
    pub(crate) fn spaces_key(&mut self, cx: &mut Cx, e: &KeyEvent) -> bool {
        if self.state.is_none() || e.key_code != KeyCode::Escape || e.modifiers.logo {
            return false;
        }
        let ui = &mut self.state_mut().spaces;
        if ui.sheet {
            ui.sheet = false;
        } else if ui.strip.open {
            ui.strip.hide();
        } else {
            return false;
        }
        self.redraw_all(cx);
        true
    }
}

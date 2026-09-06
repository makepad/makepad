//! The AI chat slot: F10 in every standalone `Window`.
//!
//! The way every app grew the Shift+F10 designer, every app gets the assistant:
//! `Window` declares `ai_chat := AiChatSlot{}` beside the tweaker,
//! hardcoded, zero cost while off. The slot owns nothing of the chat — it
//! is a place. On the first F10 it instantiates the chat's module root BY
//! NAME, `mod.widgets.AiChatOverlay{}`, which exists when the app links
//! the `makepad-aichat` crate and calls its `script_mod` (the same two
//! lines every app already does for widgets). An app that did not link it
//! gets one log line and nothing on screen. Hosted by the window manager
//! the slot is inert: the WM's own pane is the chat then, and it takes
//! F10 before any tile sees it.
//!
//! Open, the overlay slides in on the LEFT and PUSHES the body in
//! (decision 17): the slot reserves a strip — the body's left inset,
//! animated with the slide (the WM pane's motion: easeOutQuint, 260 ms in,
//! 200 ms out, 440 px clamped to 40 % of the window) — and the body is
//! laid out beside it every frame, so nothing of the app is ever under
//! the chat. The card fills the strip, a 2 px edge on its right, its
//! ground the app's own `theme.color_bg_app` read when it opens (after
//! any retint). It owns the pointer inside the strip and the keyboard
//! through its composer. Requests from outside the widget tree — the
//! bridge's `/ai` routes, the overlay's own Escape — come through
//! [`AiSlotRequests`], a `Cx` global the slot drains on its next event,
//! so nothing here is a static.

use crate::{
    keyboard_view::KeyboardView,
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_script::script_eval,
    makepad_script::trap::NoTrap,
    text_input::TextInputWidgetRefExt,
    view::View,
    widget::*,
    widget_tree::CxWidgetExt,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.AiChatSlotBase = #(AiChatSlot::register_widget(vm))
    mod.widgets.AiChatSlot = set_type_default() do mod.widgets.AiChatSlotBase{
        width: 0
        height: 0
        // An opaque ground: the container tint is translucent by design and
        // the panel draws its own bands over this.
        draw_card +: { color: theme.color_bg_app }
        draw_edge +: { color: theme.color_text_hl }
    }
}

/// Seconds the slide-in takes.
const SLIDE_IN: f64 = 0.26;
/// Seconds the slide-out takes.
const SLIDE_OUT: f64 = 0.20;
/// The overlay's width, before the window-fraction clamp.
const PANE_WIDTH: f64 = 440.0;
/// The most of the window the overlay may take.
const PANE_MAX_FRACTION: f64 = 0.4;
/// The narrowest the overlay ever gets.
const PANE_MIN_WIDTH: f64 = 240.0;
/// The edge on the overlay's right.
const EDGE: f64 = 2.0;

/// A cubic bezier from (0,0) to (1,1) evaluated as y(x), like a CSS
/// timing function: x is solved for the curve parameter first.
fn bezier(p1x: f64, p1y: f64, p2x: f64, p2y: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let curve = |a: f64, b: f64, u: f64| {
        let v = 1.0 - u;
        3.0 * v * v * u * a + 3.0 * v * u * u * b + u * u * u
    };
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..24 {
        let mid = 0.5 * (lo + hi);
        if curve(p1x, p2x, mid) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    curve(p1y, p2y, 0.5 * (lo + hi))
}

/// easeOutQuint as a bezier (0.23, 1, 0.32, 1) — the WM pane's motion.
pub fn ease_out_quint(t: f64) -> f64 {
    bezier(0.23, 1.0, 0.32, 1.0, t)
}

/// What the world asks of the slot, outside the widget tree: the bridge's
/// `/ai?on=` and `/ai?say=`, the overlay's Escape. A `Cx` global; the
/// slot takes `open`, the overlay takes `say`.
#[derive(Default)]
pub struct AiSlotRequests {
    /// `Some(true)` open, `Some(false)` close; taken by the slot.
    pub open: Option<bool>,
    /// Lines to send as if typed, in order; taken by the overlay.
    pub say: Vec<String>,
    /// The slot's state as it last reported it, for whoever asks.
    pub is_open: bool,
}

/// The slide, as plain state: open or not, how far in (0 hidden, 1
/// shown), and the animation in flight. No `Cx` so it can be tested.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct SlideState {
    open: bool,
    t: f64,
    /// (started at, from t, opening)
    anim: Option<(f64, f64, bool)>,
}

impl SlideState {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// 0 = fully hidden, 1 = fully shown.
    pub fn t(&self) -> f64 {
        self.t
    }

    /// On screen at all: open, or still sliding out.
    pub fn showing(&self) -> bool {
        self.open || self.t > 0.0
    }

    /// Start opening or closing at `now`; nothing when already so.
    pub fn set_open(&mut self, now: f64, open: bool) -> bool {
        if self.open == open {
            return false;
        }
        self.open = open;
        self.anim = Some((now, self.t, open));
        true
    }

    /// Advance to `now`; true while the slide still moves.
    pub fn animate(&mut self, now: f64) -> bool {
        let Some((start, from, opening)) = self.anim else { return false };
        let dur = if opening { SLIDE_IN } else { SLIDE_OUT };
        let to = if opening { 1.0 } else { 0.0 };
        let p = ((now - start) / dur).clamp(0.0, 1.0);
        self.t = from + (to - from) * p;
        if p >= 1.0 {
            self.t = to;
            self.anim = None;
            return false;
        }
        true
    }

    /// The overlay's resting rect beside `body` (the FULL body, before
    /// the strip is taken from it): left side, full height.
    pub fn rest_rect(body: Rect) -> Rect {
        let w = PANE_WIDTH.min(body.size.x * PANE_MAX_FRACTION).max(PANE_MIN_WIDTH).min(body.size.x.max(1.0));
        Rect { pos: body.pos, size: dvec2(w, body.size.y) }
    }

    /// The strip the body gives up right now: the card's width, eased —
    /// zero hidden, the full width at rest.
    pub fn strip(&self, body: Rect) -> f64 {
        (ease_out_quint(self.t) * Self::rest_rect(body).size.x).max(0.0)
    }

    /// The rect as drawn right now: right-aligned in the strip, so it
    /// slides in from off the body's left edge.
    pub fn slid_rect(&self, body: Rect) -> Rect {
        let rest = Self::rest_rect(body);
        Rect { pos: dvec2(rest.pos.x + self.strip(body) - rest.size.x, rest.pos.y), size: rest.size }
    }

    /// The body as it is laid out beside a strip that wide.
    pub fn body_beside(body: Rect, strip: f64) -> Rect {
        Rect { pos: dvec2(body.pos.x + strip, body.pos.y), size: dvec2((body.size.x - strip).max(0.0), body.size.y) }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct AiChatSlot {
    #[source]
    source: ScriptObjectRef,
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_card: DrawColor,
    #[live]
    draw_edge: DrawColor,
    /// The chat's module root, made by name on the first open.
    #[rust]
    overlay: Option<WidgetRef>,
    /// The app does not link the chat: said once, then quiet.
    #[rust]
    missing_logged: bool,
    #[rust]
    slide: SlideState,
    /// The window's FULL body rect (before the strip is taken from it), as
    /// the intercept last saw it: what the overlay slides in beside.
    #[rust]
    body_rect: Rect,
    /// The left inset the body currently carries for the strip.
    #[rust]
    applied_inset: f64,
    /// The app's ground colour, read from its theme when the slot opens.
    #[rust]
    ground: Option<Vec4f>,
    #[rust]
    next_frame: NextFrame,
    /// The composer was asked for the keyboard before the overlay had
    /// drawn; claimed right after its next draw.
    #[rust]
    focus_pending: bool,
    /// Where the overlay draws: a list in the window's overlay, composited
    /// over the body (see `draw_walk`). Made on the first open; RETAINED
    /// by the window between frames, so it is drawn empty while hidden.
    #[rust]
    overlay_list: Option<DrawList2d>,
}

impl AiChatSlot {
    pub fn is_open(&self) -> bool {
        self.slide.is_open()
    }

    fn showing(&self) -> bool {
        self.slide.showing()
    }

    /// Inside the overlay as drawn right now, while it is open.
    pub fn contains(&self, p: DVec2) -> bool {
        self.slide.is_open() && self.slide.slid_rect(self.body_rect).contains(p)
    }

    /// The body as it was drawn — beside whatever strip it carried then;
    /// the full rect is that plus the strip.
    pub fn set_body_rect(&mut self, drawn: Rect) {
        if drawn.size.x + self.applied_inset > 0.0 && drawn.size.y > 0.0 {
            self.body_rect = Rect {
                pos: dvec2(drawn.pos.x - self.applied_inset, drawn.pos.y),
                size: dvec2(drawn.size.x + self.applied_inset, drawn.size.y),
            };
        }
    }

    /// Hand the body its left inset for the strip as it is right now. The
    /// parent reads the body's walk on every draw, so a changed inset is a
    /// relayout of the whole window.
    fn apply_inset(&mut self, cx: &mut Cx, window_view: &View) {
        let strip = if self.showing() { self.slide.strip(self.body_rect) } else { 0.0 };
        if (strip - self.applied_inset).abs() < 0.01 {
            return;
        }
        if let Some((_, body)) = window_view.children.iter().find(|(id, _)| *id == live_id!(body)) {
            if let Some(mut body) = body.borrow_mut::<KeyboardView>() {
                body.set_left_inset(strip);
            }
        }
        self.applied_inset = strip;
        cx.redraw_all();
    }

    /// The app's ground, after its retint: `theme.color_bg_app` as the
    /// running VM has it now — not as the slot's DSL captured it.
    fn theme_ground(cx: &mut Cx) -> Option<Vec4f> {
        cx.with_vm(|vm| {
            let theme = vm.module(id!(theme));
            let value = vm.bx.heap.value(theme, id!(color_bg_app).into(), NoTrap);
            value.as_color().map(|c| Vec4f {
                x: ((c >> 24) & 0xff) as f32 / 255.0,
                y: ((c >> 16) & 0xff) as f32 / 255.0,
                z: ((c >> 8) & 0xff) as f32 / 255.0,
                w: (c & 0xff) as f32 / 255.0,
            })
        })
    }

    /// The slide's clock: on the slot's next-frame event, advance and ask
    /// for another while it moves. Called from the window intercept (which
    /// sees every event first) and from the slot's own dispatch; the same
    /// event advances the same way twice, so both may run.
    fn advance(&mut self, cx: &mut Cx, event: &Event) {
        if self.next_frame.is_event(event).is_some() {
            let now = cx.seconds_since_app_start();
            if self.slide.animate(now) {
                self.next_frame = cx.new_next_frame();
            }
            cx.redraw_all();
        }
    }

    /// Make the chat's root, by name. `mod.widgets.AiChatOverlay` exists
    /// when the app linked `makepad-aichat`; a miss is one log line.
    fn ensure_overlay(&mut self, cx: &mut Cx) -> bool {
        if self.overlay.is_some() {
            return true;
        }
        let overlay = cx.with_vm(|vm| {
            let widgets = vm.module(id!(widgets));
            let ty = vm.bx.heap.value_path(widgets, ids!(AiChatOverlay), NoTrap);
            if ty.as_object().is_none() {
                return None;
            }
            let value = script_eval!(vm, {
                use mod.widgets.*
                AiChatOverlay {}
            });
            Some(WidgetRef::script_from_value(vm, value))
        });
        match overlay {
            Some(overlay) => {
                // Part of the tree under this slot: /snap and /d see it.
                cx.widget_tree_insert_child(self.uid, live_id!(overlay), overlay.clone());
                self.overlay = Some(overlay);
                true
            }
            None => {
                if !self.missing_logged {
                    self.missing_logged = true;
                    log!("F10: this app does not link makepad-aichat (no mod.widgets.AiChatOverlay); nothing to show");
                }
                false
            }
        }
    }

    /// The open/close request the world left on `Cx`, if any. Taken here
    /// from the window intercept (every event the window sees) and from
    /// the slot's own dispatch, whichever comes first, so a bridge request
    /// lands on the very next event rather than waiting for the pointer.
    fn take_requests(&mut self, cx: &mut Cx) {
        if let Some(open) = cx.global::<AiSlotRequests>().open.take() {
            self.set_open(cx, open);
        }
    }

    fn set_open(&mut self, cx: &mut Cx, open: bool) {
        if open && !self.ensure_overlay(cx) {
            return;
        }
        let now = cx.seconds_since_app_start();
        if !self.slide.set_open(now, open) {
            return;
        }
        cx.global::<AiSlotRequests>().is_open = open;
        self.next_frame = cx.new_next_frame();
        if open {
            // The composer takes the keyboard once the overlay has drawn;
            // the ground is the app's own, as its theme stands now.
            self.focus_pending = true;
            self.ground = Self::theme_ground(cx);
        } else {
            self.focus_pending = false;
            // A hidden chat must not keep the keyboard.
            cx.set_key_focus(Area::Empty);
        }
        // The slot's own area is nothing while closed: the window redraws.
        cx.redraw_all();
    }

    fn claim_keyboard(&mut self, cx: &mut Cx) {
        if let Some(overlay) = &self.overlay {
            overlay.text_input(cx, ids!(panel.input)).set_key_focus(cx);
        }
        self.focus_pending = false;
    }

    fn is_pointer_event(event: &Event) -> Option<DVec2> {
        match event {
            Event::MouseMove(e) => Some(e.abs),
            Event::MouseDown(e) => Some(e.abs),
            Event::MouseUp(e) => Some(e.abs),
            Event::Scroll(e) => Some(e.abs),
            _ => None,
        }
    }
}

/// No modifier at all: the bare F10 the slot owns.
fn bare_key(m: &KeyModifiers) -> bool {
    !m.shift && !m.control && !m.alt && !m.logo
}

/// Called by `Window::handle_event` beside the tweaker's intercept, in
/// place of ordinary dispatch. `true` when the event was the slot's alone:
/// the bare F10 (toggled), or a pointer event inside the open overlay.
/// Hosted by the window manager nothing is intercepted — the WM's pane is
/// the chat, and it takes F10 before any tile sees it anyway.
pub fn window_intercept(
    cx: &mut Cx,
    event: &Event,
    window_view: &mut View,
    window_id: WindowId,
) -> bool {
    if cx.in_makepad_studio() {
        return false;
    }
    if let Event::KeyDown(key_event) = event {
        if key_event.key_code == KeyCode::F10 && bare_key(&key_event.modifiers) {
            // Several windows see the same key: one toggle per event.
            let event_id = cx.event_id();
            let guard = cx.global::<AiSlotToggleGuard>();
            if guard.event_id != Some(event_id) {
                guard.event_id = Some(event_id);
                let open = !cx.global::<AiSlotRequests>().is_open;
                cx.global::<AiSlotRequests>().open = Some(open);
                // The request lands on the slot's next event; make one.
                cx.new_next_frame();
                cx.redraw_all();
            }
            return true;
        }
        return false;
    }
    let slot = window_view
        .children
        .iter()
        .find(|(id, _)| *id == live_id!(ai_chat))
        .map(|(_, widget)| widget.clone());
    let Some(slot) = slot else {
        return false;
    };
    let (showing, inside) = {
        let Some(mut s) = slot.borrow_mut::<AiChatSlot>() else {
            return false;
        };
        s.take_requests(cx);
        s.advance(cx, event);
        // The body rect, live, for the strip, the draw and the hit test —
        // then the strip goes onto the body as its left inset, every
        // frame of the slide and once more when it has closed.
        if let Some((_, body)) = window_view.children.iter().find(|(id, _)| *id == live_id!(body)) {
            s.set_body_rect(body.area().clipped_rect(cx));
        }
        s.apply_inset(cx, window_view);
        if !s.showing() {
            return false;
        }
        let inside = match AiChatSlot::is_pointer_event(event) {
            Some(abs) => {
                let for_this_window = match event {
                    Event::MouseMove(e) => e.window_id == window_id,
                    Event::MouseDown(e) => e.window_id == window_id,
                    Event::MouseUp(e) => e.window_id == window_id,
                    Event::Scroll(e) => e.window_id == window_id,
                    _ => false,
                };
                for_this_window && s.contains(abs)
            }
            None => false,
        };
        (s.showing(), inside)
    };
    if showing && inside {
        // The overlay's alone: the tile of the app beneath never sees it.
        slot.handle_event(cx, event, &mut Scope::empty());
        return true;
    }
    false
}

/// One toggle per key event, however many windows saw it.
#[derive(Default)]
struct AiSlotToggleGuard {
    event_id: Option<u64>,
}

impl Widget for AiChatSlot {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // Requests from outside the tree land here (or in the intercept).
        self.take_requests(cx);
        self.advance(cx, event);
        // Once it exists the overlay keeps ticking whether or not the slot
        // shows it — a turn in flight finishes behind a closed slot, the way
        // the WM keeps its hidden pane's child running. Pointer events reach
        // it only inside its open rect; the rest belongs to the body beneath.
        let Some(overlay) = self.overlay.clone() else {
            return;
        };
        let pass = match Self::is_pointer_event(event) {
            Some(abs) => self.contains(abs),
            None => true,
        };
        if pass {
            overlay.handle_event(cx, event, scope);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        // The body is laid out BESIDE the strip (its left inset), so the
        // card never covers app content; it is still drawn in the window's
        // retained overlay list — the tweaker's and every popup's place —
        // because the body is a deferred Fill child of a Down flow, drawn
        // after every other child, and ink in the ordinary list could sit
        // under its clip. The list is begun and ended every draw once made,
        // empty while hidden, or the last picture would stay on screen.
        // Nothing exists until the first open: closed and never opened
        // costs nothing.
        if self.overlay_list.is_none() {
            if !self.showing() {
                return DrawStep::done();
            }
            self.overlay_list = Some(DrawList2d::new(cx));
        }
        let mut list = self.overlay_list.take().unwrap();
        list.begin_overlay_reuse(cx);
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());
        if self.showing() {
            if let Some(overlay) = self.overlay.clone() {
                // The card, right-aligned in the strip the body gave up:
                // beside the body, never over it.
                let r = self.slide.slid_rect(self.body_rect);
                if let Some(ground) = self.ground {
                    self.draw_card.color = ground;
                }
                self.draw_card.draw_abs(cx, r);
                self.draw_edge.draw_abs(cx, Rect { pos: dvec2(r.pos.x + r.size.x - EDGE, r.pos.y), size: dvec2(EDGE, r.size.y) });
                let inner = Rect { pos: r.pos, size: dvec2(r.size.x - EDGE, r.size.y) };
                overlay.draw_walk_all(cx, scope, Walk::abs_rect(inner));
                if self.focus_pending && self.slide.is_open() {
                    // The overlay just drew: its composer has an area to focus.
                    self.claim_keyboard(cx);
                }
            }
        }
        cx.end_pass_sized_turtle();
        list.end(cx);
        self.overlay_list = Some(list);
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slide_opens_closes_and_idles_between() {
        let mut s = SlideState::default();
        assert!(!s.showing() && !s.is_open());
        assert!(s.set_open(10.0, true));
        assert!(!s.set_open(10.0, true), "opening twice is nothing");
        assert!(s.is_open() && s.showing());
        assert!(s.animate(10.0 + SLIDE_IN * 0.5), "still moving half way in");
        assert!(s.t() > 0.0 && s.t() < 1.0);
        assert!(!s.animate(10.0 + SLIDE_IN + 0.01), "landed");
        assert_eq!(s.t(), 1.0);
        // Closing from fully open: still showing while it slides out.
        assert!(s.set_open(20.0, false));
        assert!(!s.is_open() && s.showing());
        assert!(!s.animate(20.0 + SLIDE_OUT + 0.01));
        assert_eq!(s.t(), 0.0);
        assert!(!s.showing(), "gone: zero cost from here");
        // Idle: no animation to advance.
        assert!(!s.animate(30.0));
    }

    #[test]
    fn the_overlay_rests_on_the_left_and_slides_in_from_off_screen() {
        let body = Rect { pos: dvec2(0.0, 28.0), size: dvec2(1200.0, 800.0) };
        let rest = SlideState::rest_rect(body);
        assert_eq!(rest.size, dvec2(440.0, 800.0));
        assert_eq!(rest.pos, dvec2(0.0, 28.0));
        // A narrow window: 40 % of it, never under the minimum.
        let narrow = SlideState::rest_rect(Rect { pos: dvec2(0.0, 0.0), size: dvec2(800.0, 600.0) });
        assert_eq!(narrow.size.x, 320.0);
        let tiny = SlideState::rest_rect(Rect { pos: dvec2(0.0, 0.0), size: dvec2(300.0, 600.0) });
        assert_eq!(tiny.size.x, 240.0);
        // Hidden: fully off the left edge, no strip. Shown: at rest, the
        // strip is the card.
        let mut s = SlideState::default();
        assert_eq!(s.slid_rect(body).pos.x, -440.0);
        assert_eq!(s.strip(body), 0.0);
        s.set_open(0.0, true);
        s.animate(SLIDE_IN + 0.01);
        assert_eq!(s.slid_rect(body), rest);
        assert_eq!(s.strip(body), 440.0);
    }

    #[test]
    fn the_body_is_laid_out_beside_the_strip() {
        let body = Rect { pos: dvec2(0.0, 28.0), size: dvec2(1200.0, 800.0) };
        let beside = SlideState::body_beside(body, 440.0);
        assert_eq!(beside.pos, dvec2(440.0, 28.0));
        assert_eq!(beside.size, dvec2(760.0, 800.0));
        // Mid-slide the strip is between nothing and the card, and the
        // card's right edge is exactly where the body begins.
        let mut s = SlideState::default();
        s.set_open(0.0, true);
        s.animate(SLIDE_IN * 0.5);
        let strip = s.strip(body);
        assert!(strip > 0.0 && strip < 440.0, "{strip}");
        let card = s.slid_rect(body);
        assert_eq!(card.pos.x + card.size.x, SlideState::body_beside(body, strip).pos.x);
    }

    #[test]
    fn requests_are_taken_once() {
        let mut req = AiSlotRequests::default();
        req.open = Some(true);
        assert_eq!(req.open.take(), Some(true));
        assert_eq!(req.open.take(), None);
        req.say.push("hello".into());
        assert_eq!(std::mem::take(&mut req.say), vec!["hello".to_string()]);
        assert!(req.say.is_empty());
    }

    #[test]
    fn bare_keys_have_no_modifier() {
        let none = KeyModifiers { shift: false, control: false, alt: false, logo: false };
        assert!(bare_key(&none));
        assert!(!bare_key(&KeyModifiers { shift: true, ..none }));
        assert!(!bare_key(&KeyModifiers { control: true, ..none }));
    }
}

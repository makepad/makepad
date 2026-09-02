//! The AI pane: a right-side slide-in that hosts the aichat child.
//!
//! The chat is NOT the window manager's — it is a child app like any
//! other (`apps/aichat`), launched on first use and seated here instead of
//! in a tile. This widget owns nothing but the slot: the pane rect under
//! the bar, the slide (easeOutQuint, 260 ms in, 200 ms out), an opaque
//! card with a 2 px edge on its left, and one `MpRunView` that presents
//! the child's frames and forwards its input. Open, the pane takes the
//! pointer inside its rect and the keyboard (the run view's own key
//! focus); closed, it draws nothing — but the run view keeps ticking the
//! child, so a turn in flight finishes behind a hidden pane and the next
//! slide-in shows a live picture, not a stale one.

use crate::desk::ease_out_quint;
use crate::hub::ClientId;
use crate::run_view::MpRunView;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ShellAiPaneBase = #(ShellAiPane::register_widget(vm))

    mod.widgets.ShellAiPane = set_type_default() do mod.widgets.ShellAiPaneBase {
        width: Fill
        height: Fill
        draw_card +: { color: mod.wm_theme.background }
        draw_edge +: { color: mod.wm_theme.accent }
        run := MpRunView{}
    }
}

/// Seconds the slide-in takes.
const SLIDE_IN: f64 = 0.26;
/// Seconds the slide-out takes.
const SLIDE_OUT: f64 = 0.20;
/// The pane's width, before the screen-fraction clamp.
const PANE_WIDTH: f64 = 440.0;
/// The most of the desk the pane may take.
const PANE_MAX_FRACTION: f64 = 0.4;
/// The edge on the pane's left.
const EDGE: f64 = 2.0;

#[derive(Script, ScriptHook, Widget)]
pub struct ShellAiPane {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[live]
    draw_card: DrawColor,
    #[live]
    draw_edge: DrawColor,
    #[rust]
    open: bool,
    /// 0 = fully hidden, 1 = fully shown.
    #[rust]
    t: f64,
    #[rust]
    anim_started: Option<(f64, f64, bool)>,
    #[rust]
    client: Option<ClientId>,
    /// The desk's rect (under the bar) and the outer gap, as the WM
    /// last told us; the pane rect derives from them.
    #[rust]
    desk_rect: Rect,
    #[rust]
    gap: f64,
    #[rust]
    next_frame: NextFrame,
    /// The keyboard was asked for while the run view had no live area;
    /// claimed right after its next draw.
    #[rust]
    focus_pending: bool,
}

impl ShellAiPane {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn client(&self) -> Option<ClientId> {
        self.client
    }

    pub fn set_client(&mut self, client: Option<ClientId>) {
        self.client = client;
    }

    /// Where the desk is and how far from its edges the pane sits.
    pub fn set_geometry(&mut self, desk_rect: Rect, gap: f64) {
        self.desk_rect = desk_rect;
        self.gap = gap;
    }

    /// The pane's resting rect: right side, under the bar, `gap` in.
    pub fn pane_rect(&self) -> Rect {
        let w = PANE_WIDTH.min(self.desk_rect.size.x * PANE_MAX_FRACTION).max(240.0);
        let x = self.desk_rect.pos.x + self.desk_rect.size.x - self.gap - w;
        let y = self.desk_rect.pos.y + self.gap;
        let h = (self.desk_rect.size.y - self.gap * 2.0).max(100.0);
        Rect { pos: dvec2(x, y), size: dvec2(w, h) }
    }

    /// The rect as drawn right now, slid by `t`.
    fn slid_rect(&self) -> Rect {
        let rest = self.pane_rect();
        let off = (1.0 - ease_out_quint(self.t)) * (rest.size.x + self.gap);
        Rect { pos: dvec2(rest.pos.x + off, rest.pos.y), size: rest.size }
    }

    /// True while the pane is on screen at all (open, or sliding out).
    fn showing(&self) -> bool {
        self.open || self.t > 0.0
    }

    pub fn contains(&self, p: DVec2) -> bool {
        self.open && self.slid_rect().contains(p)
    }

    pub fn set_open(&mut self, cx: &mut Cx, open: bool) {
        if self.open == open {
            return;
        }
        self.open = open;
        self.anim_started = Some((cx.seconds_since_app_start(), self.t, open));
        self.next_frame = cx.new_next_frame();
        if !open {
            // A hidden pane must not keep the keyboard: typing would go on
            // reaching the chat behind nothing.
            self.focus_pending = false;
            self.with_run_view(cx, |cx, v| v.release_keyboard(cx));
        }
        self.redraw(cx);
    }

    /// The child is gone: no client, no picture, pane shut.
    pub fn reset(&mut self, cx: &mut Cx) {
        self.client = None;
        self.with_run_view(cx, |cx, v| v.clear_run_target(cx));
        self.set_open(cx, false);
    }

    /// Advance the slide; true while it still moves.
    fn animate(&mut self, now: f64) -> bool {
        let Some((start, from, opening)) = self.anim_started else { return false };
        let dur = if opening { SLIDE_IN } else { SLIDE_OUT };
        let to = if opening { 1.0 } else { 0.0 };
        let p = ((now - start) / dur).clamp(0.0, 1.0);
        self.t = from + (to - from) * p;
        if p >= 1.0 {
            self.t = to;
            self.anim_started = None;
            return false;
        }
        true
    }

    pub fn with_run_view<R>(&mut self, cx: &mut Cx, f: impl FnOnce(&mut Cx, &mut MpRunView) -> R) -> Option<R> {
        let run = self.view.widget(cx, ids!(run));
        let mut view = run.borrow_mut::<MpRunView>()?;
        Some(f(cx, &mut view))
    }

    /// Claim the keyboard for the child. The run view is not drawn at all
    /// while the pane is hidden, so at open time its area is stale (or,
    /// the first time, empty): the claim is then kept pending and made
    /// right after the pane's next draw, when the area is live again.
    pub fn focus_keyboard(&mut self, cx: &mut Cx) {
        let ok = self.with_run_view(cx, |cx, v| v.focus_keyboard(cx)).unwrap_or(false);
        self.focus_pending = !ok;
        if !ok {
            self.redraw(cx);
        }
    }
}

impl Widget for ShellAiPane {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            let now = cx.seconds_since_app_start();
            if self.animate(now) {
                self.next_frame = cx.new_next_frame();
            }
            self.redraw(cx);
        }
        // The child is ticked whether or not the pane shows (its 8 ms
        // tick timer is what makes it run at all); pointer and keys reach
        // the run view only while the pane is on screen, so a hidden
        // pane's stale area can never swallow a click meant for a tile.
        if self.showing() || matches!(event, Event::Timer(_)) {
            self.view.handle_event(cx, event, scope);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        if !self.showing() {
            return DrawStep::done();
        }
        let r = self.slid_rect();
        self.draw_card.draw_abs(cx, r);
        self.draw_edge.draw_abs(cx, Rect { pos: r.pos, size: dvec2(EDGE, r.size.y) });
        let inner = Rect { pos: dvec2(r.pos.x + EDGE, r.pos.y), size: dvec2(r.size.x - EDGE, r.size.y) };
        while self.view.draw_walk(cx, scope, Walk::abs_rect(inner)).is_step() {}
        if self.focus_pending && self.open {
            // The run view just drew: its area is live, the claim can land.
            if self.with_run_view(cx, |cx, v| v.focus_keyboard(cx)).unwrap_or(false) {
                self.focus_pending = false;
            }
        }
        DrawStep::done()
    }
}

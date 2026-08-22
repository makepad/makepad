//! The system HOVER-TOOLTIP facility: `Tip` + `TipLayer`.
//!
//! Declaring a tooltip is one wrapper in the DSL — no bespoke code at the
//! call site:
//!
//! ```text
//! Tip{ text: "MIDI learn"
//!     learn_btn := IconButton{ ... }
//! }
//! ```
//!
//! `Tip` is a transparent wrapper (the `Learn{}` idiom): it walks its child
//! unchanged and only REPORTS hover with its text and the child's FINAL
//! screen rect. One `TipLayer` per window — the last child of the window's
//! overlay stack — owns the whole state machine: the ~0.5 s reveal delay
//! (so nothing flickers mid-performance), the instant-follow grace when
//! sliding between adjacent tipped controls, positioning that never covers
//! the hovered control (below it, above when there is no room), clamping
//! to the window, and the themed chrome. The layer draws on its own
//! overlay draw list, so a tip floats over every panel and splitter
//! regardless of where the control sits.
//!
//! Positioning uses the hovered widget's final rect carried in the report
//! — never mid-pass turtle state — per the overlay-layer law (deferred
//! turtle alignment drifts; final rects do not).

use crate::{makepad_derive_widget::*, makepad_draw::*, view::*, widget::*};

/// Hover reports from `Tip` wrappers to the window's `TipLayer`.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum TipAction {
    /// The pointer entered a tipped control: its text + final screen rect.
    HoverIn(String, Rect),
    /// The pointer left the tipped control.
    HoverOut,
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.TipBase = #(Tip::register_widget(vm))
    mod.widgets.Tip = set_type_default() do mod.widgets.TipBase{
        width: Fit
        height: Fit
    }

    mod.widgets.TipLayerBase = #(TipLayer::register_widget(vm))
    mod.widgets.TipLayer = set_type_default() do mod.widgets.TipLayerBase{
        width: Fill
        height: Fill
        draw_bg +: {
            color: #x10141bf2
            border_color: #xffffff2e
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, 4.0)
                sdf.fill(self.color)
                sdf.stroke(self.border_color, 1.0)
                return sdf.result
            }
        }
        draw_text +: {
            color: #xdfe6ec
            text_style: theme.font_regular{font_size: 9}
        }
    }
}

/// Hover dwell before a tip reveals.
const TIP_DELAY_SECS: f64 = 0.5;
/// Moving between tipped controls within this window keeps tips INSTANT
/// (the standard grace behavior).
const TIP_GRACE_SECS: f64 = 0.35;
/// Gap between the control and its tip.
const TIP_GAP: f64 = 6.0;
const TIP_PAD_X: f64 = 8.0;
const TIP_PAD_Y: f64 = 5.0;

/// Transparent tooltip DECLARATION wrapper: walks its child unchanged,
/// reports hover to the window's [`TipLayer`].
#[derive(Script, ScriptHook, Widget)]
pub struct Tip {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    /// The tip text. State-aware hosts update it with `set_text`.
    #[live]
    pub text: String,
}

impl Widget for Tip {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if self.text.is_empty() {
            return;
        }
        let uid = self.widget_uid();
        match event.hits_with_capture_overload(cx, self.view.area(), true) {
            Hit::FingerHoverIn(_) => {
                // The FINAL rect, straight from the drawn area — the one
                // positioning source the overlay law allows.
                let rect = self.view.area().rect(cx);
                cx.widget_action(uid, TipAction::HoverIn(self.text.clone(), rect));
            }
            Hit::FingerHoverOut(_) | Hit::FingerDown(_) => {
                cx.widget_action(uid, TipAction::HoverOut);
            }
            _ => {}
        }
    }
}

impl TipRef {
    /// State-aware tips ("Open output window" / "Close output window").
    pub fn set_text(&self, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.text = text.to_string();
        }
    }
}

/// The per-window tooltip HOST: delay + grace state machine, overlay
/// drawing, clamped placement. Put ONE as the last child of the window's
/// overlay stack.
#[derive(Script, Widget)]
pub struct TipLayer {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_text: DrawText,
    #[rust]
    draw_list: Option<DrawList2d>,
    #[rust]
    area: Area,
    /// The tip the pointer is dwelling toward (armed by HoverIn).
    #[rust]
    pending: Option<(String, Rect)>,
    /// The tip on screen.
    #[rust]
    showing: Option<(String, Rect)>,
    #[rust]
    timer: Timer,
    /// While the last tip was visible more recently than the grace window,
    /// the next one reveals instantly.
    #[rust]
    warm_until: f64,
}

impl ScriptHook for TipLayer {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }
}

impl TipLayer {
    fn show_pending(&mut self, cx: &mut Cx) {
        if let Some(tip) = self.pending.take() {
            self.showing = Some(tip);
            self.redraw_layer(cx);
        }
    }

    fn hide(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.timer);
        self.pending = None;
        if self.showing.take().is_some() {
            self.warm_until = cx.seconds_since_app_start() + TIP_GRACE_SECS;
            self.redraw_layer(cx);
        }
    }

    fn redraw_layer(&mut self, cx: &mut Cx) {
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
        self.area.redraw(cx);
    }
}

impl Widget for TipLayer {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // The layer's own turtle claims the window rect (for clamping);
        // the tip itself draws on the OVERLAY draw list so it floats over
        // every sibling panel.
        cx.begin_turtle(walk, Layout::default());
        let window = cx.turtle().rect();
        cx.end_turtle_with_area(&mut self.area);
        let Some((text, anchor)) = self.showing.clone() else {
            return DrawStep::done();
        };
        let Some(draw_list) = self.draw_list.as_mut() else {
            return DrawStep::done();
        };
        // The PROVEN popup idiom (PopupMenu): bubble as turtle content
        // at the overlay root, shifted to its clamped position.
        draw_list.begin_overlay_reuse(cx);
        let pass = cx.current_pass_size();
        cx.begin_root_turtle(pass, Layout::flow_down());
        let font_size = 9.0f64;
        let w = text.chars().count() as f64 * font_size * 0.62 + TIP_PAD_X * 2.0 + 4.0;
        let h = font_size * 1.5 + TIP_PAD_Y * 2.0;
        self.draw_bg.begin(
            cx,
            Walk::fixed(w, h),
            Layout::default(),
        );
        let bubble = cx.turtle().rect();
        self.draw_text.draw_abs(
            cx,
            dvec2(bubble.pos.x + TIP_PAD_X, bubble.pos.y + TIP_PAD_Y),
            &text,
        );
        self.draw_bg.end(cx);
        // Place BELOW the control, flip ABOVE at the window's bottom,
        // clamp horizontally.
        let mut x = anchor.pos.x + (anchor.size.x - w) * 0.5;
        x = x.clamp(window.pos.x + 2.0, (window.pos.x + window.size.x - w - 2.0).max(2.0));
        let below = anchor.pos.y + anchor.size.y + TIP_GAP;
        let y = if below + h > window.pos.y + window.size.y - 2.0 {
            (anchor.pos.y - TIP_GAP - h).max(window.pos.y + 2.0)
        } else {
            below
        };
        cx.end_pass_sized_turtle_with_shift(self.area, dvec2(x, y) - window.pos);
        draw_list.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.timer.is_event(event).is_some() {
            self.show_pending(cx);
        }
        // Any press or scroll dismisses instantly (performance surface:
        // tips must never linger over a working hand).
        match event {
            Event::MouseDown(_) | Event::Scroll(_) => self.hide(cx),
            _ => {}
        }
        if let Event::Actions(actions) = event {
            for action in actions.iter() {
                let Some(action) = action.as_widget_action() else { continue };
                match action.cast() {
                    TipAction::HoverIn(text, rect) => {
                        cx.stop_timer(self.timer);
                        let now = cx.seconds_since_app_start();
                        if self.showing.is_some() || now < self.warm_until {
                            // Grace: sliding along a row of tipped
                            // controls follows instantly.
                            self.pending = Some((text, rect));
                            self.show_pending(cx);
                        } else {
                            self.pending = Some((text, rect));
                            self.timer = cx.start_timeout(TIP_DELAY_SECS);
                        }
                    }
                    TipAction::HoverOut => self.hide(cx),
                    TipAction::None => {}
                }
            }
        }
    }
}

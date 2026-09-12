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

use crate::{
    badge::{BadgeIntent, BadgePalette},
    makepad_derive_widget::*,
    makepad_draw::*,
    overlay_place::{place_overlay, PlaceAlign, PlaceRequest, Placement, Side},
    view::*,
    widget::*,
};

/// Where a tip hangs, as one of the twelve places a popup can take.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Script, ScriptHook)]
#[repr(u32)]
pub enum TipPlace {
    /// Below the control, centred: the default a tooltip has always had.
    #[pick]
    Bottom = 0,
    BottomStart = 1,
    BottomEnd = 2,
    Top = 3,
    TopStart = 4,
    TopEnd = 5,
    Left = 6,
    LeftStart = 7,
    LeftEnd = 8,
    Right = 9,
    RightStart = 10,
    RightEnd = 11,
}

impl TipPlace {
    fn placement(self) -> Placement {
        let (side, align) = match self {
            TipPlace::Bottom => (Side::Bottom, PlaceAlign::Center),
            TipPlace::BottomStart => (Side::Bottom, PlaceAlign::Start),
            TipPlace::BottomEnd => (Side::Bottom, PlaceAlign::End),
            TipPlace::Top => (Side::Top, PlaceAlign::Center),
            TipPlace::TopStart => (Side::Top, PlaceAlign::Start),
            TipPlace::TopEnd => (Side::Top, PlaceAlign::End),
            TipPlace::Left => (Side::Left, PlaceAlign::Center),
            TipPlace::LeftStart => (Side::Left, PlaceAlign::Start),
            TipPlace::LeftEnd => (Side::Left, PlaceAlign::End),
            TipPlace::Right => (Side::Right, PlaceAlign::Center),
            TipPlace::RightStart => (Side::Right, PlaceAlign::Start),
            TipPlace::RightEnd => (Side::Right, PlaceAlign::End),
        };
        Placement::new(side, align)
    }
}

/// Everything a tip needs to draw itself, carried from the wrapper to the
/// layer so the layer holds no per-control settings of its own.
#[derive(Clone, Debug, PartialEq)]
pub struct TipRequest {
    pub text: String,
    /// The control's FINAL screen rect.
    pub anchor: Rect,
    pub place: TipPlace,
    /// Draw a pointer on the bubble's anchor-facing edge.
    pub arrow: bool,
    /// Wrap the text at this width; zero keeps it on one line.
    pub wrap_width: f64,
    /// Dwell before this tip reveals; negative takes the layer's own.
    pub delay_secs: f64,
    /// The role the bubble is drawn in.
    pub intent: BadgeIntent,
}

impl TipRequest {
    /// A plain tip, the way the wrapper has always asked for one.
    pub fn plain(text: String, anchor: Rect) -> Self {
        TipRequest {
            text,
            anchor,
            place: TipPlace::Bottom,
            arrow: false,
            wrap_width: 0.0,
            delay_secs: -1.0,
            intent: BadgeIntent::Neutral,
        }
    }
}

/// Hover reports from `Tip` wrappers to the window's `TipLayer`.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum TipAction {
    /// The pointer entered a tipped control: its text + final screen rect.
    /// The short form, kept because hosts outside this crate raise it.
    HoverIn(String, Rect),
    /// The pointer entered a tipped control that has more to say about how
    /// its tip should look.
    HoverInWith(TipRequest),
    /// The pointer left the tipped control.
    HoverOut,
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*

    // Registered before the `use` below: a block's `use` is a snapshot of
    // what exists when it runs.
    mod.widgets.TipPlace = set_type_default() do #(TipPlace::script_api(vm))
    mod.widgets.splat(mod.widgets.TipPlace)

    use mod.widgets.*

    mod.widgets.DrawTipBgBase = #(DrawTipBg::script_component(vm))
    set_type_default() do #(DrawTipBg::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.TipBase = #(Tip::register_widget(vm))
    mod.widgets.Tip = set_type_default() do mod.widgets.TipBase{
        width: Fit
        height: Fit
        /** where the bubble hangs: Bottom BottomStart Top Left Right and their ends */
        place: Bottom
        /** draw a pointer on the bubble's edge, aimed at the control */
        arrow: false
        /** wrap the text at this width; 0 keeps it on one line 0..600 step 10 */
        wrap_width: 0.
        /** dwell before this tip reveals; -1 takes the layer's own -1..3 step 0.05 */
        delay_secs: -1.
        /** the role the bubble is drawn in: Neutral Primary Error Warning Success Info */
        intent: Neutral
    }

    /** A tip that says something went wrong, in the error role. */
    mod.widgets.TipError = mod.widgets.Tip{
        intent: Error
    }

    /** A tip that warns, in the warning role. */
    mod.widgets.TipWarning = mod.widgets.Tip{
        intent: Warning
    }

    /** A tip with room for a sentence rather than a phrase. */
    mod.widgets.TipRich = mod.widgets.Tip{
        wrap_width: 240.
        arrow: true
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
        draw_arrow +: {
            color: #x10141bf2
            border_color: #xffffff2e
            /** which way the pointer AIMS: 0 up, 1 down, 2 right, 3 left 0..3 step 1 */
            side: 0.0
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                // A square turned by a quarter turn, centred on the BASE
                // edge — the one against the bubble. The half inside this
                // quad is the triangle, narrowing to a point at the far
                // edge, so the point is the end that aims at the control.
                // Centring it on the near edge instead makes the pointer
                // aim away from the thing it belongs to. The path fill the
                // shape language offers does not fill here, which is why
                // the popover draws its arrow the same way.
                let w = self.rect_size.x
                let h = self.rect_size.y
                // The centre sits ON the base edge, not a pixel inside it:
                // half the square is then clipped away and what is left is
                // a full triangle, base the width of the quad and apex
                // exactly on the far edge. A pixel of inset blunts the apex
                // and shortens the base, which is what made the pointer
                // read as a smear. The quad already overlaps the bubble by
                // a pixel, so nothing is lost by putting the base flush.
                let mut c = vec2(w * 0.5, h)
                let mut r = w * 0.5
                if self.side > 2.5 {
                    c = vec2(w, h * 0.5)
                    r = h * 0.5
                } else if self.side > 1.5 {
                    c = vec2(0.0, h * 0.5)
                    r = h * 0.5
                } else if self.side > 0.5 {
                    c = vec2(w * 0.5, 0.0)
                }
                sdf.rotate(PI * 0.25, c.x, c.y)
                let s = r * 1.41421356
                sdf.rect(c.x - s * 0.5, c.y - s * 0.5, s, s)
                sdf.fill_keep(self.color)
                sdf.stroke(self.border_color, 1.0)
                return sdf.result
            }
        }
    }
}

/// The tooltip bubble and its pointer: a quad that carries the colours the
/// layer swaps per role.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTipBg {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    border_color: Vec4f,
    /// Which way a pointer AIMS: 0 up, 1 down, 2 right, 3 left. The
    /// bubble ignores it.
    #[live]
    side: f32,
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
/// Half the width of the pointer on a tip that asks for one.
const TIP_ARROW: f64 = 5.0;
/// The bubble's visual corner radius. `sdf.box` draws twice the radius it
/// is given, and the bubble asks for 4, so its corners eat 8 points off
/// each end of every edge. A pointer has to stay clear of them.
const TIP_BUBBLE_R: f64 = 8.0;

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
    #[live]
    pub place: TipPlace,
    #[live]
    pub arrow: bool,
    #[live]
    pub wrap_width: f64,
    #[live(-1.0)]
    pub delay_secs: f64,
    #[live]
    pub intent: BadgeIntent,
    /// Whether the pointer is over the wrapped child right now.
    #[rust]
    hovered: bool,
    /// Whether the wrapped child held the key focus at the last look: a tip
    /// that only answers the pointer is invisible to someone working by
    /// keyboard.
    #[rust]
    focused: bool,
}

impl Tip {
    fn request(&self, cx: &Cx) -> TipRequest {
        TipRequest {
            text: self.text.clone(),
            anchor: self.view.area().rect(cx),
            place: self.place,
            arrow: self.arrow,
            wrap_width: self.wrap_width,
            delay_secs: self.delay_secs,
            intent: self.intent,
        }
    }
}

impl Widget for Tip {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // The wrapper cannot hit-test for hover itself: the child it wraps
        // claims the pointer first, and `Event::hits` answers hover only to
        // the claiming area. So use the claim itself — snapshot it around the
        // subtree dispatch (the documented `pointer_claimed_area` idiom): a
        // claim that appeared across it is the child under the pointer. This
        // also keeps tips honest under a modal grab (an open menu's
        // `sweep_lock`): a locked-out child claims nothing, so no tip.
        let claimed_before = if let Event::MouseMove(_) = event {
            event.pointer_claimed_area()
        } else {
            Area::Empty
        };
        self.view.handle_event(cx, event, scope);
        if self.text.is_empty() {
            return;
        }
        let uid = self.widget_uid();
        match event {
            Event::MouseMove(_) => {
                let claimed_after = event.pointer_claimed_area();
                let over = claimed_after != claimed_before && !claimed_after.is_empty();
                if over != self.hovered {
                    self.hovered = over;
                    if over {
                        // The FINAL rect, straight from the drawn area — the
                        // one positioning source the overlay law allows.
                        cx.widget_action(uid, TipAction::HoverInWith(self.request(cx)));
                    } else {
                        cx.widget_action(uid, TipAction::HoverOut);
                    }
                }
            }
            Event::MouseLeave(_) | Event::ClearHover => {
                if self.hovered {
                    self.hovered = false;
                    cx.widget_action(uid, TipAction::HoverOut);
                }
            }
            // Reaching a control by keyboard shows its tip too: the pointer
            // is not the only way in, and a tip nobody can reach by Tab is
            // a tip half the users never see.
            Event::KeyFocus(_) | Event::KeyFocusLost(_) => {
                let focused = cx.has_key_focus(self.view.area());
                if focused != self.focused {
                    self.focused = focused;
                    if focused && !self.hovered {
                        cx.widget_action(uid, TipAction::HoverInWith(self.request(cx)));
                    } else if !focused && !self.hovered {
                        cx.widget_action(uid, TipAction::HoverOut);
                    }
                }
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
    draw_bg: DrawTipBg,
    #[live]
    draw_text: DrawText,
    #[rust]
    draw_list: Option<DrawList2d>,
    #[rust]
    area: Area,
    /// The tip the pointer is dwelling toward (armed by HoverIn).
    #[rust]
    pending: Option<TipRequest>,
    /// The tip on screen.
    #[rust]
    showing: Option<TipRequest>,
    /// The role colours, shared with every other small mark.
    #[live]
    palette: BadgePalette,
    /// The pointer on the bubble's edge, drawn when a tip asks for one.
    #[live]
    draw_arrow: DrawTipBg,
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
        // The layer's own turtle gives the origin its overlay pass is
        // shifted FROM. It is not the room a tip has to fit in: a layer
        // declared as the last child of a page gets whatever space is left
        // at the bottom of it, and clamping tips to that strip drags every
        // one of them down there. The room is the pass, below.
        cx.begin_turtle(walk, Layout::default());
        let origin = cx.turtle().rect().pos;
        cx.end_turtle_with_area(&mut self.area);
        let Some(tip) = self.showing.clone() else {
            return DrawStep::done();
        };
        let (text, anchor) = (tip.text.clone(), tip.anchor);
        let Some(draw_list) = self.draw_list.as_mut() else {
            return DrawStep::done();
        };
        // The PROVEN popup idiom (PopupMenu): bubble as turtle content
        // at the overlay root, shifted to its clamped position.
        draw_list.begin_overlay_reuse(cx);
        let pass = cx.current_pass_size();
        cx.begin_root_turtle(pass, Layout::flow_down());
        let font_size = 9.0f64;
        // MEASURE the run rather than guess it: a per-character estimate
        // under-measures wide glyphs (all-caps text most of all) and the
        // bubble then clips its own last letter. The estimate stays only as
        // the fallback for text the layout engine returns no row for.
        let text_w = self
            .draw_text
            .prepare_single_line_run(cx, &text)
            .map(|run| run.width_in_lpxs as f64)
            .unwrap_or_else(|| text.chars().count() as f64 * font_size * 0.62);
        // A wrapped tip is as wide as it was told and as tall as the lines
        // it needs; an unwrapped one is one line, as it always was.
        let (w, lines) = if tip.wrap_width > 0.0 && text_w + TIP_PAD_X * 2.0 > tip.wrap_width {
            let room = (tip.wrap_width - TIP_PAD_X * 2.0).max(font_size * 4.0);
            (tip.wrap_width, (text_w / room).ceil().max(1.0))
        } else {
            (text_w + TIP_PAD_X * 2.0 + 4.0, 1.0)
        };
        let h = font_size * 1.6 * lines + TIP_PAD_Y * 2.0;
        // The role decides the bubble's colours; Neutral keeps the chrome
        // the theme gave it, so nothing that never asked for a role moves.
        let rest_bg = self.draw_bg.color;
        let rest_ink = self.draw_text.color;
        if tip.intent != BadgeIntent::Neutral {
            let family = self.palette.family(tip.intent);
            self.draw_bg.color = family.base;
            self.draw_text.color = family.on_base;
        }
        // Where the bubble landed in the pass. Everything drawn beside it
        // is placed against this, because the pass is shifted into position
        // only at the end.
        // A pointer on the bubble's TOP or LEFT edge is drawn before the
        // bubble's own origin, and the overlay pass is sized to what was
        // drawn from that origin onward, so it was simply clipped away.
        // Inset the bubble by the pointer's length and give the shift the
        // same amount back: there is then room on every side of it.
        let inset = if tip.arrow { TIP_ARROW } else { 0.0 };
        let pad = Inset { left: inset, top: inset, right: inset, bottom: inset };
        let bubble;
        let mut h = h;
        if lines > 1.0 {
            // Text wraps only when the turtle it is drawn into says it may,
            // so the bubble is a wrapping row with its own padding and the
            // text takes the width, finding its own height.
            //
            // That height is then MEASURED, not taken from the estimate
            // above. Words do not split, so a line holds less than the
            // width says and the guess is always short by some fraction of
            // a line: a bubble built to the guess clips its own last line
            // against its bottom edge. The estimate stays where it is
            // honest, which is deciding whether the text wraps at all.
            self.draw_bg.begin(
                cx,
                Walk {
                    width: Size::Fixed(w),
                    height: Size::fit(),
                    margin: pad,
                    ..Walk::default()
                },
                Layout {
                    flow: Flow::right_wrap(),
                    padding: Inset {
                        left: TIP_PAD_X,
                        right: TIP_PAD_X,
                        top: TIP_PAD_Y,
                        bottom: TIP_PAD_Y,
                    },
                    ..Layout::default()
                },
            );
            self.draw_text.draw_walk(
                cx,
                Walk {
                    width: Size::Fixed(w - TIP_PAD_X * 2.0),
                    height: Size::fit(),
                    ..Walk::default()
                },
                Align { x: 0.0, y: 0.0 },
                &text,
            );
            self.draw_bg.end(cx);
            bubble = self.draw_bg.area().rect(cx);
            h = bubble.size.y;
        } else {
            self.draw_bg.begin(
                cx,
                Walk { margin: pad, ..Walk::fixed(w, h) },
                Layout::default(),
            );
            bubble = cx.turtle().rect();
            self.draw_text.draw_abs(
                cx,
                dvec2(bubble.pos.x + TIP_PAD_X, bubble.pos.y + TIP_PAD_Y),
                &text,
            );
            self.draw_bg.end(cx);
        }
        // One placement helper, the same one every anchored popup uses:
        // the wanted side, flipped when there is no room, shifted to stay
        // inside the window. The room is the WHOLE PASS, which is what a
        // tooltip may cover, and it is the space the anchor rects are
        // measured in.
        let placed = place_overlay(&PlaceRequest {
            anchor,
            size: dvec2(w, h),
            bounds: Rect {
                pos: dvec2(2.0, 2.0),
                size: dvec2(pass.x - 4.0, pass.y - 4.0),
            },
            gap: TIP_GAP,
            placement: tip.place.placement(),
            match_anchor_width: false,
        });
        if tip.arrow {
            // The pointer sits on the bubble's anchor-facing edge, at the
            // point the helper worked out, so it aims at the control even
            // after a flip or a shift.
            self.draw_arrow.color = self.draw_bg.color;
            let a = TIP_ARROW;
            let at = placed.arrow_at;
            // The pointer sits ON the bubble's anchor-facing edge, overlapping
            // it by a pixel so its base covers the bubble's own outline.
            // The anchor point, carried into the bubble's own space.
            let local = dvec2(
                bubble.pos.x + (at.x - placed.rect.pos.x),
                bubble.pos.y + (at.y - placed.rect.pos.y),
            );
            // Keep the whole pointer on the FLAT part of the edge. The
            // placement helper only clamps the anchor point to the bubble's
            // extent, so on a control wider than its own tip the point lands
            // on a rounded corner and one flank of the triangle is drawn over
            // bare background, with a notch where the corner curves away.
            let along = |v: f64, start: f64, extent: f64| -> f64 {
                let lo = start + TIP_BUBBLE_R + a;
                let hi = start + extent - TIP_BUBBLE_R - a;
                if hi <= lo {
                    start + extent * 0.5
                } else {
                    v.max(lo).min(hi)
                }
            };
            let ax = along(local.x, bubble.pos.x, bubble.size.x);
            let ay = along(local.y, bubble.pos.y, bubble.size.y);
            let (rect, side) = match placed.side {
                Side::Bottom => (
                    Rect { pos: dvec2(ax - a, bubble.pos.y - a + 1.0), size: dvec2(a * 2.0, a) },
                    0.0,
                ),
                Side::Top => (
                    Rect {
                        pos: dvec2(ax - a, bubble.pos.y + bubble.size.y - 1.0),
                        size: dvec2(a * 2.0, a),
                    },
                    1.0,
                ),
                Side::Right => (
                    Rect { pos: dvec2(bubble.pos.x - a + 1.0, ay - a), size: dvec2(a, a * 2.0) },
                    3.0,
                ),
                Side::Left => (
                    Rect {
                        pos: dvec2(bubble.pos.x + bubble.size.x - 1.0, ay - a),
                        size: dvec2(a, a * 2.0),
                    },
                    2.0,
                ),
            };
            self.draw_arrow.side = side;
            self.draw_arrow.draw_abs(cx, rect);
        }
        self.draw_bg.color = rest_bg;
        self.draw_text.color = rest_ink;
        cx.end_pass_sized_turtle_with_shift(self.area, placed.rect.pos - origin - dvec2(inset, inset));
        draw_list.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.timer.is_event(event).is_some() {
            self.show_pending(cx);
        }
        // Any press or scroll dismisses instantly (performance surface:
        // tips must never linger over a working hand), and so does an
        // overlay clearing hover state for everyone.
        match event {
            Event::MouseDown(_) | Event::Scroll(_) | Event::ClearHover => self.hide(cx),
            // Escape takes a tip down like anything else on the screen,
            // which matters most for one revealed by the keyboard.
            Event::KeyDown(ke) if ke.key_code == KeyCode::Escape => self.hide(cx),
            _ => {}
        }
        if let Event::Actions(actions) = event {
            for action in actions.iter() {
                let Some(action) = action.as_widget_action() else { continue };
                let request = match action.cast() {
                    TipAction::HoverIn(text, rect) => Some(TipRequest::plain(text, rect)),
                    TipAction::HoverInWith(request) => Some(request),
                    TipAction::HoverOut => {
                        self.hide(cx);
                        None
                    }
                    TipAction::None => None,
                };
                if let Some(request) = request {
                    cx.stop_timer(self.timer);
                    let now = cx.seconds_since_app_start();
                    let delay =
                        if request.delay_secs >= 0.0 { request.delay_secs } else { TIP_DELAY_SECS };
                    self.pending = Some(request);
                    if self.showing.is_some() || now < self.warm_until || delay <= 0.0 {
                        // Grace: sliding along a row of tipped controls
                        // follows instantly.
                        self.show_pending(cx);
                    } else {
                        self.timer = cx.start_timeout(delay);
                    }
                }
            }
        }
    }
}

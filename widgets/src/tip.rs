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
//! `Tip` is a transparent wrapper: around one control it has no layout of
//! its own. Its parent lays out the control's walk, read fresh each time it
//! is asked, and the control draws with that walk straight into the parent's
//! turtle, so a tipped control sits exactly where the bare control would —
//! bounded and basis Fills, margins, and walks a host changes at runtime
//! included. The `Tip`'s own walk and layout (size, margin, padding, flow,
//! spacing, align, background) count only when it wraps none or several
//! children. The wrapper only REPORTS hover with its text and the child's
//! FINAL screen rect.
//!
//! One `TipLayer` per window — the last child of the window's
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
//!
//! The pointer is not the only way to a tip. Reaching a control by keyboard
//! raises its tip, until the control is typed into or activated. A finger
//! has no hover, so touch raises a tip by a long press, and only for as
//! long as the finger is held.

use crate::{
    badge::{BadgeIntent, BadgePalette},
    makepad_derive_widget::*,
    makepad_draw::{event::TouchState, *},
    overlay_place::{
        place_overlay, pointer_on_edge, slide_for_pointer, PlaceAlign, PlaceRequest, Placed, Placement, Pointer, Side,
    },
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
        /** The bubble and its pointer, drawn as ONE shape and filled once,
         * so nothing shows where the pointer meets the bubble. */
        draw_bg +: {
            color: #x10141bf2
            quad_shift: varying(vec2(0))
            quad_size: varying(vec2(0))
            vertex: fn() {
                // The bubble's rect, grown by the pointer past the edge it
                // stands out of, and by a point all round for the edge
                // smoothing that falls past the pointer's point.
                let room = vec2(1.0)
                let lead = room + max(-self.arrow_tip, vec2(0))
                let trail = room + max(self.arrow_tip - self.rect_size, vec2(0))
                self.quad_shift = -lead
                self.quad_size = self.rect_size + lead + trail
                return self.clip_and_transform_vertex(self.rect_pos - lead, self.quad_size)
            }
            pixel: fn() {
                // In the bubble's own points, where the layer put the pointer.
                let p = self.pos * self.quad_size + self.quad_shift
                let sdf = Sdf2d.viewport(p)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, 4.0)
                sdf.pointer(self.arrow_base.x, self.arrow_base.y, self.arrow_tip.x, self.arrow_tip.y)
                // A fill and no outline, the way a tip bubble has always looked.
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_text +: {
            color: #xdfe6ec
            text_style: theme.font_regular{font_size: 9}
        }
    }
}

/// The tooltip bubble and its pointer, one shape: a quad that carries the
/// colours the layer swaps per role and where the pointer stands.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTipBg {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    /// The middle of the pointer's base, in the bubble's own points.
    #[live]
    arrow_base: Vec2f,
    /// The pointer's point, in the bubble's own points; the same as the base
    /// when the tip has no pointer.
    #[live]
    arrow_tip: Vec2f,
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
/// Half the width of the pointer on a tip that asks for one, and how far it
/// stands out.
const TIP_ARROW: f64 = 5.0;
/// The bubble's visual corner radius. `sdf.box` draws twice the radius it
/// is given, and the bubble asks for 4, so its corners eat 8 points off
/// each end of every edge. A pointer has to stay clear of them.
const TIP_BUBBLE_R: f64 = 8.0;
/// Room kept past a pointer's point on the overlay for the edge smoothing
/// that falls beyond it.
const TIP_ROOM: f64 = 1.0;

/// How far a rounded corner of a bubble of `size` reaches along its edges
/// from each end, on the shape the bubble shader draws: a box half a point
/// in, with corners [`TIP_BUBBLE_R`] long unless the bubble is too small to
/// hold them.
fn bubble_corner(size: DVec2) -> f64 {
    0.5 + TIP_BUBBLE_R.min((size.x.min(size.y) - 1.0) * 0.5).max(0.0)
}

/// Hang a bubble of `size` off `anchor` in a pass of `pass`: where it goes,
/// and the pointer it draws when `arrow` asks for one, in its own points.
///
/// The placement comes first, then the slide off a control too small for
/// the pointer to reach its middle from where the bubble lines up, then the
/// pointer, on the straight part of the edge that faces the control. The
/// layer draws through here, and so do the tests.
fn hang_bubble(anchor: Rect, size: DVec2, pass: DVec2, place: TipPlace, arrow: bool) -> (Placed, Option<Pointer>) {
    // The room is the WHOLE PASS, which is what a tooltip may cover, and it
    // is the space the anchor rects are measured in.
    let bounds = Rect {
        pos: dvec2(2.0, 2.0),
        size: dvec2(pass.x - 4.0, pass.y - 4.0),
    };
    let placed = place_overlay(&PlaceRequest {
        anchor,
        size,
        bounds,
        gap: TIP_GAP,
        placement: place.placement(),
        match_anchor_width: false,
    });
    if !arrow {
        return (placed, None);
    }
    let corner = bubble_corner(size);
    let placed = slide_for_pointer(placed, anchor, bounds, corner + TIP_ARROW);
    // The helper only clamps the point to the bubble's extent, and on a
    // control wider than its tip that is a rounded corner, where the base
    // would leave a notch as the curve falls away under it: the pointer
    // keeps to the straight part.
    let pointer = pointer_on_edge(placed.side, size, placed.arrow_at - placed.rect.pos, TIP_ARROW, 0.5, corner);
    (placed, Some(pointer))
}

/// Transparent tooltip DECLARATION wrapper: lays its one child out as if
/// the wrapper were not there, reports hover to the window's [`TipLayer`].
///
/// `WidgetNode` is written out below rather than derived, for the methods
/// that make it transparent: `walk`, `area`, `visible` and `set_visible`.
#[derive(Script, ScriptHook, WidgetRef, WidgetSet, WidgetRegister)]
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
    /// Whether the last press, a mouse button's or a finger's, landed on the
    /// wrapped child with no key pressed since. The focus a press hands over
    /// is the pointer's and raises no tip: a finger has no hover, so nothing
    /// would ever take that tip down.
    #[rust]
    pressed: bool,
    /// The finger that came down on the wrapped child and is still down.
    #[rust]
    finger: Option<u64>,
}

impl WidgetNode for Tip {
    fn widget_uid(&self) -> WidgetUid {
        self.view.widget_uid()
    }
    /// A wrapper whose every child is hidden is hidden too. A parent asks
    /// this before it walks the child, and a Fill wrapper along the
    /// parent's flow is deferred there — spacing taken, Fill share booked —
    /// before `draw_walk` below could return early. So the answer has to
    /// come from here, or a hidden control leaves a hole for having a tip.
    fn visible(&self) -> bool {
        self.view.visible() && !self.children_all_hidden()
    }
    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        if self.sole_child().is_none() {
            return self.view.set_visible(cx, visible);
        }
        // A forwarding wrapper never draws a turtle of its own, so the
        // view's area stays empty and `View::set_visible` would take every
        // show for a first one and repaint the whole window. The child's
        // area is the wrapper's; only a child that has never drawn needs
        // the full repaint.
        if self.view.visible != visible {
            self.view.visible = visible;
            if visible && self.area().is_empty() {
                cx.redraw_all();
            } else {
                self.redraw(cx);
            }
        }
    }
    /// Around one control the wrapper's area is the control's: the rect a
    /// tip hangs from, and the area that holds the key focus.
    fn area(&self) -> Area {
        match self.sole_child() {
            Some(child) => child.area(),
            None => self.view.area(),
        }
    }
    /// Around one control this is the control's walk as it is NOW, not a
    /// copy taken when the tree was built: a host that resizes the control
    /// at runtime, or restyles it by id, moves the wrapper with it.
    fn walk(&mut self, cx: &mut Cx) -> Walk {
        match self.sole_child() {
            Some(child) => child.walk(cx),
            None => self.view.walk(cx),
        }
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.view.redraw(cx)
    }
    fn set_scroll_pos(&mut self, cx: &mut Cx, v: Vec2d) {
        self.view.set_scroll_pos(cx, v)
    }
    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        self.view.children(visit)
    }
    fn skip_widget_tree_search(&self) -> bool {
        self.view.skip_widget_tree_search()
    }
    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        self.view.find_widgets_from_point(cx, point, found)
    }
    fn selection_text_len(&self) -> usize {
        self.view.selection_text_len()
    }
    fn selection_point_to_char_index(&self, cx: &Cx, abs: DVec2) -> Option<usize> {
        self.view.selection_point_to_char_index(cx, abs)
    }
    fn selection_set(&mut self, anchor: usize, cursor: usize) {
        self.view.selection_set(anchor, cursor)
    }
    fn selection_clear(&mut self) {
        self.view.selection_clear()
    }
    fn selection_select_all(&mut self) {
        self.view.selection_select_all()
    }
    fn selection_get_text_for_range(&self, start: usize, end: usize) -> String {
        self.view.selection_get_text_for_range(start, end)
    }
    fn selection_get_full_text(&self) -> String {
        self.view.selection_get_full_text()
    }
}

impl Tip {
    /// Whether the wrapper holds children and every one of them is hidden.
    fn children_all_hidden(&self) -> bool {
        !self.view.children.is_empty() && self.view.children.iter().all(|(_, child)| !child.visible())
    }

    /// The control a wrapper forwards its layout to: its child, when it
    /// has exactly one. With none or several it lays out as the view it is.
    fn sole_child(&self) -> Option<&WidgetRef> {
        match &self.view.children[..] {
            [(_, child)] => Some(child),
            _ => None,
        }
    }

    /// The control holding the key focus was typed into, edited, or
    /// activated from the keyboard. A tip its focus raised has said what the
    /// control is by now, and goes rather than sit over what the typing is
    /// for. Only focus arriving raises one, so it stays gone until the focus
    /// moves again. A tip the pointer raised is the pointer's to take down,
    /// and the owner rule keeps one raised over another control up.
    fn typed(&self, cx: &mut Cx) {
        let area = self.area();
        if self.focused && !self.hovered && !area.is_empty() && cx.has_key_focus(area) {
            cx.widget_action(self.widget_uid(), TipAction::HoverOut);
        }
    }

    /// Keys that edit the focused control or activate it, as against keys
    /// that move about in it or away from it. Text arrives as its own event.
    fn edits_or_activates(key: &KeyEvent) -> bool {
        match key.key_code {
            KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::ReturnKey
            | KeyCode::NumpadEnter
            | KeyCode::Space => true,
            // Undo and redo.
            KeyCode::KeyZ => key.modifiers.is_primary(),
            _ => false,
        }
    }

    fn request(&self, cx: &Cx) -> TipRequest {
        TipRequest {
            text: self.text.clone(),
            anchor: self.area().rect(cx),
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
        // A hidden child leaves no walk behind, so neither does its
        // wrapper: an empty Fit box would still take the parent's spacing,
        // and a control that hides itself would then shift its row by one
        // gap for having a tip. Transparent means transparent when hidden.
        // A Fill wrapper along its parent's flow never gets here when
        // hidden: `visible` above already said so.
        if self.children_all_hidden() {
            return DrawStep::done();
        }
        let Some(child) = self.sole_child() else {
            return self.view.draw_walk(cx, scope, walk);
        };
        if !self.view.visible {
            return DrawStep::done();
        }
        // `walk` is the child's own (see `walk` above), as the parent laid
        // it out: a Fill already given its share, a margin already counted
        // once. The child takes it into the parent's turtle directly. A
        // turtle of the wrapper's own in between would size and place the
        // child a second time, against a box that is not its parent. A
        // child that yields hands its step up unchanged and is resumed
        // through here with the same walk.
        child.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // The wrapper cannot hit-test for hover itself: the child it wraps
        // claims the pointer first, and `Event::hits` answers hover only to
        // the claiming area. So use the claim itself — snapshot it around the
        // subtree dispatch (the documented `pointer_claimed_area` idiom): a
        // claim that appeared across it is the child under the pointer. This
        // also keeps tips honest under a modal grab (an open menu's
        // `sweep_lock`): a locked-out child claims nothing, so no tip. A press
        // is read the same way, a mouse button's or a finger's.
        let claimed_before = match event {
            Event::MouseMove(_) | Event::MouseDown(_) | Event::TouchUpdate(_) => {
                event.pointer_claimed_area()
            }
            _ => Area::Empty,
        };
        self.view.handle_event(cx, event, scope);
        if self.text.is_empty() {
            return;
        }
        let uid = self.widget_uid();
        let claimed = || {
            let claimed_after = event.pointer_claimed_area();
            claimed_after != claimed_before && !claimed_after.is_empty()
        };
        match event {
            Event::MouseMove(_) => {
                let over = claimed();
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
            Event::MouseDown(_) => self.pressed = claimed(),
            // A finger landing raises nothing, not even through the focus it
            // hands the control: with no hover there is nothing to take such
            // a tip down, and it would stand over every control tapped.
            Event::TouchUpdate(touch) => {
                if let Some(start) = touch.touches.iter().find(|t| t.state == TouchState::Start) {
                    self.pressed = claimed();
                    if self.pressed {
                        self.finger = Some(start.uid);
                    }
                }
                if let Some(finger) = self.finger {
                    if touch.touches.iter().any(|t| t.uid == finger && t.state == TouchState::Stop) {
                        self.finger = None;
                        cx.widget_action(uid, TipAction::HoverOut);
                    }
                }
            }
            // Holding a finger on the control is how touch asks what it is.
            // The hold was the dwell, so the tip shows at once, and lifting
            // the finger takes it down again.
            Event::LongPress(press) if self.finger == Some(press.uid) => {
                let mut request = self.request(cx);
                request.delay_secs = 0.0;
                cx.widget_action(uid, TipAction::HoverInWith(request));
            }
            // Reaching a control by keyboard shows its tip too: the pointer
            // is not the only way in, and a tip nobody can reach by Tab is
            // a tip half the users never see.
            Event::KeyFocus(_) | Event::KeyFocusLost(_) => {
                // An area that was never drawn is empty, and so is the key
                // focus when nothing holds it: without the guard every such
                // wrapper would count itself focused the moment focus left.
                let area = self.area();
                let focused = !area.is_empty() && cx.has_key_focus(area);
                if focused != self.focused {
                    self.focused = focused;
                    // Focus a press handed over came by the pointer, which
                    // raises its own tips.
                    if focused && !self.hovered && !self.pressed {
                        cx.widget_action(uid, TipAction::HoverInWith(self.request(cx)));
                    } else if !focused && !self.hovered {
                        cx.widget_action(uid, TipAction::HoverOut);
                    }
                }
            }
            Event::KeyDown(key) => {
                // The keyboard is in use: the next focus to arrive is its.
                self.pressed = false;
                if Self::edits_or_activates(key) {
                    self.typed(cx);
                }
            }
            Event::TextInput(_) | Event::TextRangeReplace(_) | Event::TextCut(_) | Event::ImeAction(_) => {
                self.typed(cx)
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
    /// The widget that raised the tip pending or on screen. Only its own
    /// HoverOut takes that tip down.
    #[rust]
    owner: Option<WidgetUid>,
    /// The role colours, shared with every other small mark.
    #[live]
    palette: BadgePalette,
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
        self.owner = None;
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
        let inset = if tip.arrow { TIP_ARROW + TIP_ROOM } else { 0.0 };
        let pad = Inset { left: inset, top: inset, right: inset, bottom: inset };
        // No pointer until the bubble is placed: its instance is written
        // when it begins, before anything knows where it will land.
        self.draw_bg.arrow_base = Vec2f::default();
        self.draw_bg.arrow_tip = Vec2f::default();
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
                // Unclipped: the pointer is drawn outside the bubble's rect.
                Layout {
                    flow: Flow::right_wrap(),
                    padding: Inset {
                        left: TIP_PAD_X,
                        right: TIP_PAD_X,
                        top: TIP_PAD_Y,
                        bottom: TIP_PAD_Y,
                    },
                    clip_x: false,
                    clip_y: false,
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
                Layout { clip_x: false, clip_y: false, ..Layout::default() },
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
        // inside the window.
        let (placed, pointer) = hang_bubble(anchor, dvec2(w, h), pass, tip.place, tip.arrow);
        if let Some(pointer) = pointer {
            // The pointer is part of the bubble's own shape, so it goes into
            // the instance the bubble has already written: on the edge that
            // faces the control, at the point the helper worked out, so it
            // aims at the control even after a flip or a shift.
            self.draw_bg.arrow_base = pointer.base.into();
            self.draw_bg.arrow_tip = pointer.tip.into();
            self.draw_bg.update_instance_area_value(cx, ids!(arrow_base));
            self.draw_bg.update_instance_area_value(cx, ids!(arrow_tip));
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
            // A finger landing is a press like a mouse button's.
            Event::TouchUpdate(touch) if touch.touches.iter().any(|t| t.state == TouchState::Start) => {
                self.hide(cx)
            }
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
                    // Reports arrive in the order the tree visits the
                    // controls, not the order things happened. When focus
                    // or the pointer moves on to a control the tree visits
                    // first, that control's HoverIn is read before the
                    // HoverOut of the one it left, and a HoverOut from
                    // anyone would hide the tip that has just arrived. A
                    // control losing a focus nobody was looking at would
                    // take down another control's tip the same way.
                    TipAction::HoverOut => {
                        if self.owner == Some(action.widget_uid) {
                            self.hide(cx);
                        }
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
                    self.owner = Some(action.widget_uid);
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

#[cfg(test)]
mod tests {
    //! A tipped control must sit exactly where the bare control sits. Each
    //! layout test draws the same row twice in one window-less pass, once
    //! bare and once tipped, one row-height apart, and compares the rects.
    //! The pointer tests at the end draw nothing: they hang a bubble and
    //! its pointer through the same function the layer draws with.
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;
    use crate::makepad_draw::event::{LongPressEvent, TouchPoint, TouchState, TouchUpdateEvent};
    use crate::makepad_script::{script, ScriptMod};

    fn cx() -> crate::PooledCx {
        crate::checkout_test_cx()
    }

    /// What a test draws into. The areas read after a draw live in the pass
    /// and the draw list, so the test keeps them for as long as it reads.
    struct Target {
        pass: DrawPass,
        draw_list: DrawList2d,
        size: DVec2,
    }

    impl Target {
        fn new(cx: &mut Cx, size: DVec2) -> Self {
            let pass = DrawPass::new(cx);
            pass.set_size(cx, size);
            let draw_list = DrawList2d::new(cx);
            Target { pass, draw_list, size }
        }

        /// One frame of `root`, to completion.
        fn draw(&mut self, cx: &mut Cx, root: &mut View) {
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&self.pass, None);
            self.draw_list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(self.size, Layout::flow_overlay());
            let walk = root.walk;
            root.draw_walk_all(&mut cx2d, &mut Scope::empty(), walk);
            cx2d.end_pass_sized_turtle();
            self.draw_list.end(&mut cx2d);
            cx2d.end_pass(&self.pass);
        }
    }

    /// The widget reached from `root` by child indices, stepping through a
    /// tip the way the widget tree does.
    fn at(root: &View, path: &[usize]) -> WidgetRef {
        let mut widget = root.children[path[0]].1.clone();
        for &index in &path[1..] {
            let mut children = Vec::new();
            widget.children(&mut |_, child| children.push(child));
            widget = children.into_iter().nth(index).expect("no such child");
        }
        widget
    }

    fn rect(cx: &Cx, widget: &WidgetRef) -> Rect {
        widget.area().rect(cx)
    }

    fn below(rect: Rect, dy: f64) -> Rect {
        Rect { pos: rect.pos + dvec2(0.0, dy), size: rect.size }
    }

    fn view(cx: &mut Cx, source: ScriptMod) -> View {
        cx.with_vm(|vm| {
            let value = vm.eval(source);
            View::script_from_value(vm, value)
        })
    }

    fn tip(cx: &mut Cx, source: ScriptMod) -> Tip {
        cx.with_vm(|vm| {
            let value = vm.eval(source);
            Tip::script_from_value(vm, value)
        })
    }

    /// The walk a parent lays out for a wrapper around one control is that
    /// control's walk as it is when asked, not the wrapper's and not a copy;
    /// around several controls it is the wrapper's own again.
    #[test]
    fn a_tip_around_one_control_walks_as_the_control_does_now() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut one = tip(&mut cx, script! {
            use mod.prelude.widgets.*
            Tip{ text: "one" width: 33 height: 44
                View{ width: Fill{min: 20. max: 120.} height: 12 margin: Inset{left: 3} }
            }
        });
        let walk = one.walk(&mut cx);
        assert_eq!(walk.width, Size::Fill { weight: 100.0, basis: FitBound::Abs(0.0), shrink: 0.0, min: Some(20.0), max: Some(120.0) });
        assert_eq!(walk.height, Size::Fixed(12.0));
        assert_eq!(walk.margin.left, 3.0);

        // A host resizing the control at runtime, as a splitter turning does.
        one.view.children[0].1.borrow_mut::<View>().unwrap().walk.height = Size::fill();
        assert!(one.walk(&mut cx).height.is_fill(), "the walk is read when asked");

        let mut several = tip(&mut cx, script! {
            use mod.prelude.widgets.*
            Tip{ text: "several" width: 33 height: 44
                View{ width: 10 height: 12 }
                View{ width: 10 height: 12 }
            }
        });
        let walk = several.walk(&mut cx);
        assert_eq!(walk.width, Size::Fixed(33.0));
        assert_eq!(walk.height, Size::Fixed(44.0));
        });
    }

    /// A bounded Fill with a margin beside a fixed sibling: the box keeps its
    /// ceiling, its margin is counted once, and the sibling follows it
    /// directly rather than after a wrapper that took the whole Fill share.
    #[test]
    fn a_bounded_fill_with_a_margin_lands_where_the_bare_control_does() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut root = view(&mut cx, script! {
            use mod.prelude.widgets.*
            View{ width: 600 height: 80 flow: Down
                View{ width: Fill height: 40 flow: Right spacing: 4
                    View{ width: Fill{min: 20. max: 120.} height: 20 margin: Inset{left: 3 top: 2} }
                    View{ width: 50 height: 20 }
                }
                View{ width: Fill height: 40 flow: Right spacing: 4
                    Tip{ text: "tipped"
                        View{ width: Fill{min: 20. max: 120.} height: 20 margin: Inset{left: 3 top: 2} }
                    }
                    View{ width: 50 height: 20 }
                }
            }
        });
        let mut target = Target::new(&mut cx, dvec2(600.0, 80.0));
        target.draw(&mut cx, &mut root);

        let bare = rect(&cx, &at(&root, &[0, 0]));
        let tipped = rect(&cx, &at(&root, &[1, 0, 0]));
        // A Fill's bounds hold its margin box: 120 less the 3 of margin.
        assert_eq!(bare.size.x, 117.0, "the bare box keeps its ceiling");
        assert_eq!(below(bare, 40.0), tipped);
        assert_eq!(rect(&cx, &at(&root, &[1, 0])), tipped, "the tip hangs from the control's rect");
        assert_eq!(below(rect(&cx, &at(&root, &[0, 1])), 40.0), rect(&cx, &at(&root, &[1, 1])));
        });
    }

    /// A control whose walk the host turns at runtime: a grip that is a
    /// full-width bar while its row is stacked and a full-height bar once the
    /// row runs sideways. The wrapper follows both ways, and the Fill beside
    /// it keeps the share the bare layout gives it.
    #[test]
    fn a_walk_turned_at_runtime_moves_the_wrapper_with_it() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut root = view(&mut cx, script! {
            use mod.prelude.widgets.*
            View{ width: 400 height: 200 flow: Down
                View{ width: Fill height: 100 flow: Right
                    View{ width: Fill height: Fill }
                    View{ width: Fill height: 7 }
                    View{ width: 100 height: Fill }
                }
                View{ width: Fill height: 100 flow: Right
                    View{ width: Fill height: Fill }
                    Tip{ text: "grip" View{ width: Fill height: 7 } }
                    View{ width: 100 height: Fill }
                }
            }
        });
        let mut target = Target::new(&mut cx, dvec2(400.0, 200.0));
        let compare = |cx: &Cx, root: &View| {
            for (bare, tipped) in [(vec![0, 0], vec![1, 0]), (vec![0, 1], vec![1, 1, 0]), (vec![0, 2], vec![1, 2])] {
                assert_eq!(
                    below(rect(cx, &at(root, &bare)), 100.0),
                    rect(cx, &at(root, &tipped)),
                    "{bare:?} against {tipped:?}"
                );
            }
        };
        target.draw(&mut cx, &mut root);
        compare(&cx, &root);

        for grip in [at(&root, &[0, 1]), at(&root, &[1, 1, 0])] {
            let mut grip = grip.borrow_mut::<View>().unwrap();
            grip.walk.width = Size::Fixed(7.0);
            grip.walk.height = Size::fill();
        }
        target.draw(&mut cx, &mut root);
        compare(&cx, &root);
        let grip = rect(&cx, &at(&root, &[1, 1, 0]));
        assert_eq!(grip.size, dvec2(7.0, 100.0), "the turned grip is full height");
        assert_eq!(rect(&cx, &at(&root, &[1, 0])).size.x, 293.0, "the Fill beside it takes the rest");
        });
    }

    /// A wrapper around a hidden control is hidden itself, so its row closes
    /// up exactly as it does for the bare hidden control.
    #[test]
    fn a_hidden_control_leaves_no_gap_for_its_tip() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut root = view(&mut cx, script! {
            use mod.prelude.widgets.*
            View{ width: 300 height: 60 flow: Down
                View{ width: Fill height: 30 flow: Right spacing: 10
                    View{ visible: false width: Fill height: 20 }
                    View{ width: 50 height: 20 }
                }
                View{ width: Fill height: 30 flow: Right spacing: 10
                    Tip{ text: "hidden" View{ visible: false width: Fill height: 20 } }
                    View{ width: 50 height: 20 }
                }
            }
        });
        assert!(!at(&root, &[1, 0]).visible());
        let mut target = Target::new(&mut cx, dvec2(300.0, 60.0));
        target.draw(&mut cx, &mut root);
        assert_eq!(below(rect(&cx, &at(&root, &[0, 1])), 30.0), rect(&cx, &at(&root, &[1, 1])));
        });
    }

    /// A control that yields mid-draw hands its step up through the wrapper
    /// and is resumed through it, finishing where the bare control would.
    #[test]
    fn a_yielding_control_resumes_through_its_tip() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let mut root = view(&mut cx, script! {
            use mod.prelude.widgets.*
            View{ width: 200 height: 20 flow: Right
                Tip{ text: "step" TurtleStep{ width: 30 height: 20 } }
                View{ width: 40 height: 20 }
            }
        });
        let target = Target::new(&mut cx, dvec2(200.0, 20.0));
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(&mut cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        let mut draw_list = target.draw_list;
        cx2d.begin_pass(&target.pass, None);
        draw_list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(target.size, Layout::flow_overlay());
        let walk = root.walk;
        assert!(root.draw_walk(&mut cx2d, &mut Scope::empty(), walk).is_step());
        assert!(root.draw_walk(&mut cx2d, &mut Scope::empty(), walk).is_done());
        cx2d.end_pass_sized_turtle();
        draw_list.end(&mut cx2d);
        cx2d.end_pass(&target.pass);
        drop(cx2d);
        drop(draw);
        assert_eq!(rect(&cx, &at(&root, &[1])).pos.x, 30.0);
        });
    }

    fn tip_layer(cx: &mut Cx) -> TipLayer {
        cx.with_vm(|vm| {
            let value = vm.eval(script! {
                use mod.prelude.widgets.*
                TipLayer{}
            });
            TipLayer::script_from_value(vm, value)
        })
    }

    /// The tip the layer holds, pending or on screen.
    fn held(layer: &TipLayer) -> Option<String> {
        layer.pending.as_ref().or(layer.showing.as_ref()).map(|tip| tip.text.clone())
    }

    /// Hands `event` to `root` and to `layer` the way a window does, then
    /// the reports the tree made, and says which tip the layer then holds.
    fn deliver(cx: &mut Cx, root: &mut View, layer: &mut TipLayer, event: &Event) -> Option<String> {
        let actions = cx.capture_actions(|cx| root.handle_event(cx, event, &mut Scope::empty()));
        layer.handle_event(cx, event, &mut Scope::empty());
        layer.handle_event(cx, &Event::Actions(actions), &mut Scope::empty());
        held(layer)
    }

    /// Lets a key focus asked for during the last dispatch change hands the
    /// way the platform does, once actions drain, and reports the change.
    fn settle_focus(cx: &mut Cx, root: &mut View, layer: &mut TipLayer) -> Option<String> {
        let prev = cx.key_focus();
        cx.action(());
        cx.handle_actions();
        let focus = cx.key_focus();
        if focus != prev {
            deliver(cx, root, layer, &Event::KeyFocus(KeyFocusEvent { prev, focus }));
        }
        held(layer)
    }

    /// Moves the key focus to `to` the way the platform does, lets `root`
    /// report it, hands the reports to `layer`, and says which tip the
    /// layer then holds, pending or on screen.
    fn focus(cx: &mut Cx, root: &mut View, layer: &mut TipLayer, to: Area) -> Option<String> {
        cx.set_key_focus(to);
        let held = settle_focus(cx, root, layer);
        assert_eq!(cx.key_focus(), to);
        held
    }

    fn middle(cx: &Cx, widget: &WidgetRef) -> DVec2 {
        let rect = rect(cx, widget);
        rect.pos + rect.size * 0.5
    }

    /// Tabbing along a row of tipped controls shows each control's tip in
    /// turn, forward as well as back. A row visits its last child first, so
    /// moving forward the newcomer reports before the control it left, and
    /// that control's HoverOut must not take the new tip down. Nor may a
    /// control losing focus take down a tip somebody else raised.
    #[test]
    fn focus_moving_along_tipped_controls_keeps_the_tip_it_reached() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        // The layer times its reveal against the app clock.
        cx.init_cx_os();
        let mut root = view(&mut cx, script! {
            use mod.prelude.widgets.*
            View{ width: 300 height: 20 flow: Right
                Tip{ text: "first" View{ width: 50 height: 20 } }
                Tip{ text: "second" View{ width: 50 height: 20 } }
                Tip{ text: "third" View{ width: 50 height: 20 } }
            }
        });
        let mut layer = tip_layer(&mut cx);
        let mut target = Target::new(&mut cx, dvec2(300.0, 20.0));
        target.draw(&mut cx, &mut root);
        let control = |root: &View, index: usize| at(root, &[index]).area();

        let to = control(&root, 0);
        assert_eq!(focus(&mut cx, &mut root, &mut layer, to).as_deref(), Some("first"));
        let to = control(&root, 1);
        assert_eq!(focus(&mut cx, &mut root, &mut layer, to).as_deref(), Some("second"), "forward");
        let to = control(&root, 2);
        assert_eq!(focus(&mut cx, &mut root, &mut layer, to).as_deref(), Some("third"), "forward again");
        let to = control(&root, 0);
        assert_eq!(focus(&mut cx, &mut root, &mut layer, to).as_deref(), Some("first"), "back");
        assert_eq!(focus(&mut cx, &mut root, &mut layer, Area::Empty), None, "focus gone, tip gone");

        // A host raises a tip of its own while a tipped control holds focus.
        let to = control(&root, 1);
        focus(&mut cx, &mut root, &mut layer, to);
        let host = root.widget_uid();
        let actions = cx.capture_actions(|cx| {
            cx.widget_action(host, TipAction::HoverIn("host".to_string(), Rect::default()))
        });
        layer.handle_event(&mut cx, &Event::Actions(actions), &mut Scope::empty());
        assert_eq!(focus(&mut cx, &mut root, &mut layer, Area::Empty).as_deref(), Some("host"));
        });
    }

    /// A tip the key focus raised is there to say what the control is. Once
    /// the control is typed into, edited or activated from the keyboard, the
    /// reader knows, and the tip goes rather than sit over whatever the
    /// typing is for. It stays gone until the focus moves again. Keys that
    /// only move about leave it, and a tip the pointer raised over another
    /// control is not the typing's to take down.
    #[test]
    fn typing_into_a_control_takes_down_the_tip_its_focus_raised() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        cx.init_cx_os();
        let mut root = view(&mut cx, script! {
            use mod.prelude.widgets.*
            View{ width: 300 height: 20 flow: Right
                Tip{ text: "search" View{ width: 50 height: 20 } }
                Tip{ text: "sync" View{ width: 50 height: 20 } }
                Tip{ text: "pointer" Button{ text: "p" width: 50 height: 20 } }
            }
        });
        let mut layer = tip_layer(&mut cx);
        let mut target = Target::new(&mut cx, dvec2(300.0, 20.0));
        target.draw(&mut cx, &mut root);
        let control = |root: &View, index: usize| at(root, &[index]).area();
        let text = |input: &str| {
            Event::TextInput(TextInputEvent { input: input.to_string(), ..Default::default() })
        };
        let key = |key_code| {
            Event::KeyDown(KeyEvent { key_code, is_repeat: false, modifiers: KeyModifiers::default(), time: 0.0 })
        };

        let to = control(&root, 0);
        assert_eq!(focus(&mut cx, &mut root, &mut layer, to).as_deref(), Some("search"));
        assert_eq!(
            deliver(&mut cx, &mut root, &mut layer, &key(KeyCode::ArrowLeft)).as_deref(),
            Some("search"),
            "a caret moving is not typing"
        );
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &text("a")), None, "typed into");
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &key(KeyCode::ArrowLeft)), None, "and it stays gone");
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &text("b")), None);

        // Each key that edits or activates does the same, from a tip the
        // focus has just raised afresh.
        for (index, key_code) in [KeyCode::Backspace, KeyCode::Delete, KeyCode::ReturnKey, KeyCode::Space]
            .into_iter()
            .enumerate()
        {
            let to = control(&root, (index + 1) % 2);
            let raised = focus(&mut cx, &mut root, &mut layer, to);
            assert!(raised.is_some(), "focus moving brings a tip back before {key_code:?}");
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &key(key_code)), None, "{key_code:?}");
        }

        // The focus moves on, the pointer comes to rest on another control,
        // and the typing goes on in the focused one.
        let to = control(&root, 1);
        assert_eq!(focus(&mut cx, &mut root, &mut layer, to).as_deref(), Some("sync"));
        let over = Event::MouseMove(MouseMoveEvent {
            abs: middle(&cx, &at(&root, &[2])),
            lock_delta: DVec2::default(),
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            time: 0.0,
            handled: std::cell::Cell::new(Area::Empty),
        });
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &over).as_deref(), Some("pointer"));
        assert_eq!(
            deliver(&mut cx, &mut root, &mut layer, &text("c")).as_deref(),
            Some("pointer"),
            "typing elsewhere leaves the pointer's tip"
        );
        });
    }

    /// A finger has no hover. A tap hands a tipped button the key focus, and
    /// that focus is the press's, not a keyboard arrival: no tip, neither on
    /// the press nor after it. A tip already up goes the moment a finger
    /// lands, as it does under a mouse press. The tip a finger can have is
    /// the long press's: it shows at once while the finger is held and goes
    /// when the finger lifts.
    #[test]
    fn a_finger_shows_a_tip_only_while_a_long_press_holds_it() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        cx.init_cx_os();
        let mut root = view(&mut cx, script! {
            use mod.prelude.widgets.*
            View{ width: 300 height: 20 flow: Right
                Tip{ text: "other" View{ width: 50 height: 20 } }
                Tip{ text: "tap" Button{ text: "t" width: 50 height: 20 } }
            }
        });
        let mut layer = tip_layer(&mut cx);
        let mut target = Target::new(&mut cx, dvec2(300.0, 20.0));
        target.draw(&mut cx, &mut root);
        let button = at(&root, &[1]);
        let abs = middle(&cx, &button);
        let touch = |uid: u64, state: TouchState| {
            Event::TouchUpdate(TouchUpdateEvent {
                time: 0.0,
                window_id: WindowId(1, 1),
                modifiers: KeyModifiers::default(),
                touches: vec![TouchPoint {
                    state,
                    abs,
                    time: 0.0,
                    uid,
                    rotation_angle: 0.0,
                    force: 1.0,
                    radius: dvec2(1.0, 1.0),
                    handled: std::cell::Cell::new(Area::Empty),
                    sweep_lock: std::cell::Cell::new(Area::Empty),
                }],
            })
        };

        // A tap: the finger lands, the button takes the focus, the finger lifts.
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &touch(1, TouchState::Start)), None);
        assert_eq!(settle_focus(&mut cx, &mut root, &mut layer), None, "the tap's focus raises no tip");
        assert!(cx.has_key_focus(button.area()), "the tap did hand the button the focus");
        assert_eq!(
            deliver(&mut cx, &mut root, &mut layer, &touch(1, TouchState::Stop)),
            None,
            "nor does the tap leave one"
        );

        // A tip up when a finger lands goes with the press.
        let to = at(&root, &[0]).area();
        assert_eq!(focus(&mut cx, &mut root, &mut layer, to).as_deref(), Some("other"));
        assert_eq!(
            deliver(&mut cx, &mut root, &mut layer, &touch(2, TouchState::Start)),
            None,
            "a finger landing takes a tip down"
        );
        assert_eq!(settle_focus(&mut cx, &mut root, &mut layer), None);
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &touch(2, TouchState::Stop)), None);

        // A long press.
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &touch(3, TouchState::Start)), None);
        assert_eq!(settle_focus(&mut cx, &mut root, &mut layer), None);
        let long_press = |uid: u64| {
            Event::LongPress(LongPressEvent { window_id: WindowId(1, 1), abs, uid, time: 0.5 })
        };
        assert_eq!(
            deliver(&mut cx, &mut root, &mut layer, &long_press(9)),
            None,
            "another finger's long press"
        );
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &long_press(3)).as_deref(), Some("tap"));
        assert_eq!(
            layer.showing.as_ref().map(|tip| tip.text.as_str()),
            Some("tap"),
            "the hold was the dwell: on screen now"
        );
        assert_eq!(
            deliver(&mut cx, &mut root, &mut layer, &touch(3, TouchState::Stable)).as_deref(),
            Some("tap"),
            "while the finger is held"
        );
        assert_eq!(
            deliver(&mut cx, &mut root, &mut layer, &touch(3, TouchState::Stop)),
            None,
            "lifted, gone"
        );
        });
    }

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect {
            pos: dvec2(x, y),
            size: dvec2(w, h),
        }
    }

    /// Hang a bubble with a pointer off `anchor` in an 800x600 pass, and
    /// give back where it went, the side it took, and its pointer's point,
    /// window-absolute.
    fn hang(anchor: Rect, size: DVec2, place: TipPlace) -> (Rect, Side, DVec2) {
        let (placed, pointer) = hang_bubble(anchor, size, dvec2(800.0, 600.0), place, true);
        (placed.rect, placed.side, placed.rect.pos + pointer.unwrap().tip)
    }

    #[test]
    fn tip_pointer_aims_at_the_control_from_below_and_above() {
        crate::on_test_cx(|| {
        let anchor = r(300.0, 100.0, 80.0, 24.0);
        let (_, side, point) = hang(anchor, dvec2(120.0, 24.0), TipPlace::Bottom);
        assert_eq!(side, Side::Bottom);
        assert_eq!(point, dvec2(340.0, 124.0 + TIP_GAP - TIP_ARROW + 0.5));
        let (_, side, point) = hang(anchor, dvec2(120.0, 24.0), TipPlace::Top);
        assert_eq!(side, Side::Top);
        assert_eq!(point, dvec2(340.0, 100.0 - TIP_GAP + TIP_ARROW - 0.5));
        });
    }

    #[test]
    fn tip_pointer_turns_a_quarter_turn_beside_the_control() {
        crate::on_test_cx(|| {
        // A one-line bubble is too short for its corners and a pointer
        // between them, so a side pointer sits at the middle of the edge.
        let anchor = r(300.0, 100.0, 80.0, 24.0);
        let (_, side, point) = hang(anchor, dvec2(100.0, 24.0), TipPlace::Left);
        assert_eq!(side, Side::Left);
        assert_eq!(point, dvec2(300.0 - TIP_GAP + TIP_ARROW - 0.5, 112.0));
        let (_, side, point) = hang(anchor, dvec2(100.0, 24.0), TipPlace::Right);
        assert_eq!(side, Side::Right);
        assert_eq!(point, dvec2(380.0 + TIP_GAP - TIP_ARROW + 0.5, 112.0));
        });
    }

    #[test]
    fn tip_pointer_slides_along_the_bubble_the_window_pushed() {
        crate::on_test_cx(|| {
        // Too near the window's left edge to centre: the bubble is pushed
        // right and the pointer still lands under the control's middle.
        let anchor = r(0.0, 100.0, 40.0, 24.0);
        let (rect, _, point) = hang(anchor, dvec2(120.0, 24.0), TipPlace::Bottom);
        assert_eq!(rect.pos.x, 2.0);
        assert_eq!(point.x, 20.0);
        // With the control's middle off the straight part of the edge, the
        // pointer stops where the corner starts.
        let anchor = r(0.0, 100.0, 10.0, 24.0);
        let (rect, _, point) = hang(anchor, dvec2(120.0, 24.0), TipPlace::Bottom);
        assert_eq!(point.x, rect.pos.x + 0.5 + TIP_BUBBLE_R + TIP_ARROW);
        });
    }

    #[test]
    fn tip_pointer_stays_off_the_corner_of_a_short_tip_on_a_wide_control() {
        crate::on_test_cx(|| {
        let anchor = r(100.0, 100.0, 260.0, 24.0);
        let (rect, _, point) = hang(anchor, dvec2(60.0, 24.0), TipPlace::BottomStart);
        assert_eq!(rect.pos.x, 100.0);
        assert_eq!(point.x, 160.0 - 0.5 - TIP_BUBBLE_R - TIP_ARROW);
        });
    }

    #[test]
    fn tip_beside_a_small_control_moves_to_aim_at_its_middle() {
        crate::on_test_cx(|| {
        // Lined up with the top of a control shorter than the bubble, the
        // pointer's middle-of-the-edge would be below the control's middle:
        // the bubble moves up until the two meet.
        let anchor = r(300.0, 100.0, 16.0, 12.0);
        let (rect, side, point) = hang(anchor, dvec2(100.0, 24.0), TipPlace::LeftStart);
        assert_eq!(side, Side::Left);
        assert_eq!(point.y, anchor.center().y);
        assert!(rect.pos.y < anchor.pos.y);
        // Below a small control, lined up with its start.
        let (rect, side, point) = hang(anchor, dvec2(100.0, 24.0), TipPlace::BottomStart);
        assert_eq!(side, Side::Bottom);
        assert_eq!(point.x, anchor.center().x);
        assert!(rect.pos.x < anchor.pos.x);
        });
    }

    #[test]
    fn tip_without_a_pointer_lines_up_with_the_control() {
        crate::on_test_cx(|| {
        let anchor = r(300.0, 100.0, 16.0, 12.0);
        let (placed, pointer) = hang_bubble(anchor, dvec2(100.0, 24.0), dvec2(800.0, 600.0), TipPlace::BottomStart, false);
        assert!(pointer.is_none());
        assert_eq!(placed.rect.pos, dvec2(300.0, 112.0 + TIP_GAP));
        });
    }
}

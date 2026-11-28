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
/// How far the system pointer reaches below and to the right of its hot
/// spot, in points (the arrow and the hand are about 22 tall, 16 wide at
/// 1x): a tip raised by hovering keeps out of that box, so the cursor never
/// sits on top of the text it is asking for.
const TIP_CURSOR_BELOW: f64 = 22.0;
const TIP_CURSOR_RIGHT: f64 = 16.0;
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

/// The anchor a hover tip hangs off, grown to take in the box the mouse
/// cursor covers when the pointer is over the control: a tip below (or to
/// the right of) a control the pointer is resting on would otherwise start
/// under the cursor. A pointer elsewhere (a tip raised by the keyboard)
/// leaves the anchor as it is.
fn clear_of_cursor(anchor: Rect, pointer: Option<DVec2>, place: TipPlace) -> Rect {
    let below = matches!(place, TipPlace::Bottom | TipPlace::BottomStart | TipPlace::BottomEnd);
    let right = matches!(place, TipPlace::Right | TipPlace::RightStart | TipPlace::RightEnd);
    rect_clear_of_cursor(anchor, pointer, below, right)
}

/// `anchor` grown by the box the cursor covers below (`below`) or to the
/// right of (`right`) the pointer, when the pointer is over it. Shared by
/// every hover tooltip.
pub(crate) fn rect_clear_of_cursor(anchor: Rect, pointer: Option<DVec2>, below: bool, right: bool) -> Rect {
    let Some(p) = pointer else { return anchor };
    let over = p.x >= anchor.pos.x - 1.0
        && p.x <= anchor.pos.x + anchor.size.x + 1.0
        && p.y >= anchor.pos.y - 1.0
        && p.y <= anchor.pos.y + anchor.size.y + 1.0;
    if !over {
        return anchor;
    }
    let mut rect = anchor;
    if below {
        let bottom = (p.y + TIP_CURSOR_BELOW - TIP_GAP).max(anchor.pos.y + anchor.size.y);
        rect.size.y = bottom - anchor.pos.y;
    }
    if right {
        let right = (p.x + TIP_CURSOR_RIGHT - TIP_GAP).max(anchor.pos.x + anchor.size.x);
        rect.size.x = right - anchor.pos.x;
    }
    rect
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
    /// Whether this wrapper currently has a raised focus tip (unsuppressed
    /// key focus arrived while non-empty and visible): a tip that only
    /// answers the pointer is invisible to someone working by keyboard.
    #[rust]
    focused: bool,
    /// A mouse press, finger press or accessibility activation reached the
    /// wrapped control. Suppress the tip for the focus that press hands over.
    #[rust]
    pressed: bool,
    /// The finger that came down on the wrapped child and is still down.
    #[rust]
    finger: Option<u64>,
    /// Whether a long press on `finger` currently has a raised tip.
    #[rust]
    long_pressed: bool,
    /// Whether a press took the tip down and the pointer has not left the child since.
    #[rust]
    spent: bool,
    /// The text changed while this tip was raised: the next finished draw
    /// reports it again, so the tip on screen follows its words.
    #[rust]
    refresh_request: bool,
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
        if !visible && self.view.visible {
            // A hidden wrapper may receive no further pointer or focus events.
            self.reset_hidden(cx, self.widget_uid());
        }
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
            if visible && self.area().is_empty() && !cx.is_in_draw_walk {
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
    fn search_scope(&self) -> bool {
        self.view.search_scope()
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

    /// Takes a raised tip down: a press on the control, or nothing left to say.
    fn drop_tip(&mut self, cx: &mut Cx, uid: WidgetUid) {
        if self.hovered || self.focused || self.long_pressed {
            self.hovered = false;
            self.focused = false;
            self.long_pressed = false;
            cx.widget_action(uid, TipAction::HoverOut);
        }
    }

    fn has_active_state(&self) -> bool {
        self.hovered
            || self.focused
            || self.long_pressed
            || self.finger.is_some()
            || self.pressed
            || self.spent
    }

    /// Clears all live interaction state when the wrapper or its control hides,
    /// dropping any raised tip once.
    fn reset_hidden(&mut self, cx: &mut Cx, uid: WidgetUid) {
        self.finger = None;
        self.drop_tip(cx, uid);
        self.spent = false;
        self.pressed = false;
        self.refresh_request = false;
    }

    /// The control holding the key focus was typed into, edited, or
    /// activated from the keyboard. A tip its focus raised has said what the
    /// control is by now, and goes rather than sit over what the typing is
    /// for. Only focus arriving raises one, so it stays gone until the focus
    /// moves again. A tip the pointer raised is the pointer's to take down,
    /// and the owner rule keeps one raised over another control up.
    fn typed(&mut self, cx: &mut Cx) {
        let area = self.area();
        if self.focused && !self.hovered && !area.is_empty() && cx.has_key_focus(area) {
            self.drop_tip(cx, self.widget_uid());
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
            KeyCode::KeyZ => key.modifiers.is_edit_primary(),
            _ => false,
        }
    }

    /// Accessibility actions that activate or edit a control, like a pointer press
    /// or keyboard activation/edit (as against `FOCUS`, `SCROLL_INTO_VIEW`, or
    /// `SET_TEXT_SELECTION`).
    fn a11y_activates(action: a11y_tree::Actions) -> bool {
        use a11y_tree::Actions as A;
        action.intersects(
            A::PRESS
                | A::TOGGLE
                | A::SELECT
                | A::EXPAND
                | A::COLLAPSE
                | A::SHOW_MENU
                | A::INCREMENT
                | A::DECREMENT
                | A::SET_VALUE
                | A::REPLACE_TEXT,
        )
    }

    /// Whether `node` (resolved to its owning widget uid) is this `Tip` or any
    /// widget inside its wrapped subtree.
    fn owns_a11y_node(&self, cx: &Cx, node: a11y_tree::NodeId) -> bool {
        let target = WidgetUid(cx.a11y.action_owner(node).uid());
        if target == WidgetUid(0) {
            return false;
        }
        if self.widget_uid() == target {
            return true;
        }
        fn walk(widget: &WidgetRef, target: WidgetUid) -> bool {
            if widget.try_widget_uid() == Some(target) {
                return true;
            }
            let mut found = false;
            widget.try_children(&mut |_, child| {
                if !found && walk(&child, target) {
                    found = true;
                }
            });
            found
        }
        self.view.children.iter().any(|(_, child)| walk(child, target))
    }

    /// Whether the wrapped control currently holds key focus or has a pending
    /// focus request queued during the current event.
    fn focus_pending_or_held(&self, cx: &Cx) -> bool {
        let area = self.area();
        !area.is_empty() && (cx.has_key_focus(area) || cx.next_key_focus() == area)
    }

    fn reports_tip(&self) -> bool {
        self.hovered || self.focused || self.long_pressed
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
    fn semantics(&self, cx: &Cx, node: &mut a11y_tree::Node) {
        let _ = cx;
        node.role = a11y_tree::Role::Tooltip;
        // the tip describes the control it wraps (its own `text`: the Widget trait's `text()`
        // is empty here); the control keeps its name
        makepad_platform::a11y::set_string(&mut node.description, &self.text);
    }

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
        let step = match self.sole_child() {
            Some(child) => child.draw_walk(cx, scope, walk),
            None => self.view.draw_walk(cx, scope, walk),
        };
        if self.refresh_request && step.is_done() {
            self.refresh_request = false;
            if self.reports_tip() {
                if self.text.is_empty() {
                    self.drop_tip(cx, self.widget_uid());
                } else {
                    let request = self.request(cx);
                    cx.widget_action(self.widget_uid(), TipAction::HoverInWith(request));
                }
            }
        }
        step
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
        let uid = self.widget_uid();
        // A hidden wrapper accepts no focus, press, hover, or touch state.
        if !self.view.visible {
            self.reset_hidden(cx, uid);
            return;
        }
        // A control that went away takes its tip with it: the pointer may never move again over
        // it, and a key focus stays on an area nothing draws any more. So does one a hidden
        // ancestor took out of sight (a row removed with its own button): no pointer event
        // reaches it there, only actions, which say so.
        let active = self.has_active_state();
        if active && (self.children_all_hidden() || crate::held_events::in_hidden_subtree(cx)) {
            self.reset_hidden(cx, uid);
            return;
        }
        // A raised tip whose text went empty has nothing left to say, so take it down.
        // Keep press, spend and finger bookkeeping running below while text stays empty,
        // without setting `hovered`/`focused`/`long_pressed` or emitting any tip actions for an empty tip.
        let empty = self.text.is_empty();
        if empty {
            self.drop_tip(cx, uid);
        }
        let claimed = || {
            let claimed_after = event.pointer_claimed_area();
            claimed_after != claimed_before && !claimed_after.is_empty()
        };
        match event {
            Event::MouseMove(_) => {
                let over = claimed();
                if !over {
                    self.spent = false;
                } else if !active && self.children_all_hidden() {
                    return;
                }
                if empty {
                    return;
                }
                // A press spent the tip: it stays down until the pointer leaves and returns.
                let over = over && !self.spent;
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
                self.spent = false;
                if self.hovered {
                    self.hovered = false;
                    cx.widget_action(uid, TipAction::HoverOut);
                }
            }
            // A pressed control drops its tip, and the tip stays down while the pointer stays.
            Event::MouseDown(_) => {
                if claimed() {
                    if !active && self.children_all_hidden() {
                        return;
                    }
                    self.pressed = true;
                    self.spent = true;
                    self.drop_tip(cx, uid);
                } else {
                    self.pressed = false;
                }
            }
            Event::MouseUp(_) => {
                if self.pressed && self.finger.is_none() && !self.focus_pending_or_held(cx) {
                    self.pressed = false;
                }
            }
            // A finger landing raises nothing, not even through the focus it
            // hands the control: with no hover there is nothing to take such
            // a tip down, and it would stand over every control tapped.
            Event::TouchUpdate(touch) => {
                if let Some(start) = touch.touches.iter().find(|t| t.state == TouchState::Start) {
                    if claimed() {
                        if !active && self.children_all_hidden() {
                            return;
                        }
                        self.pressed = true;
                        self.finger = Some(start.uid);
                        self.drop_tip(cx, uid);
                    } else if self.finger.is_none() {
                        self.pressed = false;
                    }
                }
                if let Some(finger) = self.finger {
                    if touch
                        .touches
                        .iter()
                        .any(|t| t.uid == finger && matches!(t.state, TouchState::Stop | TouchState::Cancel))
                    {
                        self.finger = None;
                        if std::mem::take(&mut self.long_pressed) && !self.hovered && !self.focused {
                            cx.widget_action(uid, TipAction::HoverOut);
                        }
                        if !self.focus_pending_or_held(cx) {
                            self.pressed = false;
                        }
                    }
                }
            }
            Event::FingerCancel(cancel) => {
                // Match touch cancels by `digit_id`, never `device`: `CxFingers::queue_cancel`
                // stamps `DigitDevice::Touch { uid: 0 }` on gesture-claim and row-recycle cancels.
                if self.finger.is_some_and(|f| cancel.digit_id == live_id_num!(touch, f).into()) {
                    self.finger = None;
                    if std::mem::take(&mut self.long_pressed) && !self.hovered && !self.focused {
                        cx.widget_action(uid, TipAction::HoverOut);
                    }
                    if !self.focus_pending_or_held(cx) {
                        self.pressed = false;
                    }
                } else if self.pressed && self.finger.is_none() && cancel.digit_id == live_id!(mouse).into() {
                    if !self.focus_pending_or_held(cx) {
                        self.pressed = false;
                    }
                }
            }
            // Holding a finger on the control is how touch asks what it is.
            // The hold was the dwell, so the tip shows at once, and lifting
            // the finger takes it down again.
            Event::LongPress(press) if !empty && self.finger == Some(press.uid) => {
                self.long_pressed = true;
                let mut request = self.request(cx);
                request.delay_secs = 0.0;
                cx.widget_action(uid, TipAction::HoverInWith(request));
            }
            // Reaching a control by keyboard shows its tip too: the pointer
            // is not the only way in, and a tip nobody can reach by Tab is
            // a tip half the users never see.
            Event::KeyFocus(_) | Event::KeyFocusLost(_) => {
                if empty && !self.pressed {
                    return;
                }
                // An area that was never drawn is empty, and so is the key
                // focus when nothing holds it: without the guard every such
                // wrapper would count itself focused the moment focus left.
                let area = self.area();
                let focused = !area.is_empty() && cx.has_key_focus(area);
                if focused && !active && self.children_all_hidden() {
                    return;
                }
                if !focused && self.finger.is_none() {
                    self.pressed = false;
                }
                if empty {
                    return;
                }
                let show_for_focus = focused && !self.pressed;
                if show_for_focus != self.focused {
                    self.focused = show_for_focus;
                    // Focus a press handed over came by the pointer or an a11y activation.
                    if show_for_focus && !self.hovered {
                        cx.widget_action(uid, TipAction::HoverInWith(self.request(cx)));
                    } else if !show_for_focus && !self.hovered && !self.long_pressed {
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
            Event::A11yAction(request) if Self::a11y_activates(request.action) => {
                let hit = self.owns_a11y_node(cx, request.node);
                if hit {
                    if !active && self.children_all_hidden() {
                        return;
                    }
                    self.spent |= self.hovered;
                    self.drop_tip(cx, uid);
                    self.pressed = self.focus_pending_or_held(cx);
                } else {
                    self.pressed = false;
                }
            }
            Event::A11yAction(request) if request.action == a11y_tree::Actions::FOCUS => {
                self.pressed = false;
            }
            _ => {}
        }
    }
}

impl TipRef {
    /// State-aware tips ("Open output window" / "Close output window").
    pub fn set_text(&self, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.text != text {
                text.clone_into(&mut inner.text);
                inner.refresh_request = inner.reports_tip();
            }
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
    /// Where the mouse pointer last was, and in which window: a hover tip
    /// is placed clear of the cursor drawn there.
    #[rust]
    pointer: Option<(WindowId, DVec2)>,
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
    /// The text of the tip pending or on screen.
    #[cfg(test)]
    pub(crate) fn held_text(&self) -> Option<String> {
        self.pending.as_ref().or(self.showing.as_ref()).map(|tip| tip.text.clone())
    }

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
    fn semantics(&self, cx: &Cx, node: &mut a11y_tree::Node) {
        let _ = cx;
        node.role = a11y_tree::Role::Tooltip;
        // the tip on screen, by its text (the Widget trait's `text()` is empty here); nothing
        // shown, no name
        makepad_platform::a11y::set_string(&mut node.name, self.showing.as_ref().map_or("", |tip| tip.text.as_str()));
    }

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
        draw_list.begin_popup_overlay(cx);
        let pass = cx.current_pass_size();
        cx.begin_root_turtle(pass, Layout::flow_down());
        let font_size = 9.0f64;
        // MEASURE the run rather than guess it: a per-character estimate
        // under-measures wide glyphs (all-caps text most of all) and the
        // bubble then clips its own last letter. The estimate stays only as
        // the fallback for text the layout engine returns no row for.
        // Line by line: a tip may carry its own line breaks (a key's
        // bindings, one per line), and the bubble is as wide as the widest
        // of them and as tall as all of them.
        let mut text_w = 0.0f64;
        let mut breaks = 0.0f64;
        for line in text.lines() {
            let line_w = self
                .draw_text
                .prepare_single_line_run(cx, line)
                .map(|run| run.width_in_lpxs as f64)
                .unwrap_or_else(|| line.chars().count() as f64 * font_size * 0.62);
            text_w = text_w.max(line_w);
            breaks += 1.0;
        }
        let breaks = breaks.max(1.0);
        // A wrapped tip is as wide as it was told and as tall as the lines
        // it needs; an unwrapped one is as tall as its own lines, one as
        // it always was.
        let (w, lines) = if tip.wrap_width > 0.0 && text_w + TIP_PAD_X * 2.0 > tip.wrap_width {
            let room = (tip.wrap_width - TIP_PAD_X * 2.0).max(font_size * 4.0);
            (tip.wrap_width, (text_w / room).ceil().max(1.0) * breaks)
        } else {
            (text_w + TIP_PAD_X * 2.0 + 4.0, breaks)
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
        let pointer_here = self
            .pointer
            .filter(|(window, _)| Some(*window) == cx.get_current_window_id())
            .map(|(_, pos)| pos);
        let anchor = clear_of_cursor(anchor, pointer_here, tip.place);
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
        if let Event::MouseMove(e) = event {
            self.pointer = Some((e.window_id, e.abs));
        }
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
                    if self.owner == Some(action.widget_uid) {
                        if self.showing.is_some() {
                            self.showing = Some(request);
                            self.redraw_layer(cx);
                        } else {
                            self.pending = Some(request);
                        }
                        continue;
                    }
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
    use crate::makepad_draw::event::{DigitDevice, FingerCancelEvent, LongPressEvent, TouchPoint, TouchState, TouchUpdateEvent};
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
        deliver_actions(cx, root, layer, event);
        held(layer)
    }

    fn deliver_actions(cx: &mut Cx, root: &mut View, layer: &mut TipLayer, event: &Event) -> ActionsBuf {
        let reports = Event::Actions(cx.capture_actions(|cx| root.handle_event(cx, event, &mut Scope::empty())));
        layer.handle_event(cx, event, &mut Scope::empty());
        layer.handle_event(cx, &reports, &mut Scope::empty());
        let Event::Actions(actions) = reports else { unreachable!() };
        actions
    }

    /// Lets a key focus asked for during the last dispatch change hands the
    /// way the platform does, once actions drain, and reports the change.
    fn settle_focus(cx: &mut Cx, root: &mut View, layer: &mut TipLayer) -> Option<String> {
        settle_focus_actions(cx, root, layer);
        held(layer)
    }

    fn settle_focus_actions(cx: &mut Cx, root: &mut View, layer: &mut TipLayer) -> ActionsBuf {
        let prev = cx.key_focus();
        cx.action(());
        cx.handle_actions();
        let focus = cx.key_focus();
        if focus != prev {
            deliver_actions(cx, root, layer, &Event::KeyFocus(KeyFocusEvent { prev, focus }))
        } else {
            Vec::new()
        }
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

    /// A control that hides with its tip up takes the tip with it, on the next event of any
    /// kind: nothing else would, with the pointer still or the key focus left behind on it. A
    /// control whose row hides does too, on the next actions.
    #[test]
    fn a_control_that_hides_takes_its_tip_with_it() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        cx.init_cx_os();
        let mut root = view(&mut cx, script! {
            use mod.prelude.widgets.*
            View{ width: 300 height: 20 flow: Right
                Tip{ text: "first" View{ width: 50 height: 20 } }
                View{ width: 100 height: 20 Tip{ text: "in a row" View{ width: 50 height: 20 } } }
            }
        });
        let mut layer = tip_layer(&mut cx);
        let mut target = Target::new(&mut cx, dvec2(300.0, 20.0));
        target.draw(&mut cx, &mut root);
        let to = at(&root, &[0]).area();
        assert_eq!(focus(&mut cx, &mut root, &mut layer, to).as_deref(), Some("first"));
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &Event::Signal).as_deref(), Some("first"), "the control is still there");
        at(&root, &[0, 0]).set_visible(&mut cx, false);
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &Event::Signal), None, "the control went, and the tip");
        // The row holding a tipped control hides (removed with its own button): the next
        // actions, all that reach a hidden subtree, take the tip down.
        let to = at(&root, &[1, 0]).area();
        assert_eq!(focus(&mut cx, &mut root, &mut layer, to).as_deref(), Some("in a row"));
        at(&root, &[1]).set_visible(&mut cx, false);
        let none = cx.capture_actions(|_| {});
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &Event::Actions(none)), None, "the row went, and the tip");
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

        // A tap: the finger lands, the finger lifts, and the button takes the focus.
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &touch(1, TouchState::Start)), None);
        assert_eq!(
            deliver(&mut cx, &mut root, &mut layer, &touch(1, TouchState::Stop)),
            None,
            "the tap leaves no tip"
        );
        assert_eq!(settle_focus(&mut cx, &mut root, &mut layer), None, "nor does the tap's focus raise one");
        assert!(cx.has_key_focus(button.area()), "the tap did hand the button the focus");

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

    /// A pressed control drops its tip, and a tip never covers live controls: the press takes it
    /// down (the tip reports it itself, not only the layer's own press rule), it stays down while
    /// the pointer stays, and it comes back once the pointer has left and returned. A raised tip
    /// whose text goes empty, or whose control hides, goes on the next event.
    #[test]
    fn a_pressed_or_emptied_or_hidden_control_drops_its_tip() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        cx.init_cx_os();
        let mut root = view(&mut cx, script! {
            use mod.prelude.widgets.*
            View{ width: 300 height: 20 flow: Right
                call_tip := Tip{ text: "call" Button{ text: "c" width: 50 height: 20 } }
                mute_tip := Tip{ text: "mute" Button{ text: "m" width: 50 height: 20 } }
                Tip{ text: "away" Button{ text: "a" width: 50 height: 20 } }
            }
        });
        let mut layer = tip_layer(&mut cx);
        let mut target = Target::new(&mut cx, dvec2(300.0, 20.0));
        target.draw(&mut cx, &mut root);
        let over = |cx: &Cx, root: &View, index: usize| {
            Event::MouseMove(MouseMoveEvent {
                abs: middle(cx, &at(root, &[index])),
                lock_delta: DVec2::default(),
                window_id: WindowId(1, 1),
                modifiers: KeyModifiers::default(),
                time: 0.0,
                handled: std::cell::Cell::new(Area::Empty),
            })
        };
        let down = |cx: &Cx, root: &View, index: usize| {
            Event::MouseDown(MouseDownEvent {
                abs: middle(cx, &at(root, &[index])),
                button: MouseButton::PRIMARY,
                window_id: WindowId(1, 1),
                modifiers: KeyModifiers::default(),
                handled: std::cell::Cell::new(Area::Empty),
                time: 0.0,
            })
        };
        let hover_outs = |cx: &mut Cx, root: &mut View, event: &Event| {
            let actions = cx.capture_actions(|cx| root.handle_event(cx, event, &mut Scope::empty()));
            actions
                .iter()
                .filter_map(|a| a.as_widget_action())
                .filter(|a| matches!(a.cast::<TipAction>(), TipAction::HoverOut))
                .count()
        };

        // The press.
        let e = over(&cx, &root, 0);
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &e).as_deref(), Some("call"));
        let update = |cx: &mut Cx, root: &mut View, target: &mut Target, layer: &mut TipLayer| {
            let actions = cx.capture_actions(|cx| target.draw(cx, root));
            layer.handle_event(cx, &Event::Actions(actions), &mut Scope::empty());
        };
        let timer = layer.timer;
        root.tip(&cx, ids!(call_tip)).set_text("pending");
        update(&mut cx, &mut root, &mut target, &mut layer);
        assert!(layer.timer.0 == timer.0 && layer.pending.as_ref().is_some_and(|t| t.text == "pending"),
            "updating the pending owner's words keeps its original dwell");
        layer.show_pending(&mut cx);
        root.tip(&cx, ids!(call_tip)).set_text("shown");
        update(&mut cx, &mut root, &mut target, &mut layer);
        assert!(layer.showing.as_ref().is_some_and(|t| t.text == "shown"), "the visible tip follows its text on the next draw without a focus or pointer move");
        root.tip(&cx, ids!(mute_tip)).set_text("away");
        update(&mut cx, &mut root, &mut target, &mut layer);
        assert!(layer.showing.as_ref().is_some_and(|t| t.text == "shown"), "changing a tip that is neither hovered nor focused leaves the visible tip alone");
        root.tip(&cx, ids!(call_tip)).set_text("call");
        root.tip(&cx, ids!(mute_tip)).set_text("mute");
        let e = down(&cx, &root, 0);
        assert_eq!(hover_outs(&mut cx, &mut root, &e), 1, "the pressed control's tip reports itself down");
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &e), None, "a press drops the tip");
        let e = over(&cx, &root, 0);
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &e), None, "the pointer staying does not raise it again");
        let e = over(&cx, &root, 1);
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &e).as_deref(), Some("mute"), "another control's tip rises");
        let e = over(&cx, &root, 0);
        assert_eq!(
            deliver(&mut cx, &mut root, &mut layer, &e).as_deref(),
            Some("call"),
            "left and back: the tip rises again"
        );

        // The text goes empty under a raised tip.
        let e = over(&cx, &root, 1);
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &e).as_deref(), Some("mute"));
        at(&root, &[1]).borrow_mut::<Tip>().unwrap().text = String::new();
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &Event::Signal), None, "emptied text drops a raised tip");

        // The control hides under a raised tip.
        let e = over(&cx, &root, 2);
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &e).as_deref(), Some("away"));
        at(&root, &[2, 0]).set_visible(&mut cx, false);
        assert_eq!(deliver(&mut cx, &mut root, &mut layer, &Event::Signal), None, "a hidden control drops its tip");
        });
    }

    fn a11y_req(node: a11y_tree::NodeId, action: a11y_tree::Actions) -> Event {
        Event::A11yAction(a11y_tree::ActionRequest { req: 1, node, action, arg: a11y_tree::ActionArg::None })
    }

    fn mouse_move(abs: DVec2) -> Event {
        Event::MouseMove(MouseMoveEvent {
            abs,
            lock_delta: DVec2::default(),
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            time: 0.0,
            handled: std::cell::Cell::new(Area::Empty),
        })
    }

    fn mouse_down(abs: DVec2, button: MouseButton) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            handled: std::cell::Cell::new(Area::Empty),
            time: 0.0,
        })
    }

    fn mouse_up(abs: DVec2, button: MouseButton) -> Event {
        Event::MouseUp(MouseUpEvent {
            abs,
            button,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            time: 0.0,
        })
    }

    fn tip_reports(actions: &Actions) -> Vec<(WidgetUid, TipAction)> {
        actions.iter().filter_map(|a| {
            let a = a.as_widget_action()?;
            Some((a.widget_uid, a.action.downcast_ref::<TipAction>()?.clone()))
        }).collect()
    }

    #[test]
    fn a11y_press_on_wrapped_control_suppresses_and_drops_its_tip() {
        use crate::button::ButtonAction;
        use a11y_tree::{Actions as A, NodeId};
        crate::on_test_cx(|| {
            let mut cx = cx();
            cx.init_cx_os();
            let mut root = view(&mut cx, script! {
                use mod.prelude.widgets.*
                View{ width: 300 height: 20 flow: Right
                    Tip{ text: "save" Button{ text: "s" width: 50 height: 20 } }
                    Tip{ text: "toggle" CheckBox{ text: "t" width: 50 height: 20 } }
                    Tip{ text: "nested" View{ width: 50 height: 20 Button{ text: "n" width: 50 height: 20 } } }
                }
            });
            let mut layer = tip_layer(&mut cx);
            let mut target = Target::new(&mut cx, dvec2(300.0, 20.0));
            target.draw(&mut cx, &mut root);
            let save_tip = at(&root, &[0]).widget_uid();
            let save = at(&root, &[0, 0]);
            let toggle_tip = at(&root, &[1]).widget_uid();
            let toggle = at(&root, &[1, 0]);
            let nested = at(&root, &[2, 0, 0]);
            let over_save = middle(&cx, &save);
            let over_nested = middle(&cx, &nested);
            let away = dvec2(280.0, 10.0);
            let press = a11y_req(NodeId::widget(save.widget_uid().0), A::PRESS);

            let actions = deliver_actions(&mut cx, &mut root, &mut layer, &press);
            assert!(actions.filter_widget_actions(save.widget_uid()).any(|a| matches!(a.cast(), ButtonAction::Clicked(_))));
            assert!(tip_reports(&actions).is_empty(), "pressing the idle button requested a tip");
            let actions = settle_focus_actions(&mut cx, &mut root, &mut layer);
            assert!(cx.has_key_focus(save.area()));
            assert!(tip_reports(&actions).is_empty(), "focus from the press requested a tip");
            assert_eq!(held(&layer), None);

            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &mouse_move(over_save)).as_deref(), Some("save"));
            let actions = deliver_actions(&mut cx, &mut root, &mut layer, &press);
            assert_eq!(tip_reports(&actions), [(save_tip, TipAction::HoverOut)]);
            assert_eq!(held(&layer), None);
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &mouse_move(over_save)), None);
            deliver(&mut cx, &mut root, &mut layer, &mouse_move(away));
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &mouse_move(over_save)).as_deref(), Some("save"));
            deliver(&mut cx, &mut root, &mut layer, &mouse_move(away));

            for action in [A::TOGGLE, A::SELECT] {
                assert_eq!(focus(&mut cx, &mut root, &mut layer, toggle.area()).as_deref(), Some("toggle"));
                let req = a11y_req(NodeId::widget(toggle.widget_uid().0), action);
                let actions = deliver_actions(&mut cx, &mut root, &mut layer, &req);
                assert_eq!(tip_reports(&actions), [(toggle_tip, TipAction::HoverOut)]);
                assert_eq!(held(&layer), None);
                focus(&mut cx, &mut root, &mut layer, Area::Empty);
            }

            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &mouse_move(over_nested)).as_deref(), Some("nested"));
            let req = a11y_req(NodeId::widget(nested.widget_uid().0), A::PRESS);
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &req), None);
            assert_eq!(settle_focus(&mut cx, &mut root, &mut layer), None);
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &mouse_move(over_save)).as_deref(), Some("save"));
            let req = a11y_req(NodeId::part(save.widget_uid().0, 1), A::PRESS);
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &req), None);
        });
    }

    #[test]
    fn mouse_press_still_drops_and_suppresses_focus_tip() {
        crate::on_test_cx(|| {
            let mut cx = cx();
            cx.init_cx_os();
            let mut root = view(&mut cx, script! {
                use mod.prelude.widgets.*
                View{ width: 100 height: 20
                    Tip{ text: "save" Button{ text: "s" width: 50 height: 20 } }
                }
            });
            let mut layer = tip_layer(&mut cx);
            let mut target = Target::new(&mut cx, dvec2(100.0, 20.0));
            target.draw(&mut cx, &mut root);
            let save = at(&root, &[0, 0]);
            let abs = middle(&cx, &save);
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &mouse_move(abs)).as_deref(), Some("save"));
            let down = Event::MouseDown(MouseDownEvent {
                abs,
                button: MouseButton::PRIMARY,
                window_id: WindowId(1, 1),
                modifiers: KeyModifiers::default(),
                handled: std::cell::Cell::new(Area::Empty),
                time: 0.0,
            });
            let actions = deliver_actions(&mut cx, &mut root, &mut layer, &down);
            assert_eq!(tip_reports(&actions), [(at(&root, &[0]).widget_uid(), TipAction::HoverOut)]);
            assert_eq!(held(&layer), None);
            let actions = settle_focus_actions(&mut cx, &mut root, &mut layer);
            assert!(cx.has_key_focus(save.area()));
            assert!(tip_reports(&actions).is_empty(), "focus from the mouse press requested a tip");
            assert_eq!(held(&layer), None);
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &mouse_move(abs)), None);
            deliver(&mut cx, &mut root, &mut layer, &mouse_move(dvec2(90.0, 10.0)));
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &mouse_move(abs)).as_deref(), Some("save"));
        });
    }

    #[test]
    fn tab_focus_without_press_still_shows_tip_including_after_a11y_press() {
        use a11y_tree::{Actions as A, NodeId};
        crate::on_test_cx(|| {
            let mut cx = cx();
            cx.init_cx_os();
            let mut root = view(&mut cx, script! {
                use mod.prelude.widgets.*
                View{ width: 100 height: 20 flow: Right
                    Tip{ text: "first" Button{ text: "1" width: 50 height: 20 } }
                    Tip{ text: "second" Button{ text: "2" width: 50 height: 20 } }
                }
            });
            let mut layer = tip_layer(&mut cx);
            let mut target = Target::new(&mut cx, dvec2(100.0, 20.0));
            target.draw(&mut cx, &mut root);
            let first_tip = at(&root, &[0]);
            let first = at(&root, &[0, 0]);
            let second = at(&root, &[1, 0]);
            let tab = Event::KeyDown(KeyEvent {
                key_code: KeyCode::Tab, is_repeat: false, modifiers: KeyModifiers::default(), time: 0.0,
            });
            let press = a11y_req(NodeId::widget(first.widget_uid().0), A::PRESS);

            deliver(&mut cx, &mut root, &mut layer, &tab);
            assert_eq!(focus(&mut cx, &mut root, &mut layer, first.area()).as_deref(), Some("first"));
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &press), None);
            assert_eq!(settle_focus(&mut cx, &mut root, &mut layer), None);
            for (button, text) in [(&second, "second"), (&first, "first")] {
                deliver(&mut cx, &mut root, &mut layer, &tab);
                assert_eq!(focus(&mut cx, &mut root, &mut layer, button.area()).as_deref(), Some(text));
            }

            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &press), None);
            assert_eq!(settle_focus(&mut cx, &mut root, &mut layer), None);
            assert!(first_tip.borrow::<Tip>().unwrap().pressed);
            for (button, text) in [(&second, "second"), (&first, "first")] {
                let req = a11y_req(NodeId::widget(button.widget_uid().0), A::FOCUS);
                deliver(&mut cx, &mut root, &mut layer, &req);
                assert!(!first_tip.borrow::<Tip>().unwrap().pressed, "FOCUS clears the press before focus moves");
                assert_eq!(focus(&mut cx, &mut root, &mut layer, button.area()).as_deref(), Some(text));
            }
        });
    }

    #[test]
    fn a11y_press_on_another_widget_does_not_affect_this_tip() {
        use a11y_tree::{Actions as A, NodeId};
        crate::on_test_cx(|| {
            let mut cx = cx();
            cx.init_cx_os();
            let mut root = view(&mut cx, script! {
                use mod.prelude.widgets.*
                View{ width: 150 height: 20 flow: Right
                    Tip{ text: "first" Button{ text: "1" width: 50 height: 20 } }
                    Tip{ text: "second" Button{ text: "2" width: 50 height: 20 } }
                    View{ width: 50 height: 20 }
                }
            });
            let mut layer = tip_layer(&mut cx);
            let mut target = Target::new(&mut cx, dvec2(150.0, 20.0));
            target.draw(&mut cx, &mut root);
            let first_tip = at(&root, &[0]);
            let first = at(&root, &[0, 0]);
            let second = at(&root, &[1, 0]);
            let bare = at(&root, &[2]);
            let over = mouse_move(middle(&cx, &first));
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &over).as_deref(), Some("first"));
            for other in [&bare, &second] {
                let req = a11y_req(NodeId::widget(other.widget_uid().0), A::PRESS);
                let actions = deliver_actions(&mut cx, &mut root, &mut layer, &req);
                assert!(tip_reports(&actions).is_empty(), "a press elsewhere emitted a tip action");
                let actions = settle_focus_actions(&mut cx, &mut root, &mut layer);
                assert!(tip_reports(&actions).is_empty(), "focus from a press elsewhere emitted a tip action");
                let tip = first_tip.borrow::<Tip>().unwrap();
                assert!(tip.hovered && !tip.spent && !tip.pressed);
                assert_eq!(held(&layer).as_deref(), Some("first"));
            }
            assert!(cx.has_key_focus(second.area()));
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &Event::ClearHover), None);
            let req = a11y_req(NodeId::widget(second.widget_uid().0), A::PRESS);
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &req), None);
            assert_eq!(focus(&mut cx, &mut root, &mut layer, first.area()).as_deref(), Some("first"));
        });
    }

    #[test]
    fn focus_leaving_while_text_is_empty_clears_press_for_later_focus() {
        use a11y_tree::{Actions as A, NodeId};
        crate::on_test_cx(|| {
            let mut cx = cx();
            cx.init_cx_os();
            let mut root = view(&mut cx, script! {
                use mod.prelude.widgets.*
                View{ width: 150 height: 20 flow: Right
                    first_tip := Tip{ text: "first" Button{ text: "1" width: 50 height: 20 } }
                    second_tip := Tip{ text: "second" Button{ text: "2" width: 50 height: 20 } }
                }
            });
            let mut layer = tip_layer(&mut cx);
            let mut target = Target::new(&mut cx, dvec2(150.0, 20.0));
            target.draw(&mut cx, &mut root);
            let first_tip = root.tip(&cx, ids!(first_tip));
            let second_tip = at(&root, &[1]).widget_uid();
            let first = at(&root, &[0, 0]);
            let second = at(&root, &[1, 0]);
            let over_first = middle(&cx, &first);
            let away = dvec2(140.0, 10.0);

            // The text empties before the accessibility press hands the button focus.
            let press = a11y_req(NodeId::widget(first.widget_uid().0), A::PRESS);
            let actions = deliver_actions(&mut cx, &mut root, &mut layer, &press);
            assert!(tip_reports(&actions).is_empty(), "pressing the idle button emitted a tip action");
            assert!(first_tip.borrow().unwrap().pressed, "the tip missed its button's accessibility press");
            assert_eq!(held(&layer), None, "the layer held a tip after the accessibility press");

            first_tip.set_text("");
            let actions = settle_focus_actions(&mut cx, &mut root, &mut layer);
            assert!(cx.has_key_focus(first.area()), "the accessibility press did not focus the first button");
            assert!(tip_reports(&actions).is_empty(), "press-driven focus emitted an empty tip action");
            assert_eq!(held(&layer), None, "press-driven focus left a tip in the layer");
            assert!(first_tip.borrow().unwrap().pressed, "the press was cleared while its button still had focus");
            assert!(!first_tip.borrow().unwrap().focused, "the empty tip recorded a focus to report");

            let actions = deliver_actions(&mut cx, &mut root, &mut layer, &mouse_move(over_first));
            assert!(tip_reports(&actions).is_empty(), "hovering the empty tip emitted a tip action");
            assert_eq!(held(&layer), None, "hovering the empty tip left a tip in the layer");
            assert!(!first_tip.borrow().unwrap().hovered, "the empty tip recorded a hover to report");

            cx.set_key_focus(second.area());
            let actions = settle_focus_actions(&mut cx, &mut root, &mut layer);
            assert!(matches!(tip_reports(&actions).as_slice(),
                [(uid, TipAction::HoverInWith(request))] if *uid == second_tip && request.text == "second"),
                "moving focus should emit only the second tip's hover-in");
            assert_eq!(held(&layer).as_deref(), Some("second"), "the layer lost the newly focused button's tip");
            assert!(!first_tip.borrow().unwrap().pressed, "focus leaving the empty tip stranded the accessibility press");

            first_tip.set_text("first");
            let draw_actions = cx.capture_actions(|cx| target.draw(cx, &mut root));
            assert!(tip_reports(&draw_actions).is_empty(), "restoring text after the accessibility press emitted a draw-time tip action");
            assert_eq!(focus(&mut cx, &mut root, &mut layer, first.area()).as_deref(), Some("first"),
                "the old accessibility press suppressed returning programmatic focus");

            // The mouse press hands over focus before the text empties.
            assert_eq!(focus(&mut cx, &mut root, &mut layer, Area::Empty), None, "clearing focus left the first tip up");
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &mouse_move(over_first)).as_deref(), Some("first"),
                "hovering before the mouse press failed to raise the first tip");
            let down = Event::MouseDown(MouseDownEvent {
                abs: over_first,
                button: MouseButton::PRIMARY,
                window_id: WindowId(1, 1),
                modifiers: KeyModifiers::default(),
                handled: std::cell::Cell::new(Area::Empty),
                time: 0.0,
            });
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &down), None, "the mouse press left the hovered tip up");
            assert!(first_tip.borrow().unwrap().pressed, "the tip missed its button's mouse press");
            assert!(first_tip.borrow().unwrap().spent, "the mouse press did not spend the hover");
            assert_eq!(settle_focus(&mut cx, &mut root, &mut layer), None, "focus from the mouse press raised the tip");
            assert!(!first_tip.borrow().unwrap().focused, "mouse-driven focus suppressed by press must not record focused");
            assert!(first_tip.borrow().unwrap().pressed, "settling mouse-driven focus cleared its press");

            first_tip.set_text("");
            deliver(&mut cx, &mut root, &mut layer, &mouse_move(away));
            assert!(!first_tip.borrow().unwrap().spent, "the hover stayed spent after the pointer left the empty tip");
            assert!(first_tip.borrow().unwrap().pressed, "the pointer leaving cleared the press before focus left");
            cx.set_key_focus(Area::Empty);
            let actions = settle_focus_actions(&mut cx, &mut root, &mut layer);
            assert!(tip_reports(&actions).is_empty(), "losing focus while empty emitted another tip action");
            assert!(!first_tip.borrow().unwrap().pressed, "focus leaving the empty tip stranded the mouse press");

            first_tip.set_text("first");
            let draw_actions = cx.capture_actions(|cx| target.draw(cx, &mut root));
            assert!(tip_reports(&draw_actions).is_empty(), "restoring text after the mouse press emitted a draw-time tip action");
            assert_eq!(focus(&mut cx, &mut root, &mut layer, first.area()).as_deref(), Some("first"),
                "the old mouse press suppressed returning programmatic focus");
            assert_eq!(focus(&mut cx, &mut root, &mut layer, Area::Empty), None, "the restored tip stayed up after focus left");
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &mouse_move(over_first)).as_deref(), Some("first"),
                "the old mouse press suppressed direct hover after text was restored");
        });
    }

    #[test]
    fn press_suppression_never_pops_blocks_focus_refresh_or_strands_without_focus() {
        use a11y_tree::{Actions as A, NodeId};
        crate::on_test_cx(|| {
            let mut cx = cx();
            cx.init_cx_os();
            let mut root = view(&mut cx, script! {
                use mod.prelude.widgets.*
                View{ width: 300 height: 20 flow: Right
                    a_tip := Tip{ text: "alpha" Button{ text: "a" width: 50 height: 20 } }
                    b_tip := Tip{ text: "beta" Button{ text: "b" width: 50 height: 20 } }
                    no_focus_tip := Tip{ text: "no focus" Button{ text: "n" width: 50 height: 20 grab_key_focus: false } }
                    disabled_tip := Tip{ text: "disabled" Button{ text: "d" width: 50 height: 20 enabled: false } }
                    check_tip := Tip{ text: "check" CheckBox{ text: "c" width: 50 height: 20 } }
                }
            });
            let mut layer = tip_layer(&mut cx);
            let mut target = Target::new(&mut cx, dvec2(300.0, 20.0));
            target.draw(&mut cx, &mut root);
            let a_tip = root.tip(&cx, ids!(a_tip));
            let b_tip = root.tip(&cx, ids!(b_tip));
            let no_focus_tip = root.tip(&cx, ids!(no_focus_tip));
            let disabled_tip = root.tip(&cx, ids!(disabled_tip));
            let check_tip = root.tip(&cx, ids!(check_tip));
            let a_tip_uid = a_tip.borrow().unwrap().widget_uid();
            let check_tip_uid = check_tip.borrow().unwrap().widget_uid();
            let a = at(&root, &[0, 0]);
            let b = at(&root, &[1, 0]);
            let no_focus = at(&root, &[2, 0]);
            let disabled = at(&root, &[3, 0]);
            let check = at(&root, &[4, 0]);
            let over_a = middle(&cx, &a);
            let over_b = middle(&cx, &b);
            let over_check = middle(&cx, &check);
            let away = dvec2(280.0, 10.0);

            // #3: clearing the press later must not revive a suppressed focus tip.
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &mouse_move(over_a)).as_deref(), Some("alpha"),
                "#3: hovering alpha should raise its tip");
            let down = mouse_down(over_a, MouseButton::PRIMARY);
            let actions = deliver_actions(&mut cx, &mut root, &mut layer, &down);
            assert_eq!(tip_reports(&actions), [(a_tip_uid, TipAction::HoverOut)], "#3: pressing alpha should only drop its tip");
            assert_eq!(held(&layer), None, "#3: the layer kept alpha after the press");
            assert!(a_tip.borrow().unwrap().pressed, "#3: alpha missed its press");
            assert!(a_tip.borrow().unwrap().spent, "#3: pressing alpha should spend its hover");
            let actions = settle_focus_actions(&mut cx, &mut root, &mut layer);
            assert!(cx.has_key_focus(a.area()), "#3: the press should focus alpha");
            assert!(tip_reports(&actions).is_empty(), "#3: press-driven focus emitted a tip action");
            assert_eq!(held(&layer), None, "#3: press-driven focus raised alpha");
            assert!(!a_tip.borrow().unwrap().focused, "#3: suppressed focus was recorded as a raised tip");
            assert!(a_tip.borrow().unwrap().pressed, "#3: settling focus cleared the press");
            deliver(&mut cx, &mut root, &mut layer, &mouse_up(over_a, MouseButton::PRIMARY));
            down.unhandle(&mut cx, &a.area());
            assert!(a_tip.borrow().unwrap().pressed, "#3: releasing the focused button cleared the press");
            assert!(!a_tip.borrow().unwrap().focused, "#3: release recorded suppressed focus");
            deliver(&mut cx, &mut root, &mut layer, &mouse_move(away));
            assert!(!a_tip.borrow().unwrap().spent, "#3: leaving alpha should clear spent hover");
            let shift = Event::KeyDown(KeyEvent { key_code: KeyCode::Shift, ..Default::default() });
            deliver(&mut cx, &mut root, &mut layer, &shift);
            assert!(!a_tip.borrow().unwrap().pressed, "#3: Shift should clear the press");
            assert!(!a_tip.borrow().unwrap().focused, "#3: Shift recorded previously suppressed focus");
            assert_eq!(held(&layer), None, "#3: Shift raised a suppressed tip");
            a_tip.set_text("alpha updated");
            let draw_actions = cx.capture_actions(|cx| target.draw(cx, &mut root));
            assert!(tip_reports(&draw_actions).is_empty(), "#3: updating a suppressed tip emitted a draw-time action");
            assert_eq!(held(&layer), None, "#3: the layer held alpha after the draw");
            cx.set_key_focus(Area::Empty);
            let actions = settle_focus_actions(&mut cx, &mut root, &mut layer);
            assert!(tip_reports(&actions).is_empty(), "#3: losing suppressed focus emitted an unmatched tip action");
            a_tip.set_text("alpha");

            // #4: return by keyboard with the pointer still resting on the button.
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &mouse_move(over_a)).as_deref(), Some("alpha"),
                "#4: hovering alpha again should show its restored text");
            let down = mouse_down(over_a, MouseButton::PRIMARY);
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &down), None, "#4: the press left alpha up");
            assert!(a_tip.borrow().unwrap().spent, "#4: the press should spend alpha's hover");
            assert!(a_tip.borrow().unwrap().pressed, "#4: alpha missed the second press");
            assert_eq!(settle_focus(&mut cx, &mut root, &mut layer), None, "#4: settling the press raised alpha");
            deliver(&mut cx, &mut root, &mut layer, &mouse_up(over_a, MouseButton::PRIMARY));
            down.unhandle(&mut cx, &a.area());
            assert!(a_tip.borrow().unwrap().spent, "#4: releasing in place cleared spent hover");
            let tab = KeyEvent { key_code: KeyCode::Tab, ..Default::default() };
            deliver(&mut cx, &mut root, &mut layer, &Event::KeyDown(tab));
            assert!(!a_tip.borrow().unwrap().pressed, "#4: Tab should clear the press");
            assert!(a_tip.borrow().unwrap().spent, "#4: Tab should preserve spent hover");
            assert_eq!(focus(&mut cx, &mut root, &mut layer, b.area()).as_deref(), Some("beta"), "#4: Tab missed beta's tip");
            let back = Event::KeyDown(KeyEvent { modifiers: KeyModifiers { shift: true, ..Default::default() }, ..tab });
            deliver(&mut cx, &mut root, &mut layer, &back);
            assert_eq!(focus(&mut cx, &mut root, &mut layer, a.area()).as_deref(), Some("alpha"), "#4: Shift+Tab missed alpha's tip");
            assert!(a_tip.borrow().unwrap().focused, "#4: keyboard focus should record a raised tip");
            assert!(a_tip.borrow().unwrap().spent, "#4: returning by keyboard should preserve spent hover");
            layer.show_pending(&mut cx);
            assert_eq!(layer.showing.as_ref().map(|t| t.text.as_str()), Some("alpha"), "#4: the layer should show alpha before refresh");
            a_tip.set_text("alpha live");
            let draw_actions = cx.capture_actions(|cx| target.draw(cx, &mut root));
            let draw_reports = tip_reports(&draw_actions);
            layer.handle_event(&mut cx, &Event::Actions(draw_actions), &mut Scope::empty());
            assert!(matches!(draw_reports.as_slice(),
                [(uid, TipAction::HoverInWith(r))] if *uid == a_tip_uid && r.text == "alpha live"),
                "#4: the draw should report alpha's updated focus tip exactly once");
            assert_eq!(layer.showing.as_ref().map(|t| t.text.as_str()), Some("alpha live"), "#4: the visible tip kept its old text");
            deliver(&mut cx, &mut root, &mut layer, &mouse_move(away));
            assert!(!a_tip.borrow().unwrap().spent, "#4: leaving the button should clear spent hover");
            assert_eq!(focus(&mut cx, &mut root, &mut layer, Area::Empty), None, "#4: clearing focus left alpha up");

            // #2: presses that end without requesting focus must allow later programmatic focus.
            let over_no_focus = middle(&cx, &no_focus);
            let down = mouse_down(over_no_focus, MouseButton::PRIMARY);
            deliver(&mut cx, &mut root, &mut layer, &down);
            assert!(no_focus_tip.borrow().unwrap().pressed, "#2: the non-focusing button missed its press");
            assert_eq!(settle_focus(&mut cx, &mut root, &mut layer), None, "#2: pressing the non-focusing button raised a tip");
            assert!(!cx.has_key_focus(no_focus.area()), "#2: the button took focus despite grab_key_focus being false");
            deliver(&mut cx, &mut root, &mut layer, &mouse_up(over_no_focus, MouseButton::PRIMARY));
            down.unhandle(&mut cx, &no_focus.area());
            assert!(!no_focus_tip.borrow().unwrap().pressed, "#2: release on the non-focusing button stranded the press");
            assert_eq!(focus(&mut cx, &mut root, &mut layer, no_focus.area()).as_deref(), Some("no focus"),
                "#2: programmatic focus after the non-focusing press missed its tip");
            assert_eq!(focus(&mut cx, &mut root, &mut layer, Area::Empty), None, "#2: clearing focus left the non-focusing button's tip up");

            for (control, tip, button, release, text, case) in [
                (&disabled, &disabled_tip, MouseButton::PRIMARY, middle(&cx, &disabled), "disabled", "disabled button"),
                (&b, &b_tip, MouseButton::SECONDARY, over_b, "beta", "secondary click"),
                (&check, &check_tip, MouseButton::PRIMARY, away, "check", "checkbox drag-off"),
            ] {
                let down = mouse_down(middle(&cx, control), button);
                deliver(&mut cx, &mut root, &mut layer, &down);
                assert!(tip.borrow().unwrap().pressed, "#2: {case} missed the press");
                deliver(&mut cx, &mut root, &mut layer, &mouse_up(release, button));
                down.unhandle(&mut cx, &control.area());
                assert!(!tip.borrow().unwrap().pressed, "#2: {case} stranded the press on release");
                assert_eq!(focus(&mut cx, &mut root, &mut layer, control.area()).as_deref(), Some(text),
                    "#2: {case} suppressed later programmatic focus");
                assert_eq!(focus(&mut cx, &mut root, &mut layer, Area::Empty), None, "#2: {case} left a tip after focus moved away");
            }

            let down = mouse_down(over_check, MouseButton::PRIMARY);
            deliver(&mut cx, &mut root, &mut layer, &down);
            assert!(check_tip.borrow().unwrap().pressed, "#2: the checkbox missed the click inside");
            assert!(!cx.has_key_focus(check.area()), "#2: the checkbox should defer focus to release");
            deliver(&mut cx, &mut root, &mut layer, &mouse_up(over_check, MouseButton::PRIMARY));
            down.unhandle(&mut cx, &check.area());
            assert_eq!(cx.next_key_focus(), check.area(), "#2: releasing inside should queue checkbox focus");
            assert!(check_tip.borrow().unwrap().pressed, "#2: release cleared the press while checkbox focus was pending");
            let actions = settle_focus_actions(&mut cx, &mut root, &mut layer);
            assert!(cx.has_key_focus(check.area()), "#2: the queued checkbox focus did not settle");
            assert!(tip_reports(&actions).is_empty(), "#2: settling deferred focus emitted a tip action");
            assert_eq!(held(&layer), None, "#2: deferred focus raised the checkbox tip");
            assert!(!check_tip.borrow().unwrap().focused, "#2: the checkbox recorded suppressed focus");
            assert_eq!(focus(&mut cx, &mut root, &mut layer, Area::Empty), None, "#2: leaving the checkbox left a tip up");
            assert!(!check_tip.borrow().unwrap().pressed, "#2: leaving the checkbox stranded the press");

            for (state, abs) in [(TouchState::Start, over_b), (TouchState::Stop, away)] {
                let touch = TouchUpdateEvent {
                    time: 0.0,
                    window_id: WindowId(1, 1),
                    modifiers: KeyModifiers::default(),
                    touches: vec![TouchPoint {
                        state, abs, time: 0.0, uid: 1, rotation_angle: 0.0, force: 1.0,
                        radius: dvec2(1.0, 1.0),
                        handled: std::cell::Cell::new(Area::Empty),
                        sweep_lock: std::cell::Cell::new(Area::Empty),
                    }],
                };
                let event = Event::TouchUpdate(touch);
                deliver(&mut cx, &mut root, &mut layer, &event);
                if state == TouchState::Start {
                    assert!(b_tip.borrow().unwrap().pressed, "#2: the touch did not press beta");
                    assert_eq!(b_tip.borrow().unwrap().finger, Some(1), "#2: beta missed the touching finger");
                } else if let Event::TouchUpdate(touch) = &event {
                    cx.fingers.process_touch_update_end(&touch.touches);
                }
            }
            assert_eq!(b_tip.borrow().unwrap().finger, None, "#2: touch drag-off kept the finger");
            assert!(!b_tip.borrow().unwrap().pressed, "#2: touch drag-off stranded the press");
            assert!(!cx.has_key_focus(b.area()), "#2: touch drag-off gave beta focus");
            assert_eq!(focus(&mut cx, &mut root, &mut layer, b.area()).as_deref(), Some("beta"),
                "#2: touch drag-off suppressed later programmatic focus");
            assert_eq!(focus(&mut cx, &mut root, &mut layer, Area::Empty), None, "#2: clearing focus after touch left beta up");

            for (control, tip, text) in [(&no_focus, &no_focus_tip, "no focus"), (&disabled, &disabled_tip, "disabled")] {
                let press = a11y_req(NodeId::widget(control.widget_uid().0), A::PRESS);
                deliver(&mut cx, &mut root, &mut layer, &press);
                assert!(!tip.borrow().unwrap().pressed, "#2: accessibility press on {text} stranded the press");
                assert_eq!(focus(&mut cx, &mut root, &mut layer, control.area()).as_deref(), Some(text),
                    "#2: accessibility press on {text} suppressed later programmatic focus");
                assert_eq!(focus(&mut cx, &mut root, &mut layer, Area::Empty), None, "#2: {text} stayed up after focus left");
            }
            deliver(&mut cx, &mut root, &mut layer, &mouse_move(away));
            assert!(!check_tip.borrow().unwrap().spent, "#2: moving away should clear spent checkbox hover");
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &mouse_move(over_check)).as_deref(), Some("check"),
                "#2: hovering the checkbox should raise its tip before the accessibility toggle");
            let toggle = a11y_req(NodeId::widget(check.widget_uid().0), A::TOGGLE);
            let actions = deliver_actions(&mut cx, &mut root, &mut layer, &toggle);
            assert_eq!(tip_reports(&actions), [(check_tip_uid, TipAction::HoverOut)], "#2: toggle should only drop the checkbox tip");
            assert_eq!(held(&layer), None, "#2: toggle left the checkbox tip up");
            assert!(check_tip.borrow().unwrap().spent, "#2: toggle should spend the hovered checkbox tip");
            assert!(!check_tip.borrow().unwrap().pressed, "#2: the unfocused toggle stranded the press");
            assert_eq!(focus(&mut cx, &mut root, &mut layer, check.area()).as_deref(), Some("check"),
                "#2: toggle suppressed later programmatic checkbox focus");
        });
    }

    #[test]
    fn cancelled_touch_and_hidden_tip_never_strand_or_raise_tips() {
        use a11y_tree::{Actions as A, NodeId};
        crate::on_test_cx(|| {
            let mut cx = cx();
            cx.init_cx_os();
            let mut root = view(&mut cx, script! {
                use mod.prelude.widgets.*
                View{ width: 200 height: 20 flow: Right
                    a_tip := Tip{ text: "alpha" Button{ text: "a" width: 50 height: 20 } }
                    b_tip := Tip{ text: "beta" Button{ text: "b" width: 50 height: 20 } }
                    empty_tip := Tip{ text: "" Button{ text: "e" width: 50 height: 20 } }
                }
            });
            let mut layer = tip_layer(&mut cx);
            let mut target = Target::new(&mut cx, dvec2(200.0, 20.0));
            target.draw(&mut cx, &mut root);
            let a_tip = root.tip(&cx, ids!(a_tip));
            let b_tip = root.tip(&cx, ids!(b_tip));
            let empty_tip = root.tip(&cx, ids!(empty_tip));
            let a_tip_uid = a_tip.borrow().unwrap().widget_uid();
            let b_tip_uid = b_tip.borrow().unwrap().widget_uid();
            let a = at(&root, &[0, 0]);
            let b = at(&root, &[1, 0]);
            let empty_btn = at(&root, &[2, 0]);
            let over_a = middle(&cx, &a);
            let over_b = middle(&cx, &b);
            let over_empty = middle(&cx, &empty_btn);
            let away = dvec2(180.0, 10.0);
            let window_id = WindowId(1, 1);
            let touch = |uid, state, abs| {
                let time = if state == TouchState::Start { 0.0 } else { 0.6 };
                Event::TouchUpdate(TouchUpdateEvent {
                    window_id,
                    time,
                    modifiers: KeyModifiers::default(),
                    touches: vec![TouchPoint {
                        uid, state, abs, time, rotation_angle: 0.0, force: 1.0,
                        radius: dvec2(1.0, 1.0),
                        handled: std::cell::Cell::new(Area::Empty),
                        sweep_lock: std::cell::Cell::new(Area::Empty),
                    }],
                })
            };
            let end_touch = |cx: &mut Cx, event: &Event| {
                let Event::TouchUpdate(update) = event else { unreachable!() };
                cx.fingers.process_touch_update_end(&update.touches);
            };
            let long_press = |uid, abs| Event::LongPress(LongPressEvent { window_id, uid, abs, time: 0.5 });

            // #1a: the host delivers both a cancelled touch and a finger cancellation.
            let start = touch(1, TouchState::Start, over_a);
            assert_eq!(deliver(&mut cx, &mut root, &mut layer, &start), None, "#1a: touch start raised alpha");
            assert!(a_tip.borrow().unwrap().pressed, "#1a: alpha missed the touch press");
            assert_eq!(a_tip.borrow().unwrap().finger, Some(1), "#1a: alpha missed the touching finger");
            assert!(!a_tip.borrow().unwrap().long_pressed, "#1a: touch start recorded a long press");
            let raised = deliver(&mut cx, &mut root, &mut layer, &long_press(1, over_a));
            assert_eq!(raised.as_deref(), Some("alpha"), "#1a: long press missed alpha's tip");
            assert_eq!(layer.showing.as_ref().map(|t| t.text.as_str()), Some("alpha"), "#1a: long press should show alpha immediately");
            assert!(a_tip.borrow().unwrap().long_pressed, "#1a: alpha missed its long press");
            let cancel = touch(1, TouchState::Cancel, over_a);
            let actions = deliver_actions(&mut cx, &mut root, &mut layer, &cancel);
            assert_eq!(tip_reports(&actions), [(a_tip_uid, TipAction::HoverOut)], "#1a: touch cancel should drop only alpha, exactly once");
            assert_eq!(held(&layer), None, "#1a: touch cancel left alpha up");
            assert_eq!(a_tip.borrow().unwrap().finger, None, "#1a: touch cancel kept the finger");
            assert!(!a_tip.borrow().unwrap().long_pressed, "#1a: touch cancel kept the long press");
            assert!(!a_tip.borrow().unwrap().pressed, "#1a: touch cancel stranded the press");
            let paired_cancel = Event::FingerCancel(FingerCancelEvent {
                window_id, digit_id: live_id_num!(touch, 1).into(), device: DigitDevice::Touch { uid: 1 },
                abs: over_a, time: 0.6, modifiers: KeyModifiers::default(),
            });
            let actions = deliver_actions(&mut cx, &mut root, &mut layer, &paired_cancel);
            assert!(tip_reports(&actions).is_empty(), "#1a: the paired finger cancel emitted a second tip action");
            end_touch(&mut cx, &cancel);
            assert_eq!(focus(&mut cx, &mut root, &mut layer, a.area()).as_deref(), Some("alpha"), "#1a: touch cancel suppressed later focus");
            assert_eq!(focus(&mut cx, &mut root, &mut layer, Area::Empty), None, "#1a: clearing focus left alpha up");

            // #1b: the gesture claim identifies the captured touch by digit, then beta reuses its UID.
            deliver(&mut cx, &mut root, &mut layer, &touch(1, TouchState::Start, over_a));
            assert!(a_tip.borrow().unwrap().pressed, "#1b: alpha missed the press before the claim");
            assert_eq!(a_tip.borrow().unwrap().finger, Some(1), "#1b: alpha missed the finger before the claim");
            let raised = deliver(&mut cx, &mut root, &mut layer, &long_press(1, over_a));
            assert_eq!(raised.as_deref(), Some("alpha"), "#1b: long press missed alpha before the claim");
            assert!(a_tip.borrow().unwrap().long_pressed, "#1b: alpha missed the long press before the claim");
            cx.fingers.cancel_digit_except(live_id_num!(touch, 1).into(), Area::Empty);
            let cancels = cx.fingers.take_pending_cancels(0.6);
            assert_eq!(cancels.len(), 1, "#1b: the claim should queue exactly one cancellation");
            assert!(matches!(cancels[0].device, DigitDevice::Touch { uid: 0 }), "#1b: a claim's cancel reports the touch device with uid 0");
            let actions = deliver_actions(&mut cx, &mut root, &mut layer, &Event::FingerCancel(cancels[0].clone()));
            assert_eq!(tip_reports(&actions), [(a_tip_uid, TipAction::HoverOut)], "#1b: the claim should drop only alpha, exactly once");
            assert_eq!(held(&layer), None, "#1b: the claim left alpha up");
            assert_eq!(a_tip.borrow().unwrap().finger, None, "#1b: the claim kept the captured finger");
            assert!(!a_tip.borrow().unwrap().long_pressed, "#1b: the claim kept the long press");
            assert!(!a_tip.borrow().unwrap().pressed, "#1b: the claim stranded the press");
            end_touch(&mut cx, &touch(1, TouchState::Stop, over_a));
            deliver(&mut cx, &mut root, &mut layer, &touch(1, TouchState::Start, over_b));
            let actions = deliver_actions(&mut cx, &mut root, &mut layer, &long_press(1, over_b));
            assert!(matches!(tip_reports(&actions).as_slice(),
                [(uid, TipAction::HoverInWith(r))] if *uid == b_tip_uid && r.text == "beta"),
                "#1b: long pressing with the reused finger should raise only beta");
            assert_eq!(held(&layer).as_deref(), Some("beta"), "#1b: the layer missed beta for the reused finger");
            let stop = touch(1, TouchState::Stop, away);
            deliver(&mut cx, &mut root, &mut layer, &stop);
            end_touch(&mut cx, &stop);
            assert_eq!(held(&layer), None, "#1b: lifting the reused finger left beta up");
            assert_eq!(b_tip.borrow().unwrap().finger, None, "#1b: beta kept the reused finger after lift");
            assert!(!b_tip.borrow().unwrap().pressed, "#1b: beta kept the press after lift");
            assert_eq!(focus(&mut cx, &mut root, &mut layer, a.area()).as_deref(), Some("alpha"), "#1b: the claim suppressed later alpha focus");
            assert_eq!(focus(&mut cx, &mut root, &mut layer, Area::Empty), None, "#1b: clearing focus left alpha up");

            // #1c: a hosted mouse cancellation ends the secondary press without a mouse-up.
            deliver(&mut cx, &mut root, &mut layer, &mouse_down(over_b, MouseButton::SECONDARY));
            assert!(b_tip.borrow().unwrap().pressed, "#1c: beta missed the secondary press");
            assert_eq!(b_tip.borrow().unwrap().finger, None, "#1c: beta recorded a touch finger for the mouse");
            cx.fingers.cancel_digit(live_id!(mouse).into());
            let mouse_cancel = Event::FingerCancel(FingerCancelEvent {
                window_id, digit_id: live_id!(mouse).into(), device: DigitDevice::Mouse { button: MouseButton::SECONDARY },
                abs: over_b, time: 0.6, modifiers: KeyModifiers::default(),
            });
            deliver(&mut cx, &mut root, &mut layer, &mouse_cancel);
            assert!(!b_tip.borrow().unwrap().pressed, "#1c: mouse cancel stranded the secondary press");
            assert_eq!(focus(&mut cx, &mut root, &mut layer, b.area()).as_deref(), Some("beta"), "#1c: mouse cancel suppressed later focus");
            assert_eq!(focus(&mut cx, &mut root, &mut layer, Area::Empty), None, "#1c: clearing focus left beta up");

            // #1d: a long-press tip whose text empties still needs exactly one dismissal.
            deliver(&mut cx, &mut root, &mut layer, &touch(2, TouchState::Start, over_a));
            let raised = deliver(&mut cx, &mut root, &mut layer, &long_press(2, over_a));
            assert_eq!(raised.as_deref(), Some("alpha"), "#1d: long press missed alpha before the text emptied");
            assert!(a_tip.borrow().unwrap().long_pressed, "#1d: alpha missed the long press before the text emptied");
            a_tip.set_text("");
            let stop = touch(2, TouchState::Stop, away);
            let actions = deliver_actions(&mut cx, &mut root, &mut layer, &stop);
            end_touch(&mut cx, &stop);
            assert_eq!(tip_reports(&actions), [(a_tip_uid, TipAction::HoverOut)], "#1d: lift should drop the emptied tip exactly once");
            assert_eq!(held(&layer), None, "#1d: lift left the emptied tip up");
            assert_eq!(a_tip.borrow().unwrap().finger, None, "#1d: the emptied tip kept the finger");
            assert!(!a_tip.borrow().unwrap().long_pressed, "#1d: the emptied tip kept the long press");
            assert!(!a_tip.borrow().unwrap().pressed, "#1d: the emptied tip kept the press");
            a_tip.set_text("alpha");

            // #5a: hiding an empty tip with a finger down clears the finger silently.
            deliver(&mut cx, &mut root, &mut layer, &touch(3, TouchState::Start, over_empty));
            assert_eq!(empty_tip.borrow().unwrap().finger, Some(3), "#5a: the empty tip missed the touching finger");
            assert!(empty_tip.borrow().unwrap().pressed, "#5a: the empty tip missed the press");
            assert!(!empty_tip.borrow().unwrap().long_pressed, "#5a: the empty tip recorded a long press");
            let actions = cx.capture_actions(|cx| at(&root, &[2]).set_visible(cx, false));
            assert!(tip_reports(&actions).is_empty(), "#5a: hiding an empty tip emitted a tip action");
            assert_eq!(empty_tip.borrow().unwrap().finger, None, "#5a: hiding the empty tip kept the finger");
            assert!(!empty_tip.borrow().unwrap().pressed, "#5a: hiding the empty tip kept the press");
            end_touch(&mut cx, &touch(3, TouchState::Stop, over_empty));
            at(&root, &[2]).set_visible(&mut cx, true);

            let press = a11y_req(NodeId::widget(a.widget_uid().0), A::PRESS);
            at(&root, &[0]).set_visible(&mut cx, false);
            cx.set_key_focus(a.area());
            let actions = settle_focus_actions(&mut cx, &mut root, &mut layer);
            assert!(tip_reports(&actions).is_empty(), "#5a: focusing the hidden wrapper emitted a tip action");
            assert_eq!(held(&layer), None, "#5a: focus raised the hidden wrapper's tip");
            assert!(!a_tip.borrow().unwrap().focused, "#5a: the hidden wrapper recorded focus");
            assert!(!a_tip.borrow().unwrap().pressed, "#5a: the hidden wrapper recorded a press on focus");
            focus(&mut cx, &mut root, &mut layer, Area::Empty);
            let actions = deliver_actions(&mut cx, &mut root, &mut layer, &press);
            assert!(tip_reports(&actions).is_empty(), "#5a: accessibility press on the hidden wrapper emitted a tip action");
            assert!(!a_tip.borrow().unwrap().pressed, "#5a: the hidden wrapper recorded the accessibility press");
            assert!(!a_tip.borrow().unwrap().focused, "#5a: the hidden wrapper recorded focus on the accessibility press");
            let actions = settle_focus_actions(&mut cx, &mut root, &mut layer);
            assert!(tip_reports(&actions).is_empty(), "#5a: settling the hidden wrapper's accessibility press emitted a tip action");
            assert_eq!(held(&layer), None, "#5a: the hidden wrapper raised a tip after the accessibility press");
            assert!(!a_tip.borrow().unwrap().focused, "#5a: the hidden wrapper recorded focus after the accessibility press");
            focus(&mut cx, &mut root, &mut layer, Area::Empty);
            at(&root, &[0]).set_visible(&mut cx, true);

            // #5b: the wrapper stays visible while only its child hides, with the last drawn rect intact.
            a.set_visible(&mut cx, false);
            let stale_hover = mouse_move(middle(&cx, &a));
            let actions = deliver_actions(&mut cx, &mut root, &mut layer, &stale_hover);
            assert!(tip_reports(&actions).is_empty(), "#5b: hovering the hidden child's rect emitted a tip action");
            assert_eq!(held(&layer), None, "#5b: hovering the hidden child's rect raised a tip");
            assert!(!a_tip.borrow().unwrap().hovered, "#5b: the wrapper recorded hover on its hidden child");
            cx.set_key_focus(a.area());
            let actions = settle_focus_actions(&mut cx, &mut root, &mut layer);
            assert!(tip_reports(&actions).is_empty(), "#5b: focusing the hidden child emitted a tip action");
            assert_eq!(held(&layer), None, "#5b: focusing the hidden child raised a tip");
            assert!(!a_tip.borrow().unwrap().focused, "#5b: the wrapper recorded focus on its hidden child");
            focus(&mut cx, &mut root, &mut layer, Area::Empty);
            let actions = deliver_actions(&mut cx, &mut root, &mut layer, &press);
            assert!(tip_reports(&actions).is_empty(), "#5b: accessibility press on the hidden child emitted a tip action");
            assert!(!a_tip.borrow().unwrap().pressed, "#5b: the wrapper recorded the hidden child's accessibility press");
            assert!(!a_tip.borrow().unwrap().focused, "#5b: the wrapper recorded focus on the hidden child's accessibility press");
            let actions = settle_focus_actions(&mut cx, &mut root, &mut layer);
            assert!(tip_reports(&actions).is_empty(), "#5b: settling the hidden child's accessibility press emitted a tip action");
            assert_eq!(held(&layer), None, "#5b: the hidden child raised a tip after the accessibility press");
            assert!(!a_tip.borrow().unwrap().focused, "#5b: the wrapper recorded focus after the hidden child's accessibility press");
            focus(&mut cx, &mut root, &mut layer, Area::Empty);
            a.set_visible(&mut cx, true);
            assert_eq!(focus(&mut cx, &mut root, &mut layer, a.area()).as_deref(), Some("alpha"), "#5b: showing the child should restore its focus tip");
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

//! EmptyState — what a list shows when it has nothing to show.
//!
//! A list with no rows in it still owes the reader an answer, and the five
//! answers are not one answer. Nothing has been made yet; nothing matched
//! the filter; the rows are there and this account may not read them; the
//! fetch failed, so the list is unknown rather than empty; the device is
//! offline and cannot tell. A blank rectangle says none of these, and a
//! single "No items" says the wrong one four times out of five.
//!
//! The shape never changes: a mark, a heading, a line of body text, and up
//! to two things to do about it, in a centred column. What changes is the
//! WORDING, so the five cases are presets of one widget rather than five
//! widgets — [`EmptyStateNothingYet`], `EmptyStateNoMatches`,
//! `EmptyStateNotAllowed`, `EmptyStateFailed` and `EmptyStateOffline` in the
//! script module, each carrying its own heading and body so the common case
//! is one line at the call site and any of it can still be overridden by
//! writing the property.
//!
//! The mark above the words is a SLOT, not a property: an `EmptyGlyph` by
//! default, or an icon, a picture, a whole illustration if the app has one.
//! `EmptyGlyph` builds its five figures out of circles, boxes and rects
//! rather than paths, because a small mark drawn as a path does not paint
//! reliably here.
//!
//! **What it is not.** It has no face of its own — no card, no border, no
//! fill — so it takes the colour of whatever it is dropped into, and a host
//! that wants it on a card puts it on one. It does not decide when to
//! appear: something else knows the list is empty. And it raises no action
//! of its own; the two action slots hold ordinary widgets the host reads by
//! id, because a single "the action was taken" event could not say WHICH of
//! two actions was taken, which is the whole reason there are two.
//!
//! [`EmptyStateNothingYet`]: EmptyState

use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

/// The figure over the words. Five, because there are five reasons a list
/// is empty and a reader acts differently on each.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum EmptyMark {
    /// An empty tray: nothing has been made yet.
    #[pick]
    Nothing = 0,
    /// A lens: the search or the filter matched nothing.
    NoMatches = 1,
    /// A barred ring: the rows are there and may not be read.
    NotAllowed = 2,
    /// A ringed exclamation: the fetch failed, so the list is unknown.
    Failed = 3,
    /// Barred signal bars: there is no connection to ask over.
    Offline = 4,
}

impl EmptyMark {
    /// The name a snapshot reports and `set_text` accepts.
    pub fn name(self) -> &'static str {
        match self {
            EmptyMark::Nothing => "nothing",
            EmptyMark::NoMatches => "no-matches",
            EmptyMark::NotAllowed => "not-allowed",
            EmptyMark::Failed => "failed",
            EmptyMark::Offline => "offline",
        }
    }

    /// Spaces and underscores read as hyphens, because a caller writing
    /// "no matches" means the same thing as one writing "no-matches".
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase().replace([' ', '_'], "-");
        [
            EmptyMark::Nothing,
            EmptyMark::NoMatches,
            EmptyMark::NotAllowed,
            EmptyMark::Failed,
            EmptyMark::Offline,
        ]
        .into_iter()
        .find(|mark| mark.name() == name)
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*

    // Declared before the `use` below, because a block's `use` only sees
    // what exists when it runs. Not splatted: "Nothing", "Failed" and
    // "Offline" are words another module could want for its own enum, so
    // they stay behind their type name.
    let EmptyMark = set_type_default() do #(EmptyMark::script_api(vm))
    mod.widgets.EmptyMark = EmptyMark

    use mod.widgets.*

    mod.widgets.DrawEmptyGlyphBase = #(DrawEmptyGlyph::script_component(vm))
    set_type_default() do #(DrawEmptyGlyph::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.EmptyGlyphBase = #(EmptyGlyph::register_widget(vm))

    /** The mark over the words: one of five figures, drawn in one ink.
     *
     * Every figure is circles, boxes and rects. Paths are avoided on
     * purpose: two mirrored identical paths in one shader here painted
     * only the second, and a mark that sometimes fails to appear is worse
     * than a plainer mark that always does. */
    mod.widgets.EmptyGlyph = set_type_default() do mod.widgets.EmptyGlyphBase{
        width: 48
        height: 48
        /** which figure: EmptyMark.Nothing NoMatches NotAllowed Failed Offline */
        mark: EmptyMark.Nothing
        /** drawn at all; false leaves the words with no mark over them */
        visible: true
        // Every value here is plain and every one has a field on the draw
        // struct behind it. Mixing uniform() into a draw type that also
        // carries instance fields moves the instance slots out from under
        // a caller who overrides one, and the figure then reads a stroke
        // width of garbage.
        draw_bg +: {
            /** the ink the figure is drawn in */
            color: theme.color_on_surface_variant
            /** stroke width in pixels 0.5..4 step 0.25 */
            stroke: 1.5
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let s = min(self.rect_size.x, self.rect_size.y)
                let cx = self.rect_size.x * 0.5
                let cy = self.rect_size.y * 0.5
                let w = max(1.0, self.stroke)
                match self.mark {
                    EmptyMark.NoMatches => {
                        // A lens. The ring and the handle are drawn in a
                        // frame turned an eighth of a turn, so the handle
                        // comes off the ring on the diagonal without a
                        // single path segment.
                        sdf.rotate(-0.7853982, cx, cy)
                        sdf.circle(cx, cy - s * 0.09, s * 0.22)
                        sdf.rotate(0.7853982, cx, cy)
                        sdf.stroke(self.color, w)
                        sdf.rotate(-0.7853982, cx, cy)
                        sdf.rect(cx - w * 0.5, cy + s * 0.13, w, s * 0.21)
                        sdf.rotate(0.7853982, cx, cy)
                        sdf.fill(self.color)
                    }
                    EmptyMark.NotAllowed => {
                        sdf.circle(cx, cy, s * 0.28)
                        sdf.stroke(self.color, w)
                        // The bar is exactly the ring's diameter, so its
                        // ends land on the ring rather than short of it.
                        sdf.rotate(-0.7853982, cx, cy)
                        sdf.rect(cx - s * 0.28, cy - w * 0.5, s * 0.56, w)
                        sdf.rotate(0.7853982, cx, cy)
                        sdf.fill(self.color)
                    }
                    EmptyMark.Failed => {
                        sdf.circle(cx, cy, s * 0.28)
                        sdf.stroke(self.color, w)
                        // Stem and dot are one shape: consecutive calls
                        // union into the same field, so one fill paints
                        // both and neither can go missing without the
                        // other.
                        sdf.rect(cx - w * 0.6, cy - s * 0.16, w * 1.2, s * 0.18)
                        sdf.circle(cx, cy + s * 0.13, w * 0.9)
                        sdf.fill(self.color)
                    }
                    EmptyMark.Offline => {
                        // Three bars standing on one line, and a bar laid
                        // across them. The bars go down to a lower alpha
                        // so the slash reads as something over them and
                        // not as a fourth bar.
                        sdf.rect(cx - s * 0.30, cy + s * 0.04, s * 0.11, s * 0.16)
                        sdf.rect(cx - s * 0.06, cy - s * 0.07, s * 0.11, s * 0.27)
                        sdf.rect(cx + s * 0.19, cy - s * 0.18, s * 0.11, s * 0.38)
                        sdf.fill(vec4(self.color.xyz, self.color.w * 0.45))
                        sdf.rotate(-0.7853982, cx, cy)
                        sdf.rect(cx - s * 0.42, cy - w * 0.5, s * 0.84, w)
                        sdf.rotate(0.7853982, cx, cy)
                        sdf.fill(self.color)
                    }
                    _ => {
                        // Nothing yet: an empty tray. Also the answer for
                        // any mark a later caller invents and this shader
                        // has not been taught, which is the harmless one
                        // to be wrong with.
                        sdf.box(cx - s * 0.32, cy - s * 0.24, s * 0.64, s * 0.48, s * 0.04)
                        sdf.stroke(self.color, w)
                        sdf.rect(cx - s * 0.32, cy - s * 0.06, s * 0.64, w)
                        sdf.fill(self.color)
                    }
                }
                return sdf.result
            }
        }
    }

    mod.widgets.EmptyStateBase = #(EmptyState::register_widget(vm))

    /** A mark, a heading, a line of body text and up to two actions, in a
     * centred column. The five presets below differ only in what they say. */
    mod.widgets.EmptyState = set_type_default() do mod.widgets.EmptyStateBase{
        width: Fill
        height: Fit
        padding: Inset{left: theme.space_3, right: theme.space_3, top: theme.space_5, bottom: theme.space_5}
        /** gap between the mark, the words and the actions 0..48 step 1 */
        spacing: theme.space_3
        /** the bold line: what is going on */
        heading: ""
        /** the quiet line under it: what to do about it */
        body: ""
        /** widest the words get before they wrap 120..640 step 10 */
        max_width: 320.0
        /** gap between the heading and the body 0..24 step 1 */
        text_spacing: theme.space_1
        /** drawn at all */
        visible: true
        /** the mark over the words: an EmptyGlyph, an icon, a picture */
        icon: mod.widgets.EmptyGlyph{}
        draw_heading +: {
            text_style: theme.font_bold{
                /** heading type size in points 6..32 step 0.5 */
                font_size: theme.type_title_s_size
                line_spacing: 1.2
            }
            color: theme.color_text
        }
        draw_body +: {
            text_style: theme.font_regular{
                /** body type size in points 6..32 step 0.5 */
                font_size: theme.type_body_m_size
                line_spacing: 1.35
            }
            color: theme.color_on_surface_variant
        }
    }

    /** Nothing has been made yet: the first run, before anyone has added
     * anything. The reader is not stuck, they have simply not started. */
    mod.widgets.EmptyStateNothingYet = mod.widgets.EmptyState{
        heading: "Nothing here yet"
        body: "Whatever you add shows up in this list."
        icon: mod.widgets.EmptyGlyph{mark: EmptyMark.Nothing}
    }

    /** A search or a filter matched nothing. The rows may well exist; the
     * way back is to ask for less, so the body says so. */
    mod.widgets.EmptyStateNoMatches = mod.widgets.EmptyState{
        heading: "No matches"
        body: "Nothing here answers to that. Try a shorter search, or clear the filters."
        icon: mod.widgets.EmptyGlyph{mark: EmptyMark.NoMatches}
    }

    /** The rows are there and this account may not read them. Naming who
     * to ask is the only useful thing this screen can do. */
    mod.widgets.EmptyStateNotAllowed = mod.widgets.EmptyState{
        heading: "You cannot see this"
        body: "This list exists, and your account is not allowed to read it. Ask whoever owns it for access."
        icon: mod.widgets.EmptyGlyph{mark: EmptyMark.NotAllowed}
    }

    /** The fetch failed, so the list is UNKNOWN rather than empty — the
     * one case where "nothing here" would be a lie. */
    mod.widgets.EmptyStateFailed = mod.widgets.EmptyState{
        heading: "Something went wrong"
        body: "The list could not be loaded. A second try often works."
        icon: mod.widgets.EmptyGlyph{mark: EmptyMark.Failed}
    }

    /** No connection to ask over. Says what still works offline, because
     * that is what the reader wants to know next. */
    mod.widgets.EmptyStateOffline = mod.widgets.EmptyState{
        heading: "You are offline"
        body: "This list needs a connection. Anything you have already opened is still here."
        icon: mod.widgets.EmptyGlyph{mark: EmptyMark.Offline}
    }
}

/// The mark's figure and ink. `mark` picks the figure in the shader.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawEmptyGlyph {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    mark: EmptyMark,
    #[live]
    color: Vec4f,
    #[live]
    stroke: f32,
}

/// The narrowest the text column is allowed to get. A column narrower than
/// this leaves the `Fill` walk the wrapped text sits in with nothing to
/// resolve to, and the words are laid out and never painted.
pub const MIN_TEXT_COLUMN: f64 = 40.0;

/// The width the words get: the room the parent gives, or the caller's cap,
/// whichever is smaller. A `Fit` parent measures NaN, and then the cap is
/// all there is to go on.
pub fn text_column(available: f64, max_width: f64) -> f64 {
    let cap = if max_width.is_finite() && max_width > 0.0 {
        max_width
    } else {
        f64::INFINITY
    };
    let room = if available.is_finite() && available > 0.0 {
        available
    } else {
        cap
    };
    let column = room.min(cap);
    if column.is_finite() {
        column.max(MIN_TEXT_COLUMN)
    } else {
        MIN_TEXT_COLUMN
    }
}

/// Whether two actions share a line: only while both, and the gap between
/// them, fit the column they are centred in. A slot that has never been
/// drawn measures zero and counts as fitting, so the first frame lays them
/// out in a row and the second corrects it — which is right, because the
/// alternative is a frame with the buttons stacked for no reason.
pub fn actions_side_by_side(column: f64, first: f64, second: f64, spacing: f64) -> bool {
    if first <= 0.0 || second <= 0.0 {
        return true;
    }
    first + spacing + second <= column
}

/// One run of text, wrapped to the column it is in and centred row by row.
/// The layouter only wraps inside a wrapping row flow, the way `Label`
/// arranges it, and only centres rows when it has a width to centre them in.
fn draw_centred(cx: &mut Cx2d, draw: &mut DrawText, text: &str) {
    cx.begin_turtle(
        Walk::fill_fit(),
        Layout {
            flow: Flow::right_wrap(),
            ..Layout::default()
        },
    );
    draw.draw_walk(cx, Walk::fill_fit(), Align { x: 0.5, y: 0.0 }, text);
    cx.end_turtle();
}

/// Whether a slot has something in it that wants drawing.
fn shown(slot: &WidgetRef) -> bool {
    !slot.is_empty() && slot.visible()
}

/// What a slot took the last time it was drawn; zero for one that never
/// has been, which is what the first frame sees.
fn slot_width(cx: &Cx, slot: &WidgetRef) -> f64 {
    if !shown(slot) {
        return 0.0;
    }
    // max() returns the other side of a NaN, which is what an area with no
    // rect yet reports.
    slot.area().rect(cx).size.x.max(0.0)
}

/// Draw a slot at the walk it asks for, or nothing when it is empty or
/// hidden — a hidden slot must take no room either, or the column keeps a
/// gap where the mark used to be.
fn draw_slot(cx: &mut Cx2d, scope: &mut Scope, slot: &WidgetRef) {
    if !shown(slot) {
        return;
    }
    let walk = slot.walk(cx.cx.cx);
    let _ = slot.draw_walk(cx, scope, walk);
}

/// The mark over the words: one figure in one ink.
#[derive(Script, ScriptHook, Widget)]
pub struct EmptyGlyph {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawEmptyGlyph,
    #[live]
    pub mark: EmptyMark,
    #[live(true)]
    #[visible]
    visible: bool,
}

impl EmptyGlyph {
    pub fn set_mark(&mut self, cx: &mut Cx, mark: EmptyMark) {
        if self.mark != mark {
            self.mark = mark;
            self.draw_bg.redraw(cx);
        }
    }
}

impl Widget for EmptyGlyph {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.draw_bg.mark = self.mark;
        self.draw_bg.draw_walk(cx, walk);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    /// The mark by name: "nothing", "no-matches", ...
    fn text(&self) -> String {
        self.mark.name().to_string()
    }

    /// Accepts a mark name; anything else leaves the mark alone.
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if let Some(mark) = EmptyMark::from_name(v) {
            self.set_mark(cx, mark);
        }
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.mark.name().to_string())
    }
}

impl EmptyGlyphRef {
    pub fn set_mark(&self, cx: &mut Cx, mark: EmptyMark) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_mark(cx, mark);
        }
    }

    pub fn mark(&self) -> Option<EmptyMark> {
        self.borrow().map(|inner| inner.mark)
    }
}

/// The whole answer: a mark, a heading, a body and up to two actions.
#[derive(Script, ScriptHook, WidgetRef, WidgetSet, WidgetRegister)]
pub struct EmptyState {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_heading: DrawText,
    #[live]
    draw_body: DrawText,

    /// The mark over the words. Any widget: the default `EmptyGlyph`, an
    /// icon, a picture, an illustration the app owns.
    #[live]
    pub icon: WidgetRef,
    /// The thing to do about it.
    #[live]
    pub action: WidgetRef,
    /// The other thing, when there are two.
    #[live]
    pub secondary: WidgetRef,

    #[live]
    pub heading: String,
    #[live]
    pub body: String,
    /// The widest the words get before they wrap.
    #[live(320.0)]
    pub max_width: f64,
    /// The gap between the heading and the body.
    #[live(3.0)]
    pub text_spacing: f64,
    #[live(true)]
    visible: bool,

    /// The whole block, which is what a snapshot and a pick see.
    #[rust]
    area: Area,
}

impl EmptyState {
    pub fn set_heading(&mut self, cx: &mut Cx, heading: &str) {
        if self.heading != heading {
            self.heading = heading.to_string();
            self.area.redraw(cx);
        }
    }

    pub fn set_body(&mut self, cx: &mut Cx, body: &str) {
        if self.body != body {
            self.body = body.to_string();
            self.area.redraw(cx);
        }
    }
}

impl WidgetNode for EmptyState {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.area
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }

    /// The slots are children under their own names, so `ids!(state.action)`
    /// reaches the button an app put there. (A `#[find]` field would hand
    /// out that button's own children instead and hide the button itself,
    /// which is exactly what a host asking for the action does not want.)
    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (id, slot) in [
            (live_id!(icon), &self.icon),
            (live_id!(action), &self.action),
            (live_id!(secondary), &self.secondary),
        ] {
            if !slot.is_empty() {
                visit(id, slot.clone());
            }
        }
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        for slot in [&self.icon, &self.action, &self.secondary] {
            slot.find_widgets_from_point(cx, point, found);
        }
    }

    fn layer_areas(&self) -> Vec<(&'static str, Area)> {
        vec![
            ("draw_heading", self.draw_heading.area()),
            ("draw_body", self.draw_body.area()),
        ]
    }

    fn visible(&self) -> bool {
        self.visible
    }

    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        // A widget that has never been drawn has no area to redraw through,
        // and its parent has to lay it out afresh before it exists at all.
        if matches!(self.area, Area::Empty) {
            cx.redraw_all();
        } else {
            self.area.redraw(cx);
        }
    }
}

impl Widget for EmptyState {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        let mut layout = self.layout;
        // The shape is the whole point of the widget: a centred column. A
        // caller who wants a row wants a different widget.
        layout.flow = Flow::Down;
        layout.align = Align { x: 0.5, y: 0.0 };
        let spacing = layout.spacing;
        // Measured before the turtle opens, the way an alert measures its
        // row: once begun, the turtle answers about the inside.
        let outer = cx.turtle().next_walk_width(walk.width, walk.margin);
        let column = text_column(
            outer - layout.padding.left - layout.padding.right,
            self.max_width,
        );

        cx.begin_turtle(walk, layout);
        draw_slot(cx, scope, &self.icon);
        if !self.heading.is_empty() || !self.body.is_empty() {
            // A FIXED width, never Fit: the wrapped text inside asks for
            // Fill, and a Fill inside a Fit resolves to nothing and never
            // paints.
            cx.begin_turtle(
                Walk {
                    width: Size::Fixed(column),
                    height: Size::fit(),
                    ..Walk::default()
                },
                Layout {
                    flow: Flow::Down,
                    spacing: self.text_spacing,
                    align: Align { x: 0.5, y: 0.0 },
                    ..Layout::default()
                },
            );
            if !self.heading.is_empty() {
                draw_centred(cx, &mut self.draw_heading, &self.heading);
            }
            if !self.body.is_empty() {
                draw_centred(cx, &mut self.draw_body, &self.body);
            }
            cx.end_turtle();
        }
        if shown(&self.action) || shown(&self.secondary) {
            // Two buttons that do not both fit the column stand one over
            // the other rather than running out past the words.
            let row = actions_side_by_side(
                column,
                slot_width(cx.cx.cx, &self.action),
                slot_width(cx.cx.cx, &self.secondary),
                spacing,
            );
            // Fit, not the column's width: the outer column centres this
            // row anyway, and a fixed width would CLIP a button whose
            // label ran wider than the words above it.
            cx.begin_turtle(
                Walk::fit(),
                Layout {
                    flow: if row { Flow::right() } else { Flow::Down },
                    spacing,
                    align: Align { x: 0.5, y: 0.5 },
                    ..Layout::default()
                },
            );
            draw_slot(cx, scope, &self.action);
            draw_slot(cx, scope, &self.secondary);
            cx.end_turtle();
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.visible && !matches!(event, Event::FingerCancel(_)) {
            return;
        }
        for slot in [&self.icon, &self.action, &self.secondary] {
            if !slot.is_empty() {
                slot.handle_event(cx, event, scope);
            }
        }
    }

    fn text(&self) -> String {
        self.heading.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.set_heading(cx, v);
    }

    /// The heading and the body as one line, so a test can wait on the
    /// wording rather than on which preset was used.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        if self.body.is_empty() {
            return Some(self.heading.clone());
        }
        Some(format!("{} — {}", self.heading, self.body))
    }
}

impl EmptyStateRef {
    pub fn set_heading(&self, cx: &mut Cx, heading: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_heading(cx, heading);
        }
    }

    pub fn set_body(&self, cx: &mut Cx, body: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_body(cx, body);
        }
    }

    pub fn heading(&self) -> String {
        self.borrow().map(|inner| inner.heading.clone()).unwrap_or_default()
    }

    pub fn body(&self) -> String {
        self.borrow().map(|inner| inner.body.clone()).unwrap_or_default()
    }

    /// The mark, for a host that switches one state between cases rather
    /// than swapping presets.
    pub fn glyph(&self) -> EmptyGlyphRef {
        match self.borrow() {
            Some(inner) => inner.icon.as_empty_glyph(),
            None => WidgetRef::empty().as_empty_glyph(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_column_is_the_room_or_the_cap_whichever_is_less() {
        assert_eq!(text_column(500.0, 320.0), 320.0);
        assert_eq!(text_column(200.0, 320.0), 200.0);
        // A Fit parent measures NaN: the cap is all there is to go on.
        assert_eq!(text_column(f64::NAN, 320.0), 320.0);
        // Narrower than the floor and the wrapped text would never paint.
        assert_eq!(text_column(10.0, 320.0), MIN_TEXT_COLUMN);
        assert_eq!(text_column(f64::NAN, 0.0), MIN_TEXT_COLUMN);
    }

    #[test]
    fn two_actions_stand_one_over_the_other_when_they_do_not_fit() {
        assert!(actions_side_by_side(320.0, 100.0, 100.0, 8.0));
        assert!(!actions_side_by_side(180.0, 100.0, 100.0, 8.0));
        // Exactly the width is still a fit.
        assert!(actions_side_by_side(208.0, 100.0, 100.0, 8.0));
        // One action, or a slot not yet drawn: nothing to stack.
        assert!(actions_side_by_side(40.0, 100.0, 0.0, 8.0));
    }

    #[test]
    fn every_mark_answers_to_its_own_name() {
        for mark in [
            EmptyMark::Nothing,
            EmptyMark::NoMatches,
            EmptyMark::NotAllowed,
            EmptyMark::Failed,
            EmptyMark::Offline,
        ] {
            assert_eq!(EmptyMark::from_name(mark.name()), Some(mark));
        }
        // The spellings a caller reaches for before the hyphenated one.
        assert_eq!(EmptyMark::from_name("No Matches"), Some(EmptyMark::NoMatches));
        assert_eq!(EmptyMark::from_name("not_allowed"), Some(EmptyMark::NotAllowed));
        assert_eq!(EmptyMark::from_name("nothing at all"), None);
    }
}

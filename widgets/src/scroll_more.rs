//! Two ways of saying what a scrolling box is not showing.
//!
//! A scroll view is very quiet about the part of the content that is off
//! screen. The bar says roughly how far down the handle is and nothing about
//! what is down there; the edge of the box says nothing at all, so a list cut
//! off mid-row looks exactly like a list that happened to end. These two
//! widgets answer those two questions, and they live together because they
//! are the same question asked at the two ends of the same box.
//!
//! # AnnotatedScrollBar
//!
//! A scrollbar whose track carries marks: a coloured tick at every place in
//! the content worth knowing about — a search hit, an error, a line somebody
//! changed. The track stops being a position readout and becomes a map of the
//! whole document, so a search that matched forty times is forty ticks you can
//! aim at rather than a count and a "next" button pressed forty times.
//!
//! Marks are given as a position between 0 and 1 and a colour. Two marks that
//! land on the same row of pixels collapse into one, because a file with five
//! thousand hits would otherwise paint the track solid and say nothing; the
//! first of a run keeps its colour, so a caller that lists what matters most
//! first gets that colour drawn.
//!
//! It deliberately does NOT scroll anything. It is told how much content
//! there is and reports where the handle went; wiring that to a view is the
//! host's job, because the thing being mapped is very often not a plain
//! scroll view — it is a virtualized list, a document model, a timeline. Nor
//! does a press on a mark jump to it: the track already means "go to here",
//! and a second meaning on the same press is one too many.
//!
//! # ScrollShadowView
//!
//! A scrolling box that fades an edge whenever there is content past it, and
//! only then. Each edge decides for itself, so the top of a list is flat until
//! it has been scrolled and the bottom is soft until the last row is reached.
//! The fade comes up over the first stretch of overflow rather than switching
//! on, so a box scrolled by two pixels does not flash a hard shadow.
//!
//! It deliberately does NOT fade an edge that cannot scroll: an axis whose
//! content fits has no "past it" to point at, and a permanent gradient down
//! one side would read as a decoration nobody chose. It also does not animate
//! on its own — the fade follows the scroll and nothing else — and it does not
//! offer an inner shadow as well as a fade, because the two are the same
//! statement made twice.
use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    scroll_bar::{ScrollAxis, ScrollBar, ScrollBarAction},
    scroll_bars::{ScrollBars, ScrollExtent},
    view::View,
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.AnnotatedScrollBarBase = #(AnnotatedScrollBar::register_widget(vm))
    mod.widgets.ScrollShadowViewBase = #(ScrollShadowView::register_widget(vm))

    set_type_default() do #(DrawScrollMark::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    set_type_default() do #(DrawEdgeFade::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    /** A scrollbar whose track carries a mark for every place worth knowing about. */
    mod.widgets.AnnotatedScrollBar = set_type_default() do mod.widgets.AnnotatedScrollBarBase{
        width: 16.
        height: Fill

        /** how much content there is, in whatever unit the host counts in 1..100000 step 10 */
        view_total: 2000.
        /** run down the page rather than across it */
        vertical: true
        // The handle's own end margin. The marks have to be told it
        // separately because the bar keeps it to itself, and a lane that
        // starts half a mark above the handle's travel is a map that lies.
        /** room left at each end of the mark lane, in pixels 0..12 step 0.5 */
        track_inset: 3.
        /** how thick one mark is, in pixels 1..8 step 0.5 */
        mark_thickness: 2.

        /** a mark that named no ink */
        mark_color: theme.color_primary
        /** the ink named "warn" */
        mark_color_warn: theme.color_warning
        /** the ink named "error" */
        mark_color_error: theme.color_error
        /** the ink named "change" */
        mark_color_change: theme.color_success

        /** marks written here: a position from 0 to 1, then warn, error or change */
        marks: []

        bar: mod.widgets.ScrollBar{
            bar_size: 8.
            bar_side_margin: 3.
        }

        draw_bg +: {
            /** track corner rounding radius 0..8 step 0.5 */
            border_radius: uniform(2.0)

            color: uniform(theme.color_inset)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(
                    0.0
                    0.0
                    self.rect_size.x
                    self.rect_size.y
                    self.border_radius
                )
                sdf.fill(self.color)
                return sdf.result
            }
        }

        draw_mark +: {
            // A tick is two pixels tall. Rounding one is the difference
            // between a mark and a smudge, so this is a rect and not a box.
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                sdf.fill(self.color)
                return sdf.result
            }
        }
    }

    /** The same bar laid across the page instead of down it. */
    mod.widgets.AnnotatedScrollBarX = mod.widgets.AnnotatedScrollBar{
        width: Fill
        height: 16.
        vertical: false
    }

    /** A scrolling box whose edges fade wherever there is more content past them. */
    mod.widgets.ScrollShadowView = set_type_default() do mod.widgets.ScrollShadowViewBase{
        width: Fill
        height: Fill
        flow: Down

        /** how deep an edge fade is, in pixels 0..80 step 1 */
        fade_size: 20.
        /** how much content must lie past an edge for its fade to reach full strength, in pixels 1..200 step 1 */
        fade_ramp: 24.

        /** fade the top edge when there is content above it */
        fade_top: true
        /** fade the bottom edge when there is content below it */
        fade_bottom: true
        /** fade the left edge when there is content before it */
        fade_left: true
        /** fade the right edge when there is content after it */
        fade_right: true

        /** let the content scroll across */
        scroll_x: false
        /** let the content scroll down */
        scroll_y: true

        bars: mod.widgets.ScrollBars{
            show_scroll_x: false
            show_scroll_y: true
            scroll_bar_y.drag_scrolling: true
        }

        draw_fade +: {
            /** 0 fades along the y axis, 1 along the x axis 0..1 step 1 */
            across: 0.0
            /** 0 hangs the fade off the near edge, 1 off the far one 0..1 step 1 */
            far: 0.0
            /** how strongly it is drawn 0..1 step 0.01 */
            weight: 0.0

            /** the colour the content dissolves into */
            color: uniform(theme.color_bg_container)
            /** how quickly the fade falls off; 1 is a straight ramp 0.2..4 step 0.1 */
            falloff: uniform(1.6)

            pixel: fn() {
                // The four edges are one ramp read along a different axis and
                // in a different direction, so they are two mixes rather than
                // four branches.
                let along = mix(self.pos.y, self.pos.x, self.across)
                let t = mix(along, 1.0 - along, self.far)
                let a = pow(clamp(1.0 - t, 0.0, 1.0), self.falloff) * self.weight
                return Pal.premul(vec4(self.color.xyz, self.color.w * a))
            }
        }
    }

    /** A box that scrolls both ways, fading all four edges. */
    mod.widgets.ScrollShadowXYView = mod.widgets.ScrollShadowView{
        scroll_x: true
        scroll_y: true
        bars: mod.widgets.ScrollBars{
            show_scroll_x: true
            show_scroll_y: true
            scroll_bar_x.drag_scrolling: true
            scroll_bar_y.drag_scrolling: true
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawScrollMark {
    #[deref]
    draw_super: DrawQuad,
    /// Set per mark from Rust, so one shader draws every ink.
    #[live]
    color: Vec4f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawEdgeFade {
    #[deref]
    draw_super: DrawQuad,
    /// Which way the ramp runs and which end it starts at.
    #[live]
    across: f32,
    #[live]
    far: f32,
    /// How much content is past this edge, as a share of `fade_ramp`.
    #[live]
    weight: f32,
}

/// One mark on a track: where it sits in the content, and what colour it is.
///
/// `at` is a share of the whole, 0 at the start and 1 at the end, rather than
/// a line number or a pixel offset. The bar has no idea what the content is
/// counted in, and asking it to would only mean telling it twice.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ScrollMark {
    pub at: f64,
    pub color: Vec4f,
}

impl ScrollMark {
    pub fn new(at: f64, color: Vec4f) -> Self {
        Self { at, color }
    }
}

/// Which of the bar's four inks a mark written in the DSL asks for.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum MarkInk {
    Plain,
    Warn,
    Error,
    Change,
}

/// Read one mark written in the DSL: a position, then optionally the name of
/// an ink.
///
/// An entry that will not read is dropped rather than guessed at. A mark in
/// the wrong place is worse than a missing one: it is a promise that there is
/// something at a point in the document where there is nothing, and the only
/// way to find that out is to go there.
fn parse_mark(text: &str) -> Option<(f64, MarkInk)> {
    let mut words = text.split_whitespace();
    let at = words.next()?.parse::<f64>().ok()?;
    let ink = match words.next() {
        None => MarkInk::Plain,
        Some("warn") => MarkInk::Warn,
        Some("error") => MarkInk::Error,
        Some("change") => MarkInk::Change,
        Some(_) => return None,
    };
    if words.next().is_some() {
        return None;
    }
    Some((at, ink))
}

/// The most rows of pixels a lane is credited with; taller than any window
/// this runs in.
const MAX_ROWS: usize = 8192;

/// The lane the marks are placed in, apart from the widget that draws it.
///
/// It is a separate type for two reasons — it can be tested without a script
/// heap, and the placement and the collapsing take their numbers from the same
/// place, so a mark that is drawn is a mark that is really there.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Track {
    /// The lane's length along the scroll axis, in pixels.
    length: f64,
    /// Room left at each end, matching the handle's own so the marks line up
    /// with the positions the handle can actually reach.
    inset: f64,
    /// One mark's drawn thickness.
    thickness: f64,
}

impl Track {
    /// How far a mark's leading edge may travel. A mark at the end has to fit
    /// inside the lane, so its own thickness comes off the travel — otherwise
    /// the last mark hangs over the end and the map is a mark long.
    fn travel(&self) -> f64 {
        let travel = self.length - self.inset * 2.0 - self.thickness;
        if travel.is_finite() && travel > 0.0 {
            travel
        } else {
            0.0
        }
    }

    /// Where the mark for `at` starts, from the leading edge of the lane.
    fn place(&self, at: f64) -> f64 {
        self.inset + at.clamp(0.0, 1.0) * self.travel()
    }

    /// The marks as rows of pixels, one row at most.
    ///
    /// Marks are collapsed rather than drawn on top of each other because a
    /// long document has far more interesting places than the track has rows:
    /// five thousand search hits down a four hundred pixel lane would paint it
    /// solid, which is the one result that carries no information at all. The
    /// first mark to claim a row keeps it, so a caller that puts errors before
    /// hits sees errors.
    fn rows(&self, marks: &[ScrollMark]) -> Vec<(f64, Vec4f)> {
        // A lane is a strip of a window. A length that says otherwise came
        // from a number somebody mistyped, and answering it with one slot per
        // pixel would turn that into a dead process.
        let count = self.travel().min(MAX_ROWS as f64) as usize + 1;
        let mut slots: Vec<Option<Vec4f>> = vec![None; count];
        for mark in marks {
            let row = (self.place(mark.at) - self.inset).round();
            let row = if row > 0.0 {
                (row as usize).min(count - 1)
            } else {
                0
            };
            if slots[row].is_none() {
                slots[row] = Some(mark.color);
            }
        }
        slots
            .iter()
            .enumerate()
            .filter_map(|(row, color)| color.map(|color| (self.inset + row as f64, color)))
            .collect()
    }
}

#[derive(Clone, Debug, Default)]
pub enum AnnotatedScrollBarAction {
    /// Where the content should now start, in the same unit as `view_total`.
    Scrolled(f64),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct AnnotatedScrollBar {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    /// The lane the marks sit in, and the widget's whole box.
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_mark: DrawScrollMark,
    /// The handle and everything it knows about being dragged. Owning one
    /// rather than redrawing its geometry keeps this bar and every other bar
    /// in the library agreeing about where a handle goes for a given scroll.
    #[live]
    bar: ScrollBar,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    /// How much content there is. The bar's own drawn length is the visible
    /// extent: a bar beside a viewport is exactly as long as the viewport, so
    /// asking for that number as well would only be a way of getting it wrong.
    #[live(2000.0)]
    pub view_total: f64,
    #[live(true)]
    pub vertical: bool,
    #[live(3.0)]
    pub track_inset: f64,
    #[live(2.0)]
    pub mark_thickness: f64,

    #[live]
    pub mark_color: Vec4f,
    #[live]
    pub mark_color_warn: Vec4f,
    #[live]
    pub mark_color_error: Vec4f,
    #[live]
    pub mark_color_change: Vec4f,

    /// Marks written in the DSL — `"0.42"`, or `"0.42 error"`. A convenience
    /// for a fixed set; anything a program works out belongs in `set_marks`.
    #[live]
    pub marks: Vec<String>,

    /// Marks handed in from Rust. Once a host has set them it owns the list,
    /// declared marks and all: two sources drawn at once would be a track
    /// nobody could account for.
    #[rust]
    given: Option<Vec<ScrollMark>>,
}

impl AnnotatedScrollBar {
    fn ink(&self, ink: MarkInk) -> Vec4f {
        match ink {
            MarkInk::Plain => self.mark_color,
            MarkInk::Warn => self.mark_color_warn,
            MarkInk::Error => self.mark_color_error,
            MarkInk::Change => self.mark_color_change,
        }
    }

    /// The marks written in the DSL, read fresh: every ink is a live property
    /// and the tweaker may have moved any of them since the last draw.
    fn declared(&self) -> Vec<ScrollMark> {
        self.marks
            .iter()
            .filter_map(|text| parse_mark(text))
            .map(|(at, ink)| ScrollMark { at, color: self.ink(ink) })
            .collect()
    }

    fn track(&self, size: Vec2d) -> Track {
        Track {
            length: if self.vertical { size.y } else { size.x },
            inset: self.track_inset,
            thickness: self.mark_thickness,
        }
    }

    /// How wide the mark lane is across the bar: everything the handle has
    /// not reserved, so a mark is never hidden under the handle that is
    /// meant to be taken to it.
    fn lane(&self, size: Vec2d) -> f64 {
        let across = if self.vertical { size.x } else { size.y };
        (across - self.bar.bar_size).max(2.0)
    }

    pub fn scroll_pos(&self) -> f64 {
        self.bar.get_scroll_pos()
    }

    pub fn set_scroll_pos(&mut self, cx: &mut Cx, pos: f64) {
        if self.bar.set_scroll_pos(cx, pos) {
            self.draw_bg.redraw(cx);
        }
    }

    /// Hand the bar a list of marks worked out by the host.
    pub fn set_marks(&mut self, cx: &mut Cx, marks: Vec<ScrollMark>) {
        if self.given.as_deref() == Some(marks.as_slice()) {
            return;
        }
        self.given = Some(marks);
        self.draw_bg.redraw(cx);
    }

    /// Give the declared marks back the track.
    pub fn clear_marks(&mut self, cx: &mut Cx) {
        if self.given.take().is_some() {
            self.draw_bg.redraw(cx);
        }
    }

    pub fn set_view_total(&mut self, cx: &mut Cx, total: f64) {
        if self.view_total != total {
            self.view_total = total;
            self.draw_bg.redraw(cx);
        }
    }
}

impl Widget for AnnotatedScrollBar {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);
        let size = cx.turtle().rect().size;
        // A Fit parent leaves the box unmeasured until it closes, and a bar
        // drawn into an unknown length would put every mark at the top.
        if size.x.is_finite() && size.y.is_finite() {
            let track = self.track(size);
            let rows = match &self.given {
                Some(marks) => track.rows(marks),
                None => track.rows(&self.declared()),
            };
            let lane = self.lane(size);
            for (at, color) in rows {
                self.draw_mark.color = color;
                let rect = if self.vertical {
                    Rect { pos: dvec2(0.0, at), size: dvec2(lane, self.mark_thickness) }
                } else {
                    Rect { pos: dvec2(at, 0.0), size: dvec2(self.mark_thickness, lane) }
                };
                self.draw_mark.draw_rel(cx, rect);
            }
            // The bar places itself against the trailing edge of the rect it
            // is handed, so it is handed this widget's own box: the strip IS
            // the viewport as far as the handle is concerned.
            let (axis, total) = if self.vertical {
                (ScrollAxis::Vertical, dvec2(0.0, self.view_total))
            } else {
                (ScrollAxis::Horizontal, dvec2(self.view_total, 0.0))
            };
            self.bar
                .draw_scroll_bar(cx, axis, Rect { pos: dvec2(0.0, 0.0), size }, total);
        }
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.uid;
        let mut moved = None;
        self.bar.handle_event_with(cx, event, &mut |_cx, action| {
            if let ScrollBarAction::Scroll { scroll_pos, .. } = action {
                moved = Some(scroll_pos);
            }
        });
        // The wheel works over the strip itself. The content it maps is
        // somebody else's widget and answers its own wheel; this is only so
        // that a pointer resting on the map is not a dead patch of window.
        let area = self.draw_bg.area();
        self.bar
            .handle_scroll_event(cx, event, area, &mut |_cx, action| {
                if let ScrollBarAction::Scroll { scroll_pos, .. } = action {
                    moved = Some(scroll_pos);
                }
            });
        if let Some(pos) = moved {
            cx.widget_action(uid, AnnotatedScrollBarAction::Scrolled(pos));
            self.draw_bg.redraw(cx);
        }
    }
}

impl AnnotatedScrollBarRef {
    /// Where the handle went, while it is being moved.
    pub fn scrolled(&self, actions: &Actions) -> Option<f64> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            AnnotatedScrollBarAction::Scrolled(pos) => Some(pos),
            AnnotatedScrollBarAction::None => None,
        }
    }

    pub fn scroll_pos(&self) -> f64 {
        self.borrow().map(|inner| inner.scroll_pos()).unwrap_or(0.0)
    }

    pub fn set_scroll_pos(&self, cx: &mut Cx, pos: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_scroll_pos(cx, pos);
        }
    }

    pub fn set_marks(&self, cx: &mut Cx, marks: Vec<ScrollMark>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_marks(cx, marks);
        }
    }

    pub fn clear_marks(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear_marks(cx);
        }
    }

    pub fn set_view_total(&self, cx: &mut Cx, total: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_view_total(cx, total);
        }
    }
}

/// One axis of the edge decision, apart from the widget that draws it: where
/// the viewport sits inside the content, and how loud the fade at each end of
/// it should be. Separate so it can be tested without a script heap.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Axis {
    /// How far into the content the viewport has moved, in pixels.
    scroll: f64,
    /// How much of the content shows.
    view: f64,
    /// How much of it there is.
    content: f64,
    /// The overflow at which a fade reaches full strength.
    ramp: f64,
}

impl Axis {
    /// Content before the viewport. A rubber band stretched past the start
    /// makes this negative, and there is nothing above the top of a document
    /// however far it has been pulled.
    fn before(&self) -> f64 {
        self.scroll.max(0.0)
    }

    fn after(&self) -> f64 {
        (self.content - self.scroll - self.view).max(0.0)
    }

    /// A fade comes up over the first `ramp` pixels of overflow instead of
    /// switching on. Scrolling is continuous and a box moved by one pixel has
    /// barely hidden anything; a hard edge appearing at that point reads as a
    /// glitch rather than as information.
    fn strength(&self, overflow: f64) -> f64 {
        if self.ramp <= 0.0 {
            if overflow > 0.0 {
                1.0
            } else {
                0.0
            }
        } else {
            (overflow / self.ramp).clamp(0.0, 1.0)
        }
    }

    fn fade_before(&self) -> f64 {
        self.strength(self.before())
    }

    fn fade_after(&self) -> f64 {
        self.strength(self.after())
    }
}

#[derive(Clone)]
enum Phase {
    Content,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ScrollShadowView {
    #[source]
    source: ScriptObjectRef,
    /// The content. It carries the layout, so `flow`, `padding` and `spacing`
    /// written on this widget mean what they mean on any other box.
    #[deref]
    view: View,
    #[live]
    draw_fade: DrawEdgeFade,
    /// The scroll lives out here rather than on the inner view: the fades are
    /// drawn from the scroll offset and the content extent, and a view keeps
    /// both of those to itself.
    #[live]
    bars: ScrollBars,

    #[live(20.0)]
    pub fade_size: f64,
    #[live(24.0)]
    pub fade_ramp: f64,
    #[live(true)]
    pub fade_top: bool,
    #[live(true)]
    pub fade_bottom: bool,
    #[live(true)]
    pub fade_left: bool,
    #[live(true)]
    pub fade_right: bool,

    /// Which way the content may overflow. The inner view is measured loosely
    /// on a scrolling axis and made to fit on one that does not scroll.
    #[live]
    pub scroll_x: bool,
    #[live(true)]
    pub scroll_y: bool,

    #[rust]
    draw_state: DrawStateWrap<Phase>,
}

impl ScrollShadowView {
    pub fn scroll_pos(&self) -> Vec2d {
        self.bars.get_scroll_pos()
    }

    pub fn set_scroll(&mut self, cx: &mut Cx, pos: Vec2d) {
        self.bars.set_scroll_pos(cx, pos);
        self.view.redraw(cx);
    }

    /// Where the box is scrolled to and how far it can go. Read from the
    /// bars out here, because the inner view has none of its own.
    pub fn scroll_extent(&self) -> ScrollExtent {
        self.bars.extent()
    }

    /// The four fades, drawn inside the scroll turtle so the turtle's own clip
    /// holds them to the visible box.
    fn draw_fades(&mut self, cx: &mut Cx2d) {
        let depth = self.fade_size;
        if depth.is_nan() || depth <= 0.0 {
            return;
        }
        let (rect, scroll, used) = {
            let turtle = cx.turtle();
            (turtle.rect(), turtle.scroll(), turtle.used())
        };
        // A Fit parent leaves the turtle unmeasured until it closes; the
        // content is then all there is to go on.
        let mut size = rect.size;
        if !size.x.is_finite() {
            size.x = used.x;
        }
        if !size.y.is_finite() {
            size.y = used.y;
        }
        if !size.x.is_finite() || !size.y.is_finite() || size.x <= 0.0 || size.y <= 0.0 {
            return;
        }
        // The turtle's origin has the scroll already taken off it, so putting
        // it back is what pins a fade to the box rather than to the content.
        let at = rect.pos + scroll;

        let ramp = self.fade_ramp;
        let x = Axis { scroll: scroll.x, view: size.x, content: used.x, ramp };
        let y = Axis { scroll: scroll.y, view: size.y, content: used.y, ramp };

        let edges = [
            (
                self.fade_top,
                0.0,
                0.0,
                y.fade_before(),
                Rect { pos: at, size: dvec2(size.x, depth) },
            ),
            (
                self.fade_bottom,
                0.0,
                1.0,
                y.fade_after(),
                Rect {
                    pos: dvec2(at.x, at.y + size.y - depth),
                    size: dvec2(size.x, depth),
                },
            ),
            (
                self.fade_left,
                1.0,
                0.0,
                x.fade_before(),
                Rect { pos: at, size: dvec2(depth, size.y) },
            ),
            (
                self.fade_right,
                1.0,
                1.0,
                x.fade_after(),
                Rect {
                    pos: dvec2(at.x + size.x - depth, at.y),
                    size: dvec2(depth, size.y),
                },
            ),
        ];
        for (wanted, across, far, weight, band) in edges {
            if !wanted || weight <= 0.0 {
                continue;
            }
            self.draw_fade.across = across;
            self.draw_fade.far = far;
            self.draw_fade.weight = weight as f32;
            self.draw_fade.draw_abs(cx, band);
        }
    }
}

impl Widget for ScrollShadowView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, Phase::Content) {
            // The default layout is the whole point: this turtle is a window
            // onto the content and nothing else, and the content's own view
            // carries every layout property the DSL set.
            self.bars.begin(cx, walk, Layout::default());
        }
        if let Some(Phase::Content) = self.draw_state.get() {
            let inner = Walk::new(
                if self.scroll_x { Size::fit() } else { Size::fill() },
                if self.scroll_y { Size::fit() } else { Size::fill() },
            );
            self.view.draw_walk(cx, scope, inner)?;
            self.draw_fades(cx);
            self.bars.end(cx);
            self.draw_state.end();
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let mut actions = Vec::new();
        self.bars.handle_main_event(cx, event, scope, &mut actions);
        if !actions.is_empty() {
            cx.redraw_area_and_children(self.bars.area());
        }
        // A press that catches a running fling is spent stopping it: passing
        // it on as well would activate whatever the finger happened to land
        // on while reaching for the brake.
        if !self.bars.catch_fling_on_press(cx, event) {
            self.view.handle_event(cx, event, scope);
        }
        self.bars
            .handle_scroll_event(cx, event, scope, &mut Vec::new());
    }
}

impl ScrollShadowViewRef {
    pub fn scroll_pos(&self) -> Vec2d {
        self.borrow()
            .map(|inner| inner.scroll_pos())
            .unwrap_or(dvec2(0.0, 0.0))
    }

    pub fn set_scroll(&self, cx: &mut Cx, pos: Vec2d) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_scroll(cx, pos);
        }
    }

    /// See [`ScrollShadowView::scroll_extent`]. Zero on every axis when empty.
    pub fn scroll_extent(&self) -> ScrollExtent {
        self.borrow().map(|inner| inner.scroll_extent()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ink(x: f32) -> Vec4f {
        Vec4f { x, y: 0.0, z: 0.0, w: 1.0 }
    }

    /// A lane with the geometry the preset gives it.
    fn lane(length: f64) -> Track {
        Track { length, inset: 3.0, thickness: 2.0 }
    }

    /// A viewport of 200 into 1000 of content, with the preset's ramp.
    fn axis(scroll: f64) -> Axis {
        Axis { scroll, view: 200.0, content: 1000.0, ramp: 24.0 }
    }

    #[test]
    fn the_ends_of_the_lane_are_reachable_and_the_mark_stays_inside_it() {
        let t = lane(100.0);
        assert_eq!(t.place(0.0), 3.0, "the first mark starts at the inset");
        assert_eq!(
            t.place(1.0) + t.thickness,
            97.0,
            "and the last one ends at the other inset"
        );
    }

    #[test]
    fn a_position_outside_the_content_lands_on_the_nearest_end() {
        let t = lane(100.0);
        assert_eq!(t.place(-3.0), t.place(0.0));
        assert_eq!(t.place(40.0), t.place(1.0));
    }

    #[test]
    fn a_lane_with_no_room_puts_every_mark_at_the_inset() {
        // A bar squeezed to nothing still draws; it must not divide the
        // content by a travel of zero or index off the end of the rows.
        let t = lane(4.0);
        assert_eq!(t.travel(), 0.0);
        assert_eq!(t.place(0.5), 3.0);
        let rows = t.rows(&[ScrollMark::new(0.0, ink(1.0)), ScrollMark::new(1.0, ink(0.5))]);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn marks_on_the_same_row_of_pixels_collapse_to_one() {
        let t = lane(100.0);
        let close = [
            ScrollMark::new(0.500, ink(1.0)),
            ScrollMark::new(0.501, ink(0.5)),
            ScrollMark::new(0.502, ink(0.25)),
        ];
        let rows = t.rows(&close);
        assert_eq!(rows.len(), 1, "three hits a tenth of a pixel apart are one tick");
        assert_eq!(rows[0].1, ink(1.0), "the first of the run keeps the row");
    }

    #[test]
    fn a_thousand_marks_never_draw_more_ticks_than_the_lane_has_rows() {
        // The whole reason for collapsing: a solid track is the one result
        // that carries no information at all.
        let t = lane(100.0);
        let many: Vec<ScrollMark> = (0..1000)
            .map(|i| ScrollMark::new(i as f64 / 1000.0, ink(1.0)))
            .collect();
        let rows = t.rows(&many);
        assert!(rows.len() <= t.travel() as usize + 1);
        assert!(rows.len() >= 90, "and it still fills the lane");
    }

    #[test]
    fn rows_come_out_in_track_order_whatever_order_they_went_in() {
        let t = lane(100.0);
        let rows = t.rows(&[
            ScrollMark::new(0.9, ink(1.0)),
            ScrollMark::new(0.1, ink(0.5)),
            ScrollMark::new(0.5, ink(0.25)),
        ]);
        assert_eq!(rows.len(), 3);
        assert!(rows[0].0 < rows[1].0 && rows[1].0 < rows[2].0);
    }

    #[test]
    fn a_mark_that_is_not_a_number_is_dropped_rather_than_placed() {
        assert_eq!(parse_mark("0.42"), Some((0.42, MarkInk::Plain)));
        assert_eq!(parse_mark("0.42 error"), Some((0.42, MarkInk::Error)));
        assert_eq!(parse_mark("  0.5   warn  "), Some((0.5, MarkInk::Warn)));
        assert_eq!(parse_mark("0 change"), Some((0.0, MarkInk::Change)));
        assert_eq!(parse_mark(""), None);
        assert_eq!(parse_mark("halfway"), None);
        assert_eq!(parse_mark("0.42 mauve"), None, "an ink nobody has is not a plain mark");
        assert_eq!(parse_mark("0.42 error 0.9"), None);
    }

    #[test]
    fn a_box_at_the_top_says_only_that_there_is_more_below() {
        let a = axis(0.0);
        assert_eq!(a.fade_before(), 0.0);
        assert_eq!(a.fade_after(), 1.0);
    }

    #[test]
    fn a_box_at_the_bottom_says_only_that_there_is_more_above() {
        let a = axis(800.0);
        assert_eq!(a.fade_before(), 1.0);
        assert_eq!(a.fade_after(), 0.0);
    }

    #[test]
    fn a_box_in_the_middle_says_both() {
        let a = axis(400.0);
        assert_eq!(a.fade_before(), 1.0);
        assert_eq!(a.fade_after(), 1.0);
    }

    #[test]
    fn content_that_fits_fades_neither_end() {
        // The reason the decision is per edge at all: a permanent gradient
        // down a box that cannot scroll is a decoration nobody asked for.
        let a = Axis { scroll: 0.0, view: 200.0, content: 150.0, ramp: 24.0 };
        assert_eq!(a.fade_before(), 0.0);
        assert_eq!(a.fade_after(), 0.0);
    }

    #[test]
    fn a_fade_comes_up_over_the_first_stretch_of_overflow() {
        let a = Axis { scroll: 6.0, view: 200.0, content: 400.0, ramp: 24.0 };
        assert_eq!(a.fade_before(), 0.25);
        let a = Axis { scroll: 24.0, view: 200.0, content: 400.0, ramp: 24.0 };
        assert_eq!(a.fade_before(), 1.0, "and is full by the end of the ramp");
    }

    #[test]
    fn a_rubber_band_past_the_start_does_not_fade_the_start() {
        // Overscroll pulls the content past the top; there is still nothing
        // above the first line, and saying otherwise every time somebody
        // flicks upwards is worse than saying nothing.
        let a = axis(-40.0);
        assert_eq!(a.fade_before(), 0.0);
    }

    #[test]
    fn a_ramp_of_zero_is_a_switch() {
        let a = Axis { scroll: 1.0, view: 200.0, content: 400.0, ramp: 0.0 };
        assert_eq!(a.fade_before(), 1.0);
        let a = Axis { scroll: 0.0, view: 200.0, content: 400.0, ramp: 0.0 };
        assert_eq!(a.fade_before(), 0.0);
    }

    #[test]
    fn an_unmeasured_box_fades_nothing_rather_than_everything() {
        // A Fit parent hands the turtle a NaN size for one frame.
        let a = Axis { scroll: 0.0, view: f64::NAN, content: f64::NAN, ramp: 24.0 };
        assert_eq!(a.fade_before(), 0.0);
        assert_eq!(a.fade_after(), 0.0);
    }
}

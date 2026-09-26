//! A scrolling box whose edges say whether there is more content past them.
//!
//! A scroll view is very quiet about what it is not showing. The edge of the
//! box says nothing at all, so a list cut off mid-row looks exactly like a
//! list that happened to end. This one answers that; `scroll_marks` answers
//! the same question along the track of a bar instead of at the edge.
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
    scroll_bars::{ScrollBars, ScrollExtent},
    view::View,
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ScrollShadowViewBase = #(ScrollShadowView::register_widget(vm))

    set_type_default() do #(DrawEdgeFade::script_shader(vm)){
        ..mod.draw.DrawQuad
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

/// How much of one axis of the box shows, as the bars measure it.
///
/// A Fit box leaves the turtle unmeasured until it closes, and the content is
/// then all there is to go on — up to the most the box was allowed to grow to.
/// `ScrollBars::draw_scroll_bars` reads the same turtle in the same frame and
/// cuts the content at that ceiling. This has to agree with it to the pixel: a
/// `Fit{max}` box measured as long as its content has nothing past its far
/// edge, so the bar showed, the box scrolled, and the bottom stayed flat.
fn shown(measured: f64, content: f64, most: Option<f64>) -> f64 {
    if measured.is_nan() {
        most.map_or(content, |most| content.min(most))
    } else {
        measured
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
        // The same turtle the bars are about to ask, put the same question, so
        // a fade and a bar never disagree about how long the box is.
        let size = dvec2(
            shown(rect.size.x, used.x, cx.current_turtle_max_width()),
            shown(rect.size.y, used.y, cx.current_turtle_max_height()),
        );
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

    /// A viewport of 200 into 1000 of content, with the preset's ramp.
    fn axis(scroll: f64) -> Axis {
        Axis { scroll, view: 200.0, content: 1000.0, ramp: 24.0 }
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

    #[test]
    fn a_fit_box_with_a_ceiling_shows_the_ceiling_and_fades_what_is_past_it() {
        // A Fit box with a ceiling of 200 round 500 of content. The turtle
        // has no height yet; the bars call the box 200 tall and scroll it,
        // and the fade has to see the same 300 past the bottom that they do.
        let view = shown(f64::NAN, 500.0, Some(200.0));
        assert_eq!(view, 200.0, "the ceiling, not the content");
        let a = Axis { scroll: 0.0, view, content: 500.0, ramp: 24.0 };
        assert_eq!(a.fade_after(), 1.0, "so the bottom is soft");
        assert_eq!(a.fade_before(), 0.0);
    }

    #[test]
    fn a_fit_box_under_its_ceiling_is_as_long_as_its_content() {
        let view = shown(f64::NAN, 150.0, Some(200.0));
        assert_eq!(view, 150.0);
        let a = Axis { scroll: 0.0, view, content: 150.0, ramp: 24.0 };
        assert_eq!(a.fade_after(), 0.0, "and has nothing past either end");
    }

    #[test]
    fn a_fit_box_with_no_ceiling_is_as_long_as_its_content() {
        assert_eq!(shown(f64::NAN, 500.0, None), 500.0);
    }

    #[test]
    fn a_measured_box_is_taken_at_its_word() {
        // A ceiling is a bound on a Fit box. A box that was given a length
        // has one, whatever the content or the bounds say.
        assert_eq!(shown(200.0, 500.0, Some(120.0)), 200.0);
        assert_eq!(shown(200.0, 50.0, None), 200.0);
    }
}

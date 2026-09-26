//! Carousel — a row of items that scrolls sideways and stops on whole ones.
//!
//! A strip of items wider than the room it has, moved by dragging it, by
//! the arrows at its sides, by the dots beneath it, by the wheel or by the
//! keyboard. Wherever a gesture leaves it, it settles on a whole number of
//! items, so nothing is ever left cut in half against an edge. That one
//! rule is the widget; the rest is settings.
//!
//! # The three layouts are two numbers
//!
//! `per_view` is how many items share the viewport and `peek` is how much
//! of the neighbours shows at each side. Several at a time is `per_view: 3`.
//! One large one with its neighbours showing at the edges is `per_view: 1`
//! with a `peek`. Full width is `per_view: 1` with no peek and no gap.
//! They are settings and not three named modes, because a mode would have
//! to rule on what `per_view: 2` with a peek means, and there is nothing
//! there to rule on.
//!
//! # The dots count stops, not items
//!
//! With six items three at a time there are four places the strip can come
//! to rest, so there are four dots. Six would be a lie: two of them could
//! never be reached, and pressing one would light a different dot from the
//! one pressed. A strip that comes round can lead with any of its items, so
//! there the two counts happen to be the same.
//!
//! # Landing on an item
//!
//! A flick does not run its momentum out and then jerk to the nearest item.
//! The release velocity predicts where a strip decaying at the library's
//! scroll rate would stop, that prediction is snapped to an item, and the
//! strip glides there starting at the speed the finger left it with — the
//! same model the drum picker uses, so a flick feels the same wherever it
//! is made.
//!
//! # What it deliberately does not do
//!
//! It does not advance by itself. A strip that moves while it is being read
//! takes the reader's place away, and there is no setting for how fast
//! somebody reads.
//!
//! Its items are a line of text, a quieter second line and a ground colour,
//! not arbitrary children. What a carousel has to get right is the motion
//! and the stopping. A strip that also hosts arbitrary widgets has to rule
//! on the focus, the hit rects and the tab order of a widget that is half
//! outside the viewport, and those are a different set of decisions. A host
//! that needs real children in a strip should say so and get a widget that
//! answers them.
use crate::{
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    badge::{measure, sized},
    makepad_derive_widget::*,
    makepad_draw::*,
    scroll_motion::{
        estimate_release_velocity, push_sample, FrameClock, ScrollSample,
        FLING_DECEL_RATE_PER_MS, FLING_MIN_TOTAL_DELTA,
    },
    widget::*,
    widget_async::ScriptAsyncResult,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.CarouselItem = #(CarouselItem::script_api(vm))

    mod.widgets.CarouselBase = #(Carousel::register_widget(vm))

    set_type_default() do #(DrawCarousel::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    set_type_default() do #(DrawCarouselItem::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    /** A row of items that scrolls sideways and settles on whole ones. */
    mod.widgets.CarouselFlat = set_type_default() do mod.widgets.CarouselBase{
        width: Fill
        height: Fit
        margin: theme.mspace_1

        /** items sharing the viewport at one time 1..8 step 1 */
        per_view: 1
        /** how much of the neighbours shows at each side, in pixels 0..160 step 4 */
        peek: 0.0
        /** room between two items in pixels 0..48 step 1 */
        gap: 8.0
        /** an item's height in pixels 40..480 step 4 */
        item_height: 120.0
        /** room above and below the items inside the well 0..32 step 1 */
        strip_pad: 6.0
        /** past the last item comes the first */
        loop_items: false
        /** which item leads when the strip is first drawn 0..64 step 1 */
        default_page: 0

        /** arrows at the two sides */
        show_arrows: true
        /** the room one arrow takes, in pixels 12..80 step 2 */
        arrow_width: 26.0
        /** the arrow glyph's size in pixels 8..48 step 1 */
        arrow_size: 20.0
        /** the arrow back through the strip */
        glyph_prev: "\u{2039}"
        /** the arrow on through the strip */
        glyph_next: "\u{203a}"

        /** dots under the strip, one per place it can stop */
        show_dots: true
        /** the room the dot row takes, in pixels 0..48 step 1 */
        band_height: 20.0
        /** one dot's box in pixels 4..24 step 1 */
        dot_size: 9.0
        /** room between two dots in pixels 2..24 step 1 */
        dot_gap: 6.0
        /** the dot for the place the strip is resting on */
        dot_glyph: "\u{25cf}"
        /** the dot for a place it is not */
        dot_glyph_off: "\u{25cb}"

        /** the ground of an item that names no colour of its own */
        color_card: theme.color_surface_container_high
        /** the arrow ink */
        color_arrow: theme.color_label_outer
        /** the arrow ink when the strip cannot go that way */
        color_arrow_off: theme.color_outline
        /** the dot for the place the strip is on */
        color_dot_on: theme.color_primary
        /** the dot for a place it is not */
        color_dot: theme.color_outline

        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_detail +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_arrow +: {
            text_style: theme.font_regular{font_size: 20.0, line_spacing: 1.0}
        }
        draw_dot +: {
            text_style: theme.font_regular{font_size: 9.0, line_spacing: 1.0}
        }

        draw_item +: {
            /** bevel thickness in pixels 0..4 step 0.5 */
            border_size: uniform(theme.beveling)
            /** corner rounding radius 0..24 step 0.5 */
            border_radius: uniform(theme.container_corner_radius)
            border_color: uniform(theme.color_outline)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(
                    self.border_size * 0.5
                    self.border_size * 0.5
                    max(self.rect_size.x - self.border_size, 1.0)
                    max(self.rect_size.y - self.border_size, 1.0)
                    self.border_radius
                )
                // The card lifts under the pointer, since it is a press
                // target and not decoration.
                sdf.fill_keep(self.color + vec4(0.06, 0.06, 0.06, 0.0) * self.hot)
                sdf.stroke(self.border_color, self.border_size)
                return sdf.result
            }
        }

        draw_bg +: {
            /** pointer-hover mix 0..1 step 0.01 */
            hover: instance(0.0)
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: instance(0.0)
            /** dragging mix 0..1 step 0.01 */
            drag: instance(0.0)
            /** disabled mix 0..1 step 0.01 */
            disabled: instance(0.0)

            /** bevel thickness in pixels 0..4 step 0.5 */
            border_size: uniform(theme.beveling)
            /** corner rounding radius 0..24 step 0.5 */
            border_radius: uniform(theme.corner_radius)

            color: uniform(theme.color_inset)
            color_hover: uniform(theme.color_inset_hover)
            color_focus: uniform(theme.color_inset_focus)
            color_drag: uniform(theme.color_inset_drag)
            color_disabled: uniform(theme.color_inset_disabled)

            border_color: uniform(theme.color_bevel_outset_2)
            border_color_hover: uniform(theme.color_bevel_outset_2)
            border_color_focus: uniform(theme.color_bevel_outset_2)
            border_color_drag: uniform(theme.color_bevel_outset_2)
            border_color_disabled: uniform(theme.color_bevel_outset_2_disabled)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let fill = self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_hover.mix(self.color_drag, self.drag), self.hover)
                    .mix(self.color_disabled, self.disabled)
                let stroke = self.border_color
                    .mix(self.border_color_focus, self.focus)
                    .mix(self.border_color_hover.mix(self.border_color_drag, self.drag), self.hover)
                    .mix(self.border_color_disabled, self.disabled)

                // The well stops above the dots: the dot row belongs to
                // whatever the carousel is standing on, not to the strip.
                // The band height comes from Rust, which is also what the
                // dots are laid out with, so the two cannot drift apart.
                sdf.box(
                    self.border_size
                    self.border_size
                    self.rect_size.x - self.border_size * 2.
                    max(self.rect_size.y - self.band_px - self.border_size * 2., 1.0)
                    self.border_radius
                )
                sdf.fill_keep(fill)
                sdf.stroke(stroke, self.border_size)

                return sdf.result
            }
        }

        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {hover: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {hover: 1.0}}
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {draw_bg: {focus: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {focus: 1.0}}
                }
            }
            drag: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {drag: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {drag: 1.0}}
                }
            }
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {disabled: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {disabled: 1.0}}
                }
            }
        }
    }

    /** The standard carousel: the flat face plus the theme's inset bevel. */
    mod.widgets.Carousel = set_type_default() do mod.widgets.CarouselFlat{
        draw_bg +: {
            border_color: theme.color_bevel_inset_1
            border_color_hover: theme.color_bevel_inset_1_hover
            border_color_focus: theme.color_bevel_inset_1_focus
            border_color_drag: theme.color_bevel_inset_1_drag
            border_color_disabled: theme.color_bevel_inset_1_disabled
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCarousel {
    #[deref]
    draw_super: DrawQuad,
    /// The height of the dot row, so the well can stop above it. Rust lays
    /// the dots out, so Rust owns the number.
    #[live]
    band_px: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCarouselItem {
    #[deref]
    draw_super: DrawQuad,
    /// This card's ground, which the item may name for itself.
    #[live]
    color: Vec4f,
    /// Whether the pointer is on this card.
    #[live]
    hot: f32,
}

/// One item in the strip.
#[derive(Script, ScriptHook, Default)]
pub struct CarouselItem {
    #[source]
    source: ScriptObjectRef,
    /// The line across the middle of the card.
    #[live]
    pub text: String,
    /// A quieter second line under it, or nothing.
    #[live]
    pub detail: String,
    /// This card's ground. A fully transparent colour — which is what an
    /// item that names none has — takes the carousel's `color_card`, so a
    /// strip is uniform until an item asks not to be.
    #[live]
    pub color: Vec4f,
}

/// The shortest and longest a glide may take, in seconds, and how close to
/// its target it must come to be over. These match the drum picker's: the
/// two widgets settle with the same gesture and there is no reason for one
/// to arrive at a different speed from the other.
const GLIDE_SECS: (f64, f64) = (0.14, 0.75);

/// A glide is over once it is this close to its target, in items. An
/// exponential approach never actually arrives.
const GLIDE_DONE: f64 = 0.002;

/// How much of an item may hang outside the viewport and still count as
/// inside it, in pixels: an item flush against the edge is in, and a
/// rounding sliver is not a cut.
const EDGE_SLACK: f64 = 0.5;

/// No viewport needs more items drawn than this. Only reachable by asking
/// for a strip far wider than its items, and a short row in that case is a
/// better outcome than a frame that never ends.
const ITEMS_MAX: usize = 256;

/// The item width a `Fit` carousel falls back to. A carousel divides the
/// width it is given between its items; `Fit` gives it nothing to divide,
/// and collapsing to nothing would be worse than a nominal card.
const FIT_ITEM_W: f64 = 200.0;

/// How far below the top of its line box a glyph's ink starts, as a share
/// of the font size. `draw_abs` takes the line box; centring the line box
/// leaves the ink riding high.
const INK_DROP: f64 = 0.30;

/// The room between the caption and the detail line, as a share of the
/// caption's size.
const LINE_GAP: f64 = 0.35;

/// The strip's position, in items rather than in pixels, and the rules it
/// follows: where it may rest, which item leads there, and where a flick
/// leaves it.
///
/// It is a separate type for three reasons — it can be tested without a
/// script heap, the draw and the hit test are handed the same numbers from
/// the same place, and the position survives a resize. A position in pixels
/// would not: the pitch changes with the width, and a strip that was
/// resting on item four would come back from a resize standing between two
/// of them.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Track {
    count: usize,
    /// Items sharing the viewport at one time.
    per_view: usize,
    /// Past the last item comes the first.
    looping: bool,
}

impl Track {
    fn new(count: usize, per_view: usize, looping: bool) -> Self {
        Self { count, per_view: per_view.max(1), looping: looping && count > 0 }
    }

    /// How many places the strip may come to rest — which is how many dots
    /// there are.
    ///
    /// With ends, the last stop is the one that brings the final item flush
    /// against the trailing edge, so a strip of six seen three at a time has
    /// four stops and not six. A strip that comes round can lead with any of
    /// its items, so there the two counts agree.
    fn stops(&self) -> usize {
        if self.count == 0 {
            0
        } else if self.looping {
            self.count
        } else {
            self.count.saturating_sub(self.per_view) + 1
        }
    }

    fn last_stop(&self) -> f64 {
        self.stops().saturating_sub(1) as f64
    }

    /// Hold an offset inside what the strip allows.
    ///
    /// A looping strip keeps its raw travel and is only reduced modulo a
    /// turn when something asks which item is leading. Storing the wrapped
    /// offset instead breaks every motion that crosses the seam: a glide
    /// from just under a turn to just over it becomes a glide backwards
    /// through the whole strip.
    ///
    /// A strip with ends stops dead at them. A rubber band belongs to a
    /// list, whose edge is far from most of its content; on a strip of a
    /// handful of cards the stretch is most of the control and reads as the
    /// carousel having lost its place.
    fn bound(&self, offset: f64) -> f64 {
        if self.count == 0 {
            0.0
        } else if self.looping {
            offset
        } else {
            offset.clamp(0.0, self.last_stop())
        }
    }

    /// The stop this offset names — the dot that lights.
    fn page_at(&self, offset: f64) -> usize {
        if self.count == 0 {
            return 0;
        }
        let i = self.bound(offset).round();
        if self.looping {
            (i as i64).rem_euclid(self.count as i64) as usize
        } else {
            (i.max(0.0) as usize).min(self.stops() - 1)
        }
    }

    /// The nearest offset at which the strip rests on whole items.
    fn snap(&self, offset: f64) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.bound(offset.round())
        }
    }

    /// Where a strip now at `offset` and carrying `travel` more items comes
    /// to rest. A looping strip keeps the whole travel, so a hard flick
    /// carries it round more than once; a strip with ends stops at them.
    fn landing(&self, offset: f64, travel: f64) -> f64 {
        self.snap(offset + travel)
    }

    /// The offset that puts `page` at the leading edge, in the same
    /// unwrapped frame the strip is standing in. A looping strip takes the
    /// short way round rather than unwinding.
    fn reach(&self, from: f64, page: usize) -> f64 {
        let target = page as f64;
        if self.looping {
            nearest_turn(from, target, self.count as f64)
        } else {
            self.bound(target)
        }
    }

    /// Move a stop by `delta`: round the ends on a looping strip, stop at
    /// them otherwise.
    fn step(&self, page: usize, delta: i64) -> usize {
        if self.count == 0 {
            return 0;
        }
        let raw = page as i64 + delta;
        if self.looping {
            raw.rem_euclid(self.count as i64) as usize
        } else {
            raw.clamp(0, self.stops() as i64 - 1) as usize
        }
    }

    /// Whether the strip has anywhere left to go that way, which is what
    /// decides whether an arrow is lit or dim.
    fn can_go(&self, offset: f64, back: bool) -> bool {
        if self.stops() <= 1 {
            return false;
        }
        if self.looping {
            return true;
        }
        if back {
            offset > GLIDE_DONE
        } else {
            offset < self.last_stop() - GLIDE_DONE
        }
    }

    /// The items showing in a band that opens `lead` items before the
    /// leading edge and runs `span` items wide, each with its leading edge
    /// measured in items from the strip's own.
    ///
    /// `lead` is negative when the viewport shows a peek of the neighbour
    /// behind, which is exactly how the item before the first one gets into
    /// the list on a strip that comes round.
    fn items_in_view(&self, offset: f64, lead: f64, span: f64) -> Vec<(usize, f64)> {
        let mut out = Vec::new();
        if self.count == 0 || span <= 0.0 || !span.is_finite() || !offset.is_finite() {
            return out;
        }
        let first = (offset + lead).floor() as i64;
        let last = ((offset + lead + span).ceil() as i64).min(first + ITEMS_MAX as i64);
        for slot in first..=last {
            let index = if self.looping {
                slot.rem_euclid(self.count as i64) as usize
            } else if slot < 0 || slot >= self.count as i64 {
                continue;
            } else {
                slot as usize
            };
            out.push((index, slot as f64 - offset));
        }
        out
    }
}

/// The representative of `target` nearest `from` on a strip that comes
/// round, so a move across the seam is the short way rather than the long
/// way back through every item.
fn nearest_turn(from: f64, target: f64, turn: f64) -> f64 {
    if turn <= 0.0 {
        return target;
    }
    let mut d = target - from;
    d -= (d / turn).round() * turn;
    from + d
}

/// Where the parts sit inside the widget's rect, in the widget's own
/// coordinates. Built fresh from the laid-out rect for every draw and every
/// press, so the arrow the eye sees is the arrow the finger finds.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Frame {
    w: f64,
    h: f64,
    /// The room one arrow takes; zero when there are none. It does not
    /// change with the content: an arrow that vanished when the strip ran
    /// out would move everything else sideways at the worst moment.
    arrow: f64,
    /// The room the dot row takes; zero when there are none.
    band: f64,
    peek: f64,
    gap: f64,
    per_view: usize,
    dot: f64,
    dot_gap: f64,
}

impl Frame {
    /// The strip, which is everything above the dots.
    fn strip_h(&self) -> f64 {
        (self.h - self.band).max(0.0)
    }

    /// The viewport: the strip without the arrows. Items are clipped to it.
    fn view_x(&self) -> f64 {
        self.arrow
    }

    fn view_w(&self) -> f64 {
        (self.w - self.arrow * 2.0).max(0.0)
    }

    /// The band the items are laid out in: the viewport without the peeks.
    fn inner_x(&self) -> f64 {
        self.view_x() + self.peek
    }

    fn inner_w(&self) -> f64 {
        (self.view_w() - self.peek * 2.0).max(0.0)
    }

    /// One item's drawn width: what is left of the inner band once the gaps
    /// between the items on show are taken out of it.
    fn item_w(&self) -> f64 {
        let n = self.per_view.max(1) as f64;
        ((self.inner_w() - self.gap * (n - 1.0)) / n).max(1.0)
    }

    /// An item plus the room after it: the unit of travel and of snapping.
    fn pitch(&self) -> f64 {
        self.item_w() + self.gap
    }

    /// How far before the leading edge the viewport starts showing, in
    /// items: the peek, and nothing when there is none.
    fn lead(&self) -> f64 {
        -self.peek / self.pitch()
    }

    /// How much of the strip the viewport shows, in items.
    fn span(&self) -> f64 {
        self.view_w() / self.pitch()
    }

    fn dots_w(&self, stops: usize) -> f64 {
        if stops == 0 {
            0.0
        } else {
            stops as f64 * self.dot + (stops - 1) as f64 * self.dot_gap
        }
    }

    /// Whether the dot row still fits across the widget. Past that the dots
    /// are dropped rather than crowded or clipped: thirty dots is not a
    /// control anybody can aim at, and the arrows carry the strip. A single
    /// dot goes too — a row of one says nothing about a strip that cannot
    /// move anyway.
    fn dots_fit(&self, stops: usize) -> bool {
        self.band > 0.0 && stops > 1 && self.dots_w(stops) <= self.w
    }

    /// The left edge of dot `i`.
    fn dot_x(&self, i: usize, stops: usize) -> f64 {
        (self.w - self.dots_w(stops)) * 0.5 + i as f64 * (self.dot + self.dot_gap)
    }

    /// The dot a press at `x` takes, if any.
    ///
    /// The room between two dots belongs to the nearer one, and the row is
    /// widened by half a gap at each end. A dot is nine points across and
    /// nobody aims at nine points.
    fn dot_at(&self, x: f64, stops: usize) -> Option<usize> {
        if stops == 0 {
            return None;
        }
        let first = self.dot_x(0, stops);
        if x < first - self.dot_gap * 0.5 || x > first + self.dots_w(stops) + self.dot_gap * 0.5 {
            return None;
        }
        let pitch = self.dot + self.dot_gap;
        let i = ((x - first - self.dot * 0.5) / pitch).round().max(0.0) as usize;
        Some(i.min(stops - 1))
    }

    /// What a press at this point takes hold of.
    fn target_at(&self, x: f64, y: f64, dot_stops: usize) -> Target {
        if self.band > 0.0 && y >= self.strip_h() {
            return self.dot_at(x, dot_stops).map_or(Target::None, Target::Dot);
        }
        if self.arrow > 0.0 {
            if x < self.arrow {
                return Target::Prev;
            }
            if x > self.w - self.arrow {
                return Target::Next;
            }
        }
        Target::Strip
    }
}

/// Whether an item whose leading edge is `at` items from the strip's own is
/// wholly inside a viewport `per_view` items wide, with `slack` items of
/// tolerance at each edge.
fn wholly_inside(at: f64, per_view: usize, slack: f64) -> bool {
    at >= -slack && at + 1.0 <= per_view as f64 + slack
}

/// Where a strip released at `velocity` items/s stops if its speed decays
/// by `decay_per_ms` every millisecond: the whole remaining travel of
/// `v(t) = v0 * decay^t` is `v0 / lambda`, with `lambda = -ln(decay) * 1000`.
///
/// The library's scrollers integrate that same decay frame by frame. A
/// strip that has to land on an item only needs to know where it ends,
/// because the landing place decides the whole motion.
fn spin_travel(velocity: f64, decay_per_ms: f64) -> f64 {
    let lambda = -decay_per_ms.ln() * 1000.0;
    if lambda <= 0.0 || !lambda.is_finite() {
        0.0
    } else {
        velocity / lambda
    }
}

/// The time constant of a glide covering `distance` from a standing start
/// of `velocity`. Matching the opening speed to the finger is what keeps
/// the hand-off from the drag invisible.
fn glide_secs(distance: f64, velocity: f64) -> f64 {
    let v = velocity.abs();
    if v <= f64::EPSILON {
        return GLIDE_SECS.0;
    }
    (distance.abs() / v).clamp(GLIDE_SECS.0, GLIDE_SECS.1)
}

/// The strip's position `t` seconds into a glide from `from` to `to`.
fn glide_at(from: f64, to: f64, tau: f64, t: f64) -> f64 {
    to + (from - to) * (-t / tau.max(f64::EPSILON)).exp()
}

/// What a wheel or trackpad event moves the strip, in items.
///
/// Sideways deltas are the strip's own. A plain vertical wheel is not: it
/// belongs to whatever the carousel is standing in, and a strip that
/// swallowed it would trap the page under it — the reader scrolls, the page
/// stops, and the only way out is to aim around the carousel. Shift is the
/// usual way to say "sideways" on a mouse with one wheel.
///
/// A notch is worth at most one item whatever size the operating system
/// says it is: notches run from a few pixels to most of a screen, and a
/// strip that jumps four cards on one notch cannot be aimed.
fn wheel_items(scroll_x: f64, scroll_y: f64, shift: bool, pitch: f64) -> f64 {
    let delta = if scroll_x.abs() > scroll_y.abs() {
        scroll_x
    } else if shift {
        scroll_y
    } else {
        0.0
    };
    (delta / pitch.max(1.0)).clamp(-1.0, 1.0)
}

/// What a press has hold of.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
enum Target {
    Prev,
    Next,
    Dot(usize),
    Strip,
    #[default]
    None,
}

/// The finger's hold on the strip.
#[derive(Copy, Clone, Debug)]
struct Grab {
    /// The offset and the finger position when the press landed, so the
    /// strip tracks the finger instead of jumping to it.
    offset: f64,
    abs: f64,
    /// Whether the press caught a strip that was still moving. Catching is
    /// a stop, so such a press must not also count as a press on a card.
    caught: bool,
}

/// The glide that carries the strip to the item it will rest on.
#[derive(Clone, Copy, Debug)]
struct Glide {
    from: f64,
    to: f64,
    tau: f64,
    /// The jitter-clamped frame clock the scrollers use, so a late frame
    /// becomes a slightly uneven step rather than a visible jerk.
    clock: FrameClock,
}

#[derive(Clone, Debug, Default)]
pub enum CarouselAction {
    /// The strip is now leading with this stop. Reported while it is still
    /// moving, not only once it settles.
    Paged(usize),
    /// The motion is over and the strip is resting on this stop.
    Settled(usize),
    /// An item wholly inside the viewport was pressed.
    Pressed(usize),
    #[default]
    None,
}

#[derive(Script, Widget, Animator)]
pub struct Carousel {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_bg: DrawCarousel,
    #[live]
    draw_item: DrawCarouselItem,
    /// The caption on a card, the detail line under it, the arrows and the
    /// dots. Four text layers rather than one because each carries its own
    /// size and ink and they are drawn interleaved.
    #[live]
    draw_text: DrawText,
    #[live]
    draw_detail: DrawText,
    #[live]
    draw_arrow: DrawText,
    #[live]
    draw_dot: DrawText,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[apply_default]
    animator: Animator,

    /// The items, leading edge first.
    #[live]
    pub items: Vec<CarouselItem>,

    #[live(1)]
    pub per_view: usize,
    #[live]
    pub peek: f64,
    #[live(8.0)]
    pub gap: f64,
    #[live(120.0)]
    pub item_height: f64,
    #[live(6.0)]
    pub strip_pad: f64,
    #[live]
    pub loop_items: bool,
    /// Which stop the strip starts on. The position itself is not a live
    /// property: a live reload would put the strip back to what the file
    /// said and throw away where the reader had got to.
    #[live]
    pub default_page: usize,

    #[live(true)]
    pub show_arrows: bool,
    #[live(26.0)]
    pub arrow_width: f64,
    #[live(20.0)]
    pub arrow_size: f64,
    #[live("\u{2039}".to_string())]
    pub glyph_prev: String,
    #[live("\u{203a}".to_string())]
    pub glyph_next: String,

    #[live(true)]
    pub show_dots: bool,
    #[live(20.0)]
    pub band_height: f64,
    #[live(9.0)]
    pub dot_size: f64,
    #[live(6.0)]
    pub dot_gap: f64,
    #[live("\u{25cf}".to_string())]
    pub dot_glyph: String,
    #[live("\u{25cb}".to_string())]
    pub dot_glyph_off: String,

    #[live]
    pub color_card: Vec4f,
    #[live]
    pub color_arrow: Vec4f,
    #[live]
    pub color_arrow_off: Vec4f,
    #[live]
    pub color_dot: Vec4f,
    #[live]
    pub color_dot_on: Vec4f,

    /// Where the strip stands, in items from the first one.
    #[rust]
    offset: f64,
    /// The stop last reported, so a move that changes nothing says nothing.
    #[rust]
    page: usize,
    /// Whether the running state has been built from the items as they
    /// stand.
    #[rust]
    ready: bool,
    #[rust]
    samples: Vec<ScrollSample>,
    #[rust]
    glide: Option<Glide>,
    #[rust]
    grab: Option<Grab>,
    /// What the pointer is over, so a press can be anticipated rather than
    /// discovered.
    #[rust]
    hot: Target,
    #[rust]
    hot_item: Option<usize>,
    #[rust]
    next_frame: NextFrame,
}

impl ScriptHook for Carousel {
    fn on_after_new(&mut self, _vm: &mut ScriptVm) {
        // Dropping the running state forces it to be rebuilt from the items
        // as they now stand, which is what a re-apply means.
        self.ready = false;
        self.grab = None;
        self.glide = None;
        self.samples.clear();
    }
}

impl Carousel {
    fn track(&self) -> Track {
        Track::new(self.items.len(), self.per_view, self.loop_items)
    }

    /// The frame for a rect this size. Every number in it is a live
    /// property and the tweaker may have moved any of them since the last
    /// draw, so it is built fresh rather than kept.
    fn frame(&self, w: f64, h: f64) -> Frame {
        Frame {
            w,
            h,
            arrow: if self.show_arrows { self.arrow_width.max(0.0) } else { 0.0 },
            band: if self.show_dots { self.band_height.max(0.0) } else { 0.0 },
            peek: self.peek.max(0.0),
            gap: self.gap.max(0.0),
            per_view: self.per_view.max(1),
            dot: self.dot_size.max(1.0),
            dot_gap: self.dot_gap.max(0.0),
        }
    }

    /// Match the running state to the items. Cheap when nothing has
    /// changed, which is every frame but the first after an apply.
    fn sync(&mut self) {
        if self.ready {
            return;
        }
        self.ready = true;
        let track = self.track();
        self.offset = track.bound(self.default_page as f64);
        self.page = track.page_at(self.offset);
        self.hot = Target::None;
        self.hot_item = None;
    }

    /// The item under `x`, how far its leading edge is from the strip's, and
    /// whether the viewport is showing all of it.
    fn item_at(&self, frame: &Frame, x: f64) -> Option<(usize, f64, bool)> {
        let pitch = frame.pitch();
        let slack = EDGE_SLACK / pitch;
        for (index, at) in self.track().items_in_view(self.offset, frame.lead(), frame.span()) {
            let left = frame.inner_x() + at * pitch;
            if x >= left && x < left + frame.item_w() {
                return Some((index, at, wholly_inside(at, frame.per_view, slack)));
            }
        }
        None
    }

    /// Put the strip at `offset` and say so if that changed the leading
    /// stop.
    ///
    /// The action fires while the strip is still moving, not only once it
    /// settles: the dots have to follow the motion, and so does a readout
    /// beside them. `Settled` is the one to listen to for anything
    /// expensive.
    fn place(&mut self, cx: &mut Cx, offset: f64) {
        let track = self.track();
        self.offset = track.bound(offset);
        let page = track.page_at(self.offset);
        if page != self.page {
            self.page = page;
            cx.widget_action(self.uid, CarouselAction::Paged(page));
        }
        self.draw_bg.redraw(cx);
    }

    /// Send the strip to `target`, opening at the speed a flick released at
    /// `velocity` items/s would have. `target` is in the same unwrapped
    /// frame the strip is standing in.
    fn glide_to(&mut self, cx: &mut Cx, target: f64, velocity: f64) {
        let from = self.offset;
        if (target - from).abs() <= GLIDE_DONE {
            self.glide = None;
            self.place(cx, target);
            cx.widget_action(self.uid, CarouselAction::Settled(self.page));
            return;
        }
        self.glide = Some(Glide {
            from,
            to: target,
            tau: glide_secs(target - from, velocity),
            clock: FrameClock::default(),
        });
        self.next_frame = cx.new_next_frame();
        self.draw_bg.redraw(cx);
    }

    /// One frame of a glide in flight.
    fn tick(&mut self, cx: &mut Cx, time: f64) {
        let Some(mut glide) = self.glide else {
            return;
        };
        let t = glide.clock.advance(time);
        let at = glide_at(glide.from, glide.to, glide.tau, t);
        let done = (at - glide.to).abs() <= GLIDE_DONE;
        self.glide = if done { None } else { Some(glide) };
        self.place(cx, if done { glide.to } else { at });
        if done {
            cx.widget_action(self.uid, CarouselAction::Settled(self.page));
        } else {
            self.next_frame = cx.new_next_frame();
        }
    }

    /// The stop the strip is leading with.
    pub fn page(&self) -> usize {
        self.track().page_at(self.offset)
    }

    pub fn stops(&self) -> usize {
        self.track().stops()
    }

    /// Move by whole stops, with the glide the arrows and the keyboard use.
    pub fn advance(&mut self, cx: &mut Cx, delta: i64) {
        self.sync();
        let track = self.track();
        if track.stops() <= 1 {
            return;
        }
        let page = track.step(track.page_at(self.offset), delta);
        let target = track.reach(self.offset, page);
        if (target - self.offset).abs() <= GLIDE_DONE {
            return;
        }
        self.glide = None;
        self.glide_to(cx, target, 0.0);
    }

    /// Glide to a stop, the way pressing its dot does.
    pub fn go_to(&mut self, cx: &mut Cx, page: usize) {
        self.sync();
        let track = self.track();
        if track.stops() == 0 {
            return;
        }
        let page = page.min(track.stops() - 1);
        let target = track.reach(self.offset, page);
        if (target - self.offset).abs() <= GLIDE_DONE {
            return;
        }
        self.glide = None;
        self.glide_to(cx, target, 0.0);
    }

    /// Put a stop at the leading edge with no motion to watch. Setting a
    /// position is not a gesture, and animating one nobody asked for makes
    /// a host that writes a carousel and a list at once look like it is
    /// fighting itself.
    pub fn set_page(&mut self, cx: &mut Cx, page: usize) {
        self.sync();
        let track = self.track();
        if track.stops() == 0 {
            return;
        }
        let page = page.min(track.stops() - 1);
        self.glide = None;
        // Silent: the host already knows, and answering its own write with
        // a Paged is how a readout and a carousel end up writing to each
        // other for as long as the frame lasts.
        self.offset = track.reach(self.offset, page);
        self.page = page;
        self.draw_bg.redraw(cx);
    }
}

impl Widget for Carousel {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.animator_toggle(cx, disabled, Animate::Yes, ids!(disabled.on), ids!(disabled.off));
    }

    fn disabled(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(disabled.on))
    }

    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(set_page) {
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                if let Some(page) = vm.bx.heap.vec_value(args_obj, 0, trap).as_f64() {
                    vm.with_cx_mut(|cx| self.set_page(cx, page.max(0.0) as usize));
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(go_to) {
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                if let Some(page) = vm.bx.heap.vec_value(args_obj, 0, trap).as_f64() {
                    vm.with_cx_mut(|cx| self.go_to(cx, page.max(0.0) as usize));
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(next) {
            vm.with_cx_mut(|cx| self.advance(cx, 1));
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(prev) {
            vm.with_cx_mut(|cx| self.advance(cx, -1));
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(page) {
            return ScriptAsyncResult::Return(ScriptValue::from_f64(self.page() as f64));
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.animator_handle_event(cx, event);
        self.sync();

        if let Some(ne) = self.next_frame.is_event(event) {
            self.tick(cx, ne.time);
        }
        if self.animator_in_state(cx, ids!(disabled.on)) || self.items.is_empty() {
            return;
        }

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                if self.hot != Target::None || self.hot_item.is_some() {
                    self.hot = Target::None;
                    self.hot_item = None;
                    self.draw_bg.redraw(cx);
                }
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerHoverOver(fe) => {
                let frame = self.frame(fe.rect.size.x, fe.rect.size.y);
                let stops = self.track().stops();
                let dot_stops = if frame.dots_fit(stops) { stops } else { 0 };
                let x = fe.abs.x - fe.rect.pos.x;
                let y = fe.abs.y - fe.rect.pos.y;
                let hot = frame.target_at(x, y, dot_stops);
                let hot_item = if hot == Target::Strip {
                    self.item_at(&frame, x).map(|(index, _, _)| index)
                } else {
                    None
                };
                if (hot, hot_item) != (self.hot, self.hot_item) {
                    self.hot = hot;
                    self.hot_item = hot_item;
                    self.draw_bg.redraw(cx);
                }
                cx.set_cursor(match hot {
                    Target::Strip => MouseCursor::Grab,
                    Target::None => MouseCursor::Default,
                    _ => MouseCursor::Hand,
                });
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                let frame = self.frame(fe.rect.size.x, fe.rect.size.y);
                let stops = self.track().stops();
                let dot_stops = if frame.dots_fit(stops) { stops } else { 0 };
                let x = fe.abs.x - fe.rect.pos.x;
                let y = fe.abs.y - fe.rect.pos.y;
                match frame.target_at(x, y, dot_stops) {
                    Target::Prev => self.advance(cx, -1),
                    Target::Next => self.advance(cx, 1),
                    Target::Dot(page) => self.go_to(cx, page),
                    Target::None => {}
                    Target::Strip => {
                        // A hand on a moving strip stops it where it stands.
                        let caught = self.glide.take().is_some();
                        self.samples.clear();
                        push_sample(&mut self.samples, fe.abs.x, fe.time);
                        self.grab = Some(Grab { offset: self.offset, abs: fe.abs.x, caught });
                        self.animator_play(cx, ids!(drag.on));
                        self.draw_bg.redraw(cx);
                    }
                }
            }
            Hit::FingerMove(fe) => {
                let Some(grab) = self.grab else {
                    return;
                };
                push_sample(&mut self.samples, fe.abs.x, fe.time);
                let frame = self.frame(fe.rect.size.x, fe.rect.size.y);
                // The finger and the strip run opposite ways: dragging right
                // brings earlier items back into the viewport.
                self.place(cx, grab.offset - (fe.abs.x - grab.abs) / frame.pitch());
            }
            Hit::FingerUp(fe) => {
                let Some(grab) = self.grab.take() else {
                    return;
                };
                self.animator_play(cx, ids!(drag.off));
                if fe.is_over && fe.device.has_hovers() {
                    self.animator_play(cx, ids!(hover.on));
                } else {
                    self.animator_play(cx, ids!(hover.off));
                }
                let frame = self.frame(fe.rect.size.x, fe.rect.size.y);
                let pitch = frame.pitch();
                // A press taken away chooses nothing and throws nothing: the
                // strip settles from rest on the nearest stop.
                let (velocity, travel) = if fe.cancelled {
                    (0.0, 0.0)
                } else {
                    estimate_release_velocity(&self.samples)
                };
                // The finger's velocity is the strip's, negated, and the
                // strip counts in items rather than in pixels.
                let items_per_s = -velocity / pitch;
                let carry = if fe.cancelled {
                    0.0
                } else if travel.abs() > FLING_MIN_TOTAL_DELTA {
                    spin_travel(items_per_s, FLING_DECEL_RATE_PER_MS)
                } else if grab.caught {
                    0.0
                } else {
                    // A press that moved nothing and stopped nothing is a
                    // choice — unless it landed on an item the edge is
                    // cutting, which is being pointed at rather than chosen.
                    match self.item_at(&frame, fe.abs.x - fe.rect.pos.x) {
                        Some((index, _, true)) => {
                            cx.widget_action(self.uid, CarouselAction::Pressed(index));
                            0.0
                        }
                        Some((_, at, false)) => at,
                        None => 0.0,
                    }
                };
                let target = self.track().landing(self.offset, carry);
                // A press that chose a card left the strip exactly where it
                // was and has nothing to settle. Saying otherwise would set
                // every host that reloads on `Settled` reloading on a press.
                let dragged = (self.offset - grab.offset).abs() > GLIDE_DONE;
                if dragged || (target - self.offset).abs() > GLIDE_DONE {
                    self.glide_to(cx, target, items_per_s);
                }
            }
            Hit::KeyFocus(_) => {
                self.animator_play(cx, ids!(focus.on));
                self.draw_bg.redraw(cx);
            }
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, ids!(focus.off));
                self.draw_bg.redraw(cx);
            }
            Hit::KeyDown(ke) => {
                let stops = self.track().stops();
                match ke.key_code {
                    KeyCode::ArrowLeft => self.advance(cx, -1),
                    KeyCode::ArrowRight => self.advance(cx, 1),
                    // Home and End are the first and last item of the strip,
                    // not of the travel: on a strip that comes round, the
                    // last item may well be one step backwards.
                    KeyCode::Home => self.go_to(cx, 0),
                    KeyCode::End => self.go_to(cx, stops.saturating_sub(1)),
                    _ => {}
                }
            }
            Hit::FingerScroll(e) => {
                let frame = self.frame(e.rect.size.x, e.rect.size.y);
                let items =
                    wheel_items(e.scroll.x, e.scroll.y, e.modifiers.shift, frame.pitch());
                if items != 0.0 {
                    self.glide = None;
                    let at = self.offset + items;
                    let target = self.track().landing(at, 0.0);
                    self.place(cx, at);
                    self.glide_to(cx, target, 0.0);
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.sync();
        let track = self.track();
        let stops = track.stops();

        // A carousel divides the width it is given between its items, so a
        // `Fit` width has nothing to divide and falls back to a nominal
        // card rather than collapsing. The height is the cards plus the dot
        // row, which is a real natural size.
        let arrows = if self.show_arrows { self.arrow_width.max(0.0) } else { 0.0 };
        let band = if self.show_dots { self.band_height.max(0.0) } else { 0.0 };
        let n = self.per_view.max(1) as f64;
        let nominal_w =
            n * FIT_ITEM_W + (n - 1.0) * self.gap + self.peek * 2.0 + arrows * 2.0;
        let nominal_h = self.item_height + self.strip_pad * 2.0 + band;
        let walk = sized(walk, nominal_w, nominal_h);

        self.draw_bg.band_px = band as f32;
        self.draw_bg.begin(cx, walk, self.layout);
        let rect = cx.turtle().rect();
        let frame = self.frame(rect.size.x, rect.size.y);
        let pitch = frame.pitch();
        let item_w = frame.item_w();
        let strip_h = frame.strip_h();
        let card_h = self.item_height.min((strip_h - self.strip_pad * 2.0).max(1.0));
        let card_y = rect.pos.y + (strip_h - card_h) * 0.5;

        // Clipped to the viewport, always: items run past both its edges by
        // design, and only the arrows' room stands between the strip and
        // the edge of the widget.
        cx.push_clip_rect(Rect {
            pos: dvec2(rect.pos.x + frame.view_x(), rect.pos.y),
            size: dvec2(frame.view_w(), strip_h),
        });
        let title_size = self.draw_text.text_style.font_size as f64;
        let detail_size = self.draw_detail.text_style.font_size as f64;
        for (index, at) in track.items_in_view(self.offset, frame.lead(), frame.span()) {
            // Copied out first: the item lives in `self` and the draw layers
            // want `self` mutably.
            let Some((text, detail, color)) = self
                .items
                .get(index)
                .map(|item| (item.text.clone(), item.detail.clone(), item.color))
            else {
                continue;
            };
            let x = rect.pos.x + frame.inner_x() + at * pitch;
            self.draw_item.color = if color.w > 0.0 { color } else { self.color_card };
            self.draw_item.hot = if self.hot_item == Some(index) { 1.0 } else { 0.0 };
            self.draw_item.draw_abs(
                cx,
                Rect { pos: dvec2(x, card_y), size: dvec2(item_w, card_h) },
            );

            let gap = title_size * LINE_GAP;
            let block = if detail.is_empty() {
                title_size
            } else {
                title_size + gap + detail_size
            };
            let mut ink = card_y + (card_h - block) * 0.5;
            if !text.is_empty() {
                let w = measure(&self.draw_text, cx, &text);
                self.draw_text.draw_abs(
                    cx,
                    dvec2(x + (item_w - w) * 0.5, ink - title_size * INK_DROP),
                    &text,
                );
            }
            ink += title_size + gap;
            if !detail.is_empty() {
                let w = measure(&self.draw_detail, cx, &detail);
                self.draw_detail.draw_abs(
                    cx,
                    dvec2(x + (item_w - w) * 0.5, ink - detail_size * INK_DROP),
                    &detail,
                );
            }
        }
        cx.pop_clip_rect();

        if frame.arrow > 0.0 {
            self.draw_arrow.text_style.font_size = self.arrow_size as f32;
            let y = rect.pos.y + (strip_h - self.arrow_size) * 0.5
                - self.arrow_size * INK_DROP;
            for back in [true, false] {
                let glyph =
                    if back { self.glyph_prev.clone() } else { self.glyph_next.clone() };
                if glyph.is_empty() {
                    continue;
                }
                let lit = track.can_go(self.offset, back);
                self.draw_arrow.color = if lit { self.color_arrow } else { self.color_arrow_off };
                let w = measure(&self.draw_arrow, cx, &glyph);
                let left = if back { 0.0 } else { frame.w - frame.arrow };
                self.draw_arrow.draw_abs(
                    cx,
                    dvec2(rect.pos.x + left + (frame.arrow - w) * 0.5, y),
                    &glyph,
                );
            }
        }

        if frame.dots_fit(stops) {
            self.draw_dot.text_style.font_size = self.dot_size as f32;
            let y = rect.pos.y + strip_h + (frame.band - self.dot_size) * 0.5
                - self.dot_size * INK_DROP;
            let here = track.page_at(self.offset);
            for i in 0..stops {
                let glyph = if i == here {
                    self.dot_glyph.clone()
                } else {
                    self.dot_glyph_off.clone()
                };
                if glyph.is_empty() {
                    continue;
                }
                self.draw_dot.color =
                    if i == here { self.color_dot_on } else { self.color_dot };
                let w = measure(&self.draw_dot, cx, &glyph);
                self.draw_dot.draw_abs(
                    cx,
                    dvec2(rect.pos.x + frame.dot_x(i, stops) + (frame.dot - w) * 0.5, y),
                    &glyph,
                );
            }
        }

        self.draw_bg.end(cx);

        if !self.animator_in_state(cx, ids!(disabled.on)) {
            cx.add_nav_stop(self.draw_bg.area(), NavRole::Slider, Inset::default());
        }
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.items.get(self.page()).map(|item| item.text.clone()).unwrap_or_default()
    }

    /// Where the strip is resting, so a test can ask rather than compare
    /// two pictures of a moving thing.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(format!("{} of {}", self.page() + 1, self.stops()))
    }
}

impl CarouselRef {
    /// The stop the strip is leading with.
    pub fn page(&self) -> usize {
        self.borrow().map(|inner| inner.page()).unwrap_or(0)
    }

    /// How many places the strip can rest, which is how many dots it draws.
    pub fn stops(&self) -> usize {
        self.borrow().map(|inner| inner.stops()).unwrap_or(0)
    }

    /// Put a stop at the leading edge, with no motion to watch.
    pub fn set_page(&self, cx: &mut Cx, page: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_page(cx, page);
        }
    }

    /// Glide to a stop, the way pressing its dot does.
    pub fn go_to(&self, cx: &mut Cx, page: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.go_to(cx, page);
        }
    }

    pub fn next(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.advance(cx, 1);
        }
    }

    pub fn prev(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.advance(cx, -1);
        }
    }

    /// The stop now leading, reported while the strip is still moving. For
    /// the dots and for a readout beside them.
    pub fn paged(&self, actions: &Actions) -> Option<usize> {
        actions
            .filter_widget_actions_cast::<CarouselAction>(self.widget_uid())
            .find_map(|action| match action {
                CarouselAction::Paged(page) => Some(page),
                _ => None,
            })
    }

    /// The stop the motion ended on. For anything expensive — a query, a
    /// picture to fetch — since a flick reports every stop it passes.
    pub fn settled(&self, actions: &Actions) -> Option<usize> {
        actions
            .filter_widget_actions_cast::<CarouselAction>(self.widget_uid())
            .find_map(|action| match action {
                CarouselAction::Settled(page) => Some(page),
                _ => None,
            })
    }

    /// An item wholly inside the viewport that was pressed. An item the
    /// edge is cutting reports nothing: pressing it scrolls it in.
    pub fn pressed(&self, actions: &Actions) -> Option<usize> {
        actions
            .filter_widget_actions_cast::<CarouselAction>(self.widget_uid())
            .find_map(|action| match action {
                CarouselAction::Pressed(index) => Some(index),
                _ => None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(count: usize, per_view: usize, looping: bool) -> Track {
        Track::new(count, per_view, looping)
    }

    /// A frame the size the preset gives it: 600 wide, arrows 26 a side,
    /// one item across, no peek, an 8 point gap. The inner band is then
    /// 548 wide, so an item is 548 and the pitch 556.
    fn frame(per_view: usize, peek: f64) -> Frame {
        Frame {
            w: 600.0,
            h: 152.0,
            arrow: 26.0,
            band: 20.0,
            peek,
            gap: 8.0,
            per_view,
            dot: 9.0,
            dot_gap: 6.0,
        }
    }

    #[test]
    fn the_dots_count_the_places_the_strip_can_stop() {
        // Six items three at a time: four stops, not six. Six dots would
        // include two the strip can never lead with.
        assert_eq!(track(6, 3, false).stops(), 4);
        assert_eq!(track(6, 1, false).stops(), 6);
        assert_eq!(track(6, 6, false).stops(), 1, "everything fits, so there is one place");
        assert_eq!(track(6, 9, false).stops(), 1, "and asking for more than there is is the same");
    }

    #[test]
    fn a_strip_that_comes_round_can_lead_with_any_item() {
        assert_eq!(track(6, 3, true).stops(), 6);
        assert_eq!(track(6, 1, true).stops(), 6);
    }

    #[test]
    fn an_empty_strip_has_no_stops_and_no_dots() {
        let t = track(0, 3, true);
        assert_eq!(t.stops(), 0);
        assert_eq!(t.page_at(7.0), 0);
        assert_eq!(t.snap(7.0), 0.0);
        assert!(t.items_in_view(0.0, -0.5, 4.0).is_empty());
    }

    #[test]
    fn the_dot_that_lights_is_the_stop_the_offset_names() {
        let t = track(8, 2, false);
        assert_eq!(t.page_at(0.0), 0);
        assert_eq!(t.page_at(3.0), 3);
        // Just under half an item either side still belongs to the stop it
        // is nearest.
        assert_eq!(t.page_at(3.49), 3);
        assert_eq!(t.page_at(2.51), 3);
        assert_eq!(t.page_at(3.51), 4);
        // And past the last stop it is the last stop, not the last item.
        assert_eq!(t.page_at(99.0), 6);
    }

    #[test]
    fn every_landing_leaves_the_strip_on_a_whole_item() {
        let t = track(20, 3, false);
        for travel in [-40.0, -1.4, 0.0, 0.3, 2.7, 40.0] {
            let landing = t.landing(5.0, travel);
            assert_eq!(landing.fract(), 0.0, "travel {travel} left the strip between items");
        }
    }

    #[test]
    fn a_strip_with_ends_stops_at_them() {
        let t = track(6, 3, false);
        assert_eq!(t.bound(-9.0), 0.0);
        assert_eq!(t.bound(9.0), 3.0, "the last item ends flush with the trailing edge");
        assert_eq!(t.landing(3.0, 20.0), 3.0);
        assert_eq!(t.step(0, -1), 0, "the first stop has nothing behind it");
        assert_eq!(t.step(3, 1), 3, "nor the last anything ahead");
        assert!(!t.can_go(0.0, true));
        assert!(t.can_go(0.0, false));
        assert!(!t.can_go(3.0, false));
    }

    #[test]
    fn a_looping_strip_never_runs_out_of_either_way() {
        let t = track(6, 3, true);
        assert!(t.can_go(0.0, true));
        assert!(t.can_go(5.0, false));
        assert_eq!(t.step(5, 1), 0);
        assert_eq!(t.step(0, -1), 5);
    }

    #[test]
    fn a_strip_with_one_stop_has_nowhere_to_go() {
        let t = track(2, 4, false);
        assert!(!t.can_go(0.0, true));
        assert!(!t.can_go(0.0, false));
    }

    #[test]
    fn a_looping_strip_keeps_its_raw_travel() {
        // The stored offset is never wrapped. Wrapping it would turn a glide
        // across the seam — from just under a turn to just over it — into a
        // glide backwards through the whole strip.
        let t = track(6, 1, true);
        let past = 6.0 + 3.0;
        assert_eq!(t.bound(past), past);
        assert_eq!(t.page_at(past), 3);
        assert_eq!(t.page_at(-1.0), 5);
    }

    #[test]
    fn a_move_across_the_seam_takes_the_short_way() {
        let t = track(10, 1, true);
        // Leading with the last item, the first is one step on, not nine
        // steps back.
        assert_eq!(t.reach(9.0, 0) - 9.0, 1.0);
        assert_eq!(t.reach(0.0, 9) - 0.0, -1.0);
    }

    #[test]
    fn a_strip_with_ends_never_takes_the_short_way() {
        let t = track(10, 1, false);
        assert_eq!(t.reach(9.0, 0), 0.0, "it winds all the way back");
    }

    #[test]
    fn a_hard_flick_carries_a_looping_strip_round_more_than_once() {
        let t = track(8, 1, true);
        let landing = t.landing(0.0, 20.5);
        assert!(landing > 16.0, "the whole travel was kept");
        assert_eq!(landing.fract(), 0.0, "and it still landed on an item");
    }

    #[test]
    fn only_the_items_the_viewport_reaches_are_drawn() {
        let t = track(30, 3, false);
        let shown: Vec<usize> =
            t.items_in_view(10.0, 0.0, 3.0).iter().map(|(i, _)| *i).collect();
        assert_eq!(shown, vec![10, 11, 12, 13], "three on show and the one arriving");
        // Their leading edges are measured from the strip's own.
        assert_eq!(t.items_in_view(10.0, 0.0, 3.0)[0].1, 0.0);
        assert_eq!(t.items_in_view(10.0, 0.0, 3.0)[1].1, 1.0);
    }

    #[test]
    fn a_peek_brings_the_item_behind_into_the_list() {
        let t = track(30, 1, false);
        let shown: Vec<usize> =
            t.items_in_view(10.0, -0.2, 1.4).iter().map(|(i, _)| *i).collect();
        assert!(shown.contains(&9), "the peek shows the tail of the one behind");
        assert!(shown.contains(&11), "and the head of the one ahead");
    }

    #[test]
    fn a_strip_with_ends_shows_nothing_before_its_first_item() {
        let t = track(4, 1, false);
        let shown: Vec<usize> =
            t.items_in_view(0.0, -0.5, 2.0).iter().map(|(i, _)| *i).collect();
        assert_eq!(shown, vec![0, 1, 2], "there is nothing behind the first");
    }

    #[test]
    fn a_looping_strip_shows_the_item_before_its_first() {
        let t = track(4, 1, true);
        let shown: Vec<usize> =
            t.items_in_view(0.0, -0.5, 1.5).iter().map(|(i, _)| *i).collect();
        assert_eq!(shown[0], 3, "and it is the last one, come round");
    }

    #[test]
    fn a_short_looping_strip_shows_its_items_more_than_once() {
        let t = track(2, 1, true);
        let shown: Vec<usize> =
            t.items_in_view(0.0, 0.0, 5.0).iter().map(|(i, _)| *i).collect();
        assert_eq!(shown.len(), 6);
        assert_eq!(shown.iter().filter(|i| **i == 0).count(), 3);
    }

    #[test]
    fn an_item_is_as_wide_as_what_the_arrows_the_peeks_and_the_gaps_leave() {
        let f = frame(3, 0.0);
        // 600 - 26 - 26 = 548 for three items and two 8 point gaps.
        assert_eq!(f.inner_w(), 548.0);
        assert!((f.item_w() - (548.0 - 16.0) / 3.0).abs() < 1e-9);
        assert!((f.pitch() - (f.item_w() + 8.0)).abs() < 1e-9);
        // A peek takes its room from the same band.
        let f = frame(1, 40.0);
        assert_eq!(f.inner_w(), 468.0);
        assert_eq!(f.item_w(), 468.0);
    }

    #[test]
    fn a_peek_opens_the_viewport_before_the_leading_edge() {
        let f = frame(1, 40.0);
        assert!(f.lead() < 0.0, "the viewport starts behind the leading item");
        assert!((f.lead() * f.pitch() + 40.0).abs() < 1e-9);
        // And the span covers the peeks at both sides.
        assert!((f.span() * f.pitch() - f.view_w()).abs() < 1e-9);
        // Without a peek the viewport starts exactly at the leading edge.
        assert_eq!(frame(1, 0.0).lead(), 0.0);
    }

    #[test]
    fn a_press_at_the_sides_is_an_arrow_and_the_middle_is_the_strip() {
        let f = frame(1, 0.0);
        assert_eq!(f.target_at(5.0, 60.0, 4), Target::Prev);
        assert_eq!(f.target_at(595.0, 60.0, 4), Target::Next);
        assert_eq!(f.target_at(300.0, 60.0, 4), Target::Strip);
        // With the arrows off the whole width is the strip.
        let bare = Frame { arrow: 0.0, ..f };
        assert_eq!(bare.target_at(5.0, 60.0, 4), Target::Strip);
    }

    #[test]
    fn a_press_under_the_strip_is_a_dot() {
        let f = frame(1, 0.0);
        let y = f.strip_h() + 4.0;
        assert_eq!(f.target_at(f.dot_x(0, 4) + 4.0, y, 4), Target::Dot(0));
        assert_eq!(f.target_at(f.dot_x(3, 4) + 4.0, y, 4), Target::Dot(3));
        // Beside the row there is nothing to press — and in particular the
        // strip is not there, so a press below the cards never scrolls.
        assert_eq!(f.target_at(4.0, y, 4), Target::None);
    }

    #[test]
    fn the_room_between_two_dots_belongs_to_the_nearer_one() {
        let f = frame(1, 0.0);
        let stops = 5;
        let between = f.dot_x(1, stops) + f.dot + f.dot_gap * 0.6;
        assert_eq!(f.dot_at(between, stops), Some(2), "past the halfway point it is the next");
        let between = f.dot_x(1, stops) + f.dot + f.dot_gap * 0.4;
        assert_eq!(f.dot_at(between, stops), Some(1), "and before it, the one it left");
        // Half a gap past either end of the row is still the row.
        assert_eq!(f.dot_at(f.dot_x(0, stops) - f.dot_gap * 0.4, stops), Some(0));
        assert_eq!(f.dot_at(f.dot_x(0, stops) - f.dot_gap * 2.0, stops), None);
    }

    #[test]
    fn the_dots_are_dropped_when_the_row_no_longer_fits() {
        let f = frame(1, 0.0);
        assert!(f.dots_fit(20), "300 points of dots fit across 600");
        assert!(!f.dots_fit(60), "900 do not, and crowding them helps nobody");
        assert!(!f.dots_fit(1), "one dot says nothing a strip that cannot move needs said");
        let bare = Frame { band: 0.0, ..f };
        assert!(!bare.dots_fit(4), "no band, no dots");
    }

    #[test]
    fn an_item_the_edge_is_cutting_is_not_wholly_inside() {
        assert!(wholly_inside(0.0, 3, 0.001));
        assert!(wholly_inside(2.0, 3, 0.001), "the last of three is in");
        assert!(!wholly_inside(2.5, 3, 0.001), "half out at the trailing edge");
        assert!(!wholly_inside(-0.3, 3, 0.001), "and at the leading one");
        // Flush against the edge counts as inside, whatever the rounding.
        assert!(wholly_inside(-0.0004, 3, 0.001));
    }

    #[test]
    fn a_plain_vertical_wheel_is_left_to_the_page() {
        // Swallowing it would trap whatever the carousel is standing in.
        assert_eq!(wheel_items(0.0, 90.0, false, 300.0), 0.0);
        assert!(wheel_items(0.0, 90.0, true, 300.0) > 0.0, "shift says sideways");
        assert!(wheel_items(60.0, 0.0, false, 300.0) > 0.0, "and a sideways delta is ours");
    }

    #[test]
    fn a_notch_is_worth_at_most_one_item() {
        assert_eq!(wheel_items(4000.0, 0.0, false, 300.0), 1.0);
        assert_eq!(wheel_items(-4000.0, 0.0, false, 300.0), -1.0);
        // A trackpad's small deltas pass through in proportion.
        assert!((wheel_items(30.0, 0.0, false, 300.0) - 0.1).abs() < 1e-9);
    }

    #[test]
    fn a_faster_release_carries_further() {
        let slow = spin_travel(2.0, FLING_DECEL_RATE_PER_MS);
        let fast = spin_travel(20.0, FLING_DECEL_RATE_PER_MS);
        assert!(fast > slow * 9.0, "travel is proportional to the release speed");
        assert_eq!(spin_travel(-2.0, FLING_DECEL_RATE_PER_MS), -slow, "and signed");
        assert_eq!(spin_travel(0.0, FLING_DECEL_RATE_PER_MS), 0.0);
    }

    #[test]
    fn a_release_that_is_not_moving_still_produces_a_finite_glide() {
        // 1.0 would be a decay that never decays; the travel must not be an
        // infinity that then becomes a NaN offset.
        assert_eq!(spin_travel(5.0, 1.0), 0.0);
    }

    #[test]
    fn a_glide_opens_at_the_speed_the_finger_left_with() {
        let (from, to) = (0.0, 3.0);
        let velocity = 12.0;
        let tau = glide_secs(to - from, velocity);
        // The opening speed of an exponential approach is distance / tau.
        assert!(((to - from) / tau - velocity).abs() < 0.01);
    }

    #[test]
    fn a_glide_is_bounded_at_both_ends() {
        assert_eq!(glide_secs(0.01, 400.0), GLIDE_SECS.0, "a tiny correction is not instant");
        assert_eq!(glide_secs(40.0, 0.5), GLIDE_SECS.1, "and a slow crawl does not last all day");
        assert_eq!(glide_secs(2.0, 0.0), GLIDE_SECS.0, "a standing start takes the short glide");
    }

    #[test]
    fn a_glide_starts_where_it_was_and_ends_where_it_is_going() {
        let (from, to, tau) = (0.0, 4.0, 0.2);
        assert_eq!(glide_at(from, to, tau, 0.0), from);
        assert!((glide_at(from, to, tau, 2.0) - to).abs() < GLIDE_DONE);
        // And it slows down rather than arriving at a constant speed.
        let first = glide_at(from, to, tau, 0.05) - glide_at(from, to, tau, 0.0);
        let last = glide_at(from, to, tau, 0.35) - glide_at(from, to, tau, 0.3);
        assert!(first > last * 4.0);
    }
}

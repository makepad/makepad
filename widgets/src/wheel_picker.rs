//! WheelPicker — columns of rows that spin like a drum and rest on one.
//!
//! Each column is a strip of rows that scrolls up and down and always comes
//! to rest with exactly one row inside the band across the middle. That row
//! is the column's value: there is no separate confirm step, and no state
//! in which the band holds nothing. Rows fade and shrink with their
//! distance from the band, so a column reads as a cylinder seen edge-on
//! rather than as a list that happens to be short. Several columns stand
//! side by side and turn independently, which is what makes one control out
//! of an hour, a minute and a meridiem.
//!
//! # What it deliberately is not
//!
//! It is not a list. There is no scrollbar, no page motion, no selection of
//! more than one row, and the number of rows on screen is a property of the
//! control rather than of the room it is given — a fixed count is what lets
//! the middle row mean something. Nor does it fetch rows as it goes: every
//! column holds its whole list. Long lists belong in a list; a picker is
//! for a set of values small enough that seeing the neighbours helps.
//!
//! # Landing on a row
//!
//! A flick does not run its momentum out and then jerk to the nearest row.
//! The release velocity is used to predict where a spin decaying at the
//! library's scroll rate would stop, that prediction is snapped to a row,
//! and the column glides there starting at the speed the finger left it
//! with. The motion is one movement and it always ends on a row.
use crate::{
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    badge::measure,
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

    mod.widgets.WheelColumn = #(WheelColumn::script_api(vm))

    mod.widgets.WheelPickerBase = #(WheelPicker::register_widget(vm))

    set_type_default() do #(DrawWheelPicker::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    /** Columns of rows that spin and settle with one row in the band. */
    mod.widgets.WheelPickerFlat = set_type_default() do mod.widgets.WheelPickerBase{
        width: Fit
        height: Fit
        margin: theme.mspace_1

        /** rows on screen; an even count gains one 3..11 step 2 */
        visible_items: 5
        /** row height in pixels 16..64 step 1 */
        row_height: 28.0
        /** width of a column that sets none of its own, in pixels 24..320 step 4 */
        column_width: 88.0
        /** gap between columns in pixels 0..24 step 1 */
        column_gap: 2.0
        /** ink taken from the outermost row 0..1 step 0.02 */
        dim_far: 0.45
        /** size taken from the outermost row 0..0.6 step 0.01 */
        shrink_far: 0.16

        /** ink of a row away from the band */
        color_item: theme.color_label_outer_off
        /** ink of the row in the band */
        color_selected: theme.color_label_outer

        draw_text +: {
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }

        draw_bg +: {
            /** pointer-hover mix 0..1 step 0.01 */
            hover: instance(0.0)
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: instance(0.0)
            /** spinning mix 0..1 step 0.01 */
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

            band_color: uniform(theme.color_val)
            band_color_hover: uniform(theme.color_val_hover)
            band_color_focus: uniform(theme.color_val_focus)
            band_color_drag: uniform(theme.color_val_drag)
            band_color_disabled: uniform(theme.color_val_disabled)

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
                let band = self.band_color
                    .mix(self.band_color_focus, self.focus)
                    .mix(self.band_color_hover.mix(self.band_color_drag, self.drag), self.hover)
                    .mix(self.band_color_disabled, self.disabled)

                // The well.
                sdf.box(
                    self.border_size
                    self.border_size
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                    self.border_radius
                )
                sdf.fill_keep(fill)
                sdf.stroke(stroke, self.border_size)

                // The band runs across every column, because the row it
                // holds in each one is that column's answer. Its height is
                // the row height Rust laid the rows out at, handed over so
                // the band and the row inside it cannot drift apart.
                let inset = self.border_size + 1.0
                let band_y = (self.rect_size.y - self.row_px) * 0.5
                sdf.box(
                    inset
                    band_y
                    max(self.rect_size.x - inset * 2., 1.0)
                    self.row_px
                    self.border_radius
                )
                sdf.fill(band)

                // The column the keyboard is on, so a picker with several
                // of them says which one the arrows will turn.
                sdf.rect(self.band_x, band_y, self.band_w, self.row_px)
                sdf.fill(band + vec4(0.06, 0.06, 0.06, 0.0) * self.focus)

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

    /** The standard picker: the flat face plus the theme's inset bevel. */
    mod.widgets.WheelPicker = set_type_default() do mod.widgets.WheelPickerFlat{
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
pub struct DrawWheelPicker {
    #[deref]
    draw_super: DrawQuad,
    /// The band's height: the same row height the rows are drawn at, so
    /// what the band frames is what it says it frames.
    #[live]
    row_px: f32,
    /// The focused column's slice of the band. Parked far off to the left
    /// when nothing is focused: a zero-width rect still leaves the
    /// anti-aliased edge of one behind, which reads as a stray hairline.
    #[live]
    band_x: f32,
    #[live]
    band_w: f32,
}

/// One column of the picker: the rows it holds, which of them is in the
/// band, and whether it comes round.
#[derive(Script, ScriptHook, Default)]
pub struct WheelColumn {
    #[source]
    source: ScriptObjectRef,
    /// The rows, top to bottom.
    #[live]
    pub items: Vec<String>,
    /// Which row starts in the band. The widget writes the row that ends up
    /// there back here, so a host that reads the column object sees what
    /// the band is actually holding.
    #[live]
    pub selected: usize,
    /// Past the last row comes the first.
    #[live]
    pub loop_items: bool,
    /// This column's width in pixels; zero takes the picker's
    /// `column_width`.
    #[live]
    pub width: f64,
}

/// The shortest and longest a glide may take, in seconds. The short bound
/// keeps a one-row correction from arriving as a jump; the long one keeps
/// the hardest flick from coasting so far that the picker feels stuck.
const GLIDE_SECS: (f64, f64) = (0.12, 0.7);

/// A glide is over once it is this close to its target, in pixels. An
/// exponential approach never actually arrives.
const GLIDE_DONE_PX: f64 = 0.15;

/// One column's arithmetic, apart from the widget that draws it: where the
/// drum stands, which row that puts in the band, and where a flick will
/// leave it. It is a separate type for two reasons — it can be tested
/// without a script heap, and the draw and the hit test are handed the same
/// numbers from the same place, so what is shown is what is grabbable.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Drum {
    /// How many rows the column holds.
    count: usize,
    /// Row height in pixels: the drum's unit of travel.
    row: f64,
    /// Past the last row comes the first.
    looping: bool,
}

impl Drum {
    fn new(count: usize, row: f64, looping: bool) -> Self {
        Self { count, row: row.max(1.0), looping: looping && count > 0 }
    }

    /// The offset at which `index` rests in the band.
    fn offset_of(&self, index: usize) -> f64 {
        index as f64 * self.row
    }

    /// The travel of one whole turn.
    fn turn(&self) -> f64 {
        self.count as f64 * self.row
    }

    /// Hold an offset inside what the column allows.
    ///
    /// A looping drum keeps its raw travel and is only reduced modulo a
    /// turn when something asks which row it is showing. Storing the
    /// wrapped offset instead breaks every motion that crosses the seam: a
    /// glide from just under a turn to just over it becomes a glide
    /// backwards through the entire column.
    ///
    /// A column with ends stops dead at them. A rubber band is right for a
    /// list, where the edge is far from most of the content; on a drum of
    /// six rows the stretch is most of the control and reads as the picker
    /// having lost its place.
    fn bound(&self, offset: f64) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        if self.looping {
            offset
        } else {
            offset.clamp(0.0, self.offset_of(self.count - 1))
        }
    }

    /// The row in the band at this offset.
    fn index_at(&self, offset: f64) -> usize {
        if self.count == 0 {
            return 0;
        }
        let i = (self.bound(offset) / self.row).round();
        if self.looping {
            (i as i64).rem_euclid(self.count as i64) as usize
        } else {
            (i.max(0.0) as usize).min(self.count - 1)
        }
    }

    /// The nearest offset at which a row rests.
    fn snap(&self, offset: f64) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        self.bound((offset / self.row).round() * self.row)
    }

    /// Where a spin now at `offset` and carrying `travel` more pixels comes
    /// to rest. A looping column keeps the whole travel, so a hard flick
    /// turns the drum more than once; a column with ends stops at them.
    fn landing(&self, offset: f64, travel: f64) -> f64 {
        self.snap(offset + travel)
    }

    /// The offset to send the drum to so `index` ends in the band, in the
    /// same unwrapped frame the drum is standing in. A looping column takes
    /// the short way round rather than unwinding.
    fn reach(&self, from: f64, index: usize) -> f64 {
        let target = self.offset_of(index);
        if self.looping {
            nearest_turn(from, target, self.turn())
        } else {
            target
        }
    }

    /// Where row `index` sits relative to the band, in rows.
    ///
    /// A looping column takes the short way round here too, so the row that
    /// has just come over the top is drawn where the eye expects it rather
    /// than a whole turn away.
    fn rows_from_band(&self, index: usize, offset: f64) -> f64 {
        let mut d = index as f64 - offset / self.row;
        if self.looping && self.count > 0 {
            let n = self.count as f64;
            d -= (d / n).round() * n;
        }
        d
    }

    /// The rows within `reach` rows of the band, each with its distance
    /// from it. Ordered outermost first, so the band's own row is laid down
    /// last and sits over its neighbours wherever they meet.
    fn rows_in_view(&self, offset: f64, reach: f64) -> Vec<(usize, f64)> {
        let mut out = Vec::new();
        if self.count == 0 || reach <= 0.0 {
            return out;
        }
        let centre = offset / self.row;
        if self.looping {
            // Walking out from the row nearest the band shows a column
            // shorter than the window more than once, which is what a drum
            // that comes round actually looks like.
            let mid = centre.round() as i64;
            let span = reach.ceil() as i64;
            for k in -span..=span {
                let raw = mid + k;
                let d = raw as f64 - centre;
                if d.abs() < reach {
                    out.push((raw.rem_euclid(self.count as i64) as usize, d));
                }
            }
        } else {
            let first = (centre - reach).ceil().max(0.0) as usize;
            let last = ((centre + reach).floor().max(0.0) as usize).min(self.count - 1);
            for index in first..=last.max(first) {
                if index >= self.count {
                    break;
                }
                let d = index as f64 - centre;
                if d.abs() < reach {
                    out.push((index, d));
                }
            }
        }
        out.sort_by(|a, b| {
            b.1.abs().partial_cmp(&a.1.abs()).unwrap_or(std::cmp::Ordering::Equal)
        });
        out
    }

    /// Move a selection by `delta` rows: round the ends on a looping
    /// column, stop at them otherwise.
    fn step(&self, index: usize, delta: i64) -> usize {
        if self.count == 0 {
            return 0;
        }
        let n = self.count as i64;
        let raw = index as i64 + delta;
        if self.looping {
            raw.rem_euclid(n) as usize
        } else {
            raw.clamp(0, n - 1) as usize
        }
    }
}

/// The representative of `target` nearest `from` on a drum that comes
/// round, so a move across the seam is the short way rather than the long
/// way back through every row.
fn nearest_turn(from: f64, target: f64, turn: f64) -> f64 {
    if turn <= 0.0 {
        return target;
    }
    let mut d = target - from;
    d -= (d / turn).round() * turn;
    from + d
}

/// How many rows are on screen. An even count gains one, because a drum
/// with no middle row has nothing to put in the band.
fn odd_rows(n: usize) -> usize {
    let n = n.max(1);
    if n % 2 == 0 {
        n + 1
    } else {
        n
    }
}

/// Where a spin released at `velocity` px/s stops if its speed decays by
/// `decay_per_ms` every millisecond: the whole remaining travel of
/// `v(t) = v0 * decay^t` is `v0 / lambda`, with `lambda = -ln(decay) * 1000`.
///
/// The library's scrollers integrate that same decay frame by frame. A drum
/// only needs to know where it ends, because it has to land on a row, and
/// the landing place is what decides the whole motion.
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

/// The drum's position `t` seconds into a glide from `from` to `to`.
fn glide_at(from: f64, to: f64, tau: f64, t: f64) -> f64 {
    to + (from - to) * (-t / tau.max(f64::EPSILON)).exp()
}

/// A wheel notch is worth at most one row, whatever size the operating
/// system says it is. Notches run from a few pixels to most of a screen,
/// and a picker that jumps four rows on one notch cannot be aimed.
fn wheel_travel(delta: f64, row: f64) -> f64 {
    delta.clamp(-row, row)
}

/// What a row `d` rows from the band keeps of a quantity, where `half` is
/// the number of rows between the band and the edge of the well and `take`
/// is how much of it the outermost row gives up. Used for both the ink and
/// the size, which is why it is one function and not two.
fn fade_at(d: f64, half: f64, take: f64) -> f64 {
    let t = (d.abs() / half.max(f64::EPSILON)).min(1.0);
    (1.0 - take.clamp(0.0, 1.0) * t).max(0.0)
}

/// The glide that carries one column to the row it will rest on.
#[derive(Clone, Copy, Debug)]
struct Glide {
    from: f64,
    to: f64,
    tau: f64,
    /// The jitter-clamped frame clock the scrollers use, so a late frame
    /// becomes a slightly uneven step rather than a visible jerk.
    clock: FrameClock,
}

/// What one column is doing between frames.
#[derive(Clone, Debug, Default)]
struct Spin {
    /// Where the drum stands, in pixels of travel from the first row.
    offset: f64,
    /// The finger's positions, for the release velocity.
    samples: Vec<ScrollSample>,
    glide: Option<Glide>,
}

/// The finger's hold on one column.
#[derive(Clone, Copy, Debug)]
struct Grab {
    col: usize,
    /// The offset and the finger position when the press landed, so the
    /// drum tracks the finger instead of jumping to it.
    offset: f64,
    abs: f64,
    /// Whether the press caught a column that was still moving. Catching is
    /// a stop, so such a press must not also count as a tap on a row.
    caught: bool,
}

#[derive(Clone, Debug, Default)]
pub enum WheelPickerAction {
    /// The band of `column` now holds `index`.
    Changed {
        column: usize,
        index: usize,
    },
    #[default]
    None,
}

#[derive(Script, Widget, Animator)]
pub struct WheelPicker {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_bg: DrawWheelPicker,
    #[live]
    draw_text: DrawText,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[apply_default]
    animator: Animator,

    /// The columns, left to right.
    #[live]
    pub columns: Vec<WheelColumn>,

    /// Rows on screen. Odd, so there is a true centre; an even count is
    /// raised by one rather than refused.
    #[live(5)]
    pub visible_items: usize,
    #[live(28.0)]
    pub row_height: f64,
    /// The width of a column that declares none of its own.
    #[live(88.0)]
    pub column_width: f64,
    #[live(2.0)]
    pub column_gap: f64,
    /// How much ink and how much size the outermost row gives up.
    #[live(0.45)]
    pub dim_far: f64,
    #[live(0.16)]
    pub shrink_far: f64,

    #[live]
    pub color_item: Vec4f,
    #[live]
    pub color_selected: Vec4f,

    #[rust]
    spins: Vec<Spin>,
    /// Where each column was last drawn, as an offset from the widget's
    /// left edge and a width. The hit test reads these rather than
    /// recomputing the layout, so a press lands on the column the eye is
    /// looking at.
    #[rust]
    spans: Vec<(f64, f64)>,
    /// The column the keyboard turns.
    #[rust]
    focus_col: usize,
    #[rust]
    grab: Option<Grab>,
    #[rust]
    next_frame: NextFrame,
}

impl ScriptHook for WheelPicker {
    fn on_after_new(&mut self, _vm: &mut ScriptVm) {
        // Dropping the running state forces it to be rebuilt from the
        // columns as they now stand, which is what a re-apply means.
        self.spins.clear();
        self.spans.clear();
        self.grab = None;
    }
}

impl WheelPicker {
    fn rows(&self) -> usize {
        odd_rows(self.visible_items)
    }

    fn row(&self) -> f64 {
        self.row_height.max(1.0)
    }

    fn drum(&self, col: usize) -> Drum {
        let c = &self.columns[col];
        Drum::new(c.items.len(), self.row(), c.loop_items)
    }

    /// Match the running state to the columns. Cheap when nothing changed,
    /// which is every frame but the first after an apply.
    fn sync(&mut self) {
        if self.spins.len() == self.columns.len() {
            return;
        }
        let row = self.row();
        let spins: Vec<Spin> = self
            .columns
            .iter()
            .map(|c| {
                let drum = Drum::new(c.items.len(), row, c.loop_items);
                Spin { offset: drum.bound(drum.offset_of(c.selected)), ..Default::default() }
            })
            .collect();
        self.spins = spins;
        self.focus_col = self.focus_col.min(self.columns.len().saturating_sub(1));
        self.grab = None;
    }

    fn col_width(&self, col: usize) -> f64 {
        let w = self.columns[col].width;
        if w > 0.0 {
            w
        } else {
            self.column_width.max(1.0)
        }
    }

    /// The left edge of a column, as an offset from the widget's own.
    fn col_start(&self, col: usize) -> f64 {
        (0..col).map(|i| self.col_width(i) + self.column_gap).sum()
    }

    fn body_width(&self) -> f64 {
        let n = self.columns.len();
        if n == 0 {
            return 0.0;
        }
        self.col_start(n - 1) + self.col_width(n - 1)
    }

    /// The column an x offset from the widget's left edge belongs to.
    fn column_at(&self, x: f64) -> usize {
        let last = self.columns.len().saturating_sub(1);
        for (i, &(start, w)) in self.spans.iter().enumerate() {
            if x < start + w + self.column_gap * 0.5 {
                return i.min(last);
            }
        }
        last
    }

    /// Put a drum at `offset` and say so if that changed the row in the
    /// band.
    ///
    /// The action fires while the drum is still moving, not only once it
    /// settles. The band is the value: a readout beside a spinning column
    /// that keeps showing the row from before the flick is telling the
    /// truth about nothing.
    fn place(&mut self, cx: &mut Cx, col: usize, offset: f64) {
        let drum = self.drum(col);
        let before = drum.index_at(self.spins[col].offset);
        self.spins[col].offset = drum.bound(offset);
        let after = drum.index_at(self.spins[col].offset);
        if after != before {
            self.columns[col].selected = after;
            cx.widget_action(self.uid, WheelPickerAction::Changed { column: col, index: after });
        }
        self.draw_bg.redraw(cx);
    }

    /// Send a column to `target`, opening at the speed a spin released at
    /// `velocity` would have. `target` is in the same unwrapped frame the
    /// drum is standing in.
    fn glide_to(&mut self, cx: &mut Cx, col: usize, target: f64, velocity: f64) {
        let from = self.spins[col].offset;
        if (target - from).abs() <= GLIDE_DONE_PX {
            self.spins[col].glide = None;
            self.place(cx, col, target);
            return;
        }
        self.spins[col].glide = Some(Glide {
            from,
            to: target,
            tau: glide_secs(target - from, velocity),
            clock: FrameClock::default(),
        });
        self.next_frame = cx.new_next_frame();
        self.draw_bg.redraw(cx);
    }

    /// One frame of every glide in flight.
    fn tick(&mut self, cx: &mut Cx, time: f64) {
        let mut alive = false;
        for col in 0..self.spins.len() {
            let Some(mut glide) = self.spins[col].glide else {
                continue;
            };
            let t = glide.clock.advance(time);
            let at = glide_at(glide.from, glide.to, glide.tau, t);
            let done = (at - glide.to).abs() <= GLIDE_DONE_PX;
            self.spins[col].glide = if done { None } else { Some(glide) };
            self.place(cx, col, if done { glide.to } else { at });
            alive |= !done;
        }
        if alive {
            self.next_frame = cx.new_next_frame();
        }
    }

    pub fn selected(&self, column: usize) -> usize {
        if column >= self.columns.len() {
            return 0;
        }
        if self.spins.len() != self.columns.len() {
            // Before the first sync the declared row is the only answer.
            return self.columns[column].selected;
        }
        self.drum(column).index_at(self.spins[column].offset)
    }

    /// Put a row in the band without a spin. Setting a value is not a
    /// gesture, and animating one that nobody asked for makes a host that
    /// writes several columns at once look like it is fighting itself.
    pub fn set_selected(&mut self, cx: &mut Cx, column: usize, index: usize) {
        self.sync();
        if column >= self.columns.len() {
            return;
        }
        let drum = self.drum(column);
        if drum.count == 0 {
            return;
        }
        let index = index.min(drum.count - 1);
        self.spins[column].glide = None;
        // Silent: the host already knows, and answering its own write with
        // a Changed is how a readout and a picker end up writing to each
        // other for as long as the frame lasts.
        self.spins[column].offset = drum.reach(self.spins[column].offset, index);
        self.columns[column].selected = index;
        self.draw_bg.redraw(cx);
    }
}

impl Widget for WheelPicker {
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
        if method == live_id!(set_selected) {
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                let column = vm.bx.heap.vec_value(args_obj, 0, trap).as_f64();
                let index = vm.bx.heap.vec_value(args_obj, 1, trap).as_f64();
                if let (Some(column), Some(index)) = (column, index) {
                    vm.with_cx_mut(|cx| {
                        self.set_selected(cx, column.max(0.0) as usize, index.max(0.0) as usize)
                    });
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(selected) {
            let mut column = 0.0;
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                if let Some(value) = vm.bx.heap.vec_value(args_obj, 0, trap).as_f64() {
                    column = value;
                }
            }
            return ScriptAsyncResult::Return(ScriptValue::from_f64(
                self.selected(column.max(0.0) as usize) as f64,
            ));
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.animator_handle_event(cx, event);
        self.sync();

        if let Some(ne) = self.next_frame.is_event(event) {
            self.tick(cx, ne.time);
        }
        if self.animator_in_state(cx, ids!(disabled.on)) || self.columns.is_empty() {
            return;
        }
        let row = self.row();

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerHoverOver(_) => {
                cx.set_cursor(MouseCursor::Grab);
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                let col = self.column_at(fe.abs.x - fe.rect.pos.x);
                self.focus_col = col;
                // A hand on a spinning wheel stops it where it stands.
                let caught = self.spins[col].glide.take().is_some();
                self.spins[col].samples.clear();
                push_sample(&mut self.spins[col].samples, fe.abs.y, fe.time);
                self.grab = Some(Grab {
                    col,
                    offset: self.spins[col].offset,
                    abs: fe.abs.y,
                    caught,
                });
                self.animator_play(cx, ids!(drag.on));
                self.draw_bg.redraw(cx);
            }
            Hit::FingerMove(fe) => {
                let Some(grab) = self.grab else {
                    return;
                };
                push_sample(&mut self.spins[grab.col].samples, fe.abs.y, fe.time);
                // The finger and the drum run opposite ways: dragging down
                // brings earlier rows into the band.
                self.place(cx, grab.col, grab.offset - (fe.abs.y - grab.abs));
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
                let col = grab.col;
                let (velocity, travel) = estimate_release_velocity(&self.spins[col].samples);
                let carry = if travel.abs() > FLING_MIN_TOTAL_DELTA {
                    // The finger's velocity is the drum's, negated.
                    spin_travel(-velocity, FLING_DECEL_RATE_PER_MS)
                } else if !grab.caught {
                    // A press that did not move and did not stop anything
                    // is a tap on a row: the distance from the band to the
                    // finger is exactly the travel that brings that row in.
                    fe.abs.y - (fe.rect.pos.y + fe.rect.size.y * 0.5)
                } else {
                    0.0
                };
                let target = self.drum(col).landing(self.spins[col].offset, carry);
                // No action of its own here: the glide reports every row it
                // passes, including the one it stops on, and a release that
                // moved nothing should say nothing.
                self.glide_to(cx, col, target, -velocity);
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
                let col = self.focus_col.min(self.columns.len() - 1);
                let drum = self.drum(col);
                let index = drum.index_at(self.spins[col].offset);
                let landing = match ke.key_code {
                    KeyCode::ArrowUp => Some(drum.step(index, -1)),
                    KeyCode::ArrowDown => Some(drum.step(index, 1)),
                    // Home and End are the first and last row of the list,
                    // not of the drum: on a column that comes round, the
                    // last row may well be one step backwards.
                    KeyCode::Home => Some(0),
                    KeyCode::End => Some(drum.count.saturating_sub(1)),
                    // Left and right choose the column, since Tab belongs
                    // to focus traversal and a picker's columns are as much
                    // one control as a slider's two ends are.
                    KeyCode::ArrowLeft => {
                        self.focus_col = col.saturating_sub(1);
                        self.draw_bg.redraw(cx);
                        None
                    }
                    KeyCode::ArrowRight => {
                        self.focus_col = (col + 1).min(self.columns.len() - 1);
                        self.draw_bg.redraw(cx);
                        None
                    }
                    _ => None,
                };
                if let Some(index) = landing {
                    let target = drum.reach(self.spins[col].offset, index);
                    self.glide_to(cx, col, target, 0.0);
                }
            }
            Hit::FingerScroll(e) => {
                let col = self.column_at(e.abs.x - e.rect.pos.x);
                let travel = wheel_travel(e.scroll.y, row);
                if travel != 0.0 {
                    self.focus_col = col;
                    self.spins[col].glide = None;
                    let at = self.spins[col].offset + travel;
                    let target = self.drum(col).landing(at, 0.0);
                    self.place(cx, col, at);
                    self.glide_to(cx, col, target, 0.0);
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.sync();
        let rows = self.rows();
        let row = self.row();

        // A drum's size is the rows it shows, not the room it is given: a
        // half row at the edge of the well would sit outside the band's
        // arithmetic and read as a list that had been cut off. `Fit` asks
        // for that natural size; any other walk is honoured as written, and
        // the columns keep their own widths and start at the left edge.
        let mut walk = walk;
        if matches!(walk.height, Size::Fit { .. }) {
            walk.height = Size::Fixed(rows as f64 * row);
        }
        if matches!(walk.width, Size::Fit { .. }) {
            walk.width = Size::Fixed(self.body_width());
        }

        self.draw_bg.row_px = row as f32;
        // The instance values are fixed when the quad is emitted, which is
        // inside `begin`, so the lit slice is worked out from the column
        // widths rather than from the laid-out rect.
        let lit = self.animator_in_state(cx, ids!(focus.on)) && !self.columns.is_empty();
        let focus_col = self.focus_col.min(self.columns.len().saturating_sub(1));
        if lit {
            self.draw_bg.band_x = self.col_start(focus_col) as f32;
            self.draw_bg.band_w = self.col_width(focus_col) as f32;
        } else {
            self.draw_bg.band_x = -1000.0;
            self.draw_bg.band_w = 0.0;
        }

        self.draw_bg.begin(cx, walk, self.layout);
        let rect = cx.turtle().rect();
        let mid = rect.pos.y + rect.size.y * 0.5;
        // Rows between the band and the edge of the well, and how far out a
        // row may be before none of it is inside.
        let half = rows as f64 * 0.5;
        let reach = half + 0.5;
        let base_font = self.draw_text.text_style.font_size;

        self.spans.clear();
        for col in 0..self.columns.len() {
            let width = self.col_width(col);
            let start = self.col_start(col);
            self.spans.push((start, width));
            let drum = self.drum(col);
            let offset = self.spins[col].offset;
            for (index, d) in drum.rows_in_view(offset, reach) {
                let Some(text) = self.columns[col].items.get(index).cloned() else {
                    continue;
                };
                let ink_left = fade_at(d, half, self.dim_far);
                let size = base_font * fade_at(d, half, self.shrink_far) as f32;
                self.draw_text.text_style.font_size = size;
                // The row in the band keeps the strong ink and its
                // neighbours hand theirs over as they leave, mixed rather
                // than switched so nothing pops as the drum turns.
                let mut ink = self.color_selected.mix(self.color_item, (d.abs() as f32).min(1.0));
                ink.w *= ink_left as f32;
                self.draw_text.color = ink;
                let text_width = measure(&self.draw_text, cx, &text);
                let top = mid + d * row - row * 0.5;
                // `draw_abs` takes the top of the line box, not the top of
                // the ink: a glyph's ink starts about 0.30 of the size
                // below it. Centring the line box leaves every row riding
                // high in its own band.
                let y = top + (row - size as f64) * 0.5 - size as f64 * 0.30;
                let x = rect.pos.x + start + (width - text_width) * 0.5;
                self.draw_text.draw_abs(cx, dvec2(x, y), &text);
            }
        }
        self.draw_text.text_style.font_size = base_font;
        self.draw_bg.end(cx);

        if !self.animator_in_state(cx, ids!(disabled.on)) {
            cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        }
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.columns
            .iter()
            .enumerate()
            .map(|(col, c)| c.items.get(self.selected(col)).cloned().unwrap_or_default())
            .collect::<Vec<String>>()
            .join(" ")
    }
}

impl WheelPickerRef {
    /// The row `column` is holding in the band.
    pub fn selected(&self, column: usize) -> usize {
        self.borrow().map(|inner| inner.selected(column)).unwrap_or(0)
    }

    /// Put a row in the band, with no spin to watch.
    pub fn set_selected(&self, cx: &mut Cx, column: usize, index: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_selected(cx, column, index);
        }
    }

    /// The column and row of a change in this pass. A spin reports every
    /// row it passes, so this fires while the drum is still moving; read
    /// `selected` for the other columns, which report only when they move.
    pub fn changed(&self, actions: &Actions) -> Option<(usize, usize)> {
        actions
            .filter_widget_actions_cast::<WheelPickerAction>(self.widget_uid())
            .find_map(|action| match action {
                WheelPickerAction::Changed { column, index } => Some((column, index)),
                _ => None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drum(count: usize, looping: bool) -> Drum {
        Drum::new(count, 28.0, looping)
    }

    #[test]
    fn the_band_holds_the_row_the_offset_names() {
        let d = drum(12, false);
        assert_eq!(d.index_at(0.0), 0);
        assert_eq!(d.index_at(d.offset_of(5)), 5);
        // Half a row either side still belongs to the row it is nearest.
        assert_eq!(d.index_at(d.offset_of(5) + 13.0), 5);
        assert_eq!(d.index_at(d.offset_of(5) - 13.0), 5);
        assert_eq!(d.index_at(d.offset_of(5) + 15.0), 6);
    }

    #[test]
    fn a_column_with_ends_stops_at_them() {
        let d = drum(4, false);
        assert_eq!(d.bound(-500.0), 0.0);
        assert_eq!(d.bound(500.0), d.offset_of(3));
        assert_eq!(d.index_at(-500.0), 0);
        assert_eq!(d.index_at(500.0), 3);
        assert_eq!(d.step(0, -1), 0, "the first row has nothing above it");
        assert_eq!(d.step(3, 1), 3, "nor the last anything below");
    }

    #[test]
    fn a_looping_column_comes_round() {
        let d = drum(4, true);
        assert_eq!(d.index_at(d.offset_of(4)), 0, "past the last is the first");
        assert_eq!(d.index_at(-28.0), 3, "and before the first is the last");
        assert_eq!(d.step(3, 1), 0);
        assert_eq!(d.step(0, -1), 3);
    }

    #[test]
    fn a_looping_drum_keeps_its_raw_travel() {
        // The stored offset is never wrapped. Wrapping it would turn a
        // glide across the seam — from just under a turn to just over it —
        // into a glide backwards through the whole column.
        let d = drum(6, true);
        let past = d.turn() + 3.0 * d.row;
        assert_eq!(d.bound(past), past);
        assert_eq!(d.index_at(past), 3);
    }

    #[test]
    fn a_move_across_the_seam_takes_the_short_way() {
        let d = drum(10, true);
        // Standing on the last row, the first row is one row forward, not
        // nine rows back.
        let from = d.offset_of(9);
        assert_eq!(d.reach(from, 0) - from, d.row);
        // And the other way about.
        let from = d.offset_of(0);
        assert_eq!(d.reach(from, 9) - from, -d.row);
    }

    #[test]
    fn a_column_with_ends_never_takes_the_short_way() {
        let d = drum(10, false);
        let from = d.offset_of(9);
        assert_eq!(d.reach(from, 0), 0.0, "it winds all the way back");
    }

    #[test]
    fn a_hard_flick_turns_a_looping_drum_more_than_once() {
        let d = drum(8, true);
        let travel = d.turn() * 2.5;
        let landing = d.landing(0.0, travel);
        assert!(landing > d.turn() * 2.0, "the whole travel was kept");
        assert_eq!(landing % d.row, 0.0, "and it still landed on a row");
    }

    #[test]
    fn a_flick_always_lands_on_a_row() {
        let d = drum(20, false);
        for travel in [-1000.0, -13.0, 0.0, 7.0, 55.0, 1000.0] {
            let landing = d.landing(d.offset_of(6), travel);
            assert_eq!(landing % d.row, 0.0, "travel {travel} left the drum between rows");
        }
    }

    #[test]
    fn an_empty_column_answers_rather_than_dividing_by_nothing() {
        let d = drum(0, true);
        assert_eq!(d.index_at(123.0), 0);
        assert_eq!(d.snap(123.0), 0.0);
        assert!(d.rows_in_view(0.0, 3.0).is_empty());
    }

    #[test]
    fn only_the_rows_near_the_band_are_drawn() {
        let d = drum(100, false);
        let rows = d.rows_in_view(d.offset_of(50), 2.5);
        assert_eq!(rows.len(), 5);
        let mut indices: Vec<usize> = rows.iter().map(|(i, _)| *i).collect();
        indices.sort();
        assert_eq!(indices, vec![48, 49, 50, 51, 52]);
        // Outermost first, so the band's own row is laid down last.
        assert_eq!(rows.last().unwrap().0, 50);
    }

    #[test]
    fn the_ends_of_a_column_with_ends_show_fewer_rows() {
        let d = drum(3, false);
        let rows = d.rows_in_view(0.0, 2.5);
        let indices: Vec<usize> = rows.iter().map(|(i, _)| *i).collect();
        assert_eq!(indices.len(), 3, "there is nothing above the first row");
        assert!(indices.contains(&0) && indices.contains(&2));
    }

    #[test]
    fn a_short_looping_column_shows_its_rows_more_than_once() {
        let d = drum(2, true);
        let rows = d.rows_in_view(0.0, 2.5);
        assert_eq!(rows.len(), 5, "a two-row drum still fills a five-row window");
        assert_eq!(rows.iter().filter(|(i, _)| *i == 0).count(), 3);
    }

    #[test]
    fn a_row_that_has_come_over_the_top_is_drawn_where_the_eye_expects() {
        let d = drum(6, true);
        // Row 0 is one row below row 5, not five rows above it.
        assert_eq!(d.rows_from_band(0, d.offset_of(5)), 1.0);
        assert_eq!(d.rows_from_band(5, d.offset_of(0)), -1.0);
    }

    #[test]
    fn an_even_row_count_gains_one_so_there_is_a_middle() {
        assert_eq!(odd_rows(5), 5);
        assert_eq!(odd_rows(4), 5);
        assert_eq!(odd_rows(0), 1);
        assert_eq!(odd_rows(1), 1);
    }

    #[test]
    fn a_faster_release_carries_further() {
        let slow = spin_travel(200.0, FLING_DECEL_RATE_PER_MS);
        let fast = spin_travel(2000.0, FLING_DECEL_RATE_PER_MS);
        assert!(fast > slow * 9.0, "travel is proportional to the release speed");
        assert_eq!(spin_travel(-200.0, FLING_DECEL_RATE_PER_MS), -slow, "and signed");
        assert_eq!(spin_travel(0.0, FLING_DECEL_RATE_PER_MS), 0.0);
    }

    #[test]
    fn a_release_that_is_not_moving_still_produces_a_finite_glide() {
        // 1.0 would be a decay that never decays; the travel must not be
        // an infinity that then becomes a NaN offset.
        assert_eq!(spin_travel(500.0, 1.0), 0.0);
    }

    #[test]
    fn a_glide_opens_at_the_speed_the_finger_left_with() {
        let (from, to) = (0.0, 300.0);
        let velocity = 1500.0;
        let tau = glide_secs(to - from, velocity);
        // The opening speed of an exponential approach is distance / tau.
        assert!(((to - from) / tau - velocity).abs() < 1.0);
    }

    #[test]
    fn a_glide_is_bounded_at_both_ends() {
        assert_eq!(glide_secs(1.0, 10_000.0), GLIDE_SECS.0, "a tiny correction is not instant");
        assert_eq!(glide_secs(5000.0, 10.0), GLIDE_SECS.1, "and a slow crawl does not last all day");
        assert_eq!(glide_secs(100.0, 0.0), GLIDE_SECS.0, "a standing start takes the short glide");
    }

    #[test]
    fn a_glide_starts_where_it_was_and_ends_where_it_is_going() {
        let (from, to, tau) = (0.0, 100.0, 0.2);
        assert_eq!(glide_at(from, to, tau, 0.0), from);
        assert!((glide_at(from, to, tau, 2.0) - to).abs() < GLIDE_DONE_PX);
        // And it slows down rather than arriving at a constant speed.
        let first = glide_at(from, to, tau, 0.05) - glide_at(from, to, tau, 0.0);
        let last = glide_at(from, to, tau, 0.35) - glide_at(from, to, tau, 0.3);
        assert!(first > last * 4.0);
    }

    #[test]
    fn a_wheel_notch_is_worth_at_most_one_row() {
        assert_eq!(wheel_travel(400.0, 28.0), 28.0);
        assert_eq!(wheel_travel(-400.0, 28.0), -28.0);
        assert_eq!(wheel_travel(6.0, 28.0), 6.0, "a trackpad's small deltas pass through");
    }

    #[test]
    fn a_row_gives_up_more_the_further_it_is_from_the_band() {
        assert_eq!(fade_at(0.0, 2.5, 0.72), 1.0);
        assert!(fade_at(1.0, 2.5, 0.72) > fade_at(2.0, 2.5, 0.72));
        assert!((fade_at(2.5, 2.5, 0.72) - 0.28).abs() < 1e-9);
        // Past the edge of the well nothing more is taken, and the share
        // never goes negative however greedy the setting.
        assert_eq!(fade_at(10.0, 2.5, 0.72), fade_at(2.5, 2.5, 0.72));
        assert_eq!(fade_at(10.0, 2.5, 5.0), 0.0);
    }

    #[test]
    fn distance_from_the_band_does_not_care_which_side() {
        assert_eq!(fade_at(-1.5, 2.5, 0.5), fade_at(1.5, 2.5, 0.5));
    }
}

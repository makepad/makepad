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
//! the middle row mean something. That last clause holds for every picker
//! that leaves `fill_travel` alone, which is every picker that does not say
//! otherwise; the single exception is a ruler that asks for the room to
//! decide, and it is spelled out under "A ruler that fills the room" below.
//! Nor does it fetch rows as it goes: every column holds its whole list.
//! Long lists belong in a list; a picker is for a set of values small enough
//! that seeing the neighbours helps.
//!
//! # Landing on a row
//!
//! A flick does not run its momentum out and then jerk to the nearest row.
//! The release velocity is used to predict where a spin decaying at the
//! library's scroll rate would stop, that prediction is snapped to a row,
//! and the column glides there starting at the speed the finger left it
//! with. The motion is one movement and it always ends on a row.
//!
//! # A scale column, and the sliding ruler
//!
//! A column that names a `step` is a SCALE rather than a list. It carries
//! no `items` at all: its rows are the numbers `min`, `min + step`, … up to
//! `max`, drawn as graduations with a number on every `label_every`th one,
//! so the column reads as a ruler sliding under the band rather than as
//! words turning. `step: 0.0` — the default — is the column of words above,
//! unchanged in every respect.
//!
//! There is deliberately NO tick-pitch property. `row_height` is the pitch
//! between graduations AND the distance the gesture moves by, because they
//! are the same number: a scale that drew its ticks at one pitch and slid at
//! another would be a ruler whose marks disagreed with its own motion. A
//! finer ruler is a smaller `row_height`, and the drag gets finer with it.
//!
//! Endless means CYCLIC. `loop_items` carries a scale round its declared
//! `min..max` for as long as the finger keeps going; a scale with no ends at
//! all is not expressible here, because the drum's whole travel is
//! `count * row_height`, and pretending otherwise would be a different
//! widget rather than a property on this one.
//!
//! # A ruler that fills the room
//!
//! `fill_travel` is the one property that suspends the fixed-count rule
//! above, and it suspends it only along the travel. With it set, the walk
//! along the travel is honoured exactly as the host wrote it — `width: Fill`
//! on a ruler lying down — and the number of graduations is however many the
//! laid-out extent holds at `row_height`, rounded down to a whole odd count
//! so there is still a middle for the band. A graduated scale usually wants
//! to span the panel it measures, and a row count picked to approximate one
//! container is a number that is wrong at every other window size.
//!
//! Off — the default — nothing moves: `visible_items` rules exactly as it
//! always has. On, `visible_items` is the FALLBACK, for a walk with no room
//! to read out of: `Fit` is the question "what is your natural size", and a
//! drum's natural size is still its declared rows.
//!
//! Everything measured in ROWS reads the derived count rather than the
//! declared one, and the fade at the ends is measured in rows. The ink and
//! the size a row gives up is its distance from the band over HALF THE ROWS
//! ON SCREEN, so a fade left on a declared 41 while the extent grew to 121
//! rows would be fully spent a third of the way out and leave the remaining
//! two thirds of the ruler flat. `rows_in` is the one answer the fade, the
//! reach and the draw all take, which is what stops them being counted
//! differently.
//!
//! # Standing it on its side
//!
//! `axis` says which way the drum travels; `DragAxis::Vertical`, the
//! default, is the picker above to the pixel. Lying down, the rows run left
//! to right and the band stands across them. The axis across the travel is
//! what the columns are laid out along and what the hit test splits, so a
//! picker lying down holds exactly ONE column: the cross axis has been spent
//! on the ruler itself.
use crate::{
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    badge::measure,
    makepad_derive_widget::*,
    makepad_draw::*,
    picker_parts::text_y,
    scroll_motion::{
        estimate_release_velocity, push_sample, FrameClock, ScrollSample,
        FLING_DECEL_RATE_PER_MS, FLING_MIN_TOTAL_DELTA,
    },
    slider::{format_readout, DragAxis},
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
        /** the travel takes its length from the room, not from visible_items */
        fill_travel: false
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
        // The row in the band is the chosen row, so it is written in the ink
        // a chosen row in a menu or a file tree is written in, on the ground
        // those rows are drawn on (the band, below). Every theme settles that
        // ink against that ground; the body ink it used to be was never
        // settled against any fill at all.
        /** ink of the row in the band */
        color_selected: theme.color_label_inner_active
        /** ink of the row in the band while the picker is disabled */
        color_selected_disabled: theme.color_label_inner

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

            // The band marks the chosen row, which is what a selected row
            // in a list, a menu or a tree marks, so it is drawn in their
            // ground and not in a slider's value fill: nothing is ever
            // written on a fill, so no theme holds an ink to one, and a
            // palette that pushed the fill dark left the digits in the band
            // dark on dark, and the darker hover fill darker still.
            //
            // The pointer and a spin move the band to the other end of a
            // chosen row, the rung a menu row's gradient runs to. In the
            // dark theme that is the dimmer wash, which the light digits
            // stand further off; in the light one a hair deeper, still well
            // clear. Its louder neighbour would have been the obvious step
            // and it is the wrong one: on the dark theme it pales the band
            // toward the digits until they barely read. A built palette
            // gives every chosen-row rung the same colour and settles the
            // ink against each of them, so there the band holds still.
            //
            // Focus keeps the resting ground and says which column the
            // arrows turn with a ring, since a lighter slice under the
            // digits is one more ground for them to fail on. Disabled drops
            // the band to the disabled face, and the digits to the label a
            // control wears at rest, which every theme settles against a
            // ground that close to the page: the colour going out of the
            // band is what says the picker is off.
            band_color: uniform(theme.color_outset_active)
            band_color_hover: uniform(theme.color_outset_2_active)
            band_color_focus: uniform(theme.color_outset_active)
            band_color_drag: uniform(theme.color_outset_2_active)
            band_color_disabled: uniform(theme.color_outset_disabled)
            /** ring round the column the arrow keys turn */
            band_ring_color: uniform(theme.color_bevel_focus)

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
                // holds in each one is that column's answer. Its thickness
                // is the row height Rust laid the rows out at, handed over
                // so the band and the row inside it cannot drift apart.
                // Lying down it stands across the travel instead: the
                // graduation it frames is the answer the same way.
                let inset = self.border_size + 1.0
                let mid_y = (self.rect_size.y - self.row_px) * 0.5
                let mid_x = (self.rect_size.x - self.row_px) * 0.5
                if self.vertical > 0.5 {
                    sdf.box(
                        inset
                        mid_y
                        max(self.rect_size.x - inset * 2., 1.0)
                        self.row_px
                        self.border_radius
                    )
                    sdf.fill(band)

                    // The column the keyboard is on, so a picker with
                    // several of them says which one the arrows will turn.
                    // A ring and not a brighter slice: the digits sit on
                    // the band, and a slice lighter than it is a ground
                    // no theme settled their ink against.
                    sdf.rect(self.band_x + 0.5, mid_y + 0.5, self.band_w - 1.0, self.row_px - 1.0)
                    sdf.stroke(self.band_ring_color * self.focus, 1.0)
                }
                else {
                    sdf.box(
                        mid_x
                        inset
                        self.row_px
                        max(self.rect_size.y - inset * 2., 1.0)
                        self.border_radius
                    )
                    sdf.fill(band)

                    // Lying down the lit slice runs across the travel too,
                    // so `band_x` and `band_w` are read down the page.
                    sdf.rect(mid_x + 0.5, self.band_x + 0.5, self.row_px - 1.0, self.band_w - 1.0)
                    sdf.stroke(self.band_ring_color * self.focus, 1.0)
                }

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

    /** A ruler that slides under the band. One scale column, lying down. */
    mod.widgets.SlidingRuler = mod.widgets.WheelPicker{
        /** which way the scale travels: Horizontal or Vertical */
        axis: mod.widgets.DragAxis.Horizontal
        /** graduations on screen; an even count gains one 9..81 step 2 */
        visible_items: 41
        /** the graduation pitch, and the rate the drag moves at 4..32 step 1 */
        row_height: 10.0
        /** how deep the ruler is, across its travel 24..160 step 4 */
        column_width: 40.0
    }

    /** The same ruler stood on end. */
    mod.widgets.SlidingRulerVertical = mod.widgets.SlidingRuler{
        axis: mod.widgets.DragAxis.Vertical
        column_width: 72.0
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
    /// Which way the drum travels, as the shader can read it: 1.0 upright,
    /// 0.0 lying down. The band has to stand ACROSS the travel, and only
    /// Rust knows which way that is.
    #[live(1.0)]
    vertical: f32,
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

    /// Where a scale starts and where it ends. A column that names a
    /// `step` over a range is a SCALE: it holds no `items` at all, and its
    /// rows are the numbers `min`, `min + step`, … up to `max`.
    #[live]
    pub min: f64,
    #[live]
    pub max: f64,
    /// The value between one graduation and the next. Zero — the default —
    /// is a column of words, unchanged in every respect.
    ///
    /// There is deliberately no separate tick-pitch property beside it: the
    /// picker's `row_height` is the pitch, and it is the same number the
    /// gesture moves by, which is what stops a scale from sliding at a rate
    /// its own graduations disagree with.
    #[live]
    pub step: f64,
    /// Decimals on a graduation's number, and the unit after it. Spelled
    /// the way `Slider` spells them and formatted by the same function, so
    /// a graduation and a readout beside it cannot disagree about either.
    #[live]
    pub precision: usize,
    #[live]
    pub unit: String,
    /// One graduation in this many is a MAJOR one: a longer tick, and the
    /// only kind that carries a number. Zero — the default — is every
    /// tenth, which is the grouping a ruler is read in.
    #[live]
    pub label_every: usize,
    /// How far a graduation reaches across the column, in pixels, and how
    /// far a major one does. Zero — the default — takes both from the
    /// column's own width, so a scale that says nothing still draws.
    #[live]
    pub tick_len: f64,
    #[live]
    pub tick_len_major: f64,
}

impl WheelColumn {
    /// A column that names a step over a range is a scale rather than a
    /// list of words. There is no mode to set and no discriminant to name:
    /// the step IS the distinction, because a column of words has no
    /// distance between one row and the next.
    fn is_scale(&self) -> bool {
        scale_count(self.min, self.max, self.step).is_some()
    }

    /// How many rows the column holds: a scale's graduations, or the items
    /// it was handed.
    ///
    /// Everything else rests on this. `Drum` is built from a count, and a
    /// scale column's `items` are empty forever — so a scale counted by its
    /// items would be a drum of no rows, whose `bound` is 0.0 and whose
    /// `rows_in_view` is empty. It would neither move nor draw.
    fn count(&self) -> usize {
        scale_count(self.min, self.max, self.step).unwrap_or(self.items.len())
    }

    /// The number graduation `index` stands for, or `None` on a column of
    /// words, which has no number to stand for.
    fn value_of(&self, index: usize) -> Option<f64> {
        scale_count(self.min, self.max, self.step).map(|_| self.min + index as f64 * self.step)
    }

    /// How often a graduation is a major one.
    fn labels_every(&self) -> usize {
        if self.label_every == 0 {
            10
        } else {
            self.label_every
        }
    }

    fn is_major(&self, index: usize) -> bool {
        index % self.labels_every() == 0
    }

    /// What a plain graduation and a major one reach across a column
    /// `across` pixels deep.
    ///
    /// A third of the depth for a major one, not half: the rest of the
    /// column is where the NUMBER goes, and a tick that took half of a
    /// narrow ruler would leave every number too wide to fit and skipped.
    fn tick_lengths(&self, across: f64) -> (f64, f64) {
        let minor = if self.tick_len > 0.0 { self.tick_len } else { across * 0.18 };
        let major = if self.tick_len_major > 0.0 { self.tick_len_major } else { across * 0.34 };
        (minor, major)
    }
}

/// The most graduations a scale may hold. A step small enough against a
/// range wide enough asks for a drum nothing could draw and arithmetic that
/// would lose the row it was on; the cap makes such a scale coarse rather
/// than making it hang.
const SCALE_MAX: usize = 100_000;

/// How many graduations `min..=max` holds at `step`, or `None` when the
/// column is a list of words rather than a scale.
///
/// Both ends are graduations, which is why there is a `+ 1`: a 0..10 scale
/// stepping by one holds eleven marks, not ten. A range the step does not
/// divide is rounded rather than refused, because the alternative is a
/// scale that silently stops short of the `max` its own host declared.
fn scale_count(min: f64, max: f64, step: f64) -> Option<usize> {
    if !(step > 0.0) || !(max > min) {
        return None;
    }
    let spans = ((max - min) / step).round();
    if !spans.is_finite() {
        return Some(SCALE_MAX);
    }
    Some(((spans.max(0.0) as i64) as usize).saturating_add(1).min(SCALE_MAX))
}

/// A graduation's own mark is one pixel thick along the travel, whatever
/// the pitch is. The pitch is the space the mark stands in, not the mark.
const TICK_PX: f64 = 1.0;

/// The air a graduation's number keeps between itself and its tick, and
/// between itself and the next number.
const LABEL_GAP: f64 = 3.0;

/// The travel axis and the one across it, in that order.
///
/// Everything the picker measures — a finger, the rect it landed in, a
/// wheel notch, a row's place on screen — is read through this one word, so
/// standing the control on its side turns all of them together or none of
/// them. A cross-axis reading that did not flip would hit-test a ruler lying
/// down against columns stacked the other way.
fn split(vertical: bool, p: DVec2) -> (f64, f64) {
    if vertical {
        (p.y, p.x)
    } else {
        (p.x, p.y)
    }
}

/// The quad one graduation draws, from the leading edge of its own cell
/// along the travel and of its column across it.
fn tick_rect(vertical: bool, lead: f64, cross: f64, pitch: f64, len: f64) -> Rect {
    let thick = TICK_PX.min(pitch);
    let at = lead + (pitch - thick) * 0.5;
    if vertical {
        Rect { pos: dvec2(cross, at), size: dvec2(len.max(1.0), thick) }
    } else {
        Rect { pos: dvec2(at, cross), size: dvec2(thick, len.max(1.0)) }
    }
}

/// The box a graduation's number has to fit inside, as a width and a
/// height.
///
/// Numbers run along x whichever way the ruler does, so the two crowd them
/// differently. Standing up the numbers are stacked: what limits their WIDTH
/// is the column's own depth beside the tick, and what limits their HEIGHT
/// is the pitch to the next numbered graduation. Lying down the two swap.
fn label_box(vertical: bool, pitch: f64, label_every: usize, across: f64, gap: f64) -> (f64, f64) {
    let along = pitch * label_every.max(1) as f64 - gap * 2.0;
    let depth = across - gap;
    if vertical {
        (depth, along)
    } else {
        (along, depth)
    }
}

/// Whether a graduation's number is drawn at all.
///
/// It is SKIPPED, never clipped and never shrunk — the rule `waveform` uses
/// for a marker's name. Half a number says less than no number and costs the
/// reader a second working out that it is half a number; the graduation's
/// tick is still there, and the number it wanted is the one two marks along.
fn label_fits(width: f64, height: f64, room_x: f64, room_y: f64) -> bool {
    width <= room_x && height <= room_y
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

/// The most graduations a ruler will draw however much room it is handed.
/// A turtle can report an extent no finite number of rows would fill, and a
/// count taken straight from one is a draw loop that does not come back —
/// and, before the cap, a cast that saturates at `usize::MAX` and an
/// `odd_rows` that overflows adding one to it.
const FILL_MAX_ROWS: usize = 10_001;

/// How many rows an extent holds at pitch `row`, for a travel axis that
/// takes its length from the room rather than from a declared count.
///
/// Rounded DOWN and then made odd, for the two reasons the declared count is
/// whole and odd: a part row at the end of a ruler is a graduation the band
/// could never centre on, and a drum with no middle row has nothing to put
/// in the band. `None` is an extent that says nothing usable — a walk that
/// asked for no room, a turtle not yet measured, or less room than a single
/// row — and the declared count is the only answer there is then.
fn rows_for_extent(extent: f64, row: f64) -> Option<usize> {
    let row = row.max(1.0);
    if !(extent >= row) || !extent.is_finite() {
        return None;
    }
    Some(odd_rows(((extent / row).floor() as usize).min(FILL_MAX_ROWS)))
}

/// How many rows are on screen, all told: the room's answer when the control
/// asked for the room to decide and the room had one, and the declared count
/// otherwise.
///
/// It is a free function rather than a method so the widget and its tests
/// read the SAME arithmetic, and so there is exactly one place where the two
/// counts can be told apart. Everything measured in rows — the fade, the
/// reach, the rows drawn — comes through here, because a fade counted
/// against one of them while the rows were counted against the other is a
/// gradient that stops before the ruler does.
fn rows_on_screen(fill_travel: bool, visible_items: usize, extent: f64, row: f64) -> usize {
    if fill_travel {
        if let Some(rows) = rows_for_extent(extent, row) {
            return rows;
        }
    }
    odd_rows(visible_items)
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
    /// One graduation of a scale column. A Rust quad rather than a ladder
    /// in `draw_bg`'s shader: a ladder would have to be told how far the
    /// drum had slid, in an f32, and a looping drum's travel is unbounded
    /// by design. Drawing a second material between `draw_bg`'s `begin` and
    /// `end` is what `time_picker` and `kbd` already do — each writes only
    /// its own area, so neither clobbers the other.
    #[live]
    draw_tick: DrawColor,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[apply_default]
    animator: Animator,

    /// The columns, left to right — or top to bottom when the picker is
    /// lying down. A picker lying down holds exactly ONE column: the axis
    /// across the travel has been spent on the ruler itself.
    #[live]
    pub columns: Vec<WheelColumn>,

    /// Which way the drum travels. `Vertical`, the default, is the picker
    /// this widget has always been, to the pixel. `Horizontal` lays the
    /// rows out left to right and stands the band across them.
    #[live(DragAxis::Vertical)]
    pub axis: DragAxis,

    /// Rows on screen. Odd, so there is a true centre; an even count is
    /// raised by one rather than refused.
    ///
    /// It settles the length of the control UNLESS `fill_travel` is set, in
    /// which case the room settles it and this is only what a walk with no
    /// room to read falls back to.
    #[live(5)]
    pub visible_items: usize,
    /// The travel axis takes its length from the room it is given rather
    /// than from `visible_items`.
    ///
    /// Off — the default — is the picker's standing contract: a drum is as
    /// long as the rows it declares, whatever it has been laid out in. On,
    /// the walk along the travel is honoured as the host wrote it and the
    /// count is read back out of the laid-out extent afterwards, which is
    /// what lets a ruler be `width: Fill` and span the panel it measures.
    ///
    /// It does not make a `Fit` walk grow. `Fit` asks what the natural size
    /// is, and a drum's natural size is its declared rows; there has to be
    /// room before a count can be read out of it.
    #[live]
    pub fill_travel: bool,
    /// The distance from one row to the next, in pixels — and, on a scale
    /// column, the pitch of the graduations as well.
    ///
    /// Those are ONE number on purpose. It is also the distance the drag
    /// and the wheel move the drum by, so a ruler cannot slide at a rate
    /// its own marks disagree with; a separate tick pitch is refused for
    /// exactly that reason.
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
    /// The in-band ink while disabled. The band goes to the disabled face
    /// then, which is nearly the page, so this is an ink settled against
    /// the page and not the faint disabled one, which no theme holds to
    /// anything and which reads at about one and a half to one there.
    #[live]
    pub color_selected_disabled: Vec4f,

    #[rust]
    spins: Vec<Spin>,
    /// Where each column was last drawn, as an offset from the widget's
    /// leading edge ACROSS the travel, and a width. The hit test reads
    /// these rather than recomputing the layout, so a press lands on the
    /// column the eye is looking at.
    #[rust]
    spans: Vec<(f64, f64)>,
    /// The shape each column's drum had at the last sync: how many rows and
    /// whether it comes round. A scale column's length says nothing about
    /// its count, so this is the only thing that can notice a live `min`,
    /// `max` or `step` moving under a stored offset.
    #[rust]
    shapes: Vec<(usize, bool)>,
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
        self.shapes.clear();
        self.grab = None;
    }
}

impl WheelPicker {
    /// The rows a `Fit` walk asks for: the declared count, always, because
    /// `Fit` is the question this is the natural answer to.
    fn rows(&self) -> usize {
        odd_rows(self.visible_items)
    }

    /// The rows actually on screen, given the extent the travel axis was
    /// laid out at. The same as `rows` unless `fill_travel` asked for the
    /// room to decide.
    fn rows_in(&self, extent: f64) -> usize {
        rows_on_screen(self.fill_travel, self.visible_items, extent, self.row())
    }

    fn row(&self) -> f64 {
        self.row_height.max(1.0)
    }

    /// Which way the drum travels. Everything the picker measures is read
    /// through this one word.
    fn is_vertical(&self) -> bool {
        matches!(self.axis, DragAxis::Vertical)
    }

    fn drum(&self, col: usize) -> Drum {
        let c = &self.columns[col];
        Drum::new(c.count(), self.row(), c.loop_items)
    }

    /// Match the running state to the columns. Cheap when nothing changed,
    /// which is every frame but the first after an apply.
    ///
    /// The SHAPE of each column's drum is watched as well as the number of
    /// columns. A scale column holds no `items`, so its count moves when
    /// `min`, `max` or `step` do and nothing about the column's length would
    /// ever say so; an offset left standing past the new end then puts
    /// `rows_in_view` outside the column altogether and the picker draws
    /// nothing at all. The pitch is deliberately NOT watched: a live
    /// `row_height` drag must behave exactly as it always did.
    fn sync(&mut self) {
        let row = self.row();
        let shapes: Vec<(usize, bool)> =
            self.columns.iter().map(|c| (c.count(), c.loop_items)).collect();
        if self.spins.len() != self.columns.len() {
            self.spins = self
                .columns
                .iter()
                .zip(shapes.iter())
                .map(|(c, &(count, looping))| {
                    let drum = Drum::new(count, row, looping);
                    Spin { offset: drum.bound(drum.offset_of(c.selected)), ..Default::default() }
                })
                .collect();
            self.focus_col = self.focus_col.min(self.columns.len().saturating_sub(1));
            self.grab = None;
        } else if self.shapes != shapes {
            // A column whose drum changed shape under a standing offset is
            // re-bounded rather than reset: the place is still the one the
            // hand left it in, it is only the end that has moved.
            for (col, &shape) in shapes.iter().enumerate() {
                if self.shapes.get(col) == Some(&shape) {
                    continue;
                }
                let drum = Drum::new(shape.0, row, shape.1);
                self.spins[col].glide = None;
                self.spins[col].offset = drum.bound(self.spins[col].offset);
            }
        } else {
            return;
        }
        self.shapes = shapes;
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

    /// The column an offset ACROSS the travel, from the widget's leading
    /// edge on that axis, belongs to. Lying down that offset is a y, which
    /// is why the hit test reads it through `split` rather than reaching
    /// for `abs.x` — and why a picker lying down holds one column.
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

    /// The number `column` is holding: a scale's graduation, or, on a
    /// column of words, the row's own index. A host reading a ruler should
    /// not be made to re-derive `min + index * step` and get the rounding
    /// subtly different from the number the ruler is drawing.
    pub fn value(&self, column: usize) -> f64 {
        let index = self.selected(column);
        self.columns.get(column).and_then(|c| c.value_of(index)).unwrap_or(index as f64)
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
        let vertical = self.is_vertical();

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
                let (along, across) = split(vertical, fe.abs);
                let (_, rect_across) = split(vertical, fe.rect.pos);
                let col = self.column_at(across - rect_across);
                self.focus_col = col;
                // A hand on a spinning wheel stops it where it stands.
                let caught = self.spins[col].glide.take().is_some();
                self.spins[col].samples.clear();
                push_sample(&mut self.spins[col].samples, along, fe.time);
                self.grab = Some(Grab {
                    col,
                    offset: self.spins[col].offset,
                    abs: along,
                    caught,
                });
                self.animator_play(cx, ids!(drag.on));
                self.draw_bg.redraw(cx);
            }
            Hit::FingerMove(fe) => {
                let Some(grab) = self.grab else {
                    return;
                };
                let (along, _) = split(vertical, fe.abs);
                push_sample(&mut self.spins[grab.col].samples, along, fe.time);
                // The finger and the drum run opposite ways: dragging down —
                // or, lying down, to the right — brings earlier rows in.
                self.place(cx, grab.col, grab.offset - (along - grab.abs));
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
                // A press taken away is no tap and no throw: the drum settles
                // from rest on the nearest row.
                let (velocity, travel) = if fe.cancelled {
                    (0.0, 0.0)
                } else {
                    estimate_release_velocity(&self.spins[col].samples)
                };
                let carry = if fe.cancelled {
                    0.0
                } else if travel.abs() > FLING_MIN_TOTAL_DELTA {
                    // The finger's velocity is the drum's, negated.
                    spin_travel(-velocity, FLING_DECEL_RATE_PER_MS)
                } else if !grab.caught {
                    // A press that did not move and did not stop anything
                    // is a tap on a row: the distance from the band to the
                    // finger is exactly the travel that brings that row in.
                    let (along, _) = split(vertical, fe.abs);
                    let (pos, _) = split(vertical, fe.rect.pos);
                    let (size, _) = split(vertical, fe.rect.size);
                    along - (pos + size * 0.5)
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
                // The pair of arrows that points ALONG the travel turns the
                // drum and the pair across it chooses the column, so the
                // keys keep pointing the way the rows actually move when the
                // picker is stood on its side. Upright this is the mapping
                // it has always had.
                let (back, fwd, prev, next) = if vertical {
                    (KeyCode::ArrowUp, KeyCode::ArrowDown, KeyCode::ArrowLeft, KeyCode::ArrowRight)
                } else {
                    (KeyCode::ArrowLeft, KeyCode::ArrowRight, KeyCode::ArrowUp, KeyCode::ArrowDown)
                };
                let landing = match ke.key_code {
                    key if key == back => Some(drum.step(index, -1)),
                    key if key == fwd => Some(drum.step(index, 1)),
                    // Home and End are the first and last row of the list,
                    // not of the drum: on a column that comes round, the
                    // last row may well be one step backwards.
                    KeyCode::Home => Some(0),
                    KeyCode::End => Some(drum.count.saturating_sub(1)),
                    // The cross arrows choose the column, since Tab belongs
                    // to focus traversal and a picker's columns are as much
                    // one control as a slider's two ends are.
                    key if key == prev => {
                        self.focus_col = col.saturating_sub(1);
                        self.draw_bg.redraw(cx);
                        None
                    }
                    key if key == next => {
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
                let (_, across) = split(vertical, e.abs);
                let (_, rect_across) = split(vertical, e.rect.pos);
                let col = self.column_at(across - rect_across);
                // Only the notch that runs ALONG the travel counts. A plain
                // vertical wheel over a ruler lying down belongs to the page
                // the ruler is sitting in, not to the ruler.
                let travel = wheel_travel(split(vertical, e.scroll).0, row);
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
        // The rows a `Fit` walk asks for. What is actually DRAWN is counted
        // again below, once the layout has settled how long the travel is.
        let fit_rows = self.rows();
        let row = self.row();
        let vertical = self.is_vertical();

        // A drum's size is the rows it shows, not the room it is given: a
        // half row at the edge of the well would sit outside the band's
        // arithmetic and read as a list that had been cut off. `Fit` asks
        // for that natural size; any other walk is honoured as written, and
        // the columns keep their own widths and start at the left edge.
        //
        // `fill_travel` turns that round along the travel, and only there:
        // the walk is left as the host wrote it, and the count is read back
        // out of the extent once the layout has settled it. A `Fit` walk is
        // still the natural size either way, because there is no room to
        // read out of a walk that is asking what the natural size is.
        //
        // The travel axis takes the rows and the axis across it takes the
        // columns, so standing the picker on its side TRANSPOSES the walk.
        // Nothing in the shader could do this: a `Fit` width that has to
        // become `rows * row` is decided here or not at all.
        let mut walk = walk;
        let along = Size::Fixed(fit_rows as f64 * row);
        let across = Size::Fixed(self.body_width());
        if vertical {
            if matches!(walk.height, Size::Fit { .. }) {
                walk.height = along;
            }
            if matches!(walk.width, Size::Fit { .. }) {
                walk.width = across;
            }
        } else {
            if matches!(walk.width, Size::Fit { .. }) {
                walk.width = along;
            }
            if matches!(walk.height, Size::Fit { .. }) {
                walk.height = across;
            }
        }

        self.draw_bg.row_px = row as f32;
        self.draw_bg.vertical = if vertical { 1.0 } else { 0.0 };
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
        let (along_pos, cross_pos) = split(vertical, rect.pos);
        let (along_size, _) = split(vertical, rect.size);
        let mid = along_pos + along_size * 0.5;
        // Rows between the band and the edge of the well, and how far out a
        // row may be before none of it is inside.
        //
        // The count is read back from the extent the travel was LAID OUT at
        // rather than from the one the walk asked for, because a ruler that
        // fills its room only finds out how long it is here. Everything
        // below is measured in these rows — the fade included, which on the
        // declared count would be spent a third of the way along a ruler
        // three times its declared length and leave the rest of it flat.
        let rows = self.rows_in(along_size);
        let half = rows as f64 * 0.5;
        let reach = half + 0.5;
        let base_font = self.draw_text.text_style.font_size;
        let chosen_ink = if self.animator_in_state(cx, ids!(disabled.on)) {
            self.color_selected_disabled
        } else {
            self.color_selected
        };

        self.spans.clear();
        for col in 0..self.columns.len() {
            let width = self.col_width(col);
            let start = self.col_start(col);
            self.spans.push((start, width));
            let drum = self.drum(col);
            let offset = self.spins[col].offset;
            // A scale GENERATES its rows out of numbers where a column of
            // words looks them up. What the generated ones need is lifted
            // out of the row loop here, so the loop borrows nothing.
            let scale = self.columns[col].is_scale();
            let (min, step) = (self.columns[col].min, self.columns[col].step);
            let precision = self.columns[col].precision;
            let unit = if scale { self.columns[col].unit.clone() } else { String::new() };
            let every = self.columns[col].labels_every();
            let (tick_len, tick_len_major) = self.columns[col].tick_lengths(width);
            // A number stands clear of the longest tick, and what is left of
            // the column is the room it has to fit in.
            let inset = if scale { tick_len_major + LABEL_GAP } else { 0.0 };
            let (room_x, room_y) = label_box(vertical, row, every, width - inset, LABEL_GAP);
            for (index, d) in drum.rows_in_view(offset, reach) {
                let ink_left = fade_at(d, half, self.dim_far);
                let size = base_font * fade_at(d, half, self.shrink_far) as f32;
                // The row in the band keeps the strong ink and its
                // neighbours hand theirs over as they leave, mixed rather
                // than switched so nothing pops as the drum turns.
                let mut ink = chosen_ink.mix(self.color_item, (d.abs() as f32).min(1.0));
                ink.w *= ink_left as f32;
                // Where this row's own cell starts along the travel, and
                // where its column starts across it.
                let lead = mid + d * row - row * 0.5;
                let cross = cross_pos + start;
                // The tick is drawn BEFORE the label is looked for, because
                // a scale has no label to look for on most of its rows and
                // a lookup that runs first would skip the graduation too.
                let text = if scale {
                    let major = self.columns[col].is_major(index);
                    self.draw_tick.color = ink;
                    let len = if major { tick_len_major } else { tick_len };
                    self.draw_tick.draw_abs(cx, tick_rect(vertical, lead, cross, row, len));
                    if !major {
                        continue;
                    }
                    Some(format_readout(min + index as f64 * step, precision, &unit))
                } else {
                    self.columns[col].items.get(index).cloned()
                };
                let Some(text) = text else {
                    continue;
                };
                self.draw_text.text_style.font_size = size;
                self.draw_text.color = ink;
                let text_width = measure(&self.draw_text, cx, &text);
                if scale && !label_fits(text_width, size as f64, room_x, room_y) {
                    continue;
                }
                // `draw_abs` takes the top of the line box, not the top of
                // the ink: a glyph's ink starts about 0.30 of the size
                // below it, and `text_y` is where the crate keeps that.
                let (x, y) = if vertical {
                    let x = if scale {
                        cross + inset
                    } else {
                        cross + (width - text_width) * 0.5
                    };
                    (x, text_y(size as f64, lead, row))
                } else {
                    let y = text_y(size as f64, cross + inset, width - inset);
                    (lead + (row - text_width) * 0.5, y)
                };
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
            .map(|(col, c)| {
                let index = self.selected(col);
                // A scale holds no `items`, so indexing them would answer
                // an empty string for every graduation. Its row is a number
                // it makes, and so is its text.
                match c.value_of(index) {
                    Some(value) => format_readout(value, c.precision, &c.unit),
                    None => c.items.get(index).cloned().unwrap_or_default(),
                }
            })
            .collect::<Vec<String>>()
            .join(" ")
    }
}

impl WheelPickerRef {
    /// The row `column` is holding in the band.
    pub fn selected(&self, column: usize) -> usize {
        self.borrow().map(|inner| inner.selected(column)).unwrap_or(0)
    }

    /// The number `column` is holding: a scale's graduation, or a row's own
    /// index on a column of words.
    pub fn value(&self, column: usize) -> f64 {
        self.borrow().map(|inner| inner.value(column)).unwrap_or(0.0)
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

    // ---- the scale column ------------------------------------------------

    fn scale(min: f64, max: f64, step: f64) -> WheelColumn {
        WheelColumn { min, max, step, ..Default::default() }
    }

    #[test]
    fn a_scale_counts_its_graduations_from_its_range() {
        // Both ends are graduations, which is the whole reason for the + 1:
        // 0..10 by ones is eleven marks, not ten.
        assert_eq!(scale_count(0.0, 10.0, 1.0), Some(11));
        assert_eq!(scale_count(-1.0, 1.0, 0.5), Some(5));
        assert_eq!(scale_count(20.0, 20_000.0, 10.0), Some(1999));
        // A range the step does not divide is rounded rather than refused:
        // the alternative is a scale that quietly stops short of the max
        // its own host declared.
        assert_eq!(scale_count(0.0, 1.0, 0.3), Some(4));
    }

    #[test]
    fn a_column_that_names_no_step_is_a_list_of_words() {
        // This is the whole discriminant. `step: 0.0` has to leave the text
        // column exactly as it was, so it must not even look like a scale.
        assert_eq!(scale_count(0.0, 10.0, 0.0), None);
        assert_eq!(scale_count(0.0, 10.0, -1.0), None, "nor does a step that runs backwards");
        assert_eq!(scale_count(10.0, 0.0, 1.0), None, "nor a range that does");
        assert_eq!(scale_count(5.0, 5.0, 1.0), None, "nor a range of nothing");
        assert_eq!(scale_count(f64::NAN, 1.0, 1.0), None);
        assert_eq!(scale_count(0.0, 1.0, f64::NAN), None);
    }

    #[test]
    fn a_scale_that_would_never_end_is_capped_rather_than_left_to_hang() {
        assert_eq!(scale_count(0.0, 1.0, f64::MIN_POSITIVE), Some(SCALE_MAX));
        assert_eq!(scale_count(0.0, f64::INFINITY, 1.0), Some(SCALE_MAX));
        assert_eq!(scale_count(0.0, 1e18, 1e-9), Some(SCALE_MAX));
    }

    #[test]
    fn a_scale_column_carries_no_items_and_still_has_a_drum() {
        // The edit the whole ruler rests on: a count of zero would give a
        // drum that neither moves nor draws.
        let c = scale(0.0, 100.0, 5.0);
        assert!(c.items.is_empty());
        assert!(c.is_scale());
        assert_eq!(c.count(), 21);
        let d = Drum::new(c.count(), 10.0, false);
        assert_eq!(d.index_at(d.offset_of(7)), 7);
        assert!(!d.rows_in_view(d.offset_of(7), 2.5).is_empty());
    }

    #[test]
    fn the_graduation_in_the_band_is_the_number_the_scale_names() {
        let c = scale(20.0, 20_000.0, 10.0);
        assert_eq!(c.value_of(0), Some(20.0));
        assert_eq!(c.value_of(80), Some(820.0));
        // A column of words has no number to stand for, and says so rather
        // than answering zero.
        let mut words = WheelColumn::default();
        words.items = vec!["Small".to_string(), "Large".to_string()];
        assert_eq!(words.value_of(1), None);
        assert_eq!(words.count(), 2, "and it still counts by its items");
    }

    #[test]
    fn one_graduation_in_label_every_is_a_major_one() {
        let c = scale(0.0, 100.0, 1.0);
        assert_eq!(c.labels_every(), 10, "a scale that names none groups by ten");
        assert!(c.is_major(0) && c.is_major(10) && c.is_major(100));
        assert!(!c.is_major(9) && !c.is_major(11));
        let mut c = scale(0.0, 100.0, 1.0);
        c.label_every = 4;
        assert!(c.is_major(8) && !c.is_major(7));
    }

    #[test]
    fn a_scale_that_names_no_tick_lengths_takes_them_from_the_column() {
        let c = scale(0.0, 10.0, 1.0);
        let (minor, major) = c.tick_lengths(40.0);
        assert!(minor > 0.0 && major > minor && major < 40.0);
        let mut c = scale(0.0, 10.0, 1.0);
        c.tick_len = 5.0;
        c.tick_len_major = 9.0;
        assert_eq!(c.tick_lengths(40.0), (5.0, 9.0));
    }

    // ---- standing it on its side ----------------------------------------

    #[test]
    fn the_axis_swaps_what_travel_means_and_what_across_means() {
        // A finger, a rect, a wheel notch and a row's place on screen all
        // read through this one word, so they turn together or not at all.
        assert_eq!(split(true, dvec2(3.0, 7.0)), (7.0, 3.0), "upright the drum travels in y");
        assert_eq!(split(false, dvec2(3.0, 7.0)), (3.0, 7.0), "lying down, in x");
        // Which is what makes the hit test flip: the offset handed to
        // `column_at` is the second of the pair either way round.
        let (abs, origin) = (dvec2(120.0, 40.0), dvec2(100.0, 10.0));
        assert_eq!(split(true, abs).1 - split(true, origin).1, 20.0);
        assert_eq!(split(false, abs).1 - split(false, origin).1, 30.0);
    }

    #[test]
    fn a_graduation_is_a_hairline_across_the_way_the_drum_moves() {
        // The pitch is the room the mark stands in, not the mark.
        let up = tick_rect(true, 100.0, 20.0, 12.0, 8.0);
        assert_eq!((up.size.x, up.size.y), (8.0, 1.0));
        assert_eq!((up.pos.x, up.pos.y), (20.0, 105.5), "centred in its own pitch");
        let flat = tick_rect(false, 100.0, 20.0, 12.0, 8.0);
        assert_eq!((flat.size.x, flat.size.y), (1.0, 8.0));
        assert_eq!((flat.pos.x, flat.pos.y), (105.5, 20.0));
        // A pitch finer than the mark gives the whole pitch to the mark
        // rather than a rect wider than the space it has.
        let fine = tick_rect(false, 0.0, 0.0, 0.5, 8.0);
        assert_eq!(fine.size.x, 0.5);
    }

    // ---- which numbers are drawn ----------------------------------------

    #[test]
    fn a_number_is_skipped_when_it_does_not_fit_the_room_it_has() {
        // Lying down, what crowds a number's WIDTH is the pitch to the next
        // numbered graduation and what crowds its HEIGHT is the column's own
        // depth beside the tick.
        let (room_x, room_y) = label_box(false, 10.0, 10, 40.0, 3.0);
        assert_eq!((room_x, room_y), (94.0, 37.0));
        assert!(label_fits(28.0, 11.0, room_x, room_y));
        assert!(!label_fits(120.0, 11.0, room_x, room_y), "wider than the next number's place");
        assert!(!label_fits(28.0, 60.0, room_x, room_y), "taller than the ruler is deep");
        // Standing up the two swap, because the numbers are stacked.
        assert_eq!(label_box(true, 10.0, 10, 40.0, 3.0), (37.0, 94.0));
    }

    #[test]
    fn a_ruler_too_fine_to_number_still_draws_its_graduations() {
        // Four pixels between marks with every mark numbered leaves no room
        // at all. The numbers go and the ticks stay, which is what a ruler
        // does; nothing is clipped and nothing is shrunk to fit.
        let (room_x, room_y) = label_box(false, 4.0, 1, 40.0, 3.0);
        assert!(!label_fits(18.0, 11.0, room_x, room_y));
        // Number one mark in ten at that pitch and they fit again.
        let (room_x, room_y) = label_box(false, 4.0, 10, 40.0, 3.0);
        assert!(label_fits(18.0, 11.0, room_x, room_y));
    }

    #[test]
    fn a_scale_reads_the_same_number_as_a_readout_beside_it() {
        // Both go through `format_readout`, so the decimals and the unit
        // cannot drift apart.
        let mut c = scale(0.0, 1.0, 0.25);
        c.precision = 2;
        c.unit = "s".to_string();
        let value = c.value_of(3).unwrap();
        assert_eq!(format_readout(value, c.precision, &c.unit), "0.75 s");
    }

    // ---- a ruler that fills the room -------------------------------------

    #[test]
    fn a_count_read_out_of_the_room_is_whole_odd_and_fits_it() {
        assert_eq!(rows_for_extent(410.0, 10.0), Some(41));
        assert_eq!(rows_for_extent(405.0, 10.0), Some(41), "the part row is dropped");
        assert_eq!(rows_for_extent(396.0, 10.0), Some(39));
        // Odd for the band's sake, rounded down for the ruler's: a part
        // graduation at the end is one the band could never centre on.
        for extent in [10.0, 100.0, 101.0, 199.9, 200.0, 4096.0] {
            let rows = rows_for_extent(extent, 10.0).unwrap();
            assert_eq!(rows % 2, 1, "{extent} left no middle row for the band");
            assert!(
                (rows - 1) as f64 * 10.0 <= extent,
                "{extent} asked for more rows than it has room for"
            );
        }
        // The pitch is what it is counted in, so a finer ruler in the same
        // room holds proportionally more.
        assert_eq!(rows_for_extent(400.0, 5.0), Some(81));
    }

    #[test]
    fn a_room_that_says_nothing_hands_back_no_count() {
        // A walk that asked for nothing, a turtle not yet measured, a ruler
        // with less room than one graduation: the declared count is the
        // only answer any of these has.
        assert_eq!(rows_for_extent(0.0, 10.0), None);
        assert_eq!(rows_for_extent(-40.0, 10.0), None);
        assert_eq!(rows_for_extent(9.0, 10.0), None, "less room than a single row");
        assert_eq!(rows_for_extent(f64::INFINITY, 10.0), None);
        assert_eq!(rows_for_extent(f64::NAN, 10.0), None);
        // And a room no finite drum could fill is capped rather than turned
        // into a cast that saturates and an odd_rows that overflows past it.
        assert_eq!(rows_for_extent(1e12, 1.0), Some(FILL_MAX_ROWS));
        assert_eq!(rows_for_extent(f64::MAX, 1.0), Some(FILL_MAX_ROWS));
    }

    #[test]
    fn a_picker_that_did_not_ask_for_the_room_never_reads_it() {
        // The contract the rest of the widget rests on, unchanged: the
        // count is a property of the control, not of what it was laid out
        // in. Whatever the extent, the answer is the declared one.
        for extent in [0.0, 27.9, 140.0, 4000.0, f64::INFINITY, f64::NAN] {
            assert_eq!(rows_on_screen(false, 5, extent, 28.0), 5, "extent {extent}");
            assert_eq!(rows_on_screen(false, 41, extent, 10.0), 41, "extent {extent}");
        }
        // Including the even count that gains one, exactly as before.
        assert_eq!(rows_on_screen(false, 4, 4000.0, 28.0), odd_rows(4));
        assert_eq!(rows_on_screen(false, 0, 4000.0, 28.0), 1);
    }

    #[test]
    fn a_ruler_that_asked_for_the_room_takes_its_count_from_it() {
        assert_eq!(rows_on_screen(true, 41, 800.0, 10.0), 81, "eighty rows fit, so eighty-one");
        assert_eq!(rows_on_screen(true, 41, 140.0, 10.0), 15, "and a narrow panel gets fewer");
        // A walk with no room to read out of — `Fit`, or a turtle not yet
        // measured — falls back to the declared count rather than to a
        // ruler of one graduation.
        assert_eq!(rows_on_screen(true, 41, 0.0, 10.0), 41);
        assert_eq!(rows_on_screen(true, 41, f64::INFINITY, 10.0), 41);
    }

    #[test]
    fn the_fade_still_reaches_both_ends_of_a_long_ruler() {
        // The failure this exists to rule out: the fade's half-width is a
        // number of ROWS, so a ruler whose extent grew while its fade kept
        // the declared count would run out of gradient partway along and
        // leave the rest of itself flat.
        let (row, declared, take) = (10.0, 41usize, 0.45);
        let extent = 1210.0; // three times the declared forty-one rows
        let rows = rows_on_screen(true, declared, extent, row);
        assert_eq!(rows, 121);
        let half = rows as f64 * 0.5;
        // The outermost row on screen is half the ruler away from the band.
        let edge = extent / row * 0.5;
        assert!((half - edge).abs() <= 1.0, "the fade reaches as far as the ruler does");
        // At the very end the outermost row has given up exactly its share,
        // and no more — more would be a fade that had gone out early.
        assert!((fade_at(edge, half, take) - (1.0 - take)).abs() < 1e-9);
        // And it is still fading on the way there, at every station.
        let mut last = 1.0;
        for tenth in 1..=10 {
            let ink = fade_at(edge * tenth as f64 / 10.0, half, take);
            assert!(ink < last, "the gradient had already stopped {tenth} tenths out");
            last = ink;
        }
        // What `visible_items` alone would have given: a half-width spent a
        // third of the way out, with everything past it flat.
        let stale = odd_rows(declared) as f64 * 0.5;
        assert!(stale / edge < 0.4, "the stale fade is over a third of the way along");
        assert_eq!(
            fade_at(edge * 0.5, stale, take),
            fade_at(edge, stale, take),
            "the outer half of the ruler would all be one flat tone"
        );
        assert!(
            fade_at(edge * 0.5, half, take) > fade_at(edge, half, take),
            "where the derived count still has gradient left there"
        );
    }

    #[test]
    fn a_filled_ruler_is_graduated_all_the_way_to_its_ends() {
        // The count decides `reach` as well as the fade, so the drum is
        // asked for every row the extent has room for rather than for the
        // declared forty-one with bare well either side of them.
        let (row, declared, extent) = (10.0, 41usize, 1210.0);
        let rows = rows_on_screen(true, declared, extent, row);
        let d = Drum::new(4000, row, false);
        let drawn = d.rows_in_view(d.offset_of(2000), rows as f64 * 0.5 + 0.5);
        assert_eq!(drawn.len(), rows, "the whole extent is graduated");
        let stale = odd_rows(declared);
        let short = d.rows_in_view(d.offset_of(2000), stale as f64 * 0.5 + 0.5);
        assert_eq!(short.len(), stale, "where the declared count leaves most of it bare");
        assert!(drawn.len() > short.len() * 2);
    }
}

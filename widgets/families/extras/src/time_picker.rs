//! A time of day: one you type, and one you pick from columns.
//!
//! The core is [`CivilTime`] — an hour and a minute, and nothing else. No
//! date, no zone, no clock reading. "What time does it start" is a question
//! a form asks on its own, and answering it does not need to know what a day
//! is; keeping the two apart is what lets a caller hold a time in a struct
//! field without dragging a calendar in behind it.
//!
//! # Two widgets over one core
//!
//! * [`TimeField`] is text. You type, and it puts back what it understood:
//!   `9`, `930`, `9.30`, `9:30 pm` all land somewhere sensible, and what it
//!   cannot read it refuses, restoring the last time it could. Up and down
//!   move the part the caret is in, so the hour, the minute and the am/pm
//!   mark are each reachable without retyping the other two.
//! * [`TimePicker`] is two columns, or three on a 12-hour clock. They list
//!   only the times the grid actually offers, which is the whole reason
//!   [`TimeGrid`] exists as a type: the offers are counted from `min`, so a
//!   step that does not divide the hour carries on across the hour instead
//!   of restarting at every one. At seven minutes the offers run 0, 7, ...
//!   56, then 1:03 — and the minute column for the second hour says so.
//!
//! # What "12-hour" means here
//!
//! `hour12` changes what is written and what is offered, never what is
//! held. A [`CivilTime`] is always 0..23; the 12-hour reading is a
//! presentation, produced by [`CivilTime::hour12`] and consumed by
//! [`CivilTime::from_hour12`]. Both ends of that conversion are where the
//! bugs live — midnight and noon are BOTH twelve o'clock, one am and one pm
//! — so the conversion is one pair of functions with tests on it rather
//! than arithmetic spread over the drawing code.
//!
//! # There is no dial
//!
//! The round clock face — an hour hand you swing to a number — is
//! deliberately not here. Its marks are small triangles and tapered hands,
//! which means Sdf2d paths: `move_to`, `line_to`, `close_path`, `fill`.
//! Path fills do not paint reliably in this repository — two mirrored,
//! otherwise identical paths were drawn and only the second ever appeared —
//! so a dial built today would be a control that is sometimes invisible and
//! always differently invisible. Everything here is drawn as a box, a
//! rounded box, or a text glyph, all of which paint. When paths are fixed a
//! dial can be added beside these two; until then a dial would be a bug
//! with a face on it.
use crate::{
    family_api::measure,
    field_well::FieldWell,
    makepad_derive_widget::*,
    makepad_draw::{
        text::selection::{Cursor, Selection},
        *,
    },
    text_input::{TextInputAction, TextInputWidgetRefExt},
    widget::*,
    widget_tree::CxWidgetExt,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.TimeColumnBase = #(TimeColumn::register_widget(vm))
    mod.widgets.TimeFieldBase = #(TimeField::register_widget(vm))
    mod.widgets.TimePickerBase = #(TimePicker::register_widget(vm))

    /** The ground a column of times stands on. Mostly invisible: the picker
     * draws the box around all of them, and a second box drawn inside that
     * one is the thing every hand-rolled field wrapper gets wrong. It is
     * here to be aimed at and to show the keyboard. */
    set_type_default() do #(DrawTimeColumn::script_shader(vm)){
        ..mod.draw.DrawQuad

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(
                self.border_size
                self.border_size
                self.rect_size.x - self.border_size * 2.
                self.rect_size.y - self.border_size * 2.
                self.border_radius
            )
            sdf.fill_keep(self.color.mix(self.color_disabled, self.disabled))
            // Only the focus ring is stroked. An unfocused column has no
            // outline at all, so the three of them read as one face.
            sdf.stroke(self.border_color.mix(self.border_color_focus, self.focus), self.border_size)
            return sdf.result
        }
    }

    /** One row in a column: the plate under a value. */
    set_type_default() do #(DrawTimeRow::script_shader(vm)){
        ..mod.draw.DrawQuad

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0., 0., self.rect_size.x, self.rect_size.y, self.border_radius)
            // The disabled mix is scaled by `active` on purpose. The plate's
            // base colour is transparent, so mixing every row towards a grey
            // would give a disabled column a full set of plates it does not
            // have when it is live.
            sdf.fill(
                self.color
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_active, self.active)
                    .mix(self.color_disabled, self.disabled * self.active)
            )
            return sdf.result
        }
    }

    /** The card the columns sit on. */
    set_type_default() do #(DrawTimePicker::script_shader(vm)){
        ..mod.draw.DrawQuad

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(
                self.border_size
                self.border_size
                self.rect_size.x - self.border_size * 2.
                self.rect_size.y - self.border_size * 2.
                self.border_radius
            )
            sdf.fill_keep(self.color.mix(self.color_disabled, self.disabled))
            sdf.stroke(self.border_color.mix(self.border_color_disabled, self.disabled), self.border_size)
            return sdf.result
        }
    }

    /** A column of values you pick one of: hours, minutes, or am and pm.
     *
     * Every value here is plain rather than uniform(), because every one has
     * a field on the draw struct behind it and the two must agree. */
    mod.widgets.TimeColumn = set_type_default() do mod.widgets.TimeColumnBase{
        width: 54.
        // A stated height, not Fit: the column decides how many rows it can
        // show from the height it is given, and Fit would ask it back.
        height: 154.
        /** height of one row in pixels 14..40 step 1 */
        row_height: 22.

        draw_bg +: {
            focus: 0.0
            disabled: 0.0

            /** focus ring thickness in pixels 0..4 step 0.5 */
            border_size: theme.beveling
            /** corner rounding radius 0..24 step 0.5 */
            border_radius: theme.corner_radius

            color: #00000000
            color_disabled: #00000000
            border_color: #00000000
            border_color_focus: theme.color_bevel_focus
        }

        draw_row +: {
            hover: 0.0
            active: 0.0
            disabled: 0.0

            /** corner rounding radius 0..24 step 0.5 */
            border_radius: theme.corner_radius

            color: #00000000
            color_hover: theme.color_inset_hover
            color_active: theme.color_val
            color_disabled: theme.color_val_disabled
        }

        /** The values that were not chosen: present, readable, quieter. */
        draw_text +: {
            color: theme.color_label_inner_inactive
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }

        /** The value that was chosen. */
        draw_text_active +: {
            color: theme.color_label_inner
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
    }

    /** A time you type: it reads what you meant and writes back what it read. */
    mod.widgets.TimeField = set_type_default() do mod.widgets.TimeFieldBase{
        width: 120.
        // A real height, not Fit. The well and the input inside it are both
        // Fill so the text centres in the box, and Fill inside Fit resolves
        // to nothing: the field would be laid out and never painted.
        height: 24.

        /** write and read a 12-hour clock with an am/pm mark 0..1 step 1 */
        hour12: false
        /** minutes the arrow keys move the minute part 1..60 step 1 */
        step: 1.0
        /** earliest time accepted, written as a time; empty means midnight */
        min: ""
        /** latest time accepted, written as a time; empty means 23:59 */
        max: ""
        /** the time it starts on */
        value: "12:00"

        well: mod.widgets.FieldWell{
            width: Fill
            height: Fill
            align: Align{y: 0.5}
            padding: Inset{left: theme.space_2, right: theme.space_2, top: 0., bottom: 0.}
            input: mod.widgets.WellInput{
                width: Fill
                height: Fill
                empty_text: ""
            }
        }
    }

    /** A time you pick: an hour column, a minute column, and am/pm when the
     * clock has twelve hours on it. */
    mod.widgets.TimePicker = set_type_default() do mod.widgets.TimePickerBase{
        width: Fit
        height: Fit
        flow: Right
        spacing: theme.space_1
        padding: theme.mspace_1

        /** minutes between one offered time and the next 1..60 step 1 */
        step: 5.0
        /** offer twelve hours and an am/pm column 0..1 step 1 */
        hour12: false
        /** earliest time offered, written as a time; empty means midnight */
        min: ""
        /** latest time offered, written as a time; empty means 23:59 */
        max: ""
        /** the time it starts on */
        value: "12:00"

        draw_bg +: {
            disabled: 0.0

            /** bevel thickness in pixels 0..4 step 0.5 */
            border_size: theme.beveling
            /** corner rounding radius 0..24 step 0.5 */
            border_radius: theme.corner_radius

            color: theme.color_inset
            color_disabled: theme.color_inset_disabled
            border_color: theme.color_bevel_inset_1
            border_color_disabled: theme.color_bevel_inset_1_disabled
        }

        // Slots, so they take a value: `hours: TimeColumn{}`. Written with
        // `:=` they would become named children and leave the slots empty,
        // and the card would draw as an empty box.
        hours: mod.widgets.TimeColumn{}
        minutes: mod.widgets.TimeColumn{}
        meridiem: mod.widgets.TimeColumn{width: 46.}
    }
}

// ---------------------------------------------------------------------------
// The core: a time of day, and the set of times a picker offers.
// ---------------------------------------------------------------------------

/// Minutes in a day. Every piece of arithmetic below is on this one number.
const DAY: i64 = 24 * 60;

const AM: &str = "AM";
const PM: &str = "PM";

/// A time of day on a wall clock: an hour and a minute, and nothing else.
///
/// It has no date and no zone, so it cannot say when it is; it says what a
/// clock would read. That is the whole contract, and it is why the type can
/// be `Copy`, ordered, and tested without a calendar.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct CivilTime {
    pub hour: u32,
    pub minute: u32,
}

/// Which clock a time is written on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Clock {
    /// `13:05`, and midnight is `00:00`.
    H24,
    /// `1:05 PM`, and midnight is `12:00 AM`.
    H12,
}

impl Clock {
    pub fn of(hour12: bool) -> Self {
        if hour12 {
            Clock::H12
        } else {
            Clock::H24
        }
    }
}

impl CivilTime {
    /// A time, or nothing if that is not one.
    pub fn new(hour: u32, minute: u32) -> Option<Self> {
        if hour > 23 || minute > 59 {
            None
        } else {
            Some(Self { hour, minute })
        }
    }

    /// Minutes since midnight.
    pub fn to_minutes(self) -> i64 {
        self.hour as i64 * 60 + self.minute as i64
    }

    /// The time that many minutes after midnight, coming round the day. A
    /// time has no date to carry the overflow into, so the day is a cycle:
    /// there is nowhere else for 24:30 to go.
    pub fn from_minutes(minutes: i64) -> Self {
        let m = minutes.rem_euclid(DAY);
        Self { hour: (m / 60) as u32, minute: (m % 60) as u32 }
    }

    pub fn add_minutes(self, n: i64) -> Self {
        Self::from_minutes(self.to_minutes() + n)
    }

    /// The hour a 12-hour clock shows, and whether it is the afternoon.
    ///
    /// Midnight and noon are both twelve. That is the trap this function
    /// exists to hold in one place: `hour % 12` gives zero for both of them,
    /// and no clock face has a zero on it.
    pub fn hour12(self) -> (u32, bool) {
        let pm = self.hour >= 12;
        let h = self.hour % 12;
        (if h == 0 { 12 } else { h }, pm)
    }

    /// The other direction: twelve am is midnight, twelve pm is noon.
    pub fn from_hour12(hour: u32, pm: bool, minute: u32) -> Option<Self> {
        if hour == 0 || hour > 12 {
            return None;
        }
        let h = if hour == 12 { 0 } else { hour };
        Self::new(if pm { h + 12 } else { h }, minute)
    }

    /// How the field writes it.
    pub fn format(self, clock: Clock) -> String {
        match clock {
            Clock::H24 => format!("{:02}:{:02}", self.hour, self.minute),
            Clock::H12 => {
                let (hour, pm) = self.hour12();
                format!("{}:{:02} {}", hour, self.minute, if pm { PM } else { AM })
            }
        }
    }

    /// What a person typed, if it was a time.
    ///
    /// Accepted: an hour alone (`9`, `09`), an hour and a minute separated
    /// by `:` or `.` (`9:30`, `9.30`), the bare digits (`930`, `0930`), any
    /// of those followed by an am/pm mark (`9:30 pm`, `9pm`, `9 p.m.`).
    /// Refused: anything else, including an out-of-range hour or minute and
    /// an hour outside 1..12 carrying an am/pm mark. Refusing is the point —
    /// a field that silently keeps nonsense is worse than one that will not
    /// take it.
    pub fn parse(text: &str) -> Option<Self> {
        let lowered = text.trim().to_ascii_lowercase();
        let mut body = lowered.as_str();
        let mut meridiem = None;
        // Longest mark first: "a.m." would otherwise be eaten by "a" and
        // leave a stray full stop behind to fail the digit check.
        for (mark, pm) in [("a.m.", false), ("p.m.", true), ("am", false), ("pm", true), ("a", false), ("p", true)] {
            if let Some(head) = body.strip_suffix(mark) {
                meridiem = Some(pm);
                body = head.trim_end();
                break;
            }
        }
        if body.is_empty() {
            return None;
        }
        let (hour_text, minute_text) = match body.find(|c: char| c == ':' || c == '.') {
            Some(at) => (&body[..at], &body[at + 1..]),
            None => split_bare(body)?,
        };
        let hour = two_digits(hour_text)?;
        let minute = if minute_text.trim().is_empty() { 0 } else { two_digits(minute_text)? };
        if minute > 59 {
            return None;
        }
        match meridiem {
            Some(pm) => Self::from_hour12(hour, pm, minute),
            None => Self::new(hour, minute),
        }
    }
}

/// A run of one or two digits, and nothing else. `parse::<u32>` would take
/// `+9` and a stray sign is not a time.
fn two_digits(text: &str) -> Option<u32> {
    let text = text.trim();
    if text.is_empty() || text.len() > 2 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Digits with no separator in them: `9`, `09`, `930`, `0930`.
fn split_bare(text: &str) -> Option<(&str, &str)> {
    if !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match text.len() {
        1 | 2 => Some((text, "")),
        3 => Some((&text[..1], &text[1..])),
        4 => Some((&text[..2], &text[2..])),
        _ => None,
    }
}

/// The times a picker offers: everything from `min` to `max`, `step` minutes
/// apart.
///
/// It is not a filter over the clock. The offers are COUNTED from `min`, so
/// a step that does not divide the hour keeps its spacing across the hour
/// boundary rather than restarting: at seven minutes from midnight the
/// offers are 0:00, 0:07 ... 0:56, 1:03. That is why the minute column has
/// to be asked which minutes this hour has, instead of being handed the same
/// list every hour.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TimeGrid {
    pub min: CivilTime,
    pub max: CivilTime,
    pub step: u32,
}

impl TimeGrid {
    pub fn new(min: CivilTime, max: CivilTime, step: u32) -> Self {
        // A backwards range is one instant, not an error: the widget's two
        // bounds are separate live properties and the tweaker can drag one
        // past the other at any moment.
        let max = if max < min { min } else { max };
        Self { min, max, step: step.max(1) }
    }

    /// The index of the last offer. Never past `max`: a grid that offered a
    /// time outside its own bounds would be a bound nobody could trust.
    fn last(&self) -> i64 {
        ((self.max.to_minutes() - self.min.to_minutes()) / self.step as i64).max(0)
    }

    fn at(&self, index: i64) -> CivilTime {
        let index = index.clamp(0, self.last());
        CivilTime::from_minutes(self.min.to_minutes() + index * self.step as i64)
    }

    fn offers_iter(&self) -> impl Iterator<Item = CivilTime> + '_ {
        (0..=self.last()).map(move |index| self.at(index))
    }

    /// The nearest time this grid offers.
    pub fn snap(&self, time: CivilTime) -> CivilTime {
        let step = self.step as i64;
        let offset = time.to_minutes() - self.min.to_minutes();
        // Round to nearest, not down: a time halfway between two offers
        // should not always fall backwards.
        self.at((offset * 2 + step) / (step * 2))
    }

    pub fn offers(&self, time: CivilTime) -> bool {
        self.snap(time) == time
    }

    /// The hours that have at least one offer in them, in order.
    pub fn hours(&self) -> Vec<u32> {
        let mut out: Vec<u32> = Vec::new();
        for time in self.offers_iter() {
            if out.last() != Some(&time.hour) {
                out.push(time.hour);
            }
        }
        out
    }

    /// The minutes this grid offers inside one hour.
    pub fn minutes_in(&self, hour: u32) -> Vec<u32> {
        self.offers_iter().filter(|t| t.hour == hour).map(|t| t.minute).collect()
    }

    /// Move to another hour, keeping the minute when that hour has it and
    /// taking the nearest minute it does have otherwise. An hour with no
    /// offers in it is not moved to at all.
    pub fn with_hour(&self, time: CivilTime, hour: u32) -> CivilTime {
        let minutes = self.minutes_in(hour);
        let Some(nearest) = minutes
            .iter()
            .copied()
            .min_by_key(|m| (*m as i64 - time.minute as i64).abs())
        else {
            return time;
        };
        CivilTime { hour, minute: nearest }
    }

    pub fn with_minute(&self, time: CivilTime, minute: u32) -> CivilTime {
        let wanted = CivilTime { hour: time.hour, minute };
        if self.offers(wanted) {
            wanted
        } else {
            self.snap(wanted)
        }
    }
}

/// Which piece of a written time something is pointing at.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TimePart {
    Hour,
    Minute,
    Meridiem,
}

/// Where the hour, the minute and the am/pm mark sit in a written time, as
/// byte offsets. The mark is `None` when the text has none.
fn parts_of(text: &str) -> ((usize, usize), (usize, usize), Option<(usize, usize)>) {
    let bytes = text.as_bytes();
    let separator = bytes.iter().position(|c| *c == b':' || *c == b'.');
    let hour_end = separator
        .unwrap_or_else(|| bytes.iter().position(|c| !c.is_ascii_digit()).unwrap_or(bytes.len()));
    let minute_start = separator.map(|at| at + 1).unwrap_or(hour_end);
    let mut minute_end = minute_start.min(bytes.len());
    while minute_end < bytes.len() && bytes[minute_end].is_ascii_digit() {
        minute_end += 1;
    }
    let mark = (minute_end..bytes.len())
        .find(|at| bytes[*at].is_ascii_alphabetic())
        .map(|start| {
            let mut end = start;
            while end < bytes.len() && (bytes[end].is_ascii_alphabetic() || bytes[end] == b'.') {
                end += 1;
            }
            (start, end)
        });
    ((0, hour_end), (minute_start, minute_end), mark)
}

/// The part the caret is in.
///
/// A caret sitting exactly at the end of a part belongs to that part, which
/// is the convention typing produces: type the hour and the caret is after
/// it, and up should still move the hour.
fn part_at(text: &str, caret: usize) -> TimePart {
    let (hour, _, mark) = parts_of(text);
    if let Some(mark) = mark {
        if caret >= mark.0 {
            return TimePart::Meridiem;
        }
    }
    // Everything past the hour is the minute, including a caret that has run
    // off the end of the digits: the minute is the part that was being edited
    // when it got there.
    if caret <= hour.1 {
        TimePart::Hour
    } else {
        TimePart::Minute
    }
}

/// The span of one part, for putting the selection back over what the arrow
/// keys just moved.
fn part_span(text: &str, part: TimePart) -> (usize, usize) {
    let (hour, minute, mark) = parts_of(text);
    match part {
        TimePart::Hour => hour,
        TimePart::Minute => minute,
        TimePart::Meridiem => mark.unwrap_or(minute),
    }
}

/// How far one press of an arrow key moves a part.
fn bump(time: CivilTime, part: TimePart, direction: i64, step: u32) -> CivilTime {
    match part {
        TimePart::Hour => time.add_minutes(60 * direction),
        TimePart::Minute => time.add_minutes(step.max(1) as i64 * direction),
        // Twelve hours either way is the same instant, so the mark toggles
        // whichever arrow was pressed. Down on "PM" giving "AM" is what the
        // key looks like it should do.
        TimePart::Meridiem => time.add_minutes(12 * 60),
    }
}

/// The bounds a pair of live properties describes. Empty means the end of
/// the day it stands at, so a caller may bound one side and leave the other.
fn bounds_of(min: &str, max: &str) -> (CivilTime, CivilTime) {
    let low = CivilTime::parse(min).unwrap_or(CivilTime { hour: 0, minute: 0 });
    let high = CivilTime::parse(max).unwrap_or(CivilTime { hour: 23, minute: 59 });
    if high < low {
        (low, low)
    } else {
        (low, high)
    }
}

// ---------------------------------------------------------------------------
// A column of values.
// ---------------------------------------------------------------------------

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTimeColumn {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    focus: f32,
    #[live]
    disabled: f32,
    #[live]
    border_size: f32,
    #[live]
    border_radius: f32,
    #[live]
    color: Vec4f,
    #[live]
    color_disabled: Vec4f,
    #[live]
    border_color: Vec4f,
    #[live]
    border_color_focus: Vec4f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTimeRow {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    active: f32,
    #[live]
    disabled: f32,
    #[live]
    border_radius: f32,
    #[live]
    color: Vec4f,
    #[live]
    color_hover: Vec4f,
    #[live]
    color_active: Vec4f,
    #[live]
    color_disabled: Vec4f,
}

/// Which rows of a list of values a box of a given height can show, and
/// where each one goes.
///
/// It is a separate value for the usual two reasons: it can be tested
/// without a script heap, and the hit test and the drawing are handed the
/// same numbers from the same place, so what is drawn is what is clickable.
#[derive(Copy, Clone, Debug, PartialEq)]
struct ColumnView {
    row: f64,
    height: f64,
    count: usize,
    selected: usize,
}

impl ColumnView {
    /// Whole rows only. A row half off the bottom edge would have to be
    /// clipped, and a clipped digit is a digit read wrong.
    fn visible(&self) -> usize {
        ((self.height / self.row).floor() as usize).max(1)
    }

    fn shown(&self) -> usize {
        self.count.min(self.visible())
    }

    /// The first value drawn: the selection is kept in the middle where
    /// there is a middle to keep it in.
    fn first(&self) -> usize {
        let visible = self.visible();
        if self.count <= visible {
            return 0;
        }
        let half = (visible - 1) / 2;
        self.selected.saturating_sub(half).min(self.count - visible)
    }

    /// The leftover height, split above and below, so a short list sits in
    /// the middle of its box rather than hanging from the top.
    fn pad(&self) -> f64 {
        ((self.height - self.shown() as f64 * self.row) * 0.5).max(0.0)
    }

    /// The top of a value's row, measured from the top of the box, or
    /// nothing when that value is not on screen.
    fn top_of(&self, index: usize) -> Option<f64> {
        let first = self.first();
        if index < first || index >= first + self.shown() {
            return None;
        }
        Some(self.pad() + (index - first) as f64 * self.row)
    }

    /// The value at an offset down from the top of the box.
    fn index_at(&self, y: f64) -> Option<usize> {
        let offset = y - self.pad();
        if offset < 0.0 {
            return None;
        }
        let row = (offset / self.row).floor() as usize;
        if row >= self.shown() {
            return None;
        }
        Some(self.first() + row)
    }
}

#[derive(Clone, Debug, Default)]
pub enum TimeColumnAction {
    /// A value was chosen, by index into the values the column was given.
    Picked(usize),
    #[default]
    None,
}

/// A column of values, one of which is chosen.
///
/// It holds no times. The picker hands it labels and an index and reads back
/// an index, which is what lets the same widget be the hours, the minutes
/// and the am/pm mark without three copies of the drawing and the hit test.
#[derive(Script, ScriptHook, Widget)]
pub struct TimeColumn {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_bg: DrawTimeColumn,
    #[live]
    draw_row: DrawTimeRow,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_text_active: DrawText,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[live(22.0)]
    pub row_height: f64,
    #[live]
    disabled: bool,

    #[rust]
    items: Vec<String>,
    #[rust]
    selected: usize,
    /// The row the pointer is over, so a press can be anticipated rather
    /// than discovered.
    #[rust]
    hot: Option<usize>,
    /// How many rows the last draw could fit, so Page Up moves by a screen
    /// of the column as it actually is rather than a guess.
    #[rust]
    rows_shown: usize,
}

impl TimeColumn {
    fn view(&self, height: f64) -> ColumnView {
        ColumnView {
            row: self.row_height.max(1.0),
            height,
            count: self.items.len(),
            selected: self.selected,
        }
    }

    /// The labels this column offers and which of them is chosen. Called by
    /// the picker every draw, because a change of hour changes the minutes.
    pub fn set_items(&mut self, items: Vec<String>, selected: usize) {
        self.items = items;
        self.selected = selected.min(self.items.len().saturating_sub(1));
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    fn pick(&mut self, cx: &mut Cx, index: usize) {
        if index >= self.items.len() || index == self.selected {
            return;
        }
        self.selected = index;
        cx.widget_action(self.uid, TimeColumnAction::Picked(index));
        self.draw_bg.redraw(cx);
    }
}

impl Widget for TimeColumn {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if self.disabled != disabled {
            self.disabled = disabled;
            self.draw_bg.redraw(cx);
        }
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // Asked every pass rather than tracked: the focus can leave for
        // reasons this column never hears about, and one that only listened
        // would keep claiming a keyboard it no longer has.
        let focused = cx.cx.cx.has_key_focus(self.draw_bg.area());
        self.draw_bg.focus = if focused { 1.0 } else { 0.0 };
        self.draw_bg.disabled = if self.disabled { 1.0 } else { 0.0 };
        self.draw_row.disabled = if self.disabled { 1.0 } else { 0.0 };

        self.draw_bg.begin(cx, walk, self.layout);
        let rect = cx.turtle().rect();
        let view = self.view(rect.size.y);
        let first = view.first();
        let shown = view.shown();
        self.rows_shown = shown.max(1);

        let size = self.draw_text.text_style.font_size as f64;
        for index in first..first + shown {
            let Some(top) = view.top_of(index) else {
                continue;
            };
            let row = Rect {
                pos: dvec2(rect.pos.x, rect.pos.y + top),
                size: dvec2(rect.size.x, view.row),
            };
            let active = index == self.selected;
            self.draw_row.active = if active { 1.0 } else { 0.0 };
            self.draw_row.hover = if self.hot == Some(index) && !active && !self.disabled {
                1.0
            } else {
                0.0
            };
            self.draw_row.draw_abs(cx, row);

            let label = self.items[index].clone();
            // A disabled column draws every value in the quiet face: the
            // bright one is the shader's active plate, and a dead control
            // should not look like it has a live selection on it.
            let text = if active && !self.disabled {
                &mut self.draw_text_active
            } else {
                &mut self.draw_text
            };
            let width = measure(text, cx, &label);
            // draw_abs takes the top of the LINE box, not of the ink; a
            // digit's ink starts about 0.30 of the font size below it.
            let pos = dvec2(
                row.pos.x + (row.size.x - width) * 0.5,
                row.pos.y + (row.size.y - size) * 0.5 - size * 0.30,
            );
            text.draw_abs(cx, pos, &label);
        }
        self.draw_bg.end(cx);

        if !self.disabled {
            cx.add_nav_stop(self.draw_bg.area(), NavRole::DropDown, Inset::default());
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.disabled {
            return;
        }
        let area = self.draw_bg.area();
        let height = area.rect(cx).size.y;
        let view = self.view(height);

        match event.hits(cx, area) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
            }
            Hit::FingerHoverOver(fe) => {
                let hot = view.index_at(fe.abs.y - fe.rect.pos.y);
                if self.hot != hot {
                    self.hot = hot;
                    self.draw_bg.redraw(cx);
                }
                cx.set_cursor(MouseCursor::Hand);
            }
            Hit::FingerHoverOut(_) => {
                if self.hot.is_some() {
                    self.hot = None;
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(area);
                if let Some(index) = view.index_at(fe.abs.y - fe.rect.pos.y) {
                    self.pick(cx, index);
                }
            }
            Hit::FingerScroll(fe) => {
                // Down the wheel is down the column, which is the direction
                // the values move under it.
                let last = self.items.len() as i64 - 1;
                let notches = fe.scroll.y.signum() as i64;
                if last >= 0 && notches != 0 {
                    let next = (self.selected as i64 + notches).clamp(0, last);
                    self.pick(cx, next as usize);
                }
            }
            Hit::KeyFocus(_) | Hit::KeyFocusLost(_) => {
                self.draw_bg.redraw(cx);
            }
            Hit::KeyDown(ke) => {
                let last = self.items.len().saturating_sub(1) as i64;
                let page = self.rows_shown.max(1) as i64;
                let next = match ke.key_code {
                    KeyCode::ArrowUp => self.selected as i64 - 1,
                    KeyCode::ArrowDown => self.selected as i64 + 1,
                    KeyCode::PageUp => self.selected as i64 - page,
                    KeyCode::PageDown => self.selected as i64 + page,
                    KeyCode::Home => 0,
                    KeyCode::End => last,
                    _ => return,
                };
                self.pick(cx, next.clamp(0, last) as usize);
            }
            _ => {}
        }
    }

    /// The chosen value, so a host reads the column the way it reads a label.
    fn text(&self) -> String {
        self.items.get(self.selected).cloned().unwrap_or_default()
    }
}

impl TimeColumnRef {
    pub fn selected(&self) -> usize {
        self.borrow().map(|inner| inner.selected()).unwrap_or(0)
    }

    /// The value chosen, when this column was picked in `actions`.
    pub fn picked(&self, actions: &Actions) -> Option<usize> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            TimeColumnAction::Picked(index) => Some(index),
            TimeColumnAction::None => None,
        }
    }
}

// ---------------------------------------------------------------------------
// A time you type.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub enum TimeFieldAction {
    /// The time changed, however it was changed.
    Changed(CivilTime),
    #[default]
    None,
}

/// A text field that reads a time and writes it back in one shape.
///
/// It is a [`FieldWell`] with a text input in it, so selection, the
/// clipboard and undo are all the real ones. What this widget owns is the
/// reading, the writing, and the arrow keys.
#[derive(Script, Widget)]
pub struct TimeField {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[find]
    #[redraw]
    #[live]
    pub well: WidgetRef,
    #[walk]
    walk: Walk,

    /// Write and read a 12-hour clock. It changes the writing, never what is
    /// held: the time is 0..23 either way.
    #[live]
    pub hour12: bool,
    /// Minutes one press of an arrow key moves the minute part by. It does
    /// NOT quantize what is typed: somebody who typed 9:07 meant 9:07, and a
    /// field that rounded it away would be arguing with them.
    #[live(1.0)]
    pub step: f64,
    #[live]
    pub min: String,
    #[live]
    pub max: String,
    /// The time it starts on, as written. Read once, and again whenever it
    /// is set to something else; it is not a mirror of the current time.
    #[live]
    pub value: String,
    #[live]
    pub disabled: bool,

    #[rust]
    time: CivilTime,
    /// The last `value` adopted, so re-applying an unrelated property does
    /// not throw away what the person has since typed.
    #[rust]
    taken: String,
    /// The text last written into the input, so a redraw does not fight the
    /// caret while somebody is typing.
    #[rust]
    shown: String,
}

impl ScriptHook for TimeField {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.settle(vm);
    }

    fn on_after_reload(&mut self, vm: &mut ScriptVm) {
        self.settle(vm);
    }
}

impl TimeField {
    fn settle(&mut self, vm: &mut ScriptVm) {
        vm.with_cx_mut(|cx| {
            self.adopt(cx);
            if self.disabled {
                self.well.set_disabled(cx, true);
            }
        });
    }

    fn adopt(&mut self, cx: &mut Cx) {
        if self.taken != self.value {
            self.taken = self.value.clone();
            if let Some(time) = CivilTime::parse(&self.value) {
                self.time = time;
            }
        }
        self.time = self.bound(self.time);
        self.write_text(cx);
    }

    fn clock(&self) -> Clock {
        Clock::of(self.hour12)
    }

    fn bound(&self, time: CivilTime) -> CivilTime {
        let (low, high) = bounds_of(&self.min, &self.max);
        time.clamp(low, high)
    }

    fn input(&self) -> WidgetRef {
        self.well
            .borrow::<FieldWell>()
            .map(|well| well.input.clone())
            .unwrap_or_else(WidgetRef::empty)
    }

    fn focused(&self, cx: &Cx) -> bool {
        self.well.borrow::<FieldWell>().map(|well| well.focused(cx)).unwrap_or(false)
    }

    fn write_text(&mut self, cx: &mut Cx) {
        let text = self.time.format(self.clock());
        if self.shown == text {
            return;
        }
        self.shown = text.clone();
        self.input().as_text_input().set_text(cx, &text);
    }

    fn commit(&mut self, cx: &mut Cx, time: CivilTime) {
        let time = self.bound(time);
        self.time = time;
        self.write_text(cx);
        cx.widget_action(self.uid, TimeFieldAction::Changed(time));
    }

    /// Read whatever is in the box. What will not parse is refused and the
    /// last time that did is put back: keeping nonsense silently is worse
    /// than refusing it, and clearing the box would lose the value the
    /// person was editing away from.
    fn take_typed(&mut self, cx: &mut Cx, typed: &str) {
        match CivilTime::parse(typed) {
            Some(time) => {
                // Forced, because the text on screen is the typed shape and
                // the written shape may be the same string: 9:30 typed into
                // a 24-hour field must still redraw as 09:30.
                self.shown.clear();
                self.commit(cx, time);
            }
            None => {
                self.shown.clear();
                self.write_text(cx);
            }
        }
    }

    /// Move the part the caret is in, and leave it selected so the next
    /// press moves the same part.
    fn bump_at_caret(&mut self, cx: &mut Cx, direction: i64) {
        let input = self.input().as_text_input();
        let typed = input.text();
        let caret = input.cursor().index.min(typed.len());
        let part = part_at(&typed, caret);
        // Whatever is in the box wins if it reads as a time, so a typed
        // hour that has not been committed yet is the one that moves.
        let from = CivilTime::parse(&typed).unwrap_or(self.time);
        let step = self.step.round().max(1.0) as u32;
        self.shown.clear();
        self.commit(cx, bump(from, part, direction, step));

        let text = self.shown.clone();
        let (start, end) = part_span(&text, part);
        input.set_selection(
            cx,
            Selection {
                anchor: Cursor { index: start.min(text.len()), prefer_next_row: false },
                cursor: Cursor { index: end.min(text.len()), prefer_next_row: false },
            },
        );
    }

    pub fn time(&self) -> CivilTime {
        self.time
    }

    pub fn set_time(&mut self, cx: &mut Cx, time: CivilTime) {
        self.time = self.bound(time);
        self.write_text(cx);
    }

    pub fn set_hour12(&mut self, cx: &mut Cx, hour12: bool) {
        if self.hour12 != hour12 {
            self.hour12 = hour12;
            self.write_text(cx);
        }
    }
}

impl Widget for TimeField {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.disabled = disabled;
        self.well.set_disabled(cx, disabled);
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // The well is drawn here rather than by a container, so nothing else
        // puts it in the tree; without this a host cannot reach the input
        // inside it at all.
        cx.widget_tree_insert_child(self.uid, live_id!(well), self.well.clone());
        self.well.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.disabled {
            return;
        }
        for action in cx.capture_actions(|cx| self.well.handle_event(cx, event, scope)) {
            match action.as_widget_action().cast() {
                TextInputAction::Returned(text, _) => {
                    self.take_typed(cx, &text);
                }
                // Leaving the field commits it too, so a time typed and then
                // clicked away from is not quietly discarded. The action
                // carries nothing, so the box is asked.
                TextInputAction::KeyFocusLost => {
                    let typed = self.input().as_text_input().text();
                    self.take_typed(cx, &typed);
                }
                TextInputAction::Escaped => {
                    self.shown.clear();
                    self.write_text(cx);
                }
                // A one-line input has nowhere to move the caret up or down
                // to, so it hands the key back. That is the hook the parts
                // hang on.
                TextInputAction::KeyDownUnhandled(ke) => match ke.key_code {
                    KeyCode::ArrowUp => self.bump_at_caret(cx, 1),
                    KeyCode::ArrowDown => self.bump_at_caret(cx, -1),
                    _ => {}
                },
                _ => {}
            }
        }

        // The wheel moves the part under the caret — and only while the
        // field holds the keyboard, because without a caret there is no part
        // to move and a field scrolled past in a long form must not change
        // itself on the way by.
        if let Hit::FingerScroll(fe) = event.hits(cx, self.well.area()) {
            if self.focused(cx) {
                let notches = -fe.scroll.y.signum() as i64;
                if notches != 0 {
                    self.bump_at_caret(cx, notches);
                }
            }
        }
    }

    fn text(&self) -> String {
        self.time.format(self.clock())
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if let Some(time) = CivilTime::parse(v) {
            self.set_time(cx, time);
        }
    }
}

impl TimeFieldRef {
    pub fn time(&self) -> CivilTime {
        self.borrow().map(|inner| inner.time()).unwrap_or_default()
    }

    pub fn set_time(&self, cx: &mut Cx, time: CivilTime) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_time(cx, time);
        }
    }

    pub fn set_hour12(&self, cx: &mut Cx, hour12: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_hour12(cx, hour12);
        }
    }

    /// The new time, when this field changed in `actions`.
    pub fn changed(&self, actions: &Actions) -> Option<CivilTime> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            TimeFieldAction::Changed(time) => Some(time),
            TimeFieldAction::None => None,
        }
    }
}

// ---------------------------------------------------------------------------
// A time you pick.
// ---------------------------------------------------------------------------

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTimePicker {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    disabled: f32,
    #[live]
    border_size: f32,
    #[live]
    border_radius: f32,
    #[live]
    color: Vec4f,
    #[live]
    color_disabled: Vec4f,
    #[live]
    border_color: Vec4f,
    #[live]
    border_color_disabled: Vec4f,
}

#[derive(Clone, Debug, Default)]
pub enum TimePickerAction {
    /// A column was picked and the time moved.
    Changed(CivilTime),
    #[default]
    None,
}

/// Columns of times to choose from: hours, minutes, and am/pm on a 12-hour
/// clock.
///
/// The columns are told what to list every draw, from the [`TimeGrid`] the
/// live properties describe. That is what keeps a step that does not divide
/// the hour honest — the minute column for one o'clock is not the minute
/// column for midnight, and it should not pretend to be.
#[derive(Script, Widget)]
pub struct TimePicker {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_bg: DrawTimePicker,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[find]
    #[live]
    pub hours: WidgetRef,
    #[find]
    #[live]
    pub minutes: WidgetRef,
    /// The am/pm column. Drawn only on a 12-hour clock, where it is the
    /// switch between the two halves of the day.
    #[find]
    #[live]
    pub meridiem: WidgetRef,

    /// Twelve hours and an am/pm column, rather than twenty-four hours.
    #[live]
    pub hour12: bool,
    /// Minutes between one offered time and the next.
    #[live(5.0)]
    pub step: f64,
    #[live]
    pub min: String,
    #[live]
    pub max: String,
    /// The time it starts on, as written.
    #[live]
    pub value: String,
    #[live]
    pub disabled: bool,

    #[rust]
    time: CivilTime,
    #[rust]
    taken: String,
}

impl ScriptHook for TimePicker {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.settle(vm);
    }

    fn on_after_reload(&mut self, vm: &mut ScriptVm) {
        self.settle(vm);
    }
}

impl TimePicker {
    fn settle(&mut self, vm: &mut ScriptVm) {
        self.adopt();
        // A `disabled: true` written in the DSL sets the field directly and
        // never reaches `set_disabled`, so without this the card would dim
        // while the columns inside it stayed live.
        if self.disabled {
            vm.with_cx_mut(|cx| {
                for slot in [&self.hours, &self.minutes, &self.meridiem] {
                    slot.set_disabled(cx, true);
                }
            });
        }
    }

    fn adopt(&mut self) {
        if self.taken != self.value {
            self.taken = self.value.clone();
            if let Some(time) = CivilTime::parse(&self.value) {
                self.time = time;
            }
        }
        self.time = self.grid().snap(self.time);
    }

    /// The grid as it stands, built fresh each time: every number in it is a
    /// live property and the tweaker may have moved any of them since the
    /// last draw.
    fn grid(&self) -> TimeGrid {
        let (low, high) = bounds_of(&self.min, &self.max);
        TimeGrid::new(low, high, self.step.round().max(1.0) as u32)
    }

    /// The hours this column offers, which on a 12-hour clock is only the
    /// half of the day the am/pm column is showing.
    fn column_hours(&self, grid: &TimeGrid) -> Vec<u32> {
        let all = grid.hours();
        if !self.hour12 {
            return all;
        }
        let pm = self.time.hour >= 12;
        all.into_iter().filter(|hour| (*hour >= 12) == pm).collect()
    }

    fn hour_label(&self, hour: u32) -> String {
        if self.hour12 {
            CivilTime { hour, minute: 0 }.hour12().0.to_string()
        } else {
            format!("{hour:02}")
        }
    }

    fn fill(slot: &WidgetRef, items: Vec<String>, selected: usize) {
        if let Some(mut column) = slot.borrow_mut::<TimeColumn>() {
            column.set_items(items, selected);
        }
    }

    /// Hand each column its list. Done every draw rather than on change:
    /// the hour decides the minutes, the am/pm mark decides the hours, and
    /// the bounds and the step are live properties that move under all
    /// three.
    fn refill(&mut self) {
        let grid = self.grid();
        self.time = grid.snap(self.time);

        let hours = self.column_hours(&grid);
        let hour_index = hours.iter().position(|h| *h == self.time.hour).unwrap_or(0);
        let hour_labels = hours.iter().map(|hour| self.hour_label(*hour)).collect();
        Self::fill(&self.hours, hour_labels, hour_index);

        let minutes = grid.minutes_in(self.time.hour);
        let minute_index = minutes.iter().position(|m| *m == self.time.minute).unwrap_or(0);
        let minute_labels = minutes.iter().map(|m| format!("{m:02}")).collect();
        Self::fill(&self.minutes, minute_labels, minute_index);

        let pm = self.time.hour >= 12;
        Self::fill(
            &self.meridiem,
            vec![AM.to_string(), PM.to_string()],
            if pm { 1 } else { 0 },
        );
    }

    fn moved(&mut self, cx: &mut Cx, time: CivilTime) {
        if time == self.time {
            return;
        }
        self.time = time;
        cx.widget_action(self.uid, TimePickerAction::Changed(time));
        self.redraw(cx);
    }

    pub fn time(&self) -> CivilTime {
        self.time
    }

    pub fn set_time(&mut self, cx: &mut Cx, time: CivilTime) {
        self.time = self.grid().snap(time);
        self.redraw(cx);
    }

    pub fn set_hour12(&mut self, cx: &mut Cx, hour12: bool) {
        if self.hour12 != hour12 {
            self.hour12 = hour12;
            self.redraw(cx);
        }
    }
}

impl Widget for TimePicker {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if self.disabled != disabled {
            self.disabled = disabled;
            for slot in [&self.hours, &self.minutes, &self.meridiem] {
                slot.set_disabled(cx, disabled);
            }
            self.redraw(cx);
        }
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.refill();
        self.draw_bg.disabled = if self.disabled { 1.0 } else { 0.0 };
        self.draw_bg.begin(cx, walk, self.layout);
        // The columns are drawn here rather than by a container, so nothing
        // else puts them in the tree: without this a host could not reach
        // ids!(picker.hours) at all.
        let mut slots = vec![
            (live_id!(hours), self.hours.clone()),
            (live_id!(minutes), self.minutes.clone()),
        ];
        if self.hour12 {
            slots.push((live_id!(meridiem), self.meridiem.clone()));
        }
        for (name, slot) in slots {
            cx.widget_tree_insert_child(self.uid, name, slot.clone());
            let slot_walk = slot.walk(cx.cx.cx);
            let _ = slot.draw_walk(cx, scope, slot_walk);
        }
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.disabled {
            return;
        }
        let grid = self.grid();
        let hours = self.column_hours(&grid);
        let minutes = grid.minutes_in(self.time.hour);
        let hours_uid = self.hours.widget_uid();
        let minutes_uid = self.minutes.widget_uid();
        let meridiem_uid = self.meridiem.widget_uid();
        let hour12 = self.hour12;

        let actions = cx.capture_actions(|cx| {
            self.hours.handle_event(cx, event, scope);
            self.minutes.handle_event(cx, event, scope);
            if hour12 {
                self.meridiem.handle_event(cx, event, scope);
            }
        });

        for action in actions {
            let widget_action = action.as_widget_action();
            if let TimeColumnAction::Picked(index) = widget_action.widget_uid_eq(hours_uid).cast() {
                if let Some(hour) = hours.get(index) {
                    let moved = grid.with_hour(self.time, *hour);
                    self.moved(cx, moved);
                }
            }
            if let TimeColumnAction::Picked(index) = widget_action.widget_uid_eq(minutes_uid).cast()
            {
                if let Some(minute) = minutes.get(index) {
                    let moved = grid.with_minute(self.time, *minute);
                    self.moved(cx, moved);
                }
            }
            if let TimeColumnAction::Picked(index) =
                widget_action.widget_uid_eq(meridiem_uid).cast()
            {
                // The mark is the other half of the day at the same hour of
                // it. An hour the grid does not offer is not moved to, which
                // is how a picker bounded to a morning refuses the afternoon.
                let half = if index == 1 { 12 } else { 0 };
                let moved = grid.with_hour(self.time, self.time.hour % 12 + half);
                self.moved(cx, moved);
            }
        }
    }

    fn text(&self) -> String {
        self.time.format(Clock::of(self.hour12))
    }
}

impl TimePickerRef {
    pub fn time(&self) -> CivilTime {
        self.borrow().map(|inner| inner.time()).unwrap_or_default()
    }

    pub fn set_time(&self, cx: &mut Cx, time: CivilTime) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_time(cx, time);
        }
    }

    pub fn set_hour12(&self, cx: &mut Cx, hour12: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_hour12(cx, hour12);
        }
    }

    /// The new time, when this picker moved in `actions`.
    pub fn changed(&self, actions: &Actions) -> Option<CivilTime> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            TimePickerAction::Changed(time) => Some(time),
            TimePickerAction::None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(hour: u32, minute: u32) -> CivilTime {
        CivilTime::new(hour, minute).expect("a valid time")
    }

    #[test]
    fn midnight_is_the_start_of_the_day_on_both_clocks() {
        let midnight = t(0, 0);
        assert_eq!(midnight.to_minutes(), 0);
        assert_eq!(midnight.format(Clock::H24), "00:00");
        // Not "0:00 AM": no clock face has a zero on it.
        assert_eq!(midnight.format(Clock::H12), "12:00 AM");
        assert_eq!(midnight.hour12(), (12, false));
        assert_eq!(CivilTime::parse("00:00"), Some(midnight));
        assert_eq!(CivilTime::parse("12:00 am"), Some(midnight));
        assert_eq!(CivilTime::parse("12am"), Some(midnight));
        // And the day comes round rather than running past its end.
        assert_eq!(t(23, 59).add_minutes(1), midnight);
        assert_eq!(midnight.add_minutes(-1), t(23, 59));
    }

    #[test]
    fn noon_is_twelve_on_both_clocks_but_not_the_same_twelve() {
        let noon = t(12, 0);
        assert_eq!(noon.format(Clock::H24), "12:00");
        assert_eq!(noon.format(Clock::H12), "12:00 PM");
        assert_eq!(noon.hour12(), (12, true));
        assert_eq!(CivilTime::parse("12pm"), Some(noon));
        assert_eq!(CivilTime::parse("12:00 p.m."), Some(noon));
        // The pair that catches everybody: same hour digit, twelve hours apart.
        assert_ne!(CivilTime::parse("12am"), CivilTime::parse("12pm"));
    }

    #[test]
    fn the_twelve_hour_reading_survives_a_round_trip_at_every_hour() {
        for hour in 0..24 {
            let time = t(hour, 30);
            let (shown, pm) = time.hour12();
            assert!((1..=12).contains(&shown), "hour {hour} showed as {shown}");
            assert_eq!(CivilTime::from_hour12(shown, pm, 30), Some(time));
        }
    }

    #[test]
    fn the_afternoon_boundary_is_where_the_hour_changes_meaning() {
        assert_eq!(CivilTime::parse("1 pm"), Some(t(13, 0)));
        assert_eq!(CivilTime::parse("1 am"), Some(t(1, 0)));
        assert_eq!(CivilTime::parse("11:59 am"), Some(t(11, 59)));
        assert_eq!(CivilTime::parse("12:01 am"), Some(t(0, 1)));
        assert_eq!(t(11, 59).add_minutes(1).format(Clock::H12), "12:00 PM");
        assert_eq!(t(12, 59).add_minutes(1).format(Clock::H12), "1:00 PM");
        // An hour outside a clock face carrying an am/pm mark is not a time.
        assert_eq!(CivilTime::parse("13 pm"), None);
        assert_eq!(CivilTime::parse("0 am"), None);
    }

    #[test]
    fn what_is_typed_is_read_the_way_it_was_meant() {
        assert_eq!(CivilTime::parse("9"), Some(t(9, 0)));
        assert_eq!(CivilTime::parse("09"), Some(t(9, 0)));
        assert_eq!(CivilTime::parse("930"), Some(t(9, 30)));
        assert_eq!(CivilTime::parse("0930"), Some(t(9, 30)));
        assert_eq!(CivilTime::parse("9.30"), Some(t(9, 30)));
        assert_eq!(CivilTime::parse(" 9:5 "), Some(t(9, 5)));
        assert_eq!(CivilTime::parse("21:45"), Some(t(21, 45)));
    }

    #[test]
    fn what_is_not_a_time_is_refused_rather_than_guessed_at() {
        for bad in ["", "abc", "24:00", "9:60", "99999", "pm", "+9:00", "9:3o"] {
            assert_eq!(CivilTime::parse(bad), None, "{bad:?} was read as a time");
        }
        assert_eq!(CivilTime::new(24, 0), None);
        assert_eq!(CivilTime::new(0, 60), None);
    }

    #[test]
    fn a_step_that_does_not_divide_the_hour_carries_on_across_it() {
        // Seven minutes from midnight. The first hour ends on :56 and the
        // second starts on :03 — the grid is counted from min, so it does
        // not restart at every hour, and each hour therefore has its own
        // list of minutes.
        let grid = TimeGrid::new(t(0, 0), t(23, 59), 7);
        assert_eq!(grid.minutes_in(0), vec![0, 7, 14, 21, 28, 35, 42, 49, 56]);
        assert_eq!(grid.minutes_in(1), vec![3, 10, 17, 24, 31, 38, 45, 52, 59]);
        assert_eq!(grid.snap(t(1, 5)), t(1, 3));
        assert_eq!(grid.snap(t(1, 7)), t(1, 10));
        assert!(grid.offers(t(1, 3)));
        assert!(!grid.offers(t(1, 5)));
    }

    #[test]
    fn the_last_offer_is_never_past_the_top_bound() {
        // Twenty-five minutes does not reach 17:00 from 09:00, and a grid
        // that offered 17:05 would be a bound nobody could trust.
        let grid = TimeGrid::new(t(9, 0), t(17, 0), 25);
        assert_eq!(grid.snap(t(23, 0)), t(16, 55));
        assert_eq!(grid.snap(t(0, 0)), t(9, 0));
        assert_eq!(*grid.hours().last().unwrap(), 16);
    }

    #[test]
    fn changing_the_hour_keeps_the_minute_when_that_hour_has_it() {
        let grid = TimeGrid::new(t(0, 0), t(23, 59), 7);
        // 0:21 exists; 1:21 does not, so the nearest minute that hour offers
        // is taken instead of silently moving to the top of it.
        assert_eq!(grid.with_hour(t(0, 21), 1), t(1, 24));
        let five = TimeGrid::new(t(0, 0), t(23, 59), 5);
        assert_eq!(five.with_hour(t(0, 20), 9), t(9, 20));
    }

    #[test]
    fn a_backwards_pair_of_bounds_is_one_instant_rather_than_an_error() {
        // Both bounds are live properties, and a tweaker can drag one past
        // the other at any moment.
        let grid = TimeGrid::new(t(17, 0), t(9, 0), 15);
        assert_eq!(grid.snap(t(12, 0)), t(17, 0));
        assert_eq!(grid.hours(), vec![17]);
    }

    #[test]
    fn a_zero_step_still_offers_something() {
        let grid = TimeGrid::new(t(0, 0), t(1, 0), 0);
        assert_eq!(grid.step, 1);
        assert_eq!(grid.minutes_in(0).len(), 60);
    }

    #[test]
    fn the_caret_says_which_part_the_arrows_move() {
        let text = "13:45";
        assert_eq!(part_at(text, 0), TimePart::Hour);
        // At the end of the hour is still the hour: that is where typing it
        // leaves the caret.
        assert_eq!(part_at(text, 2), TimePart::Hour);
        assert_eq!(part_at(text, 3), TimePart::Minute);
        assert_eq!(part_at(text, 5), TimePart::Minute);

        let text = "1:45 PM";
        assert_eq!(part_at(text, 1), TimePart::Hour);
        assert_eq!(part_at(text, 4), TimePart::Minute);
        assert_eq!(part_at(text, 5), TimePart::Meridiem);
        assert_eq!(part_at(text, 7), TimePart::Meridiem);
    }

    #[test]
    fn the_part_that_moved_is_the_part_left_selected() {
        assert_eq!(part_span("13:45", TimePart::Hour), (0, 2));
        assert_eq!(part_span("13:45", TimePart::Minute), (3, 5));
        assert_eq!(part_span("1:45 PM", TimePart::Hour), (0, 1));
        assert_eq!(part_span("1:45 PM", TimePart::Meridiem), (5, 7));
        // With no mark to select, the minute is the last thing there is.
        assert_eq!(part_span("13:45", TimePart::Meridiem), (3, 5));
    }

    #[test]
    fn an_arrow_key_moves_one_part_and_leaves_the_others_alone() {
        assert_eq!(bump(t(9, 30), TimePart::Hour, 1, 5), t(10, 30));
        assert_eq!(bump(t(9, 30), TimePart::Minute, -1, 5), t(9, 25));
        // Up on the hour at 23:xx comes round to 00:xx: a time has no date
        // to overflow into.
        assert_eq!(bump(t(23, 30), TimePart::Hour, 1, 5), t(0, 30));
        // The mark toggles whichever arrow was pressed, because twelve hours
        // either way is the same instant.
        assert_eq!(bump(t(9, 30), TimePart::Meridiem, 1, 5), t(21, 30));
        assert_eq!(bump(t(21, 30), TimePart::Meridiem, -1, 5), t(9, 30));
    }

    #[test]
    fn empty_bounds_mean_the_whole_day_from_either_end() {
        assert_eq!(bounds_of("", ""), (t(0, 0), t(23, 59)));
        assert_eq!(bounds_of("09:00", ""), (t(9, 0), t(23, 59)));
        assert_eq!(bounds_of("", "17:00"), (t(0, 0), t(17, 0)));
        // A top bound below the bottom one collapses to the bottom one
        // rather than inverting the range.
        assert_eq!(bounds_of("17:00", "09:00"), (t(17, 0), t(17, 0)));
    }

    #[test]
    fn a_column_keeps_the_chosen_row_in_the_middle_of_its_box() {
        // Seven rows of 22 in a box of 154.
        let view = ColumnView { row: 22.0, height: 154.0, count: 24, selected: 12 };
        assert_eq!(view.visible(), 7);
        assert_eq!(view.shown(), 7);
        assert_eq!(view.first(), 9);
        assert_eq!(view.top_of(12), Some(66.0));
        assert_eq!(view.top_of(0), None);
        assert_eq!(view.index_at(66.0), Some(12));
        assert_eq!(view.index_at(87.9), Some(12));
        assert_eq!(view.index_at(88.0), Some(13));
    }

    #[test]
    fn a_column_at_either_end_of_its_list_stops_scrolling() {
        let top = ColumnView { row: 22.0, height: 154.0, count: 24, selected: 0 };
        assert_eq!(top.first(), 0);
        let bottom = ColumnView { row: 22.0, height: 154.0, count: 24, selected: 23 };
        assert_eq!(bottom.first(), 17, "the last row is the last one drawn");
        assert_eq!(bottom.top_of(23), Some(132.0));
    }

    #[test]
    fn a_short_list_sits_in_the_middle_rather_than_hanging_from_the_top() {
        // The am/pm column: two values in a box built for seven.
        let view = ColumnView { row: 22.0, height: 154.0, count: 2, selected: 1 };
        assert_eq!(view.shown(), 2);
        assert_eq!(view.first(), 0);
        assert_eq!(view.pad(), 55.0);
        assert_eq!(view.top_of(0), Some(55.0));
        assert_eq!(view.top_of(1), Some(77.0));
        assert_eq!(view.index_at(10.0), None, "the padding is not a row");
        assert_eq!(view.index_at(120.0), None);
    }

    #[test]
    fn an_empty_column_has_no_rows_to_hit() {
        let view = ColumnView { row: 22.0, height: 154.0, count: 0, selected: 0 };
        assert_eq!(view.shown(), 0);
        assert_eq!(view.top_of(0), None);
        assert_eq!(view.index_at(30.0), None);
    }
}

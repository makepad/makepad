//! Calendar — a month grid, and the civil-date arithmetic underneath it.
//!
//! Two things live here. The first is [`CivilDate`]: a year, a month and a
//! day, with no time of day, no timezone and no clock behind it. Every date
//! widget in the library shares it, which is why it is dependency-free and
//! why it is tested harder than anything that draws.
//!
//! The second is the grid: rows of seven, weekday headings, the days either
//! side of the month drawn dim, and one, several or a span of days chosen
//! from it. [`MonthPicker`] and [`YearPicker`] are the same contract on a
//! twelve-cell grid, for the two steps above a day.
//!
//! # What this deliberately does not do
//!
//! It does not read a clock. There is no `today` until a host says what
//! today is, because a widget that asks the machine what time it is cannot
//! be tested, cannot be driven from a script, and is wrong in every
//! timezone but one. `today` is a property, and an empty one marks nothing.
//!
//! It does not localize. The month and weekday names are English and can be
//! replaced wholesale through `month_names` and `weekday_names`; there is no
//! locale database, no ordering rule beyond `first_day`, and no calendar but
//! the proleptic Gregorian one.
//!
//! It carries no time of day, no recurrence and no duration. A day is the
//! smallest thing it can name.
use crate::{
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    badge::measure,
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
};

// ---------------------------------------------------------------------------
// The civil-date core. No Cx, no script heap, no dependencies: everything
// from here down to the script block is plain arithmetic, and the tests at
// the bottom of the file exercise it directly.
// ---------------------------------------------------------------------------

/// A date on the proleptic Gregorian calendar: a year, a month 1..=12 and a
/// day 1..=31. It names a day and nothing else — no hour, no zone, no
/// instant.
///
/// The field order is what makes the derived `Ord` chronological, so dates
/// sort and compare without a helper.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CivilDate {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl Default for CivilDate {
    /// The epoch. It is the one day this type can name without asking
    /// anything outside itself, which is the whole point of the type.
    fn default() -> Self {
        Self { year: 1970, month: 1, day: 1 }
    }
}

impl CivilDate {
    /// A date, or nothing if there is no such day. February 30th and month
    /// 13 are refused rather than rolled over: rolling over turns a typo
    /// into a plausible wrong answer.
    pub fn new(year: i32, month: u32, day: u32) -> Option<Self> {
        if !(1..=12).contains(&month) || day < 1 || day > Self::month_length(year, month) {
            return None;
        }
        Some(Self { year, month, day })
    }

    /// The date `days` after 1970-01-01, counting backwards for negatives.
    ///
    /// The formula works in eras of 400 years and years that start in
    /// March, so the leap day falls at the end of a year rather than in the
    /// middle of one and every month length below it follows a fixed
    /// pattern. That is what removes the table lookup and the special case.
    pub fn from_days(days: i64) -> Self {
        let z = days + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = z - era * 146097; // 0..=146096
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // 0..=399
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // 0..=365
        let mp = (5 * doy + 2) / 153; // 0..=11, counting from March
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
        Self { year: (y + i64::from(month <= 2)) as i32, month, day }
    }

    /// Days since 1970-01-01. The inverse of [`CivilDate::from_days`].
    pub fn to_days(self) -> i64 {
        let m = self.month as i64;
        let d = self.day as i64;
        let y = self.year as i64 - i64::from(m <= 2);
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400; // 0..=399
        let mp = if m > 2 { m - 3 } else { m + 9 }; // 0..=11
        let doy = (153 * mp + 2) / 5 + d - 1; // 0..=365
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    }

    /// 0 is Monday, 6 is Sunday.
    ///
    /// Monday-first because [`CivilDate::iso_week`] counts from Monday, and
    /// a type with two different ideas of where a week starts is a type
    /// that will be got wrong. A calendar that draws Sunday first says so
    /// with `first_day`, which is a drawing decision, not this one.
    pub fn weekday(self) -> u32 {
        // The epoch was a Thursday, which is index 3.
        (self.to_days() + 3).rem_euclid(7) as u32
    }

    pub fn add_days(self, n: i64) -> Self {
        Self::from_days(self.to_days() + n)
    }

    /// The same day of a month `n` months away, with the day clamped back
    /// to the end of the month when it will not fit: 31 January plus one
    /// month is the 28th or the 29th.
    ///
    /// Clamping loses information — the two 31sts either side of February
    /// both land on the 28th — so stepping a month forward and back is not
    /// always where you started. That is the price of the operation being
    /// total, and the alternatives (refusing, or rolling into March) are
    /// worse for a header arrow, which is where this is used.
    pub fn add_months(self, n: i32) -> Self {
        let total = self.year as i64 * 12 + (self.month as i64 - 1) + n as i64;
        let year = total.div_euclid(12) as i32;
        let month = total.rem_euclid(12) as u32 + 1;
        let day = self.day.min(Self::month_length(year, month));
        Self { year, month, day }
    }

    /// The ISO week-numbering year and week, 1..=53.
    ///
    /// The year is not always this date's own year: the last days of
    /// December can belong to week 1 of the next year, and the first days
    /// of January to week 52 or 53 of the last one.
    pub fn iso_week(self) -> (i32, u32) {
        // A week belongs to the year that holds most of it, and the
        // Thursday is the day that is always on the majority side. Find
        // this week's Thursday and the rest is counting.
        let thursday = self.add_days(3 - self.weekday() as i64);
        let jan1 = Self { year: thursday.year, month: 1, day: 1 };
        let ordinal = thursday.to_days() - jan1.to_days();
        (thursday.year, (ordinal / 7) as u32 + 1)
    }

    /// How many days a month has, or 0 for a month that does not exist.
    pub fn month_length(year: i32, month: u32) -> u32 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if Self::is_leap(year) {
                    29
                } else {
                    28
                }
            }
            _ => 0,
        }
    }

    pub fn is_leap(year: i32) -> bool {
        year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
    }
}

/// A date written as `YYYY-MM-DD`, or nothing.
///
/// Strict on purpose: exactly ten characters, both dashes where they
/// belong, and a day that exists. Every date a host hands this library
/// arrives as text from a property, and a lenient parser there turns a
/// mistyped property into a date nobody chose.
pub fn parse_iso(text: &str) -> Option<CivilDate> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year: i32 = text[0..4].parse().ok()?;
    let month: u32 = text[5..7].parse().ok()?;
    let day: u32 = text[8..10].parse().ok()?;
    CivilDate::new(year, month, day)
}

/// Several dates separated by commas. Anything that does not parse is
/// dropped, so a half-typed property leaves the rest of the list working.
pub fn parse_iso_list(text: &str) -> Vec<CivilDate> {
    text.split(',').filter_map(|part| parse_iso(part)).collect()
}

pub fn format_iso(date: CivilDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year, date.month, date.day)
}

/// The English month names, used when `month_names` is empty.
pub const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// The English weekday headings, from Monday, used when `weekday_names` is
/// empty.
pub const WEEKDAY_NAMES: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];

/// Names read from a comma-separated property, or the built-in ones when
/// the property is empty or the wrong length. The wrong length is refused
/// whole rather than padded: eleven month names would silently shift half
/// the year by one.
fn names_or(text: &str, fallback: &[&'static str]) -> Vec<String> {
    let given: Vec<String> = text.split(',').map(|s| s.trim().to_string()).collect();
    if text.trim().is_empty() || given.len() != fallback.len() {
        return fallback.iter().map(|s| s.to_string()).collect();
    }
    given
}

/// A month laid out on seven columns, including the days either side of it
/// that fill the first and last rows.
///
/// Always six rows. A grid that is five rows in February and six in March
/// changes height when the header arrow is pressed, and everything under it
/// jumps; the sixth row costs one row of dim days and buys a control that
/// holds still.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MonthGrid {
    pub year: i32,
    pub month: u32,
    /// The weekday a row starts on: 0 is Monday.
    pub first_day: u32,
}

pub const GRID_COLS: usize = 7;
pub const GRID_ROWS: usize = 6;
pub const GRID_CELLS: usize = GRID_COLS * GRID_ROWS;

impl MonthGrid {
    pub fn first(self) -> CivilDate {
        CivilDate { year: self.year, month: self.month.clamp(1, 12), day: 1 }
    }

    /// How many cells of the month before come first.
    pub fn lead(self) -> u32 {
        (self.first().weekday() + 7 - self.first_day % 7) % 7
    }

    /// The date in cell `index`, counting left to right and top to bottom.
    pub fn cell(self, index: usize) -> CivilDate {
        self.first().add_days(index as i64 - self.lead() as i64)
    }

    /// Whether a date is one of this month's own days rather than a
    /// neighbour's.
    pub fn owns(self, date: CivilDate) -> bool {
        date.year == self.year && date.month == self.month.clamp(1, 12)
    }

    /// Where a date sits in the grid, if it is in it at all.
    pub fn index_of(self, date: CivilDate) -> Option<usize> {
        let offset = date.to_days() - self.cell(0).to_days();
        (0..GRID_CELLS as i64).contains(&offset).then_some(offset as usize)
    }
}

/// The limits a host puts on which days may be chosen, short of a rule of
/// its own.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct DayBounds {
    pub min: Option<CivilDate>,
    pub max: Option<CivilDate>,
    /// One bit per weekday from Monday; a set bit closes that weekday
    /// everywhere. 96 is the weekend.
    pub weekdays_off: u32,
}

impl DayBounds {
    pub fn allows(&self, date: CivilDate) -> bool {
        if self.min.is_some_and(|edge| date < edge) {
            return false;
        }
        if self.max.is_some_and(|edge| date > edge) {
            return false;
        }
        self.weekdays_off & (1 << date.weekday()) == 0
    }
}

/// How far the keyboard will look for an open day before giving up.
///
/// A calendar whose every day is closed must not spin, and a year and a day
/// is further than any real rule closes.
pub const SEEK_LIMIT: u32 = 366;

/// The first day at or after `from`, walking by `step`, that `allow`
/// accepts. Nothing, if it does not find one inside `limit` days.
pub fn seek(
    from: CivilDate,
    step: i64,
    limit: u32,
    allow: impl Fn(CivilDate) -> bool,
) -> Option<CivilDate> {
    let mut at = from;
    for _ in 0..=limit {
        if allow(at) {
            return Some(at);
        }
        at = at.add_days(step);
    }
    None
}

/// How many days may be chosen at once.
#[derive(Copy, Clone, Debug, Default, PartialEq, Script, ScriptHook)]
pub enum CalendarMode {
    /// One day. Choosing another puts the last one back.
    #[pick]
    #[default]
    Single,
    /// Any number of days, each pressed on and off.
    Multiple,
    /// Two days and everything between them, chosen with two presses.
    Range,
}

/// What one press did, so the widget knows what to report and the
/// arithmetic can be tested without one.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Picked {
    /// A range with one end chosen and the other still to come. There is
    /// nothing to report yet: half a range is not an answer.
    Waiting(CivilDate),
    Day(CivilDate),
    Span(CivilDate, CivilDate),
    /// A day left a multiple selection.
    Cleared(CivilDate),
}

/// Which days are chosen, and what a press does to them.
///
/// Held apart from the widget so the three modes can be tested without a
/// script heap, and so the grid and the keyboard go through one place
/// rather than two that drift.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DaySelection {
    mode: CalendarMode,
    days: Vec<CivilDate>,
    /// The first end of a range, while the second is still being chosen.
    anchor: Option<CivilDate>,
}

impl DaySelection {
    pub fn new(mode: CalendarMode) -> Self {
        Self { mode, days: Vec::new(), anchor: None }
    }

    pub fn mode(&self) -> CalendarMode {
        self.mode
    }

    /// The chosen days, in order. In `Range` this is the two ends, not
    /// every day between them: a span of a year is two dates, and spelling
    /// it out would make reading the selection cost the length of it.
    pub fn days(&self) -> &[CivilDate] {
        &self.days
    }

    pub fn anchor(&self) -> Option<CivilDate> {
        self.anchor
    }

    pub fn clear(&mut self) {
        self.days.clear();
        self.anchor = None;
    }

    /// Put a set of days in, sorted and trimmed to what the mode can hold.
    pub fn set_days(&mut self, days: &[CivilDate]) {
        let mut days = days.to_vec();
        days.sort_unstable();
        days.dedup();
        self.anchor = None;
        match self.mode {
            CalendarMode::Single => days.truncate(1),
            CalendarMode::Multiple => {}
            CalendarMode::Range => {
                if days.len() == 1 {
                    // One end given is a range half made, not a range: the
                    // next press finishes it rather than starting again.
                    self.anchor = Some(days[0]);
                } else if days.len() > 2 {
                    // Only the ends can be meant. Keep the widest reading.
                    let (lo, hi) = (days[0], days[days.len() - 1]);
                    days = vec![lo, hi];
                }
            }
        }
        self.days = days;
    }

    /// Whether a day is chosen in its own right — an end of a range counts,
    /// a day inside one does not.
    pub fn contains(&self, date: CivilDate) -> bool {
        self.days.contains(&date)
    }

    /// The finished span, once both ends are chosen.
    pub fn span(&self) -> Option<(CivilDate, CivilDate)> {
        if self.mode == CalendarMode::Range && self.days.len() == 2 {
            Some((self.days[0], self.days[1]))
        } else {
            None
        }
    }

    /// The span to draw, counting a half-made one against wherever the
    /// pointer is. Showing the range a second press would make is the only
    /// way to tell which end is anchored.
    pub fn preview(&self, hover: Option<CivilDate>) -> Option<(CivilDate, CivilDate)> {
        if let (CalendarMode::Range, Some(anchor), Some(at)) = (self.mode, self.anchor, hover) {
            return Some((anchor.min(at), anchor.max(at)));
        }
        self.span()
    }

    /// Press a day.
    pub fn pick(&mut self, date: CivilDate) -> Picked {
        match self.mode {
            CalendarMode::Single => {
                self.days = vec![date];
                self.anchor = None;
                Picked::Day(date)
            }
            CalendarMode::Multiple => {
                if let Some(at) = self.days.iter().position(|day| *day == date) {
                    self.days.remove(at);
                    Picked::Cleared(date)
                } else {
                    self.days.push(date);
                    self.days.sort_unstable();
                    Picked::Day(date)
                }
            }
            CalendarMode::Range => match self.anchor.take() {
                None => {
                    self.anchor = Some(date);
                    self.days = vec![date];
                    Picked::Waiting(date)
                }
                Some(anchor) => {
                    let (lo, hi) = (anchor.min(date), anchor.max(date));
                    self.days = vec![lo, hi];
                    Picked::Span(lo, hi)
                }
            },
        }
    }
}

/// One move of the keyboard focus across a month grid.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Motion {
    PrevDay,
    NextDay,
    PrevWeek,
    NextWeek,
    PrevMonth,
    NextMonth,
    WeekStart,
    WeekEnd,
}

/// Where a move lands, before anything is asked about whether that day is
/// open.
///
/// Nothing here stops at the edge of the drawn month: the arrows walk into
/// the next month and the grid follows them. Stopping instead would make
/// the last week of a month a dead end, and the only way out of it a mouse.
pub fn move_focus(from: CivilDate, first_day: u32, motion: Motion) -> CivilDate {
    let column = (from.weekday() + 7 - first_day % 7) % 7;
    match motion {
        Motion::PrevDay => from.add_days(-1),
        Motion::NextDay => from.add_days(1),
        Motion::PrevWeek => from.add_days(-7),
        Motion::NextWeek => from.add_days(7),
        Motion::PrevMonth => from.add_months(-1),
        Motion::NextMonth => from.add_months(1),
        Motion::WeekStart => from.add_days(-(column as i64)),
        Motion::WeekEnd => from.add_days((6 - column) as i64),
    }
}

/// The first year of the twelve-year page that holds `year`.
///
/// Pages are aligned on multiples of twelve so stepping is reversible: the
/// page you come back to is the page you left, whatever year the focus was
/// on when you left it.
pub fn year_page(year: i32) -> i32 {
    year - year.rem_euclid(12)
}

/// A host's own rule for which days may be chosen, on top of the bounds. It
/// answers true for a day that is open.
pub type DayAllowed = Box<dyn Fn(CivilDate) -> bool>;

/// What a calendar reports.
#[derive(Clone, Debug, Default)]
pub enum CalendarAction {
    /// The day a press acted on. In `Multiple` this fires for a day taken
    /// out as well as one put in — it names the day that was pressed, not
    /// the state it ended in. Ask `selected_days` for the whole set.
    Selected(CivilDate),
    /// Both ends of a finished span, earliest first. A half-made range
    /// reports nothing.
    RangeSelected(CivilDate, CivilDate),
    /// The grid moved to another month, by an arrow, by the keyboard, or by
    /// a press on a day belonging to a neighbour.
    MonthChanged(i32, u32),
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*

    /** How many days may be chosen at once. */
    mod.widgets.CalendarMode = #(CalendarMode::script_api(vm))

    use mod.widgets.*

    mod.widgets.CalendarBase = #(Calendar::register_widget(vm))
    mod.widgets.MonthPickerBase = #(MonthPicker::register_widget(vm))
    mod.widgets.YearPickerBase = #(YearPicker::register_widget(vm))

    set_type_default() do #(DrawCalendarFrame::script_shader(vm)){
        ..mod.draw.DrawQuad

        /** keyboard-focus mix 0..1 step 0.01 */
        focus: 0.0
        /** disabled mix 0..1 step 0.01 */
        disabled: 0.0

        /** bevel thickness in pixels 0..4 step 0.5 */
        border_size: uniform(theme.beveling)
        /** corner rounding radius 0..24 step 0.5 */
        border_radius: uniform(theme.radius_m)

        color: uniform(theme.color_bg_container)
        color_focus: uniform(theme.color_bg_container)
        color_disabled: uniform(theme.color_inset_disabled)

        border_color: uniform(theme.color_bevel_inset_1)
        border_color_focus: uniform(theme.color_bevel_inset_1_focus)
        border_color_disabled: uniform(theme.color_bevel_inset_1_disabled)

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let fill = self.color
                .mix(self.color_focus, self.focus)
                .mix(self.color_disabled, self.disabled)
            let stroke = self.border_color
                .mix(self.border_color_focus, self.focus)
                .mix(self.border_color_disabled, self.disabled)
            sdf.box(
                self.border_size
                self.border_size
                self.rect_size.x - self.border_size * 2.
                self.rect_size.y - self.border_size * 2.
                self.border_radius
            )
            sdf.fill_keep(fill)
            sdf.stroke(stroke, self.border_size)
            return sdf.result
        }
    }

    set_type_default() do #(DrawCalendarCell::script_shader(vm)){
        ..mod.draw.DrawQuad

        /** the day is chosen in its own right 0..1 step 1 */
        selected: 0.0
        /** the day is inside a chosen span 0..1 step 1 */
        span: 0.0
        /** -1 at the start of a span, 1 at the end, 0 in the middle -1..1 step 1 */
        span_end: 0.0
        /** the day the host marked as today 0..1 step 1 */
        today: 0.0
        /** the day the keyboard is on 0..1 step 1 */
        focus: 0.0
        /** the day the pointer is over 0..1 step 1 */
        hover: 0.0
        /** room left around the mark inside the cell, in pixels 0..8 step 0.5 */
        pad_px: 2.0

        /** corner rounding radius 0..16 step 0.5 */
        border_radius: uniform(theme.radius_s)
        color_selected: uniform(theme.color_primary)
        color_span: uniform(theme.color_primary_container)
        color_hover: uniform(theme.color_surface_container_high)
        color_focus: uniform(theme.color_outline)
        color_today: uniform(theme.color_primary)

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let w = self.rect_size.x
            let h = self.rect_size.y
            let pad = self.pad_px
            let bh = max(h - pad * 2., 1.)

            // The band that joins one day of a span to the next runs to the
            // cell edges, so a week of chosen days reads as one bar rather
            // than seven boxes. At the two ends it stops halfway and lets
            // the rounded cap drawn after it finish the shape.
            if self.span > 0.5 {
                let mut x0 = 0.
                let mut x1 = w
                if self.span_end < -0.5 { x0 = w * 0.5 }
                if self.span_end > 0.5 { x1 = w * 0.5 }
                sdf.rect(x0, pad, max(x1 - x0, 0.), bh)
                sdf.fill(self.color_span)
            }
            if self.hover > 0.5 {
                sdf.box(pad, pad, w - pad * 2., bh, self.border_radius)
                sdf.fill(self.color_hover)
            }
            if self.selected > 0.5 {
                sdf.box(pad, pad, w - pad * 2., bh, self.border_radius)
                sdf.fill(self.color_selected)
            }
            // A stroke, not a fill: the focus has to be legible on a chosen
            // day as well as an empty one, and a fill under a chosen day is
            // invisible.
            if self.focus > 0.5 {
                sdf.box(pad + 0.5, pad + 0.5, w - pad * 2. - 1., bh - 1., self.border_radius)
                sdf.stroke(self.color_focus, 1.)
            }
            // Today is a dot under the number rather than a ring around it.
            // A ring beside a chosen day reads as a second, weaker
            // selection, and people press it.
            if self.today > 0.5 {
                sdf.circle(w * 0.5, h - pad - 2.5, 1.5)
                sdf.fill(self.color_today)
            }
            return sdf.result
        }
    }

    /** A month grid: rows of seven, the neighbouring months dimmed, and one
     * day, several days or a span chosen from it. */
    mod.widgets.Calendar = set_type_default() do mod.widgets.CalendarBase{
        width: Fit
        height: Fit
        margin: theme.mspace_1

        /** how many days may be chosen at once */
        mode: CalendarMode.Single
        /** the weekday a row starts on; 0 is Monday 0..6 step 1 */
        first_day: 0
        /** the year on show; 0 takes it from the selection, then from today 0..3000 step 1 */
        year: 0
        /** the month on show; 0 takes it from the selection, then from today 0..12 step 1 */
        month: 0
        /** the day marked as today, as an ISO date; empty marks none */
        today: ""
        /** the days chosen at the start, ISO dates separated by commas */
        selected: ""
        /** earliest day that may be chosen, as an ISO date; empty for none */
        min: ""
        /** latest day that may be chosen, as an ISO date; empty for none */
        max: ""
        /** weekdays closed everywhere, one bit each from Monday; 96 is the weekend 0..127 step 1 */
        weekdays_off: 0
        /** number each row with its ISO week down the left */
        show_week_numbers: false
        /** the twelve month names, comma separated; empty uses the built-in ones */
        month_names: ""
        /** the seven weekday headings from Monday, comma separated */
        weekday_names: ""

        /** width of one day cell in pixels 20..64 step 1 */
        cell_width: 34.
        /** height of one day cell in pixels 20..64 step 1 */
        cell_height: 30.
        /** height of the month-and-year band in pixels 0..60 step 1 */
        header_height: 32.
        /** height of the weekday headings in pixels 0..40 step 1 */
        weekday_height: 22.
        /** width of the week-number column in pixels 12..48 step 1 */
        week_width: 26.
        /** width of each step mark in the header in pixels 16..48 step 1 */
        arrow_width: 28.
        /** room between the frame and everything in it in pixels 0..24 step 1 */
        inset: 8.

        color_title: theme.color_text
        color_arrow: theme.color_text_meta
        color_weekday: theme.color_text_meta
        color_day: theme.color_text
        color_outside: theme.color_text_disabled
        color_off: theme.color_text_disabled
        color_on_selected: theme.color_on_primary
        color_week: theme.color_text_meta

        draw_title +: {text_style: theme.font_bold{font_size: theme.font_size_p}}
        draw_arrow +: {text_style: theme.font_regular{font_size: theme.font_size_4}}
        draw_day +: {text_style: theme.font_regular{font_size: theme.font_size_p}}
        draw_meta +: {text_style: theme.font_regular{font_size: theme.font_size_p}}

        animator: Animator{
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {focus: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {focus: 1.0}}
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

    /** Twelve months on one page, chosen the way a calendar chooses days.
     * The step marks move a year at a time. */
    mod.widgets.MonthPicker = set_type_default() do mod.widgets.MonthPickerBase{
        width: Fit
        height: Fit
        margin: theme.mspace_1

        /** how many months may be chosen at once */
        mode: CalendarMode.Single
        /** the year on show; 0 takes it from the selection 0..3000 step 1 */
        year: 0
        /** the months chosen at the start, ISO dates separated by commas */
        selected: ""
        /** earliest month that may be chosen, as an ISO date; empty for none */
        min: ""
        /** latest month that may be chosen, as an ISO date; empty for none */
        max: ""
        /** the twelve month names, comma separated; empty uses the built-in ones */
        month_names: ""
        /** write the month names in full rather than shortened to three letters */
        long_names: false

        /** how many cells to a row 2..6 step 1 */
        columns: 3
        /** width of one cell in pixels 40..140 step 2 */
        cell_width: 72.
        /** height of one cell in pixels 24..64 step 1 */
        cell_height: 34.
        /** height of the year band in pixels 0..60 step 1 */
        header_height: 32.
        /** width of each step mark in the header in pixels 16..48 step 1 */
        arrow_width: 28.
        /** room between the frame and everything in it in pixels 0..24 step 1 */
        inset: 8.

        color_title: theme.color_text
        color_arrow: theme.color_text_meta
        color_day: theme.color_text
        color_off: theme.color_text_disabled
        color_on_selected: theme.color_on_primary

        draw_title +: {text_style: theme.font_bold{font_size: theme.font_size_p}}
        draw_arrow +: {text_style: theme.font_regular{font_size: theme.font_size_4}}
        draw_day +: {text_style: theme.font_regular{font_size: theme.font_size_p}}

        animator: Animator{
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {focus: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {focus: 1.0}}
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

    /** Twelve years on one page, chosen the way a calendar chooses days.
     * The step marks move a whole page, so the pages line up. */
    mod.widgets.YearPicker = set_type_default() do mod.widgets.YearPickerBase{
        width: Fit
        height: Fit
        margin: theme.mspace_1

        /** how many years may be chosen at once */
        mode: CalendarMode.Single
        /** the year the page opens on; 0 takes it from the selection 0..3000 step 1 */
        year: 0
        /** the years chosen at the start, ISO dates separated by commas */
        selected: ""
        /** earliest year that may be chosen, as an ISO date; empty for none */
        min: ""
        /** latest year that may be chosen, as an ISO date; empty for none */
        max: ""

        /** how many cells to a row 2..6 step 1 */
        columns: 3
        /** width of one cell in pixels 40..140 step 2 */
        cell_width: 72.
        /** height of one cell in pixels 24..64 step 1 */
        cell_height: 34.
        /** height of the page band in pixels 0..60 step 1 */
        header_height: 32.
        /** width of each step mark in the header in pixels 16..48 step 1 */
        arrow_width: 28.
        /** room between the frame and everything in it in pixels 0..24 step 1 */
        inset: 8.

        color_title: theme.color_text
        color_arrow: theme.color_text_meta
        color_day: theme.color_text
        color_off: theme.color_text_disabled
        color_on_selected: theme.color_on_primary

        draw_title +: {text_style: theme.font_bold{font_size: theme.font_size_p}}
        draw_arrow +: {text_style: theme.font_regular{font_size: theme.font_size_4}}
        draw_day +: {text_style: theme.font_regular{font_size: theme.font_size_p}}

        animator: Animator{
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {focus: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {focus: 1.0}}
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
}

/// The frame all three draw themselves inside. It is also the widget's hit
/// area and its redraw area: every cell is placed absolutely and leaves
/// neither.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCalendarFrame {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    focus: f32,
    #[live]
    disabled: f32,
}

/// One cell of a grid. The same shader draws a day, a month and a year:
/// what changes between them is the text, not the mark.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCalendarCell {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    selected: f32,
    #[live]
    span: f32,
    #[live]
    span_end: f32,
    #[live]
    today: f32,
    #[live]
    focus: f32,
    #[live]
    hover: f32,
    #[live]
    pad_px: f32,
}

/// How one cell should look, worked out before anything is drawn so the
/// quads and the glyphs can go down in two passes rather than interleaved.
#[derive(Copy, Clone, Debug, Default)]
struct CellLook {
    selected: bool,
    span: bool,
    span_end: f32,
    today: bool,
    focus: bool,
    hover: bool,
    outside: bool,
    off: bool,
}

/// Centre one line of text in a box.
///
/// `draw_abs` is handed the top of the LINE box, not the top of the ink,
/// and a digit's ink starts about three tenths of the font size below it.
/// Centring the line box — which is what an `Align` of 0.5 does — leaves
/// every number in the grid riding high in its cell.
fn draw_centered(draw_text: &mut DrawText, cx: &mut Cx2d, rect: Rect, text: &str) {
    let size = draw_text.text_style.font_size as f64;
    let width = measure(draw_text, cx, text);
    let x = rect.pos.x + (rect.size.x - width) * 0.5;
    let y = rect.pos.y + (rect.size.y - size) * 0.5 - size * 0.30;
    draw_text.draw_abs(cx, dvec2(x, y), text);
}

/// The header band: a title between two step marks. Returns the two mark
/// rects, so the hit test uses the geometry that was actually drawn.
fn draw_header(
    draw_title: &mut DrawText,
    draw_arrow: &mut DrawText,
    cx: &mut Cx2d,
    band: Rect,
    arrow_width: f64,
    title: &str,
) -> (Rect, Rect) {
    let prev = Rect { pos: band.pos, size: dvec2(arrow_width, band.size.y) };
    let next = Rect {
        pos: dvec2(band.pos.x + band.size.x - arrow_width, band.pos.y),
        size: dvec2(arrow_width, band.size.y),
    };
    let middle = Rect {
        pos: dvec2(band.pos.x + arrow_width, band.pos.y),
        size: dvec2((band.size.x - arrow_width * 2.0).max(0.0), band.size.y),
    };
    draw_centered(draw_title, cx, middle, title);
    // The two marks are glyphs. Small chevrons drawn as sdf paths do not
    // paint reliably here — two mirrored paths, and only the second one
    // appears — and a glyph costs one draw call either way.
    draw_centered(draw_arrow, cx, prev, "\u{2039}");
    draw_centered(draw_arrow, cx, next, "\u{203a}");
    (prev, next)
}

/// Where cell `column` of a `cols`-wide row sits inside `row_area`.
fn grid_cell(row_area: Rect, cols: usize, column: usize) -> Rect {
    let cell_w = row_area.size.x / cols.max(1) as f64;
    Rect {
        pos: dvec2(row_area.pos.x + column as f64 * cell_w, row_area.pos.y),
        size: dvec2(cell_w, row_area.size.y),
    }
}

#[derive(Script, Widget, Animator)]
pub struct Calendar {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_bg: DrawCalendarFrame,
    #[live]
    draw_cell: DrawCalendarCell,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_arrow: DrawText,
    #[live]
    draw_day: DrawText,
    #[live]
    draw_meta: DrawText,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[apply_default]
    animator: Animator,

    #[live]
    pub mode: CalendarMode,
    #[live]
    pub first_day: u32,
    #[live]
    pub year: u32,
    #[live]
    pub month: u32,
    #[live]
    pub today: String,
    #[live]
    pub selected: String,
    #[live]
    pub min: String,
    #[live]
    pub max: String,
    #[live]
    pub weekdays_off: u32,
    #[live]
    pub show_week_numbers: bool,
    #[live]
    pub month_names: String,
    #[live]
    pub weekday_names: String,

    #[live(34.0)]
    pub cell_width: f64,
    #[live(30.0)]
    pub cell_height: f64,
    #[live(32.0)]
    pub header_height: f64,
    #[live(22.0)]
    pub weekday_height: f64,
    #[live(26.0)]
    pub week_width: f64,
    #[live(28.0)]
    pub arrow_width: f64,
    #[live(8.0)]
    pub inset: f64,

    #[live]
    pub color_title: Vec4f,
    #[live]
    pub color_arrow: Vec4f,
    #[live]
    pub color_weekday: Vec4f,
    #[live]
    pub color_day: Vec4f,
    #[live]
    pub color_outside: Vec4f,
    #[live]
    pub color_off: Vec4f,
    #[live]
    pub color_on_selected: Vec4f,
    #[live]
    pub color_week: Vec4f,

    /// The month on show. Kept apart from the `year` and `month`
    /// properties, which say where to start: an arrow press moves this, and
    /// the next live re-apply must not undo it.
    #[rust]
    view_year: i32,
    #[rust]
    view_month: u32,
    #[rust]
    focus: CivilDate,
    #[rust]
    selection: DaySelection,
    #[rust]
    bounds: DayBounds,
    #[rust]
    today_date: Option<CivilDate>,
    #[rust]
    hover: Option<CivilDate>,
    #[rust]
    cells: Vec<(Rect, CivilDate)>,
    #[rust]
    prev_rect: Rect,
    #[rust]
    next_rect: Rect,
    /// The `selected` text this widget last read. A press changes the
    /// selection without changing the text, and re-reading unchanged text
    /// would throw the press away.
    #[rust]
    adopted: String,
    #[rust]
    adopted_month: (u32, u32),
    #[rust]
    allowed: Option<DayAllowed>,
}

impl ScriptHook for Calendar {
    fn on_after_new(&mut self, _vm: &mut ScriptVm) {
        self.adopt();
    }
}

impl Calendar {
    fn adopt(&mut self) {
        self.selection = DaySelection::new(self.mode);
        self.adopted = self.selected.clone();
        self.selection.set_days(&parse_iso_list(&self.selected));
        self.adopted_month = (self.year, self.month);
        self.today_date = parse_iso(&self.today);
        // No clock. The month on show is whatever the host named, then the
        // first day it chose, then the day it called today, and only then
        // the epoch — a visible, honest wrong answer rather than a guess
        // dressed as a right one.
        let anchor = CivilDate::new(self.year as i32, self.month, 1)
            .or_else(|| self.selection.days().first().copied())
            .or(self.today_date)
            .unwrap_or_default();
        self.view_year = anchor.year;
        self.view_month = anchor.month;
        self.focus = self.selection.days().first().copied().unwrap_or(anchor);
    }

    /// Take up whatever the properties say now. Called every draw because
    /// every one of them is live, and the tweaker may have moved any of
    /// them since the last one.
    fn sync(&mut self) {
        if self.selection.mode() != self.mode {
            // A range half made cannot be read as a day, and a set of five
            // days cannot be read as a range. Start again rather than
            // reinterpret.
            self.selection = DaySelection::new(self.mode);
            self.adopted.clear();
        }
        if self.adopted != self.selected {
            self.adopted = self.selected.clone();
            self.selection.set_days(&parse_iso_list(&self.selected));
        }
        if self.adopted_month != (self.year, self.month) {
            self.adopted_month = (self.year, self.month);
            if let Some(date) = CivilDate::new(self.year as i32, self.month, 1) {
                self.view_year = date.year;
                self.view_month = date.month;
            }
        }
        self.bounds = DayBounds {
            min: parse_iso(&self.min),
            max: parse_iso(&self.max),
            weekdays_off: self.weekdays_off,
        };
        self.today_date = parse_iso(&self.today);
        if self.view_month == 0 {
            self.view_month = 1;
        }
    }

    fn grid(&self) -> MonthGrid {
        MonthGrid { year: self.view_year, month: self.view_month, first_day: self.first_day }
    }

    /// Whether a day may be chosen: the bounds first, then the host's own
    /// rule.
    pub fn allows(&self, date: CivilDate) -> bool {
        self.bounds.allows(date) && self.allowed.as_ref().map(|rule| rule(date)).unwrap_or(true)
    }

    fn title(&self) -> String {
        let names = names_or(&self.month_names, &MONTH_NAMES);
        let index = (self.view_month.clamp(1, 12) - 1) as usize;
        format!("{} {}", names[index], self.view_year)
    }

    fn show(&mut self, cx: &mut Cx, date: CivilDate) {
        if date.year != self.view_year || date.month != self.view_month {
            self.view_year = date.year;
            self.view_month = date.month;
            cx.widget_action(
                self.uid,
                CalendarAction::MonthChanged(self.view_year, self.view_month),
            );
        }
    }

    fn step_month(&mut self, cx: &mut Cx, delta: i32) {
        let shown =
            CivilDate { year: self.view_year, month: self.view_month, day: 1 }.add_months(delta);
        // The focus travels with the grid, or the next arrow key would jump
        // the view back to wherever the focus was left.
        self.focus = self.focus.add_months(delta);
        self.show(cx, shown);
        self.redraw(cx);
    }

    fn choose(&mut self, cx: &mut Cx, date: CivilDate) {
        if !self.allows(date) {
            return;
        }
        self.focus = date;
        // A day belonging to a neighbouring month is a day like any other:
        // pressing it moves the grid to the month that owns it rather than
        // refusing, which is what makes the two dim rows worth drawing.
        self.show(cx, date);
        let uid = self.uid;
        match self.selection.pick(date) {
            Picked::Day(day) | Picked::Cleared(day) => {
                cx.widget_action(uid, CalendarAction::Selected(day));
            }
            Picked::Span(start, end) => {
                cx.widget_action(uid, CalendarAction::RangeSelected(start, end));
            }
            Picked::Waiting(_) => {}
        }
        self.redraw(cx);
    }

    /// The days chosen now. In `Range` this is the two ends.
    pub fn selected_days(&self) -> Vec<CivilDate> {
        self.selection.days().to_vec()
    }

    pub fn range(&self) -> Option<(CivilDate, CivilDate)> {
        self.selection.span()
    }

    pub fn set_selected_days(&mut self, cx: &mut Cx, days: &[CivilDate]) {
        self.selection.set_days(days);
        if let Some(first) = days.first() {
            self.focus = *first;
            self.view_year = first.year;
            self.view_month = first.month;
        }
        self.redraw(cx);
    }

    /// The month on show, as a year and a month 1..=12.
    pub fn shown_month(&self) -> (i32, u32) {
        (self.view_year, self.view_month)
    }

    pub fn set_shown_month(&mut self, cx: &mut Cx, year: i32, month: u32) {
        self.view_year = year;
        self.view_month = month.clamp(1, 12);
        self.redraw(cx);
    }
}

impl Widget for Calendar {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.animator_toggle(cx, disabled, Animate::Yes, ids!(disabled.on), ids!(disabled.off));
    }

    fn disabled(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(disabled.on))
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.sync();
        let week_col = if self.show_week_numbers { self.week_width } else { 0.0 };
        // A Fit calendar asks for exactly what it is about to draw. Fill
        // inside a Fit parent resolves to nothing and is never painted, so
        // there is no useful default here but a real number.
        let natural_w = self.inset * 2.0 + week_col + GRID_COLS as f64 * self.cell_width;
        let natural_h = self.inset * 2.0
            + self.header_height
            + self.weekday_height
            + GRID_ROWS as f64 * self.cell_height;
        let walk = Walk {
            width: match walk.width {
                Size::Fit { .. } => Size::Fixed(natural_w),
                other => other,
            },
            height: match walk.height {
                Size::Fit { .. } => Size::Fixed(natural_h),
                other => other,
            },
            ..walk
        };

        self.draw_bg.begin(cx, walk, self.layout);
        let frame = cx.turtle().rect();
        let inner = Rect {
            pos: dvec2(frame.pos.x + self.inset, frame.pos.y + self.inset),
            size: dvec2(
                (frame.size.x - self.inset * 2.0).max(1.0),
                (frame.size.y - self.inset * 2.0).max(1.0),
            ),
        };

        self.draw_title.color = self.color_title;
        self.draw_arrow.color = self.color_arrow;
        let title = self.title();
        let band = Rect { pos: inner.pos, size: dvec2(inner.size.x, self.header_height) };
        let (prev, next) = draw_header(
            &mut self.draw_title,
            &mut self.draw_arrow,
            cx,
            band,
            self.arrow_width,
            &title,
        );
        self.prev_rect = prev;
        self.next_rect = next;

        let grid_x = inner.pos.x + week_col;
        let grid_w = (inner.size.x - week_col).max(1.0);
        let cell_w = grid_w / GRID_COLS as f64;
        let cell_h =
            ((inner.size.y - self.header_height - self.weekday_height) / GRID_ROWS as f64).max(1.0);
        let heads_y = inner.pos.y + self.header_height;
        let grid_y = heads_y + self.weekday_height;

        let weekdays = names_or(&self.weekday_names, &WEEKDAY_NAMES);
        self.draw_meta.color = self.color_weekday;
        for col in 0..GRID_COLS {
            let name = weekdays[((self.first_day as usize % 7) + col) % 7].clone();
            let rect = Rect {
                pos: dvec2(grid_x + col as f64 * cell_w, heads_y),
                size: dvec2(cell_w, self.weekday_height),
            };
            draw_centered(&mut self.draw_meta, cx, rect, &name);
        }

        let grid = self.grid();
        let focused = self.animator_in_state(cx, ids!(focus.on));
        let off = self.animator_in_state(cx, ids!(disabled.on));
        let preview = self.selection.preview(self.hover);

        self.cells.clear();
        let mut looks: Vec<(Rect, CivilDate, CellLook)> = Vec::with_capacity(GRID_CELLS);
        for index in 0..GRID_CELLS {
            let date = grid.cell(index);
            let rect = Rect {
                pos: dvec2(
                    grid_x + (index % GRID_COLS) as f64 * cell_w,
                    grid_y + (index / GRID_COLS) as f64 * cell_h,
                ),
                size: dvec2(cell_w, cell_h),
            };
            let allowed = !off && self.allows(date);
            let selected = self.selection.contains(date);
            let mut look = CellLook {
                selected,
                today: self.today_date == Some(date),
                focus: focused && self.focus == date,
                hover: allowed && self.hover == Some(date) && !selected,
                outside: !grid.owns(date),
                off: !allowed,
                ..CellLook::default()
            };
            if let Some((start, end)) = preview {
                if start < end && date >= start && date <= end {
                    look.span = true;
                    look.span_end = if date == start {
                        -1.0
                    } else if date == end {
                        1.0
                    } else {
                        0.0
                    };
                    look.selected = date == start || date == end;
                }
            }
            self.cells.push((rect, date));
            looks.push((rect, date, look));
        }

        // Quads first, then glyphs. Interleaving them would put a cell's
        // background over the number of the cell drawn before it the moment
        // any two cells overlap, and it costs the same number of draw calls
        // either way.
        for (rect, _, look) in &looks {
            self.draw_cell.selected = if look.selected { 1.0 } else { 0.0 };
            self.draw_cell.span = if look.span { 1.0 } else { 0.0 };
            self.draw_cell.span_end = look.span_end;
            self.draw_cell.today = if look.today { 1.0 } else { 0.0 };
            self.draw_cell.focus = if look.focus { 1.0 } else { 0.0 };
            self.draw_cell.hover = if look.hover { 1.0 } else { 0.0 };
            self.draw_cell.draw_abs(cx, *rect);
        }
        for (rect, date, look) in &looks {
            self.draw_day.color = if look.off {
                self.color_off
            } else if look.selected {
                self.color_on_selected
            } else if look.outside {
                self.color_outside
            } else {
                self.color_day
            };
            let label = date.day.to_string();
            draw_centered(&mut self.draw_day, cx, *rect, &label);
        }

        if self.show_week_numbers {
            self.draw_meta.color = self.color_week;
            for row in 0..GRID_ROWS {
                let (_, week) = grid.cell(row * GRID_COLS).iso_week();
                let rect = Rect {
                    pos: dvec2(inner.pos.x, grid_y + row as f64 * cell_h),
                    size: dvec2(week_col, cell_h),
                };
                draw_centered(&mut self.draw_meta, cx, rect, &week.to_string());
            }
        }

        self.draw_bg.end(cx);
        if !off {
            cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.animator_handle_event(cx, event);
        if self.animator_in_state(cx, ids!(disabled.on)) {
            return;
        }
        let uid = self.uid;
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let at = self
                    .cells
                    .iter()
                    .find(|(rect, _)| rect.contains(fe.abs))
                    .map(|(_, date)| *date)
                    .filter(|date| self.allows(*date));
                let on_step = self.prev_rect.contains(fe.abs) || self.next_rect.contains(fe.abs);
                cx.set_cursor(if at.is_some() || on_step {
                    MouseCursor::Hand
                } else {
                    MouseCursor::Default
                });
                if at != self.hover {
                    self.hover = at;
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.redraw(cx);
                }
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                if self.prev_rect.contains(fe.abs) {
                    self.step_month(cx, -1);
                    return;
                }
                if self.next_rect.contains(fe.abs) {
                    self.step_month(cx, 1);
                    return;
                }
                let hit = self
                    .cells
                    .iter()
                    .find(|(rect, _)| rect.contains(fe.abs))
                    .map(|(_, date)| *date);
                if let Some(date) = hit {
                    self.choose(cx, date);
                }
            }
            Hit::KeyFocus(_) => {
                self.animator_play(cx, ids!(focus.on));
                self.redraw(cx);
            }
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, ids!(focus.off));
                self.redraw(cx);
            }
            Hit::KeyDown(ke) => {
                let motion = match ke.key_code {
                    KeyCode::ArrowLeft => Some(Motion::PrevDay),
                    KeyCode::ArrowRight => Some(Motion::NextDay),
                    KeyCode::ArrowUp => Some(Motion::PrevWeek),
                    KeyCode::ArrowDown => Some(Motion::NextWeek),
                    KeyCode::PageUp => Some(Motion::PrevMonth),
                    KeyCode::PageDown => Some(Motion::NextMonth),
                    KeyCode::Home => Some(Motion::WeekStart),
                    KeyCode::End => Some(Motion::WeekEnd),
                    _ => None,
                };
                if let Some(motion) = motion {
                    let want = move_focus(self.focus, self.first_day, motion);
                    // Step over the days the host has closed rather than
                    // parking on one: the focus would otherwise sit
                    // somewhere Return does nothing and say nothing about
                    // why.
                    let step = if want < self.focus { -1 } else { 1 };
                    let landed =
                        seek(want, step, SEEK_LIMIT, |date| self.allows(date)).unwrap_or(want);
                    self.focus = landed;
                    self.show(cx, landed);
                    self.redraw(cx);
                }
                if matches!(ke.key_code, KeyCode::ReturnKey | KeyCode::Space) {
                    let focus = self.focus;
                    self.choose(cx, focus);
                }
                // Half a range is not an answer, and once one end is picked
                // up there is otherwise no way to put it down.
                if ke.key_code == KeyCode::Escape && self.selection.anchor().is_some() {
                    self.selection.clear();
                    cx.widget_action(uid, CalendarAction::Selected(self.focus));
                    self.redraw(cx);
                }
            }
            _ => {}
        }
    }

    /// The month on show and how many days are chosen, so a test can read
    /// the whole widget in one line.
    fn text(&self) -> String {
        format!("{} ({} chosen)", self.title(), self.selection.days().len())
    }
}

impl CalendarRef {
    pub fn selected_days(&self) -> Vec<CivilDate> {
        self.borrow().map(|inner| inner.selected_days()).unwrap_or_default()
    }

    /// The one chosen day, for a calendar in `Single`.
    pub fn selected_day(&self) -> Option<CivilDate> {
        self.borrow().and_then(|inner| inner.selected_days().first().copied())
    }

    pub fn range(&self) -> Option<(CivilDate, CivilDate)> {
        self.borrow().and_then(|inner| inner.range())
    }

    pub fn set_selected_days(&self, cx: &mut Cx, days: &[CivilDate]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_selected_days(cx, days);
        }
    }

    pub fn shown_month(&self) -> (i32, u32) {
        self.borrow().map(|inner| inner.shown_month()).unwrap_or((1970, 1))
    }

    pub fn set_shown_month(&self, cx: &mut Cx, year: i32, month: u32) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_shown_month(cx, year, month);
        }
    }

    /// A rule of the host's own for which days are open, on top of `min`,
    /// `max` and `weekdays_off`. It answers true for a day that may be
    /// chosen.
    pub fn set_day_allowed(&self, rule: DayAllowed) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.allowed = Some(rule);
        }
    }

    /// The day a press acted on this pass.
    pub fn selected(&self, actions: &Actions) -> Option<CivilDate> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            CalendarAction::Selected(date) => Some(date),
            _ => None,
        }
    }

    /// Both ends of a span, the pass the second one was chosen.
    pub fn range_selected(&self, actions: &Actions) -> Option<(CivilDate, CivilDate)> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            CalendarAction::RangeSelected(start, end) => Some((start, end)),
            _ => None,
        }
    }

    /// The month the grid moved to this pass.
    pub fn month_changed(&self, actions: &Actions) -> Option<(i32, u32)> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            CalendarAction::MonthChanged(year, month) => Some((year, month)),
            _ => None,
        }
    }
}

#[derive(Script, Widget, Animator)]
pub struct MonthPicker {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_bg: DrawCalendarFrame,
    #[live]
    draw_cell: DrawCalendarCell,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_arrow: DrawText,
    #[live]
    draw_day: DrawText,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[apply_default]
    animator: Animator,

    #[live]
    pub mode: CalendarMode,
    #[live]
    pub year: u32,
    #[live]
    pub selected: String,
    #[live]
    pub min: String,
    #[live]
    pub max: String,
    #[live]
    pub month_names: String,
    #[live]
    pub long_names: bool,

    #[live(3)]
    pub columns: usize,
    #[live(72.0)]
    pub cell_width: f64,
    #[live(34.0)]
    pub cell_height: f64,
    #[live(32.0)]
    pub header_height: f64,
    #[live(28.0)]
    pub arrow_width: f64,
    #[live(8.0)]
    pub inset: f64,

    #[live]
    pub color_title: Vec4f,
    #[live]
    pub color_arrow: Vec4f,
    #[live]
    pub color_day: Vec4f,
    #[live]
    pub color_off: Vec4f,
    #[live]
    pub color_on_selected: Vec4f,

    #[rust]
    view_year: i32,
    /// Which of the twelve cells the keyboard is on.
    #[rust]
    focus: usize,
    #[rust]
    selection: DaySelection,
    #[rust]
    bounds: DayBounds,
    #[rust]
    hover: Option<usize>,
    #[rust]
    cells: Vec<(Rect, CivilDate)>,
    #[rust]
    prev_rect: Rect,
    #[rust]
    next_rect: Rect,
    #[rust]
    adopted: String,
    #[rust]
    adopted_year: u32,
    #[rust]
    allowed: Option<DayAllowed>,
}

impl ScriptHook for MonthPicker {
    fn on_after_new(&mut self, _vm: &mut ScriptVm) {
        self.selection = DaySelection::new(self.mode);
        self.adopted = self.selected.clone();
        self.selection.set_days(&parse_iso_list(&self.selected));
        self.adopted_year = self.year;
        let anchor = CivilDate::new(self.year as i32, 1, 1)
            .or_else(|| self.selection.days().first().copied())
            .unwrap_or_default();
        self.view_year = anchor.year;
        self.focus = self
            .selection
            .days()
            .first()
            .filter(|date| date.year == self.view_year)
            .map(|date| (date.month.clamp(1, 12) - 1) as usize)
            .unwrap_or(0);
    }
}

impl MonthPicker {
    fn sync(&mut self) {
        if self.selection.mode() != self.mode {
            self.selection = DaySelection::new(self.mode);
            self.adopted.clear();
        }
        if self.adopted != self.selected {
            self.adopted = self.selected.clone();
            self.selection.set_days(&parse_iso_list(&self.selected));
        }
        if self.adopted_year != self.year {
            self.adopted_year = self.year;
            if let Some(date) = CivilDate::new(self.year as i32, 1, 1) {
                self.view_year = date.year;
            }
        }
        self.bounds =
            DayBounds { min: parse_iso(&self.min), max: parse_iso(&self.max), weekdays_off: 0 };
        self.columns = self.columns.clamp(1, 12);
    }

    /// The date a cell stands for: the first of that month. A month is
    /// named by the day it starts on, so everything downstream — bounds,
    /// selection, actions — works on the same type a calendar does.
    fn cell_date(&self, index: usize) -> CivilDate {
        CivilDate { year: self.view_year, month: index as u32 % 12 + 1, day: 1 }
    }

    pub fn allows(&self, date: CivilDate) -> bool {
        self.bounds.allows(date) && self.allowed.as_ref().map(|rule| rule(date)).unwrap_or(true)
    }

    fn label(&self, index: usize) -> String {
        let names = names_or(&self.month_names, &MONTH_NAMES);
        let name = &names[index % 12];
        if self.long_names {
            name.clone()
        } else {
            // Three letters is the shortest form that stays unambiguous in
            // English; two collides on Ja/Ju and Ma.
            name.chars().take(3).collect()
        }
    }

    fn step_year(&mut self, cx: &mut Cx, delta: i32) {
        self.view_year += delta;
        cx.widget_action(
            self.uid,
            CalendarAction::MonthChanged(self.view_year, self.focus as u32 % 12 + 1),
        );
        self.redraw(cx);
    }

    fn choose(&mut self, cx: &mut Cx, index: usize) {
        let date = self.cell_date(index);
        if !self.allows(date) {
            return;
        }
        self.focus = index % 12;
        let uid = self.uid;
        match self.selection.pick(date) {
            Picked::Day(day) | Picked::Cleared(day) => {
                cx.widget_action(uid, CalendarAction::Selected(day));
            }
            Picked::Span(start, end) => {
                cx.widget_action(uid, CalendarAction::RangeSelected(start, end));
            }
            Picked::Waiting(_) => {}
        }
        self.redraw(cx);
    }

    /// Move the focus by `delta` cells, rolling into the year either side
    /// rather than stopping. A grid that stops at December leaves the next
    /// year reachable only through the header.
    fn move_focus_by(&mut self, cx: &mut Cx, delta: i64) {
        let flat = self.view_year as i64 * 12 + self.focus as i64 + delta;
        let year = flat.div_euclid(12) as i32;
        let index = flat.rem_euclid(12) as usize;
        if year != self.view_year {
            self.view_year = year;
            cx.widget_action(self.uid, CalendarAction::MonthChanged(self.view_year, index as u32 + 1));
        }
        self.focus = index;
        self.redraw(cx);
    }

    pub fn selected_months(&self) -> Vec<CivilDate> {
        self.selection.days().to_vec()
    }
}

impl Widget for MonthPicker {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.animator_toggle(cx, disabled, Animate::Yes, ids!(disabled.on), ids!(disabled.off));
    }

    fn disabled(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(disabled.on))
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.sync();
        let cols = self.columns;
        let rows = 12_usize.div_ceil(cols);
        let natural_w = self.inset * 2.0 + cols as f64 * self.cell_width;
        let natural_h = self.inset * 2.0 + self.header_height + rows as f64 * self.cell_height;
        let walk = Walk {
            width: match walk.width {
                Size::Fit { .. } => Size::Fixed(natural_w),
                other => other,
            },
            height: match walk.height {
                Size::Fit { .. } => Size::Fixed(natural_h),
                other => other,
            },
            ..walk
        };

        self.draw_bg.begin(cx, walk, self.layout);
        let frame = cx.turtle().rect();
        let inner = Rect {
            pos: dvec2(frame.pos.x + self.inset, frame.pos.y + self.inset),
            size: dvec2(
                (frame.size.x - self.inset * 2.0).max(1.0),
                (frame.size.y - self.inset * 2.0).max(1.0),
            ),
        };

        self.draw_title.color = self.color_title;
        self.draw_arrow.color = self.color_arrow;
        let title = self.view_year.to_string();
        let band = Rect { pos: inner.pos, size: dvec2(inner.size.x, self.header_height) };
        let (prev, next) = draw_header(
            &mut self.draw_title,
            &mut self.draw_arrow,
            cx,
            band,
            self.arrow_width,
            &title,
        );
        self.prev_rect = prev;
        self.next_rect = next;

        let cell_h = ((inner.size.y - self.header_height) / rows as f64).max(1.0);
        let focused = self.animator_in_state(cx, ids!(focus.on));
        let off = self.animator_in_state(cx, ids!(disabled.on));
        let preview = self.selection.preview(self.hover.map(|index| self.cell_date(index)));

        self.cells.clear();
        let mut looks: Vec<(Rect, usize, CellLook)> = Vec::with_capacity(12);
        for index in 0..12 {
            let row_area = Rect {
                pos: dvec2(
                    inner.pos.x,
                    inner.pos.y + self.header_height + (index / cols) as f64 * cell_h,
                ),
                size: dvec2(inner.size.x, cell_h),
            };
            let rect = grid_cell(row_area, cols, index % cols);
            let date = self.cell_date(index);
            let allowed = !off && self.allows(date);
            let selected = self.selection.contains(date);
            let mut look = CellLook {
                selected,
                focus: focused && self.focus == index,
                hover: allowed && self.hover == Some(index) && !selected,
                off: !allowed,
                ..CellLook::default()
            };
            if let Some((start, end)) = preview {
                if start < end && date >= start && date <= end {
                    look.span = true;
                    look.span_end = if date == start {
                        -1.0
                    } else if date == end {
                        1.0
                    } else {
                        0.0
                    };
                    look.selected = date == start || date == end;
                }
            }
            self.cells.push((rect, date));
            looks.push((rect, index, look));
        }

        for (rect, _, look) in &looks {
            self.draw_cell.selected = if look.selected { 1.0 } else { 0.0 };
            self.draw_cell.span = if look.span { 1.0 } else { 0.0 };
            self.draw_cell.span_end = look.span_end;
            self.draw_cell.today = 0.0;
            self.draw_cell.focus = if look.focus { 1.0 } else { 0.0 };
            self.draw_cell.hover = if look.hover { 1.0 } else { 0.0 };
            self.draw_cell.draw_abs(cx, *rect);
        }
        for (rect, index, look) in &looks {
            self.draw_day.color = if look.off {
                self.color_off
            } else if look.selected {
                self.color_on_selected
            } else {
                self.color_day
            };
            let label = self.label(*index);
            draw_centered(&mut self.draw_day, cx, *rect, &label);
        }

        self.draw_bg.end(cx);
        if !off {
            cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.animator_handle_event(cx, event);
        if self.animator_in_state(cx, ids!(disabled.on)) {
            return;
        }
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let at = self
                    .cells
                    .iter()
                    .position(|(rect, date)| rect.contains(fe.abs) && self.allows(*date));
                let on_step = self.prev_rect.contains(fe.abs) || self.next_rect.contains(fe.abs);
                cx.set_cursor(if at.is_some() || on_step {
                    MouseCursor::Hand
                } else {
                    MouseCursor::Default
                });
                if at != self.hover {
                    self.hover = at;
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.redraw(cx);
                }
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                if self.prev_rect.contains(fe.abs) {
                    self.step_year(cx, -1);
                    return;
                }
                if self.next_rect.contains(fe.abs) {
                    self.step_year(cx, 1);
                    return;
                }
                let hit = self.cells.iter().position(|(rect, _)| rect.contains(fe.abs));
                if let Some(index) = hit {
                    self.choose(cx, index);
                }
            }
            Hit::KeyFocus(_) => {
                self.animator_play(cx, ids!(focus.on));
                self.redraw(cx);
            }
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, ids!(focus.off));
                self.redraw(cx);
            }
            Hit::KeyDown(ke) => {
                let cols = self.columns.clamp(1, 12) as i64;
                match ke.key_code {
                    KeyCode::ArrowLeft => self.move_focus_by(cx, -1),
                    KeyCode::ArrowRight => self.move_focus_by(cx, 1),
                    KeyCode::ArrowUp => self.move_focus_by(cx, -cols),
                    KeyCode::ArrowDown => self.move_focus_by(cx, cols),
                    KeyCode::PageUp => self.move_focus_by(cx, -12),
                    KeyCode::PageDown => self.move_focus_by(cx, 12),
                    KeyCode::Home => {
                        self.focus = 0;
                        self.redraw(cx);
                    }
                    KeyCode::End => {
                        self.focus = 11;
                        self.redraw(cx);
                    }
                    KeyCode::ReturnKey | KeyCode::Space => {
                        let focus = self.focus;
                        self.choose(cx, focus);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn text(&self) -> String {
        format!("{} ({} chosen)", self.view_year, self.selection.days().len())
    }
}

impl MonthPickerRef {
    pub fn selected_months(&self) -> Vec<CivilDate> {
        self.borrow().map(|inner| inner.selected_months()).unwrap_or_default()
    }

    pub fn selected_month(&self) -> Option<CivilDate> {
        self.borrow().and_then(|inner| inner.selected_months().first().copied())
    }

    pub fn set_day_allowed(&self, rule: DayAllowed) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.allowed = Some(rule);
        }
    }

    pub fn selected(&self, actions: &Actions) -> Option<CivilDate> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            CalendarAction::Selected(date) => Some(date),
            _ => None,
        }
    }

    pub fn range_selected(&self, actions: &Actions) -> Option<(CivilDate, CivilDate)> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            CalendarAction::RangeSelected(start, end) => Some((start, end)),
            _ => None,
        }
    }
}

#[derive(Script, Widget, Animator)]
pub struct YearPicker {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_bg: DrawCalendarFrame,
    #[live]
    draw_cell: DrawCalendarCell,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_arrow: DrawText,
    #[live]
    draw_day: DrawText,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[apply_default]
    animator: Animator,

    #[live]
    pub mode: CalendarMode,
    #[live]
    pub year: u32,
    #[live]
    pub selected: String,
    #[live]
    pub min: String,
    #[live]
    pub max: String,

    #[live(3)]
    pub columns: usize,
    #[live(72.0)]
    pub cell_width: f64,
    #[live(34.0)]
    pub cell_height: f64,
    #[live(32.0)]
    pub header_height: f64,
    #[live(28.0)]
    pub arrow_width: f64,
    #[live(8.0)]
    pub inset: f64,

    #[live]
    pub color_title: Vec4f,
    #[live]
    pub color_arrow: Vec4f,
    #[live]
    pub color_day: Vec4f,
    #[live]
    pub color_off: Vec4f,
    #[live]
    pub color_on_selected: Vec4f,

    /// The first year of the page on show.
    #[rust]
    page: i32,
    #[rust]
    focus: usize,
    #[rust]
    selection: DaySelection,
    #[rust]
    bounds: DayBounds,
    #[rust]
    hover: Option<usize>,
    #[rust]
    cells: Vec<(Rect, CivilDate)>,
    #[rust]
    prev_rect: Rect,
    #[rust]
    next_rect: Rect,
    #[rust]
    adopted: String,
    #[rust]
    adopted_year: u32,
    #[rust]
    allowed: Option<DayAllowed>,
}

impl ScriptHook for YearPicker {
    fn on_after_new(&mut self, _vm: &mut ScriptVm) {
        self.selection = DaySelection::new(self.mode);
        self.adopted = self.selected.clone();
        self.selection.set_days(&parse_iso_list(&self.selected));
        self.adopted_year = self.year;
        let anchor = CivilDate::new(self.year as i32, 1, 1)
            .or_else(|| self.selection.days().first().copied())
            .unwrap_or_default();
        self.page = year_page(anchor.year);
        self.focus = (anchor.year - self.page).clamp(0, 11) as usize;
    }
}

impl YearPicker {
    fn sync(&mut self) {
        if self.selection.mode() != self.mode {
            self.selection = DaySelection::new(self.mode);
            self.adopted.clear();
        }
        if self.adopted != self.selected {
            self.adopted = self.selected.clone();
            self.selection.set_days(&parse_iso_list(&self.selected));
        }
        if self.adopted_year != self.year {
            self.adopted_year = self.year;
            if let Some(date) = CivilDate::new(self.year as i32, 1, 1) {
                self.page = year_page(date.year);
                self.focus = (date.year - self.page).clamp(0, 11) as usize;
            }
        }
        self.bounds =
            DayBounds { min: parse_iso(&self.min), max: parse_iso(&self.max), weekdays_off: 0 };
        self.columns = self.columns.clamp(1, 12);
    }

    /// A year is named by the day it starts on, for the same reason a month
    /// is: one date type through the whole family.
    fn cell_date(&self, index: usize) -> CivilDate {
        CivilDate { year: self.page + index as i32, month: 1, day: 1 }
    }

    pub fn allows(&self, date: CivilDate) -> bool {
        self.bounds.allows(date) && self.allowed.as_ref().map(|rule| rule(date)).unwrap_or(true)
    }

    fn step_page(&mut self, cx: &mut Cx, delta: i32) {
        self.page += delta * 12;
        cx.widget_action(self.uid, CalendarAction::MonthChanged(self.page, 1));
        self.redraw(cx);
    }

    fn choose(&mut self, cx: &mut Cx, index: usize) {
        let date = self.cell_date(index);
        if !self.allows(date) {
            return;
        }
        self.focus = index.min(11);
        let uid = self.uid;
        match self.selection.pick(date) {
            Picked::Day(day) | Picked::Cleared(day) => {
                cx.widget_action(uid, CalendarAction::Selected(day));
            }
            Picked::Span(start, end) => {
                cx.widget_action(uid, CalendarAction::RangeSelected(start, end));
            }
            Picked::Waiting(_) => {}
        }
        self.redraw(cx);
    }

    fn move_focus_by(&mut self, cx: &mut Cx, delta: i64) {
        let flat = self.page as i64 + self.focus as i64 + delta;
        let page = year_page(flat as i32);
        if page != self.page {
            self.page = page;
            cx.widget_action(self.uid, CalendarAction::MonthChanged(self.page, 1));
        }
        self.focus = (flat as i32 - self.page).clamp(0, 11) as usize;
        self.redraw(cx);
    }

    pub fn selected_years(&self) -> Vec<CivilDate> {
        self.selection.days().to_vec()
    }
}

impl Widget for YearPicker {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.animator_toggle(cx, disabled, Animate::Yes, ids!(disabled.on), ids!(disabled.off));
    }

    fn disabled(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(disabled.on))
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.sync();
        let cols = self.columns;
        let rows = 12_usize.div_ceil(cols);
        let natural_w = self.inset * 2.0 + cols as f64 * self.cell_width;
        let natural_h = self.inset * 2.0 + self.header_height + rows as f64 * self.cell_height;
        let walk = Walk {
            width: match walk.width {
                Size::Fit { .. } => Size::Fixed(natural_w),
                other => other,
            },
            height: match walk.height {
                Size::Fit { .. } => Size::Fixed(natural_h),
                other => other,
            },
            ..walk
        };

        self.draw_bg.begin(cx, walk, self.layout);
        let frame = cx.turtle().rect();
        let inner = Rect {
            pos: dvec2(frame.pos.x + self.inset, frame.pos.y + self.inset),
            size: dvec2(
                (frame.size.x - self.inset * 2.0).max(1.0),
                (frame.size.y - self.inset * 2.0).max(1.0),
            ),
        };

        self.draw_title.color = self.color_title;
        self.draw_arrow.color = self.color_arrow;
        let title = format!("{} \u{2013} {}", self.page, self.page + 11);
        let band = Rect { pos: inner.pos, size: dvec2(inner.size.x, self.header_height) };
        let (prev, next) = draw_header(
            &mut self.draw_title,
            &mut self.draw_arrow,
            cx,
            band,
            self.arrow_width,
            &title,
        );
        self.prev_rect = prev;
        self.next_rect = next;

        let cell_h = ((inner.size.y - self.header_height) / rows as f64).max(1.0);
        let focused = self.animator_in_state(cx, ids!(focus.on));
        let off = self.animator_in_state(cx, ids!(disabled.on));
        let preview = self.selection.preview(self.hover.map(|index| self.cell_date(index)));

        self.cells.clear();
        let mut looks: Vec<(Rect, usize, CellLook)> = Vec::with_capacity(12);
        for index in 0..12 {
            let row_area = Rect {
                pos: dvec2(
                    inner.pos.x,
                    inner.pos.y + self.header_height + (index / cols) as f64 * cell_h,
                ),
                size: dvec2(inner.size.x, cell_h),
            };
            let rect = grid_cell(row_area, cols, index % cols);
            let date = self.cell_date(index);
            let allowed = !off && self.allows(date);
            let selected = self.selection.contains(date);
            let mut look = CellLook {
                selected,
                focus: focused && self.focus == index,
                hover: allowed && self.hover == Some(index) && !selected,
                off: !allowed,
                ..CellLook::default()
            };
            if let Some((start, end)) = preview {
                if start < end && date >= start && date <= end {
                    look.span = true;
                    look.span_end = if date == start {
                        -1.0
                    } else if date == end {
                        1.0
                    } else {
                        0.0
                    };
                    look.selected = date == start || date == end;
                }
            }
            self.cells.push((rect, date));
            looks.push((rect, index, look));
        }

        for (rect, _, look) in &looks {
            self.draw_cell.selected = if look.selected { 1.0 } else { 0.0 };
            self.draw_cell.span = if look.span { 1.0 } else { 0.0 };
            self.draw_cell.span_end = look.span_end;
            self.draw_cell.today = 0.0;
            self.draw_cell.focus = if look.focus { 1.0 } else { 0.0 };
            self.draw_cell.hover = if look.hover { 1.0 } else { 0.0 };
            self.draw_cell.draw_abs(cx, *rect);
        }
        for (rect, index, look) in &looks {
            self.draw_day.color = if look.off {
                self.color_off
            } else if look.selected {
                self.color_on_selected
            } else {
                self.color_day
            };
            let label = (self.page + *index as i32).to_string();
            draw_centered(&mut self.draw_day, cx, *rect, &label);
        }

        self.draw_bg.end(cx);
        if !off {
            cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.animator_handle_event(cx, event);
        if self.animator_in_state(cx, ids!(disabled.on)) {
            return;
        }
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let at = self
                    .cells
                    .iter()
                    .position(|(rect, date)| rect.contains(fe.abs) && self.allows(*date));
                let on_step = self.prev_rect.contains(fe.abs) || self.next_rect.contains(fe.abs);
                cx.set_cursor(if at.is_some() || on_step {
                    MouseCursor::Hand
                } else {
                    MouseCursor::Default
                });
                if at != self.hover {
                    self.hover = at;
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.redraw(cx);
                }
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                if self.prev_rect.contains(fe.abs) {
                    self.step_page(cx, -1);
                    return;
                }
                if self.next_rect.contains(fe.abs) {
                    self.step_page(cx, 1);
                    return;
                }
                let hit = self.cells.iter().position(|(rect, _)| rect.contains(fe.abs));
                if let Some(index) = hit {
                    self.choose(cx, index);
                }
            }
            Hit::KeyFocus(_) => {
                self.animator_play(cx, ids!(focus.on));
                self.redraw(cx);
            }
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, ids!(focus.off));
                self.redraw(cx);
            }
            Hit::KeyDown(ke) => {
                let cols = self.columns.clamp(1, 12) as i64;
                match ke.key_code {
                    KeyCode::ArrowLeft => self.move_focus_by(cx, -1),
                    KeyCode::ArrowRight => self.move_focus_by(cx, 1),
                    KeyCode::ArrowUp => self.move_focus_by(cx, -cols),
                    KeyCode::ArrowDown => self.move_focus_by(cx, cols),
                    KeyCode::PageUp => self.move_focus_by(cx, -12),
                    KeyCode::PageDown => self.move_focus_by(cx, 12),
                    KeyCode::Home => {
                        self.focus = 0;
                        self.redraw(cx);
                    }
                    KeyCode::End => {
                        self.focus = 11;
                        self.redraw(cx);
                    }
                    KeyCode::ReturnKey | KeyCode::Space => {
                        let focus = self.focus;
                        self.choose(cx, focus);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn text(&self) -> String {
        format!(
            "{} \u{2013} {} ({} chosen)",
            self.page,
            self.page + 11,
            self.selection.days().len()
        )
    }
}

impl YearPickerRef {
    pub fn selected_years(&self) -> Vec<CivilDate> {
        self.borrow().map(|inner| inner.selected_years()).unwrap_or_default()
    }

    pub fn selected_year(&self) -> Option<i32> {
        self.borrow()
            .and_then(|inner| inner.selected_years().first().map(|date| date.year))
    }

    pub fn set_day_allowed(&self, rule: DayAllowed) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.allowed = Some(rule);
        }
    }

    pub fn selected(&self, actions: &Actions) -> Option<CivilDate> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            CalendarAction::Selected(date) => Some(date),
            _ => None,
        }
    }

    pub fn range_selected(&self, actions: &Actions) -> Option<(CivilDate, CivilDate)> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            CalendarAction::RangeSelected(start, end) => Some((start, end)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> CivilDate {
        CivilDate::new(year, month, day).expect("a real date")
    }

    #[test]
    fn the_epoch_is_day_zero_and_a_thursday() {
        assert_eq!(date(1970, 1, 1).to_days(), 0);
        assert_eq!(date(1970, 1, 1).weekday(), 3);
        assert_eq!(CivilDate::from_days(0), date(1970, 1, 1));
    }

    /// Days out and dates back, every day for two centuries. This is the
    /// one property the whole family rests on: if the two directions
    /// disagree anywhere, every arrow key and every span is wrong there and
    /// nowhere else, which is the worst kind of wrong to find by hand.
    #[test]
    fn days_and_dates_round_trip_across_two_centuries() {
        let start = date(1900, 1, 1).to_days();
        let end = date(2100, 12, 31).to_days();
        let mut days = start;
        while days <= end {
            let back = CivilDate::from_days(days);
            assert_eq!(back.to_days(), days, "{back:?} did not survive the round trip");
            assert!((1..=12).contains(&back.month));
            assert!(back.day >= 1 && back.day <= CivilDate::month_length(back.year, back.month));
            days += 1;
        }
    }

    /// And the weekday advances by exactly one across each of those days,
    /// which is the other half of the same property.
    #[test]
    fn the_weekday_advances_one_day_at_a_time() {
        let mut at = date(1999, 12, 25);
        for _ in 0..4000 {
            let next = at.add_days(1);
            assert_eq!(next.weekday(), (at.weekday() + 1) % 7);
            at = next;
        }
    }

    #[test]
    fn known_weekdays() {
        // 0 is Monday.
        assert_eq!(date(2000, 1, 1).weekday(), 5, "a Saturday");
        assert_eq!(date(2000, 2, 29).weekday(), 1, "a Tuesday");
        assert_eq!(date(2026, 9, 10).weekday(), 3, "a Thursday");
        assert_eq!(date(1900, 1, 1).weekday(), 0, "a Monday");
    }

    #[test]
    fn leap_years_follow_all_three_rules() {
        assert!(CivilDate::is_leap(2024));
        assert!(!CivilDate::is_leap(2023));
        assert!(!CivilDate::is_leap(1900), "a century is not enough");
        assert!(CivilDate::is_leap(2000), "four centuries are");
        assert_eq!(CivilDate::month_length(2024, 2), 29);
        assert_eq!(CivilDate::month_length(1900, 2), 28);
        assert!(CivilDate::new(2023, 2, 29).is_none());
        assert!(CivilDate::new(2024, 2, 29).is_some());
    }

    #[test]
    fn a_month_that_does_not_exist_has_no_days_and_no_date() {
        assert_eq!(CivilDate::month_length(2026, 0), 0);
        assert_eq!(CivilDate::month_length(2026, 13), 0);
        assert!(CivilDate::new(2026, 13, 1).is_none());
        assert!(CivilDate::new(2026, 4, 31).is_none());
        assert!(CivilDate::new(2026, 1, 0).is_none());
    }

    #[test]
    fn adding_months_clamps_the_day_to_the_month_it_lands_in() {
        assert_eq!(date(2026, 1, 31).add_months(1), date(2026, 2, 28));
        assert_eq!(date(2024, 1, 31).add_months(1), date(2024, 2, 29));
        assert_eq!(date(2026, 3, 31).add_months(-1), date(2026, 2, 28));
        assert_eq!(date(2026, 5, 31).add_months(1), date(2026, 6, 30));
    }

    /// The clamp is not reversible, and it is worth a test saying so:
    /// somebody will otherwise try to make month stepping round trip.
    #[test]
    fn a_clamped_month_step_does_not_come_back() {
        assert_eq!(date(2026, 1, 31).add_months(1).add_months(-1), date(2026, 1, 28));
    }

    #[test]
    fn adding_months_crosses_years_in_both_directions() {
        assert_eq!(date(2026, 12, 15).add_months(1), date(2027, 1, 15));
        assert_eq!(date(2026, 1, 15).add_months(-1), date(2025, 12, 15));
        assert_eq!(date(2026, 6, 15).add_months(-30), date(2023, 12, 15));
        assert_eq!(date(2026, 6, 15).add_months(30), date(2028, 12, 15));
    }

    #[test]
    fn iso_weeks_at_the_edges_of_a_year() {
        // A year that starts on a Thursday starts in its own week 1.
        assert_eq!(date(2026, 1, 1).iso_week(), (2026, 1));
        // A Friday the 1st belongs to the last week of the year before.
        assert_eq!(date(2021, 1, 1).iso_week(), (2020, 53));
        assert_eq!(date(2020, 12, 31).iso_week(), (2020, 53));
        // A Monday in late December can already be week 1 of the next year.
        assert_eq!(date(2019, 12, 30).iso_week(), (2020, 1));
        assert_eq!(date(2018, 12, 31).iso_week(), (2019, 1));
        // And the ordinary middle of a year.
        assert_eq!(date(2026, 9, 10).iso_week(), (2026, 37));
    }

    #[test]
    fn every_iso_week_is_in_range_and_holds_for_a_whole_week() {
        let mut at = date(1995, 1, 1);
        let end = date(2035, 1, 1);
        while at < end {
            let (_, week) = at.iso_week();
            assert!((1..=53).contains(&week), "{at:?} reported week {week}");
            // Every day of a week reports the same week as its Monday.
            let monday = at.add_days(-(at.weekday() as i64));
            assert_eq!(at.iso_week(), monday.iso_week());
            at = at.add_days(1);
        }
    }

    #[test]
    fn a_month_grid_starts_on_the_weekday_it_was_told_to() {
        let grid = MonthGrid { year: 2026, month: 9, first_day: 0 };
        // September 2026 starts on a Tuesday, so one Monday leads it.
        assert_eq!(grid.lead(), 1);
        assert_eq!(grid.cell(0), date(2026, 8, 31));
        assert_eq!(grid.cell(1), date(2026, 9, 1));
        assert!(!grid.owns(grid.cell(0)));
        assert!(grid.owns(grid.cell(1)));

        let sunday_first = MonthGrid { year: 2026, month: 9, first_day: 6 };
        assert_eq!(sunday_first.lead(), 2);
        assert_eq!(sunday_first.cell(0), date(2026, 8, 30));
    }

    #[test]
    fn a_month_grid_is_six_rows_whatever_the_month() {
        // February 2027 starts on a Monday and has 28 days: it fits in four
        // rows, and is still drawn on six so the control holds still.
        let grid = MonthGrid { year: 2027, month: 2, first_day: 0 };
        assert_eq!(grid.lead(), 0);
        assert_eq!(grid.cell(0), date(2027, 2, 1));
        assert_eq!(grid.cell(GRID_CELLS - 1), date(2027, 3, 14));
        assert_eq!(grid.index_of(date(2027, 2, 1)), Some(0));
        assert_eq!(grid.index_of(date(2027, 1, 31)), None);
    }

    #[test]
    fn bounds_close_the_ends_and_the_weekends() {
        let bounds = DayBounds {
            min: Some(date(2026, 9, 5)),
            max: Some(date(2026, 9, 20)),
            weekdays_off: (1 << 5) | (1 << 6),
        };
        assert!(!bounds.allows(date(2026, 9, 4)), "before the first day");
        assert!(!bounds.allows(date(2026, 9, 21)), "after the last");
        assert!(!bounds.allows(date(2026, 9, 12)), "a Saturday");
        assert!(!bounds.allows(date(2026, 9, 13)), "a Sunday");
        assert!(bounds.allows(date(2026, 9, 14)), "the Monday between them");
        // The 5th is both the earliest day and a Saturday; the weekday mask
        // closes it even though the bound opens it.
        assert!(!bounds.allows(date(2026, 9, 5)));
    }

    #[test]
    fn seeking_finds_the_next_open_day_and_gives_up_on_a_closed_calendar() {
        let weekdays_only = |day: CivilDate| day.weekday() < 5;
        // Saturday the 12th forwards is Monday the 14th.
        assert_eq!(
            seek(date(2026, 9, 12), 1, SEEK_LIMIT, weekdays_only),
            Some(date(2026, 9, 14))
        );
        // And backwards it is Friday the 11th.
        assert_eq!(
            seek(date(2026, 9, 12), -1, SEEK_LIMIT, weekdays_only),
            Some(date(2026, 9, 11))
        );
        assert_eq!(seek(date(2026, 9, 12), 1, SEEK_LIMIT, |_| false), None);
    }

    #[test]
    fn single_selection_replaces_rather_than_grows() {
        let mut sel = DaySelection::new(CalendarMode::Single);
        assert_eq!(sel.pick(date(2026, 9, 3)), Picked::Day(date(2026, 9, 3)));
        assert_eq!(sel.pick(date(2026, 9, 7)), Picked::Day(date(2026, 9, 7)));
        assert_eq!(sel.days(), &[date(2026, 9, 7)]);
    }

    #[test]
    fn multiple_selection_toggles_and_stays_in_order() {
        let mut sel = DaySelection::new(CalendarMode::Multiple);
        sel.pick(date(2026, 9, 7));
        sel.pick(date(2026, 9, 3));
        assert_eq!(sel.days(), &[date(2026, 9, 3), date(2026, 9, 7)]);
        assert_eq!(sel.pick(date(2026, 9, 3)), Picked::Cleared(date(2026, 9, 3)));
        assert_eq!(sel.days(), &[date(2026, 9, 7)]);
    }

    #[test]
    fn a_range_takes_two_presses_and_reports_nothing_after_one() {
        let mut sel = DaySelection::new(CalendarMode::Range);
        assert_eq!(sel.pick(date(2026, 9, 10)), Picked::Waiting(date(2026, 9, 10)));
        assert_eq!(sel.span(), None, "half a range is not an answer");
        assert_eq!(
            sel.pick(date(2026, 9, 20)),
            Picked::Span(date(2026, 9, 10), date(2026, 9, 20))
        );
        assert_eq!(sel.span(), Some((date(2026, 9, 10), date(2026, 9, 20))));
    }

    /// The second press may be before the first. The span still comes out
    /// earliest first, so a host never has to sort the two ends.
    #[test]
    fn a_range_chosen_backwards_still_comes_out_in_order() {
        let mut sel = DaySelection::new(CalendarMode::Range);
        sel.pick(date(2026, 9, 20));
        assert_eq!(
            sel.pick(date(2026, 9, 10)),
            Picked::Span(date(2026, 9, 10), date(2026, 9, 20))
        );
    }

    #[test]
    fn a_third_press_starts_a_new_range() {
        let mut sel = DaySelection::new(CalendarMode::Range);
        sel.pick(date(2026, 9, 10));
        sel.pick(date(2026, 9, 20));
        assert_eq!(sel.pick(date(2026, 9, 25)), Picked::Waiting(date(2026, 9, 25)));
        assert_eq!(sel.span(), None);
    }

    #[test]
    fn a_half_made_range_previews_against_the_pointer() {
        let mut sel = DaySelection::new(CalendarMode::Range);
        sel.pick(date(2026, 9, 10));
        assert_eq!(
            sel.preview(Some(date(2026, 9, 4))),
            Some((date(2026, 9, 4), date(2026, 9, 10))),
            "the preview runs the other way when the pointer is before the anchor"
        );
        assert_eq!(sel.preview(None), None);
    }

    #[test]
    fn given_days_are_trimmed_to_what_the_mode_can_hold() {
        let days = [date(2026, 9, 20), date(2026, 9, 3), date(2026, 9, 10)];

        let mut single = DaySelection::new(CalendarMode::Single);
        single.set_days(&days);
        assert_eq!(single.days(), &[date(2026, 9, 3)], "the earliest, and only it");

        let mut many = DaySelection::new(CalendarMode::Multiple);
        many.set_days(&days);
        assert_eq!(many.days().len(), 3);

        let mut range = DaySelection::new(CalendarMode::Range);
        range.set_days(&days);
        assert_eq!(range.days(), &[date(2026, 9, 3), date(2026, 9, 20)], "the two ends");

        // One day given to a range is a range half made, so the next press
        // finishes it rather than throwing it away.
        let mut half = DaySelection::new(CalendarMode::Range);
        half.set_days(&[date(2026, 9, 3)]);
        assert_eq!(half.anchor(), Some(date(2026, 9, 3)));
        assert_eq!(
            half.pick(date(2026, 9, 9)),
            Picked::Span(date(2026, 9, 3), date(2026, 9, 9))
        );
    }

    #[test]
    fn the_keyboard_walks_out_of_the_drawn_month() {
        let at = date(2026, 9, 30);
        assert_eq!(move_focus(at, 0, Motion::NextDay), date(2026, 10, 1));
        assert_eq!(move_focus(at, 0, Motion::NextWeek), date(2026, 10, 7));
        assert_eq!(move_focus(date(2026, 9, 1), 0, Motion::PrevDay), date(2026, 8, 31));
    }

    #[test]
    fn home_and_end_are_the_ends_of_the_drawn_row_not_the_month() {
        // Thursday 10 September 2026, in a Monday-first grid.
        let at = date(2026, 9, 10);
        assert_eq!(move_focus(at, 0, Motion::WeekStart), date(2026, 9, 7));
        assert_eq!(move_focus(at, 0, Motion::WeekEnd), date(2026, 9, 13));
        // The same day in a Sunday-first grid sits in a row shifted by one.
        assert_eq!(move_focus(at, 6, Motion::WeekStart), date(2026, 9, 6));
        assert_eq!(move_focus(at, 6, Motion::WeekEnd), date(2026, 9, 12));
    }

    #[test]
    fn a_month_step_from_the_keyboard_clamps_the_way_the_arrow_does() {
        assert_eq!(move_focus(date(2026, 1, 31), 0, Motion::NextMonth), date(2026, 2, 28));
        assert_eq!(move_focus(date(2026, 3, 31), 0, Motion::PrevMonth), date(2026, 2, 28));
    }

    #[test]
    fn year_pages_are_aligned_so_stepping_is_reversible() {
        assert_eq!(year_page(2026), 2016);
        assert_eq!(year_page(2016), 2016);
        assert_eq!(year_page(2015), 2004);
        // Every year in a page maps to the same page, which is what makes
        // the two header arrows inverses of each other.
        for offset in 0..12 {
            assert_eq!(year_page(2016 + offset), 2016);
        }
        assert_eq!(year_page(-1), -12);
    }

    #[test]
    fn iso_text_is_read_strictly_and_written_back_the_same_way() {
        assert_eq!(parse_iso("2026-09-10"), Some(date(2026, 9, 10)));
        assert_eq!(parse_iso("  2026-09-10 "), Some(date(2026, 9, 10)));
        assert_eq!(parse_iso("2026-9-10"), None, "the short form is refused");
        assert_eq!(parse_iso("2026-02-30"), None, "and so is a day that does not exist");
        assert_eq!(parse_iso(""), None);
        assert_eq!(format_iso(date(2026, 9, 10)), "2026-09-10");
        assert_eq!(format_iso(date(7, 1, 2)), "0007-01-02");
    }

    #[test]
    fn a_list_of_dates_drops_only_the_parts_it_cannot_read() {
        assert_eq!(
            parse_iso_list("2026-09-03, 2026-09-07 ,nonsense"),
            vec![date(2026, 9, 3), date(2026, 9, 7)]
        );
        assert!(parse_iso_list("").is_empty());
    }

    #[test]
    fn names_of_the_wrong_length_are_refused_rather_than_used_short() {
        assert_eq!(names_or("", &MONTH_NAMES)[0], "January");
        // Eleven names would shift half the year by one; take none of them.
        assert_eq!(names_or("a,b,c,d,e,f,g,h,i,j,k", &MONTH_NAMES)[0], "January");
        assert_eq!(names_or("a,b,c,d,e,f,g,h,i,j,k,l", &MONTH_NAMES)[11], "l");
        assert_eq!(names_or("", &WEEKDAY_NAMES)[0], "Mo");
    }
}

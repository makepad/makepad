//! Document types, layout decision, local clock, and validation.

use makepad_civil_time::{self as civil, Day};
use std::cmp::Ordering;

pub const SCHEMA_VERSION: u32 = 1;
pub const SEED_VERSION: u32 = 1;
pub const MAX_TITLE_SCALARS: usize = 240;
pub const MAX_NOTES_SCALARS: usize = 4096;
pub const MAX_MASTERS: usize = 1000;
pub const MAX_DURATION_DAYS: i32 = 366;
pub const MAX_QUERY_DAYS: i32 = 366;
pub const UI_EXPAND_CAP: usize = 4096;
pub const SEARCH_LIMIT: usize = 100;
pub const TOOL_OCCURRENCE_LIMIT: usize = 200;
pub const TOOL_BYTE_LIMIT: usize = 64 * 1024;
pub const SEARCH_BACK_DAYS: i32 = 90;
pub const SEARCH_FORWARD_DAYS: i32 = 276;
pub const LAYOUT_COMPACT_BELOW: f64 = 700.0;
pub const LAYOUT_MID_BELOW: f64 = 1040.0;
pub const SHORT_HEIGHT_BELOW: f64 = 420.0;
pub const MINUTES_PER_DAY: u16 = 1440;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CalendarId(pub u8);

pub const CAL_HOME: CalendarId = CalendarId(1);
pub const CAL_WORK: CalendarId = CalendarId(2);
pub const CAL_BIRTHDAYS: CalendarId = CalendarId(3);
pub const CAL_HOLIDAYS: CalendarId = CalendarId(4);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalendarColour {
    Home,
    Work,
    Birthdays,
    Holidays,
}

impl CalendarColour {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Work => "work",
            Self::Birthdays => "birthdays",
            Self::Holidays => "holidays",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "home" => Some(Self::Home),
            "work" => Some(Self::Work),
            "birthdays" => Some(Self::Birthdays),
            "holidays" => Some(Self::Holidays),
            _ => None,
        }
    }

    /// Light-theme calendar colour, then dark-theme colour, as 0xRRGGBB.
    pub fn rgb_pair(self) -> (u32, u32) {
        match self {
            Self::Home => (0x3478C5, 0x64A8F5),
            Self::Work => (0x8552B8, 0xB68CDF),
            Self::Birthdays => (0x2D8556, 0x71C698),
            Self::Holidays => (0xA76524, 0xE3A566),
        }
    }

    pub fn rgb(self, dark: bool) -> u32 {
        let (light, dark_rgb) = self.rgb_pair();
        if dark {
            dark_rgb
        } else {
            light
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Calendar {
    pub id: CalendarId,
    pub name: String,
    pub colour: CalendarColour,
    pub visible: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalMinute {
    pub day: Day,
    pub minute: u16,
}

impl LocalMinute {
    pub fn new(day: Day, minute: u16) -> Option<Self> {
        if minute < MINUTES_PER_DAY {
            Some(Self { day, minute })
        } else {
            None
        }
    }

    pub fn from_hm(day: Day, hour: u32, minute: u32) -> Option<Self> {
        if hour > 23 || minute > 59 {
            return None;
        }
        Self::new(day, (hour * 60 + minute) as u16)
    }

    pub fn hour(self) -> u32 {
        self.minute as u32 / 60
    }

    pub fn minute_of_hour(self) -> u32 {
        self.minute as u32 % 60
    }

    pub fn add_minutes(self, minutes: i32) -> Option<Self> {
        let abs = self.day as i64 * MINUTES_PER_DAY as i64 + self.minute as i64 + minutes as i64;
        let day = abs.div_euclid(MINUTES_PER_DAY as i64);
        let minute = abs.rem_euclid(MINUTES_PER_DAY as i64) as u16;
        if day < i32::MIN as i64 || day > i32::MAX as i64 {
            return None;
        }
        Some(Self {
            day: day as Day,
            minute,
        })
    }

    pub fn abs_minutes(self) -> i64 {
        self.day as i64 * MINUTES_PER_DAY as i64 + self.minute as i64
    }

    pub fn format_hm(self) -> String {
        format!("{:02}:{:02}", self.hour(), self.minute_of_hour())
    }

    pub fn format_iso(self) -> String {
        format!("{}T{}", civil::format_iso(self.day), self.format_hm())
    }

    pub fn parse_iso(text: &str) -> Option<Self> {
        let text = text.trim();
        if !text.is_ascii() || text.len() != 16 || text.as_bytes()[10] != b'T' || text.as_bytes()[13] != b':' {
            return None;
        }
        if text.contains('Z') || text.contains('+') || text.ends_with('Z') {
            return None;
        }
        let day = civil::parse_iso(&text[..10])?;
        let hour: u32 = text[11..13].parse().ok()?;
        let minute: u32 = text[14..16].parse().ok()?;
        if !text[11..13].bytes().all(|b| b.is_ascii_digit())
            || !text[14..16].bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        Self::from_hm(day, hour, minute)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timing {
    Timed {
        start: LocalMinute,
        end: LocalMinute,
    },
    AllDay {
        start: Day,
        end_exclusive: Day,
    },
}

impl Timing {
    pub fn start_day(self) -> Day {
        match self {
            Self::Timed { start, .. } => start.day,
            Self::AllDay { start, .. } => start,
        }
    }

    pub fn end_day_exclusive(self) -> Day {
        match self {
            Self::Timed { end, .. } => {
                if end.minute == 0 {
                    end.day
                } else {
                    end.day + 1
                }
            }
            Self::AllDay { end_exclusive, .. } => end_exclusive,
        }
    }

    pub fn duration_minutes(self) -> Option<i32> {
        match self {
            Self::Timed { start, end } => {
                let d = end.abs_minutes() - start.abs_minutes();
                if d > 0 && d <= MAX_DURATION_DAYS as i64 * MINUTES_PER_DAY as i64 {
                    Some(d as i32)
                } else {
                    None
                }
            }
            Self::AllDay { .. } => None,
        }
    }

    pub fn duration_days(self) -> Option<i32> {
        match self {
            Self::AllDay {
                start,
                end_exclusive,
            } => {
                let d = end_exclusive - start;
                if d > 0 && d <= MAX_DURATION_DAYS {
                    Some(d)
                } else {
                    None
                }
            }
            Self::Timed { .. } => None,
        }
    }

    /// Half-open interval intersection with `[day, day+1)`.
    pub fn intersects_day(self, day: Day) -> bool {
        self.intersects_range(day, day + 1)
    }

    pub fn intersects_range(self, start: Day, end_exclusive: Day) -> bool {
        match self {
            Self::Timed { start: s, end: e } => {
                let a = s.abs_minutes();
                let b = e.abs_minutes();
                let rs = start as i64 * MINUTES_PER_DAY as i64;
                let re = end_exclusive as i64 * MINUTES_PER_DAY as i64;
                a < re && b > rs
            }
            Self::AllDay {
                start: s,
                end_exclusive: e,
            } => s < end_exclusive && e > start,
        }
    }

    pub fn shift_start_keep_duration(
        self,
        new_start_day: Day,
        new_minute: Option<u16>,
    ) -> Option<Self> {
        match self {
            Self::Timed { start, end } => {
                let dur = end.abs_minutes() - start.abs_minutes();
                let minute = new_minute.unwrap_or(start.minute);
                let new_start = LocalMinute::new(new_start_day, minute)?;
                let new_end = new_start.add_minutes(dur as i32)?;
                Some(Self::Timed {
                    start: new_start,
                    end: new_end,
                })
            }
            Self::AllDay {
                start,
                end_exclusive,
            } => {
                let dur = end_exclusive - start;
                Some(Self::AllDay {
                    start: new_start_day,
                    end_exclusive: new_start_day + dur,
                })
            }
        }
    }

    pub fn to_all_day(self) -> Option<Self> {
        match self {
            Self::AllDay { .. } => Some(self),
            Self::Timed { start, end } => {
                let mut end_day = end.day;
                if end.minute > 0 {
                    end_day += 1;
                }
                if end_day <= start.day {
                    end_day = start.day + 1;
                }
                if end_day - start.day > MAX_DURATION_DAYS {
                    return None;
                }
                Some(Self::AllDay {
                    start: start.day,
                    end_exclusive: end_day,
                })
            }
        }
    }

    pub fn to_timed(self, start_minute: u16, end_minute_offset: i32) -> Option<Self> {
        match self {
            Self::Timed { .. } => Some(self),
            Self::AllDay {
                start,
                end_exclusive,
            } => {
                let s = LocalMinute::new(start, start_minute)?;
                let e = s.add_minutes(end_minute_offset)?;
                if e.abs_minutes() <= s.abs_minutes() {
                    return None;
                }
                let _ = end_exclusive;
                Some(Self::Timed { start: s, end: e })
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repeat {
    None,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Repeat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
            Self::Yearly => "yearly",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Self::None),
            "daily" => Some(Self::Daily),
            "weekly" => Some(Self::Weekly),
            "monthly" => Some(Self::Monthly),
            "yearly" => Some(Self::Yearly),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Daily => "Daily",
            Self::Weekly => "Weekly",
            Self::Monthly => "Monthly",
            Self::Yearly => "Yearly",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Daily => "Repeats daily",
            Self::Weekly => "Repeats weekly",
            Self::Monthly => "Repeats monthly",
            Self::Yearly => "Repeats yearly",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarEvent {
    pub id: EventId,
    pub calendar_id: CalendarId,
    pub title: String,
    pub timing: Timing,
    pub repeat: Repeat,
    pub notes: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CalendarMode {
    #[default]
    Month,
    Week,
    Day,
}

impl CalendarMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Month => "month",
            Self::Week => "week",
            Self::Day => "day",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "month" => Some(Self::Month),
            "week" => Some(Self::Week),
            "day" => Some(Self::Day),
            _ => None,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Month => 0,
            Self::Week => 1,
            Self::Day => 2,
        }
    }

    pub fn from_index(i: usize) -> Self {
        match i {
            1 => Self::Week,
            2 => Self::Day,
            _ => Self::Month,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preferences {
    pub wide_mode: CalendarMode,
    pub default_calendar: CalendarId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarDocument {
    pub schema_version: u32,
    pub revision: u32,
    pub seed_version: u32,
    pub seed_anchor: Day,
    pub next_event_id: u32,
    pub calendars: Vec<Calendar>,
    pub events: Vec<CalendarEvent>,
    pub preferences: Preferences,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OccurrenceKey {
    pub event_id: EventId,
    pub start_day: Day,
}

impl OccurrenceKey {
    pub fn as_tool_id(self) -> String {
        format!("{}@{}", self.event_id.0, civil::format_iso(self.start_day))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Occurrence {
    pub key: OccurrenceKey,
    pub timing: Timing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventDraft {
    pub editing: Option<EventId>,
    pub value: CalendarEvent,
    pub original: Option<CalendarEvent>,
    pub end_was_edited: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClockSnapshot {
    pub today: Day,
    pub minute: u16,
}

impl ClockSnapshot {
    pub fn now_minute(self) -> LocalMinute {
        LocalMinute {
            day: self.today,
            minute: self.minute.min(MINUTES_PER_DAY - 1),
        }
    }

    /// Next half-hour boundary strictly after now, used as the default start
    /// when creating an event on today.
    pub fn next_half_hour(self) -> LocalMinute {
        let mut minute = ((self.minute / 30) + 1) * 30;
        let mut day = self.today;
        if minute >= MINUTES_PER_DAY {
            minute -= MINUTES_PER_DAY;
            day += 1;
        }
        LocalMinute { day, minute }
    }

    pub fn from_epoch_secs(secs: i64) -> Self {
        let local = local_civil(secs);
        let today = civil::from_ymd(local.year, local.month, local.day);
        let minute = (local.hour * 60 + local.minute).min(MINUTES_PER_DAY as u32 - 1) as u16;
        Self { today, minute }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalCivil {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// True when the platform applied a real local zone.
    pub zoned: bool,
}

/// Host-local civil breakdown of Unix epoch seconds. Unix targets use
/// `localtime_r`; UTC is not substituted on those targets.
pub fn local_civil(secs: i64) -> LocalCivil {
    sys::local(secs).unwrap_or_else(|| {
        // Only reached when the platform call fails. Still a civil breakdown
        // of the same instant, not a zoned conversion.
        let days = secs.div_euclid(86_400) as Day;
        let rem = secs.rem_euclid(86_400) as u32;
        let (year, month, day) = civil::to_ymd(days);
        LocalCivil {
            year,
            month,
            day,
            hour: rem / 3600,
            minute: (rem / 60) % 60,
            second: rem % 60,
            zoned: false,
        }
    })
}

#[cfg(unix)]
mod sys {
    use super::LocalCivil;
    use std::os::raw::{c_char, c_int, c_long};

    #[repr(C)]
    struct Tm {
        tm_sec: c_int,
        tm_min: c_int,
        tm_hour: c_int,
        tm_mday: c_int,
        tm_mon: c_int,
        tm_year: c_int,
        tm_wday: c_int,
        tm_yday: c_int,
        tm_isdst: c_int,
        tm_gmtoff: c_long,
        tm_zone: *const c_char,
    }

    extern "C" {
        fn tzset();
        fn localtime_r(time: *const c_long, out: *mut Tm) -> *mut Tm;
    }

    pub fn local(secs: i64) -> Option<LocalCivil> {
        unsafe { tzset() };
        let t: c_long = c_long::try_from(secs).ok()?;
        let mut tm = Tm {
            tm_sec: 0,
            tm_min: 0,
            tm_hour: 0,
            tm_mday: 1,
            tm_mon: 0,
            tm_year: 70,
            tm_wday: 4,
            tm_yday: 0,
            tm_isdst: 0,
            tm_gmtoff: 0,
            tm_zone: std::ptr::null(),
        };
        let ok = unsafe { !localtime_r(&t, &mut tm).is_null() };
        if !ok {
            return None;
        }
        Some(LocalCivil {
            year: tm.tm_year + 1900,
            month: (tm.tm_mon + 1).clamp(1, 12) as u32,
            day: tm.tm_mday.clamp(1, 31) as u32,
            hour: tm.tm_hour.clamp(0, 23) as u32,
            minute: tm.tm_min.clamp(0, 59) as u32,
            second: tm.tm_sec.clamp(0, 60) as u32,
            zoned: true,
        })
    }
}

#[cfg(not(unix))]
mod sys {
    pub fn local(_secs: i64) -> Option<super::LocalCivil> {
        None
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LayoutKind {
    Compact,
    #[default]
    Wide,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayoutDecision {
    pub kind: LayoutKind,
    pub short_height: bool,
    pub mid_width: bool,
}

impl LayoutDecision {
    /// Widths ≤ 1 are unusable first-draw values: keep the previous decision.
    pub fn decide(width: f64, height: f64, previous: Option<Self>) -> Self {
        if width <= 1.0 {
            return previous.unwrap_or(Self {
                kind: LayoutKind::Wide,
                short_height: false,
                mid_width: false,
            });
        }
        let kind = if width < LAYOUT_COMPACT_BELOW {
            LayoutKind::Compact
        } else {
            LayoutKind::Wide
        };
        Self {
            kind,
            short_height: height < SHORT_HEIGHT_BELOW,
            mid_width: width >= LAYOUT_COMPACT_BELOW && width < LAYOUT_MID_BELOW,
        }
    }

    pub fn sidebar_width(self) -> f64 {
        if self.kind != LayoutKind::Wide || self.short_height {
            0.0
        } else if self.mid_width {
            180.0
        } else {
            220.0
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DraftErrors {
    pub title: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub calendar: Option<String>,
}

impl DraftErrors {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.start.is_none()
            && self.end.is_none()
            && self.calendar.is_none()
    }
}

pub fn scalar_len(s: &str) -> usize {
    s.chars().count()
}

pub fn validate_title(title: &str) -> Result<String, String> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Err("Title is required".into());
    }
    if scalar_len(trimmed) > MAX_TITLE_SCALARS {
        return Err("Title is too long".into());
    }
    Ok(trimmed.to_string())
}

pub fn validate_notes(notes: &str) -> Result<String, String> {
    if scalar_len(notes) > MAX_NOTES_SCALARS {
        return Err("Notes are too long".into());
    }
    Ok(notes.to_string())
}

pub fn validate_timing(timing: Timing) -> Result<(), String> {
    match timing {
        Timing::Timed { start, end } => {
            if start.minute >= MINUTES_PER_DAY || end.minute >= MINUTES_PER_DAY {
                return Err("Invalid time".into());
            }
            let dur = end.abs_minutes() - start.abs_minutes();
            if dur <= 0 {
                return Err("End must be after start".into());
            }
            if dur > MAX_DURATION_DAYS as i64 * MINUTES_PER_DAY as i64 {
                return Err("Event is too long".into());
            }
            Ok(())
        }
        Timing::AllDay {
            start,
            end_exclusive,
        } => {
            let d = end_exclusive - start;
            if d <= 0 {
                return Err("End must be after start".into());
            }
            if d > MAX_DURATION_DAYS {
                return Err("Event is too long".into());
            }
            Ok(())
        }
    }
}

pub fn calendar_by_id(doc: &CalendarDocument, id: CalendarId) -> Option<&Calendar> {
    doc.calendars.iter().find(|c| c.id == id)
}

pub fn event_by_id(doc: &CalendarDocument, id: EventId) -> Option<&CalendarEvent> {
    doc.events.iter().find(|e| e.id == id)
}

pub fn validate_document(doc: &CalendarDocument) -> Result<(), String> {
    if doc.schema_version != SCHEMA_VERSION {
        return Err(format!("unsupported schema_version {}", doc.schema_version));
    }
    if doc.events.len() > MAX_MASTERS {
        return Err("too many events".into());
    }
    let mut seen_cal = std::collections::BTreeSet::new();
    for cal in &doc.calendars {
        if !seen_cal.insert(cal.id) {
            return Err(format!("duplicate calendar id {}", cal.id.0));
        }
        if cal.name.is_empty() {
            return Err("calendar name is empty".into());
        }
    }
    if calendar_by_id(doc, doc.preferences.default_calendar).is_none() {
        return Err("default calendar is unknown".into());
    }
    let mut seen_ev = std::collections::BTreeSet::new();
    for event in &doc.events {
        if !seen_ev.insert(event.id) {
            return Err(format!("duplicate event id {}", event.id.0));
        }
        if event.id.0 >= doc.next_event_id {
            return Err("next_event_id does not follow the masters".into());
        }
        if calendar_by_id(doc, event.calendar_id).is_none() {
            return Err(format!("unknown calendar {}", event.calendar_id.0));
        }
        validate_title(&event.title)?;
        validate_notes(&event.notes)?;
        validate_timing(event.timing)?;
    }
    Ok(())
}

pub fn monday_of(day: Day) -> Day {
    day - civil::weekday(day) as Day
}

pub fn month_grid_monday(year: i32, month: u32) -> [[civil::GridDay; 7]; 6] {
    civil::month_grid(year, month, 0)
}

/// Shift the displayed month by `delta` months. The selected date clamps
/// into the new month while `anchor_dom` (the original day-of-month) is
/// kept so January 31 → February 28 → March 31.
pub fn shift_month(displayed: Day, _selected: Day, anchor_dom: u32, delta: i32) -> (Day, Day, u32) {
    let next = civil::add_months(civil::month_start(displayed), delta);
    let (y, m, _) = civil::to_ymd(next);
    let dom = anchor_dom.clamp(1, civil::days_in_month(y, m));
    (next, civil::from_ymd(y, m, dom), anchor_dom)
}

pub fn shift_week(selected: Day, delta_weeks: i32) -> Day {
    civil::add_days(selected, delta_weeks * 7)
}

pub fn format_heading_wide(day: Day) -> (String, String) {
    let (y, m, d) = civil::to_ymd(day);
    let left = format!("{} {}", civil::month_name(m), d);
    (left, y.to_string())
}

pub fn format_heading_compact_day(day: Day) -> String {
    let (y, m, d) = civil::to_ymd(day);
    let _ = y;
    format!(
        "{}, {} {}",
        civil::weekday_name(civil::weekday(day)),
        d,
        civil::month_name(m)
    )
}

pub fn format_month_year(day: Day) -> String {
    let (y, m, _) = civil::to_ymd(day);
    format!("{} {y}", civil::month_name(m))
}

pub fn parse_hm(text: &str) -> Option<(u32, u32)> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' {
        return None;
    }
    if !bytes[..2].iter().all(u8::is_ascii_digit) || !bytes[3..].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let hour: u32 = text[..2].parse().ok()?;
    let minute: u32 = text[3..].parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((hour, minute))
}

pub fn snap_minute_15(minute: u16) -> u16 {
    ((minute + 7) / 15 * 15).min(MINUTES_PER_DAY - 15)
}

pub fn occurrence_sort_key(occ: &Occurrence, title: &str, event_id: EventId) -> OccSort {
    let (all_day, start, end) = match occ.timing {
        Timing::AllDay {
            start,
            end_exclusive,
        } => (
            0u8,
            start as i64 * MINUTES_PER_DAY as i64,
            end_exclusive as i64 * MINUTES_PER_DAY as i64,
        ),
        Timing::Timed { start, end } => (1u8, start.abs_minutes(), end.abs_minutes()),
    };
    OccSort {
        all_day,
        start,
        end,
        title_folded: title.to_lowercase(),
        event_id,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OccSort {
    pub all_day: u8,
    pub start: i64,
    pub end: i64,
    pub title_folded: String,
    pub event_id: EventId,
}

impl PartialOrd for OccSort {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OccSort {
    fn cmp(&self, other: &Self) -> Ordering {
        self.start.div_euclid(MINUTES_PER_DAY as i64)
            .cmp(&other.start.div_euclid(MINUTES_PER_DAY as i64))
            .then(self.all_day.cmp(&other.all_day))
            .then(self.start.cmp(&other.start))
            .then(self.end.cmp(&other.end))
            .then(self.title_folded.cmp(&other.title_folded))
            .then(self.event_id.cmp(&other.event_id))
    }
}

pub fn mix_rgb(ground: u32, accent: u32, t: f32) -> u32 {
    let unpack = |c: u32| -> (f32, f32, f32) {
        (
            ((c >> 16) & 0xff) as f32 / 255.0,
            ((c >> 8) & 0xff) as f32 / 255.0,
            (c & 0xff) as f32 / 255.0,
        )
    };
    let pack = |r: f32, g: f32, b: f32| -> u32 {
        let ch = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
        (ch(r) << 16) | (ch(g) << 8) | ch(b)
    };
    let (gr, gg, gb) = unpack(ground);
    let (ar, ag, ab) = unpack(accent);
    pack(gr + (ar - gr) * t, gg + (ag - gg) * t, gb + (ab - gb) * t)
}

pub fn luminance(rgb: u32) -> f32 {
    let r = ((rgb >> 16) & 0xff) as f32 / 255.0;
    let g = ((rgb >> 8) & 0xff) as f32 / 255.0;
    let b = (rgb & 0xff) as f32 / 255.0;
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_at_phone_tablet_and_desktop_widths() {
        assert_eq!(
            LayoutDecision::decide(402.0, 780.0, None).kind,
            LayoutKind::Compact
        );
        assert_eq!(
            LayoutDecision::decide(699.0, 800.0, None).kind,
            LayoutKind::Compact
        );
        let wide = LayoutDecision::decide(700.0, 800.0, None);
        assert_eq!(wide.kind, LayoutKind::Wide);
        assert!(wide.mid_width);
        assert!(!wide.short_height);
        let desktop = LayoutDecision::decide(1240.0, 800.0, None);
        assert_eq!(desktop.kind, LayoutKind::Wide);
        assert!(!desktop.mid_width);
        assert_eq!(desktop.sidebar_width(), 220.0);
        assert_eq!(wide.sidebar_width(), 180.0);
    }

    #[test]
    fn short_height_and_unusable_first_width() {
        let short = LayoutDecision::decide(874.0, 300.0, None);
        assert_eq!(short.kind, LayoutKind::Wide);
        assert!(short.short_height);
        assert!(short.mid_width);
        assert_eq!(short.sidebar_width(), 0.0);
        let prev = LayoutDecision::decide(1240.0, 800.0, None);
        let kept = LayoutDecision::decide(0.0, 0.0, Some(prev));
        assert_eq!(kept, prev);
        let kept1 = LayoutDecision::decide(1.0, 800.0, Some(prev));
        assert_eq!(kept1, prev);
        let first = LayoutDecision::decide(0.5, 800.0, None);
        assert_eq!(first.kind, LayoutKind::Wide);
    }

    #[test]
    fn monday_first_grids_cover_edges() {
        // Year boundary: January 2026 starts on Thursday.
        let jan = month_grid_monday(2026, 1);
        assert_eq!(jan[0][0].day, civil::from_ymd(2025, 12, 29));
        assert!(!jan[0][0].in_month);
        assert_eq!(jan[0][3].day, civil::from_ymd(2026, 1, 1));
        assert!(jan[0][3].in_month);
        // Four-week February 2015 starts on Sunday — still six rows.
        let feb = month_grid_monday(2015, 2);
        assert_eq!(feb.len(), 6);
        assert_eq!(feb.iter().flatten().filter(|c| c.in_month).count(), 28);
        // Six-week October 2022 starts on Saturday.
        let oct = month_grid_monday(2022, 10);
        assert!(oct[0][0].in_month == false);
        assert_eq!(oct[0][5].day, civil::from_ymd(2022, 10, 1));
        assert_eq!(oct[5][0].day, civil::from_ymd(2022, 10, 31));
        // Leap February 2024 starts on Thursday.
        let leap = month_grid_monday(2024, 2);
        assert_eq!(leap.iter().flatten().filter(|c| c.in_month).count(), 29);
        assert_eq!(leap[4][3].day, civil::from_ymd(2024, 2, 29));
    }

    #[test]
    fn month_navigation_keeps_the_day_of_month_anchor() {
        let jan31 = civil::from_ymd(2024, 1, 31);
        let (feb, sel, anchor) = shift_month(jan31, jan31, 31, 1);
        assert_eq!(civil::to_ymd(feb), (2024, 2, 1));
        assert_eq!(civil::to_ymd(sel), (2024, 2, 29));
        assert_eq!(anchor, 31);
        let (mar, sel, anchor) = shift_month(feb, sel, anchor, 1);
        assert_eq!(civil::to_ymd(mar), (2024, 3, 1));
        assert_eq!(civil::to_ymd(sel), (2024, 3, 31));
        assert_eq!(anchor, 31);
    }

    #[test]
    fn timed_strings_and_minutes_reject_seconds_and_zones() {
        let m = LocalMinute::parse_iso("2026-09-09T09:00").unwrap();
        assert_eq!(m.hour(), 9);
        assert_eq!(m.format_iso(), "2026-09-09T09:00");
        assert!(LocalMinute::parse_iso("2026-09-09T09:00:00").is_none());
        assert!(LocalMinute::parse_iso("2026-09-09T09:00Z").is_none());
        assert!(LocalMinute::new(0, 1440).is_none());
        assert!(parse_hm("24:00").is_none());
        assert_eq!(parse_hm("09:30"), Some((9, 30)));
    }

    #[test]
    fn midnight_end_does_not_occupy_the_next_day() {
        let start = LocalMinute::from_hm(10, 22, 0).unwrap();
        let end = LocalMinute::from_hm(11, 0, 0).unwrap();
        let t = Timing::Timed { start, end };
        assert!(t.intersects_day(10));
        assert!(!t.intersects_day(11));
    }

    #[cfg(unix)]
    #[test]
    fn local_civil_is_zoned_on_unix() {
        let local = local_civil(1_788_698_096);
        assert!(local.zoned, "localtime_r must supply a zone");
        assert!(local.hour < 24 && local.minute < 60);
        let snap = ClockSnapshot::from_epoch_secs(1_788_698_096);
        assert_eq!(
            civil::to_ymd(snap.today),
            (local.year, local.month, local.day)
        );
    }

    #[test]
    fn next_half_hour_snaps_forward() {
        let c = ClockSnapshot {
            today: 0,
            minute: 9 * 60 + 5,
        };
        let n = c.next_half_hour();
        assert_eq!(n.minute, 9 * 60 + 30);
        let c = ClockSnapshot {
            today: 0,
            minute: 23 * 60 + 50,
        };
        let n = c.next_half_hour();
        assert_eq!(n.day, 1);
        assert_eq!(n.minute, 0);
    }

    #[test]
    fn title_and_notes_scalar_caps() {
        assert!(validate_title("  ").is_err());
        assert!(validate_title(&"é".repeat(240)).is_ok());
        assert!(validate_title(&"é".repeat(241)).is_err());
        assert!(validate_notes(&"😀".repeat(4096)).is_ok());
        assert!(validate_notes(&"😀".repeat(4097)).is_err());
    }
    #[test]
    fn iso_minutes_and_hm_reject_multibyte_tokens() {
        for value in ["2026-09-09Té:00", "2026-09-09T09:é", "202é-09-9T09:00", "2026-09-09T😀00", "日本語"] {
            assert!(LocalMinute::parse_iso(value).is_none(), "{value}");
        }
        for value in ["é:00", "09:é", "😀:", "日:0"] { assert!(parse_hm(value).is_none()); }
    }

}

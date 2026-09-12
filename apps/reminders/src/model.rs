//! Pure reminder types. No widgets, storage, or clock.

use makepad_civil_time::{self as civil, Day};

pub type ListId = u32;
pub type ReminderId = u32;

pub const SCHEMA_VERSION: u32 = 1;
pub const SEED_VERSION: u32 = 1;
pub const MAX_TITLE_CHARS: usize = 240;
pub const MAX_NOTES_BYTES: usize = 4096;
pub const MAX_LIST_NAME_CHARS: usize = 64;
pub const MAX_LISTS: usize = 32;
pub const MAX_REMINDERS: usize = 2000;
pub const MAX_DOCUMENT_BYTES: usize = 2 * 1024 * 1024;
pub const YEAR_MIN: i32 = 1900;
pub const YEAR_MAX: i32 = 2199;
pub const HOME_LIST_ID: ListId = 3;
pub const GROCERIES_LIST_ID: ListId = 1;
pub const WORK_LIST_ID: ListId = 2;
pub const TRAVEL_LIST_ID: ListId = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListColour {
    Blue,
    Green,
    Orange,
    Purple,
}

impl ListColour {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blue => "blue",
            Self::Green => "green",
            Self::Orange => "orange",
            Self::Purple => "purple",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "blue" => Some(Self::Blue),
            "green" => Some(Self::Green),
            "orange" => Some(Self::Orange),
            "purple" => Some(Self::Purple),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Priority {
    #[default]
    None,
    Low,
    Medium,
    High,
}

impl Priority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "none" => Some(Self::None),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    pub fn bangs(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Low => "! ",
            Self::Medium => "!! ",
            Self::High => "!!! ",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::None => 0,
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 3,
        }
    }

    pub fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Low,
            2 => Self::Medium,
            3 => Self::High,
            _ => Self::None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReminderList {
    pub id: ListId,
    pub name: String,
    pub colour: ListColour,
    pub order: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Due {
    pub day: Day,
    pub minute: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletedAt {
    pub day: Day,
    pub minute: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reminder {
    pub id: ReminderId,
    pub list_id: ListId,
    pub title: String,
    pub notes: String,
    pub due: Option<Due>,
    pub flagged: bool,
    pub priority: Priority,
    pub order: u32,
    pub completed_at: Option<CompletedAt>,
}

impl Reminder {
    pub fn is_completed(&self) -> bool {
        self.completed_at.is_some()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Filter {
    #[default]
    Today,
    Scheduled,
    All,
    Flagged,
    Completed,
    List(ListId),
}

impl Filter {
    pub fn label(self, lists: &[ReminderList]) -> String {
        match self {
            Self::Today => "Today".into(),
            Self::Scheduled => "Scheduled".into(),
            Self::All => "All".into(),
            Self::Flagged => "Flagged".into(),
            Self::Completed => "Completed".into(),
            Self::List(id) => lists
                .iter()
                .find(|list| list.id == id)
                .map(|list| list.name.clone())
                .unwrap_or_else(|| "List".into()),
        }
    }

    pub fn parse_name(name: &str, lists: &[ReminderList]) -> Option<Self> {
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.chars().count() > MAX_LIST_NAME_CHARS {
            return None;
        }
        let lower = trimmed.to_ascii_lowercase();
        let smart = match lower.as_str() {
            "today" => Some(Self::Today),
            "scheduled" => Some(Self::Scheduled),
            "all" => Some(Self::All),
            "flagged" => Some(Self::Flagged),
            "completed" => Some(Self::Completed),
            _ => None,
        };
        if smart.is_some() {
            return smart;
        }
        lists
            .iter()
            .find(|list| list.name.eq_ignore_ascii_case(trimmed))
            .map(|list| Self::List(list.id))
    }

    pub fn allows_create(self) -> bool {
        !matches!(self, Self::Completed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    pub schema_version: u32,
    pub seed_version: u32,
    pub seed_day: Day,
    pub revision: u64,
    pub next_reminder_id: ReminderId,
    pub lists: Vec<ReminderList>,
    pub reminders: Vec<Reminder>,
    pub show_completed: bool,
}

impl Document {
    pub fn list(&self, id: ListId) -> Option<&ReminderList> {
        self.lists.iter().find(|list| list.id == id)
    }

    pub fn reminder(&self, id: ReminderId) -> Option<&Reminder> {
        self.reminders.iter().find(|item| item.id == id)
    }

    pub fn reminder_mut(&mut self, id: ReminderId) -> Option<&mut Reminder> {
        self.reminders.iter_mut().find(|item| item.id == id)
    }

    pub fn next_order_in(&self, list_id: ListId) -> u32 {
        self.reminders
            .iter()
            .filter(|item| item.list_id == list_id)
            .map(|item| item.order)
            .max()
            .map(|order| order.saturating_add(1))
            .unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Now {
    pub day: Day,
    pub minute: u16,
}

impl Now {
    pub fn from_epoch_secs(epoch_secs: i64, offset_secs: i64) -> Self {
        let local = epoch_secs.saturating_add(offset_secs);
        Self {
            day: local.div_euclid(86_400) as Day,
            minute: (local.rem_euclid(86_400) / 60) as u16,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub original_id: Option<ReminderId>,
    pub value: Reminder,
}

/// Raw editor contents belong to the root too: an invalid date or unfinished
/// title must survive switching between the compact and wide forms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftFields {
    pub title: String,
    pub notes: String,
    pub date_on: bool,
    pub date: String,
    pub time_on: bool,
    pub time: String,
    pub list_id: ListId,
    pub flagged: bool,
    pub priority: Priority,
}

impl DraftFields {
    pub fn from_draft(draft: &Draft) -> Self {
        let value = &draft.value;
        Self {
            title: value.title.clone(),
            notes: value.notes.clone(),
            date_on: value.due.is_some(),
            date: value
                .due
                .map(|due| civil::format_iso(due.day))
                .unwrap_or_default(),
            time_on: value.due.and_then(|due| due.minute).is_some(),
            time: value
                .due
                .and_then(|due| due.minute)
                .map(format_time)
                .unwrap_or_default(),
            list_id: value.list_id,
            flagged: value.flagged,
            priority: value.priority,
        }
    }

    pub fn apply_to(&self, mut draft: Draft) -> Result<Draft, &'static str> {
        draft.value.title = normalize_title(&self.title);
        if let Some(error) = title_error(&draft.value.title) {
            return Err(error);
        }
        if let Some(error) = notes_error(&self.notes) {
            return Err(error);
        }
        draft.value.notes = self.notes.clone();
        draft.value.due = if self.date_on {
            Some(Due {
                day: parse_date(&self.date)?,
                minute: if self.time_on {
                    Some(parse_time(&self.time)?)
                } else {
                    None
                },
            })
        } else {
            None
        };
        draft.value.list_id = self.list_id;
        draft.value.flagged = self.flagged;
        draft.value.priority = self.priority;
        Ok(draft)
    }

    pub fn set_date_enabled(&mut self, on: bool, now: Now) {
        self.date_on = on;
        if on {
            self.date = civil::format_iso(now.day);
        } else {
            self.time_on = false;
        }
    }

    pub fn set_time_enabled(&mut self, on: bool, now: Now) -> Result<(), &'static str> {
        // Validate the typed date before updating any scheduling field.
        let day = if self.date_on {
            parse_date(&self.date)?
        } else {
            now.day
        };
        if on {
            if !self.date_on {
                self.date = civil::format_iso(day);
            }
            self.date_on = true;
            self.time = "09:00".into();
        }
        self.time_on = on;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Home,
    List(Filter),
    Detail,
}

impl Default for Route {
    fn default() -> Self {
        Self::List(Filter::Today)
    }
}

pub fn normalize_title(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch == '\n' || ch == '\r' {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out.trim().to_string()
}

pub fn title_error(title: &str) -> Option<&'static str> {
    let chars = title.chars().count();
    if chars == 0 {
        Some("Title is required")
    } else if chars > MAX_TITLE_CHARS {
        Some("Title is too long")
    } else {
        None
    }
}

pub fn notes_error(notes: &str) -> Option<&'static str> {
    if notes.len() > MAX_NOTES_BYTES {
        Some("Notes are too long")
    } else {
        None
    }
}

pub fn list_name_error(name: &str) -> Option<&'static str> {
    let chars = name.chars().count();
    if chars == 0 || chars > MAX_LIST_NAME_CHARS {
        Some("List name is invalid")
    } else {
        None
    }
}

pub fn day_in_range(day: Day) -> bool {
    let year = civil::year_of(day);
    (YEAR_MIN..=YEAR_MAX).contains(&year)
}

pub fn parse_date(text: &str) -> Result<Day, &'static str> {
    let day = civil::parse_iso(text).ok_or("Date must be YYYY-MM-DD")?;
    if !day_in_range(day) {
        return Err("Date must be between 1900 and 2199");
    }
    Ok(day)
}

pub fn parse_time(text: &str) -> Result<u16, &'static str> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' {
        return Err("Time must be HH:MM");
    }
    if !bytes[..2].iter().all(u8::is_ascii_digit) || !bytes[3..].iter().all(u8::is_ascii_digit) {
        return Err("Time must be HH:MM");
    }
    let hour: u16 = text[..2].parse().map_err(|_| "Time must be HH:MM")?;
    let minute: u16 = text[3..].parse().map_err(|_| "Time must be HH:MM")?;
    if hour > 23 || minute > 59 {
        return Err("Time must be HH:MM");
    }
    Ok(hour * 60 + minute)
}

pub fn format_time(minute: u16) -> String {
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

pub fn format_due_day(day: Day, today: Day) -> String {
    if day == today {
        "Today".into()
    } else if day == today - 1 {
        "Yesterday".into()
    } else if day == today + 1 {
        "Tomorrow".into()
    } else {
        civil::format_short(day)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_civil_time::from_ymd;

    #[test]
    fn title_normalizes_line_breaks_and_unicode_limits() {
        assert_eq!(normalize_title("  Hello\nthere\r\n "), "Hello there");
        assert!(title_error("").is_some());
        assert!(title_error("ok").is_none());
        let long: String = "é".repeat(MAX_TITLE_CHARS);
        assert!(title_error(&long).is_none());
        assert!(title_error(&format!("{long}x")).is_some());
        assert!(notes_error(&"a".repeat(MAX_NOTES_BYTES)).is_none());
        assert!(notes_error(&"a".repeat(MAX_NOTES_BYTES + 1)).is_some());
    }

    #[test]
    fn dates_and_times_validate() {
        assert!(parse_date("2024-02-29").is_ok());
        assert!(parse_date("2023-02-29").is_err());
        assert!(parse_date("1899-12-31").is_err());
        assert!(parse_date("2200-01-01").is_err());
        assert_eq!(parse_time("09:00").unwrap(), 9 * 60);
        assert!(parse_time("24:00").is_err());
        assert!(parse_time("9:00").is_err());
        assert_eq!(format_time(0), "00:00");
        assert_eq!(format_time(17 * 60), "17:00");
    }

    #[test]
    fn compact_title_edit_keeps_the_rest_of_reminder_12() {
        // The unseeded compact form is blank / Groceries. Committing that
        // snapshot would wipe notes, date, flag, priority and the Work list.
        let reminder = Reminder {
            id: 12,
            list_id: WORK_LIST_ID,
            title: "Review release checklist".into(),
            notes: "Check keyboard navigation and compact layout".into(),
            due: Some(Due {
                day: from_ymd(2026, 9, 9),
                minute: Some(10 * 60),
            }),
            flagged: true,
            priority: Priority::High,
            order: 1,
            completed_at: None,
        };
        let draft = Draft {
            original_id: Some(12),
            value: reminder.clone(),
        };
        let unseeded = DraftFields {
            title: "New title".into(),
            notes: String::new(),
            date_on: false,
            date: String::new(),
            time_on: false,
            time: String::new(),
            list_id: GROCERIES_LIST_ID,
            flagged: false,
            priority: Priority::None,
        };
        let wiped = unseeded.apply_to(draft.clone()).unwrap();
        assert!(wiped.value.notes.is_empty());
        assert_eq!(wiped.value.list_id, GROCERIES_LIST_ID);
        assert!(!wiped.value.flagged);
        assert_eq!(wiped.value.priority, Priority::None);
        assert!(wiped.value.due.is_none());

        let mut fields = DraftFields::from_draft(&draft);
        fields.title = "New title".into();
        let committed = fields.apply_to(draft).unwrap();
        assert_eq!(committed.value.title, "New title");
        assert_eq!(committed.value.notes, reminder.notes);
        assert_eq!(committed.value.due, reminder.due);
        assert_eq!(committed.value.flagged, true);
        assert_eq!(committed.value.priority, Priority::High);
        assert_eq!(committed.value.list_id, WORK_LIST_ID);
    }

    #[test]
    fn enabling_time_reads_the_typed_date_not_the_stale_due() {
        let now = Now {
            day: from_ymd(2026, 9, 9),
            minute: 12 * 60,
        };
        let reminder = Reminder {
            id: 1,
            list_id: HOME_LIST_ID,
            title: "All day".into(),
            notes: String::new(),
            due: Some(Due {
                day: now.day,
                minute: None,
            }),
            flagged: false,
            priority: Priority::None,
            order: 0,
            completed_at: None,
        };
        let mut fields = DraftFields::from_draft(&Draft {
            original_id: Some(1),
            value: reminder,
        });
        fields.date = "2026-09-20".into();
        fields.set_time_enabled(true, now).unwrap();
        assert_eq!(fields.date, "2026-09-20");
        assert_eq!(fields.time, "09:00");
        assert!(fields.date_on && fields.time_on);
        let committed = fields
            .apply_to(Draft {
                original_id: Some(1),
                value: Reminder {
                    id: 1,
                    list_id: HOME_LIST_ID,
                    title: "All day".into(),
                    notes: String::new(),
                    due: Some(Due {
                        day: now.day,
                        minute: None,
                    }),
                    flagged: false,
                    priority: Priority::None,
                    order: 0,
                    completed_at: None,
                },
            })
            .unwrap();
        assert_eq!(
            committed.value.due,
            Some(Due {
                day: from_ymd(2026, 9, 20),
                minute: Some(9 * 60)
            })
        );
    }
}

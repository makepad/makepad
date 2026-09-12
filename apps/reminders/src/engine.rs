//! Projection and commands. UI never filters independently.

use crate::model::*;
use makepad_civil_time::Day;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LayoutMode {
    Compact,
    #[default]
    Wide,
    WideShort,
}

impl LayoutMode {
    pub fn is_compact(self) -> bool {
        matches!(self, Self::Compact)
    }

    pub fn is_short(self) -> bool {
        matches!(self, Self::WideShort)
    }

    pub fn is_wide(self) -> bool {
        !self.is_compact()
    }
}

/// Root-owned date refresh: one minute, not one second.
pub const MINUTE_TICK_SECS: f64 = 60.0;
/// Popup list-picker rows must stay a 44-point target.
pub const LIST_PICKER_ITEM_HEIGHT: f64 = 44.0;

pub fn smart_tile_height(mode: LayoutMode) -> f64 {
    match mode {
        LayoutMode::Compact => 88.0,
        LayoutMode::Wide => 76.0,
        LayoutMode::WideShort => 68.0,
    }
}

pub fn completed_tile_height(mode: LayoutMode) -> f64 {
    match mode {
        LayoutMode::Compact => 56.0,
        LayoutMode::Wide => 52.0,
        LayoutMode::WideShort => 44.0,
    }
}

pub fn personal_row_height(mode: LayoutMode) -> f64 {
    match mode {
        LayoutMode::Compact => 56.0,
        LayoutMode::Wide | LayoutMode::WideShort => 44.0,
    }
}

/// Width below 700 is compact. Non-finite or uninitialized width keeps
/// `previous`. Height below 420 selects the short-height wide variant.
pub fn decide_layout(width: f64, height: f64, previous: Option<LayoutMode>) -> LayoutMode {
    if !width.is_finite() || width <= 0.0 {
        return previous.unwrap_or(LayoutMode::Wide);
    }
    if width < 700.0 {
        LayoutMode::Compact
    } else if height.is_finite() && height > 0.0 && height < 420.0 {
        LayoutMode::WideShort
    } else {
        LayoutMode::Wide
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FilterCounts {
    pub today: usize,
    pub scheduled: usize,
    pub all: usize,
    pub flagged: usize,
    pub completed: usize,
    pub groceries: usize,
    pub work: usize,
    pub home: usize,
    pub travel: usize,
}

impl FilterCounts {
    pub fn for_list(self, id: ListId) -> usize {
        match id {
            GROCERIES_LIST_ID => self.groceries,
            WORK_LIST_ID => self.work,
            HOME_LIST_ID => self.home,
            TRAVEL_LIST_ID => self.travel,
            _ => 0,
        }
    }

    pub fn for_filter(self, filter: Filter) -> usize {
        match filter {
            Filter::Today => self.today,
            Filter::Scheduled => self.scheduled,
            Filter::All => self.all,
            Filter::Flagged => self.flagged,
            Filter::Completed => self.completed,
            Filter::List(id) => self.for_list(id),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectedItem {
    pub id: ReminderId,
    pub title: String,
    pub notes_preview: Option<String>,
    pub metadata: Option<String>,
    pub flagged: bool,
    pub priority: Priority,
    pub completed: bool,
    pub overdue: bool,
    pub list_id: ListId,
    pub list_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectedGroup {
    pub heading: Option<String>,
    pub items: Vec<ProjectedItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    pub title: String,
    pub active_count: usize,
    pub completed_count: usize,
    pub groups: Vec<ProjectedGroup>,
    pub completed_groups: Vec<ProjectedGroup>,
    pub counts: FilterCounts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListRow {
    Section(String),
    Item(ProjectedItem),
    CompletedSummary { count: usize, expanded: bool },
    Empty(&'static str),
}

pub fn is_overdue(item: &Reminder, now: Now) -> bool {
    if item.is_completed() {
        return false;
    }
    let Some(due) = item.due else {
        return false;
    };
    if due.day < now.day {
        return true;
    }
    if due.day > now.day {
        return false;
    }
    match due.minute {
        Some(minute) => minute < now.minute,
        None => false,
    }
}

pub fn due_cmp(a: &Reminder, b: &Reminder) -> std::cmp::Ordering {
    match (a.due, b.due) {
        (None, None) => a.id.cmp(&b.id),
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(da), Some(db)) => da
            .day
            .cmp(&db.day)
            .then_with(|| match (da.minute, db.minute) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(ma), Some(mb)) => ma.cmp(&mb),
            })
            .then_with(|| a.id.cmp(&b.id)),
    }
}

fn list_order_cmp(doc: &Document, a: &Reminder, b: &Reminder) -> std::cmp::Ordering {
    let ao = doc.list(a.list_id).map(|l| l.order).unwrap_or(u32::MAX);
    let bo = doc.list(b.list_id).map(|l| l.order).unwrap_or(u32::MAX);
    ao.cmp(&bo)
        .then_with(|| a.order.cmp(&b.order))
        .then_with(|| a.id.cmp(&b.id))
}

fn matches_active(item: &Reminder, filter: Filter, now: Now) -> bool {
    match filter {
        Filter::Today => {
            !item.is_completed() && item.due.map(|d| d.day <= now.day).unwrap_or(false)
        }
        Filter::Scheduled => !item.is_completed() && item.due.is_some(),
        Filter::All => !item.is_completed(),
        Filter::Flagged => !item.is_completed() && item.flagged,
        Filter::Completed => item.is_completed(),
        Filter::List(id) => !item.is_completed() && item.list_id == id,
    }
}

fn matches_completed_footer(item: &Reminder, filter: Filter, now: Now) -> bool {
    if !item.is_completed() {
        return false;
    }
    match filter {
        Filter::Completed => true,
        Filter::Today => item.due.map(|d| d.day <= now.day).unwrap_or(false),
        Filter::Scheduled => item.due.is_some(),
        Filter::All => true,
        Filter::Flagged => item.flagged,
        Filter::List(id) => item.list_id == id,
    }
}

fn notes_preview(notes: &str) -> Option<String> {
    let line = notes.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        None
    } else {
        Some(line.to_string())
    }
}

fn metadata_line(item: &Reminder, list_name: &str, now: Now, show_list: bool) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(due) = item.due {
        let mut text = format_due_day(due.day, now.day);
        if let Some(minute) = due.minute {
            text.push_str(", ");
            text.push_str(&format_time(minute));
        }
        parts.push(text);
    }
    if item.flagged {
        parts.push("Flagged".into());
    }
    if show_list {
        parts.push(list_name.to_string());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

fn to_item(doc: &Document, item: &Reminder, now: Now, show_list: bool) -> ProjectedItem {
    let list_name = doc
        .list(item.list_id)
        .map(|l| l.name.clone())
        .unwrap_or_default();
    ProjectedItem {
        id: item.id,
        title: format!("{}{}", item.priority.bangs(), item.title),
        notes_preview: notes_preview(&item.notes),
        metadata: metadata_line(item, &list_name, now, show_list),
        flagged: item.flagged,
        priority: item.priority,
        completed: item.is_completed(),
        overdue: is_overdue(item, now),
        list_id: item.list_id,
        list_name,
    }
}

fn push_group(
    groups: &mut Vec<ProjectedGroup>,
    heading: Option<String>,
    items: Vec<ProjectedItem>,
) {
    if !items.is_empty() {
        groups.push(ProjectedGroup { heading, items });
    }
}

pub fn counts(doc: &Document, now: Now) -> FilterCounts {
    let mut counts = FilterCounts::default();
    for item in &doc.reminders {
        if item.is_completed() {
            counts.completed += 1;
            continue;
        }
        counts.all += 1;
        if item.due.map(|d| d.day <= now.day).unwrap_or(false) {
            counts.today += 1;
        }
        if item.due.is_some() {
            counts.scheduled += 1;
        }
        if item.flagged {
            counts.flagged += 1;
        }
        match item.list_id {
            GROCERIES_LIST_ID => counts.groceries += 1,
            WORK_LIST_ID => counts.work += 1,
            HOME_LIST_ID => counts.home += 1,
            TRAVEL_LIST_ID => counts.travel += 1,
            _ => {}
        }
    }
    counts
}

pub fn project(doc: &Document, filter: Filter, now: Now) -> Projection {
    let counts = counts(doc, now);
    let show_list = matches!(
        filter,
        Filter::Today | Filter::Scheduled | Filter::All | Filter::Flagged | Filter::Completed
    );
    let mut active: Vec<&Reminder> = doc
        .reminders
        .iter()
        .filter(|item| matches_active(item, filter, now))
        .collect();
    let mut completed: Vec<&Reminder> = doc
        .reminders
        .iter()
        .filter(|item| matches_completed_footer(item, filter, now))
        .collect();

    let mut groups = Vec::new();
    match filter {
        Filter::Today => {
            active.sort_by(|a, b| due_cmp(a, b));
            let mut overdue = Vec::new();
            let mut today = Vec::new();
            for item in active {
                if item.due.map(|d| d.day < now.day).unwrap_or(false) {
                    overdue.push(to_item(doc, item, now, show_list));
                } else {
                    today.push(to_item(doc, item, now, show_list));
                }
            }
            push_group(&mut groups, Some("Overdue".into()), overdue);
            push_group(&mut groups, Some("Today".into()), today);
        }
        Filter::Scheduled => {
            active.sort_by(|a, b| due_cmp(a, b));
            let mut overdue = Vec::new();
            let mut by_day: Vec<(Day, Vec<ProjectedItem>)> = Vec::new();
            for item in active {
                let day = item.due.map(|d| d.day).unwrap_or(now.day);
                if day < now.day {
                    overdue.push(to_item(doc, item, now, show_list));
                    continue;
                }
                if let Some((_, bucket)) = by_day.iter_mut().find(|(d, _)| *d == day) {
                    bucket.push(to_item(doc, item, now, show_list));
                } else {
                    by_day.push((day, vec![to_item(doc, item, now, show_list)]));
                }
            }
            push_group(&mut groups, Some("Overdue".into()), overdue);
            for (day, items) in by_day {
                push_group(&mut groups, Some(format_due_day(day, now.day)), items);
            }
        }
        Filter::All | Filter::Flagged => {
            active.sort_by(|a, b| list_order_cmp(doc, a, b));
            let mut current: Option<ListId> = None;
            let mut bucket = Vec::new();
            let mut heading = None;
            for item in active {
                if current != Some(item.list_id) {
                    if let Some(h) = heading.take() {
                        push_group(&mut groups, Some(h), std::mem::take(&mut bucket));
                    }
                    current = Some(item.list_id);
                    heading = Some(
                        doc.list(item.list_id)
                            .map(|l| l.name.clone())
                            .unwrap_or_default(),
                    );
                }
                bucket.push(to_item(doc, item, now, show_list));
            }
            if let Some(h) = heading {
                push_group(&mut groups, Some(h), bucket);
            }
        }
        Filter::Completed => {
            active.sort_by(|a, b| completed_cmp(a, b));
            let items: Vec<_> = active
                .into_iter()
                .map(|item| to_item(doc, item, now, show_list))
                .collect();
            push_group(&mut groups, None, items);
        }
        Filter::List(_) => {
            active.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
            let items: Vec<_> = active
                .into_iter()
                .map(|item| to_item(doc, item, now, false))
                .collect();
            push_group(&mut groups, None, items);
        }
    }

    let mut completed_groups = Vec::new();
    if !matches!(filter, Filter::Completed) {
        match filter {
            Filter::Today => {
                completed.sort_by(|a, b| completed_cmp(a, b));
                let items: Vec<_> = completed
                    .into_iter()
                    .map(|item| to_item(doc, item, now, show_list))
                    .collect();
                push_group(&mut completed_groups, None, items);
            }
            Filter::Scheduled => {
                completed.sort_by(|a, b| due_cmp(a, b).then_with(|| completed_cmp(a, b)));
                let items: Vec<_> = completed
                    .into_iter()
                    .map(|item| to_item(doc, item, now, show_list))
                    .collect();
                push_group(&mut completed_groups, None, items);
            }
            Filter::All | Filter::Flagged => {
                completed
                    .sort_by(|a, b| list_order_cmp(doc, a, b).then_with(|| completed_cmp(a, b)));
                let mut current: Option<ListId> = None;
                let mut bucket = Vec::new();
                let mut heading = None;
                for item in completed {
                    if current != Some(item.list_id) {
                        if let Some(h) = heading.take() {
                            push_group(&mut completed_groups, Some(h), std::mem::take(&mut bucket));
                        }
                        current = Some(item.list_id);
                        heading = Some(
                            doc.list(item.list_id)
                                .map(|l| l.name.clone())
                                .unwrap_or_default(),
                        );
                    }
                    bucket.push(to_item(doc, item, now, show_list));
                }
                if let Some(h) = heading {
                    push_group(&mut completed_groups, Some(h), bucket);
                }
            }
            Filter::List(_) => {
                completed.sort_by(|a, b| completed_cmp(a, b));
                let items: Vec<_> = completed
                    .into_iter()
                    .map(|item| to_item(doc, item, now, false))
                    .collect();
                push_group(&mut completed_groups, None, items);
            }
            Filter::Completed => {}
        }
    }

    let active_count = groups.iter().map(|g| g.items.len()).sum();
    let completed_count = completed_groups.iter().map(|g| g.items.len()).sum();
    Projection {
        title: filter.label(&doc.lists),
        active_count,
        completed_count,
        groups,
        completed_groups,
        counts,
    }
}

fn completed_cmp(a: &Reminder, b: &Reminder) -> std::cmp::Ordering {
    match (a.completed_at, b.completed_at) {
        (Some(ca), Some(cb)) => cb
            .day
            .cmp(&ca.day)
            .then_with(|| cb.minute.cmp(&ca.minute))
            .then_with(|| a.id.cmp(&b.id)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.id.cmp(&b.id),
    }
}

pub fn flatten_rows(projection: &Projection, filter: Filter, show_completed: bool) -> Vec<ListRow> {
    let mut rows = Vec::new();
    if projection.groups.is_empty()
        && (matches!(filter, Filter::Completed)
            || !show_completed
            || projection.completed_count == 0)
    {
        let empty = if matches!(filter, Filter::Completed) {
            "No completed reminders"
        } else {
            "No reminders"
        };
        rows.push(ListRow::Empty(empty));
    } else {
        for group in &projection.groups {
            if let Some(heading) = &group.heading {
                if projection.groups.len() > 1 || heading.as_str() != "Today" {
                    rows.push(ListRow::Section(heading.clone()));
                } else if heading.as_str() != projection.title {
                    rows.push(ListRow::Section(heading.clone()));
                }
            }
            for item in &group.items {
                rows.push(ListRow::Item(item.clone()));
            }
        }
        if groups_need_today_heading(projection) {
            // headings already added
        }
    }
    if !matches!(filter, Filter::Completed) && projection.completed_count > 0 {
        rows.push(ListRow::CompletedSummary {
            count: projection.completed_count,
            expanded: show_completed,
        });
        if show_completed {
            for group in &projection.completed_groups {
                if let Some(heading) = &group.heading {
                    rows.push(ListRow::Section(heading.clone()));
                }
                for item in &group.items {
                    rows.push(ListRow::Item(item.clone()));
                }
            }
        }
    }
    rows
}

fn groups_need_today_heading(_projection: &Projection) -> bool {
    false
}

pub fn row_height(compact: bool, title_lines: u32, notes: bool, metadata: bool) -> f64 {
    let title_lines = title_lines.max(1).min(2);
    let raw = if compact {
        (20.0
            + 22.0 * title_lines as f64
            + if notes { 18.0 } else { 0.0 }
            + if metadata { 18.0 } else { 0.0 })
        .max(52.0)
    } else {
        (16.0
            + 20.0 * title_lines as f64
            + if notes { 17.0 } else { 0.0 }
            + if metadata { 17.0 } else { 0.0 })
        .max(44.0)
    };
    (raw / 4.0).ceil() * 4.0
}

#[derive(Clone, Debug)]
pub enum Command {
    Create {
        title: String,
        list_id: ListId,
        due: Option<Due>,
        flagged: bool,
    },
    ApplyDraft(Draft),
    SetCompleted {
        id: ReminderId,
        completed: bool,
    },
    SetFlagged {
        id: ReminderId,
        flagged: bool,
    },
    MoveToList {
        id: ReminderId,
        list_id: ListId,
    },
    Delete {
        id: ReminderId,
    },
    SetShowCompleted {
        show: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    Rejected(&'static str),
    Missing,
    NoOp,
}

pub struct CreateDefaults {
    pub list_id: ListId,
    pub due: Option<Due>,
    pub flagged: bool,
}

pub fn create_defaults(filter: Filter, now: Now) -> Option<CreateDefaults> {
    match filter {
        Filter::Completed => None,
        Filter::Today | Filter::Scheduled => Some(CreateDefaults {
            list_id: HOME_LIST_ID,
            due: Some(Due {
                day: now.day,
                minute: None,
            }),
            flagged: false,
        }),
        Filter::Flagged => Some(CreateDefaults {
            list_id: HOME_LIST_ID,
            due: None,
            flagged: true,
        }),
        Filter::All => Some(CreateDefaults {
            list_id: HOME_LIST_ID,
            due: None,
            flagged: false,
        }),
        Filter::List(id) => Some(CreateDefaults {
            list_id: id,
            due: None,
            flagged: false,
        }),
    }
}

pub fn home_create_defaults() -> CreateDefaults {
    CreateDefaults {
        list_id: HOME_LIST_ID,
        due: None,
        flagged: false,
    }
}

fn validate_reminder_fields(value: &Reminder, lists: &[ReminderList]) -> Result<(), CommandError> {
    if let Some(error) = title_error(&value.title) {
        return Err(CommandError::Rejected(error));
    }
    if notes_error(&value.notes).is_some() {
        return Err(CommandError::Rejected("Notes are too long"));
    }
    if lists.iter().all(|list| list.id != value.list_id) {
        return Err(CommandError::Rejected("Unknown list"));
    }
    if let Some(due) = value.due {
        if !day_in_range(due.day) {
            return Err(CommandError::Rejected("Date must be between 1900 and 2199"));
        }
        if let Some(minute) = due.minute {
            if minute > 1439 {
                return Err(CommandError::Rejected("Time must be HH:MM"));
            }
        }
    }
    Ok(())
}

pub fn apply(doc: &mut Document, command: Command, now: Now) -> Result<(), CommandError> {
    // Publish only a complete, serializable candidate. Rejected commands leave
    // both the document and its revision untouched, including aggregate limits.
    let mut candidate = doc.clone();
    apply_candidate(&mut candidate, command, now)?;
    candidate.revision = doc
        .revision
        .checked_add(1)
        .filter(|revision| *revision <= i64::MAX as u64)
        .ok_or(CommandError::Rejected("Revision limit reached"))?;
    crate::storage::encode(&candidate)
        .map_err(|_| CommandError::Rejected("Reminders document is too large"))?;
    *doc = candidate;
    Ok(())
}

fn allocate_id(doc: &Document) -> Result<(ReminderId, ReminderId), CommandError> {
    if doc.reminders.len() >= MAX_REMINDERS || doc.next_reminder_id == 0 {
        return Err(CommandError::Rejected("Reminder limit reached"));
    }
    let next = doc
        .next_reminder_id
        .checked_add(1)
        .ok_or(CommandError::Rejected("Reminder limit reached"))?;
    Ok((doc.next_reminder_id, next))
}

fn apply_candidate(doc: &mut Document, command: Command, now: Now) -> Result<(), CommandError> {
    match command {
        Command::Create {
            title,
            list_id,
            due,
            flagged,
        } => {
            let title = normalize_title(&title);
            if title.is_empty() {
                return Err(CommandError::NoOp);
            }
            let (id, next_id) = allocate_id(doc)?;
            let order = doc.next_order_in(list_id);
            let value = Reminder {
                id,
                list_id,
                title,
                notes: String::new(),
                due,
                flagged,
                priority: Priority::None,
                order,
                completed_at: None,
            };
            validate_reminder_fields(&value, &doc.lists)?;
            doc.reminders.push(value);
            doc.next_reminder_id = next_id;
            Ok(())
        }
        Command::ApplyDraft(draft) => {
            let mut value = draft.value;
            value.title = normalize_title(&value.title);
            validate_reminder_fields(&value, &doc.lists)?;
            if let Some(id) = draft.original_id {
                let Some(existing) = doc.reminder(id).cloned() else {
                    return Err(CommandError::Missing);
                };
                if existing.list_id != value.list_id {
                    value.order = doc.next_order_in(value.list_id);
                } else {
                    value.order = existing.order;
                }
                value.id = id;
                value.completed_at = existing.completed_at;
                if existing == value {
                    return Err(CommandError::NoOp);
                }
                if let Some(slot) = doc.reminder_mut(id) {
                    *slot = value;
                }
                Ok(())
            } else {
                let (id, next_id) = allocate_id(doc)?;
                value.id = id;
                value.order = doc.next_order_in(value.list_id);
                value.completed_at = None;
                doc.reminders.push(value);
                doc.next_reminder_id = next_id;
                Ok(())
            }
        }
        Command::SetCompleted { id, completed } => {
            if completed && (!day_in_range(now.day) || now.minute > 1439) {
                return Err(CommandError::Rejected("Invalid completion time"));
            }
            let Some(item) = doc.reminder_mut(id) else {
                return Err(CommandError::Missing);
            };
            match (item.completed_at.is_some(), completed) {
                (true, true) | (false, false) => Err(CommandError::NoOp),
                (false, true) => {
                    item.completed_at = Some(CompletedAt {
                        day: now.day,
                        minute: now.minute,
                    });
                    Ok(())
                }
                (true, false) => {
                    item.completed_at = None;
                    Ok(())
                }
            }
        }
        Command::SetFlagged { id, flagged } => {
            let Some(item) = doc.reminder_mut(id) else {
                return Err(CommandError::Missing);
            };
            if item.flagged == flagged {
                return Err(CommandError::NoOp);
            }
            item.flagged = flagged;
            Ok(())
        }
        Command::MoveToList { id, list_id } => {
            if doc.list(list_id).is_none() {
                return Err(CommandError::Rejected("Unknown list"));
            }
            let Some(item) = doc.reminder(id) else {
                return Err(CommandError::Missing);
            };
            if item.list_id == list_id {
                return Err(CommandError::NoOp);
            }
            let order = doc.next_order_in(list_id);
            let item = doc.reminder_mut(id).unwrap();
            item.list_id = list_id;
            item.order = order;
            Ok(())
        }
        Command::Delete { id } => {
            let Some(index) = doc.reminders.iter().position(|item| item.id == id) else {
                return Err(CommandError::Missing);
            };
            doc.reminders.remove(index);
            Ok(())
        }
        Command::SetShowCompleted { show } => {
            if doc.show_completed == show {
                return Err(CommandError::NoOp);
            }
            doc.show_completed = show;
            Ok(())
        }
    }
}

pub fn blank_draft(defaults: &CreateDefaults) -> Draft {
    Draft {
        original_id: None,
        value: Reminder {
            id: 0,
            list_id: defaults.list_id,
            title: String::new(),
            notes: String::new(),
            due: defaults.due,
            flagged: defaults.flagged,
            priority: Priority::None,
            order: 0,
            completed_at: None,
        },
    }
}

pub fn draft_from(item: &Reminder) -> Draft {
    Draft {
        original_id: Some(item.id),
        value: item.clone(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavState {
    pub layout: LayoutMode,
    pub filter: Filter,
    pub routes: Vec<Route>,
    pub selected: Option<ReminderId>,
    pub has_draft: bool,
}

impl Default for NavState {
    fn default() -> Self {
        Self::new()
    }
}

impl NavState {
    pub fn new() -> Self {
        Self {
            layout: LayoutMode::Wide,
            filter: Filter::Today,
            routes: vec![Route::List(Filter::Today)],
            selected: None,
            has_draft: false,
        }
    }

    pub fn resize(&mut self, layout: LayoutMode) {
        if layout == self.layout {
            return;
        }
        let from = self.layout;
        self.layout = layout;
        if from.is_wide() && layout.is_compact() {
            if self.has_draft {
                self.routes = vec![Route::List(self.filter), Route::Detail];
            } else {
                self.routes = vec![Route::List(self.filter)];
            }
        } else if from.is_compact() && layout.is_wide() {
            if self.routes.first() == Some(&Route::Home) && self.routes.len() == 1 {
                self.filter = Filter::Today;
                self.routes = vec![Route::List(Filter::Today)];
            } else if self.has_draft {
                self.routes = vec![Route::List(self.filter), Route::Detail];
            } else {
                self.routes = vec![Route::List(self.filter)];
            }
        }
    }

    pub fn open_list(&mut self, filter: Filter) {
        self.filter = filter;
        self.selected = None;
        if self.layout.is_compact() {
            self.routes = vec![Route::List(filter)];
        } else {
            self.routes = vec![Route::List(filter)];
        }
        self.has_draft = false;
    }

    pub fn open_home(&mut self) {
        self.routes = vec![Route::Home];
        self.has_draft = false;
        self.selected = None;
    }

    pub fn open_detail(&mut self, id: Option<ReminderId>) {
        self.selected = id.or(self.selected);
        self.has_draft = true;
        if self.layout.is_compact() {
            if self.routes.last() != Some(&Route::Detail) {
                if self.routes.is_empty() || matches!(self.routes.last(), Some(Route::Home)) {
                    self.routes = vec![Route::List(self.filter), Route::Detail];
                } else {
                    self.routes.push(Route::Detail);
                }
            }
        } else if !self.routes.iter().any(|r| matches!(r, Route::Detail)) {
            self.routes.push(Route::Detail);
        }
    }

    pub fn close_detail(&mut self) {
        self.has_draft = false;
        self.routes.retain(|r| !matches!(r, Route::Detail));
        if self.routes.is_empty() {
            self.routes.push(Route::List(self.filter));
        }
    }

    pub fn back(&mut self) {
        match self.routes.last().copied() {
            Some(Route::Detail) => self.close_detail(),
            Some(Route::List(_)) if self.layout.is_compact() => self.open_home(),
            _ => {}
        }
    }

    pub fn current(&self) -> Route {
        self.routes.last().copied().unwrap_or(Route::Home)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactNavOp {
    PopToRoot,
    PushList,
    PushDetail,
    PopToList,
}

/// One stack step toward `want`. Home → Detail records List first so the
/// following step can push Detail after the transition completes.
pub fn compact_nav_step(
    visual: Route,
    want: Route,
    filter: Filter,
) -> Option<(CompactNavOp, Route)> {
    if visual == want {
        return None;
    }
    match (visual, want) {
        (_, Route::Home) => Some((CompactNavOp::PopToRoot, Route::Home)),
        (Route::Home, Route::List(list)) => Some((CompactNavOp::PushList, Route::List(list))),
        (Route::Home, Route::Detail) => Some((CompactNavOp::PushList, Route::List(filter))),
        (Route::List(_), Route::Detail) => Some((CompactNavOp::PushDetail, Route::Detail)),
        (Route::Detail, Route::List(list)) => Some((CompactNavOp::PopToList, Route::List(list))),
        (Route::List(_), Route::List(_)) => None,
        (Route::Detail, Route::Detail) => None,
    }
}

pub fn dated_incomplete<'a>(doc: &'a Document) -> Vec<&'a Reminder> {
    let mut items: Vec<_> = doc
        .reminders
        .iter()
        .filter(|item| !item.is_completed() && item.due.is_some())
        .collect();
    items.sort_by(|a, b| due_cmp(a, b));
    items
}

pub fn due_within<'a>(doc: &'a Document, now: Now, days: i32) -> Vec<&'a Reminder> {
    let limit = now.day + days;
    dated_incomplete(doc)
        .into_iter()
        .filter(|item| item.due.map(|d| d.day < limit).unwrap_or(false))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seed::seed;
    use makepad_civil_time::{self as civil, from_ymd};

    fn doc() -> (Document, Now) {
        let day = from_ymd(2026, 9, 9);
        (
            seed(day),
            Now {
                day,
                minute: 12 * 60,
            },
        )
    }

    #[test]
    fn every_filter_membership_and_completed_visibility_leaves_open_counts() {
        let (mut document, now) = doc();
        let before = counts(&document, now);
        document.show_completed = true;
        let after = counts(&document, now);
        assert_eq!(before, after);
        let today = project(&document, Filter::Today, now);
        assert_eq!(today.active_count, 8);
        let scheduled = project(&document, Filter::Scheduled, now);
        assert_eq!(scheduled.active_count, 20);
        let all = project(&document, Filter::All, now);
        assert_eq!(all.active_count, 29);
        let flagged = project(&document, Filter::Flagged, now);
        assert_eq!(flagged.active_count, 6);
        let completed = project(&document, Filter::Completed, now);
        assert_eq!(completed.active_count, 6);
        assert_eq!(
            project(&document, Filter::List(GROCERIES_LIST_ID), now).active_count,
            8
        );
        assert_eq!(
            project(&document, Filter::List(WORK_LIST_ID), now).active_count,
            8
        );
        assert_eq!(
            project(&document, Filter::List(HOME_LIST_ID), now).active_count,
            7
        );
        assert_eq!(
            project(&document, Filter::List(TRAVEL_LIST_ID), now).active_count,
            6
        );
        let today_footer = project(&document, Filter::Today, now).completed_count;
        assert!(today_footer > 0);
    }

    #[test]
    fn due_ordering_undated_all_day_and_id_ties() {
        let now = Now {
            day: from_ymd(2026, 1, 1),
            minute: 0,
        };
        let mut document = seed(now.day);
        document.reminders.clear();
        document.reminders.extend([
            Reminder {
                id: 3,
                list_id: HOME_LIST_ID,
                title: "c".into(),
                notes: String::new(),
                due: Some(Due {
                    day: now.day,
                    minute: Some(10),
                }),
                flagged: false,
                priority: Priority::None,
                order: 0,
                completed_at: None,
            },
            Reminder {
                id: 1,
                list_id: HOME_LIST_ID,
                title: "a".into(),
                notes: String::new(),
                due: Some(Due {
                    day: now.day,
                    minute: None,
                }),
                flagged: false,
                priority: Priority::None,
                order: 1,
                completed_at: None,
            },
            Reminder {
                id: 2,
                list_id: HOME_LIST_ID,
                title: "b".into(),
                notes: String::new(),
                due: Some(Due {
                    day: now.day,
                    minute: None,
                }),
                flagged: false,
                priority: Priority::None,
                order: 2,
                completed_at: None,
            },
            Reminder {
                id: 4,
                list_id: HOME_LIST_ID,
                title: "d".into(),
                notes: String::new(),
                due: None,
                flagged: false,
                priority: Priority::None,
                order: 3,
                completed_at: None,
            },
        ]);
        let ids: Vec<_> = project(&document, Filter::Scheduled, now)
            .groups
            .iter()
            .flat_map(|g| g.items.iter().map(|i| i.id))
            .collect();
        assert_eq!(ids, vec![1, 2, 3]);
        let all_ids: Vec<_> = project(&document, Filter::All, now)
            .groups
            .iter()
            .flat_map(|g| g.items.iter().map(|i| i.id))
            .collect();
        assert_eq!(all_ids.last().copied(), Some(4));
    }

    #[test]
    fn overdue_boundaries() {
        let today = from_ymd(2026, 9, 9);
        let item = |due: Option<Due>| Reminder {
            id: 1,
            list_id: HOME_LIST_ID,
            title: "x".into(),
            notes: String::new(),
            due,
            flagged: false,
            priority: Priority::None,
            order: 0,
            completed_at: None,
        };
        let now = Now {
            day: today,
            minute: 10 * 60,
        };
        assert!(is_overdue(
            &item(Some(Due {
                day: today - 1,
                minute: None
            })),
            now
        ));
        assert!(!is_overdue(
            &item(Some(Due {
                day: today,
                minute: None
            })),
            now
        ));
        assert!(is_overdue(
            &item(Some(Due {
                day: today,
                minute: Some(10 * 60 - 1)
            })),
            now
        ));
        assert!(!is_overdue(
            &item(Some(Due {
                day: today,
                minute: Some(10 * 60)
            })),
            now
        ));
        assert!(!is_overdue(
            &item(Some(Due {
                day: today,
                minute: Some(10 * 60 + 1)
            })),
            now
        ));
        let midnight = Now {
            day: today,
            minute: 0,
        };
        assert!(!is_overdue(
            &item(Some(Due {
                day: today,
                minute: Some(0)
            })),
            midnight
        ));
        assert!(is_overdue(
            &item(Some(Due {
                day: today,
                minute: Some(0)
            })),
            Now {
                day: today,
                minute: 1
            }
        ));
        let mut completed = item(Some(Due {
            day: today - 1,
            minute: None,
        }));
        completed.completed_at = Some(CompletedAt {
            day: today,
            minute: 0,
        });
        assert!(!is_overdue(&completed, now));
        assert!(!is_overdue(&item(None), now));
    }

    #[test]
    fn civil_date_integration_leap_invalid_and_offsets() {
        assert_eq!(civil::parse_iso("2024-02-29"), Some(from_ymd(2024, 2, 29)));
        assert_eq!(civil::parse_iso("2023-02-29"), None);
        assert_eq!(
            civil::add_days(from_ymd(2024, 2, 28), 1),
            from_ymd(2024, 2, 29)
        );
        assert_eq!(
            civil::add_days(from_ymd(2024, 12, 31), 1),
            from_ymd(2025, 1, 1)
        );
        assert!(parse_date("2024-02-29").is_ok());
        assert!(parse_date("1900-01-01").is_ok());
        assert!(parse_date("2199-12-31").is_ok());
        assert!(parse_date("1899-12-31").is_err());
        assert!(parse_date("2200-01-01").is_err());
    }

    #[test]
    fn complete_reopen_preserves_fields_and_repeats_are_noops() {
        let (mut document, now) = doc();
        let before = document.reminder(12).unwrap().clone();
        assert!(apply(
            &mut document,
            Command::SetCompleted {
                id: 12,
                completed: true
            },
            now
        )
        .is_ok());
        let after = document.reminder(12).unwrap().clone();
        assert_eq!(after.title, before.title);
        assert_eq!(after.notes, before.notes);
        assert_eq!(after.due, before.due);
        assert_eq!(after.flagged, before.flagged);
        assert_eq!(after.priority, before.priority);
        assert_eq!(after.order, before.order);
        assert!(after.completed_at.is_some());
        let rev = document.revision;
        assert_eq!(
            apply(
                &mut document,
                Command::SetCompleted {
                    id: 12,
                    completed: true
                },
                now
            ),
            Err(CommandError::NoOp)
        );
        assert_eq!(document.revision, rev);
        assert!(apply(
            &mut document,
            Command::SetCompleted {
                id: 12,
                completed: false
            },
            now
        )
        .is_ok());
        let reopened = document.reminder(12).unwrap();
        assert!(reopened.completed_at.is_none());
        assert_eq!(reopened.due, before.due);
        assert_eq!(reopened.notes, before.notes);
    }

    #[test]
    fn create_defaults_and_title_limits() {
        let now = Now {
            day: from_ymd(2026, 9, 9),
            minute: 0,
        };
        assert_eq!(
            create_defaults(Filter::List(GROCERIES_LIST_ID), now)
                .unwrap()
                .list_id,
            GROCERIES_LIST_ID
        );
        assert_eq!(
            create_defaults(Filter::Today, now).unwrap().due,
            Some(Due {
                day: now.day,
                minute: None
            })
        );
        assert!(create_defaults(Filter::Flagged, now).unwrap().flagged);
        assert!(create_defaults(Filter::Completed, now).is_none());
        let mut document = seed(now.day);
        let rev = document.revision;
        assert_eq!(
            apply(
                &mut document,
                Command::Create {
                    title: "  ".into(),
                    list_id: HOME_LIST_ID,
                    due: None,
                    flagged: false
                },
                now
            ),
            Err(CommandError::NoOp)
        );
        assert_eq!(document.revision, rev);
        assert!(apply(
            &mut document,
            Command::Create {
                title: "Buy tea".into(),
                list_id: HOME_LIST_ID,
                due: None,
                flagged: false
            },
            now
        )
        .is_ok());
        assert_eq!(document.next_reminder_id, 37);
        let long = "x".repeat(MAX_TITLE_CHARS + 1);
        assert!(apply(
            &mut document,
            Command::Create {
                title: long,
                list_id: HOME_LIST_ID,
                due: None,
                flagged: false
            },
            now
        )
        .is_err());
        document.next_reminder_id = 0;
        assert_eq!(
            apply(
                &mut document,
                Command::Create {
                    title: "nope".into(),
                    list_id: HOME_LIST_ID,
                    due: None,
                    flagged: false
                },
                now
            ),
            Err(CommandError::Rejected("Reminder limit reached"))
        );
    }

    #[test]
    fn draft_done_is_atomic_and_cancel_changes_nothing() {
        let (mut document, now) = doc();
        let rev = document.revision;
        let mut draft = draft_from(document.reminder(12).unwrap());
        draft.value.title.clear();
        assert!(apply(&mut document, Command::ApplyDraft(draft), now).is_err());
        assert_eq!(document.revision, rev);
        assert_eq!(
            document.reminder(12).unwrap().title,
            "Review release checklist"
        );
        let mut draft = draft_from(document.reminder(12).unwrap());
        draft.value.title = "Review the checklist".into();
        assert!(apply(&mut document, Command::ApplyDraft(draft), now).is_ok());
        assert_eq!(document.reminder(12).unwrap().title, "Review the checklist");
        assert_eq!(document.revision, rev + 1);
    }

    #[test]
    fn move_and_delete_update_projections_and_missing_ids_fail() {
        let (mut document, now) = doc();
        let before_home = counts(&document, now).home;
        let before_work = counts(&document, now).work;
        assert!(apply(
            &mut document,
            Command::MoveToList {
                id: 12,
                list_id: HOME_LIST_ID
            },
            now
        )
        .is_ok());
        let after = counts(&document, now);
        assert_eq!(after.home, before_home + 1);
        assert_eq!(after.work, before_work - 1);
        assert_eq!(document.reminder(12).unwrap().list_id, HOME_LIST_ID);
        assert_eq!(
            apply(&mut document, Command::Delete { id: 999 }, now),
            Err(CommandError::Missing)
        );
        assert!(apply(&mut document, Command::Delete { id: 12 }, now).is_ok());
        assert!(document.reminder(12).is_none());
        assert_eq!(
            apply(
                &mut document,
                Command::MoveToList {
                    id: 12,
                    list_id: WORK_LIST_ID
                },
                now
            ),
            Err(CommandError::Missing)
        );
    }

    #[test]
    fn layout_thresholds_and_invalid_geometry() {
        assert_eq!(decide_layout(402.0, 780.0, None), LayoutMode::Compact);
        assert_eq!(decide_layout(699.0, 780.0, None), LayoutMode::Compact);
        assert_eq!(decide_layout(700.0, 800.0, None), LayoutMode::Wide);
        assert_eq!(decide_layout(1240.0, 800.0, None), LayoutMode::Wide);
        assert_eq!(decide_layout(874.0, 300.0, None), LayoutMode::WideShort);
        assert_eq!(
            decide_layout(f64::NAN, 800.0, Some(LayoutMode::Compact)),
            LayoutMode::Compact
        );
        assert_eq!(
            decide_layout(0.0, 800.0, Some(LayoutMode::Wide)),
            LayoutMode::Wide
        );
        assert_eq!(row_height(false, 1, false, false), 44.0);
        assert_eq!(row_height(true, 1, false, false), 52.0);
    }

    #[test]
    fn route_history_resize_and_detail_pops_to_list() {
        let mut nav = NavState::new();
        nav.open_list(Filter::List(WORK_LIST_ID));
        nav.open_detail(Some(12));
        nav.back();
        assert_eq!(nav.current(), Route::List(Filter::List(WORK_LIST_ID)));
        assert!(!matches!(nav.current(), Route::Home));
        nav.open_detail(Some(12));
        nav.resize(LayoutMode::Compact);
        assert_eq!(
            nav.routes,
            vec![Route::List(Filter::List(WORK_LIST_ID)), Route::Detail]
        );
        nav.resize(LayoutMode::Wide);
        assert!(nav.has_draft);
        nav.close_detail();
        nav.resize(LayoutMode::Compact);
        nav.open_home();
        nav.resize(LayoutMode::Wide);
        assert_eq!(nav.filter, Filter::Today);
        assert_eq!(nav.current(), Route::List(Filter::Today));
    }

    #[test]
    fn home_new_reaches_detail_through_an_intermediate_list_route() {
        let filter = Filter::Today;
        let (op, next) = compact_nav_step(Route::Home, Route::Detail, filter).unwrap();
        assert_eq!(op, CompactNavOp::PushList);
        assert_eq!(next, Route::List(filter));
        assert_ne!(
            next,
            Route::Detail,
            "recording Detail here would skip the second push"
        );
        let (op, next) = compact_nav_step(next, Route::Detail, filter).unwrap();
        assert_eq!(op, CompactNavOp::PushDetail);
        assert_eq!(next, Route::Detail);
        assert!(compact_nav_step(Route::Detail, Route::Detail, filter).is_none());
        let (op, next) =
            compact_nav_step(Route::Home, Route::Detail, Filter::List(WORK_LIST_ID)).unwrap();
        assert_eq!(
            (op, next),
            (
                CompactNavOp::PushList,
                Route::List(Filter::List(WORK_LIST_ID))
            )
        );
        nav_from_wide_draft_maps_to_the_same_two_steps();
    }

    fn nav_from_wide_draft_maps_to_the_same_two_steps() {
        let mut nav = NavState::new();
        nav.open_detail(Some(12));
        nav.resize(LayoutMode::Compact);
        assert_eq!(nav.routes, vec![Route::List(Filter::Today), Route::Detail]);
        let (op, next) = compact_nav_step(Route::Home, nav.current(), nav.filter).unwrap();
        assert_eq!(
            (op, next),
            (CompactNavOp::PushList, Route::List(Filter::Today))
        );
        let (op, next) = compact_nav_step(next, nav.current(), nav.filter).unwrap();
        assert_eq!((op, next), (CompactNavOp::PushDetail, Route::Detail));
    }

    #[test]
    fn mode_specific_geometry_and_wrapped_rows() {
        assert_eq!(smart_tile_height(LayoutMode::Compact), 88.0);
        assert_eq!(smart_tile_height(LayoutMode::Wide), 76.0);
        assert_eq!(smart_tile_height(LayoutMode::WideShort), 68.0);
        assert_eq!(completed_tile_height(LayoutMode::Compact), 56.0);
        assert_eq!(completed_tile_height(LayoutMode::Wide), 52.0);
        assert_eq!(completed_tile_height(LayoutMode::WideShort), 44.0);
        assert_eq!(personal_row_height(LayoutMode::Compact), 56.0);
        assert_eq!(personal_row_height(LayoutMode::Wide), 44.0);
        assert_eq!(LIST_PICKER_ITEM_HEIGHT, 44.0);
        assert_eq!(MINUTE_TICK_SECS, 60.0);
        assert!(row_height(false, 2, true, true) > row_height(false, 1, false, false));
        assert!(row_height(true, 2, true, true) > row_height(true, 1, false, false));
        assert_eq!(row_height(false, 2, true, true) % 4.0, 0.0);
        assert_eq!(row_height(true, 2, true, true) % 4.0, 0.0);
    }

    #[test]
    fn cancel_never_issues_a_command() {
        let (document, _) = doc();
        let before = document.clone();
        let _draft = draft_from(document.reminder(12).unwrap());
        assert_eq!(document, before);
    }
    fn ids(document: &Document, filter: Filter, now: Now) -> Vec<ReminderId> {
        project(document, filter, now)
            .groups
            .iter()
            .flat_map(|group| group.items.iter().map(|item| item.id))
            .collect()
    }

    #[test]
    fn exact_filter_memberships_and_completed_footers() {
        let (mut document, now) = doc();
        let cases = [
            (
                Filter::Today,
                vec![11, 21, 1, 2, 12, 13, 3, 22],
                vec![9, 19, 35, 10, 28, 20],
            ),
            (
                Filter::Scheduled,
                vec![
                    11, 21, 1, 2, 12, 13, 3, 22, 7, 29, 14, 4, 24, 30, 15, 25, 16, 33, 17, 34,
                ],
                vec![20, 10, 28, 9, 19, 35],
            ),
            (
                Filter::All,
                vec![
                    1, 2, 3, 4, 5, 6, 7, 8, 11, 12, 13, 14, 15, 16, 17, 18, 21, 22, 23, 24, 25, 26,
                    27, 29, 30, 31, 32, 33, 34,
                ],
                vec![9, 10, 19, 20, 28, 35],
            ),
            (Filter::Flagged, vec![4, 11, 12, 17, 21, 29], vec![]),
            (Filter::Completed, vec![9, 19, 35, 10, 28, 20], vec![]),
            (Filter::List(1), vec![1, 2, 3, 4, 5, 6, 7, 8], vec![9, 10]),
            (
                Filter::List(2),
                vec![11, 12, 13, 14, 15, 16, 17, 18],
                vec![19, 20],
            ),
            (Filter::List(3), vec![21, 22, 23, 24, 25, 26, 27], vec![28]),
            (Filter::List(4), vec![29, 30, 31, 32, 33, 34], vec![35]),
        ];
        let before = counts(&document, now);
        for show in [false, true] {
            document.show_completed = show;
            for (filter, active, footer) in &cases {
                let projection = project(&document, *filter, now);
                assert_eq!(ids(&document, *filter, now), *active, "{filter:?}");
                assert_eq!(
                    projection
                        .completed_groups
                        .iter()
                        .flat_map(|group| group.items.iter().map(|item| item.id))
                        .collect::<Vec<_>>(),
                    *footer,
                    "{filter:?}"
                );
                assert_eq!(projection.counts, before);
                let visible: Vec<_> = flatten_rows(&projection, *filter, show)
                    .iter()
                    .filter_map(|row| match row {
                        ListRow::Item(item) => Some(item.id),
                        _ => None,
                    })
                    .collect();
                let mut expected = active.clone();
                if show {
                    expected.extend(footer);
                }
                assert_eq!(visible, expected);
            }
        }
    }

    #[test]
    fn adversarial_order_ties_ignore_input_order() {
        let (mut document, now) = doc();
        document.reminders.retain(|item| item.id <= 8);
        for item in &mut document.reminders {
            item.order = 0;
            item.flagged = true;
            item.due = match item.id {
                1 | 2 => Some(Due {
                    day: now.day,
                    minute: None,
                }),
                3 | 4 => Some(Due {
                    day: now.day,
                    minute: Some(600),
                }),
                5 => Some(Due {
                    day: now.day - 1,
                    minute: Some(1439),
                }),
                6 => Some(Due {
                    day: now.day + 1,
                    minute: None,
                }),
                _ => None,
            };
        }
        document.reminders.reverse();
        assert_eq!(ids(&document, Filter::Today, now), vec![5, 1, 2, 3, 4]);
        assert_eq!(
            ids(&document, Filter::Scheduled, now),
            vec![5, 1, 2, 3, 4, 6]
        );
        for filter in [Filter::All, Filter::Flagged, Filter::List(1)] {
            assert_eq!(ids(&document, filter, now), vec![1, 2, 3, 4, 5, 6, 7, 8]);
        }
        let mut sorted: Vec<_> = document.reminders.iter().collect();
        sorted.sort_by(|a, b| due_cmp(a, b));
        assert_eq!(
            sorted.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![5, 1, 2, 3, 4, 6, 7, 8]
        );
        for item in &mut document.reminders {
            item.completed_at = Some(CompletedAt {
                day: now.day,
                minute: if item.id <= 4 { 600 } else { 601 },
            });
        }
        assert_eq!(
            ids(&document, Filter::Completed, now),
            vec![5, 6, 7, 8, 1, 2, 3, 4]
        );
    }

    #[test]
    fn id_exhaustion_rejects_both_creation_paths_without_mutation() {
        let (mut document, now) = doc();
        document.next_reminder_id = u32::MAX - 1;
        let create = || Command::Create {
            title: "Last ID".into(),
            list_id: 3,
            due: None,
            flagged: false,
        };
        apply(&mut document, create(), now).unwrap();
        assert_eq!(document.next_reminder_id, u32::MAX);
        assert_eq!(
            crate::storage::decode(&crate::storage::encode(&document).unwrap()).unwrap(),
            document
        );
        let before = document.clone();
        let mut draft = blank_draft(&home_create_defaults());
        draft.value.title = "Exhausted".into();
        for command in [create(), Command::ApplyDraft(draft), create()] {
            assert_eq!(
                apply(&mut document, command, now),
                Err(CommandError::Rejected("Reminder limit reached"))
            );
            assert_eq!(document, before);
        }
    }

    #[test]
    fn create_and_draft_share_complete_due_validation() {
        let (mut document, now) = doc();
        let before = document.clone();
        for due in [
            Due {
                day: now.day,
                minute: Some(1440),
            },
            Due {
                day: now.day,
                minute: Some(u16::MAX),
            },
            Due {
                day: from_ymd(2200, 1, 1),
                minute: None,
            },
        ] {
            assert!(apply(
                &mut document,
                Command::Create {
                    title: "Invalid time".into(),
                    list_id: 3,
                    due: Some(due),
                    flagged: false
                },
                now
            )
            .is_err());
            let mut draft = draft_from(document.reminder(12).unwrap());
            draft.value.due = Some(due);
            assert!(apply(&mut document, Command::ApplyDraft(draft), now).is_err());
            assert_eq!(document, before);
        }
        apply(
            &mut document,
            Command::Create {
                title: "Midnight boundary".into(),
                list_id: 3,
                due: Some(Due {
                    day: now.day,
                    minute: Some(1439),
                }),
                flagged: false,
            },
            now,
        )
        .unwrap();
        assert_eq!(
            crate::storage::decode(&crate::storage::encode(&document).unwrap()).unwrap(),
            document
        );
    }

    #[test]
    fn oversized_draft_is_rejected_before_committing_or_incrementing_revision() {
        let (mut document, now) = doc();
        let template = document.reminder(12).unwrap().clone();
        document.reminders = (1..=498)
            .map(|id| Reminder {
                id,
                notes: String::new(),
                ..template.clone()
            })
            .collect();
        document.next_reminder_id = 499;
        let mut overflow_id = 0;
        for id in 1..=498 {
            document.reminder_mut(id).unwrap().notes = "n".repeat(MAX_NOTES_BYTES);
            if crate::storage::encode(&document).is_err() {
                document.reminder_mut(id).unwrap().notes.clear();
                overflow_id = id;
                break;
            }
        }
        assert!(
            overflow_id > 0,
            "498 individually valid reminders exceed 2 MiB"
        );
        let before = document.clone();
        assert!(crate::storage::encode(&before).is_ok());
        let mut draft = draft_from(document.reminder(overflow_id).unwrap());
        draft.value.notes = "n".repeat(MAX_NOTES_BYTES);
        assert_eq!(
            apply(&mut document, Command::ApplyDraft(draft), now),
            Err(CommandError::Rejected("Reminders document is too large"))
        );
        assert_eq!(document, before);
        let mut draft = blank_draft(&home_create_defaults());
        draft.value.title = "Another large note".into();
        draft.value.notes = "n".repeat(MAX_NOTES_BYTES);
        assert!(apply(&mut document, Command::ApplyDraft(draft), now).is_err());
        assert_eq!(document, before);
    }

    #[test]
    fn create_defaults_unicode_and_reminder_count_boundaries() {
        let (mut document, now) = doc();
        for (filter, list_id, dated, flagged) in [
            (Filter::Today, 3, true, false),
            (Filter::Scheduled, 3, true, false),
            (Filter::All, 3, false, false),
            (Filter::Flagged, 3, false, true),
            (Filter::List(1), 1, false, false),
            (Filter::List(2), 2, false, false),
            (Filter::List(3), 3, false, false),
            (Filter::List(4), 4, false, false),
        ] {
            let defaults = create_defaults(filter, now).unwrap();
            assert_eq!(
                (defaults.list_id, defaults.due, defaults.flagged),
                (
                    list_id,
                    dated.then_some(Due {
                        day: now.day,
                        minute: None
                    }),
                    flagged
                )
            );
        }
        let defaults = home_create_defaults();
        assert_eq!(
            (defaults.list_id, defaults.due, defaults.flagged),
            (3, None, false)
        );
        assert!(create_defaults(Filter::Completed, now).is_none());
        for length in [240, 241] {
            let before = document.clone();
            let result = apply(
                &mut document,
                Command::Create {
                    title: "界".repeat(length),
                    list_id: 3,
                    due: None,
                    flagged: false,
                },
                now,
            );
            assert_eq!(result.is_ok(), length == 240);
            if result.is_err() {
                assert_eq!(document, before);
            }
        }
        let template = document.reminders[0].clone();
        document.reminders = (1..=MAX_REMINDERS as u32)
            .map(|id| Reminder {
                id,
                ..template.clone()
            })
            .collect();
        document.next_reminder_id = MAX_REMINDERS as u32 + 1;
        let before = document.clone();
        assert!(apply(
            &mut document,
            Command::Create {
                title: "Too many".into(),
                list_id: 3,
                due: None,
                flagged: false
            },
            now
        )
        .is_err());
        let mut draft = blank_draft(&home_create_defaults());
        draft.value.title = "Too many".into();
        assert!(apply(&mut document, Command::ApplyDraft(draft), now).is_err());
        assert_eq!(document, before);
    }

    #[test]
    fn mutations_update_all_affected_projections_and_counts() {
        let (mut document, now) = doc();
        let original = document.reminder(12).unwrap().clone();
        let old_order = document.next_order_in(3);
        apply(
            &mut document,
            Command::MoveToList { id: 12, list_id: 3 },
            now,
        )
        .unwrap();
        assert!(!ids(&document, Filter::List(2), now).contains(&12));
        assert_eq!(ids(&document, Filter::List(3), now).last(), Some(&12));
        let moved = document.reminder(12).unwrap();
        assert_eq!(
            moved,
            &Reminder {
                list_id: 3,
                order: old_order,
                ..original.clone()
            }
        );
        for filter in [
            Filter::Today,
            Filter::Scheduled,
            Filter::Flagged,
            Filter::All,
        ] {
            let projection = project(&document, filter, now);
            let item = projection
                .groups
                .iter()
                .flat_map(|group| &group.items)
                .find(|item| item.id == 12)
                .unwrap();
            assert_eq!(item.list_name, "Home");
        }
        apply(
            &mut document,
            Command::SetFlagged {
                id: 12,
                flagged: false,
            },
            now,
        )
        .unwrap();
        assert!(!ids(&document, Filter::Flagged, now).contains(&12));
        assert_eq!(counts(&document, now).flagged, 5);
        let before_complete = document.reminder(12).unwrap().clone();
        apply(
            &mut document,
            Command::SetCompleted {
                id: 12,
                completed: true,
            },
            now,
        )
        .unwrap();
        assert_eq!(ids(&document, Filter::Completed, now).first(), Some(&12));
        for filter in [
            Filter::Today,
            Filter::Scheduled,
            Filter::All,
            Filter::List(3),
        ] {
            assert!(!ids(&document, filter, now).contains(&12));
        }
        apply(
            &mut document,
            Command::SetCompleted {
                id: 12,
                completed: false,
            },
            now,
        )
        .unwrap();
        assert_eq!(document.reminder(12).unwrap(), &before_complete);
        apply(&mut document, Command::Delete { id: 12 }, now).unwrap();
        for filter in [
            Filter::Today,
            Filter::Scheduled,
            Filter::All,
            Filter::Flagged,
            Filter::Completed,
            Filter::List(2),
            Filter::List(3),
        ] {
            assert!(!ids(&document, filter, now).contains(&12));
        }
        let after = counts(&document, now);
        assert_eq!(
            (
                after.today,
                after.scheduled,
                after.all,
                after.flagged,
                after.completed,
                after.work,
                after.home
            ),
            (7, 19, 28, 5, 6, 7, 7)
        );
        let before = document.clone();
        for command in [
            Command::Delete { id: 12 },
            Command::MoveToList { id: 12, list_id: 2 },
            Command::SetFlagged {
                id: 12,
                flagged: true,
            },
            Command::SetCompleted {
                id: 12,
                completed: true,
            },
        ] {
            assert_eq!(
                apply(&mut document, command, now),
                Err(CommandError::Missing)
            );
            assert_eq!(document, before);
        }
    }
}

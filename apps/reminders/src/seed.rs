//! Deterministic first-run document. `seed` never reads a clock.

use crate::model::*;
use makepad_civil_time::Day;

struct SeedRow {
    id: ReminderId,
    list_id: ListId,
    title: &'static str,
    notes: &'static str,
    due_offset: Option<i32>,
    due_minute: Option<u16>,
    flagged: bool,
    priority: Priority,
    completed: bool,
}

const ROWS: &[SeedRow] = &[
    SeedRow { id: 1, list_id: GROCERIES_LIST_ID, title: "Oat milk", notes: "", due_offset: Some(0), due_minute: None, flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 2, list_id: GROCERIES_LIST_ID, title: "Cherry tomatoes", notes: "", due_offset: Some(0), due_minute: None, flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 3, list_id: GROCERIES_LIST_ID, title: "Sourdough", notes: "Ask for the sliced loaf", due_offset: Some(0), due_minute: Some(18 * 60), flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 4, list_id: GROCERIES_LIST_ID, title: "Coffee beans", notes: "Whole beans, medium roast", due_offset: Some(2), due_minute: None, flagged: true, priority: Priority::Low, completed: false },
    SeedRow { id: 5, list_id: GROCERIES_LIST_ID, title: "Olive oil", notes: "", due_offset: None, due_minute: None, flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 6, list_id: GROCERIES_LIST_ID, title: "Eggs", notes: "", due_offset: None, due_minute: None, flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 7, list_id: GROCERIES_LIST_ID, title: "Spinach", notes: "", due_offset: Some(1), due_minute: None, flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 8, list_id: GROCERIES_LIST_ID, title: "Rice", notes: "", due_offset: None, due_minute: None, flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 9, list_id: GROCERIES_LIST_ID, title: "Apples", notes: "", due_offset: Some(-1), due_minute: None, flagged: false, priority: Priority::None, completed: true },
    SeedRow { id: 10, list_id: GROCERIES_LIST_ID, title: "Yogurt", notes: "", due_offset: Some(-2), due_minute: None, flagged: false, priority: Priority::None, completed: true },
    SeedRow { id: 11, list_id: WORK_LIST_ID, title: "Send proposal", notes: "Include the revised delivery dates", due_offset: Some(-2), due_minute: Some(16 * 60), flagged: true, priority: Priority::High, completed: false },
    SeedRow { id: 12, list_id: WORK_LIST_ID, title: "Review release checklist", notes: "Check keyboard navigation and compact layout", due_offset: Some(0), due_minute: Some(10 * 60), flagged: true, priority: Priority::High, completed: false },
    SeedRow { id: 13, list_id: WORK_LIST_ID, title: "Reply to Jordan", notes: "", due_offset: Some(0), due_minute: Some(15 * 60), flagged: false, priority: Priority::Medium, completed: false },
    SeedRow { id: 14, list_id: WORK_LIST_ID, title: "Prepare planning notes", notes: "", due_offset: Some(1), due_minute: Some(9 * 60), flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 15, list_id: WORK_LIST_ID, title: "Book design review", notes: "", due_offset: Some(3), due_minute: Some(14 * 60), flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 16, list_id: WORK_LIST_ID, title: "File expenses", notes: "", due_offset: Some(5), due_minute: None, flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 17, list_id: WORK_LIST_ID, title: "Update roadmap", notes: "", due_offset: Some(7), due_minute: None, flagged: true, priority: Priority::Medium, completed: false },
    SeedRow { id: 18, list_id: WORK_LIST_ID, title: "Archive sprint notes", notes: "", due_offset: None, due_minute: None, flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 19, list_id: WORK_LIST_ID, title: "Share meeting recap", notes: "", due_offset: Some(-1), due_minute: None, flagged: false, priority: Priority::None, completed: true },
    SeedRow { id: 20, list_id: WORK_LIST_ID, title: "Submit timesheet", notes: "", due_offset: Some(-3), due_minute: None, flagged: false, priority: Priority::None, completed: true },
    SeedRow { id: 21, list_id: HOME_LIST_ID, title: "Pay water bill", notes: "Reference is on the kitchen noticeboard", due_offset: Some(-1), due_minute: None, flagged: true, priority: Priority::High, completed: false },
    SeedRow { id: 22, list_id: HOME_LIST_ID, title: "Water balcony plants", notes: "", due_offset: Some(0), due_minute: Some(19 * 60), flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 23, list_id: HOME_LIST_ID, title: "Replace hallway bulb", notes: "", due_offset: None, due_minute: None, flagged: false, priority: Priority::Low, completed: false },
    SeedRow { id: 24, list_id: HOME_LIST_ID, title: "Book bicycle service", notes: "Mention the rear brake", due_offset: Some(2), due_minute: None, flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 25, list_id: HOME_LIST_ID, title: "Return library books", notes: "", due_offset: Some(4), due_minute: None, flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 26, list_id: HOME_LIST_ID, title: "Order printer paper", notes: "", due_offset: None, due_minute: None, flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 27, list_id: HOME_LIST_ID, title: "Sort photos", notes: "", due_offset: None, due_minute: None, flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 28, list_id: HOME_LIST_ID, title: "Recycle packaging", notes: "", due_offset: Some(-2), due_minute: None, flagged: false, priority: Priority::None, completed: true },
    SeedRow { id: 29, list_id: TRAVEL_LIST_ID, title: "Check passport expiry", notes: "Check the expiry date before booking", due_offset: Some(1), due_minute: None, flagged: true, priority: Priority::High, completed: false },
    SeedRow { id: 30, list_id: TRAVEL_LIST_ID, title: "Reserve train seats", notes: "Window seats if available", due_offset: Some(3), due_minute: Some(12 * 60), flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 31, list_id: TRAVEL_LIST_ID, title: "Download offline map", notes: "", due_offset: None, due_minute: None, flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 32, list_id: TRAVEL_LIST_ID, title: "Pack charger", notes: "", due_offset: None, due_minute: None, flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 33, list_id: TRAVEL_LIST_ID, title: "Confirm hotel booking", notes: "", due_offset: Some(6), due_minute: None, flagged: false, priority: Priority::None, completed: false },
    SeedRow { id: 34, list_id: TRAVEL_LIST_ID, title: "Choose walking route", notes: "", due_offset: Some(10), due_minute: None, flagged: false, priority: Priority::Low, completed: false },
    SeedRow { id: 35, list_id: TRAVEL_LIST_ID, title: "Save boarding pass", notes: "", due_offset: Some(-1), due_minute: None, flagged: false, priority: Priority::None, completed: true },
];

/// Build the first-run document for civil day `D`. Absolute dates are stored
/// and never shifted on later launches.
pub fn seed(day: Day) -> Document {
    let lists = vec![
        ReminderList { id: GROCERIES_LIST_ID, name: "Groceries".into(), colour: ListColour::Green, order: 0 },
        ReminderList { id: WORK_LIST_ID, name: "Work".into(), colour: ListColour::Blue, order: 1 },
        ReminderList { id: HOME_LIST_ID, name: "Home".into(), colour: ListColour::Orange, order: 2 },
        ReminderList { id: TRAVEL_LIST_ID, name: "Travel".into(), colour: ListColour::Purple, order: 3 },
    ];
    let mut order_in_list = [0u32; 5];
    let reminders = ROWS
        .iter()
        .map(|row| {
            let slot = row.list_id as usize;
            let order = order_in_list[slot];
            order_in_list[slot] += 1;
            let due = row.due_offset.map(|offset| Due {
                day: day + offset,
                minute: row.due_minute,
            });
            let completed_at = if row.completed {
                let due_day = due.map(|d| d.day).unwrap_or(day);
                Some(CompletedAt { day: due_day, minute: 17 * 60 })
            } else {
                None
            };
            Reminder {
                id: row.id,
                list_id: row.list_id,
                title: row.title.into(),
                notes: row.notes.into(),
                due,
                flagged: row.flagged,
                priority: row.priority,
                order,
                completed_at,
            }
        })
        .collect();
    Document {
        schema_version: SCHEMA_VERSION,
        seed_version: SEED_VERSION,
        seed_day: day,
        revision: 1,
        next_reminder_id: 36,
        lists,
        reminders,
        show_completed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{counts, FilterCounts};
    use makepad_civil_time::from_ymd;

    #[test]
    fn seed_is_reproducible_with_fixed_ids_notes_and_counts() {
        let day = from_ymd(2026, 9, 9);
        let a = seed(day);
        let b = seed(day);
        assert_eq!(a, b);
        assert_eq!(a.next_reminder_id, 36);
        assert_eq!(a.revision, 1);
        assert!(!a.show_completed);
        assert_eq!(a.lists.len(), 4);
        assert_eq!(a.reminders.len(), 35);
        assert_eq!(a.reminders.iter().map(|r| r.id).collect::<Vec<_>>(), (1..=35).collect::<Vec<_>>());
        let notes: Vec<_> = a.reminders.iter().filter(|r| !r.notes.is_empty()).map(|r| (r.id, r.notes.as_str())).collect();
        assert_eq!(
            notes,
            vec![
                (3, "Ask for the sliced loaf"),
                (4, "Whole beans, medium roast"),
                (11, "Include the revised delivery dates"),
                (12, "Check keyboard navigation and compact layout"),
                (21, "Reference is on the kitchen noticeboard"),
                (24, "Mention the rear brake"),
                (29, "Check the expiry date before booking"),
                (30, "Window seats if available"),
            ]
        );
        let groceries: Vec<_> = a.reminders.iter().filter(|r| r.list_id == GROCERIES_LIST_ID).map(|r| r.id).collect();
        assert_eq!(groceries, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let now = Now { day, minute: 12 * 60 };
        assert_eq!(
            counts(&a, now),
            FilterCounts { today: 8, scheduled: 20, all: 29, flagged: 6, completed: 6, groceries: 8, work: 8, home: 7, travel: 6 }
        );
    }
}

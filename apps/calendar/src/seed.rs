//! Deterministic fake calendar: 40 masters, recipe version 1.

use crate::model::*;
use makepad_civil_time::{self as civil, Day};

const WORK_TITLES: [&str; 5] = [
    "Planning",
    "Design review",
    "Project check-in",
    "Research session",
    "Team lunch",
];
const HOME_TITLES: [&str; 4] = [
    "Dentist",
    "Dinner with friends",
    "Bike ride",
    "Family visit",
];
const NOTES: [&str; 8] = [
    "Review the updated navigation.",
    "Bring the slides.",
    "Confirm the room.",
    "Pack a charger.",
    "Leave 10 minutes early.",
    "Send notes after.",
    "Optional: walk over.",
    "Check the guest list.",
];
const ONE_OFF_OFFSETS: [i32; 29] = [
    -84, -77, -70, -63, -56, -49, -42, -35, -28, -21, -14, -7, -3, -1, 0, 0, 0, 1, 2, 3, 7, 14,
    21, 28, 35, 42, 56, 70, 84,
];
const BIRTHDAY_OFFSETS: [i32; 6] = [-70, -35, -7, 3, 35, 70];
const BIRTHDAY_NAMES: [&str; 6] = [
    "Maya Chen",
    "Jordan Hale",
    "Priya Nair",
    "Alex Rivera",
    "Sam Okonkwo",
    "Elena Voss",
];

fn note_for(id: u32, title: &str) -> String {
    format!("{} — {}", NOTES[(id as usize - 1) % NOTES.len()], title)
}

fn work_time(id: u32, day: Day) -> Timing {
    // Anchor-day overlap fixtures occupy ids 15–17 (the three 0 offsets).
    match id {
        15 => Timing::Timed {
            start: LocalMinute::from_hm(day, 9, 0).unwrap(),
            end: LocalMinute::from_hm(day, 10, 0).unwrap(),
        },
        16 => Timing::Timed {
            start: LocalMinute::from_hm(day, 9, 30).unwrap(),
            end: LocalMinute::from_hm(day, 10, 30).unwrap(),
        },
        17 => Timing::Timed {
            start: LocalMinute::from_hm(day, 14, 0).unwrap(),
            end: LocalMinute::from_hm(day, 15, 0).unwrap(),
        },
        _ => {
            let hour = 9 + ((id - 1) % 7);
            let hour = if hour == 12 { 13 } else { hour };
            Timing::Timed {
                start: LocalMinute::from_hm(day, hour, 0).unwrap(),
                end: LocalMinute::from_hm(day, hour + 1, 0).unwrap(),
            }
        }
    }
}

/// Window: three calendar months before through three months after `anchor`.
pub fn seed_window(anchor: Day) -> (Day, Day) {
    let start = civil::month_start(civil::add_months(anchor, -3));
    let end = civil::month_end(civil::add_months(anchor, 3));
    (start, end)
}

fn first_monday_on_or_after(day: Day) -> Day {
    let wd = civil::weekday(day);
    if wd == 0 {
        day
    } else {
        day + (7 - wd as i32)
    }
}

/// Pure seed. Recipe version 1. No clock, no random source.
pub fn seed(anchor_today: Day) -> CalendarDocument {
    let (window_start, _window_end) = seed_window(anchor_today);
    let mut events = Vec::with_capacity(40);
    let mut id: u32 = 1;

    for (i, offset) in ONE_OFF_OFFSETS.iter().copied().enumerate() {
        let day = anchor_today + offset;
        if i < 20 {
            let title = WORK_TITLES[(id as usize - 1) % WORK_TITLES.len()].to_string();
            events.push(CalendarEvent {
                id: EventId(id),
                calendar_id: CAL_WORK,
                title: title.clone(),
                timing: work_time(id, day),
                repeat: Repeat::None,
                notes: note_for(id, &title),
            });
        } else if i == 27 {
            // Home event crossing midnight.
            events.push(CalendarEvent {
                id: EventId(id),
                calendar_id: CAL_HOME,
                title: "Late concert".into(),
                timing: Timing::Timed {
                    start: LocalMinute::from_hm(day, 22, 0).unwrap(),
                    end: LocalMinute::from_hm(day + 1, 1, 0).unwrap(),
                },
                repeat: Repeat::None,
                notes: note_for(id, "Late concert"),
            });
        } else if i == 28 {
            // Three-day all-day Home event.
            events.push(CalendarEvent {
                id: EventId(id),
                calendar_id: CAL_HOME,
                title: "Weekend away".into(),
                timing: Timing::AllDay {
                    start: day,
                    end_exclusive: day + 3,
                },
                repeat: Repeat::None,
                notes: note_for(id, "Weekend away"),
            });
        } else {
            let title = HOME_TITLES[(id as usize) % HOME_TITLES.len()].to_string();
            let hour = 18 + ((id % 3) as u32);
            events.push(CalendarEvent {
                id: EventId(id),
                calendar_id: CAL_HOME,
                title: title.clone(),
                timing: Timing::Timed {
                    start: LocalMinute::from_hm(day, hour.min(21), 0).unwrap(),
                    end: LocalMinute::from_hm(day, (hour.min(21) + 1).min(23), 0).unwrap(),
                },
                repeat: Repeat::None,
                notes: note_for(id, &title),
            });
        }
        id += 1;
    }

    for (i, offset) in BIRTHDAY_OFFSETS.iter().copied().enumerate() {
        let day = anchor_today + offset;
        let name = BIRTHDAY_NAMES[i];
        let title = format!("{name}'s birthday");
        events.push(CalendarEvent {
            id: EventId(id),
            calendar_id: CAL_BIRTHDAYS,
            title: title.clone(),
            timing: Timing::AllDay {
                start: day,
                end_exclusive: day + 1,
            },
            repeat: Repeat::Yearly,
            notes: note_for(id, &title),
        });
        id += 1;
    }

    let (ay, am, _) = civil::to_ymd(anchor_today);
    let prev = civil::add_months(civil::from_ymd(ay, am, 1), -1);
    let next = civil::add_months(civil::from_ymd(ay, am, 1), 1);
    let (py, pm, _) = civil::to_ymd(prev);
    let (ny, nm, _) = civil::to_ymd(next);
    let holidays = [
        (
            civil::from_ymd(py, pm, 15.min(civil::days_in_month(py, pm))),
            "Demo Independence Day",
        ),
        (
            civil::from_ymd(ay, am, 20.min(civil::days_in_month(ay, am))),
            "Demo Harvest Festival",
        ),
        (
            civil::from_ymd(ny, nm, 12.min(civil::days_in_month(ny, nm))),
            "Demo Founders' Day",
        ),
    ];
    for (day, title) in holidays {
        events.push(CalendarEvent {
            id: EventId(id),
            calendar_id: CAL_HOLIDAYS,
            title: title.into(),
            timing: Timing::AllDay {
                start: day,
                end_exclusive: day + 1,
            },
            repeat: Repeat::None,
            notes: note_for(id, title),
        });
        id += 1;
    }

    let standup_start = first_monday_on_or_after(window_start);
    events.push(CalendarEvent {
        id: EventId(id),
        calendar_id: CAL_WORK,
        title: "Standup".into(),
        timing: Timing::Timed {
            start: LocalMinute::from_hm(standup_start, 9, 30).unwrap(),
            end: LocalMinute::from_hm(standup_start, 9, 45).unwrap(),
        },
        repeat: Repeat::Weekly,
        notes: note_for(id, "Standup"),
    });
    id += 1;

    let (wy, wm, _) = civil::to_ymd(window_start);
    let review_day = civil::from_ymd(wy, wm, 15.min(civil::days_in_month(wy, wm)));
    events.push(CalendarEvent {
        id: EventId(id),
        calendar_id: CAL_WORK,
        title: "Monthly review".into(),
        timing: Timing::Timed {
            start: LocalMinute::from_hm(review_day, 15, 0).unwrap(),
            end: LocalMinute::from_hm(review_day, 16, 0).unwrap(),
        },
        repeat: Repeat::Monthly,
        notes: note_for(id, "Monthly review"),
    });

    CalendarDocument {
        schema_version: SCHEMA_VERSION,
        revision: 1,
        seed_version: SEED_VERSION,
        seed_anchor: anchor_today,
        next_event_id: 41,
        calendars: vec![
            Calendar {
                id: CAL_HOME,
                name: "Home".into(),
                colour: CalendarColour::Home,
                visible: true,
            },
            Calendar {
                id: CAL_WORK,
                name: "Work".into(),
                colour: CalendarColour::Work,
                visible: true,
            },
            Calendar {
                id: CAL_BIRTHDAYS,
                name: "Birthdays".into(),
                colour: CalendarColour::Birthdays,
                visible: true,
            },
            Calendar {
                id: CAL_HOLIDAYS,
                name: "Holidays".into(),
                colour: CalendarColour::Holidays,
                visible: true,
            },
        ],
        events,
        preferences: Preferences {
            wide_mode: CalendarMode::Month,
            default_calendar: CAL_HOME,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::occurrences_for_day;
    use crate::model::validate_document;

    #[test]
    fn seed_is_forty_valid_masters_with_overlap_and_window() {
        let anchor = civil::from_ymd(2026, 9, 9);
        let doc = seed(anchor);
        assert_eq!(doc.events.len(), 40);
        assert_eq!(doc.next_event_id, 41);
        assert_eq!(doc.seed_anchor, anchor);
        assert_eq!(doc.seed_version, 1);
        assert_eq!(doc.calendars.len(), 4);
        assert!(validate_document(&doc).is_ok());
        for (i, event) in doc.events.iter().enumerate() {
            assert_eq!(event.id.0, (i as u32) + 1);
        }
        let ids: std::collections::BTreeSet<_> = doc.events.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), 40);

        let day = occurrences_for_day(&doc, anchor, false, 50);
        let timed: Vec<_> = day
            .items
            .iter()
            .filter(|o| matches!(o.timing, Timing::Timed { .. }))
            .collect();
        assert!(timed.len() >= 3, "anchor day has the overlap fixture");
        let starts: Vec<_> = timed
            .iter()
            .filter_map(|o| match o.timing {
                Timing::Timed { start, .. } => Some(start.minute),
                _ => None,
            })
            .collect();
        assert!(starts.contains(&(9 * 60)));
        assert!(starts.contains(&(9 * 60 + 30)));
        assert!(starts.contains(&(14 * 60)));

        assert!(doc.events.iter().any(|e| matches!(
            e.timing,
            Timing::Timed { start, end } if end.day == start.day + 1
        )));
        assert!(doc.events.iter().any(|e| matches!(
            e.timing,
            Timing::AllDay { start, end_exclusive } if end_exclusive - start == 3
        )));

        let (ws, we) = seed_window(anchor);
        let birthdays: Vec<_> = doc
            .events
            .iter()
            .filter(|e| e.calendar_id == CAL_BIRTHDAYS)
            .collect();
        assert_eq!(birthdays.len(), 6);
        for b in birthdays {
            assert_eq!(b.repeat, Repeat::Yearly);
            let d = b.timing.start_day();
            assert!(d >= ws - 1 && d <= we + 1);
        }
        let holidays: Vec<_> = doc
            .events
            .iter()
            .filter(|e| e.calendar_id == CAL_HOLIDAYS)
            .collect();
        assert_eq!(holidays.len(), 3);
        for h in &holidays {
            assert!(h.title.starts_with("Demo "), "{}", h.title);
            let (y, m, _) = civil::to_ymd(h.timing.start_day());
            let (ay, am, _) = civil::to_ymd(anchor);
            let delta = (y * 12 + m as i32) - (ay * 12 + am as i32);
            assert!((-1..=1).contains(&delta), "holiday month {y}-{m}");
        }
        assert!(doc.events.iter().any(|e| e.title == "Standup" && e.repeat == Repeat::Weekly));
        assert!(doc
            .events
            .iter()
            .any(|e| e.title == "Monthly review" && e.repeat == Repeat::Monthly));
        assert_eq!(doc.events.iter().filter(|e| e.repeat == Repeat::None && e.calendar_id == CAL_WORK).count(), 20);
        let home_one_offs = doc
            .events
            .iter()
            .filter(|e| e.repeat == Repeat::None && e.calendar_id == CAL_HOME)
            .count();
        assert_eq!(home_one_offs, 9);
    }

    #[test]
    fn seed_is_deterministic() {
        let a = seed(civil::from_ymd(2026, 9, 9));
        let b = seed(civil::from_ymd(2026, 9, 9));
        assert_eq!(a, b);
    }
}

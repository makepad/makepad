//! The calendar story: a month grid, the three ways of choosing days from
//! it, and the two grids above it.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.CalendarOverview = StoryPage{
        StoryNote{text: "A month on seven columns, with the days either side of it drawn dim so the first and last rows are never half empty. It reads no clock: the day marked as today is a property, and an empty one marks nothing."}

        StoryHeading{text: "One day"}
        StoryRow{
            subject := Calendar{
                year: 2026
                month: 9
                today: "2026-09-10"
                selected: "2026-09-10"
            }
        }
        StoryRow{
            day_note := Label{text: "2026-09-10"}
        }

        StoryHeading{text: "Several days"}
        StoryNote{text: "In Multiple every press is a toggle, and the widget keeps the set in date order so a host reading it never has to sort."}
        StoryRow{
            many := Calendar{
                mode: CalendarMode.Multiple
                year: 2026
                month: 9
                selected: "2026-09-03,2026-09-04,2026-09-17"
            }
            many_note := Label{text: "3 days chosen"}
        }

        StoryHeading{text: "A span"}
        StoryNote{text: "In Range the first press anchors an end and the second finishes it. Between the two, moving the pointer shows the span that press would make — without it there is nothing on screen to say which end is held. The ends come out earliest first however they were pressed, and Escape puts a half-made range down again."}
        StoryRow{
            span := Calendar{
                mode: CalendarMode.Range
                year: 2026
                month: 9
                selected: "2026-09-07,2026-09-18"
            }
            span_note := Label{text: "2026-09-07 to 2026-09-18 (12 days)"}
        }

        StoryHeading{text: "Where the week starts"}
        StoryNote{text: "first_day is the weekday a row begins on, counting 0 for Monday. It moves the headings and the grid together; it does not change what iso_week counts, which is Monday-first by definition."}
        StoryRow{
            Calendar{year: 2026 month: 9 first_day: 0}
            Calendar{year: 2026 month: 9 first_day: 6}
        }

        StoryHeading{text: "Days that are closed"}
        StoryNote{text: "min and max are the two ends, and weekdays_off closes a weekday everywhere — 96 is Saturday and Sunday. A closed day cannot be pressed, and the arrow keys step over it rather than parking on it, so the keyboard never sits somewhere Return does nothing."}
        StoryRow{
            bounded := Calendar{
                year: 2026
                month: 9
                min: "2026-09-07"
                max: "2026-09-25"
                weekdays_off: 96
                selected: "2026-09-09"
            }
        }

        StoryHeading{text: "Week numbers"}
        StoryNote{text: "The ISO week down the left. The week a row belongs to is the week of its Thursday, which is why the first row of January can be numbered 52 or 53."}
        StoryRow{
            Calendar{year: 2026 month: 1 show_week_numbers: true}
        }

        StoryHeading{text: "The two steps above a day"}
        StoryNote{text: "The same selection contract on twelve cells. The month grid steps a year at a time; the year grid steps a whole page of twelve, aligned so the two arrows are inverses of each other."}
        StoryRow{
            months := MonthPicker{year: 2026 selected: "2026-09-01"}
            years := YearPicker{year: 2026 selected: "2026-01-01"}
        }
        StoryRow{
            picker_note := Label{text: "September 2026"}
        }

        StoryHeading{text: "Disabled"}
        StoryRow{
            Calendar{
                year: 2026
                month: 9
                animator +: {disabled: {default: @on}}
            }
        }
    }
}

fn calendar_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let subject = root.calendar(cx, ids!(subject));
    if let Some(day) = subject.selected(actions) {
        root.label(cx, ids!(day_note))
            .set_text(cx, &format!("{:04}-{:02}-{:02}", day.year, day.month, day.day));
    }

    let many = root.calendar(cx, ids!(many));
    if many.selected(actions).is_some() {
        let count = many.selected_days().len();
        root.label(cx, ids!(many_note))
            .set_text(cx, &format!("{count} days chosen"));
    }

    let span = root.calendar(cx, ids!(span));
    if let Some((start, end)) = span.range_selected(actions) {
        // Both ends count, so a span from a day to itself is one day long.
        let days = end.to_days() - start.to_days() + 1;
        root.label(cx, ids!(span_note)).set_text(
            cx,
            &format!(
                "{:04}-{:02}-{:02} to {:04}-{:02}-{:02} ({days} days)",
                start.year, start.month, start.day, end.year, end.month, end.day
            ),
        );
    }

    let months = root.month_picker(cx, ids!(months));
    if let Some(month) = months.selected(actions) {
        root.label(cx, ids!(picker_note))
            .set_text(cx, &format!("{:04}-{:02}", month.year, month.month));
    }
    let years = root.year_picker(cx, ids!(years));
    if let Some(year) = years.selected(actions) {
        root.label(cx, ids!(picker_note))
            .set_text(cx, &format!("{}", year.year));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/calendar/overview",
    category: "Inputs",
    component: "Calendar",
    also: &["MonthPicker", "YearPicker"],
    name: "Overview",
    dsl: "CalendarOverview",
    added: "2026-09-10",
    tags: &[
        "new",
        "date",
        "date picker",
        "month",
        "day",
        "week",
        "range",
        "schedule",
        "booking",
    ],
    doc: "# Calendar

A month on seven columns: weekday headings, six rows of days, and the days either side of the month drawn dim so the first and last rows are never half empty. `MonthPicker` and `YearPicker` are the same contract on twelve cells, for the two steps above a day.

## It reads no clock

There is no `today` until a host says what today is. A widget that asks the machine for the time cannot be tested, cannot be driven from a script, and is right in exactly one timezone. `today` is an ISO date property, and an empty one marks nothing. The month the grid opens on follows the same rule: whatever `year` and `month` say, then the first day chosen, then the day called today.

## The three modes

| Mode | A press does |
|---|---|
| `Single` | chooses that day, and puts the last one back |
| `Multiple` | toggles that day in the set |
| `Range` | anchors an end, then finishes the span |

A half-made range reports nothing — half a range is not an answer — but it does draw one, previewed against wherever the pointer is, because otherwise there is nothing on screen to say which end is held. The ends always come out earliest first, so a host never sorts them. Escape puts a half-made range down.

## Always six rows

February that starts on a Monday fits in four rows. It is still drawn on six. A grid that changes height when the header arrow is pressed moves everything under it, and the sixth row of dim days is cheaper than a control that jumps.

## Bounds

`min` and `max` are the two ends, as ISO dates; `weekdays_off` is one bit per weekday from Monday, so 96 closes the weekend. Beyond that a host can hand `set_day_allowed` a rule of its own. Closed days cannot be pressed, and the arrow keys step over them rather than parking on one, so the keyboard is never somewhere Return does nothing.

## Keyboard

| Key | Moves |
|---|---|
| left / right | a day |
| up / down | a week |
| page up / page down | a month |
| home / end | the ends of the drawn row |
| return, space | chooses the focused day |

Nothing stops at the edge of the drawn month: the arrows walk into the next one and the grid follows. Stopping would make the last week of a month a dead end whose only way out is a mouse.

## The date type

`CivilDate` is a year, a month and a day, and nothing else — no hour, no zone, no instant. It is dependency-free on purpose, it sorts chronologically because its fields are in that order, and `add_months` clamps the day rather than rolling it over, so 31 January plus a month is the 28th. That clamp is not reversible, which is the price of the operation being total.",
    subject: "subject",
    feature: None,
    controls: &[
        Control {
            label: "Mode",
            target: "subject",
            kind: ControlKind::Choice {
                prop: "mode",
                options: &[
                    "CalendarMode.Single",
                    "CalendarMode.Multiple",
                    "CalendarMode.Range",
                ],
                default: 0,
            },
        },
        Control { label: "Week starts on", target: "subject", kind: ControlKind::Number { prop: "first_day", min: 0., max: 6., step: 1., default: 0. } },
        Control { label: "Month", target: "subject", kind: ControlKind::Number { prop: "month", min: 1., max: 12., step: 1., default: 9. } },
        Control { label: "Year", target: "subject", kind: ControlKind::Number { prop: "year", min: 2000., max: 2050., step: 1., default: 2026. } },
        Control { label: "Week numbers", target: "subject", kind: ControlKind::Bool { prop: "show_week_numbers", default: false } },
        Control { label: "Cell width", target: "subject", kind: ControlKind::Number { prop: "cell_width", min: 20., max: 64., step: 1., default: 34. } },
        Control { label: "Cell height", target: "subject", kind: ControlKind::Number { prop: "cell_height", min: 20., max: 64., step: 1., default: 30. } },
        Control { label: "Weekdays closed", target: "subject", kind: ControlKind::Number { prop: "weekdays_off", min: 0., max: 127., step: 1., default: 0. } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(calendar_actions),
}];

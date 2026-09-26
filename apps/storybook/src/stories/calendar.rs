//! The calendar story: a month grid, the three ways of choosing days from
//! it, and the same grid at its two coarser grains.
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
                mode: CalendarPickMode.Multiple
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
                mode: CalendarPickMode.Range
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
        StoryNote{text: "The same calendar with a coarser grain. The grain says what a cell stands for: a month or a year puts twelve of them on a page, under the same modes, bounds and actions as a day. MonthPicker and YearPicker are Calendar with the grain set and a cell wide enough for a name. A page of months steps a year at a time; a page of years steps a whole page of twelve, aligned so the two arrows are inverses of each other."}
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

    // A page of months and a page of years are calendars, and report a
    // month or a year as the day it starts on.
    let months = root.calendar(cx, ids!(months));
    if let Some(month) = months.selected(actions) {
        root.label(cx, ids!(picker_note))
            .set_text(cx, &format!("{:04}-{:02}", month.year, month.month));
    }
    let years = root.calendar(cx, ids!(years));
    if let Some(year) = years.selected(actions) {
        root.label(cx, ids!(picker_note))
            .set_text(cx, &format!("{}", year.year));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/datepicker/calendar",
    category: "Inputs",
    component: "DatePicker",
    also: &["Calendar", "MonthPicker", "YearPicker"],
    name: "Calendar",
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

A month on seven columns: weekday headings, six rows of days, and the days either side of the month drawn dim so the first and last rows are never half empty. For the two steps above a day it is the same widget with a coarser `grain`.

## It reads no clock

There is no `today` until a host says what today is. A widget that asks the machine for the time cannot be tested, cannot be driven from a script, and is right in exactly one timezone. `today` is an ISO date property, and an empty one marks nothing. The month the grid opens on follows the same rule: whatever `year` and `month` say, then the first day chosen, then the day called today.

## The three modes

| Mode | A press does |
|---|---|
| `Single` | chooses that day, and puts the last one back |
| `Multiple` | toggles that day in the set |
| `Range` | anchors an end, then finishes the span |

A half-made range reports nothing — half a range is not an answer — but it does draw one, previewed against wherever the pointer is, because otherwise there is nothing on screen to say which end is held. The ends always come out earliest first, so a host never sorts them. Escape puts a half-made range down.

## Grain

`grain` is what one cell stands for.

| Grain | The page | The step marks move |
|---|---|---|
| `Day` | a month on seven columns, under weekday headings | a month |
| `Month` | the twelve months of a year | a year |
| `Year` | twelve years, the pages aligned on multiples of twelve | a page |

`MonthPicker` is `Calendar{grain: CalendarGrain.Month}` and `YearPicker` is `Calendar{grain: CalendarGrain.Year}`, each with a cell 72 by 34 so a name fits in it; neither is a type of its own. Everything else is the calendar's: `mode`, `selected`, `min`, `max`, `set_day_allowed`, and the same `Selected` and `RangeSelected` actions. A month or a year is named by the day it starts on, so `2026-09-01` is September and `2026-01-01` is 2026, and one date type runs through all three.

A twelve-cell page lays out on `columns` cells to a row, writes its months in three letters unless `long_names` is set, and takes its names from `month_names` like the header does. `year` opens it; it has no use for `month`, the weekday properties or the week numbers. `today` marks the cell that holds the day.

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

On a twelve-cell page left and right move a cell, up and down a row, page up and page down a whole page, and home and end go to the first and last cell. The moves roll into the page either side the same way. They do not step over a closed cell, and Escape is a day grid's key only.

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
                    "CalendarPickMode.Single",
                    "CalendarPickMode.Multiple",
                    "CalendarPickMode.Range",
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The page is built from the DSL, which the Rust compiler never reads:
    /// a mistake in a `script_mod!` block shows up only in a running app's
    /// log. `MonthPicker` and `YearPicker` are declared there and nowhere
    /// else — a calendar with its grain set — so a grain that did not
    /// resolve would leave two more month grids on the page and no error
    /// anywhere. Building the page and reading the three back is what turns
    /// that into a failed build.
    #[test]
    fn the_page_builds_and_the_two_pickers_are_calendars_with_their_grain_set() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
        });
        let story = &STORIES[0];
        let page = cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {}", story.dsl);
            WidgetRef::script_from_value(vm, value)
        });
        assert!(!page.is_empty(), "{} built no widget", story.key);
        assert_eq!(
            makepad_platform::shader_error::take(),
            None,
            "a calendar shader failed to compile"
        );

        for target in std::iter::once(story.subject)
            .chain(story.controls.iter().map(|c| c.target))
            .filter(|t| !t.is_empty())
        {
            assert!(
                !page.widget(&cx, &[LiveId::from_str(target)]).is_empty(),
                "no widget at {target}"
            );
        }

        // The subject is what it always was: a day grid, on the cell a day
        // takes, opened on the month it was given.
        let subject = page.calendar(&cx, ids!(subject));
        {
            let subject = subject.borrow().expect("the subject is a calendar");
            assert_eq!(subject.grain, CalendarGrain::Day);
            assert_eq!((subject.cell_width, subject.cell_height), (34.0, 30.0));
        }
        assert_eq!(subject.text(), "September 2026 (1 chosen)");

        // And the two names are that same type, with the grain and the cell
        // the preset sets and the twelve-cell page the grain opens.
        for (name, id, grain, text) in [
            ("months", ids!(months), CalendarGrain::Month, "2026 (1 chosen)"),
            ("years", ids!(years), CalendarGrain::Year, "2016 \u{2013} 2027 (1 chosen)"),
        ] {
            let picker = page.calendar(&cx, id);
            {
                let picker = picker.borrow().unwrap_or_else(|| panic!("{name} is not a calendar"));
                assert_eq!(picker.grain, grain, "{name}");
                assert_eq!((picker.cell_width, picker.cell_height), (72.0, 34.0), "{name}");
                assert_eq!(picker.columns, 3, "{name}");
            }
            assert_eq!(picker.text(), text, "{name}");
        }
        assert_eq!(
            page.calendar(&cx, ids!(months)).selected_day(),
            Some(CivilDate { year: 2026, month: 9, day: 1 }),
            "a month is named by the day it starts on"
        );
    }
}

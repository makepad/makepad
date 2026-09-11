//! The date stories: a date you type, a date you press, and a range with
//! two ends.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.DatePickerOverview = StoryPage{
        StoryNote{text: "Three widgets over one date. A field you type into, the same field with a calendar under it, and the two-ended form for a start and an end. All three read a date against `format`, put it back in that shape when the field is left, and keep what will not parse instead of clearing it."}

        StoryHeading{text: "A calendar under the field"}
        StoryNote{text: "The button opens it, and only the button: a press into the text is a press into the text. The calendar arrives on the day the field is holding, so it lands on the month already being thought about, and one press there is the whole answer — the panel goes away without waiting to be dismissed."}
        StoryRow{
            Label{text: "Starts"}
            subject := DatePicker{
                date: "2026-09-10"
            }
            subject_note := Label{text: "2026-09-10"}
        }

        StoryHeading{text: "A date you only type"}
        StoryNote{text: "The same field without the calendar. Type into it and leave it — press Tab, or click elsewhere — and what parses comes back tidied: `2026-9-1` is put back as `2026-09-01`, and slashes are accepted where the shape asks for dashes."}
        StoryRow{
            Label{text: "Invoiced"}
            typed := DateField{}
            typed_note := Label{text: "nothing yet"}
        }

        StoryHeading{text: "What it does with a typo"}
        StoryNote{text: "It keeps it. Type `2026-13-40` into the field above and leave it: the text stays exactly as typed, a mark appears at the head of the box, and the field reports that it is holding no date. A date is nine characters that have to be right, and emptying the box throws away eight correct ones to punish the wrong one."}

        StoryHeading{text: "Another shape"}
        StoryNote{text: "`format` is written with `YYYY`, `MM` and `DD` and whatever separators go between them. It is also the placeholder, so an empty box says what shape it wants. A year may not be typed short — `26` is a year two people read two different ways, and there is nothing here that could ask which was meant."}
        StoryRow{
            Label{text: "Day first"}
            DateField{format: "DD/MM/YYYY" date: "2026-09-10"}
            Filler{}
            Label{text: "Month first"}
            DateField{format: "MM.DD.YYYY" date: "2026-09-10"}
        }

        StoryHeading{text: "A start and an end"}
        StoryNote{text: "Two boxes, two calendars side by side, and one invariant: the start is never after the end. Press a day for one end and a day for the other; a second press before the first is read as the pair rather than as a mistake. The panel stays up between the two presses and closes once the range is whole."}
        StoryRow{
            width: Fill
            range := DateRangePicker{
                start: "2026-09-07"
                end: "2026-09-21"
            }
        }
        StoryRow{
            range_note := Label{text: "2026-09-07 – 2026-09-21"}
        }

        StoryHeading{text: "Typing an end past the other one"}
        StoryNote{text: "Type a start later than the end in the range above and leave the box: the end moves up to meet it. The end being touched is the one that means something, and moving the one that is not would silently undo a choice made on purpose — the same rule the range slider follows when a handle reaches the other."}

        StoryHeading{text: "Disabled"}
        StoryRow{
            Label{text: "Off"}
            DatePicker{date: "2026-09-10" disabled: true}
            DateField{date: "2026-09-10" disabled: true}
        }
    }
}

fn date_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if let Some(date) = root.date_picker(cx, ids!(subject)).changed(actions) {
        let text = match date {
            Some(date) => format!("{:04}-{:02}-{:02}", date.year, date.month, date.day),
            None => "no date".to_string(),
        };
        root.label(cx, ids!(subject_note)).set_text(cx, &text);
    }
    if let Some(date) = root.date_field(cx, ids!(typed)).changed(actions) {
        let text = match date {
            // The field is holding text that will not parse. It still has
            // the text; it has no date, and says so.
            None => "no date".to_string(),
            Some(date) => format!("{:04}-{:02}-{:02}", date.year, date.month, date.day),
        };
        root.label(cx, ids!(typed_note)).set_text(cx, &text);
    }
    if let Some((start, end)) = root.date_range_picker(cx, ids!(range)).changed(actions) {
        root.label(cx, ids!(range_note)).set_text(
            cx,
            &format!(
                "{:04}-{:02}-{:02} \u{2013} {:04}-{:02}-{:02}",
                start.year, start.month, start.day, end.year, end.month, end.day
            ),
        );
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/datepicker/overview",
    category: "Inputs",
    component: "DatePicker",
    also: &["DateField", "DateRangePicker"],
    name: "Overview",
    dsl: "DatePickerOverview",
    added: "2026-09-10",
    tags: &["new", "controls", "date", "calendar", "range", "field", "format"],
    doc: "# DatePicker\n\nThree widgets over one date: `DateField` is the box you type into, `DatePicker` is that box with a calendar in a popover under it, and `DateRangePicker` is two of them with a start and an end.\n\n## What a date field does with a typo\n\nIt keeps it. A date is eight to ten characters that all have to be right, and the commonest thing that happens to one is a single wrong digit. A field that answers that by emptying itself has thrown away nine correct characters to punish one wrong one, and the person types the whole thing again. So text that will not parse **stays in the box**, a mark appears beside it, and the field reports that it holds no date — the text is kept, the value is not.\n\nWhat parses is put back in the field's own shape when the field is left, so `2026-9-1` becomes `2026-09-01` and a column of dates lines up.\n\n## The shape\n\n`format` is written with `YYYY`, `MM` and `DD` and whatever separators go between them: `YYYY-MM-DD`, `DD/MM/YYYY`, `MM.DD.YYYY`. It doubles as the placeholder, so an empty box says what shape it wants.\n\nReading is looser than writing, in the two places where that is about what hands do rather than what the format says: a month or a day may be typed without its leading zero, and any of `-`, `/`, `.` or a space will do for any other. A year may **not** be typed short. `26` is a year two different people read two different ways, and there is nothing here that could ask which was meant.\n\n## Opening the calendar\n\nOnly the trailing button opens it. The field is the popover's anchor, and a popover that opened on any press on its anchor would take the pointer away from the text the moment somebody tried to click into it. The calendar arrives showing the day the field holds, so it lands on the month already being thought about, and one press in it is the whole answer: the panel closes rather than waiting to be dismissed.\n\n## The range\n\nTwo boxes and two calendars, the left one the start and the right one the end, and `start <= end` at all times however the two are set.\n\nA range takes two presses. The second one **before** the first is read as the pair rather than as a mistake — the two presses are the two ends, and the order they were made in says nothing about which end is earlier; throwing the first away instead would make choosing a range backwards cost three presses. A third press starts a new range. The panel stays up between the two presses and goes away once the range is whole.\n\nTyping is the same rule from the other side: put a start past the end and the end gives way, put an end before the start and the start does. The end being touched is the one that means something, and moving the one that is not would silently undo a choice made on purpose.\n\n## What these deliberately do not do\n\n**No clock and no timezone.** Nothing here asks what day it is; a host that wants a field to open on today hands it today. The answer depends on where the person is standing, and this crate has no way to find out.\n\n**No month names and no locale.** `format` writes digits. A field that could show `10 Sept` would have to know a language, and there is nowhere to ask which one.\n\n**No time of day.** These pick a date.\n\n## Reading them\n\n`changed` on the field and the picker reports `Option<Option<CivilDate>>`: the outer answer is whether anything settled this pass, and the inner one is the date — `Some(None)` is a real answer, meaning the box is holding a typo. The range picker's `changed` reports both ends, and only once both are known.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Shape", target: "subject", kind: ControlKind::Text { prop: "format", default: "YYYY-MM-DD" } },
        Control { label: "Date", target: "subject", kind: ControlKind::Text { prop: "date", default: "2026-09-10" } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(date_actions),
}];

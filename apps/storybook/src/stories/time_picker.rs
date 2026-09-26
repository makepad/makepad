//! The time story: a time of day typed into a field, and a time of day
//! picked out of columns.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.TimePickerOverview = StoryPage{
        StoryNote{text: "An hour and a minute, and nothing else — no date, no zone, no clock reading. Type one into a field, or pick one out of columns. The two share one core, so both read and write the same times."}

        StoryHeading{text: "Picking a time"}
        StoryNote{text: "The columns list what the grid actually offers. Click a value, roll the wheel over a column, or give one the keyboard and use the arrows; Page Up and Page Down move by a screenful and Home and End go to the ends."}
        StoryRow{
            subject := TimePicker{
                step: 5.0
                value: "09:30"
            }
            picked_note := Label{text: "09:30"}
        }

        StoryHeading{text: "Typing a time"}
        StoryNote{text: "It takes 9, 09, 930, 0930, 9.30, 9:30 and 9:30 pm, and puts each of them back in one shape. What it cannot read it refuses, restoring the last time it could — a field that silently keeps nonsense is worse than one that will not take it."}
        StoryRow{
            Label{text: "Starts"}
            starts := TimeField{
                value: "09:30"
            }
            starts_note := Label{text: "09:30"}
        }

        StoryHeading{text: "The part under the caret"}
        StoryNote{text: "Up and down move the part the caret is in and leave it selected, so the next press moves the same part: the hour, the minute, and on a twelve-hour field the am/pm mark. The wheel does the same, but only while the field holds the keyboard — without a caret there is no part to move, and a field scrolled past in a long form must not change itself on the way by."}
        StoryRow{
            Label{text: "Twelve-hour"}
            noon := TimeField{
                width: 140.
                hour12: true
                step: 15.0
                value: "12:00"
            }
            noon_note := Label{text: "12:00 PM"}
        }

        StoryHeading{text: "A step that does not divide the hour"}
        StoryNote{text: "Seven minutes. The offers are counted from the bottom bound rather than filtered out of the clock, so they keep their spacing across the hour: the first hour ends on :56 and the second starts on :03. Move the hour and the minute column changes with it, because each hour really does have a different set of minutes in it."}
        StoryRow{
            seven := TimePicker{
                step: 7.0
                value: "01:03"
            }
            seven_note := Label{text: "01:03"}
        }

        StoryHeading{text: "Bounded to a working day"}
        StoryNote{text: "`min` and `max` are written as times. The hour column only lists hours that have an offer in them, so nothing outside the bounds is reachable at all — there is no state where the control shows a time it would refuse."}
        StoryRow{
            shift := TimePicker{
                step: 15.0
                min: "09:00"
                max: "17:30"
                value: "09:00"
            }
            shift_note := Label{text: "09:00"}
        }

        StoryHeading{text: "Twelve hours and an am/pm column"}
        StoryNote{text: "`hour12` changes what is offered and what is written, never what is held: the time is 0..23 either way. The third column is the switch between the two halves of the day, and the hour column follows it — twelve hours are on offer at a time, not twenty-four with half of them repeated."}
        StoryRow{
            half_day := TimePicker{
                hour12: true
                step: 10.0
                value: "14:20"
            }
            half_day_note := Label{text: "2:20 PM"}
        }

        StoryHeading{text: "Disabled"}
        StoryRow{
            TimeField{
                value: "09:30"
                disabled: true
            }
            TimePicker{
                step: 30.0
                value: "09:30"
                disabled: true
            }
        }

        StoryHeading{text: "There is no dial"}
        StoryNote{text: "The round clock face is deliberately absent. Its marks and hands are tapered shapes, which means SDF paths, and path fills do not paint reliably here — two mirrored, otherwise identical paths were drawn and only the second ever appeared. Everything on this page is a box, a rounded box, or a text glyph, all of which paint. A dial built today would be a control that is sometimes invisible and always differently invisible."}
    }
}

fn time_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if let Some(time) = root.time_picker(cx, ids!(subject)).changed(actions) {
        root.label(cx, ids!(picked_note)).set_text(cx, &time.format(Clock::H24));
    }
    if let Some(time) = root.time_picker(cx, ids!(seven)).changed(actions) {
        root.label(cx, ids!(seven_note)).set_text(cx, &time.format(Clock::H24));
    }
    if let Some(time) = root.time_picker(cx, ids!(shift)).changed(actions) {
        root.label(cx, ids!(shift_note)).set_text(cx, &time.format(Clock::H24));
    }
    // The twelve-hour ones are read back on the clock they are shown on, so
    // the note beside a control says what the control says.
    if let Some(time) = root.time_picker(cx, ids!(half_day)).changed(actions) {
        root.label(cx, ids!(half_day_note)).set_text(cx, &time.format(Clock::H12));
    }
    if let Some(time) = root.time_field(cx, ids!(starts)).changed(actions) {
        root.label(cx, ids!(starts_note)).set_text(cx, &time.format(Clock::H24));
    }
    if let Some(time) = root.time_field(cx, ids!(noon)).changed(actions) {
        root.label(cx, ids!(noon_note)).set_text(cx, &time.format(Clock::H12));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/timepicker/overview",
    category: "Inputs",
    component: "TimePicker",
    also: &["TimeField", "TimeColumn"],
    name: "Overview",
    dsl: "TimePickerOverview",
    added: "2026-09-10",
    tags: &["new", "controls", "time", "clock", "hour", "minute", "am", "pm", "schedule"],
    doc: "# TimePicker and TimeField\n\nAn hour and a minute, and nothing else. The core is a `CivilTime` — no date, no zone, no clock reading. \"What time does it start\" is a question a form asks on its own, and answering it does not need to know what a day is; keeping the two apart is what lets a caller hold a time in a struct field without dragging a calendar in behind it.\n\n## Two widgets over one core\n\n| Widget | What it is |\n|---|---|\n| `TimeField` | a text field that reads a time and writes it back in one shape |\n| `TimePicker` | two columns of offered times, or three on a twelve-hour clock |\n| `TimeColumn` | one column of values; the picker owns three of them |\n\n## What the field takes\n\n`9`, `09`, `930`, `0930`, `9.30`, `9:30`, and any of those with an am/pm mark after them: `9:30 pm`, `9pm`, `9 p.m.`. It parses on Return and on leaving the field. What will not parse is refused and the last good time comes back — keeping nonsense silently is worse than refusing it, and clearing the box would lose the value the person was editing away from. An hour outside 1..12 carrying an am/pm mark is not a time and is refused with the rest.\n\n## The part under the caret\n\nUp and down move the part the caret is in — the hour, the minute, or the am/pm mark — and leave that part selected, so the next press moves the same one. A caret sitting exactly at the end of a part belongs to that part, which is the convention typing produces. The wheel does the same thing, but only while the field holds the keyboard: without a caret there is no part to move.\n\nThe field's `step` is the minute part's arrow step and nothing else. It does **not** quantize what is typed — somebody who typed 9:07 meant 9:07, and a field that rounded it away would be arguing with them.\n\n## Why the picker has a grid\n\nThe offers are counted from `min`, not filtered out of the clock. A step that divides the hour hides the difference; a step of seven does not: the offers run 0:00, 0:07 ... 0:56, then 1:03. Each hour therefore has its own set of minutes, and the minute column is asked for the minutes of the hour that is currently chosen rather than handed the same list every time. `min` and `max` are written as times, and the hour column only lists hours that have an offer in them, so there is no state where the control shows a time it would refuse.\n\n## Twelve hours\n\n`hour12` changes what is offered and what is written, never what is held: a time is 0..23 either way. Midnight and noon are both twelve o'clock, one am and one pm, and that conversion lives in one pair of functions with tests on it rather than being spread through the drawing code.\n\n## There is no dial\n\nThe round clock face is deliberately absent, and it is not an oversight. Its marks and hands are tapered shapes, which means SDF paths — and path fills do not paint reliably in this library: two mirrored, otherwise identical paths were drawn and only the second ever appeared. Everything these widgets draw is a box, a rounded box, or a text glyph, all of which paint. A dial built on top of the broken primitive would be a control that is sometimes invisible and always differently invisible. When paths are fixed a dial can stand beside these two.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Step in minutes", target: "subject", kind: ControlKind::Number { prop: "step", min: 1., max: 60., step: 1., default: 5. } },
        Control { label: "Twelve-hour clock", target: "subject", kind: ControlKind::Bool { prop: "hour12", default: false } },
        Control { label: "Earliest", target: "subject", kind: ControlKind::Text { prop: "min", default: "" } },
        Control { label: "Latest", target: "subject", kind: ControlKind::Text { prop: "max", default: "" } },
        Control { label: "Starts at", target: "subject", kind: ControlKind::Text { prop: "value", default: "09:30" } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(time_actions),
}];

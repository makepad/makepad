//! The wheel picker story: columns that spin and rest with one row in the
//! band.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.WheelPickerOverview = StoryPage{
        StoryNote{text: "Columns of rows that turn like a drum. Drag a column to spin it, let go and it carries on and settles with exactly one row inside the band. The wheel turns a column too, and the arrow keys move the focused one a row at a time."}

        StoryHeading{text: "A time"}
        StoryNote{text: "Two columns, both coming round: past 23 is 00 again, and past 55 is 00. Each column keeps its own row, and the picker is one control — the arrows move the column that is lit, and left and right choose which that is."}
        StoryRow{
            width: Fill
            subject := WheelPicker{
                column_width: 64.0
                columns: [
                    WheelColumn{
                        loop_items: true
                        selected: 9
                        items: [
                            "00" "01" "02" "03" "04" "05" "06" "07" "08" "09" "10" "11"
                            "12" "13" "14" "15" "16" "17" "18" "19" "20" "21" "22" "23"
                        ]
                    }
                    WheelColumn{
                        loop_items: true
                        selected: 6
                        items: ["00" "05" "10" "15" "20" "25" "30" "35" "40" "45" "50" "55"]
                    }
                ]
            }
            time_note := Label{text: "09:30"}
        }

        StoryHeading{text: "A column with ends"}
        StoryNote{text: "`loop_items: false` gives the column a first row and a last one, and it stops dead at both. There is no stretch past the stop: on a drum of a handful of rows the stretch would be most of the control and would read as the picker having lost its place."}
        StoryRow{
            width: Fill
            sizes := WheelPicker{
                column_width: 140.0
                columns: [
                    WheelColumn{
                        selected: 2
                        items: ["Extra small" "Small" "Medium" "Large" "Extra large"]
                    }
                ]
            }
            size_note := Label{text: "Medium"}
        }

        StoryHeading{text: "How many rows"}
        StoryNote{text: "`visible_items` is the height of the control, in rows. It is always odd, because a drum with no middle row has nothing to put in the band; an even number is raised by one rather than refused. Three rows shows only the neighbours; seven shows the shape of the list."}
        StoryRow{
            width: Fill
            WheelPicker{
                visible_items: 3
                column_width: 72.0
                columns: [
                    WheelColumn{
                        loop_items: true
                        selected: 2
                        items: ["Mon" "Tue" "Wed" "Thu" "Fri" "Sat" "Sun"]
                    }
                ]
            }
            WheelPicker{
                visible_items: 7
                column_width: 72.0
                columns: [
                    WheelColumn{
                        loop_items: true
                        selected: 2
                        items: ["Mon" "Tue" "Wed" "Thu" "Fri" "Sat" "Sun"]
                    }
                ]
            }
        }

        StoryHeading{text: "How far the rows fall away"}
        StoryNote{text: "`dim_far` is the ink the outermost row gives up and `shrink_far` is the size. Together they are what makes a column read as a cylinder rather than as a list: turn both off and the rows away from the band compete with the row inside it."}
        StoryRow{
            width: Fill
            WheelPicker{
                column_width: 96.0
                dim_far: 0.0
                shrink_far: 0.0
                columns: [
                    WheelColumn{selected: 2 items: ["Flat" "and" "even" "all" "the" "way"]}
                ]
            }
            WheelPicker{
                column_width: 96.0
                dim_far: 0.88
                shrink_far: 0.34
                columns: [
                    WheelColumn{selected: 2 items: ["Deep" "and" "steep" "at" "the" "edge"]}
                ]
            }
        }

        StoryHeading{text: "Disabled"}
        StoryRow{
            width: Fill
            WheelPicker{
                column_width: 96.0
                columns: [
                    WheelColumn{selected: 1 items: ["One" "Two" "Three"]}
                ]
                animator +: {disabled: {default: @on}}
            }
        }

        StoryHeading{text: "The ladder"}
        StoryRow{
            width: Fill
            WheelPickerFlat{
                column_width: 128.0
                columns: [
                    WheelColumn{selected: 1 items: ["WheelPickerFlat" "the same drum" "without the bevel"]}
                ]
            }
        }
    }
}

/// The rows of the `sizes` column, so the readout can name the one in the
/// band. The picker reports an index; the words belong to whoever wrote
/// them.
const SIZES: &[&str] = &["Extra small", "Small", "Medium", "Large", "Extra large"];

fn wheel_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let clock = root.wheel_picker(cx, ids!(subject));
    if clock.changed(actions).is_some() {
        // Either column may have moved, and a spin reports each row it
        // passes, so the readout is rebuilt from both rather than patched
        // from the one action that arrived.
        let hour = clock.selected(0);
        let minute = clock.selected(1) * 5;
        root.label(cx, ids!(time_note))
            .set_text(cx, &format!("{hour:02}:{minute:02}"));
    }
    let sizes = root.wheel_picker(cx, ids!(sizes));
    if let Some((_, index)) = sizes.changed(actions) {
        root.label(cx, ids!(size_note))
            .set_text(cx, SIZES.get(index).copied().unwrap_or(""));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/wheelpicker/overview",
    category: "Inputs",
    component: "WheelPicker",
    also: &["WheelPickerFlat"],
    name: "Overview",
    dsl: "WheelPickerOverview",
    added: "2026-09-10",
    tags: &["new", "controls", "drum", "spinner", "date", "time", "column", "picker"],
    doc: "# WheelPicker\n\nColumns of rows that turn like a drum and rest with exactly one row inside the band across the middle. That row is the column's value: there is no separate confirm step and no state in which the band holds nothing.\n\n## Why a drum and not a list\n\nA list is for finding something; a drum is for choosing between a handful of values whose neighbours are worth seeing. So there is no scrollbar, no page motion and no multiple selection, and the number of rows on screen is a property of the control rather than of the room it is given — a fixed count is what lets the middle row mean something. Every column holds its whole list; nothing is fetched as it turns.\n\n## Landing on a row\n\nA flick does not run its momentum out and then jerk to the nearest row. The release velocity predicts where a spin decaying at the library's scroll rate would stop, that prediction is snapped to a row, and the column glides there opening at the speed the finger left it with. The motion is one movement and it always ends on a row.\n\n## The gestures\n\n| Gesture | Result |\n|---|---|\n| drag a column | turns it |\n| let go while moving | carries on, and settles on a row |\n| press a moving column | stops it where it stands, then settles |\n| tap a row | brings that row into the band |\n| wheel or trackpad | turns the column under the pointer, one row per notch at most |\n\nA wheel notch is worth at most one row whatever size the operating system says it is: notches run from a few pixels to most of a screen, and a picker that jumps four rows on one notch cannot be aimed.\n\n## Keyboard\n\nUp and down move the lit column by a row. Left and right choose which column is lit, since Tab belongs to focus traversal and a picker's columns are as much one control as a slider's two ends are. Home and End are the first and last row of the *list* — on a column that comes round, the last row may well be one step backwards.\n\n## Columns\n\nEach `WheelColumn` carries its own `items`, its own `selected`, its own `loop_items` and, if it wants one, its own `width`; a column that sets no width takes the picker's `column_width`. `loop_items` is per column, so an hour that comes round can stand beside a list of names that does not.\n\n## Reading it\n\n`changed` reports a column and a row while the drum is still moving, not only once it settles — the band is the value, and a readout that keeps showing the row from before the flick is telling the truth about nothing. `selected(column)` is the row in the band right now, and `set_selected` puts one there with no spin to watch, because setting a value is not a gesture.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Rows on screen", target: "subject", kind: ControlKind::Number { prop: "visible_items", min: 3., max: 11., step: 2., default: 5. } },
        Control { label: "Row height", target: "subject", kind: ControlKind::Number { prop: "row_height", min: 16., max: 64., step: 1., default: 28. } },
        Control { label: "Column width", target: "subject", kind: ControlKind::Number { prop: "column_width", min: 32., max: 200., step: 4., default: 64. } },
        Control { label: "Gap", target: "subject", kind: ControlKind::Number { prop: "column_gap", min: 0., max: 24., step: 1., default: 2. } },
        Control { label: "Ink lost at the edge", target: "subject", kind: ControlKind::Number { prop: "dim_far", min: 0., max: 1., step: 0.02, default: 0.45 } },
        Control { label: "Size lost at the edge", target: "subject", kind: ControlKind::Number { prop: "shrink_far", min: 0., max: 0.6, step: 0.01, default: 0.16 } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(wheel_actions),
}];

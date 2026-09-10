//! The number field story: a number with a stepper you can see.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.NumberFieldOverview = StoryPage{
        StoryNote{text: "A number you can type into, with two buttons that are always there. Press one to step, hold it to repeat, drag it up or down, or roll the wheel anywhere over the field."}

        StoryHeading{text: "One field, under the controls"}
        StoryRow{
            Label{text: "Quantity"}
            subject := NumberField{
                width: 140.
                min: 0.0
                max: 99.0
                step: 1.0
            }
            quantity_note := Label{text: "0"}
        }

        StoryHeading{text: "Holding a button repeats"}
        StoryNote{text: "Slowly at first and then faster, so a range of a thousand is reachable without a thousand presses and a single press is still a single step."}
        StoryRow{
            Label{text: "Cents"}
            cents := NumberField{
                width: 160.
                min: 0.0
                max: 1000.0
                step: 1.0
            }
        }

        StoryHeading{text: "Dragging the buttons"}
        StoryNote{text: "Up is more. The hand is already on the stepper, and a drag is the gesture it is about to make; without it, wanting twenty more means twenty presses."}
        StoryRow{
            Label{text: "Weight"}
            weight := NumberField{
                width: 170.
                min: 0.0
                max: 500.0
                step: 0.5
                precision: 1
                suffix: " kg"
            }
        }

        StoryHeading{text: "Stopping, or coming round"}
        StoryNote{text: "`min` and `max` bound both of these. The quantity above stops at its ends. An angle is a cycle rather than a quantity, so it wraps: step past 359 and it is 0. Stopping is the default, since a count that wraps is how a form ends up ordering none of something."}
        StoryRow{
            Label{text: "Angle"}
            angle := NumberField{
                width: 160.
                min: 0.0
                max: 360.0
                step: 1.0
                wrap: true
                suffix: " deg"
            }
            angle_note := Label{text: "0 deg"}
        }

        StoryHeading{text: "Refusing what will not parse"}
        StoryNote{text: "Type a word into any of them and leave the field: the last good value comes back. Silently keeping nonsense is worse than refusing it, and emptying the box would lose the value being edited away from."}

        StoryHeading{text: "Disabled"}
        StoryRow{
            Label{text: "Off"}
            NumberField{
                width: 140.
                disabled: true
            }
        }

        StoryHeading{text: "Beside the other numbers"}
        StoryNote{text: "ValueInput is the same number for a toolbar: it hides its marks until the pointer arrives, because a toolbar cannot afford chrome that is only sometimes useful. A form cannot afford the opposite — a control whose affordance only appears on hover reads as a plain box, and the arrows are never found."}
        StoryRow{
            Label{text: "NumberField"}
            NumberField{width: 130. min: 0.0 max: 100.0 step: 1.0}
            Filler{}
            Label{text: "ValueInput"}
            ValueInput{}
        }
    }
}

fn number_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if let Some(v) = root.number_field(cx, ids!(subject)).changed(actions) {
        root.label(cx, ids!(quantity_note)).set_text(cx, &format!("{v:.0}"));
    }
    if let Some(v) = root.number_field(cx, ids!(angle)).changed(actions) {
        root.label(cx, ids!(angle_note)).set_text(cx, &format!("{v:.0} deg"));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/numberfield/overview",
    category: "Inputs",
    component: "NumberField",
    also: &["NumberSpin"],
    name: "Overview",
    dsl: "NumberFieldOverview",
    added: "2026-09-10",
    tags: &["new", "controls", "stepper", "spinner", "number", "numeric", "quantity"],
    doc: "# NumberField\n\nA number you type into, with a stepper you can see.\n\nThe library already had a number you **drag**: `ValueInput` hides its step marks until the pointer arrives, because a toolbar cannot afford chrome that is only occasionally useful. A form cannot afford the opposite. In a column of fields somebody is filling in, a control whose affordance appears only on hover reads as a plain text box, and the two arrows that would have saved them typing are never found. So this one's buttons are always there.\n\n## Four ways in, and why each is here\n\n| Gesture | Result |\n|---|---|\n| type | a real text box: selection, clipboard and undo all work |\n| press a button | one step |\n| hold a button | repeats, slowly at first and then faster |\n| drag the buttons | up is more, one step per six points of travel |\n| wheel, anywhere over the field | one step a notch; Shift takes ten |\n\nThe hold and the drag are the same press: once the pointer leaves the half it started on, the repeat stops and the drag takes over, so the two never fight over the same value.\n\n## The ends\n\n`min` and `max` bound it. At a bound it either stops or **wraps**, and `wrap` says which. An angle in degrees wants 359 + 1 to be 0; a quantity of things wants it to stay at the top. Stopping is the default, because a count that wraps is how a form ends up ordering none of something.\n\n## What it does with bad text\n\nWhat is typed is parsed on Return and on leaving the field. What will not parse is refused and the last good value comes back — keeping nonsense silently is worse than refusing it, and clearing the box would lose the value the person was editing away from. A `suffix` may be left in place or typed out; either parses.\n\n## The shell\n\nIt is a `FieldWell` with a `TextInput` in the input slot and a `NumberSpin` in the trailing one — which is what the well's trailing slot was documented as being for. The well draws the box and carries the focus, so this widget only has to own the arithmetic.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Minimum", target: "subject", kind: ControlKind::Number { prop: "min", min: -100., max: 0., step: 1., default: 0. } },
        Control { label: "Maximum", target: "subject", kind: ControlKind::Number { prop: "max", min: 1., max: 1000., step: 1., default: 99. } },
        Control { label: "Step", target: "subject", kind: ControlKind::Number { prop: "step", min: 0.1, max: 25., step: 0.1, default: 1. } },
        Control { label: "Decimals", target: "subject", kind: ControlKind::Number { prop: "precision", min: 0., max: 4., step: 1., default: 0. } },
        Control { label: "Suffix", target: "subject", kind: ControlKind::Text { prop: "suffix", default: "" } },
        Control { label: "Wrap at the ends", target: "subject", kind: ControlKind::Bool { prop: "wrap", default: false } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(number_actions),
}];

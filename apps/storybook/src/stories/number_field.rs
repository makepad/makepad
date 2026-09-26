//! The number field story: a number with a stepper you can see, the small
//! number a toolbar uses, and which number control belongs where.
use crate::makepad_widgets::value_input::ValueInputWidgetRefExt;
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

        StoryHeading{text: "A number for a toolbar"}
        StoryNote{text: "ValueInput is a small number with an arrow at each end. Drag across it to change the value, or click to type one. It is sized for a toolbar rather than a form: seventy-six points by twenty-two, label outside, no gutter."}
        StoryNote{text: "Each of these is bounded and stepped differently. The tempo moves in tenths and stops at forty and three hundred; the count is whole numbers only; the gain is fine enough to need two decimals. There is no value property to declare, so this page seeds each one with set_value, which is the call every host has to make."}
        StoryRow{
            View{
                width: Fit height: Fit flow: Right spacing: theme.space_2
                align: Align{y: 0.5}
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                Label{text: "Tempo" draw_text +: {color: theme.color_text_meta}}
                tempo := ValueInput{min: 40.0 max: 300.0 step: 0.1 precision: 1.0}
                Label{text: "Count" draw_text +: {color: theme.color_text_meta}}
                count := ValueInput{min: 1.0 max: 64.0 step: 1.0 precision: 0.0}
                Label{text: "Gain" draw_text +: {color: theme.color_text_meta}}
                gain := ValueInput{min: 0.0 max: 2.0 step: 0.01 precision: 2.0}
            }
        }
        StoryRow{
            reported := Label{text: "nothing changed yet"}
        }

        StoryHeading{text: "Which number to reach for"}
        StoryNote{text: "The library has four number controls, and each is shaped for a place. NumberField is for a form: its buttons are always there, because a control whose affordance only appears on hover reads as a plain box and the arrows are never found. ValueInput is for a toolbar, which cannot afford chrome that is only sometimes useful, so its marks stay out of the way until the pointer arrives. FabValueInput is for a property panel, a full-width row with the label pinned left, and is shown on the PropertyInspector page. DropSlider is for a bar where a drag would fight dragging the window, so its slider lives in a popover and the chip only ever takes a click."}
        StoryRow{
            Label{text: "NumberField"}
            NumberField{width: 130. min: 0.0 max: 100.0 step: 1.0}
            Filler{}
            Label{text: "ValueInput"}
            ValueInput{}
        }

        StoryHeading{text: "Refusing what will not parse"}
        StoryNote{text: "Type a word into any of the number fields and leave the field: the last good value comes back. Silently keeping nonsense is worse than refusing it, and emptying the box would lose the value being edited away from."}

        StoryHeading{text: "Disabled"}
        StoryRow{
            Label{text: "Off"}
            NumberField{
                width: 140.
                disabled: true
            }
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

fn value_input_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    // Seeding, because there is no `value` property to declare. A field
    // starts at zero however high its min is - the display clamps, the value
    // does not - so a scrub from an unseeded field computes from zero and
    // lands on the floor. This is the call every real host has to make.
    for (id, seed) in [
        (ids!(tempo), 128.0),
        (ids!(count), 8.0),
        (ids!(gain), 1.0),
    ] {
        let field = root.value_input(cx, id);
        if field.value() == 0.0 {
            field.set_value(cx, seed);
        }
    }

    for (name, id) in [
        ("tempo", ids!(tempo)),
        ("count", ids!(count)),
        ("gain", ids!(gain)),
    ] {
        if let Some(v) = root.value_input(cx, id).changed(actions) {
            root.label(cx, ids!(reported))
                .set_text(cx, &format!("{name} is {v}"));
        }
    }
}

/// The page's one handler: the number fields, then the toolbar numbers.
fn number_page_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    number_actions(cx, root, actions);
    value_input_actions(cx, root, actions);
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/numberfield/overview",
    category: "Inputs",
    component: "NumberField",
    also: &["NumberSpin", "ValueInput"],
    name: "Overview",
    dsl: "NumberFieldOverview",
    added: "2026-09-10",
    tags: &["new", "controls", "stepper", "spinner", "number", "numeric", "quantity", "spinbox", "scrub"],
    doc: "# NumberField\n\nA number you type into, with a stepper you can see. Its buttons are always there: in a column of fields somebody is filling in, a control whose affordance appears only on hover reads as a plain text box, and the two arrows that would have saved them typing are never found.\n\n## Four ways in, and why each is here\n\n| Gesture | Result |\n|---|---|\n| type | a real text box: selection, clipboard and undo all work |\n| press a button | one step |\n| hold a button | repeats, slowly at first and then faster |\n| drag the buttons | up is more, one step per six points of travel |\n| wheel, anywhere over the field | one step a notch; Shift takes ten |\n\nThe hold and the drag are the same press: once the pointer leaves the half it started on, the repeat stops and the drag takes over, so the two never fight over the same value.\n\n## The ends\n\n`min` and `max` bound it. At a bound it either stops or **wraps**, and `wrap` says which. An angle in degrees wants 359 + 1 to be 0; a quantity of things wants it to stay at the top. Stopping is the default, because a count that wraps is how a form ends up ordering none of something.\n\n## What it does with bad text\n\nWhat is typed is parsed on Return and on leaving the field. What will not parse is refused and the last good value comes back — keeping nonsense silently is worse than refusing it, and clearing the box would lose the value the person was editing away from. A `suffix` may be left in place or typed out; either parses.\n\n## The shell\n\nIt is a `FieldWell` with a `TextInput` in the input slot and a `NumberSpin` in the trailing one — which is what the well's trailing slot was documented as being for. The well draws the box and carries the focus, so this widget only has to own the arithmetic.\n\n## ValueInput\n\nA small number with an arrow at each end, sized for a toolbar — seventy-six points by twenty-two, with any label of its own outside it. Drag across it to change the value, or click to type one.\n\n`min` and `max` bound it, `step` is the granularity, and `precision` is how many decimals are shown. It reports through `changed`, and `value`/`set_value` read and write it.\n\n**There is no `value` property to declare.** min, max, step and precision are all settable in the DSL and the value is not, so a field starts at zero — clamped up to `min` — until a host calls `set_value`. Writing `value:` in the DSL is accepted by the compiler and rejected by the script at runtime, which is to say it fails where only the log can see it.\n\n## Which number to reach for\n\nThe library has four number controls, and choosing between them is a question of where the number sits:\n\n- **`NumberField`** — a form. Typed, with step buttons that are always visible.\n- **`ValueInput`** — a toolbar. Small, bounded, and you supply the label beside it; a toolbar cannot afford chrome that is only occasionally useful, so its marks stay quiet until the pointer arrives.\n- **`FabValueInput`** — a property panel. A full-width row with the label pinned left and the value right, so a column of them reads as a table; and a press is not a drag until three points of travel. It is shown on the PropertyInspector page.\n- **`DropSlider`** — a window's own chrome, where a sideways drag would fight dragging the window. The chip takes only a click and the slider it opens lives in a popover.\n\nThey look similar in a screenshot and are shaped for different places. Reaching for the toolbar one inside a property panel gives you a column that will not line up; reaching for it in a title bar gives you a drag that competes with the window.",
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
    on_actions: Some(number_page_actions),
}];

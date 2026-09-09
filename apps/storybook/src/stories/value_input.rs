//! The value input story: a small number you can drag or type, and how it
//! differs from the two other things in this library that do that.
use crate::makepad_widgets::value_input::ValueInputWidgetRefExt;
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ValueInputOverview = StoryPage{
        StoryNote{text: "A small number with an arrow at each end. Drag across it to change the value, or click to type one. It is sized for a toolbar rather than a form: seventy-six points by twenty-two, label outside, no gutter."}

        StoryHeading{text: "Drag it, or type in it"}
        StoryNote{text: "Each of these is bounded and stepped differently. The tempo moves in tenths and stops at forty and three hundred; the count is whole numbers only; the gain is fine enough to need two decimals. They all start at their minimum, because there is no value property to declare — a host seeds one with set_value."}
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

        StoryHeading{text: "Which of the three to reach for"}
        StoryNote{text: "The library has three numbers you can drag, and they are not interchangeable. This one is for a toolbar: small, bounded, and its own label. FabValueInput is for a property panel, a full-width row with the label pinned left. DropSlider is for a bar where a drag would fight dragging the window, so its slider lives in a popover and the chip only ever takes a click."}
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

pub const STORIES: &[Story] = &[Story {
    key: "inputs/valueinput/overview",
    category: "Inputs",
    component: "ValueInput",
    also: &[],
    name: "Overview",
    dsl: "ValueInputOverview",
    added: "2025-05-06",
    tags: &[],
    doc: "# ValueInput

A small number with an arrow at each end. Drag across it to change the value, or click to type one.

`min` and `max` bound it, `step` is the granularity, and `precision` is how many decimals are shown. It reports through `changed`, and `value`/`set_value` read and write it.

**There is no `value` property to declare.** min, max, step and precision are all settable in the DSL and the value is not, so a field starts at zero — clamped up to `min` — until a host calls `set_value`. Writing `value:` in the DSL is accepted by the compiler and rejected by the script at runtime, which is to say it fails where only the log can see it. It is sized for a toolbar — seventy-six points by twenty-two, with any label of its own outside it.

**The library has three numbers you can drag, and choosing between them is the whole decision:**

- **`ValueInput`** — a toolbar. Small, bounded, and you supply the label beside it.
- **`FabValueInput`** — a property panel. A full-width row with the label pinned left and the value right, so a column of them reads as a table; and a press is not a drag until three points of travel.
- **`DropSlider`** — a window's own chrome, where a sideways drag would fight dragging the window. The chip takes only a click and the slider it opens lives in a popover.

They look similar in a screenshot and are shaped for three different places. Reaching for the toolbar one inside a property panel gives you a column that will not line up; reaching for it in a title bar gives you a drag that competes with the window.",
    subject: "tempo",
    feature: None,
    controls: &[],
    on_actions: Some(value_input_actions),
}];

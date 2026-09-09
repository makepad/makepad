//! The checkbox beyond on and off: the mixed state a select-all box needs,
//! the error intent, the round box, the label-first row, and the toggle
//! with its knob icons, per-state labels and draggable knob.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.CheckBoxStates = StoryPage{
        StoryHeading{text: "Mixed"}
        StoryNote{text: "A select-all box over a group. The button walks it through Off, On and Mixed; a click on Mixed resolves to On."}
        StoryRow{
            mixed := CheckBox{text: "All items" state: CheckState.Mixed}
            cycle := Button{text: "Cycle state"}
            state_label := Label{text: "state: Mixed"}
        }
        StoryHeading{text: "Circular"}
        StoryRow{
            CheckBoxCircle{text: "Round box"}
            CheckBoxCircle{text: "Round box, on" active: true}
        }
        StoryHeading{text: "Error intent"}
        StoryRow{
            CheckBox{text: "Accept the terms" error: true}
            CheckBox{text: "Checked, still wrong" error: true active: true}
            Toggle{text: "Pill in error" error: true}
        }
        StoryHeading{text: "Label first"}
        StoryRow{
            CheckBox{text: "Label before the box" label_before: true}
            Toggle{text: "Label before the pill" label_before: true}
        }
        StoryHeading{text: "Toggle: drag, growth, outline, labels"}
        StoryNote{text: "Drag the knob along the track; a release past the middle commits. The knob grows while pressed, the pill takes an outline while off, and the label changes with the state."}
        StoryRow{
            switch := Toggle{
                text: "Switch"
                text_on: "Switch is on"
                text_off: "Switch is off"
                draw_bg +: {
                    knob_grow: 0.25
                    outline_size: 2.0
                }
            }
        }
        StoryHeading{text: "Toggle: knob icons"}
        StoryRow{
            Toggle{
                text: "A mark on the knob"
                draw_bg +: {size: 24.0 knob_grow: 0.2}
                label_walk +: {margin: Inset{left: 44.}}
                draw_icon_on +: {
                    svg: crate_resource("self:resources/mark_check.svg")
                    color: theme.color_bg_app
                }
                draw_icon_off +: {
                    svg: crate_resource("self:resources/mark_cross.svg")
                    color: theme.color_label_outer
                }
            }
        }
    }
}

fn state_name(state: CheckState) -> &'static str {
    match state {
        CheckState::Off => "Off",
        CheckState::On => "On",
        CheckState::Mixed => "Mixed",
    }
}

fn checkbox_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let mixed = root.check_box(cx, ids!(mixed));
    if root.button(cx, ids!(cycle)).clicked(actions) {
        let next = match mixed.state(cx) {
            CheckState::Off => CheckState::On,
            CheckState::On => CheckState::Mixed,
            CheckState::Mixed => CheckState::Off,
        };
        mixed.set_state(cx, next, Animate::Yes);
        root.label(cx, ids!(state_label))
            .set_text(cx, &format!("state: {}", state_name(next)));
    }
    if mixed.changed(actions).is_some() {
        let now = mixed.state(cx);
        root.label(cx, ids!(state_label))
            .set_text(cx, &format!("state: {}", state_name(now)));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/checkbox/states",
    category: "Inputs",
    component: "CheckBox",
    also: &["CheckBoxCircle"],
    name: "States",
    dsl: "CheckBoxStates",
    added: "2026-09-05",
    tags: &["new"],
    doc: "# CheckBox states\n\nA checkbox carries a `CheckState`: `Off`, `On` or `Mixed`. Mixed is the select-all box's \"some of them\" state: it draws a dash, reports as not checked, and a click on it resolves to `On`. `state()` and `set_state()` sit beside the unchanged `active()`, `set_active()` and `changed()`.\n\n`error` recolours the box, mark and label with the error ink; `CheckBoxCircle` rounds the box all the way; `label_before` draws the label first and the box at the end of the row.\n\nThe toggle's knob drags along the track and commits on release past the middle, grows while pressed by `knob_grow`, takes an `outline_size` stroke while off, swaps its label through `text_on` and `text_off`, and carries an icon per state in `draw_icon_on` and `draw_icon_off`.\n\nThe controls drive the labelled switch.",
    subject: "switch",
    feature: None,
    controls: &[
        Control { label: "Label", target: "switch", kind: ControlKind::Text { prop: "text", default: "Switch" } },
        Control { label: "Knob growth", target: "switch", kind: ControlKind::Number { prop: "draw_bg.knob_grow", min: 0., max: 1., step: 0.05, default: 0.25 } },
        Control { label: "Outline", target: "switch", kind: ControlKind::Number { prop: "draw_bg.outline_size", min: 0., max: 4., step: 0.5, default: 2. } },
        Control { label: "Disabled", target: "switch", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(checkbox_actions),
}];

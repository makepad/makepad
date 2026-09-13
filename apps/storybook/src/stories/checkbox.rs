//! The checkbox page: one box and one switch under the controls, a counted
//! change, the mixed state a select-all box needs, the faces, the disabled
//! and error faces, and a mark of your own.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};
use std::sync::atomic::{AtomicUsize, Ordering};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.CheckBoxOverview = StoryPage{
        StoryHeading{text: "One checkbox and one switch, under the controls"}
        StoryNote{text: "The controls write the box's label, the switch's label at rest, its knob growth and outline, and can disable either. Drag the switch's knob along the track: a release past the middle commits. The knob grows while pressed, the pill takes an outline while off, and the label changes with the state."}
        StoryRow{
            subject := CheckBox{text: "Option"}
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

        StoryHeading{text: "A change, counted"}
        StoryNote{text: "Every flip raises a change carrying the new value; the label beside the box counts them."}
        StoryRow{
            simplecheckbox := CheckBox{text: "CheckBox"}
            simplecheckbox_output := Label{text: ""}
        }

        StoryHeading{text: "Mixed"}
        StoryNote{text: "A select-all box over a group. The button walks it through Off, On and Mixed; a click on Mixed resolves to On."}
        StoryRow{
            mixed := CheckBox{text: "All items" state: CheckState.Mixed}
            cycle := Button{text: "Cycle state"}
            state_label := Label{text: "state: Mixed"}
        }

        StoryHeading{text: "Faces"}
        StoryRow{
            CheckBox{text: "CheckBox"}
            CheckBoxFlat{text: "CheckBoxFlat"}
            CheckBoxCircle{text: "CheckBoxCircle"}
            CheckBoxCircle{text: "CheckBoxCircle, on" active: true}
        }
        StoryRow{
            Toggle{text: "Toggle"}
            ToggleFlat{text: "ToggleFlat"}
        }

        StoryHeading{text: "Label first"}
        StoryRow{
            CheckBox{text: "Label before the box" label_before: true}
            Toggle{text: "Label before the pill" label_before: true}
        }

        StoryHeading{text: "Disabled"}
        StoryRow{
            CheckBox{
                text: "CheckBox"
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }
        }

        StoryHeading{text: "Error intent"}
        StoryRow{
            CheckBox{text: "Accept the terms" error: true}
            CheckBox{text: "Checked, still wrong" error: true active: true}
            Toggle{text: "Pill in error" error: true}
        }

        StoryHeading{text: "A mark of your own"}
        StoryNote{text: "CheckBoxCustom draws no box, so the icon it is given is the whole mark. A toggle carries one icon per state on its knob."}
        StoryRow{
            CheckBoxCustom{
                text: "CheckBoxCustom"
                align: Align{x: 0. y: 0.5}
                padding: Inset{top: 0. left: 0. bottom: 0. right: 0.}
                margin: Inset{top: 0. left: 0. bottom: 0. right: 0.}

                label_walk: Walk{
                    width: Fit height: Fit
                    margin: theme.mspace_h_1{left: 5.5}
                }

                draw_icon +: {
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                }

                icon_walk: Walk{
                    width: 13.0
                    height: Fit
                }
            }
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

/// How many times the counted checkbox changed.
static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn state_name(state: CheckState) -> &'static str {
    match state {
        CheckState::Off => "Off",
        CheckState::On => "On",
        CheckState::Mixed => "Mixed",
    }
}

/// The counted checkbox writes each change to the label beside it.
fn counter_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if let Some(check) = root.check_box(cx, ids!(simplecheckbox)).changed(actions) {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        log!("CHECK BUTTON CLICKED {} {}", n, check);
        root.label(cx, ids!(simplecheckbox_output))
            .set_text(cx, &format!("{} {}", n + 1, check));
    }
}

/// The cycle button walks the select-all box through its three states, and
/// the readout follows a click on the box itself as well.
fn mixed_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
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

fn overview_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    counter_actions(cx, root, actions);
    mixed_actions(cx, root, actions);
}

pub const STORIES: &[Story] = &[Story {
    key: "selection/checkbox/overview",
    category: "Selection",
    component: "CheckBox",
    also: &["CheckBoxCircle", "CheckBoxCustom", "Toggle", "ToggleFlat"],
    name: "Overview",
    dsl: "CheckBoxOverview",
    added: "2026-02-23",
    tags: &["controls", "ported"],
    doc: "# CheckBox\n\nA checkbox turns one option on or off. `Toggle` is the same widget drawn as a pill with a knob that slides, for a setting that takes effect the moment it flips; a checkbox suits an option that is read later, when a form is sent.\n\n## States\n\nA checkbox carries a `CheckState`: `Off`, `On` or `Mixed`. Mixed is the select-all box's \"some of them\" state: it draws a dash, reports as not checked, and a click on it resolves to `On`. `state()` and `set_state()` sit beside `active()`, `set_active()` and `changed()`.\n\n## The toggle\n\nThe knob drags along the track and commits on release past the middle, grows while pressed by `knob_grow`, takes an `outline_size` stroke while off, and swaps its label through `text_on` and `text_off`.\n\n## Faces\n\n`CheckBox` is the bevelled standard and `CheckBoxFlat` the plain face; `Toggle` and `ToggleFlat` are the same pair as pills. `CheckBoxCircle` rounds the box all the way, and `label_before` draws the label first and the box at the end of the row.\n\n## Disabled and error\n\nDisabled draws the box, its mark and its label in the theme's disabled colours. `set_disabled` plays it, and a box that starts disabled sets its animator's `disabled` state to `@on`, as the one on this page does.\n\n`error` recolours the box, mark and label with the error ink, whether the box is on or off.\n\n## Styling\n\n`CheckBoxCustom` draws no box of its own, so the icon in `draw_icon` is the whole mark. A toggle carries an icon per state on its knob, in `draw_icon_on` and `draw_icon_off`.\n\nThe controls drive the checkbox and the switch beside it.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Label", target: "subject", kind: ControlKind::Text { prop: "text", default: "Option" } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        Control { label: "Switch label", target: "switch", kind: ControlKind::Text { prop: "text_off", default: "Switch is off" } },
        Control { label: "Knob growth", target: "switch", kind: ControlKind::Number { prop: "draw_bg.knob_grow", min: 0., max: 1., step: 0.05, default: 0.25 } },
        Control { label: "Outline", target: "switch", kind: ControlKind::Number { prop: "draw_bg.outline_size", min: 0., max: 4., step: 0.5, default: 2. } },
        Control { label: "Switch disabled", target: "switch", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(overview_actions),
}];

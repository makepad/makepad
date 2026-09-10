//! The check box stories: the check box ladder, the toggles, a custom icon
//! check box and a change counter, ported from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};
use std::sync::atomic::{AtomicUsize, Ordering};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.CheckBoxOverview = StoryPage{
        H4{text: "Checkbox"}
        CheckBox{text: "CheckBox"}

        Hr{}
        H4{text: "Checkbox, disabled"}
        CheckBox{
            text: "CheckBox"
            animator +: {
                disabled: {
                    default: @on
                }
            }
        }

        Hr{}
        H4{text: "CheckBoxFlat"}
        CheckBoxFlat{text: "CheckBoxFlat"}

        Hr{}
        H4{text: "Toggle"}
        StoryRow{
            Toggle{text: "Toggle"}
        }

        Hr{}
        H4{text: "ToggleFlat"}
        StoryRow{
            ToggleFlat{text: "ToggleFlat"}
        }

        Hr{}
        H4{text: "Output demo"}
        StoryRow{
            height: Fit
            flow: Right
            align: Align{x: 0.0 y: 0.5}
            simplecheckbox := CheckBox{text: "CheckBox"}
            simplecheckbox_output := Label{text: ""}
        }

        Hr{}
        H4{text: "Custom Checkbox"}
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
        }
    

        StoryHeading{text: "One checkbox, under the controls"}
        StoryNote{text: "One checkbox and one toggle, driven from the controls."}
        StoryRow{
            subject := CheckBox{text: "Option"}
            toggle := Toggle{text: "Switch"}
        }
    }
}

/// How many times the output demo's check box changed.
static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn overview_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if let Some(check) = root.check_box(cx, ids!(simplecheckbox)).changed(actions) {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        log!("CHECK BUTTON CLICKED {} {}", n, check);
        root.label(cx, ids!(simplecheckbox_output))
            .set_text(cx, &format!("{} {}", n + 1, check));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/checkbox/overview",
    category: "Inputs",
    component: "CheckBox",
    also: &["CheckBoxCustom", "Toggle", "ToggleFlat"],
    name: "Overview",
    dsl: "CheckBoxOverview",
    added: "2026-02-23",
    tags: &["controls", "ported"],
    doc: "# CheckBox\n\nCheckboxes allow toggling options on/off.",
    subject: "",
    feature: None,
    controls: &[
            Control { label: "Label", target: "subject", kind: ControlKind::Text { prop: "text", default: "Option" } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
    on_actions: Some(overview_actions),
}];

//! The radio button stories: the radio ladder and the two tab groups, each kept exclusive by a radio set on the story root, ported from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.RadioButtonOverview = StoryPage{
        H4{text: "Default"}
        StoryRow{
            radios_demo_1 := View{
                spacing: theme.space_2
                width: Fit height: Fit
                radio1 := RadioButton{text: "Option 1"}
                radio2 := RadioButton{text: "Option 2"}
                radio3 := RadioButton{text: "Option 3"}
                radio4 := RadioButton{
                    text: "Option 4, disabled"
                    animator +: {
                        disabled: {
                            default: @on
                        }
                    }
                }
            }
        }

        Hr{}
        H4{text: "RadioButtonFlat"}
        StoryRow{
            radios_demo_2 := View{
                spacing: theme.space_2
                width: Fit height: Fit
                radio1 := RadioButtonFlat{text: "Option 1"}
                radio2 := RadioButtonFlat{text: "Option 2"}
                radio3 := RadioButtonFlat{text: "Option 3"}
                radio4 := RadioButtonFlat{text: "Option 4"}
            }
        }

        Hr{}
        H4{text: "RadioButtonFlatter"}
        StoryRow{
            radios_demo_3 := View{
                spacing: theme.space_2
                width: Fit height: Fit
                radio1 := RadioButtonFlatter{text: "Option 1"}
                radio2 := RadioButtonFlatter{text: "Option 2"}
                radio3 := RadioButtonFlatter{text: "Option 3"}
                radio4 := RadioButtonFlatter{text: "Option 4"}
            }
        }

        Hr{}
        H4{text: "Button Group"}
        radios_demo_11 := View{
            spacing: theme.space_2
            width: Fit height: Fit
            flow: Right
            radio1 := RadioButtonTab{text: "Option 1"}
            radio2 := RadioButtonTab{text: "Option 2"}
            radio3 := RadioButtonTab{text: "Option 3"}
            radio4 := RadioButtonTab{
                text: "Option 4, disabled"
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }
        }

        Hr{}
        H4{text: "Button Group Flat"}
        radios_demo_12 := View{
            spacing: theme.space_2
            width: Fit height: Fit
            flow: Right
            radio1 := RadioButtonTabFlat{text: "Option 1"}
            radio2 := RadioButtonTabFlat{text: "Option 2"}
            radio3 := RadioButtonTabFlat{text: "Option 3"}
            radio4 := RadioButtonTabFlat{text: "Option 4"}
        }
    }
}

/// Each demo group is a radio set: selecting one deselects the others.
fn overview_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    root.radio_button_set(
        cx,
        ids_array!(radios_demo_1.radio1, radios_demo_1.radio2, radios_demo_1.radio3, radios_demo_1.radio4),
    )
    .selected(cx, actions);

    root.radio_button_set(
        cx,
        ids_array!(radios_demo_2.radio1, radios_demo_2.radio2, radios_demo_2.radio3, radios_demo_2.radio4),
    )
    .selected(cx, actions);

    root.radio_button_set(
        cx,
        ids_array!(radios_demo_3.radio1, radios_demo_3.radio2, radios_demo_3.radio3, radios_demo_3.radio4),
    )
    .selected(cx, actions);

    root.radio_button_set(
        cx,
        ids_array!(radios_demo_11.radio1, radios_demo_11.radio2, radios_demo_11.radio3, radios_demo_11.radio4),
    )
    .selected(cx, actions);

    root.radio_button_set(
        cx,
        ids_array!(radios_demo_12.radio1, radios_demo_12.radio2, radios_demo_12.radio3, radios_demo_12.radio4),
    )
    .selected(cx, actions);
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/radiobutton/overview",
    category: "Inputs",
    component: "RadioButton",
    also: &["RadioButtonTab", "RadioButtonTabFlat"],
    name: "Overview",
    dsl: "RadioButtonOverview",
    added: "2026-02-16",
    tags: &["ported"],
    doc: "# RadioButton\n\nRadio buttons allow selecting one option from a group.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: Some(overview_actions),
}];

//! The radio group page: one question, a set of answers, and one keyboard
//! stop for the lot of them; then the buttons a group is made of, on their
//! own.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.RadioGroupOverview = StoryPage{
        StoryNote{text: "A group finds its RadioButton children at draw time and gives them one shared answer: one row lit, one tab stop for the whole set, and arrows that move the answer rather than only the focus. Tab into a group below and try the arrow keys, Home and End."}

        StoryHeading{text: "A question and its answers"}
        StoryRow{
            subject := mod.widgets.RadioGroup{
                text: "Delivery"
                RadioButton{text: "Standard"}
                RadioButton{text: "Express"}
                RadioButton{text: "Collect in person"}
            }
        }
        StoryRow{
            answer_note := Label{text: "Standard"}
        }

        StoryHeading{text: "Across the page instead of down it"}
        StoryNote{text: "`RadioGroupRow` is the same group with its flow turned. Both arrow pairs work either way round: a group laid across the page is still a column to somebody who reaches for Down first, and a key that does nothing reads as a broken control."}
        StoryRow{
            sizes := mod.widgets.RadioGroupRow{
                text: "Size"
                RadioButton{text: "Small"}
                RadioButton{text: "Medium"}
                RadioButton{text: "Large"}
            }
        }
        StoryRow{
            size_note := Label{text: "Small"}
        }

        StoryHeading{text: "Answers that do not have to be direct children"}
        StoryNote{text: "The group walks its subtree, so the answers may sit inside rows, columns or any other wrapper and still belong to the same question."}
        StoryRow{
            seats := mod.widgets.RadioGroupRow{
                text: "Seat"
                spacing: theme.space_3
                View{
                    width: Fit
                    height: Fit
                    flow: Down
                    spacing: theme.space_1
                    RadioButton{text: "Window"}
                    RadioButton{text: "Middle"}
                }
                View{
                    width: Fit
                    height: Fit
                    flow: Down
                    spacing: theme.space_1
                    RadioButton{text: "Aisle"}
                    RadioButton{text: "Anywhere"}
                }
            }
        }

        StoryHeading{text: "The buttons on their own"}
        StoryNote{text: "A RadioButton outside a group knows only whether it is lit. Each row below is kept to one answer by a radio set in the page's handler instead, which is the hand-rolled set the group replaces: one tab stop per answer, and arrow keys that do nothing. The rows are the three faces; the fourth answer in the first row is disabled."}
        StoryRow{
            Label{width: 150. text: "RadioButton"}
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
        StoryRow{
            Label{width: 150. text: "RadioButtonFlat"}
            radios_demo_2 := View{
                spacing: theme.space_2
                width: Fit height: Fit
                radio1 := RadioButtonFlat{text: "Option 1"}
                radio2 := RadioButtonFlat{text: "Option 2"}
                radio3 := RadioButtonFlat{text: "Option 3"}
                radio4 := RadioButtonFlat{text: "Option 4"}
            }
        }
        StoryRow{
            Label{width: 150. text: "RadioButtonFlatter"}
            radios_demo_3 := View{
                spacing: theme.space_2
                width: Fit height: Fit
                radio1 := RadioButtonFlatter{text: "Option 1"}
                radio2 := RadioButtonFlatter{text: "Option 2"}
                radio3 := RadioButtonFlatter{text: "Option 3"}
                radio4 := RadioButtonFlatter{text: "Option 4"}
            }
        }
        StoryNote{text: "A row of choices drawn as buttons is a segmented control, on the ButtonGroup page, or a chip group, on the Chip page."}

        StoryHeading{text: "Answers that cannot be taken"}
        StoryNote{text: "A disabled answer is stepped straight past by the arrows, and Home and End go to the first and last answer that can actually be taken. A stop on an answer no press can reach is a dead end with nothing on screen to explain it."}
        StoryRow{
            plans := mod.widgets.RadioGroup{
                text: "Plan"
                RadioButton{
                    text: "Free, no longer offered"
                    animator +: {disabled: {default: @on}}
                }
                RadioButton{text: "Monthly"}
                RadioButton{
                    text: "Quarterly, sold out"
                    animator +: {disabled: {default: @on}}
                }
                RadioButton{text: "Yearly"}
            }
        }
        StoryRow{
            plan_note := Label{text: "Monthly"}
        }

        StoryHeading{text: "Without a shared label"}
        StoryNote{text: "Leave `text` empty for a group whose host has already asked the question in its own words. The room above the answers goes with it."}
        StoryRow{
            mod.widgets.RadioGroupRow{
                RadioButton{text: "Yes"}
                RadioButton{text: "No"}
            }
        }

        StoryHeading{text: "The whole question unavailable"}
        StoryNote{text: "`disabled: true` reaches every answer and lets go again without dragging the answers that were disabled on their own back up with it."}
        StoryRow{
            mod.widgets.RadioGroup{
                text: "Postage"
                disabled: true
                RadioButton{text: "First class"}
                RadioButton{text: "Second class"}
            }
        }
    }
}

/// The readouts follow the answers. `selected` reports only a move, so a
/// second press on the answer already taken writes nothing.
fn group_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let subject = root.radio_group(cx, ids!(subject));
    if subject.selected(actions).is_some() {
        let text = subject.answer_text();
        root.label(cx, ids!(answer_note)).set_text(cx, &text);
    }
    let sizes = root.radio_group(cx, ids!(sizes));
    if sizes.selected(actions).is_some() {
        let text = sizes.answer_text();
        root.label(cx, ids!(size_note)).set_text(cx, &text);
    }
    let plans = root.radio_group(cx, ids!(plans));
    if let Some(index) = plans.selected(actions) {
        let text = plans.answer_text();
        root.label(cx, ids!(plan_note))
            .set_text(cx, &format!("{text} (answer {index})"));
    }
}

/// Each row of plain buttons is a radio set: selecting one deselects the
/// others in that row.
fn radio_set_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
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
}

fn overview_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    group_actions(cx, root, actions);
    radio_set_actions(cx, root, actions);
}

pub const STORIES: &[Story] = &[Story {
    key: "selection/radiogroup/overview",
    category: "Selection",
    component: "RadioGroup",
    also: &["RadioGroupRow", "RadioButton", "RadioButtonTab", "RadioButtonTabFlat"],
    name: "Overview",
    dsl: "RadioGroupOverview",
    added: "2026-09-10",
    tags: &["new", "controls", "radio", "choice", "one of", "exclusive", "keyboard"],
    doc: "# RadioGroup\n\nOne question, a set of RadioButton children, and exactly one answer shared between them.\n\nA radio button on its own knows whether it is lit and nothing else; making a set of them exclusive is left to whoever puts them on a page. Every hand-rolled set has the same two holes — it is one tab stop per answer instead of one for the question, and the arrow keys do nothing, so somebody working by keyboard has to Tab through every answer and still cannot change the one that is taken.\n\n## Keyboard\n\n| Key | Result |\n|---|---|\n| Tab | into the group, and out of it again — one stop for the whole set |\n| arrows | move the ANSWER, not merely the focus |\n| Home / End | the first and last answer that can be taken |\n\nThe arrows moving the answer is the behaviour that makes a group usable by keyboard. Moving the focus alone and waiting for a second press to take the answer is how a list of rows behaves, and somebody who has to press twice reads the first press as nothing having happened. Both arrow pairs work whichever way the group runs.\n\n`wrap: true` carries the arrows on round from either end; turn it off and they stop.\n\n## Reading it\n\n`selected` reports the index of the answer when it moves — a second press on the answer already taken raises nothing, because a group never lets go. `answer_text` is the label of the answer being held, and `answer` is its index, or nothing when no answer can be taken.\n\n## The buttons on their own\n\n`RadioButton` is the row a group is made of, in three faces: `RadioButton`, `RadioButtonFlat`, and `RadioButtonFlatter`, which draws no circle and shows the answer in its label colour alone. Outside a group, a `radio_button_set` over their ids in a host's handler makes a handful of them exclusive; that is the hand-rolled set described above. `RadioButtonTab` and `RadioButtonTabFlat` draw an answer as a tab. For a row of choices drawn as buttons, use a segmented control, on the ButtonGroup page, or a chip group, on the Chip page.\n\n## Answers that cannot be taken\n\nA row may be disabled on its own, and the arrows step straight past it. Home and End go to the first and last answer that can be taken, not to the ends of the set. If every answer is disabled the group holds no answer at all rather than lighting one no press can reach.\n\nThe group's own `disabled` reaches every answer and lets go again without dragging the answers that were disabled on their own back up with it.\n\n## What it is not\n\nIt does not own its answers as data: the rows are real widgets, so any radio preset in the library can be an answer, with its own icon and its own shader. A control that holds a list of words and draws them itself is a segmented control, which lives on its own page. And a set of answers where the current one can be un-taken by pressing it again is a set of checkboxes.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Label", target: "subject", kind: ControlKind::Text { prop: "text", default: "Delivery" } },
        Control { label: "Answer", target: "subject", kind: ControlKind::Number { prop: "selected", min: 0., max: 2., step: 1., default: 0. } },
        Control { label: "Arrows wrap", target: "subject", kind: ControlKind::Bool { prop: "wrap", default: true } },
        Control { label: "Label room", target: "subject", kind: ControlKind::Number { prop: "label_height", min: 0., max: 48., step: 1., default: 18. } },
        Control { label: "Spacing", target: "subject", kind: ControlKind::Number { prop: "spacing", min: 0., max: 24., step: 1., default: 3. } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(overview_actions),
}];

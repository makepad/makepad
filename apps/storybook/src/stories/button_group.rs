//! The group stories: buttons that belong together, and one question with
//! a fixed set of answers.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ButtonGroupOverview = StoryPage{
        StoryNote{text: "A button group lays separate verbs out together and shapes their corners. A segmented control is a different thing: one question with a fixed set of answers, and a pill on the current one."}

        StoryHeading{text: "Connected"}
        StoryNote{text: "The inner corners collapse, so the row reads as one object."}
        StoryRow{
            ButtonGroup{
                Button{text: "Cut"}
                Button{text: "Copy"}
                Button{text: "Paste"}
            }
        }

        StoryHeading{text: "Spaced"}
        StoryNote{text: "Every button keeps its own shape and its own gap."}
        StoryRow{
            ButtonGroupSpaced{
                Button{text: "Cut"}
                Button{text: "Copy"}
                Button{text: "Paste"}
            }
        }

        StoryHeading{text: "Down a column"}
        StoryRow{
            ButtonGroup{
                axis: GroupAxis.Vertical
                flow: Down
                Button{text: "Top"}
                Button{text: "Middle"}
                Button{text: "Bottom"}
            }
        }

        StoryHeading{text: "Segmented control"}
        StoryNote{text: "The answers are data, not children: the control draws the words, keeps the current one and reports its index. The pill glides."}
        StoryRow{
            period := SegmentedControl{
                options: ["Day" "Week" "Month"]
                selected: 0
            }
        }
        StoryRow{
            period_note := Label{text: "period: Day"}
        }

        StoryHeading{text: "Equal width"}
        StoryNote{text: "Every segment as wide as the widest, so the row does not shuffle when the answer changes."}
        StoryRow{
            SegmentedControl{
                options: ["S" "Medium" "Extra large"]
                equal_width: true
                selected: 1
            }
        }

        StoryHeading{text: "Down a column"}
        StoryRow{
            SegmentedControlVertical{
                options: ["North" "East" "South" "West"]
                selected: 2
            }
        }

        StoryHeading{text: "Several at once"}
        StoryNote{text: "A toggle group is the same control with more than one answer allowed: a set of switches sharing a shape."}
        StoryRow{
            style := ToggleGroup{
                options: ["Bold" "Italic" "Underline"]
            }
        }
        StoryRow{
            style_note := Label{text: "on: nothing"}
        }

        StoryHeading{text: "Offering the rest"}
        StoryNote{text: "A split button does the thing its label names on the wide half, and offers the rest on the chevron. A menu button only offers: its label is a verb and never changes to whatever was picked."}
        StoryRow{
            save := SplitButton{}
            actions_menu := MenuButton{}
        }
        StoryRow{
            offered := Label{text: "nothing chosen yet"}
        }

        StoryHeading{text: "Roles and disabled"}
        StoryRow{
            SegmentedControl{options: ["One" "Two"] intent: Primary}
            SegmentedControl{options: ["One" "Two"] intent: Success selected: 1}
            SegmentedControl{options: ["One" "Two"] disabled: true}
        }

        menus := MenuLayer{}
    }

    mod.stories.SegmentedControlBasic = StoryPage{
        StoryNote{text: "One segmented control. Every control on the right writes into it."}
        StoryRow{
            subject := SegmentedControl{
                options: ["Day" "Week" "Month"]
            }
        }
        StoryRow{
            state := Label{text: "answer: Day"}
        }
    }
}

fn share_rows() -> Vec<MenuRow> {
    vec![
        MenuRow::new(live_id!(save_copy), "Save a copy"),
        MenuRow::new(live_id!(save_all), "Save all").key("Ctrl+Alt+S"),
        MenuRow::separator(),
        MenuRow::new(live_id!(revert), "Revert").danger(true),
    ]
}

fn action_rows() -> Vec<MenuRow> {
    vec![
        MenuRow::new(live_id!(duplicate), "Duplicate"),
        MenuRow::new(live_id!(rename), "Rename…"),
        MenuRow::separator(),
        MenuRow::new(live_id!(archive), "Archive"),
    ]
}

fn group_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let split = root.split_button(cx, ids!(save));
    split.set_rows(share_rows());
    let menu_button = root.menu_button(cx, ids!(actions_menu));
    menu_button.set_rows(action_rows());
    if split.clicked(actions) {
        root.label(cx, ids!(offered)).set_text(cx, "saved");
    }
    for action in menu_actions(actions) {
        if let MenuAction::Picked { owner, id } = action {
            // The host knows its own rows, so the caption says what was
            // chosen rather than the id it came back as.
            let (who, rows) = if *owner == split.menu_owner() {
                ("split", share_rows())
            } else {
                ("menu button", action_rows())
            };
            let label = rows
                .iter()
                .find(|row| row.id == *id)
                .map(|row| row.label.clone())
                .unwrap_or_default();
            root.label(cx, ids!(offered)).set_text(cx, &format!("{who}: {label}"));
        }
    }
    let period = root.segmented_control(cx, ids!(period));
    if period.selected(actions).is_some() {
        let text = format!("period: {}", period.selected_text());
        root.label(cx, ids!(period_note)).set_text(cx, &text);
    }
    let style = root.segmented_control(cx, ids!(style));
    if style.selected(actions).is_some() {
        let on = style.selected_text();
        let text = if on.is_empty() { "on: nothing".to_string() } else { format!("on: {on}") };
        root.label(cx, ids!(style_note)).set_text(cx, &text);
    }
}

fn segmented_basic_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let subject = root.segmented_control(cx, ids!(subject));
    if let Some(index) = subject.selected(actions) {
        let text = format!("answer: {} ({index})", subject.selected_text());
        root.label(cx, ids!(state)).set_text(cx, &text);
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "actions/buttongroup/overview",
        category: "Actions",
        component: "ButtonGroup",
        also: &["ButtonGroupSpaced", "MenuButton", "SegmentedControlVertical", "SplitButton", "ToggleGroup"],
        name: "Overview",
        dsl: "ButtonGroupOverview",
        added: "2026-09-05",
        tags: &["new"],
        doc: "# Button group and segmented control\n\nTwo shapes that are easy to confuse, so the library names them apart.\n\nA **button group** is a set of separate verbs sharing a row: cut, copy, paste. It reports which button was pressed and forgets. `join: Connected` collapses the inner corners so the row reads as one object; `Spaced` leaves every button its own shape. The group only lays out and shapes, so any button in the library can sit in one.\n\nA **segmented control** is one question with a fixed set of answers: day, week, month. It remembers which answer is current and shows it under a pill that glides over `theme.motion_short_4`. Its answers are `options`, a list of words it owns, rather than child widgets a caller has to keep in step with the selection.\n\nSet `selection: Multi` and the same control becomes a toggle group, where any number of answers can be on at once. The row is one tab stop: the arrows walk it, and with a single answer they move the answer as they go.",
        subject: "period",
        feature: None,
        controls: &[],
        on_actions: Some(group_actions),
    },
    Story {
        key: "actions/buttongroup/basic",
        category: "Actions",
        component: "ButtonGroup",
        also: &["SegmentedControl"],
        name: "Basic",
        dsl: "SegmentedControlBasic",
        added: "2026-09-05",
        tags: &["new", "controls"],
        doc: "# Segmented control\n\nOne control under the controls: which answer is current, whether several may be, whether the segments share a width, and which way the row runs.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Selected", target: "subject", kind: ControlKind::Number { prop: "selected", min: 0., max: 2., step: 1., default: 0. } },
            Control { label: "Equal width", target: "subject", kind: ControlKind::Bool { prop: "equal_width", default: false } },
            Control { label: "Vertical", target: "subject", kind: ControlKind::Bool { prop: "vertical", default: false } },
            Control { label: "Height", target: "subject", kind: ControlKind::Number { prop: "segment_height", min: 18., max: 56., step: 1., default: 30. } },
            Control { label: "Padding", target: "subject", kind: ControlKind::Number { prop: "segment_padding", min: 4., max: 40., step: 1., default: 12. } },
            Control { label: "Glide (s)", target: "subject", kind: ControlKind::Number { prop: "glide_secs", min: 0., max: 1., step: 0.05, default: 0.2 } },
            Control { label: "Intent", target: "subject", kind: ControlKind::Choice { prop: "intent", options: &["Neutral", "Primary", "Secondary", "Tertiary", "Error", "Warning", "Success", "Info"], default: 0 } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
        on_actions: Some(segmented_basic_actions),
    },
];

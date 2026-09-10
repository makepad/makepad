//! The drop down story: a face that names the current value and opens a
//! list of the rest.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let VALUES = ["Value One" "Value Two" "Third" "Fourth Value" "Option E" "Hexagons"]

    mod.stories.DropDownOverview = StoryPage{
        StoryNote{text: "A closed face carrying the current value. Pressing it drops the list; choosing a row closes it again. Nothing here is typed, so the face is never mistaken for a field someone is meant to fill in."}

        StoryHeading{text: "Choosing a value"}
        StoryRow{
            subject := DropDown{labels: VALUES}
            chosen := Label{text: "chosen: Value One"}
        }

        StoryHeading{text: "Disabled"}
        StoryNote{text: "The value stays legible — a control that is off still has to say what it is set to."}
        StoryRow{
            DropDown{
                labels: VALUES
                animator +: {disabled: {default: @on}}
            }
        }

        StoryHeading{text: "The ladder"}
        StoryNote{text: "The same widget with the library's four faces: flat, the standard bevel, and the two gradient axes."}
        StoryRow{
            DropDownFlat{labels: VALUES}
            DropDown{labels: VALUES}
        }
        StoryRow{
            DropDownGradientX{labels: VALUES}
            DropDownGradientY{labels: VALUES}
        }
    }
}

fn dropdown_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let subject = root.drop_down(cx, ids!(subject));
    if subject.changed(actions).is_some() {
        let text = format!("chosen: {}", subject.selected_label());
        root.label(cx, ids!(chosen)).set_text(cx, &text);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/dropdown/overview",
    category: "Inputs",
    component: "DropDown",
    also: &["DropDownFlat", "DropDownGradientX", "DropDownGradientY"],
    name: "Overview",
    dsl: "DropDownOverview",
    added: "2026-08-23",
    tags: &["ported", "select", "picker", "menu"],
    doc: "# DropDown\n\nA face showing the value in force, and a list of the alternatives behind it. The face is a button, not a field: it takes no typing, and its shape says so.\n\nReach for it when the list is short enough to read, when every option is a real answer rather than a filter, and when the current value is worth showing all the time. When the list gets long enough that scanning it is work, a `ComboBox` lets the person type three letters instead. When the answer is a set rather than one value, a `MultiSelect` shows the count and keeps the list open.",
    subject: "subject",
    feature: None,
    controls: &[],
    on_actions: Some(dropdown_actions),
}];

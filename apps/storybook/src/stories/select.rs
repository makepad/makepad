//! The select stories: one value, or a set of switches.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.SelectOverview = StoryPage{
        StoryNote{text: "A dropdown is a menu with a value, so it uses the same rows the menu does. A select shows one value and offers the rest; a multi-select shows a set, puts a check on every chosen row and stays open while they are pressed."}

        StoryHeading{text: "One value"}
        StoryRow{
            deck := Select{
                options: ["Deck A" "Deck B" "Deck C" "Deck D"]
                placeholder: "Choose a deck"
            }
            deck_note := Label{text: "nothing chosen"}
        }

        StoryHeading{text: "A set of switches"}
        StoryNote{text: "Every row carries a check, the list stays open while they are pressed, and the face says how many are on rather than naming one of them."}
        StoryRow{
            stems := MultiSelect{
                options: ["Drums" "Bass" "Vocals" "Other"]
                placeholder: "No stems"
            }
            stems_note := Label{text: "none"}
        }

        StoryHeading{text: "Naming them instead"}
        StoryNote{text: "A caller who knows the list is short can ask for the names on the face rather than the count."}
        StoryRow{
            flags := MultiSelect{
                options: ["Loop" "Sync"]
                placeholder: "Off"
                count_summary: false
            }
        }

        menus := MenuLayer{}
    }
}

fn select_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let deck = root.select(cx, ids!(deck));
    if deck.changed(actions).is_some() {
        let text = format!("chosen: {}", deck.face_text());
        root.label(cx, ids!(deck_note)).set_text(cx, &text);
    }
    let stems = root.select(cx, ids!(stems));
    if stems.toggled(actions).is_some() {
        let chosen = stems.chosen_labels();
        let text = if chosen.is_empty() { "none".to_string() } else { chosen.join(", ") };
        root.label(cx, ids!(stems_note)).set_text(cx, &text);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/select/overview",
    category: "Inputs",
    component: "Select",
    also: &["MultiSelect"],
    name: "Overview",
    dsl: "SelectOverview",
    added: "2026-09-05",
    tags: &["new"],
    doc: "# Select\n\nA dropdown is a menu with a value. Rather than grow a second row model beside the menu's, a select builds `MenuRow`s and hands them to the one `MenuLayer` an app already declares, so a dropdown row can carry everything a menu row can — a mark, a heading, a disabled state — and anything added for menus is immediately available here.\n\n`MultiSelect` makes every row a switch: a check on each chosen row, and the list **stays open** while they are pressed, because a set is not finished after one press. Its face then says how many are chosen rather than naming one of them, since \"Bass\" would be a lie about the other two; `count_summary: false` asks for the names instead, for a list short enough to fit.\n\nThe values are `options`, a list the control owns, the way a segmented control owns its answers. A dropdown assembled from child widgets makes the caller keep the children and the value in step by hand.\n\nThe older `DropDown` is untouched: it carries an icon list and an icon-only face that several apps drive by name.",
    subject: "deck",
    feature: None,
    controls: &[],
    on_actions: Some(select_actions),
}];

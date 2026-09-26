//! The select page: one value, a set of switches, and the classic dropdown
//! face that names the current value and opens a list of the rest.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let VALUES = ["Value One" "Value Two" "Third" "Fourth Value" "Option E" "Hexagons"]

    mod.stories.SelectOverview = StoryPage{
        StoryNote{text: "A dropdown is a menu with a value, so it uses the same rows the menu does. A select shows one value and offers the rest; a multi-select shows a set, puts a check on every chosen row and stays open while they are pressed. DropDown, the classic face, is further down, and a list long enough to type into is a combo box, on its own page."}

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

        StoryHeading{text: "The classic face"}
        StoryNote{text: "DropDown: a closed face carrying the current value. Pressing it drops the list; choosing a row closes it again. Nothing here is typed, so the face is never mistaken for a field someone is meant to fill in."}
        StoryRow{
            classic := DropDown{labels: VALUES}
            classic_note := Label{text: "chosen: Value One"}
        }
        StoryNote{text: "The same widget with the library's four faces: flat, the standard bevel, and the two gradient axes."}
        StoryRow{
            DropDownFlat{labels: VALUES}
            DropDown{labels: VALUES}
        }
        StoryRow{
            DropDownGradientX{labels: VALUES}
            DropDownGradientY{labels: VALUES}
        }

        StoryHeading{text: "Disabled"}
        StoryNote{text: "The value stays legible: a control that is off still has to say what it is set to."}
        StoryRow{
            DropDown{
                labels: VALUES
                animator +: {disabled: {default: @on}}
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

/// The classic face's readout follows its value.
fn dropdown_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let classic = root.drop_down(cx, ids!(classic));
    if classic.changed(actions).is_some() {
        let text = format!("chosen: {}", classic.selected_label());
        root.label(cx, ids!(classic_note)).set_text(cx, &text);
    }
}

fn overview_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    select_actions(cx, root, actions);
    dropdown_actions(cx, root, actions);
}

pub const STORIES: &[Story] = &[Story {
    key: "selection/select/overview",
    category: "Selection",
    component: "Select",
    also: &["MultiSelect", "DropDown", "DropDownFlat", "DropDownGradientX", "DropDownGradientY"],
    name: "Overview",
    dsl: "SelectOverview",
    added: "2026-09-05",
    tags: &["new", "select", "picker", "menu"],
    doc: "# Select\n\nOne value out of a list the control owns, or a set of them.\n\n## Which one to use\n\n| Control | Reach for it when |\n|---|---|\n| `Select` | the list is short enough to read, every option is a real answer rather than a filter, and the current value is worth showing all the time |\n| `MultiSelect` | the answer is a set rather than one value |\n| `DropDown` | the rows carry icons, or the closed face has room for an icon and nothing else |\n| `ComboBox` | the list is long enough that typing three letters beats scrolling: a font, a colour, a key signature. It has its own page, Combo box |\n\nNone of them takes free text: a combo box filters its list as it is typed into, but commits only items from that list.\n\n## One value\n\nA dropdown is a menu with a value. Rather than grow a second row model beside the menu's, a select builds `MenuRow`s and hands them to the one `MenuLayer` an app already declares, so a dropdown row can carry everything a menu row can — a mark, a heading, a disabled state — and anything added for menus is immediately available here.\n\nThe values are `options`, a list the control owns, the way a segmented control owns its answers. A dropdown assembled from child widgets makes the caller keep the children and the value in step by hand.\n\n## A set\n\n`MultiSelect` makes every row a switch: a check on each chosen row, and the list **stays open** while they are pressed, because a set is not finished after one press. Its face then says how many are chosen rather than naming one of them, since \"Bass\" would be a lie about the other two; `count_summary: false` asks for the names instead, for a list short enough to fit.\n\n## The classic face\n\n`DropDown` is a face showing the value in force, and a list of the alternatives behind it. It takes `labels` rather than `options`, `icons` gives the rows an icon each, and `icon_only` draws the closed face as the chosen row's icon alone. `DropDownFlat` is the plain face, `DropDown` the standard bevel, and `DropDownGradientX` and `DropDownGradientY` shade it along either axis.\n\n## Disabled\n\nA disabled dropdown keeps its value legible: a control that is off still has to say what it is set to.",
    subject: "deck",
    feature: None,
    controls: &[],
    on_actions: Some(overview_actions),
}];

//! The combo box story: a field you can type into that only ever commits
//! one of its own items.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let COLOURS = [
        "amber" "azure" "basalt" "beacon" "bramble" "cinder" "citrine" "cobalt"
        "coral" "cypress" "dahlia" "dusk" "ember" "fathom" "fennel" "flint"
        "garnet" "gossamer" "harbor" "indigo" "juniper" "kestrel" "lantern" "lichen"
        "marble" "meadow" "nimbus" "onyx" "opal" "pewter" "quarry" "quill"
        "russet" "saffron" "slate" "thistle" "umber" "verdant" "willow" "zephyr"
    ]

    mod.stories.ComboBoxOverview = StoryPage{
        StoryNote{text: "A field with a list behind it: type to narrow the list, press Return to take the match. It looks like a text box and behaves like a selector — half-typed text is never a value, so leaving the field puts back whatever was last committed."}

        StoryHeading{text: "Type to narrow it"}
        StoryRow{
            subject := ComboBox{
                width: 220
                labels: ["Value One" "Value Two" "Third" "Fourth Value" "Option E" "Hexagons"]
            }
            chosen := Label{text: "chosen: Value One"}
        }
        StoryNote{text: "Down opens the list, the arrows walk it, Return takes the one under the highlight and Escape puts the old answer back. Typing something no item matches leaves the field editable and commits nothing."}

        StoryHeading{text: "A list longer than the popup"}
        StoryNote{text: "Forty items in a popup twelve rows tall. The list scrolls, the highlight stays in view, and the filter is what makes a long list usable at all."}
        StoryRow{
            long := ComboBox{
                width: 220
                labels: COLOURS
            }
            long_note := Label{text: "chosen: amber"}
        }

        StoryHeading{text: "How many rows to show"}
        StoryNote{text: "The same forty items, four rows at a time."}
        StoryRow{
            short_popup := ComboBox{
                width: 220
                max_visible_items: 4
                labels: COLOURS
            }
        }

        StoryHeading{text: "When nothing matches"}
        StoryNote{text: "Type \"zzz\" into either box: the popup says so rather than closing, because a closed popup reads as a committed answer."}

        StoryHeading{text: "The face"}
        StoryNote{text: "One face, under two names: `ComboBox` is declared as a `ComboBoxFlat` with nothing added. Most families in the library part here — flat, bevelled, gradient — and this one has had no reason to. Use the plain name."}
        StoryRow{
            ComboBoxFlat{
                width: 200
                labels: ["ComboBoxFlat" "the same face" "under the other name"]
            }
        }
    }
}

fn combobox_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    for (id, note) in [(ids!(subject), ids!(chosen)), (ids!(long), ids!(long_note))] {
        let combo = root.combo_box(cx, id);
        if let Some(label) = combo.changed_label(actions) {
            root.label(cx, note).set_text(cx, &format!("chosen: {label}"));
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "selection/select/combo-box",
    category: "Selection",
    component: "Select",
    also: &["ComboBox", "ComboBoxFlat"],
    name: "Combo box",
    dsl: "ComboBoxOverview",
    added: "2026-09-10",
    tags: &["new", "dropdown", "autocomplete", "typeahead", "filter", "picker"],
    doc: "# ComboBox\n\nA text field with a list behind it, over a **closed set**. Typing filters the list; it does not author a value. Only items from `labels` are ever committed, so a host reading `changed` never has to decide what a half-typed word meant.\n\nThat one rule settles the awkward cases. Leaving the field with unmatched text in it restores the last committed item rather than keeping the text or clearing the box, because an empty box is not a member of the set either. Return with no match does nothing and leaves the filter editable, which is the state where the person can still fix a typo.\n\n## Keyboard\n\nKey focus lives in the embedded field the whole time; the highlight in the popup is drawn, not focused.\n\n| Key | Closed | Open |\n|---|---|---|\n| a printable key | opens and filters, highlights the top match | filters, highlights the top match |\n| Down / Up | opens, highlights the first / last item | walks the highlight, wrapping |\n| PageDown / PageUp | caret to start / end | moves a page |\n| Return | nothing | commits the highlight and closes |\n| Escape | restores the committed label | closes and restores it |\n| Tab | moves on | commits the highlight, then moves on |\n\n## The popup\n\n`max_visible_items` sets how tall the popup gets before it scrolls; `no_match_text` is what it says when the filter matches nothing.\n\nThe Select overview says when a combo box is the one to reach for, rather than a select, a multi-select or a dropdown.",
    subject: "subject",
    feature: None,
    controls: &[],
    on_actions: Some(combobox_actions),
}];

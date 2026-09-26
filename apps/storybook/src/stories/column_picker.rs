//! The picker story: columns that fill each other, a tree in a popover with
//! boxes that cascade, and two lists with the room to move between them.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ColumnPickerOverview = StoryPage{
        StoryNote{text: "A drop-down works while the answers fit on one screen and the reader already knows which one they want. Past that it stops working, and these are the three shapes that replace it: a path through a hierarchy, a set out of a hierarchy, and what is in against what is out."}

        StoryHeading{text: "Columns, one per level"}
        StoryNote{text: "Choose in a column and the column to its right fills with what is under that choice. A leaf opens nothing, so there is never an empty column hanging off the end for the reader to press at. The mark at the right edge of a row says it has a column behind it."}
        StoryRow{
            subject := mod.widgets.ColumnPicker{
                outline: [
                    "Europe"
                    "  France"
                    "    Brittany"
                    "      Rennes"
                    "      Brest"
                    "    Alsace"
                    "      Colmar"
                    "  Spain"
                    "    Galicia"
                    "      Vigo"
                    "Asia"
                    "  Japan"
                    "    Kansai"
                    "      Kobe"
                    "      Osaka"
                ]
            }
        }
        StoryRow{
            path_note := Label{text: "nothing chosen yet"}
        }

        StoryHeading{text: "Choosing again throws away everything to the right"}
        StoryNote{text: "Walk down to a city, then choose a different country in the second column: the two columns past it go, because they were listing the children of a branch nobody is standing in any more. Keeping them would leave a highlighted row on the screen that the host reading the path would take as an answer."}
        StoryNote{text: "The keyboard walks it the same way. Up and down move the choice inside one column, right steps into the column that choice opened and lands on its first row, and left steps back out. The whole picker is ONE tab stop."}
        StoryRow{
            mod.widgets.ColumnPicker{
                column_width: 120.
                row_budget: 5
                outline: [
                    "Drums"
                    "  Kit"
                    "    Kick"
                    "    Snare"
                    "  Percussion"
                    "    Shaker"
                    "Keys"
                    "  Piano"
                    "    Upright"
                    "  Organ"
                ]
            }
        }

        StoryHeading{text: "A tree in a popover, with boxes that cascade"}
        StoryNote{text: "The face carries the chosen leaves as chips and the panel carries the tree. Ticking a branch reaches every leaf under it, and a branch whose leaves disagree shows a dash rather than a tick — the branch state is worked out from the leaves every time it is drawn, so a parent can never sit there claiming something its children contradict."}
        StoryNote{text: "A press on a chip takes that leaf off. Without it the chips would be decoration and every removal would be a round trip back through the tree."}
        StoryRow{
            tree := mod.widgets.TreeSelect{
                outline: [
                    "Images"
                    "  Full size"
                    "  Thumbnails"
                    "Text"
                    "  Captions"
                    "  Notes"
                    "Audio"
                    "  Stems"
                    "  Mixdown"
                ]
            }
            chips_note := Label{text: "nothing ticked yet"}
        }

        StoryHeading{text: "Two lists, and the room between them"}
        StoryNote{text: "One list is what is out and the other is what is in, both readable at once. Click rows to light them and use the single arrows, or double-click a row to send it straight across. The count over each list says how many are on that side, and how many of them a search is leaving showing."}
        StoryNote{text: "The double arrows move everything the search is SHOWING, not the whole side. A move-all that reached past the search would take rows the reader cannot see and did not ask for, which is the one way a control like this can silently lose somebody's work."}
        StoryRow{
            width: Fill
            mover := mod.widgets.Transfer{
                width: Fill
                items: [
                    "Kick"
                    "Snare"
                    "Closed hat"
                    "Open hat"
                    "Clap"
                    "Rim"
                    "Crash"
                    "Ride"
                    "Low tom"
                    "High tom"
                    "Bass"
                    "Pad"
                ]
            }
        }
        StoryRow{
            moved_note := Label{text: "nothing moved yet"}
        }

        StoryHeading{text: "Without the searches"}
        StoryNote{text: "Turn the searches off for a short set, where a box to narrow twelve rows is chrome that earns nothing."}
        StoryRow{
            width: Fill
            mod.widgets.Transfer{
                width: Fill
                searchable: false
                list_height: 110.
                left_title: "Off"
                right_title: "On"
                items: [
                    "Reverb"
                    "Delay"
                    "Chorus"
                    "Drive"
                ]
            }
        }
    }
}

fn picker_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let picker = root.column_picker(cx, ids!(subject));
    if picker.chosen(actions).is_some() {
        let path = picker.path_labels();
        let text = if path.is_empty() {
            "nothing chosen".to_string()
        } else {
            path.join(" / ")
        };
        root.label(cx, ids!(path_note)).set_text(cx, &text);
    }

    let tree = root.tree_select(cx, ids!(tree));
    if tree.changed(actions) {
        let names = tree.chosen_labels();
        let text = if names.is_empty() {
            "nothing ticked".to_string()
        } else {
            format!("{} ticked: {}", names.len(), names.join(", "))
        };
        root.label(cx, ids!(chips_note)).set_text(cx, &text);
    }

    let mover = root.transfer(cx, ids!(mover));
    if let Some((count, to_right)) = mover.moved(actions) {
        let names = mover.chosen_labels();
        let way = if to_right { "moved in" } else { "moved out" };
        let text = if names.is_empty() {
            format!("{count} {way}; nothing chosen")
        } else {
            format!("{count} {way}; {} chosen: {}", names.len(), names.join(", "))
        };
        root.label(cx, ids!(moved_note)).set_text(cx, &text);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "selection/column-picker/overview",
    category: "Selection",
    component: "ColumnPicker",
    also: &["TreeSelect", "Transfer"],
    name: "Overview",
    dsl: "ColumnPickerOverview",
    added: "2026-09-10",
    tags: &[
        "new",
        "controls",
        "hierarchy",
        "columns",
        "cascade",
        "chips",
        "two lists",
        "move across",
        "search",
    ],
    doc: "# ColumnPicker, TreeSelect and Transfer

Three ways to choose out of a set that will not fit in one list.

## ColumnPicker

Columns side by side, one per level. Choosing in a column fills the column to its right; **choosing again at any level throws away everything to the right of it.** That is the whole rule, and it is not tidiness — those columns listed the children of a node that is no longer chosen, so keeping them would leave a highlighted row on the screen that a host reading `path_ids` would take as an answer.

A leaf opens nothing, so there is never an empty column at the end. The mark at a row's trailing edge says the row has a column behind it, and it is an angle quote rather than an arrow because the default face carries one and does not carry the other.

The keyboard walks it as a grid: up and down move the choice inside one column, right steps into the column that choice opened and lands on its first row, left steps back out, Home and End reach the ends of a column. The whole picker is ONE tab stop, so Tab leaves it rather than walking every row on the way past.

## TreeSelect

A tree in a popover with boxes that cascade, the chosen leaves standing as chips on the face.

**A branch's box is derived, never stored.** Ticking a branch writes to the leaves under it and nothing else; the branch reads its own state back out of them every time it draws. That is the whole of why a parent can never disagree with its children, and why a mixed box exists at all. A press on a mixed box resolves to on, the way a select-all box does.

The chips are the removal path: a press on one takes that leaf off. Without that they would be decoration and every removal would be a round trip back through the tree. Past `chip_limit` the face counts the rest rather than growing.

While the panel is open it holds the pointer for the whole widget tree, the pairing every popup here uses: the panel floats over widgets that were already walked this dispatch, so marking a press handled by the time it reaches this widget is too late.

## Transfer

Two lists with move-across controls, a search over each side and a count on each side.

**The double arrows move what the search is SHOWING, not the whole side.** A move-all that reached past the search would take rows the reader cannot see and did not ask for, and that is the one way a control like this can silently lose somebody's work. It is the rule most worth testing and it has its own test.

Rows are held in ITEM order on both sides, never in the order they were moved: two readers who moved the same set in a different sequence must end up looking at the same list. A single click lights a row and leaves the rest alone, because this is a set being built; a double click sends that row straight across, because the hand is already on it.

## What all three deliberately do NOT do

No scrolling of their own — every row draws every pass and rows past the stated budget are not drawn, which is honest for the few dozen rows these are for. No data source and no lazy children: a picker that fetched the next column would need a loading state, a failure state and a cancel, and none of those are decisions a widget gets to make. No sorting and no grouping — the order given is the order shown.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Column width", target: "subject", kind: ControlKind::Number { prop: "column_width", min: 80., max: 300., step: 5., default: 150. } },
        Control { label: "Row height", target: "subject", kind: ControlKind::Number { prop: "row_height", min: 16., max: 40., step: 1., default: 24. } },
        Control { label: "Rows shown", target: "subject", kind: ControlKind::Number { prop: "row_budget", min: 3., max: 20., step: 1., default: 8. } },
        Control { label: "Column rule", target: "subject", kind: ControlKind::Number { prop: "rule_size", min: 0., max: 3., step: 0.5, default: 1. } },
        Control { label: "Label inset", target: "subject", kind: ControlKind::Number { prop: "pad_x", min: 0., max: 20., step: 1., default: 8. } },
    ],
    on_actions: Some(picker_actions),
}];

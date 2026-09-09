//! The field well story: the box an input sits in, and the state it can
//! finally show.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.FieldWellOverview = StoryPage{
        StoryNote{text: "Eight places in this repository put a text input inside a wrapper so the wrapper can draw the box, and every one of them blanks the input's own chrome first. In four, the wrapper is a plain view with no hover, focus or disabled state — so the field looks the same whether or not you are typing in it."}

        StoryHeading{text: "Three wells, one keyboard"}
        StoryNote{text: "Click into one. Only that one lights, and it keeps saying so while you type. Click its padding rather than its text and it still takes the keyboard, because the whole box is what a person aims at."}
        StoryRow{
            View{
                width: 220. height: Fit
                first := FieldWell{
                    input: WellInput{empty_text: "First"}
                }
            }
            View{
                width: 220. height: Fit
                second := FieldWell{
                    input: WellInput{empty_text: "Second"}
                }
            }
            View{
                width: 220. height: Fit
                third := FieldWell{
                    input: WellInput{empty_text: "Third"}
                }
            }
        }
        StoryRow{
            which := Label{text: "nothing has the keyboard"}
        }

        StoryHeading{text: "What sits beside the input"}
        StoryNote{text: "A leading slot and a trailing slot, for the magnifier, the unit, the clear mark. They are part of the box, so a press on either still hands the keyboard to the input."}
        StoryRow{
            View{
                width: 300. height: Fit
                searchy := FieldWell{
                    leading: Icon{
                        icon_walk: Walk{width: 13. height: 13.}
                        draw_icon +: {color: theme.color_text_meta}
                    }
                    input: WellInput{empty_text: "Search"}
                    trailing: Label{text: "\u{2318}K" draw_text +: {color: theme.color_text_meta}}
                }
            }
            View{
                width: 220. height: Fit
                measured := FieldWell{
                    input: WellInput{empty_text: "120"}
                    trailing: Label{text: "px" draw_text +: {color: theme.color_text_meta}}
                }
            }
        }

        StoryHeading{text: "Disabled reaches all three"}
        StoryNote{text: "Set it in one place, not three. The well dims its box and hands the same flag to each slot, so the input goes quiet with it. A slot that has no disabled state of its own — a plain Label, as the currency mark here is — cannot honour it, which is worth knowing before you put one in a field that can be switched off."}
        StoryRow{
            View{
                width: 300. height: Fit
                off := FieldWell{
                    disabled: true
                    leading: Label{text: "$" draw_text +: {color: theme.color_text_meta}}
                    input: WellInput{text: "1000"}
                    trailing: Label{text: "USD" draw_text +: {color: theme.color_text_meta}}
                }
            }
        }
    }
}

fn field_actions(cx: &mut Cx, root: &WidgetRef, _actions: &Actions) {
    let named = [
        ("first", ids!(first)),
        ("second", ids!(second)),
        ("third", ids!(third)),
        ("search", ids!(searchy)),
        ("measured", ids!(measured)),
    ];
    let holding = named
        .iter()
        .find(|(_, id)| root.field_well(cx, *id).focused(cx))
        .map(|(name, _)| *name);
    let text = match holding {
        Some(name) => format!("{name} has the keyboard"),
        None => "nothing has the keyboard".to_string(),
    };
    let label = root.label(cx, ids!(which));
    if label.text() != text {
        label.set_text(cx, &text);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/field-well/overview",
    category: "Inputs",
    component: "FieldWell",
    also: &["WellInput"],
    name: "Overview",
    dsl: "FieldWellOverview",
    added: "2026-09-08",
    tags: &["new"],
    doc: "# FieldWell

The box a field's input sits in, which knows what the input is doing.

Eight places in this repository put a text input inside a wrapper so the wrapper can draw the box — a search well with a magnifier, a page number between two arrows, a labelled row with a unit after it. Every one of them blanks the input's own chrome first: a dozen lines of `color_hover`, `color_focus`, `border_color_down` and the rest set to one flat colour, so two boxes are not drawn inside each other.

**And then four of them can never say they have the keyboard.** Once the chrome is blanked, the state lives in a widget that is no longer drawing, while the wrapper that *is* drawing has no hover, focus or disabled instance to set. The field looks identical whether or not you are typing in it.

So the well draws the chrome **and** carries the slot's state into it. It asks the input each pass whether it holds the key focus, rather than tracking it — focus can leave for reasons the well never hears about, and a well that only listened would keep claiming a keyboard it no longer has. A press anywhere on the well hands the focus to the input, because the whole box is what a person aims at. And one `disabled` reaches all three slots — reaches, not repaints: a slot honours it if it has a disabled state, and a plain `Label` has none.

**What it is not.** It carries no label and no message. The label half of this card measured out at about seventy-five hand-rolled instances that would each re-override a shared shell — the same finding that made a shared splitter preset worthless — and the validation half has no caller at all in this repository.",
    subject: "first",
    feature: None,
    controls: &[],
    on_actions: Some(field_actions),
}];

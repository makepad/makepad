//! The tag field story: chips for what has been chosen, a box for the next
//! one, and the two rules the field enforces out loud.
use crate::makepad_widgets::tag_field::TagFieldWidgetRefExt;
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.TagFieldOverview = StoryPage{
        StoryNote{text: "A box holding what has already been chosen, and a place to type the next one. Return commits what is typed, and so does a comma. Backspace on the empty box takes the last tag back, and pasting a list splits it on the same characters."}

        StoryHeading{text: "A field of tags"}
        StoryNote{text: "Type a word and press Return, or type one and then a comma. Press a chip's cross to take it away, or Backspace with the box empty. Every control on the right writes into this one."}
        StoryRow{
            width: Fill
            subject := TagField{
                width: Fill
                tags: ["urgent", "invoice", "follow up"]
            }
        }
        StoryRow{
            tag_count := Label{text: "3 tags: urgent, invoice, follow up"}
        }
        StoryRow{
            tag_report := Label{text: "nothing has happened yet"}
        }

        StoryHeading{text: "What ends a tag"}
        StoryNote{text: "`split_chars` is the set of characters that finish a tag as they arrive — typed or pasted, the same rule for both, so a list off the clipboard lands as a list rather than as one very long tag. This field ends a tag on a comma or a semicolon; paste `red;green;blue` into it."}
        StoryRow{
            width: Fill
            commit := TagField{
                width: Fill
                split_chars: ",;"
            }
        }

        StoryHeading{text: "A refusal is spoken"}
        StoryNote{text: "Offering a tag the field already holds does not quietly drop it: the field names the tag it already has, on a line under the box, and the border takes the warning colour until the next keystroke. The same for a field that is full. Try `two`, then a third tag, then a fourth."}
        StoryRow{
            width: Fill
            capped := TagField{
                width: Fill
                max: 3
                tags: ["one", "two"]
            }
        }
        StoryRow{
            capped_report := Label{text: "two of three"}
        }

        StoryHeading{text: "Case"}
        StoryNote{text: "Two tags differing only in case are one tag by default, which is what a field of words wants. `match_case: true` for a field of things where the case carries meaning — an identifier, a key, a code."}
        StoryRow{
            width: Fill
            cased := TagField{
                width: Fill
                match_case: true
                tags: ["Draft"]
            }
        }

        StoryHeading{text: "The rows wrap"}
        StoryNote{text: "A field of many tags is several lines tall rather than one line with the rest off the side. The box for typing takes what is left of the row it is on, and drops onto a row of its own once the tags have left it too little to type in."}
        StoryRow{
            width: Fill
            wrapped := TagField{
                width: Fill
                tags: ["morning", "afternoon", "evening", "weekend", "public holiday", "the week after next", "whenever there is room"]
            }
        }

        StoryHeading{text: "Without the clear control"}
        StoryNote{text: "`show_clear: false` for a field of one or two tags, where every chip already carries a cross and a second way to empty the field is one control too many."}
        StoryRow{
            width: Fill
            TagField{
                width: Fill
                show_clear: false
                tags: ["owner"]
            }
        }

        StoryHeading{text: "Disabled"}
        StoryRow{
            width: Fill
            TagField{
                width: Fill
                disabled: true
                tags: ["frozen", "still"]
            }
        }
    }
}

fn tag_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let subject = root.tag_field(cx, ids!(subject));
    if let Some(tags) = subject.changed(actions) {
        let text = match tags.len() {
            0 => "no tags".to_string(),
            1 => format!("1 tag: {}", tags.join(", ")),
            n => format!("{n} tags: {}", tags.join(", ")),
        };
        root.label(cx, ids!(tag_count)).set_text(cx, &text);
    }
    // The four reports in the order they are worth reading: what was turned
    // away first, since that is the one the person is waiting to hear about.
    let report = if let Some(note) = subject.refused(actions) {
        Some(format!("refused: {note}"))
    } else if let Some(tag) = subject.added(actions) {
        Some(format!("added: {tag}"))
    } else if let Some(tag) = subject.removed(actions) {
        Some(format!("removed: {tag}"))
    } else if subject.cleared(actions) {
        Some("cleared".to_string())
    } else {
        None
    };
    if let Some(report) = report {
        root.label(cx, ids!(tag_report)).set_text(cx, &report);
    }

    let capped = root.tag_field(cx, ids!(capped));
    if let Some(tags) = capped.changed(actions) {
        root.label(cx, ids!(capped_report))
            .set_text(cx, &format!("{} of three", tags.len()));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/tagfield/overview",
    category: "Inputs",
    component: "TagField",
    also: &[],
    name: "Overview",
    dsl: "TagFieldOverview",
    added: "2026-09-10",
    tags: &["new", "controls", "tags", "chips", "multi value", "keywords", "recipients", "labels"],
    doc: "# TagField\n\nA text field whose answer is a *list*. The whole of the design sits at one moment: when a word stops being typing and becomes an entry.\n\n## What commits a tag\n\n| Gesture | Result |\n|---|---|\n| Return | commits everything in the box |\n| one of `split_chars` | commits what came before it and leaves the rest being typed |\n| paste | splits on the same characters, so a pasted list becomes a list of tags |\n| Backspace on the empty box | takes the last tag back |\n| a chip's cross | takes that tag away |\n\nBackspace is the gesture people try first, and it is the reason a tag field feels like a field rather than a list with a box on the end. It has to be caught before the box sees the key: an empty text box handles Backspace itself — as nothing at all — so it never reports one as unhandled and there is no later moment to catch it at.\n\n## A refusal is spoken\n\nA tag the field already holds is not silently dropped. The field says which tag it already has, on a line under the box, and the border takes the warning colour until the next keystroke; `max` refuses the same way when the field is full. Quietly ignoring what someone typed leaves them believing it went in, and they find out later from whatever was supposed to have been filtered, addressed or labelled.\n\nThe refused text leaves the box rather than sitting in it. The field has already said what is wrong with it, and someone typing a list wants the box ready for the next one instead of holding a word they must now delete by hand.\n\nTwo tags differing only in case are one tag unless `match_case` is set — right for a field of words, wrong for one holding identifiers or codes.\n\n## What it deliberately does not do\n\nIt does not suggest, complete, or know what tags exist anywhere: a field that offers a list to choose from is a combo box over data the host owns, with a popup and a different keyboard, and it is a different widget. It does not judge the *shape* of a tag either — that an address is an address, that a word is in some vocabulary — because only the host knows what a tag means here. It keeps its own two rules and reports everything else.\n\n## Reading it\n\n`changed` carries the whole list after every move and is the one to read for the answer. `added`, `removed`, `refused` and `cleared` say what happened, for a host that wants to undo it or say something of its own about it.\n\nThe tags are drawn from the `chip` template on the instance, so a caller keeps its own look and still gets the cross, its separate hit area, and the removal report.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Ends a tag", target: "subject", kind: ControlKind::Text { prop: "split_chars", default: "," } },
        Control { label: "Most tags", target: "subject", kind: ControlKind::Number { prop: "max", min: 0., max: 12., step: 1., default: 0. } },
        Control { label: "Case matters", target: "subject", kind: ControlKind::Bool { prop: "match_case", default: false } },
        Control { label: "Clear control", target: "subject", kind: ControlKind::Bool { prop: "show_clear", default: true } },
        Control { label: "Corner radius", target: "subject", kind: ControlKind::Number { prop: "draw_bg.border_radius", min: 0., max: 16., step: 0.5, default: 2.5 } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(tag_actions),
}];

//! The rating story: a row of marks that shows one number and takes one.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.RatingOverview = StoryPage{
        StoryNote{text: "A row of marks holding one number. The pointer previews what a press would set, in a quieter ink, and the row comes back when it leaves. Pressing the mark the value already stands on clears the row."}

        StoryHeading{text: "Setting one"}
        StoryRow{
            subject := Rating{
                count: 5
                default_value: 3.0
                show_value: true
            }
        }
        StoryRow{
            subject_note := Label{text: "3 \u{2014} good"}
        }
        StoryNote{text: "The marks are never named by the control: \"poor\", \"good\" and the rest are the page's words, and this page follows the pointer with them. The count beside the marks goes on showing the value that is really held while the preview is up, so nothing is hidden by the lie the marks are telling for a moment."}

        StoryHeading{text: "Halves"}
        StoryNote{text: "`precision: 0.5` splits every mark down the middle, so the left of a mark is a half and the right of it a whole. The part mark is the whole glyph cut to the fraction, not a second glyph, so it works with whatever mark the app hands it."}
        StoryRow{
            halves := Rating{
                count: 5
                default_value: 3.5
                precision: 0.5
            }
            halves_note := Label{text: "3.5 of 5"}
        }

        StoryHeading{text: "A number it was given"}
        StoryNote{text: "`read_only: true` shows any fraction and takes no hits at all, so the table row or the link underneath still answers. An average is a fraction: a row that rounded 4.3 onto its own grid would be making a different claim from the one the number makes."}
        StoryRow{
            Rating{
                count: 5
                default_value: 4.3
                read_only: true
                show_value: true
                text: "1204 ratings"
            }
        }

        StoryHeading{text: "Marks of your own"}
        StoryNote{text: "`glyph` and `glyph_empty` are text, in whatever face the app has, and `mark_size` sizes them. The defaults are a filled and a hollow circle because the faces shipped here carry those."}
        StoryRow{
            Rating{
                count: 5
                default_value: 4.0
                glyph: "\u{25a0}"
                glyph_empty: "\u{25a1}"
            }
            Rating{
                count: 3
                default_value: 2.0
                glyph: "\u{25c6}"
                glyph_empty: "\u{25c7}"
                mark_size: 26.
                gap: 6.
            }
        }

        StoryHeading{text: "A row that must be answered"}
        StoryNote{text: "`clearable: false` takes away the press-again-to-clear gesture, for a form where an answer is required. Home then goes to one mark rather than to none."}
        StoryRow{
            Rating{
                count: 5
                default_value: 2.0
                clearable: false
                show_value: true
            }
        }

        StoryHeading{text: "Ten of them"}
        StoryNote{text: "Nothing stops a longer row, but past about ten marks nobody counts them and a number would say it faster."}
        StoryRow{
            Rating{
                count: 10
                default_value: 7.0
                mark_size: 14.
                gap: 2.
                show_value: true
            }
        }

        StoryHeading{text: "Disabled"}
        StoryRow{
            Rating{
                count: 5
                default_value: 3.0
                show_value: true
                animator +: {disabled: {default: @on}}
            }
        }
    }
}

/// What the page calls each mark. The control never names them, because the
/// words belong to whatever is being rated.
fn verdict(value: f64) -> &'static str {
    match value.ceil() as i32 {
        0 => "not rated",
        1 => "poor",
        2 => "fair",
        3 => "good",
        4 => "very good",
        _ => "excellent",
    }
}

fn rating_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let subject = root.rating(cx, ids!(subject));
    if let Some(value) = subject.previewed(actions) {
        root.label(cx, ids!(subject_note))
            .set_text(cx, &format!("a press here sets {value:.0} \u{2014} {}", verdict(value)));
    }
    if let Some(value) = subject.changed(actions) {
        root.label(cx, ids!(subject_note))
            .set_text(cx, &format!("{value:.0} \u{2014} {}", verdict(value)));
    }
    // The pointer left: back to the value that is actually held.
    if subject.preview_ended(actions) {
        let value = subject.value();
        root.label(cx, ids!(subject_note))
            .set_text(cx, &format!("{value:.0} \u{2014} {}", verdict(value)));
    }
    let halves = root.rating(cx, ids!(halves));
    if let Some(value) = halves.changed(actions) {
        root.label(cx, ids!(halves_note))
            .set_text(cx, &format!("{value:.1} of 5"));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/rating/overview",
    category: "Inputs",
    component: "Rating",
    also: &[],
    name: "Overview",
    dsl: "RatingOverview",
    added: "2026-09-10",
    tags: &["new", "controls", "rating", "stars", "score", "marks", "review", "vote"],
    doc: "# Rating\n\nA row of marks holding one number between zero and `count`. It is the smallest control that carries a judgement, and everything in it serves making that number easy to see from across the room and easy to change without aiming.\n\n## What the row shows while the pointer is on it\n\nThe value a press would set, in a quieter ink \u{2014} not the value it holds. The other school previews only the marks past the current value; this one shows the whole row as it would be, because that answers the question actually being asked, which is *what do I get if I press here*. The quieter ink is what says the row is lying for a moment, and the count beside the marks goes on showing the value that is really held, so nothing is lost while the preview is up.\n\n## Clearing\n\nPressing the mark the value already stands on clears the row to zero. Without it there is no way back to \"not rated\" once a mark has been pressed, and a rating that cannot be taken back is a trap \u{2014} the finger slips onto four marks and four marks is what the record says forever. `clearable: false` turns it off for a form that requires an answer, and Home then goes to one mark rather than to none.\n\n## The gestures\n\n| Gesture | Result |\n|---|---|\n| hover | previews the value a press would set |\n| press | sets it, or clears the row if it was already there |\n| drag along the row | follows the finger, for a touch screen with no pointer to hover with |\n\nA drag is only recognised once the finger has travelled half a mark, so a shaky hand does not undo its own clear.\n\n## Precision\n\n`precision: 1` takes whole marks, `0.5` takes halves: the left of a mark is a half and the right of it a whole. A part mark is the whole glyph cut down to the fraction rather than a second glyph, so it works with whatever mark the app hands it \u{2014} and so a `read_only` row can show a 4.3 as honestly as a 4.5.\n\n## Keyboard\n\nThe arrows move by one step of `precision`, snapping an odd fraction onto the grid on the way. Home is the bottom of the row and End the top.\n\n## The marks\n\n`glyph` and `glyph_empty` are text, drawn in whatever face the app has, and `mark_size` sizes them. They default to a filled and a hollow circle because those are carried by the faces shipped here; a star or a heart is one property away in a font that has one.\n\n## Reading it\n\n`changed` reports the value a gesture settled on. `previewed` reports the value the pointer is over and `preview_ended` says it left, for a page that wants to name its marks in words beside the row.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Marks", target: "subject", kind: ControlKind::Number { prop: "count", min: 1., max: 10., step: 1., default: 5. } },
        Control { label: "Precision", target: "subject", kind: ControlKind::Number { prop: "precision", min: 0.5, max: 1., step: 0.5, default: 1. } },
        Control { label: "Mark size", target: "subject", kind: ControlKind::Number { prop: "mark_size", min: 10., max: 40., step: 1., default: 18. } },
        Control { label: "Gap", target: "subject", kind: ControlKind::Number { prop: "gap", min: 0., max: 16., step: 0.5, default: 3. } },
        Control { label: "Count label", target: "subject", kind: ControlKind::Bool { prop: "show_value", default: true } },
        Control { label: "Clearable", target: "subject", kind: ControlKind::Bool { prop: "clearable", default: true } },
        Control { label: "Read only", target: "subject", kind: ControlKind::Bool { prop: "read_only", default: false } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(rating_actions),
}];

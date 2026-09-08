//! The tooltip stories: where a tip hangs, what it may carry, and how it
//! is reached without a pointer.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.TipOverview = StoryPage{
        StoryNote{text: "A tooltip is a wrapper, not a call site: put a Tip around a control and the window's one TipLayer does the dwell, the placement and the chrome. Hover a button below, or Tab to one."}

        StoryHeading{text: "Where it hangs"}
        StoryNote{text: "Twelve places, through the same helper every anchored popup uses: the wanted side, flipped when there is no room, shifted to stay in the window."}
        StoryRow{
            tip_bottom := Tip{text: "Below, centred" place: Bottom Button{text: "Bottom"}}
            tip_top := Tip{text: "Above, centred" place: Top Button{text: "Top"}}
            tip_left := Tip{text: "To the left" place: Left Button{text: "Left"}}
            tip_right := Tip{text: "To the right" place: Right Button{text: "Right"}}
        }
        StoryRow{
            Tip{text: "Below, left edges" place: BottomStart Button{text: "BottomStart"}}
            Tip{text: "Below, right edges" place: BottomEnd Button{text: "BottomEnd"}}
            Tip{text: "Above, left edges" place: TopStart Button{text: "TopStart"}}
            Tip{text: "Above, right edges" place: TopEnd Button{text: "TopEnd"}}
        }

        StoryNote{text: "A side can align too: Start puts the tip's top edge level with the control's, End its bottom. The controls here are tall on purpose, since against a control the tip's own height the two look the same."}
        StoryRow{
            Tip{text: "Left, top edges" place: LeftStart Button{height: 70. text: "LeftStart"}}
            Tip{text: "Left, bottom edges" place: LeftEnd Button{height: 70. text: "LeftEnd"}}
            Tip{text: "Right, top edges" place: RightStart Button{height: 70. text: "RightStart"}}
            Tip{text: "Right, bottom edges" place: RightEnd Button{height: 70. text: "RightEnd"}}
        }

        StoryHeading{text: "With a pointer"}
        StoryNote{text: "The arrow sits on the bubble's anchor-facing edge, at the point the placement worked out, so it keeps aiming at the control after a flip."}
        StoryRow{
            Tip{text: "Aimed at this one" arrow: true Button{text: "Arrow"}}
            Tip{text: "Aimed from above" arrow: true place: Top Button{text: "Arrow, above"}}
        }
        StoryNote{text: "A tip beside a control turns its pointer a quarter turn, so it still aims at the thing it belongs to."}
        StoryRow{
            Tip{text: "Short" arrow: true place: BottomStart Button{width: 260. text: "Wider than its own tip"}}
        }
        StoryNote{text: "When the control is wider than the tip, the point would sit on a rounded corner, so it stops at the flat part of the edge instead of straddling it."}
        StoryRow{
            Tip{text: "Aimed from the left" arrow: true place: Left Button{text: "Arrow, left"}}
            Tip{text: "Aimed from the right" arrow: true place: Right Button{text: "Arrow, right"}}
        }

        StoryHeading{text: "Room for a sentence"}
        StoryNote{text: "A wrap width turns a phrase into a paragraph, for the tip that has to explain rather than name."}
        StoryRow{
            TipRich{
                text: "Rendering writes every frame to disk as a PNG. It is slower than playback and needs room for the whole sequence."
                Button{text: "Render"}
            }
        }

        StoryHeading{text: "Roles"}
        StoryNote{text: "A tip can carry a role when it is saying something is wrong rather than naming a control."}
        StoryRow{
            TipError{text: "This file no longer exists" Button{text: "Broken link"}}
            TipWarning{text: "Unsaved changes will be lost" Button{text: "Discard"}}
        }

        StoryHeading{text: "Timing"}
        StoryNote{text: "The default dwell is half a second, and moving along a row of tipped controls follows instantly. A tip can ask for its own."}
        StoryRow{
            Tip{text: "No dwell at all" delay_secs: 0. Button{text: "Instant"}}
            Tip{text: "A long wait" delay_secs: 1.5 Button{text: "Slow"}}
        }

        StoryHeading{text: "Without a pointer"}
        StoryNote{text: "Tab into the field below: its tip appears on the focus, and Escape takes it down."}
        StoryRow{
            Tip{text: "The name people will see" TextInput{width: 200. empty_text: "Display name"}}
        }

        tips := TipLayer{}
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "feedback/tip/overview",
    category: "Feedback",
    component: "Tip",
    also: &[],
    name: "Overview",
    dsl: "TipOverview",
    added: "2026-09-05",
    tags: &["new"],
    doc: "# Tooltip\n\nA tooltip is declared by wrapping, not by writing code at the call site: put a `Tip` around a control, give it text, and the window's one `TipLayer` owns the rest.\n\nThe wrapper reports the control's final drawn rect and how the tip should look; the layer owns the dwell, the grace window that makes a row of tipped controls follow instantly, the placement and the chrome. `place` picks one of twelve positions and the shared placement helper flips and shifts it to stay in the window. `arrow` puts a pointer on the bubble's anchor-facing edge, aimed at the control even after a flip. `wrap_width` turns a phrase into a paragraph. `delay_secs` overrides the dwell for one tip, and `intent` gives it a role colour.\n\nA tip also appears when its control takes the keyboard focus, and Escape takes it down: a tip only the pointer can reach is a tip half the people using the app never see.",
    subject: "tip_left",
    feature: None,
    controls: &[],
    on_actions: None,
}];

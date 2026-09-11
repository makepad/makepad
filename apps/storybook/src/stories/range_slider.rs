//! The range slider story: two handles on one track, and the span between
//! them.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.RangeSliderOverview = StoryPage{
        StoryNote{text: "Two handles and the span between them. Drag either end to move it, drag the band to move both at once, or press bare track to send the nearer handle there. The two never cross."}

        StoryHeading{text: "A range"}
        StoryRow{
            width: Fill
            subject := RangeSlider{
                width: Fill
                text: "Price"
                min: 0.0
                max: 500.0
                step: 5.0
                default_start: 100.0
                default_end: 350.0
                unit: " EUR"
                precision: 0
            }
        }
        StoryRow{
            price_note := Label{text: "100 – 350"}
        }

        StoryHeading{text: "Moving the whole span"}
        StoryNote{text: "The band between the handles is a grab target of its own: it lights under the pointer and drags both ends together, keeping the width. At either stop the span stops rather than being squashed against it — a two-hour window stays two hours long when it is pushed to the end of the day."}
        StoryRow{
            width: Fill
            window := RangeSlider{
                width: Fill
                text: "Window"
                min: 0.0
                max: 24.0
                step: 0.5
                default_start: 9.0
                default_end: 17.0
                unit: "h"
                precision: 1
            }
        }

        StoryHeading{text: "Room the two must leave each other"}
        StoryNote{text: "`min_span` is the least the range may be. Push either handle towards the other and it stops with that much still between them; push the pair into a stop and the other end gives way instead."}
        StoryRow{
            width: Fill
            floor := RangeSlider{
                width: Fill
                text: "At least a fifth of the track"
                min: 0.0
                max: 100.0
                step: 1.0
                min_span: 20.0
                default_start: 30.0
                default_end: 70.0
                precision: 0
            }
        }

        StoryHeading{text: "Without the readout"}
        StoryNote{text: "`show_readout: false` for a control whose host already says what the span is."}
        StoryRow{
            width: Fill
            bare := RangeSlider{
                width: Fill
                text: "Gain"
                min: -24.0
                max: 24.0
                step: 0.5
                default_start: -6.0
                default_end: 6.0
                show_readout: false
            }
        }
        StoryRow{
            bare_note := Label{text: "-6.0 – 6.0 dB"}
        }

        StoryHeading{text: "Disabled"}
        StoryRow{
            width: Fill
            RangeSlider{
                width: Fill
                text: "Off"
                default_start: 0.25
                default_end: 0.75
                animator +: {disabled: {default: @on}}
            }
        }

        StoryHeading{text: "The ladder"}
        StoryRow{
            width: Fill
            RangeSliderFlat{
                width: Fill
                text: "RangeSliderFlat"
                default_start: 0.2
                default_end: 0.6
            }
        }
    }
}

fn range_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let price = root.range_slider(cx, ids!(subject));
    if let Some((start, end)) = price.slided(actions) {
        root.label(cx, ids!(price_note))
            .set_text(cx, &format!("{start:.0} – {end:.0}"));
    }
    let bare = root.range_slider(cx, ids!(bare));
    if let Some((start, end)) = bare.slided(actions) {
        root.label(cx, ids!(bare_note))
            .set_text(cx, &format!("{start:.1} – {end:.1} dB"));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/rangeslider/overview",
    category: "Inputs",
    component: "RangeSlider",
    also: &["RangeSliderFlat"],
    name: "Overview",
    dsl: "RangeSliderOverview",
    added: "2026-09-10",
    tags: &["new", "controls", "range", "span", "min", "max", "two handles", "slider"],
    doc: "# RangeSlider\n\nOne track, two handles, and the span between them. `start` and `end` are held in order at all times, so a host reading them never has to sort them first.\n\n## What a handle does when it reaches the other one\n\nIt stops. The other school swaps the handles over so the drag can carry on past, and it is wrong here: the finger is on one handle, and a control that hands the finger a *different* handle mid-drag turns a small overshoot into a silent role change — the person is now dragging the end while still thinking they hold the start. Stopping is legible, and `min_span` says how much room the two must leave each other.\n\n## The gestures\n\n| Gesture | Result |\n|---|---|\n| drag a handle | moves that end |\n| drag the band between them | moves both, keeping the width |\n| press bare track | the nearer handle jumps there, and the drag carries on from it |\n| double tap | both ends back to `default_start` / `default_end` |\n\nThe press-on-track rule is what makes reaching a distant value one gesture instead of two, and the band is what makes this widget worth having over a pair of sliders: the span is usually the thing being chosen, and it is the thing a pair cannot move.\n\n## Keyboard\n\nThe arrows move the handle touched last by `step`, or by a hundredth of the range when `step` is 0. Shift moves the span. Home and End send that handle to its own limit — for the end handle that is the start handle, not the track's stop. `[` and `]` choose which handle the keyboard has, since Tab belongs to focus traversal.\n\n## Reading it\n\n`slided` reports both ends on every frame of a drag; `end_slide` reports the ends the gesture settled on. Prefer `end_slide` for anything expensive — a query, a re-render of a list — and `slided` for a readout that should follow the finger.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Label", target: "subject", kind: ControlKind::Text { prop: "text", default: "Price" } },
        Control { label: "Minimum", target: "subject", kind: ControlKind::Number { prop: "min", min: -500., max: 0., step: 10., default: 0. } },
        Control { label: "Maximum", target: "subject", kind: ControlKind::Number { prop: "max", min: 10., max: 1000., step: 10., default: 500. } },
        Control { label: "Step", target: "subject", kind: ControlKind::Number { prop: "step", min: 0., max: 50., step: 1., default: 5. } },
        Control { label: "Least span", target: "subject", kind: ControlKind::Number { prop: "min_span", min: 0., max: 200., step: 5., default: 0. } },
        Control { label: "Handle width", target: "subject", kind: ControlKind::Number { prop: "handle_size", min: 8., max: 40., step: 1., default: 14. } },
        Control { label: "Readout", target: "subject", kind: ControlKind::Bool { prop: "show_readout", default: true } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(range_actions),
}];

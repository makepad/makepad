//! The fader story: a desk cap on a slotted track, lying down and stood on
//! end.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.SliderFaderOverview = StoryPage{
        StoryNote{text: "A mixing-desk fader: a cap running a slotted track. It is the same Slider as the rest of the family, drawn along whichever axis it is dragged on — SliderFader lies down, SliderFaderY stands up — so one material, one travel law and one set of properties answer for both."}

        StoryHeading{text: "Lying down"}
        StoryRow{
            width: Fill
            subject := SliderFader{width: Fill default: 0.35}
        }
        StoryRow{
            level_note := Label{text: "0.35"}
        }

        StoryHeading{text: "Stood on end"}
        StoryNote{text: "The same widget with axis: Vertical, which flips the drawing and the drag together. The bottom of the strip is zero, as on a desk. A press on the cap owns the pointer until it is let go, so a fader in a scrolling rack moves its value and never scrolls the rack."}
        StoryRow{
            align: Align{x: 0., y: 0.}
            spacing: theme.space_4
            View{
                width: Fit height: Fit flow: Down spacing: theme.space_1 align: Align{x: 0.5, y: 0.}
                Label{text: "Kick"}
                SliderFaderY{default: 0.8}
            }
            View{
                width: Fit height: Fit flow: Down spacing: theme.space_1 align: Align{x: 0.5, y: 0.}
                Label{text: "Bass"}
                SliderFaderY{default: 0.62}
            }
            View{
                width: Fit height: Fit flow: Down spacing: theme.space_1 align: Align{x: 0.5, y: 0.}
                Label{text: "Keys"}
                SliderFaderY{default: 0.45}
            }
            View{
                width: Fit height: Fit flow: Down spacing: theme.space_1 align: Align{x: 0.5, y: 0.}
                Label{text: "Vox"}
                SliderFaderY{default: 0.7}
            }
        }
        StoryNote{text: "The legend is the sibling Label above each strip, not a property of the fader. Forty points across has no room beside the track for a word, and the material fills the whole quad, so anything drawn inside it would land on the cap."}

        StoryHeading{text: "A centre, not a floor"}
        StoryNote{text: "arc_from_origin grows the bar out of the default instead of out of the stop, which is what a gain fader wants: a cut and a boost of the same size point opposite ways, and unity reads as nothing at all rather than as a half-filled track that looks like a setting. It is off by default, because a level fader fills from the bottom whatever its default is."}
        StoryRow{
            set_cut := Button{text: "Cut"}
            set_unity := Button{text: "Unity"}
            set_boost := Button{text: "Boost"}
            gain_note := Label{text: "both faders at 0.00"}
        }
        StoryRow{
            align: Align{x: 0., y: 0.}
            spacing: theme.space_4
            View{
                width: Fit height: Fit flow: Down spacing: theme.space_1 align: Align{x: 0.5, y: 0.}
                Label{text: "from origin"}
                from_origin := SliderFaderY{min: -1. max: 1. default: 0. arc_from_origin: true}
            }
            View{
                width: Fit height: Fit flow: Down spacing: theme.space_1 align: Align{x: 0.5, y: 0.}
                Label{text: "from stop"}
                from_stop := SliderFaderY{min: -1. max: 1. default: 0.}
            }
        }

        StoryHeading{text: "The cap and the track"}
        StoryNote{text: "cap_size is the cap's length along the track and track_inset holds the track off both ends. The cap's centre is what the value means, so it stops half a cap in from either end — and the drag divides by that same distance, so a finger that has crossed the whole track leaves the cap on the stop."}
        StoryRow{
            width: Fill
            SliderFader{width: Fill cap_size: 34. track_inset: 16. default: 0.5}
        }

        StoryHeading{text: "Disabled"}
        StoryRow{
            width: Fill
            SliderFader{width: Fill default: 0.4 animator +: {disabled: {default: @on}}}
        }
    }
}

fn fader_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let level = root.slider(cx, ids!(subject));
    if let Some(v) = level.slided(actions) {
        root.label(cx, ids!(level_note)).set_text(cx, &format!("{v:.2}"));
    }
    for (id, v) in [
        (ids!(set_cut), -0.6f64),
        (ids!(set_unity), 0.0),
        (ids!(set_boost), 0.6),
    ] {
        if root.button(cx, id).clicked(actions) {
            root.slider(cx, ids!(from_origin)).set_value(cx, v);
            root.slider(cx, ids!(from_stop)).set_value(cx, v);
            root.label(cx, ids!(gain_note))
                .set_text(cx, &format!("both faders at {v:.2}"));
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/slider/fader",
    category: "Inputs",
    component: "Slider",
    also: &["SliderFader", "SliderFaderY"],
    name: "Fader",
    dsl: "SliderFaderOverview",
    added: "2026-09-19",
    tags: &["new", "controls", "fader", "mixer", "channel strip", "vertical", "slider", "gain"],
    doc: "# SliderFader\n\nA desk fader: a cap running a slotted track. `SliderFader` lies down, `SliderFaderY` stands up, and they are one declaration twice — the material reads the widget's own `axis`, so the face cannot end up drawn one way and dragged the other.\n\n## The travel law\n\nThe cap's *centre* is what the value means, so the centre stops half a cap in from either end of the track and never hangs off it: a `cap_size` long cap on a track held `track_inset` off both ends travels `length - 2 x track_inset - cap_size`. The drag divides by that same distance, so the finger and the cap arrive at the stop together — this is why the two numbers belong to the widget and not to the material.\n\n| Property | What it does |\n|---|---|\n| `axis` | `Vertical` stands the fader on end, drawing and drag alike |\n| `cap_size` | the cap's length along the track |\n| `track_inset` | how far the track's ends are held off the control's |\n| `arc_from_origin` | grow the bar out of the default instead of out of the stop |\n| `draw_bg.track_size` | the slot's width across the track |\n| `draw_bg.cap_cross` | the cap's width, as a share of the control |\n\n## No inline label\n\nThere is none, and that is the design. A fader stood on end is forty points across — a channel strip is narrower still — and the material fills the whole quad, so a word or a readout drawn inside it lands on the cap. The legend is a sibling's job: a `Label` over the column and another under it, the way a desk prints them. The readout is sized away with it, though the field still takes key focus, so a value can be typed blind.\n\n## The gestures\n\nA drag moves the value along the fader's own axis and ignores the other. A **double tap** puts it back to `default` — the tap-the-label reset the rest of the family has needs a label, and this one has none. The press owns the pointer until it is released, so a rack of faders inside a drag-scrolling column moves values and never scrolls the column.\n\n## Reading it\n\n`slided` reports every frame of a drag, `end_slide` the value the gesture settled on. Prefer `end_slide` for anything expensive and `slided` for a readout that should follow the cap.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Cap length", target: "subject", kind: ControlKind::Number { prop: "cap_size", min: 8., max: 60., step: 1., default: 18. } },
        Control { label: "Track inset", target: "subject", kind: ControlKind::Number { prop: "track_inset", min: 0., max: 40., step: 1., default: 8. } },
        Control { label: "Slot width", target: "subject", kind: ControlKind::Number { prop: "draw_bg.track_size", min: 2., max: 24., step: 0.5, default: 7. } },
        Control { label: "Cap width", target: "subject", kind: ControlKind::Number { prop: "draw_bg.cap_cross", min: 0.2, max: 1., step: 0.05, default: 0.72 } },
        Control { label: "Grip line", target: "subject", kind: ControlKind::Number { prop: "draw_bg.cap_line", min: 0., max: 4., step: 0.5, default: 2. } },
        Control { label: "From the origin", target: "subject", kind: ControlKind::Bool { prop: "arc_from_origin", default: false } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(fader_actions),
}];

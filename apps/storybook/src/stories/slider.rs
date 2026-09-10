//! The slider stories: every slider rung with its disabled, ranged, precise and stepped forms, ported from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.SliderOverview = StoryPage{
        H4{text: "Slider"}
        Slider{text: "Default"}
        Slider{
            text: "Default, disabled"
            animator +: {
                disabled: {
                    default: @on
                }
            }
        }
        Slider{text: "min/max" min: 0. max: 100.}
        Slider{text: "precision" precision: 20}
        Slider{text: "stepped" step: 0.1}

        Hr{}
        H4{text: "SliderGradientY"}
        SliderGradientY{text: "Default"}
        SliderGradientY{
            text: "disabled"
            animator +: {
                disabled: {
                    default: @on
                }
            }
        }
        SliderGradientY{text: "min/max" min: 0. max: 100.}
        SliderGradientY{text: "precision" precision: 20}
        SliderGradientY{text: "stepped" step: 0.1}

        Hr{}
        H4{text: "SliderGradientX"}
        SliderGradientX{text: "Default"}
        SliderGradientX{
            text: "disabled"
            animator +: {
                disabled: {
                    default: @on
                }
            }
        }
        SliderGradientX{text: "min/max" min: 0. max: 100.}
        SliderGradientX{text: "precision" precision: 20}
        SliderGradientX{text: "stepped" step: 0.1}

        Hr{}
        H4{text: "SliderFlat"}
        SliderFlat{text: "Default"}
        SliderFlat{
            text: "disabled"
            animator +: {
                disabled: {
                    default: @on
                }
            }
        }
        SliderFlat{text: "min/max" min: 0. max: 100.}
        SliderFlat{text: "precision" precision: 20}
        SliderFlat{text: "stepped" step: 0.1}

        Hr{}
        H4{text: "SliderMinimal"}
        SliderMinimal{text: "Default"}
        SliderMinimal{
            text: "Default, disabled"
            animator +: {
                disabled: {
                    default: @on
                }
            }
        }
        SliderMinimal{text: "min/max" min: 0. max: 100.}
        SliderMinimal{text: "precision" precision: 20}
        SliderMinimal{text: "stepped" step: 0.1}

        Hr{}
        H4{text: "SliderMinimalFlat"}
        SliderMinimalFlat{text: "Default"}
        SliderMinimalFlat{
            text: "disabled"
            animator +: {
                disabled: {
                    default: @on
                }
            }
        }
        SliderMinimalFlat{text: "min/max" min: 0. max: 100.}
        SliderMinimalFlat{text: "precision" precision: 20}
        SliderMinimalFlat{text: "stepped" step: 0.1}

        Hr{}
        H4{text: "SliderRound"}
        SliderRound{text: "Default"}
        SliderRound{
            text: "Disabled"
            animator +: {
                disabled: {
                    default: @on
                }
            }
        }
        SliderRound{
            text: "Solid"
            draw_text +: {
                color: #0ff
            }
            draw_bg +: {
                val_color: uniform(#xF08)
                val_color_hover: uniform(#xF4A)
                val_color_focus: uniform(#xC04)
                val_color_drag: uniform(#xF08)

                val_color_2: uniform(#xF08)
                val_color_2_hover: uniform(#xF4A)
                val_color_2_focus: uniform(#xC04)
                val_color_2_drag: uniform(#xF08)

                handle_color: uniform(#xF)
                handle_color_hover: uniform(#xF)
                handle_color_focus: uniform(#xF)
                handle_color_drag: uniform(#xF)
            }
        }
        SliderRound{
            text: "Solid"
            draw_bg +: {
                val_color: uniform(#6)
                val_color_2: uniform(#6)
                handle_color: uniform(#0)
            }
        }
        SliderRound{text: "min/max" min: 0. max: 100.}
        SliderRound{text: "precision" precision: 20}
        SliderRound{text: "stepped" step: 0.1}
        SliderRound{
            text: "label_size"
            draw_bg +: {label_size: 150.}
        }

        Hr{}
        H4{text: "SliderRoundGradientY"}
        SliderRoundGradientY{text: "min/max" min: 0. max: 100.}
        SliderRoundGradientY{
            text: "min/max"
            animator +: {
                disabled: {
                    default: @on
                }
            }
        }
        SliderRoundGradientY{text: "precision" precision: 20}
        SliderRoundGradientY{text: "stepped" step: 0.1}

        Hr{}
        H4{text: "SliderRoundGradientX"}
        SliderRoundGradientX{text: "min/max" min: 0. max: 100.}
        SliderRoundGradientX{
            text: "min/max"
            animator +: {
                disabled: {
                    default: @on
                }
            }
        }
        SliderRoundGradientX{text: "precision" precision: 20}
        SliderRoundGradientX{text: "stepped" step: 0.1}

        Hr{}
        H4{text: "SliderRoundFlat"}
        SliderRoundFlat{text: "min/max" min: 0. max: 100.}
        SliderRoundFlat{
            text: "disabled"
            animator +: {
                disabled: {
                    default: @on
                }
            }
        }
        SliderRoundFlat{text: "precision" precision: 20}
        SliderRoundFlat{text: "stepped" step: 0.1}
    

        StoryHeading{text: "One slider, under the controls"}
        StoryNote{text: "One slider; its range and step come from the controls."}
        StoryRow{
            subject := Slider{width: 240. text: "Amount"}
        }
    }
    mod.stories.SliderTapers = StoryPage{
        StoryNote{text: "A taper is the law that turns travel into a value. Every slider below carries the SAME value; what differs is where that value sits along the track, which is the whole of what a taper is."}

        StoryHeading{text: "One value, four laws"}
        StoryNote{text: "Range 0 to 2, value 0.5. Linear puts it a quarter along. Audio treats the value as unity and keeps it at half travel, so the cut side is a square law and the boost side runs straight to the top. Stepped snaps to the nearest quarter. Binary has only two positions."}
        StoryRow{width: Fill flow: Down
            Slider{width: Fill text: "Linear"  min: 0. max: 2. default: 0.5 taper: Linear}
            Slider{width: Fill text: "Audio"   min: 0. max: 2. default: 0.5 taper: Audio}
            Slider{width: Fill text: "Stepped" min: 0. max: 2. default: 0.5 step: 0.25 taper: Stepped}
            Slider{width: Fill text: "Binary"  min: 0. max: 2. default: 0.5 taper: Binary}
        }

        StoryHeading{text: "Equal travel, equal ratio"}
        StoryNote{text: "A frequency knob over 20 to 20000, both holding 1000. Under a linear law a thousand is a twentieth of the way along and the whole useful range is crushed into the first inch; under a logarithmic one it sits near the middle, because equal travel is equal ratio."}
        StoryRow{width: Fill flow: Down
            Slider{width: Fill text: "Linear, 20..20000" min: 20. max: 20000. default: 1000. precision: 0 taper: Linear}
            Slider{width: Fill text: "Log, 20..20000"    min: 20. max: 20000. default: 1000. precision: 0 taper: Log}
        }
    }

}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/slider/overview",
    category: "Inputs",
    component: "Slider",
    also: &["SliderMinimal", "SliderMinimalFlat", "SliderRound", "SliderRoundFlat", "SliderRoundGradientX", "SliderRoundGradientY"],
    name: "Overview",
    dsl: "SliderOverview",
    added: "2026-02-16",
    tags: &["controls", "ported"],
    doc: "# Slider\n\nSliders allow selecting numeric values.",
    subject: "",
    feature: None,
    controls: &[
            Control { label: "Label", target: "subject", kind: ControlKind::Text { prop: "text", default: "Amount" } },
            Control { label: "Maximum", target: "subject", kind: ControlKind::Number { prop: "max", min: 1., max: 1000., step: 1., default: 1. } },
            Control { label: "Step", target: "subject", kind: ControlKind::Number { prop: "step", min: 0., max: 10., step: 0.1, default: 0. } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
    on_actions: None,
}, Story {
    key: "inputs/slider/taper",
    category: "Inputs",
    component: "Slider",
    also: &[],
    name: "Taper",
    dsl: "SliderTapers",
    added: "2026-09-05",
    tags: &["new"],
    doc: "# Taper

A taper is the law that turns TRAVEL into a VALUE, and it is the difference between a control that feels right and one that fights you. Every slider on this page carries the same value; what differs is where that value sits along the track.

- **Linear** is the plain map, and `step` floors it.
- **Audio** is a gain law. The default is treated as unity and sits at half travel, the boost half runs straight to the top, and the cut half is a square law, so the control is fine near unity and fast near the kill. It falls back to Linear when the default is not strictly inside the range, because then there is no half to be either side of.
- **Log** makes equal travel equal RATIO, which is what a frequency control wants: over 20 to 20000, a thousand sits near the middle rather than a twentieth of the way along. It falls back to Linear when the range admits no ratio.
- **Stepped** snaps to the NEAREST multiple of `step`, where Linear floors to it. Detents rather than quantisation.
- **Binary** has two positions and nothing between them.

None of the five had ever been shown. A taper is geometric, not behavioural: the same value under two laws puts the handle in two places, so a still picture is enough to see one go wrong.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];

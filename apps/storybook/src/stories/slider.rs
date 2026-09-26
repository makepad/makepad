//! The slider story: one slider under the controls, range, step and taper,
//! every face in two columns, the disabled faces, and the styling reference
//! last.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Column = View{
        width: Fill
        height: Fit
        flow: Down
    }

    mod.stories.SliderOverview = StoryPage{
        StoryHeading{text: "One slider, under the controls"}
        StoryNote{text: "One slider; its range and step come from the controls."}
        StoryRow{
            subject := Slider{width: 240. text: "Amount"}
        }

        StoryHeading{text: "Range, decimals and step"}
        StoryNote{text: "`min` and `max` bound the value, `precision` sets how many decimals the value field shows, and `step` quantizes it; a step of 0 is continuous."}
        StoryRow{width: Fill flow: Down
            Slider{width: Fill text: "0 to 100" min: 0. max: 100.}
            Slider{width: Fill text: "Four decimals" precision: 4}
            Slider{width: Fill text: "Steps of 0.1" step: 0.1}
        }

        StoryHeading{text: "Taper"}
        StoryNote{text: "A taper is the law that turns travel into a value. Every slider in this section carries the SAME value; what differs is where that value sits along the track, which is the whole of what a taper is."}
        StoryNote{text: "Range 0 to 2, value 0.5. Linear puts it a quarter along. Audio treats the value as unity and keeps it at half travel, so the cut side is a square law and the boost side runs straight to the top. Stepped snaps to the nearest quarter. Binary has only two positions."}
        StoryRow{width: Fill flow: Down
            Slider{width: Fill text: "Linear"  min: 0. max: 2. default: 0.5 taper: Linear}
            Slider{width: Fill text: "Audio"   min: 0. max: 2. default: 0.5 taper: Audio}
            Slider{width: Fill text: "Stepped" min: 0. max: 2. default: 0.5 step: 0.25 taper: Stepped}
            Slider{width: Fill text: "Binary"  min: 0. max: 2. default: 0.5 taper: Binary}
        }
        StoryNote{text: "A frequency control over 20 to 20000, both holding 1000. Under a linear law a thousand is a twentieth of the way along and the whole useful range is crushed into the first inch; under a logarithmic one it sits near the middle, because equal travel is equal ratio."}
        StoryRow{width: Fill flow: Down
            Slider{width: Fill text: "Linear, 20..20000" min: 20. max: 20000. default: 1000. precision: 0 taper: Linear}
            Slider{width: Fill text: "Log, 20..20000"    min: 20. max: 20000. default: 1000. precision: 0 taper: Log}
        }

        StoryHeading{text: "Faces"}
        StoryNote{text: "The bar faces are on the left and the round faces, which put the label in a column beside a pill track, are on the right; their label columns are widened here to fit the names. The gradient faces fill the track with two stops, the flat faces leave out the theme bevel, and the minimal faces draw only two bands and the value."}
        StoryRow{
            align: Align{x: 0. y: 0.}
            Column{
                Slider{width: Fill text: "Slider"}
                SliderGradientY{width: Fill text: "SliderGradientY"}
                SliderGradientX{width: Fill text: "SliderGradientX"}
                SliderFlat{width: Fill text: "SliderFlat"}
                SliderMinimal{width: Fill text: "SliderMinimal"}
                SliderMinimalFlat{width: Fill text: "SliderMinimalFlat"}
            }
            Column{
                SliderRound{width: Fill text: "SliderRound" draw_bg +: {label_size: 150.}}
                SliderRoundGradientY{width: Fill text: "SliderRoundGradientY" draw_bg +: {label_size: 150.}}
                SliderRoundGradientX{width: Fill text: "SliderRoundGradientX" draw_bg +: {label_size: 150.}}
                SliderRoundFlat{width: Fill text: "SliderRoundFlat" draw_bg +: {label_size: 150.}}
            }
        }

        StoryHeading{text: "Disabled"}
        StoryNote{text: "A disabled slider takes the theme's disabled colours and does not answer the pointer or the keyboard."}
        StoryRow{
            align: Align{x: 0. y: 0.}
            Column{
                Slider{width: Fill text: "Slider" animator +: {disabled: {default: @on}}}
                SliderGradientY{width: Fill text: "SliderGradientY" animator +: {disabled: {default: @on}}}
                SliderGradientX{width: Fill text: "SliderGradientX" animator +: {disabled: {default: @on}}}
                SliderFlat{width: Fill text: "SliderFlat" animator +: {disabled: {default: @on}}}
                SliderMinimal{width: Fill text: "SliderMinimal" animator +: {disabled: {default: @on}}}
                SliderMinimalFlat{width: Fill text: "SliderMinimalFlat" animator +: {disabled: {default: @on}}}
            }
            Column{
                SliderRound{width: Fill text: "SliderRound" draw_bg +: {label_size: 150.} animator +: {disabled: {default: @on}}}
                SliderRoundGradientY{width: Fill text: "SliderRoundGradientY" draw_bg +: {label_size: 150.} animator +: {disabled: {default: @on}}}
                SliderRoundGradientX{width: Fill text: "SliderRoundGradientX" draw_bg +: {label_size: 150.} animator +: {disabled: {default: @on}}}
                SliderRoundFlat{width: Fill text: "SliderRoundFlat" draw_bg +: {label_size: 150.} animator +: {disabled: {default: @on}}}
            }
        }

        StoryHeading{text: "Styling reference"}
        StoryNote{text: "The round face's value fill and handle have a colour per state, and `label_size` sets the width of its label column."}
        StoryRow{width: Fill flow: Down
            SliderRound{
                width: Fill
                text: "Solid"
                draw_text +: {
                    color: #0ff
                }
                draw_bg +: {
                    val_color: #xF08
                    val_color_hover: #xF4A
                    val_color_focus: #xC04
                    val_color_drag: #xF08

                    val_color_2: #xF08
                    val_color_2_hover: #xF4A
                    val_color_2_focus: #xC04
                    val_color_2_drag: #xF08

                    handle_color: #xF
                    handle_color_hover: #xF
                    handle_color_focus: #xF
                    handle_color_drag: #xF
                }
            }
            SliderRound{
                width: Fill
                text: "Solid"
                draw_bg +: {
                    val_color: #6
                    val_color_2: #6
                    handle_color: #0
                }
            }
            SliderRound{
                width: Fill
                text: "label_size"
                draw_bg +: {label_size: 150.}
            }
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
    tags: &["controls", "ported", "taper"],
    doc: "# Slider\n\nA slider picks one number from a range by dragging a handle along a track. `min` and `max` bound it, `step` quantizes it (0 is continuous) and `precision` sets the decimals the value field shows.\n\n## Taper\n\nA taper is the law that turns TRAVEL into a VALUE, and it is the difference between a control that feels right and one that fights you. The same value under two laws puts the handle in two places, so a still picture is enough to see one go wrong.\n\n- **Linear** is the plain map, and `step` floors it.\n- **Audio** is a gain law. The default is treated as unity and sits at half travel, the boost half runs straight to the top, and the cut half is a square law, so the control is fine near unity and fast near the kill. It falls back to Linear when the default is not strictly inside the range, because then there is no half to be either side of.\n- **Log** makes equal travel equal RATIO, which is what a frequency control wants: over 20 to 20000, a thousand sits near the middle rather than a twentieth of the way along. It falls back to Linear when the range admits no ratio.\n- **Stepped** snaps to the NEAREST multiple of `step`, where Linear floors to it. Detents rather than quantisation.\n- **Binary** has two positions and nothing between them.\n\n## Faces\n\n`SliderFlat` is a boxed track with a centre ridge, a value line and a drag handle, and `Slider` adds the theme's inset bevel and handle gradient to it. `SliderGradientY` and `SliderGradientX` fill the track with two stops. `SliderMinimal` and `SliderMinimalFlat` draw only a shadow band, a highlight band and the value. `SliderRound` and its gradient and flat faces put the label in a column beside a pill track with a capsule fill.\n\n## Styling\n\nThe round faces take a colour per state for the value fill (`val_color`, `val_color_2`) and the handle (`handle_color`), each with `_hover`, `_focus` and `_drag`, and `label_size` sets the width of the label column.",
    subject: "",
    feature: None,
    controls: &[
            Control { label: "Label", target: "subject", kind: ControlKind::Text { prop: "text", default: "Amount" } },
            Control { label: "Maximum", target: "subject", kind: ControlKind::Number { prop: "max", min: 1., max: 1000., step: 1., default: 1. } },
            Control { label: "Step", target: "subject", kind: ControlKind::Number { prop: "step", min: 0., max: 10., step: 0.1, default: 0. } },
            Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
        ],
    on_actions: None,
}];

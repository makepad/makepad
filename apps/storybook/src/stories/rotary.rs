//! The rotary stories: the standard, gradient and flat dials with their gap, size and padding variants, ported from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.RotaryOverview = StoryPage{
        H4{text: "Rotary"}
        StoryRow{
            align: Align{x: 0. y: 0.}

            Rotary{text: "Label"}

            Rotary{
                text: "Label"
                draw_bg +: {
                    val_size: uniform(10.)
                    val_padding: uniform(2.)
                    gap: uniform(0.)
                }
            }

            Rotary{
                text: "Label"
                draw_bg +: {
                    val_size: uniform(5.)
                    val_padding: uniform(2.5)
                    gap: uniform(180.)
                }
            }

            Rotary{
                text: "Label"
                draw_bg +: {
                    val_size: uniform(5.)
                    val_padding: uniform(0.)
                    gap: uniform(180.)
                }
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }

            Rotary{
                width: Fill
                height: 150
                text: "Label"
                draw_bg +: {
                    val_size: uniform(10.)
                    val_padding: uniform(5.)
                }
            }
        }

        Hr{}
        H4{text: "RotaryGradientY"}
        StoryRow{
            align: Align{x: 0. y: 0.}
            RotaryGradientY{text: "Label"}
            RotaryGradientY{
                text: "Label"
                draw_bg +: {gap: uniform(0.)}
            }
            RotaryGradientY{
                text: "Label"
                draw_bg +: {gap: uniform(180.)}
            }
            RotaryGradientY{
                text: "Label"
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
                draw_bg +: {val_size: uniform(20.)}
            }
            RotaryGradientY{
                width: Fill
                height: 150
                text: "Label"
                draw_bg +: {
                    val_size: uniform(10.)
                    val_padding: uniform(5.)
                }
            }
        }

        Hr{}
        H4{text: "RotaryFlat"}
        StoryRow{
            align: Align{x: 0. y: 0.}
            RotaryFlat{text: "Label"}
            RotaryFlat{
                text: "Label"
                draw_bg +: {gap: uniform(0.)}
            }
            RotaryFlat{
                text: "Label"
                draw_bg +: {gap: uniform(180.)}
            }
            RotaryFlat{
                text: "Label"
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
                draw_bg +: {val_size: uniform(10.)}
            }
            RotaryFlat{
                width: Fill
                height: 150
                text: "Label"
                draw_bg +: {
                    val_size: uniform(10.)
                    val_padding: uniform(8.)
                }
            }
        }
    }
    mod.stories.RotaryBipolar = StoryPage{
        StoryNote{text: "A pan or a tone control has a CENTRE, not a floor. arc_from_origin fills the arc from the default outward instead of from the stop, so a cut and a boost point opposite ways and zero shows as nothing at all rather than as a half-filled ring that looks like a setting."}

        StoryRow{
            set_cut := Button{text: "Cut"}
            set_centre := Button{text: "Centre"}
            set_boost := Button{text: "Boost"}
            bip_note := Label{text: "both knobs at 0.00"}
        }

        StoryHeading{text: "From the origin, and from the stop"}
        StoryNote{text: "The same value in both. The left one fills from the centre, so the sign is in the picture; the right one fills from the stop, where a cut and a boost of the same size look nothing like each other and zero looks like half of something."}
        StoryRow{
            align: Align{x: 0. y: 0.}
            spacing: theme.space_4
            from_origin := Rotary{text: "from origin" min: -1. max: 1. default: 0.0 arc_from_origin: true}
            from_stop := Rotary{text: "from stop" min: -1. max: 1. default: 0.0}
        }
    }

    mod.stories.RotaryKnobPage = StoryPage{
        StoryNote{text: "One knob, under the controls. The gap, the groove and the needle are the three numbers worth turning; everything else in the drawing follows the radius."}
        StoryRow{
            subject := RotaryKnob{width: 64. height: 64. default: 0.35}
        }

        StoryHeading{text: "Four sizes, one drawing"}
        StoryNote{text: "24, 32, 44 and 64 across, all holding the same value. The groove, the clearances and the needle are fractions of the radius under floors of half a pixel to a pixel, and the fractions are picked so that at 24 every term is still the fraction: the first floor bites at 22.2 across and the rest between 21.9 and 16.7. Only the bezel is absolute at every size. Thin the groove and its own floor arrives much sooner -- at about 62 across for the thinnest the sidebar offers."}
        StoryRow{
            align: Align{x: 0. y: 1.}
            RotaryKnob{width: 24. height: 24. default: 0.35}
            RotaryKnob{width: 32. height: 32. default: 0.35}
            RotaryKnob{width: 44. height: 44. default: 0.35}
            RotaryKnob{width: 64. height: 64. default: 0.35}
        }

        StoryHeading{text: "The same box, given to a Rotary"}
        StoryNote{text: "A stock Rotary in a 24 square box, for comparison. Its shader reserves twenty pixels above the dial, so at 24 the dial centre lands below the bottom edge of its own box and what is left inside is the top four pixels of its track, with the readout it always draws still sitting over them. That reserve is the rule that a Rotary fits only when its box is twenty taller than it is wide, which a square box cannot be at any size."}
        StoryRow{
            align: Align{x: 0. y: 1.}
            RotaryKnob{width: 24. height: 24. default: 0.35}
            Rotary{width: 24. height: 24. default: 0.35}
            Rotary{default: 0.35}
        }

        StoryHeading{text: "A row of them, legends and all"}
        StoryNote{text: "The legend row is whatever height the box has over its width, so these are 44 by 64 with the text on top -- the same twenty pixel row the rest of the slider family reserves for a one-line label, and 15 points of it are used in all three themes. A square box has no room for one, which is the compact form above. A tap on the legend puts a knob back to its default; a knob with no legend has no strip to tap, and no reset zone over its face either. The last one is disabled: the groove is gone entirely, which is a state no live value can imitate."}
        StoryRow{
            spacing: theme.space_1
            RotaryKnob{text: "GAIN" default: 0.62}
            RotaryKnob{text: "LOW" min: -1. max: 1. default: 0. arc_from_origin: true}
            RotaryKnob{text: "MID" min: -1. max: 1. default: 0. arc_from_origin: true}
            RotaryKnob{text: "HIGH" min: -1. max: 1. default: 0. arc_from_origin: true}
            RotaryKnob{text: "SEND" default: 0.18}
            RotaryKnob{
                text: "OFF"
                default: 0.4
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }
        }

        StoryHeading{text: "Centre, and where the arc says it is"}
        StoryNote{text: "Both controls hold the same value and both fill from their default. There is no separate centre mark and none is needed: the arc runs BETWEEN the resting angle and the value, so one of its ends is always at rest, and at rest the arc collapses to a single round cap sitting exactly on that angle. Press the three buttons and watch which end stays put."}
        StoryRow{
            set_cut := Button{text: "Cut"}
            set_home := Button{text: "Centre"}
            set_boost := Button{text: "Boost"}
            knob_note := Label{text: "both at 0.00"}
        }
        StoryRow{
            align: Align{x: 0. y: 1.}
            spacing: theme.space_4
            tone_knob := RotaryKnob{text: "TONE" min: -1. max: 1. default: 0. arc_from_origin: true}
            tone_dial := Rotary{text: "Tone" min: -1. max: 1. default: 0. arc_from_origin: true}
        }

        StoryHeading{text: "The three numbers, and the material"}
        StoryNote{text: "A gap of 25 nearly closes the groove; 160 leaves a quarter of it open. The sidebar stops at 20 because at 0 both stops land on the same angle and the needle points the same way at the minimum and at the maximum. A fat groove reads across a room, a thin one with a long needle reads as an instrument. The last one is the same drawing on a paler material, which is the one line it takes to move the disc off the darkest step -- and it costs something: the unlit groove and the lit arc are spaced off the material by a rule, and a paler material leaves them less room. At color_opaque_d_4 the two steps fall from about 3.2 and 3.6 to 1 to about 3.1 and 3.7 in the dark theme, and to 2.5 and 2.8 in the light ones. The darkest step is the darkest step for a reason."}
        StoryRow{
            align: Align{x: 0. y: 1.}
            RotaryKnob{width: 44. height: 44. default: 0.62 draw_bg +: {gap: uniform(25.)}}
            RotaryKnob{width: 44. height: 44. default: 0.62 draw_bg +: {gap: uniform(160.)}}
            RotaryKnob{width: 44. height: 44. default: 0.62 draw_bg +: {ring_size: uniform(0.28)}}
            RotaryKnob{width: 44. height: 44. default: 0.62 draw_bg +: {ring_size: uniform(0.05) pointer_length: uniform(0.9)}}
            RotaryKnob{
                width: 44. height: 44. default: 0.62
                draw_bg +: {
                    color: uniform(theme.color_opaque_d_4)
                }
            }
            RotaryKnob{
                width: 44. height: 44. default: 0.62
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
                draw_bg +: {
                    color: uniform(theme.color_opaque_d_4)
                }
            }
        }
    }

}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/rotary/overview",
    category: "Inputs",
    component: "Rotary",
    also: &[],
    name: "Overview",
    dsl: "RotaryOverview",
    added: "2026-02-16",
    tags: &["ported"],
    doc: "# Rotary\n\nRotary controls allow selecting values with a circular dial.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}, Story {
    key: "inputs/rotary/bipolar",
    category: "Inputs",
    component: "Rotary",
    also: &[],
    name: "Bipolar",
    dsl: "RotaryBipolar",
    added: "2026-09-05",
    tags: &["new"],
    doc: "# Bipolar

Some controls have a CENTRE rather than a floor. Pan, pitch, a tone control: zero is not \"none of it\", it is \"neither way\", and the two directions mean opposite things.

`arc_from_origin` fills the arc from the DEFAULT outward instead of from the stop. Three things follow, and all three are the point. A cut and a boost of the same size point opposite ways, so the sign is in the picture. Zero shows as nothing at all rather than as a half-filled ring that looks like a setting. And the eye reads distance from centre, which is the quantity that matters, instead of distance from a stop nobody cares about.

The second row is the same three values without it, where all three fill from the same end and the sign is simply absent.",
    subject: "from_origin",
    feature: None,
    controls: &[],
    on_actions: Some(bipolar_actions),
}, Story {
    key: "inputs/rotary/knob",
    category: "Inputs",
    component: "Rotary",
    also: &[],
    name: "Knob",
    dsl: "RotaryKnobPage",
    added: "2026-09-10",
    tags: &["new", "controls"],
    doc: "# Knob

A `Rotary` is a dial with its label and its readout in a row ABOVE it, drawn for a 65 by 95 well: one control, read one at a time, in a form. A `RotaryKnob` is the panel version of the same slider. One dark disc with the value cut into a groove near its edge, meant to be built into a row of twenty at 24 pixels a side, where nobody reads any single one of them and the SHAPE of the row is the reading.

Everything else about it follows from that.

**It fits the box it is given.** The knob is the largest circle the box holds, pinned to the bottom, so the height left over on top is the legend row and a square box is all knob. The stock rotary reserves twenty pixels for its label inside the shader, which is why it only fits when the box is twenty taller than it is wide: at 24 square its dial centre lands below its own bottom edge and what stays inside the box is the top four pixels of its track.

**One disc, and everything is drawn on it.** The groove is INSIDE the disc rather than floating outside it, and that is arithmetic rather than taste. Whatever sits on the page has to stand off the page, and in the dark theme the page is a mid grey that nothing dark can stand off: pure black on it is 2.44 to 1, short of the 3 to 1 a boundary wants, while a light mark has to reach #999999 before it clears that -- which leaves under 2.9 to 1 above it for a lit arc to be told apart from an unlit one. Draw the groove on the material instead and there is one ground for everything, the same ground in all three themes, and every number below is checkable.

**Almost nothing is measured in pixels.** The groove thickness, its clearance inside the box, the material either side of it, the needle length and its width are all fractions of the radius, each under a floor of half a pixel to a pixel so that nothing degenerates into a sub-pixel smear. The fractions are picked so that at 24 across every one of them is still the fraction: the first floor to bite is the material either side of the groove, at 22.2 across, and the rest follow at 21.9, 20.8, 20.0 and 16.7. Below that the drawing grows a shade heavier than proportion asks. The bezel alone is absolute at every size, because a bezel is a hairline and scaling it would make it a second groove. A caller who thins the groove meets its floor much sooner: at `ring_size: 0.04` it draws its floor below about 62 across.

**The material does not move, and the three inks are spaced off it.** The disc is `color_opaque_d_5`, the darkest opaque step every theme carries, and it is the same in every state -- one rung up that ladder lands within seven values of the dark theme own page, so a body that lifted under the hand vanished into the panel at the moment it was being turned. The value ink is `color_opaque_u_6`, at 13.3, 11.4 and 10.8 to 1 on that material in the three themes. The unlit groove is not a token at all: it is the material carried four tenths of the way to the ink, which puts it 3.17, 3.19 and 3.15 to 1 above the material and 4.20, 3.56 and 3.41 to 1 below the ink. No token could have done that -- the light themes bright rungs are all bunched within 1.4 to 1 of white -- and a rule follows a caller who re-colours either end.

**The bezel is the only thing that touches the page.** It takes `color_inverse_surface`, the one token whose job is to stand against the surface and the only one that changes sides with the theme: 6.4, 8.7 and 8.1 to 1 against the page. In the light themes it resolves to the material itself, so there is no bezel to see -- and none is needed, the disc already being 8 to 1 off its own page. In the dark theme it is the whole reason a 24 pixel knob has an edge, the disc managing 2.1 to 1 there and nothing darker able to do better.

**The needle is always there.** The groove and the needle answer to the value and to nothing else, because a lit groove that also brightened under the pointer would be saying two things with one colour.

**Hover, focus and drag are said by size, not by colour.** A handle grows out of nothing on the value end of the arc. Colour had nowhere to go: the theme bevel pair composited over this material steps 3.1 to 1 in the dark theme but only 1.9 and 1.5 to 1 in the light ones, so a colour step that reads in one theme is invisible in the other two. A handle that grows is the same step in every theme, being no contrast at all. Here it is an ADDITION to a pointer that is always drawn, which is what makes it right on a knob and wrong as the only indicator.

**What says dead is the groove going out -- all of it.** The ink falls to the material and the unlit groove is derived from the ink, so both ends of the groove land on the material and the knob becomes a plain disc. No live value can imitate that: value 0 still shows a full groove with a lit cap on the stop, and value 1 a fully lit one. The bezel takes the material too, so in the dark theme the edge softens as well. The needle stays, at the unlit groove own brightness -- a fall of about 4 to 1 in every theme. A dead knob still points.

**A centre needs no mark.** With `arc_from_origin` the arc runs between the resting angle and the value rather than from the stop, so a cut and a boost point opposite ways and zero is nothing at all rather than a half-filled groove that looks like a setting. One end of that arc is always AT the resting angle, and at rest the whole arc collapses to a single round cap sitting on it. Home is drawn by the arc itself at every value.

**There is no number.** The value field is collapsed to nothing, because four digits do not fit beside a 24 pixel knob and on a square one there is no row to put them in -- the label and the readout are drawn inside the same box the knob fills. The field still takes key focus on a press, so a value can be typed, blind, which is the price; giving `text_input` a size back puts the readout on screen.

**Drag is measured in points, not in box heights.** `drag_travel: 160` puts the whole range in 160 points of pointer travel whatever the knob is drawn at. Left at the family default of 0, a slider divides by its own height along the drag axis, which is 95 on a stock rotary and would be 24 here -- four percent of the range per pixel -- and there is no fine-drag modifier; the Shift and Ctrl ladder is on the wheel only. The wheel is not the answer either: nothing marks a scroll as consumed, so a knob that took the wheel inside a scrolling panel would move its value and scroll the panel with the same gesture. `scroll_step` stays at 0, here as on every other slider in the library.

Two behaviours are the family and stay the family. A tap on the legend puts the control back to its `default`, and a double click anywhere does the same. A knob with no legend has no strip to tap, so the compact form cannot be reset by a stationary click -- and, just as important, cannot be reset by a stationary click meant to focus it.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Gap", target: "subject", kind: ControlKind::Number { prop: "draw_bg.gap", min: 20., max: 180., step: 5., default: 90. } },
        Control { label: "Groove", target: "subject", kind: ControlKind::Number { prop: "draw_bg.ring_size", min: 0.04, max: 0.3, step: 0.01, default: 0.12 } },
        Control { label: "Needle", target: "subject", kind: ControlKind::Number { prop: "draw_bg.pointer_length", min: 0.1, max: 1., step: 0.05, default: 0.55 } },
        Control { label: "Disabled", target: "subject", kind: ControlKind::Disabled { default: false } },
    ],
    on_actions: Some(knob_actions),
}];

fn bipolar_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    for (id, v) in [
        (ids!(set_cut), -0.6f64),
        (ids!(set_centre), 0.0),
        (ids!(set_boost), 0.6),
    ] {
        if root.button(cx, id).clicked(actions) {
            root.slider(cx, ids!(from_origin)).set_value(cx, v);
            root.slider(cx, ids!(from_stop)).set_value(cx, v);
            root.label(cx, ids!(bip_note)).set_text(cx, &format!("both knobs at {v:.2}"));
        }
    }
}

fn knob_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    for (id, v) in [
        (ids!(set_cut), -0.7f64),
        (ids!(set_home), 0.0),
        (ids!(set_boost), 0.7),
    ] {
        if root.button(cx, id).clicked(actions) {
            // No redraw here: Slider::set_value repaints its own material
            // now. It did not before, and this page would have been the
            // one to notice -- the knob readout is sized to nothing, so
            // the readout own repaint, which was the only one a caller
            // ever got, would have dirtied nothing at all.
            root.slider(cx, ids!(tone_knob)).set_value(cx, v);
            root.slider(cx, ids!(tone_dial)).set_value(cx, v);
            root.label(cx, ids!(knob_note)).set_text(cx, &format!("both at {v:.2}"));
        }
    }
}

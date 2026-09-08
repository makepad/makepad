//! The rotary stories: the standard, gradient and flat dials with their gap, size and padding variants, ported from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::Story;

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

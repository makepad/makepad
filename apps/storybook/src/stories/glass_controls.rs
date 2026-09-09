//! The glass controls story: four controls that show you what is behind
//! them, and therefore need something to be behind them.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    // The ground the lens bends. A plain View cannot hand a caller a slot,
    // so the scene is written out where it is used rather than wrapped in a
    // template with a hole in it.
    let Ground = View{
        width: Fill
        height: Fill
        show_bg: true
        draw_bg +: {
            pixel: fn() {
                let p = self.pos
                let a = vec3(0.05, 0.12, 0.38)
                let b = vec3(0.62, 0.16, 0.42)
                let c = vec3(0.05, 0.42, 0.45)
                let m = mix(a, b, p.x)
                let n = mix(c, b, p.y)
                return vec4(mix(m, n, 0.45 + 0.35 * sin(p.x * 6.0 + p.y * 3.0)), 1.0)
            }
        }
    }

    mod.stories.GlassControlsOverview = StoryPage{
        StoryNote{text: "Four controls that bend what is behind them. They are the panel's family: the same lensing, applied to a toggle, a button, a slider and a segmented row."}

        StoryHeading{text: "Over something worth bending"}
        StoryNote{text: "The colours under these are drawn by the page, not by the controls. Put one on a flat ground and the lensing has nothing to work with, which is the commonest way this family disappoints."}
        StoryRow{
            View{
                width: Fill
                // Fixed, not Fit: the ground is height: Fill, and a Fill
                // child of a Fit overlay is given nothing at all - which
                // drew no gradient and quietly made this page's whole
                // comparison a lie.
                height: 230.
                flow: Overlay
                Ground{}
                View{
                    width: Fill
                    height: Fit
                    flow: Down
                    spacing: theme.space_2
                    padding: theme.mspace_3
                    View{
                        width: Fill height: Fit flow: Right spacing: theme.space_2
                        align: Align{y: 0.5}
                        radio_one := mod.widgets.glass.GlassRadio{}
                        mod.widgets.glass.OptionLabel{text: "Air"}
                        radio_two := mod.widgets.glass.GlassRadio{}
                        mod.widgets.glass.OptionLabel{text: "Water"}
                    }
                    View{
                        width: Fill height: Fit flow: Right spacing: theme.space_2
                        mod.widgets.glass.GlassButtonProminent{text: "Continue"}
                        subject := mod.widgets.glass.GlassButton{text: "Cancel"}
                    }
                    mod.widgets.glass.GlassSlider{width: Fill}
                    mod.widgets.glass.GlassSegmented{
                        width: Fill
                        labels: ["Day" "Week" "Month"]
                    }
                }
            }
        }

        StoryHeading{text: "The same four on a flat ground"}
        StoryNote{text: "Identical declarations, over one colour. This is what the family looks like when there is nothing behind it to refract, and it is worth seeing next to the row above before choosing it for a page that has a plain background."}
        StoryRow{
            View{
                width: Fill height: Fit flow: Down spacing: theme.space_2
                padding: theme.mspace_3
                show_bg: true
                draw_bg +: {color: #x101018}
                View{
                    width: Fill height: Fit flow: Right spacing: theme.space_2
                    align: Align{y: 0.5}
                    mod.widgets.glass.GlassRadio{}
                    mod.widgets.glass.OptionLabel{text: "Air"}
                }
                View{
                    width: Fill height: Fit flow: Right spacing: theme.space_2
                    mod.widgets.glass.GlassButtonProminent{text: "Continue"}
                    mod.widgets.glass.GlassButton{text: "Cancel"}
                }
                mod.widgets.glass.GlassSlider{width: Fill}
                mod.widgets.glass.GlassSegmented{
                    width: Fill
                    labels: ["Day" "Week" "Month"]
                }
            }
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/glass/controls",
    category: "Inputs",
    component: "GlassControls",
    also: &["GlassButton", "GlassButtonProminent", "GlassRadio", "GlassSegmented", "GlassSlider", "OptionLabel"],
    name: "Controls",
    dsl: "GlassControlsOverview",
    added: "2026-02-12",
    tags: &[],
    doc: "# Glass controls

A toggle, a button, a slider and a segmented row, all built on the same lensing the glass panel uses. They live under `mod.widgets.glass`, not at the top of the widget module, so they are written `glass.GlassButton` and so on.

**They draw what is behind them, so something has to be behind them.** The lens refracts the scene underneath; over a flat colour there is nothing to bend and the effect collapses to a faint outline. The two rows on this page are the same four declarations over a colourful ground and over one colour, which is the comparison worth making before choosing this family for a page whose background is plain.

They must also be drawn in the same pass as what they refract. The glass example in this repository puts its content in the background pass rather than in a `glass.Layer` for exactly that reason — a layer would hide the base from the lens, and each toggle needs to refract its own track and knob.

`GlassButtonProminent` is the filled variant of `GlassButton`; the rest take the shapes you would expect — `labels` on the segmented row, a value on the slider.",
    subject: "subject",
    feature: None,
    controls: &[],
    on_actions: None,
}];

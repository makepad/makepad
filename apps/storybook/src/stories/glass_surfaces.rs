//! The glass surfaces gallery: the lensing backing and every preset of it,
//! over something worth bending.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

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

    let Cap = Label{draw_text +: {color: #ffffffcc}}

    mod.stories.GlassSurfacesOverview = StoryPage{
        StoryNote{text: "The lensing surface the glass family is built on, and the presets of it that each control uses. They all bend what is behind them, so they are shown over the same colourful ground — on a flat one they collapse to an outline."}

        StoryHeading{text: "The surfaces"}
        StoryNote{text: "LensSurface is the base every control's backing derives from. The rest are the same surface tuned for what sits on it: a button, a prominent button, a chip, an icon, an input, a radio."}
        StoryRow{
            View{
                width: Fill height: 210. flow: Overlay
                Ground{}
                View{
                    width: Fill height: Fit flow: Down spacing: theme.space_2
                    padding: theme.mspace_3
                    View{
                        width: Fill height: Fit flow: Right spacing: theme.space_2
                        align: Align{y: 0.5}
                        mod.widgets.glass.LensSurface{width: 110. height: 44.}
                        Cap{text: "LensSurface"}
                        mod.widgets.glass.ButtonSurface{width: 110. height: 44.}
                        Cap{text: "ButtonSurface"}
                    }
                    View{
                        width: Fill height: Fit flow: Right spacing: theme.space_2
                        align: Align{y: 0.5}
                        mod.widgets.glass.ProminentButtonSurface{width: 110. height: 44.}
                        Cap{text: "ProminentButtonSurface"}
                        mod.widgets.glass.ChipSurface{width: 90. height: 32.}
                        Cap{text: "ChipSurface"}
                    }
                    View{
                        width: Fill height: Fit flow: Right spacing: theme.space_2
                        align: Align{y: 0.5}
                        mod.widgets.glass.IconSurface{width: 44. height: 44.}
                        Cap{text: "IconSurface"}
                        mod.widgets.glass.InputSurface{width: 140. height: 36.}
                        Cap{text: "InputSurface"}
                        mod.widgets.glass.RadioSurface{width: 44. height: 26.}
                        Cap{text: "RadioSurface"}
                    }
                }
            }
        }

        StoryHeading{text: "The controls made from them"}
        StoryNote{text: "A lens button and a lens chip are those same surfaces with a padding and a centring — despite the names they are not buttons. They carry no text and no press: the label goes inside them, and if you want something that answers a click, that is GlassButton on the glass controls page."}
        StoryRow{
            View{
                width: Fill height: 190. flow: Overlay
                Ground{}
                View{
                    width: Fill height: Fit flow: Down spacing: theme.space_2
                    padding: theme.mspace_3
                    View{
                        width: Fill height: Fit flow: Right spacing: theme.space_2
                        align: Align{y: 0.5}
                        // No text on any of these: they are surfaces with a
                        // padding and a centring, so the label goes inside.
                        mod.widgets.glass.LensButton{Cap{text: "LensButton"}}
                        mod.widgets.glass.LensButtonProminent{Cap{text: "LensButtonProminent"}}
                        mod.widgets.glass.LensChip{Cap{text: "LensChip"}}
                    }
                    mod.widgets.glass.ClearPanel{
                        width: Fill height: Fit
                        padding: theme.mspace_2
                        Cap{text: "ClearPanel"}
                    }
                    mod.widgets.glass.List{
                        width: Fill height: Fit
                        mod.widgets.glass.ListRow{Cap{text: "a glass list row"}}
                        mod.widgets.glass.ListRow{Cap{text: "and another"}}
                    }
                }
            }
        }

        StoryHeading{text: "The two rounded surfaces underneath"}
        StoryNote{text: "AppleGlassRoundedView is the tuned preset of GaussRoundedView the whole family sits on, and GaussGradientRoundedView is the version that also lays a gradient over the blur."}
        StoryRow{
            View{
                width: Fill height: 150. flow: Overlay
                Ground{}
                View{
                    width: Fill height: Fit flow: Right spacing: theme.space_2
                    align: Align{y: 0.5}
                    padding: theme.mspace_3
                    AppleGlassRoundedView{width: 150. height: 70.}
                    Cap{text: "AppleGlassRoundedView"}
                    GaussGradientRoundedView{width: 150. height: 70.}
                    Cap{text: "GaussGradientRoundedView"}
                }
            }
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/glasssurfaces/overview",
    category: "Containers",
    component: "GlassSurfaces",
    also: &[
        "LensSurface", "ButtonSurface", "ProminentButtonSurface", "ChipSurface",
        "IconSurface", "InputSurface", "RadioSurface",
        "LensButton", "LensButtonProminent", "LensChip", "ClearPanel",
        "List", "ListRow",
        "AppleGlassRoundedView", "GaussGradientRoundedView",
    ],
    name: "Surfaces",
    dsl: "GlassSurfacesOverview",
    added: "2026-02-12",
    tags: &[],
    doc: "# Glass surfaces

The lensing backing the glass family is built on, and every preset of it the library ships.

`GaussRoundedView` is the raw surface: it samples the scene behind itself through a chain of mip textures and blurs it. `AppleGlassRoundedView` is the tuned preset the family actually sits on, and `GaussGradientRoundedView` lays a gradient over the blur as well.

`LensSurface` derives from that, and everything else derives from `LensSurface` — the same surface adjusted for what sits on top of it. `ButtonSurface` and `ProminentButtonSurface` for buttons, `ChipSurface` for a chip, `IconSurface` for a square icon target, `InputSurface` for a field, `RadioSurface` for a toggle. **`LensButton`, `LensButtonProminent` and `LensChip` are not buttons.** They are the same surfaces with a padding and a centring, and they carry neither a label nor a press — writing `text:` on one is rejected at runtime, where only the log can see it. Put the label inside. The thing that answers a click is `GlassButton`, on the glass controls page.

`ClearPanel` is the plain sheet, and `List` with `ListRow` are the family's own rows — note that these live under `mod.widgets.glass` and are *not* general-purpose list views, which is a mistake worth making only once.

**They are shown here over a coloured ground on purpose.** Every one of them draws what is behind it, so on a flat background the whole family collapses to a faint outline. If the page you are putting one on has nothing worth refracting, this is not the family you want — see the glass controls page for the same comparison made side by side.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];

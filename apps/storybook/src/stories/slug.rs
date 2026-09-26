//! The large-text page: the same text widgets on both sides of the size at
//! which glyphs stop coming from the atlas and are drawn from their
//! outlines, and a set of probes above that size that each vary one thing.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let LargeTextCard = RoundedView{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2
        padding: theme.mspace_2
        show_bg: true
        // Plain values: a uniform(..) in this merge does not set RoundedView's instance colours, and the card drew no fill or border.
        draw_bg +: {
            color: theme.color_inset
            border_radius: theme.corner_radius
            border_size: 1.0
            border_color: #fff2
        }
    }

    let LargeTextTitle = Pbold{
        width: Fill
        margin: 0.
    }

    let LargeTextNote = P{
        width: Fill
        margin: 0.
    }

    let LargeTextProbeGrid = View{
        width: Fill
        height: Fit
        flow: Flow.Right{wrap: true}
        spacing: theme.space_2
    }

    let LargeTextProbeCard = RoundedView{
        width: 200.
        height: Fit
        flow: Down
        spacing: theme.space_2
        padding: theme.mspace_2
        show_bg: true
        // Plain values, as on LargeTextCard: the uniform(..) colours never drew.
        draw_bg +: {
            color: theme.color_inset_1
            border_radius: theme.corner_radius
            border_size: 1.0
            border_color: #fff1
        }
    }

    let LargeDefaultLabel = Label{
        draw_text +: {
            text_style +: {
                font_size: 160.
            }
        }
    }

    let LargeLiteralLabel = Label{
        draw_text +: {
            color: #fff
            text_style +: {
                font_size: 160.
            }
        }
    }

    let LargeThemeLabel = Label{
        draw_text +: {
            color: theme.color_text
            text_style +: {
                font_size: 160.
            }
        }
    }

    let LargeAccentLabel = Label{
        draw_text +: {
            color: theme.color_makepad
            text_style +: {
                font_size: 160.
            }
        }
    }

    let LargeGradientLabel = Label{
        draw_text +: {
            color: #x6CF
            color_2: #xFD6
            gradient_fill_horizontal: 1.0
            text_style +: {
                font_size: 160.
            }
        }
    }

    let LargeCustomLiteralLabel = Label{
        draw_text +: {
            color: #xF75
            text_style +: {
                font_size: 160.
            }
            get_color: fn() -> vec4 {
                return mix(#xF75 #0000 self.pos.x)
            }
        }
    }

    let LargeCustomThemeLabel = Label{
        draw_text +: {
            color: theme.color_makepad
            text_style +: {
                font_size: 160.
            }
            get_color: fn() -> vec4 {
                return mix(theme.color_makepad #0000 self.pos.x)
            }
        }
    }

    mod.stories.SlugOverview = StoryPage{
        StoryNote{text: "Text is drawn one of two ways. Up to the largest size the glyph atlas holds, each glyph is a small picture taken from the atlas. Past that size the glyph is drawn from its outline, so large type stays sharp instead of turning soft. Some systems draw every size from outlines."}
        StoryNote{text: "Each pair below is one widget with only its font size changed, the left one under that size and the right one over it. Any difference between the two is a difference between the two ways of drawing."}

        StoryHeading{text: "A plain label"}
        StoryRow{
            align: Align{x: 0. y: 0.}
            LargeTextCard{
                LargeTextTitle{text: "Font size 32"}
                LargeTextNote{text: "Under the size: drawn from the atlas."}
                Label{
                    draw_text +: {
                        text_style +: {
                            font_size: 32.
                        }
                    }
                    text: "Ag"
                }
            }
            LargeTextCard{
                LargeTextTitle{text: "Font size 192"}
                LargeTextNote{text: "Over the size: drawn from the outline."}
                Label{
                    draw_text +: {
                        text_style +: {
                            font_size: 192.
                        }
                    }
                    text: "Ag"
                }
            }
        }

        StoryHeading{text: "A gradient label"}
        StoryNote{text: "The same two-colour label at both sizes, so the gradient can be checked against both ways of drawing."}
        StoryRow{
            align: Align{x: 0. y: 0.}
            LargeTextCard{
                LargeTextTitle{text: "Font size 32"}
                LargeTextNote{text: "LabelGradientX under the size."}
                LabelGradientX{
                    draw_text +: {
                        color: #x6CF
                        color_2: #xFD6
                        text_style +: {
                            font_size: 32.
                        }
                    }
                    text: "Type"
                }
            }
            LargeTextCard{
                LargeTextTitle{text: "Font size 192"}
                LargeTextNote{text: "LabelGradientX over the size."}
                LabelGradientX{
                    draw_text +: {
                        color: #x6CF
                        color_2: #xFD6
                        text_style +: {
                            font_size: 192.
                        }
                    }
                    text: "Ty"
                }
            }
        }

        StoryHeading{text: "A text shader of its own"}
        StoryNote{text: "Both labels replace get_color with the same fade, so a custom colour function can be checked on both sides."}
        StoryRow{
            align: Align{x: 0. y: 0.}
            LargeTextCard{
                LargeTextTitle{text: "Font size 32"}
                LargeTextNote{text: "A custom get_color under the size."}
                Label{
                    draw_text +: {
                        color: theme.color_makepad
                        text_style +: {
                            font_size: 32.
                        }
                        get_color: fn() -> vec4 {
                            return mix(theme.color_makepad #0000 self.pos.x)
                        }
                    }
                    text: "WAVE"
                }
            }
            LargeTextCard{
                LargeTextTitle{text: "Font size 192"}
                LargeTextNote{text: "The same get_color over the size."}
                Label{
                    draw_text +: {
                        color: theme.color_makepad
                        text_style +: {
                            font_size: 192.
                        }
                        get_color: fn() -> vec4 {
                            return mix(theme.color_makepad #0000 self.pos.x)
                        }
                    }
                    text: "W"
                }
            }
        }

        StoryHeading{text: "A link"}
        StoryNote{text: "The same interactive text widget at two sizes, with the link's own styling."}
        StoryRow{
            align: Align{x: 0. y: 0.}
            LargeTextCard{
                LargeTextTitle{text: "Font size 28"}
                LargeTextNote{text: "LinkLabel under the size."}
                LinkLabel{
                    draw_text +: {
                        gradient_fill_horizontal: 1.0
                        color: #x8CF
                        color_2: #xFB8
                        text_style +: {
                            font_size: 28.
                        }
                    }
                    text: "Open docs"
                }
            }
            LargeTextCard{
                LargeTextTitle{text: "Font size 144"}
                LargeTextNote{text: "LinkLabel over the size."}
                LinkLabel{
                    draw_text +: {
                        gradient_fill_horizontal: 1.0
                        color: #x8CF
                        color_2: #xFB8
                        text_style +: {
                            font_size: 144.
                        }
                    }
                    text: "Go"
                }
            }
        }

        StoryHeading{text: "Probes"}
        StoryNote{text: "Everything from here on is over the size, and each set varies one thing: where the colour comes from, the shape of the glyph, or a text shader of its own. When large text goes missing, the cards that go missing with it say which of those it depends on."}

        StoryHeading{text: "Where the colour comes from"}
        StoryNote{text: "One large label and one text, with the colour taken from five different places."}
        LargeTextProbeGrid{
            LargeTextProbeCard{
                LargeTextTitle{text: "No colour set"}
                LargeTextNote{text: "The label's own default."}
                LargeDefaultLabel{text: "Ag"}
            }
            LargeTextProbeCard{
                LargeTextTitle{text: "A literal white"}
                LargeTextNote{text: "color: #fff"}
                LargeLiteralLabel{text: "Ag"}
            }
            LargeTextProbeCard{
                LargeTextTitle{text: "The text colour"}
                LargeTextNote{text: "theme.color_text"}
                LargeThemeLabel{text: "Ag"}
            }
            LargeTextProbeCard{
                LargeTextTitle{text: "The accent"}
                LargeTextNote{text: "theme.color_makepad"}
                LargeAccentLabel{text: "Ag"}
            }
            LargeTextProbeCard{
                LargeTextTitle{text: "Two colours"}
                LargeTextNote{text: "color and color_2 as a gradient."}
                LargeGradientLabel{text: "Ag"}
            }
        }

        StoryHeading{text: "The shape of the glyph"}
        StoryNote{text: "The same white label with one glyph each, so a glyph that fails can be told from a colour that fails."}
        LargeTextProbeGrid{
            LargeTextProbeCard{
                LargeTextTitle{text: "A"}
                LargeTextNote{text: "A capital with a counter."}
                LargeLiteralLabel{text: "A"}
            }
            LargeTextProbeCard{
                LargeTextTitle{text: "g"}
                LargeTextNote{text: "A descender."}
                LargeLiteralLabel{text: "g"}
            }
            LargeTextProbeCard{
                LargeTextTitle{text: "W"}
                LargeTextNote{text: "A wide capital."}
                LargeLiteralLabel{text: "W"}
            }
            LargeTextProbeCard{
                LargeTextTitle{text: "S"}
                LargeTextNote{text: "One curve."}
                LargeLiteralLabel{text: "S"}
            }
            LargeTextProbeCard{
                LargeTextTitle{text: "L"}
                LargeTextNote{text: "Straight strokes and a corner."}
                LargeLiteralLabel{text: "L"}
            }
            LargeTextProbeCard{
                LargeTextTitle{text: "O"}
                LargeTextNote{text: "A closed loop."}
                LargeLiteralLabel{text: "O"}
            }
            LargeTextProbeCard{
                LargeTextTitle{text: "8"}
                LargeTextNote{text: "Two counters."}
                LargeLiteralLabel{text: "8"}
            }
            LargeTextProbeCard{
                LargeTextTitle{text: "y"}
                LargeTextNote{text: "A simpler descender."}
                LargeLiteralLabel{text: "y"}
            }
        }

        StoryHeading{text: "A fade in get_color"}
        StoryNote{text: "The same fade in get_color, with the base colour literal or from the theme, on a short word and on a wide glyph."}
        LargeTextProbeGrid{
            LargeTextProbeCard{
                LargeTextTitle{text: "Literal, Ag"}
                LargeTextNote{text: "A literal base colour."}
                LargeCustomLiteralLabel{text: "Ag"}
            }
            LargeTextProbeCard{
                LargeTextTitle{text: "Theme, Ag"}
                LargeTextNote{text: "The base colour from the theme."}
                LargeCustomThemeLabel{text: "Ag"}
            }
            LargeTextProbeCard{
                LargeTextTitle{text: "Literal, W"}
                LargeTextNote{text: "A literal base colour."}
                LargeCustomLiteralLabel{text: "W"}
            }
            LargeTextProbeCard{
                LargeTextTitle{text: "Theme, W"}
                LargeTextNote{text: "The base colour from the theme."}
                LargeCustomThemeLabel{text: "W"}
            }
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "overview/large-text/overview",
    category: "Overview",
    component: "Large text",
    also: &[],
    name: "Overview",
    dsl: "SlugOverview",
    added: "2026-04-16",
    tags: &["ported"],
    doc: "# Large text\n\nText is drawn from the glyph atlas up to the largest size the atlas holds, 128 device pixels to the em, and from each glyph's outline above that size, so large type stays sharp. Some systems draw every size from outlines.\n\nThe page sets the same widget on both sides of that size with nothing else changed: a plain label, a gradient label, a label with its own `get_color`, and a `LinkLabel`. Below them are probes, all of them over the size, that each vary one thing: where the colour comes from, the shape of the glyph, or a custom `get_color`.\n\nIt documents no widget. It is the check that the two ways of drawing text agree.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];

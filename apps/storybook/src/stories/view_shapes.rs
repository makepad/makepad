//! The view shapes gallery: every preset of View the library ships, in one
//! place, because the difference between them is what they draw.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Tile = View{
        width: Fit height: Fit
        flow: Down
        spacing: theme.space_1
        align: Align{x: 0.5}
        caption := Label{text: "" draw_text +: {color: theme.color_text_meta}}
    }

    mod.stories.ViewShapesOverview = StoryPage{
        StoryNote{text: "A View draws nothing by default. These are the presets that draw something: a shape, a shadow, a gradient, or a scroll. Each one below is the same size with the same colour, so what differs is only what the preset itself does."}

        StoryHeading{text: "Shapes"}
        StoryNote{text: "RectView is square, RoundedView rounds every corner by the theme's radius, and the X and Y variants round only one pair — which is what a tab, a sheet or a panel joined to an edge needs."}
        StoryRow{
            Tile{caption: Label{text: "RectView"} RectView{width: 96. height: 56. draw_bg +: {color: theme.color_primary_container}}}
            Tile{caption: Label{text: "RoundedView"} RoundedView{width: 96. height: 56. draw_bg +: {color: theme.color_primary_container}}}
            Tile{caption: Label{text: "RoundedAllView"} RoundedAllView{width: 96. height: 56. draw_bg +: {color: theme.color_primary_container}}}
            Tile{caption: Label{text: "RoundedXView"} RoundedXView{width: 96. height: 56. draw_bg +: {color: theme.color_primary_container}}}
            Tile{caption: Label{text: "RoundedYView"} RoundedYView{width: 96. height: 56. draw_bg +: {color: theme.color_primary_container}}}
        }
        StoryRow{
            Tile{caption: Label{text: "CircleView"} CircleView{width: 56. height: 56. draw_bg +: {color: theme.color_secondary_container}}}
            Tile{caption: Label{text: "HexagonView"} HexagonView{width: 56. height: 56. draw_bg +: {color: theme.color_secondary_container}}}
            Tile{caption: Label{text: "GradientXView"} GradientXView{width: 96. height: 56.}}
            Tile{caption: Label{text: "GradientYView"} GradientYView{width: 96. height: 56.}}
        }

        StoryHeading{text: "Shadows"}
        StoryNote{text: "The same two shapes again, lifted off the page. A shadow view carries its own blur and drop rather than taking one from the elevation tokens."}
        StoryRow{
            Tile{caption: Label{text: "RectShadowView"} RectShadowView{width: 96. height: 56. draw_bg +: {color: theme.color_surface_container_high}}}
            Tile{caption: Label{text: "RoundedShadowView"} RoundedShadowView{width: 96. height: 56. draw_bg +: {color: theme.color_surface_container_high}}}
        }

        StoryHeading{text: "Rules"}
        StoryNote{text: "Hr is a horizontal rule and Vr a vertical one — a view whose whole job is a line."}
        StoryRow{
            View{
                width: 240. height: Fit flow: Down spacing: theme.space_2
                Label{text: "above the rule"}
                Hr{}
                Label{text: "below it"}
            }
            View{
                width: Fit height: 60. flow: Right spacing: theme.space_2
                align: Align{y: 0.5}
                Label{text: "left"}
                Vr{}
                Label{text: "right"}
            }
        }

        StoryHeading{text: "Scrolling, and scrolling that is cached"}
        StoryNote{text: "ScrollXView, ScrollYView and ScrollXYView scroll on one axis or both. The Cached variants draw their contents into a texture once and reuse it while nothing inside changes, which is what a long list of expensive rows wants."}
        StoryRow{
            Tile{
                caption: Label{text: "ScrollXView"}
                View{
                    width: 200. height: 70.
                    show_bg: true
                    draw_bg +: {color: theme.color_surface_container_low}
                    ScrollXView{
                        width: Fill height: Fill
                        flow: Right
                        spacing: theme.space_2
                        padding: theme.mspace_1
                        RectView{width: 90. height: 40. draw_bg +: {color: theme.color_primary_container}}
                        RectView{width: 90. height: 40. draw_bg +: {color: theme.color_secondary_container}}
                        RectView{width: 90. height: 40. draw_bg +: {color: theme.color_tertiary_container}}
                    }
                }
            }
            Tile{
                caption: Label{text: "ScrollYView"}
                View{
                    width: 200. height: 70.
                    show_bg: true
                    draw_bg +: {color: theme.color_surface_container_low}
                    ScrollYView{
                        width: Fill height: Fill
                        flow: Down
                        spacing: theme.space_1
                        padding: theme.mspace_1
                        Label{text: "one"} Label{text: "two"} Label{text: "three"}
                        Label{text: "four"} Label{text: "five"} Label{text: "six"}
                    }
                }
            }
            Tile{
                caption: Label{text: "CachedScrollY"}
                View{
                    width: 200. height: 70.
                    show_bg: true
                    draw_bg +: {color: theme.color_surface_container_low}
                    CachedScrollY{
                        width: Fill height: Fill
                        flow: Down
                        spacing: theme.space_1
                        padding: theme.mspace_1
                        Label{text: "one"} Label{text: "two"} Label{text: "three"}
                        Label{text: "four"} Label{text: "five"} Label{text: "six"}
                    }
                }
            }
        }
        StoryRow{
            Tile{
                caption: Label{text: "CachedRoundedView"}
                CachedRoundedView{
                    width: 140. height: 56.
                    align: Align{x: 0.5, y: 0.5}
                    draw_bg +: {color: theme.color_surface_container_high}
                    Label{text: "drawn once"}
                }
            }
            Tile{
                caption: Label{text: "CachedScrollX"}
                View{
                    width: 200. height: 56.
                    show_bg: true
                    draw_bg +: {color: theme.color_surface_container_low}
                    CachedScrollX{
                        width: Fill height: Fill
                        flow: Right
                        spacing: theme.space_2
                        padding: theme.mspace_1
                        RectView{width: 90. height: 30. draw_bg +: {color: theme.color_primary_container}}
                        RectView{width: 90. height: 30. draw_bg +: {color: theme.color_secondary_container}}
                    }
                }
            }
            Tile{
                caption: Label{text: "CachedScrollXY"}
                View{
                    width: 200. height: 56.
                    show_bg: true
                    draw_bg +: {color: theme.color_surface_container_low}
                    CachedScrollXY{
                        width: Fill height: Fill
                        flow: Right
                        spacing: theme.space_2
                        padding: theme.mspace_1
                        RectView{width: 120. height: 60. draw_bg +: {color: theme.color_tertiary_container}}
                        RectView{width: 120. height: 60. draw_bg +: {color: theme.color_primary_container}}
                    }
                }
            }
        }

        StoryHeading{text: "Rows a list is made of"}
        StoryNote{text: "The Fab presets are the property panel's rows, sized to line up in a dense column. StackViewHeader is the bar a stack navigation puts above its page, and VoiceWave is the level meter one app draws while it is listening."}
        StoryRow{
            View{
                width: 260. height: Fit flow: Down spacing: theme.space_1
                FabSection{Label{text: "FabSection"}}
                FabPropRow{Label{text: "FabPropRow"}}
                FabSearch{}
            }
        }
        StoryRow{
            View{
                width: 300. height: Fit
                StackViewHeader{}
            }
            Tile{caption: Label{text: "VoiceWave"} VoiceWave{width: 160. height: 40.}}
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/viewshapes/overview",
    category: "Containers",
    component: "ViewShapes",
    also: &[
        "RectView", "RoundedAllView", "RoundedXView", "RoundedYView", "HexagonView",
        "GradientXView", "RectShadowView", "RoundedShadowView", "Vr",
        "ScrollXView", "CachedScrollX", "CachedScrollY", "CachedScrollXY", "CachedRoundedView",
        "FabPropRow", "FabSearch", "FabSection", "StackViewHeader", "VoiceWave",
    ],
    name: "Shapes",
    dsl: "ViewShapesOverview",
    added: "2025-05-06",
    tags: &["layout"],
    doc: "# View shapes

A `View` draws nothing at all by default. These are the presets that draw something, gathered here because the only difference between them *is* what they draw — every tile below is the same size in the same colour.

**Shapes.** `RectView` is square-cornered; `RoundedView` and `RoundedAllView` round by the theme's radius; `RoundedXView` and `RoundedYView` round one pair of corners only, which is what a tab, a sheet, or a panel joined to an edge needs. `CircleView` and `HexagonView` are what they say. `GradientXView` and `GradientYView` fill along an axis.

**Shadows.** `RectShadowView` and `RoundedShadowView` carry their own blur and drop rather than taking one from the elevation tokens — reach for `ElevatedView1`–`5` on the elevation page when you want the token scale instead.

**Rules.** `Hr` and `Vr` are views whose entire job is a line.

**Scrolling.** `ScrollXView`, `ScrollYView` and `ScrollXYView` scroll on one axis or both. The `Cached` variants draw their contents into a texture and reuse it while nothing inside changes — worth it for a long run of expensive rows, wasted on anything that redraws every frame anyway.

**Rows.** `FabSection`, `FabPropRow` and `FabSearch` are the property panel's rows, sized to line up in a dense column. `StackViewHeader` is the bar a stack navigation puts above its page, and `VoiceWave` is the level meter one app draws while listening.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];

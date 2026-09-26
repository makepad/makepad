//! The icon story: a tinted icon with a turn under the controls, the faces,
//! the glyphs of the icon font, and the styling reference.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Face = View{
        width: Fit
        height: Fit
        flow: Down
        spacing: theme.space_1
        align: Align{x: 0.5 y: 0.0}
    }

    let Caption = Label{draw_text +: {color: theme.color_text_meta}}

    mod.stories.IconOverview = StoryPage{
        StoryNote{text: "A vector drawing from a file, sized and inked like a glyph so it can sit inside a control. A single-colour icon takes its ink from draw_icon.color, whatever colour the file says; icon_walk sets both sides of its box. IconRotated adds rotation_angle, a turn about the centre of that box."}
        StoryNote{text: "Which one to use, against IconRotated, IconSet and Svg, is set out on the Docs tab."}

        StoryHeading{text: "A turned icon, under the controls"}
        StoryRow{
            rotated := IconRotated{
                icon_walk: Walk{width: 48 height: 48}
                draw_icon +: {svg: crate_resource("makepad_widgets:resources/icons/icon_select.svg") color: #f80 rotation_angle: 1.57}
            }
        }

        StoryHeading{text: "A plain tint"}
        StoryRow{
            Icon{
                icon_walk: Walk{width: 32 height: 32}
                draw_icon +: {svg: crate_resource("makepad_widgets:resources/icons/icon_file.svg") color: #x00a0c0}
            }
            Icon{
                icon_walk: Walk{width: 32 height: 32}
                draw_icon +: {svg: crate_resource("makepad_widgets:resources/icons/icon_select.svg") color: #f80}
            }
        }

        StoryHeading{text: "Turned"}
        StoryNote{text: "The angle is in radians: 0.785 is an eighth of a turn, 1.571 a quarter and 3.142 a half. An angle past a full turn wraps: 7.0 is one turn and about 41 degrees more."}
        StoryRow{
            IconRotated{
                icon_walk: Walk{width: 32 height: 32}
                draw_icon +: {svg: crate_resource("makepad_widgets:resources/icons/icon_select.svg") color: #f80 rotation_angle: 0.0}
            }
            Label{text: "0" draw_text +: {color: theme.color_text_meta}}
            IconRotated{
                icon_walk: Walk{width: 32 height: 32}
                draw_icon +: {svg: crate_resource("makepad_widgets:resources/icons/icon_select.svg") color: #f80 rotation_angle: 0.785}
            }
            Label{text: "0.785" draw_text +: {color: theme.color_text_meta}}
            IconRotated{
                icon_walk: Walk{width: 32 height: 32}
                draw_icon +: {svg: crate_resource("makepad_widgets:resources/icons/icon_select.svg") color: #f80 rotation_angle: 1.571}
            }
            Label{text: "1.571" draw_text +: {color: theme.color_text_meta}}
            IconRotated{
                icon_walk: Walk{width: 32 height: 32}
                draw_icon +: {svg: crate_resource("makepad_widgets:resources/icons/icon_select.svg") color: #f80 rotation_angle: 3.142}
            }
            Label{text: "3.142" draw_text +: {color: theme.color_text_meta}}
        }

        StoryHeading{text: "Faces"}
        StoryNote{text: "The same file through each face. The plain Icon here is inked with the theme's text colour, because the file's own paint is white and vanishes on the light theme's ground. IconGradientX runs draw_icon.color into color_2 from top to bottom and IconGradientY from left to right. IconFilled sets the glyph on a disc in the primary colour, IconLight on a disc in the primary container colour, and IconOutline inside a thin primary ring."}
        StoryRow{
            align: Align{x: 0. y: 0.}
            Face{
                Icon{
                    draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg") color: theme.color_on_surface}
                }
                Caption{text: "Icon"}
            }
            Face{
                IconGradientX{
                    // A fresh Walk{} has a Fill height, and Fill inside this Fit host resolves to nought, so the glyph was scaled to nothing: say Fit.
                    icon_walk: Walk{width: 100. height: Fit}
                    draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
                }
                Caption{text: "IconGradientX"}
            }
            Face{
                IconGradientY{
                    icon_walk: Walk{width: 100. height: Fit}
                    draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
                }
                Caption{text: "IconGradientY"}
            }
            Face{
                IconFilled{draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}}
                Caption{text: "IconFilled"}
            }
            Face{
                IconLight{draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}}
                Caption{text: "IconLight"}
            }
            Face{
                IconOutline{draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}}
                Caption{text: "IconOutline"}
            }
        }

        StoryHeading{text: "Font icons"}
        StoryNote{text: "IconSet is a label set in the theme's icon font rather than a drawing from a file. Its text is the glyph's code point and draw_text.color is its ink. The preset sets a hundred points; these sixteen are set at forty, and wrap with the page instead of running off its right edge."}
        View{
            width: Fill
            height: Fit
            flow: Flow.Right{wrap: true}
            spacing: 16.
            IconSet{text: "\u{f015}" draw_text +: {color: #0ff text_style +: {font_size: 40.}}}
            IconSet{text: "\u{f2bd}" draw_text +: {color: #0ff text_style +: {font_size: 40.}}}
            IconSet{text: "\u{f03e}" draw_text +: {color: #0ff text_style +: {font_size: 40.}}}
            IconSet{text: "\u{f15b}" draw_text +: {color: #0ff text_style +: {font_size: 40.}}}
            IconSet{text: "\u{f030}" draw_text +: {color: #0ff text_style +: {font_size: 40.}}}
            IconSet{text: "\u{f133}" draw_text +: {color: #0ff text_style +: {font_size: 40.}}}
            IconSet{text: "\u{f0c2}" draw_text +: {color: #0ff text_style +: {font_size: 40.}}}
            IconSet{text: "\u{f0d1}" draw_text +: {color: #0ff text_style +: {font_size: 40.}}}
            IconSet{text: "\u{f164}" draw_text +: {color: #0ff text_style +: {font_size: 40.}}}
            IconSet{text: "\u{f118}" draw_text +: {color: #0ff text_style +: {font_size: 40.}}}
            IconSet{text: "\u{f025}" draw_text +: {color: #0ff text_style +: {font_size: 40.}}}
            IconSet{text: "\u{f0f3}" draw_text +: {color: #0ff text_style +: {font_size: 40.}}}
            IconSet{text: "\u{f007}" draw_text +: {color: #0ff text_style +: {font_size: 40.}}}
            IconSet{text: "\u{f075}" draw_text +: {color: #0ff text_style +: {font_size: 40.}}}
            IconSet{text: "\u{f0e0}" draw_text +: {color: #0ff text_style +: {font_size: 40.}}}
            IconSet{text: "\u{f1b9}" draw_text +: {color: #0ff text_style +: {font_size: 40.}}}
        }

        StoryHeading{text: "Styling reference"}
        StoryNote{text: "draw_bg paints the box behind the glyph, icon_walk sizes and spaces the glyph inside it, and draw_icon carries the file and its ink."}
        Icon{
            width: Fit
            height: Fit
            icon_walk: Walk{
                width: 50.
                // Fit, for the same reason as the gradient icons above: without it the red ground was twenty points tall and empty.
                height: Fit
                margin: 10.
            }
            // Plain value: a uniform(..) in this merge redeclares the input instead of setting it, and the red ground never drew.
            draw_bg +: {color: #f00}
            draw_icon +: {
                svg: crate_resource("self:resources/Icon_Favorite.svg")
                color: #f0f
                color_2: #ff0
            }
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "media/icon/overview",
    category: "Media",
    component: "Icon",
    also: &["IconRotated", "IconSet"],
    name: "Overview",
    dsl: "IconOverview",
    added: "2026-02-23",
    tags: &["ported", "rotate", "tint"],
    doc: "# Icon\n\nA vector drawing from a file, sized and inked like a glyph so it can sit inside a control: a background quad, `draw_bg`, with the drawing, `draw_icon`, fitted into `icon_walk` inside it.\n\n## Which one to use\n\n| Want | Use |\n|---|---|\n| a drawing from a file inside a control | `Icon` |\n| the same, turned | `IconRotated` |\n| a glyph out of the icon font | `IconSet` |\n| a drawing on its own, at the widget's own size, or one written in the DSL | `Svg` or `Vector`: Svg > Overview says which |\n\n## Tint\n\nA single-colour icon takes its ink from `draw_icon.color`. Any colour with a non-negative red channel replaces every colour in the file and keeps the file's own alpha, so the shape's edges stay soft; the default of -1 leaves the file's colours alone. `icon_walk` sets both sides of the box the glyph is fitted into. With `height: Fit` the height follows the file's aspect; a fresh `Walk{width: 32}` has a `Fill` height, which is nothing inside a `Fit` row, so write both sides or merge with `icon_walk +:`. With nothing set the widget's own walk is 17.5 wide and `Fit` high.\n\n## Rotation\n\n`IconRotated` adds `rotation_angle`, a uniform in radians. Its `transform_svg_point` hook turns every vertex about the centre of the icon's box with a cos and a sin before it is placed, so a change of angle costs no re-tessellation and the box never grows: a long glyph at a quarter turn is clipped by nothing, but it can reach past its own row.\n\n## Faces\n\n`IconGradientX` runs `draw_icon.color` into `color_2` from top to bottom and `IconGradientY` from left to right; `gradient_fill_horizontal` is the switch between them. `IconFilled` sets the glyph on a disc in the primary colour with its on-colour as the ink, `IconLight` on a disc in the primary container colour, and `IconOutline` inside a one-pixel primary ring. Their `padding` is what grows the disc around the glyph.\n\n## Font icons\n\n`IconSet` is a `Label` set in the theme's icon font. Its `text` is the glyph's code point, written as an escape such as `\\u{f015}`, and `draw_text.color` is its ink. The preset sets the font size to a hundred points and leaves the glyph on the font's own baseline rather than centring its ink, because an icon font's cap height describes a capital nobody is drawing.",
    subject: "rotated",
    feature: None,
    controls: &[
        Control { label: "angle", target: "rotated", kind: ControlKind::Number { prop: "draw_icon.rotation_angle", min: 0.0, max: 6.2832, step: 0.01, default: 1.57 } },
        Control { label: "tint", target: "rotated", kind: ControlKind::Color { prop: "draw_icon.color", default: 0xFF8800FF } },
    ],
    on_actions: None,
}];

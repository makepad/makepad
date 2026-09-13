//! The icon stories: the icon ladder and the styling reference, ported from the widget zoo, and a tinted icon with a turn.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.IconOverview = StoryPage{
        H4{text: "Standard"}
        // The theme's ink: the file's own paint is white, which vanished on the light theme's ground.
        Icon{
            draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg") color: theme.color_on_surface}
        }

        Hr{}
        H4{text: "IconGradientX"}
        IconGradientX{
            // A fresh Walk{} has a Fill height, and Fill inside this Fit host resolves to nought, so the glyph was scaled to nothing: say Fit.
            icon_walk: Walk{width: 100. height: Fit}
            draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
        }

        Hr{}
        H4{text: "IconGradientY"}
        IconGradientY{
            icon_walk: Walk{width: 100. height: Fit}
            draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
        }

        Hr{}
        H4{text: "Weights and rotation"}
        View{
            width: Fit height: Fit flow: Right spacing: 12.
            align: Align{y: 0.5}
            IconFilled{draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}}
            IconLight{draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}}
            IconOutline{draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}}
            IconRotated{draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg") color: theme.color_on_surface rotation_angle: 0.785}}
        }

        H4{text: "Styling Attributes Reference"}
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

    /** A single-colour icon inked through draw_icon.color, and the same
     * icon turned by IconRotated's rotation_angle. */
    mod.stories.IconTintRotation = StoryPage{
        StoryNote{text: "A single-colour icon takes its ink from draw_icon.color, whatever colour the file says; icon_walk sets both sides of its box. IconRotated adds rotation_angle, a turn about the centre of that box."}

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

        StoryHeading{text: "Drive it"}
        StoryRow{
            rotated := IconRotated{
                icon_walk: Walk{width: 48 height: 48}
                draw_icon +: {svg: crate_resource("makepad_widgets:resources/icons/icon_select.svg") color: #f80 rotation_angle: 1.57}
            }
        }
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "media/icon/overview",
        category: "Media",
        component: "Icon",
        also: &[],
        name: "Overview",
        dsl: "IconOverview",
        added: "2026-02-23",
        tags: &["ported"],
        doc: "# Icon\n\nIcons display SVG vector graphics.",
        subject: "",
        feature: None,
        controls: &[],
        on_actions: None,
    },
    Story {
        key: "media/icon/tint-and-rotation",
        category: "Media",
        component: "Icon",
        also: &["IconRotated"],
        name: "Tint and rotation",
        dsl: "IconTintRotation",
        added: "2026-02-23",
        tags: &["ported", "rotate", "tint"],
        doc: "# Icon\n\n## Tint\n\nA single-colour icon takes its ink from `draw_icon.color`. Any colour with a non-negative red channel replaces every colour in the file and keeps the file's own alpha, so the shape's edges stay soft; the default of -1 leaves the file's colours alone. `icon_walk` sets both sides of the box the glyph is fitted into. With `height: Fit` the height follows the file's aspect; a fresh `Walk{width: 32}` has a `Fill` height, which is nothing inside a `Fit` row, so write both sides or merge with `icon_walk +:`. With nothing set the widget's own walk is 17.5 wide and `Fit` high.\n\n## Rotation\n\n`IconRotated` adds `rotation_angle`, a uniform in radians. Its `transform_svg_point` hook turns every vertex about the centre of the icon's box with a cos and a sin before it is placed, so a change of angle costs no re-tessellation and the box never grows: a long glyph at a quarter turn is clipped by nothing, but it can reach past its own row.",
        subject: "rotated",
        feature: None,
        controls: &[
            Control { label: "angle", target: "rotated", kind: ControlKind::Number { prop: "draw_icon.rotation_angle", min: 0.0, max: 6.2832, step: 0.01, default: 1.57 } },
            Control { label: "tint", target: "rotated", kind: ControlKind::Color { prop: "draw_icon.color", default: 0xFF8800FF } },
        ],
        on_actions: None,
    },
];

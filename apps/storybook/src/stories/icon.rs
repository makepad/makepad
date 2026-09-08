//! The icon stories: the icon ladder and the styling reference, ported from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.IconOverview = StoryPage{
        H4{text: "Standard"}
        Icon{
            draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
        }

        Hr{}
        H4{text: "IconGradientX"}
        IconGradientX{
            icon_walk: Walk{width: 100.}
            draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
        }

        Hr{}
        H4{text: "IconGradientY"}
        IconGradientY{
            icon_walk: Walk{width: 100.}
            draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
        }

        H4{text: "Styling Attributes Reference"}
        Icon{
            width: Fit
            height: Fit
            icon_walk: Walk{
                width: 50.
                margin: 10.
            }
            draw_bg +: {color: uniform(#f00)}
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
}];

//! The link label stories: the standard, disabled and fully styled links, ported from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.LinkLabelOverview = StoryPage{
        H4{text: "Standard"}
        StoryRow{
            LinkLabel{text: "Click me!"}
        }

        Hr{}
        H4{text: "Standard, disabled"}
        StoryRow{
            LinkLabel{
                text: "Click me!"
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }
        }

        Hr{}
        H4{text: "Styling Attributes Reference"}
        StoryRow{
            LinkLabel{
                draw_text +: {
                    color: #xA
                    color_hover: #xC
                    color_down: #8
                    text_style +: {
                        font_size: 20.
                        line_spacing: 1.4
                    }

                }

                draw_bg +: {
                    color: uniform(#x0A0)
                    color_hover: uniform(#x0C0)
                    color_down: uniform(#080)
                }

                icon_walk: Walk{
                    width: 20.
                    height: Fit
                }

                draw_icon +: {
                    color: #xA00
                    color_hover: #xC00
                    color_down: #800
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                }

                text: "Click me!"
            }
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "actions/linklabel/overview",
    category: "Actions",
    component: "LinkLabel",
    also: &[],
    name: "Overview",
    dsl: "LinkLabelOverview",
    added: "2026-02-23",
    tags: &["ported"],
    doc: "# LinkLabel\n\nLinkLabels are clickable text links.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];

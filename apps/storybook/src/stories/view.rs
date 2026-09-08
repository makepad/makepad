//! The view stories: the plain view, the styled view templates, the scroll views and the cached view, ported from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ViewOverview = StoryPage{
        H4{text: "View"}
        View{
            width: Fit height: Fit
            padding: theme.mspace_2
            align: Align{x: 0.5 y: 0.5}
            Label{text: "View"}
        }

        Hr{}
        H4{text: "Style Templates"}
        StoryRow{
            height: Fit,
            flow: Right
            spacing: 10

            SolidView{
                width: Fit height: Fit
                padding: theme.mspace_2
                align: Align{x: 0.5 y: 0.5}
                draw_bg +: {color: #F00}
                Label{text: "SolidView"}
            }

            RoundedView{
                width: Fit height: Fit
                padding: theme.mspace_2
                align: Align{x: 0.5 y: 0.5}
                draw_bg +: {
                    color: #F00
                    border_radius: 5.0
                    border_size: 2.0
                    border_color: #FFF
                }
                Label{text: "RoundedView"}
            }

            CircleView {
                width: Fit height: Fit
                padding: 0
                align: Align{x: 0.5 y: 0.5}
                draw_bg +: {
                    color: #F00
                    border_size: 2.0
                    border_color: #FFF
                }
                Label{
                    align: Align{x: 0.5 y: 0.5}
                    text: "CircleView\nFit"
                }
            }

            CircleView {
                width: Fit height: Fit
                padding: 30
                align: Align{x: 0.5 y: 0.5}
                draw_bg +: {
                    color: #F00
                    border_size: 2.0
                    border_color: #FFF
                }
                Label{
                    align: Align{x: 0.5 y: 0.5}
                    text: "CircleView\nFit Pad 30"
                }
            }

            CircleView {
                width: 60 height: 60
                padding: 0
                align: Align{x: 0.5 y: 0.5}
                draw_bg +: {
                    color: #F00
                    border_size: 1.0
                    border_color: #00F
                }
                Label{
                    width: Fill
                    align: Align{x: 0.5 y: 0.5}
                    text: "Circle\n60x60"
                }
            }

            ScrollXYView{
                width: 100 height: 100
                padding: theme.mspace_2
                align: Align{x: 0. y: 0.}
                show_bg: true
                draw_bg +: {
                    color: uniform(theme.color_inset)
                }
                View{
                    width: 400. height: 400.
                    flow: Down
                    show_bg: true
                    draw_bg +: {color: uniform(theme.color_inset)}
                    Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                    Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                    Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                    Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                    Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                    Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                    Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                    Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                    Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                    Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                    Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                    Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                    Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                }
            }

            ScrollYView{
                width: 100 height: 100
                padding: theme.mspace_2
                align: Align{x: 0. y: 0.}
                show_bg: true
                draw_bg +: {
                    color: uniform(theme.color_inset)
                }
                View{
                    width: 400. height: 400.
                    flow: Down
                    show_bg: true
                    draw_bg +: {color: uniform(theme.color_inset)}
                    Label{text: "ScrollYView ScrollYView ScrollYView"}
                    Label{text: "ScrollYView ScrollYView ScrollYView"}
                    Label{text: "ScrollYView ScrollYView ScrollYView"}
                    Label{text: "ScrollYView ScrollYView ScrollYView"}
                    Label{text: "ScrollYView ScrollYView ScrollYView"}
                    Label{text: "ScrollYView ScrollYView ScrollYView"}
                    Label{text: "ScrollYView ScrollYView ScrollYView"}
                    Label{text: "ScrollYView ScrollYView ScrollYView"}
                    Label{text: "ScrollYView ScrollYView ScrollYView"}
                    Label{text: "ScrollYView ScrollYView ScrollYView"}
                    Label{text: "ScrollYView ScrollYView ScrollYView"}
                    Label{text: "ScrollYView ScrollYView ScrollYView"}
                    Label{text: "ScrollYView ScrollYView ScrollYView"}
                }
            }
        }

        Hr{}
        H4{text: "Special functions"}
        StoryRow{
            CachedView{
                width: Fit height: Fit
                padding: theme.mspace_2
                align: Align{x: 0.5 y: 0.5}
                View{
                    width: Fit height: Fit
                    show_bg: true
                    draw_bg +: {color: uniform(theme.color_inset)}
                    Label{text: "CachedView"}
                }
            }
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/view/overview",
    category: "Containers",
    component: "View",
    also: &[],
    name: "Overview",
    dsl: "ViewOverview",
    added: "2026-02-25",
    tags: &["ported"],
    doc: "# View\n\nViews are the basic layout containers.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];

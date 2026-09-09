//! The scroll bar stories: a tall gradient the page scrolls through with draggable bars, ported from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ScrollBarOverview = StoryPage{
        GradientYView{
            height: 4000.
            width: Fill
            draw_bg +: {
                color_2: uniform(#f00)
            }
        }
        scroll_bars: ScrollBars{
            scroll_bar_x.drag_scrolling: true
            scroll_bar_y.drag_scrolling: true
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/scrollbar/overview",
    category: "Containers",
    component: "ScrollBar",
    also: &["GradientYView"],
    name: "Overview",
    dsl: "ScrollBarOverview",
    added: "2026-04-16",
    tags: &["ported"],
    doc: "# ScrollBar\n\nScrollBars enable scrolling through content.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];

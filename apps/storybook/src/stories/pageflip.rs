//! The page flip stories: three buttons switching three pages, the handlers on the story root, ported from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.PageFlipOverview = StoryPage{
        View{
            height: Fit width: Fill
            flow: Right
            spacing: theme.space_2
            pageflipbutton_a := Button{text: "Page A"}
            pageflipbutton_b := Button{text: "Page B"}
            pageflipbutton_c := Button{text: "Page C"}
        }

        page_flip := PageFlip{
            width: Fill height: Fill
            flow: Down
            active_page: @page_a

            page_a := View{
                align: Align{x: 0.5 y: 0.5}
                show_bg: true
                draw_bg +: {color: uniform(#f00)}
                width: Fill height: Fill
                H3{width: Fit text: "Page A"}
            }

            page_b := View{
                align: Align{x: 0.5 y: 0.5}
                show_bg: true
                draw_bg +: {color: uniform(#080)}
                width: Fill height: Fill
                H3{width: Fit text: "Page B"}
            }

            page_c := View{
                align: Align{x: 0.5 y: 0.5}
                show_bg: true
                draw_bg +: {color: uniform(#008)}
                width: Fill height: Fill
                H3{width: Fit text: "Page C"}
            }
        }
    }
}

fn overview_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if root.button(cx, ids!(pageflipbutton_a)).clicked(actions) {
        root.page_flip(cx, ids!(page_flip)).set_active_page(cx, live_id!(page_a));
    }
    if root.button(cx, ids!(pageflipbutton_b)).clicked(actions) {
        root.page_flip(cx, ids!(page_flip)).set_active_page(cx, live_id!(page_b));
    }
    if root.button(cx, ids!(pageflipbutton_c)).clicked(actions) {
        root.page_flip(cx, ids!(page_flip)).set_active_page(cx, live_id!(page_c));
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "navigation/pageflip/overview",
    category: "Navigation",
    component: "PageFlip",
    name: "Overview",
    dsl: "PageFlipOverview",
    added: "2026-02-16",
    tags: &["ported"],
    doc: "# PageFlip\n\nPageFlip switches between pages.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: Some(overview_actions),
}];

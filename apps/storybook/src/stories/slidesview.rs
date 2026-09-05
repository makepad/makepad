//! The slides view stories: a chapter slide and a second slide driven by the cursor keys, ported from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.SlidesViewOverview = StoryPage{
        SlidesView{
            width: Fill height: Fill

            SlideChapter{
                title := H1{text: "Hey!"}
                SlideBody{text: "This is the 1st slide. Use your right\ncursor key to show the next slide."}
            }

            Slide{
                title := H1{text: "Second slide"}
                SlideBody{text: "This is the 2nd slide. Use your left\ncursor key to show the previous slide."}
            }
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "navigation/slidesview/overview",
    category: "Navigation",
    component: "SlidesView",
    name: "Overview",
    dsl: "SlidesViewOverview",
    added: "2026-02-16",
    tags: &["ported"],
    doc: "# SlidesView\n\nSlidesView displays presentation slides.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];

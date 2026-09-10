//! The slides view stories: a chapter slide and a second slide driven by the cursor keys, ported from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.SlidesViewOverview = StoryPage{
        StoryNote{text: "A deck: one slide fills the space and the arrow keys move between them. Press it first — it takes the keyboard on a press, so the arrows go to the deck rather than scrolling the page behind it."}

        StoryHeading{text: "A deck of two"}
        StoryRow{
            width: Fill
            // A stated height. The page scrolls, so `height: Fill` inside
            // it resolves to nothing and the whole deck was laid out and
            // never painted.
            SlidesView{
                width: Fill
                height: 320.

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

        StoryNote{text: "A chapter slide is the same widget with a heavier ground, for the title of a section. Nothing here paginates or animates between decks: a deck is one slide at a time and the transition is the widget's own."}
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "navigation/slidesview/overview",
    category: "Navigation",
    component: "SlidesView",
    also: &["Slide", "SlideBody", "SlideChapter"],
    name: "Overview",
    dsl: "SlidesViewOverview",
    added: "2026-02-16",
    tags: &["ported"],
    doc: "# SlidesView\n\nA deck. One slide fills the space it is given and the arrow keys move between them; nothing else is on screen, which is the point of a slide.\n\nIt takes the keyboard on a press, so the arrows reach the deck rather than scrolling whatever is behind it. A deck given `height: Fill` inside a scrolling page resolves to no height at all and is laid out without ever being painted — give it a real height.\n\n`SlideChapter` is the same widget with a heavier ground, for the title of a section. `SlideBody` is the text under a slide title.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];

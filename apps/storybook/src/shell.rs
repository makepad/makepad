//! The two script namespaces every story file relies on and the page
//! templates stories are built from.
//!
//! `mod.stories` holds one template per story, looked up by name at runtime
//! by the canvas. `mod.storybook` holds the shared building blocks so a story
//! file can say `use mod.storybook.*` and write `StoryPage{ StoryRow{ ... } }`.
//! This module registers first; the story files come after it and the app
//! shell last, because a block's `use` only sees what exists when it runs.
use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.storybook = {}
    mod.stories = {}

    /** A story's page: a scrolling column the story fills top to bottom. */
    mod.storybook.StoryPage = View{
        width: Fill
        height: Fill
        flow: Down
        spacing: theme.space_2
        padding: theme.mspace_3
        scroll_bars: ScrollBars{
            show_scroll_x: false
            show_scroll_y: true
            scroll_bar_y.drag_scrolling: true
        }
    }

    /** A row of instances, laid out left to right and centred vertically. */
    mod.storybook.StoryRow = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
        align: Align{x: 0. y: 0.5}
    }

    /** A section heading inside a story page. */
    mod.storybook.StoryHeading = H4{}

    /** A line of explanation under a heading. */
    mod.storybook.StoryNote = P{}
}

//! The rotated image story: the zoo's note that the widget is not in the new widget system, ported as it stands.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.RotatedImageOverview = StoryPage{
        H4{text: "RotatedImage"}
        P{text: "RotatedImage widget is not available in the new widget system."}
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "media/rotatedimage/overview",
    category: "Media",
    component: "RotatedImage",
    also: &[],
    name: "Overview",
    dsl: "RotatedImageOverview",
    added: "2026-02-16",
    tags: &["ported"],
    doc: "# RotatedImage\n\nRotatedImage displays rotated images.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];

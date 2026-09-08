//! The video stories: a network video at the zoo's size, idle until played, ported from the widget zoo.
//!
//! The zoo streams a sample clip from a third-party address. The story
//! points the same widget at a placeholder address and does not decode an
//! idle thumbnail, so nothing is fetched until play is pressed.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.VideoOverview = StoryPage{
        H4{text: "Network Video (autoplay, looping)"}
        Video{
            source: VideoDataSource.Network { url: "https://example.com/sample_360p_10s.mp4"}
            height: 240
            width: 426
            show_idle_thumbnail: false
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "media/video/overview",
    category: "Media",
    component: "Video",
    also: &[],
    name: "Overview",
    dsl: "VideoOverview",
    added: "2026-07-23",
    tags: &["ported"],
    doc: "# Video\n\nVideo widget for hardware-accelerated video playback.\n\nThe zoo streams a sample clip from a third-party address. This story keeps the widget and its size but points it at a placeholder address, so it stays idle until play is pressed and then shows the widget's error state.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];

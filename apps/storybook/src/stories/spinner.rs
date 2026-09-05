//! The spinner stories: the loading spinner, ported from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.SpinnerOverview = StoryPage{
        H4{text: "Default"}
        LoadingSpinner{}
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "feedback/spinner/overview",
    category: "Feedback",
    component: "Spinner",
    name: "Overview",
    dsl: "SpinnerOverview",
    added: "2026-02-16",
    tags: &["ported"],
    doc: "# Spinner\n\nA loading spinner shows that something is in progress.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];

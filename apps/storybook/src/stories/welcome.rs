use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.Welcome = StoryPage{
        align: Align{x: 0.5 y: 0.5}
        ScrollYView{
            flow: Down
            width: 480.
            height: Fill
            align: Align{x: 0.0 y: 0.4}
            spacing: theme.space_3

            H4{text: "Makepad is an open-source, cross-platform UI framework written in and for Rust. It runs natively and on the web, on every major desktop and mobile platform."}
            P{text: "Its shader-based architecture draws every widget on the GPU, which is what makes it fast enough for dense tools, media applications and 3D or immersive work."}
            P{text: "Live styling reflects UI code changes without recompiling or restarting, so designers and developers work in the same loop."}
            P{text: "This catalogue lists every component of the widget library. Pick a story on the left; the panels on the right document it, let you drive its properties and show what it raised. A green dot marks a story added on or after the baseline date in the settings."}
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "overview/welcome/welcome",
    category: "Overview",
    component: "Welcome",
    name: "Welcome",
    dsl: "Welcome",
    added: "2025-06-01",
    tags: &["ported"],
    doc: "# Welcome\n\nWhat this catalogue is and how to read it.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];

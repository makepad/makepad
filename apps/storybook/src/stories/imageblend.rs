//! The image blend stories: two images and a button that crossfades between them, ported from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ImageBlendOverview = StoryPage{
        H4{text: "Standard"}
        blendbutton := Button{text: "Blend Image"}

        blendimage := ImageBlend{
            align: Align{x: 0.0 y: 0.0}
            image_a +: {
                src: crate_resource("self:resources/ducky.png")
                fit: ImageFit.Smallest
                width: Fill
                height: Fill
            }
            image_b +: {
                src: crate_resource("self:resources/photo_landscape.jpg")
                fit: ImageFit.Smallest
                width: Fill
                height: Fill
            }
        }
    }
}

fn overview_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    if root.button(cx, ids!(blendbutton)).clicked(actions) {
        root.image_blend(cx, ids!(blendimage)).switch_image(cx);
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "media/imageblend/overview",
    category: "Media",
    component: "ImageBlend",
    name: "Overview",
    dsl: "ImageBlendOverview",
    added: "2026-02-16",
    tags: &["ported"],
    doc: "# ImageBlend\n\nImageBlend blends between two images.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: Some(overview_actions),
}];

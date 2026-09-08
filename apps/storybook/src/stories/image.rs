//! The image stories: one bitmap under every fit mode, ported from the widget zoo.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.ImageOverview = StoryPage{
        H4{text: "Default"}
        View{
            show_bg: true draw_bg +: {color: uniform(theme.color_inset_1)} width: Fill height: 150 flow: Down
            Image{src: crate_resource("self:resources/ducky.png")}
        }

        Hr{}
        H4{text: "fit: Stretch"}
        View{
            show_bg: true draw_bg +: {color: uniform(theme.color_inset_1)} width: Fill height: 150
            Image{width: Fill height: Fill src: crate_resource("self:resources/ducky.png") fit: ImageFit.Stretch}
        }

        Hr{}
        H4{text: "fit: Horizontal"}
        View{
            show_bg: true draw_bg +: {color: uniform(theme.color_inset_1)} width: Fill height: 150
            Image{width: Fill height: Fill src: crate_resource("self:resources/ducky.png") fit: ImageFit.Horizontal}
        }

        Hr{}
        H4{text: "fit: Vertical"}
        View{
            show_bg: true draw_bg +: {color: uniform(theme.color_inset_1)} width: Fill height: 150
            Image{width: Fill height: Fill src: crate_resource("self:resources/ducky.png") fit: ImageFit.Vertical}
        }

        Hr{}
        H4{text: "fit: Smallest"}
        View{
            show_bg: true draw_bg +: {color: uniform(theme.color_inset_1)} width: Fill height: 150
            Image{width: Fill height: Fill src: crate_resource("self:resources/ducky.png") fit: ImageFit.Smallest}
        }

        Hr{}
        H4{text: "fit: Biggest"}
        View{
            show_bg: true draw_bg +: {color: uniform(theme.color_inset_1)} width: Fill height: 150
            Image{width: Fill height: Fill src: crate_resource("self:resources/ducky.png") fit: ImageFit.Biggest}
        }

        Hr{}
        H4{text: "fit: CropToFill"}
        View{
            show_bg: true draw_bg +: {color: uniform(theme.color_inset_1)} width: Fill height: 150
            Image{width: Fill height: Fill src: crate_resource("self:resources/ducky.png") fit: ImageFit.CropToFill}
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "media/image/overview",
    category: "Media",
    component: "Image",
    also: &[],
    name: "Overview",
    dsl: "ImageOverview",
    added: "2026-06-04",
    tags: &["ported"],
    doc: "# Image\n\nImages display bitmap content.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];

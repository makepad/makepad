use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoRotatedImage = <UIZooTabLayout_B> {
        desc = {
            <H3> { text: "<RotatedImage>"}
        }
        demos = {
            <RotatedImage> {
                width: Fill, height: Fill,
                draw_bg: {
                    scale: 1., 
                    rotation: 45.
                    opacity: .25
                }

                source: dep("crate://self/resources/ducky.png"),
            }
        }
    }
}
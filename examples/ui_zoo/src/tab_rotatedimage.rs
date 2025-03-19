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
                width: 200, height: 200,
                draw_bg: {
                    scale: .75, 
                    rotation: 45.
                    opacity: .5
                }
                source: dep("crate://self/resources/ducky.png"),
            }

            // <Modal> {
            //     content: {
            //         show_bg: true,
            //         draw_bg: {color: #f00},
            //         <H3> { text: "hallo" }
            //     }
            // }
            <H4> { text: "Standard" }
        }
    }
}
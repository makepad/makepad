use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoImageBlend = <UIZooTabLayout_B> {
        desc = {
            <H3> { text: "<ImageBlend>"}
        }
        demos = {
            <H4> { text: "Standard" }
            blendbutton = <Button> { text: "Blend Image"}

            blendimage = <ImageBlend> {
                align: { x: 0.0, y: 0.0 }
                image_a: {
                    source: dep("crate://self/resources/ducky.png"),
                    fit: Smallest
                    width: Fill,
                    height: Fill
                }
                image_b: {
                    source: dep("crate://self/resources/ismael-jean-deGBOI6yQv4-unsplash.jpg")
                    fit: Smallest
                    width: Fill,
                    height: Fill
                }
            
            }
        }
    }
}
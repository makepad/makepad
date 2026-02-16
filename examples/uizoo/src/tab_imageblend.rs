use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoImageBlend = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# ImageBlend\n\nImageBlend blends between two images."}
        }
        demos +: {
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
                    src: crate_resource("self:resources/ismael-jean-deGBOI6yQv4-unsplash.jpg")
                    fit: ImageFit.Smallest
                    width: Fill
                    height: Fill
                }
            }
        }
    }
}

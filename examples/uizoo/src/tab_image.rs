use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoImage = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# Image\n\nImages display bitmap content."}
        }
        demos +: {
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
        }
    }
}

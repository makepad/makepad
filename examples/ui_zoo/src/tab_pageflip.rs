use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoPageFlip = <UIZooTabLayout_B> {
        desc = {
            <H3> { text: "<PageFlip>"}
        }
        demos = {
            <View> {
                height: Fit, width: Fill,
                flow: Right,
                spacing: 10.
                pageflipbutton_a = <Button> { text: "Page A" }
                pageflipbutton_b = <Button> { text: "Page B" }
                pageflipbutton_c = <Button> { text: "Page C" }
            }

            page_flip = <PageFlip> {
                width: Fill, height: Fill,
                flow: Down

                active_page: page_a 

                page_a = <View> {
                    show_bg: true,
                    draw_bg: { color: #f00 }
                    width: Fill, height: Fill,
                    <H3> { text: "Page A"}
                }

                page_b = <View> {
                    show_bg: true,
                    draw_bg: { color: #080 }
                    width: Fill, height: Fill,
                    <H3> { text: "Page B"}
                }

                page_c = <View> {
                    show_bg: true,
                    draw_bg: { color: #008 }
                    width: Fill, height: Fill,
                    <H3> { text: "Page C"}
                }
            }

        }
    }
}
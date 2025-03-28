use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoSlidesView = <UIZooTabLayout_B> {
        desc = {
            <H3> { text: "<SlidesView>"}
        }
        demos = {
            <SlidesView> {
                width: Fill, height: Fill,

                <SlideChapter> {
                    title = {text: "Hey!"},
                    <SlideBody> {text: "This is the 1st slide. Use your right\ncursor key to show the next slide."}
                }

                <Slide> {
                    title = {text: "Second slide"},
                    <SlideBody> {text: "This is the 2nd slide. Use your left\ncursor key to show the previous slide."}
                }

            }
        }
    }
}
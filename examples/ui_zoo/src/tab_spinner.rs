use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoSpinner = <UIZooTabLayout_B> {
        desc = {
            // <Markdown> { body: dep("crate://self/resources/image.md") } 
        }
        demos = {
            <H4> { text: "Default" }
            <LoadingSpinner> {}
        }
    }
}
use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoExpandablePanel = <UIZooTabLayout_B> {
        desc = {
            <H3> { text: "<ExpandablePanel>"}
        }
        demos = {
            // <Modal> {
            //     content: {
            //         show_bg: true,
            //         draw_bg: {color: #f00},
            //         <H3> { text: "hallo" }
            //     }
            // }
        }
    }
}
use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoFoldButton = <UIZooTabLayout_B> {
        desc = {
            <H3> { text: "<FoldButton>"}
        }
        demos = {
            <H4> { text: "Standard" }
            <FoldButton> { }
        }
    }
}
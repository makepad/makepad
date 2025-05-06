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
            <Markdown> { body: dep("crate://self/resources/foldbutton.md") } 
        }
        demos = {
            <H4> { text: "Standard" }
            <FoldButton> { }
        }
    }
}
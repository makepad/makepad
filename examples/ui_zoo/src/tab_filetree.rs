use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use makepad_example_ui_zoo::demofiletree::*;
    use crate::layout_templates::*;

    pub DemoFT = <UIZooTabLayout_B> {
        desc = {
            <Markdown> { body: dep("crate://self/resources/filetree.md") } 
        }
        demos = {
            <DemoFileTree> { file_tree:{ width: Fill, height: Fill } }
        }
    }
}
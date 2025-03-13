use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoTooltip = <UIZooTabLayout_B> {
        desc = {
            <H3> { text: "<Tooltip>"}
        }
        demos = {
            <H4> { text: "Default", width: 175.}
            <TextInput> { }
        }
    }
}
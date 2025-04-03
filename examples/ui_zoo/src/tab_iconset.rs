use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoIconSet = <UIZooTabLayout_B> {
        desc = {
            <H3> { text: "<Icon>"}
        }
        demos = {
            // <IconSet> { text: "Car" }
        }
    }
}
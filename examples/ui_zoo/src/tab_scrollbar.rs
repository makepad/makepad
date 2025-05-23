use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoScrollBar = <UIZooTabLayout_B> {
        desc = {
            <Markdown> { body: dep("crate://self/resources/scrollbar.md") } 
        }
        demos = {
            <GradientYView> {
                height: 4000.
                width: Fill,
            }
            scroll_bars: <ScrollBars> { }
        }
    }
}
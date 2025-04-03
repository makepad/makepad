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
            <H3> { text: "<ScrollBar>"}
        }
        demos = {
            <H1> { text: "Just some random Text to trigger the Scrollbar widget to show up. Just some random Text to trigger the Scrollbar widget to show up. Just some random Text to trigger the Scrollbar widget to show up. Just some random Text to trigger the Scrollbar widget to show up. Just some random Text to trigger the Scrollbar widget to show up. Just some random Text to trigger the Scrollbar widget to show up. Just some random Text to trigger the Scrollbar widget to show up. Just some random Text to trigger the Scrollbar widget to show up. Just some random Text to trigger the Scrollbar widget to show up."}
            scroll_bars: <ScrollBars> { }
        }
    }
}
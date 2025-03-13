
use crate::{
    makepad_widgets::*,
};


live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;

    pub UIZooTabLayout_B = <View> {
        height: Fill, width: Fill
        flow: Right,
        padding: 0
        spacing: 0.

        desc = <View> {
            width: 350., height: Fill,
            margin: { bottom: 10.}
            flow: Down,
            spacing: (THEME_SPACE_2)
            padding: <THEME_MSPACE_3> {}
            scroll_bars: <ScrollBars> {show_scroll_x: false, show_scroll_y: true}
        }

        <Vr> {}

        demos = <View> {
            width: Fill, height: Fill,
            flow: Down,
            spacing: (THEME_SPACE_2)
            padding: <THEME_MSPACE_3> {}
            scroll_bars: <ScrollBars> {show_scroll_x: false, show_scroll_y: true}
        }

    }

}
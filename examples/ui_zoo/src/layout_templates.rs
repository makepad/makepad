
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

        desc = <RoundedView> {
            width: 350., height: Fill,
            show_bg: true,
            draw_bg: { color: (THEME_COLOR_D_1) }
            padding: <THEME_MSPACE_3> {}
            margin: <THEME_MSPACE_V_2> {}

            flow: Down,
            spacing: (THEME_SPACE_2)
            scroll_bars: <ScrollBars> {show_scroll_x: false, show_scroll_y: true}
        }

        demos = <View> {
            width: Fill, height: Fill,
            flow: Down,
            spacing: (THEME_SPACE_2)
            padding: <THEME_MSPACE_3> {}
            margin: <THEME_MSPACE_V_2> {}
            scroll_bars: <ScrollBars> {show_scroll_x: false, show_scroll_y: true}
        }

    }
    pub UIZooRowH = <View> {
        height: Fit, width: Fill,
        spacing: (THEME_SPACE_2)
        flow: Right,
        align: { x: 0., y: 0.5 }
    }

}
use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoImage = <UIZooTabLayout_B> {
        desc = {
            <H3> { text: "<Image>"}
        }
        demos = {
            flow: Right,

            <View> {
                width: Fit, height: Fit, flow: Down,
                <View> {
                    show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: 125, height: 250, flow: Down,
                    <Image> { source: dep("crate://self/resources/ducky.png" ) }
                }
                <P> { text: "Default" }
            }
            <View> {
                width: Fit, height: Fit, flow: Down,
                <View> {
                    show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: 125, height: 250,
                    <Image> { height: Fill, source: dep("crate://self/resources/ducky.png" ), min_height: 100 }
                }
                <P> { text: "min_height: 100" } // TODO: get this to work correctly
            }
            <View> {
                width: Fit, height: Fit, flow: Down,
                <View> {
                    show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: 125, height: 250,
                    <Image> { width: Fill, source: dep("crate://self/resources/ducky.png" ), width_scale: 1.1 }
                }
                <P> { text: "width_scale: 1.5" } // TODO: get this to work correctly
            }
            <View> {
                width: Fit, height: Fit, flow: Down,
                <View> {
                    show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: 125, height: 250,
                    <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png"), fit: Stretch }
                }
                <P> { text: "fit: Stretch" }
            }
            <View> {
                width: Fit, height: Fit, flow: Down,
                <View> {
                    show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: 125, height: 250,
                    <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png" ), fit: Horizontal }
                }
                <P> { text: "fit: Horizontal" }
            }
            <View> {
                width: Fit, height: Fit, flow: Down,
                <View> {
                    show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: 125, height: 250,
                    <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png" ), fit: Vertical }
                }
                <P> { text: "fit: Vertical" }
            }
            <View> {
                width: Fit, height: Fit, flow: Down,
                <View> {
                    show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: 125, height: 250,
                    <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png" ), fit: Smallest }
                }
                <P> { text: "fit: Smallest" }
            }
            <View> {
                width: Fit, height: Fit, flow: Down,
                <View> {
                    show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: 125, height: 250,
                    <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png" ), fit: Biggest }
                }
                <P> { text: "fit: Biggest" }
            }
        }
    }
}
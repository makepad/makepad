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
            <Markdown> { body: dep("crate://self/resources/image.md") } 
        }
        demos = {
            <H4> { text: "Default" }
            <View> {
                show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: Fill, height: 150, flow: Down,
                <Image> { source: dep("crate://self/resources/ducky.png" ) }
            }

            <Hr> {}
            <H4> { text: "min_height" }
            <View> {
                show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: Fill, height: 150,
                <Image> { height: Fill, source: dep("crate://self/resources/ducky.png" ), min_height: 100 }
            }

            <Hr> {}
            <H4> { text: "width_scale" }
            <View> {
                show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: Fill, height: 150,
                <Image> { width: Fill, source: dep("crate://self/resources/ducky.png" ), width_scale: 1.1 }
            }

            <Hr> {}
            <H4> { text: "fit: Stretch" }
            <View> {
                show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: Fill, height: 150,
                <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png"), fit: Stretch }
            }


            <Hr> {}
            <H4> { text: "fit: Horizontal" }
            <View> {
                show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: Fill, height: 150,
                <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png" ), fit: Horizontal }
            }

            <Hr> {}
            <H4> { text: "fit: Vertical" }
            <View> {
                show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: Fill, height: 150,
                <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png" ), fit: Vertical }
            }

            <Hr> {}
            <H4> { text: "fit: Smallest" }
            <View> {
                show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: Fill, height: 150,
                <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png" ), fit: Smallest }
            }
            
            <Hr> {}
            <H4> { text: "fit: Biggest" }
            <View> {
                show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: Fill, height: 150,
                <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png" ), fit: Biggest }
            }

        }
    }
}
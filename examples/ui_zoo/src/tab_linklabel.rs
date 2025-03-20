use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoLinkLabel = <UIZooTabLayout_B> {
        desc = {
            <H3> { text: "<LinkLabel>"}
        }
        demos = {
            <H4> { text: "Standard" }
            <UIZooRowH> {
                <LinkLabel> { text: "Click me!"}
            }

            <Hr> {}
            <H4> { text: "Styled" }
            <UIZooRowH> {
                <LinkLabel> {
                    draw_bg: {
                        color: #0AA
                        color_hover: #0FF
                        color_down: #0
                    }

                    draw_text: {
                        color: #0AA
                        color_hover: #0FF
                        color_down: #0
                    }

                    text: "Click me!"
                }
            }

            <Hr> {}
            <H4> { text: "LinkLabelGradientX" }
            <UIZooRowH> {
                <LinkLabelGradientX> { text: "<LinkLabelGradientX>"}
            }

            <Hr> {}
            <H4> { text: "LinkLabelGradientY" }
            <UIZooRowH> {
                <LinkLabelGradientY> { text: "<LinkLabelGradientY>"}
            }

            <Hr> {}
            <H4> { text: "LinkLabelIcon" }
            <UIZooRowH> {
                <LinkLabelIcon> {
                    text: "Click me!"
                    draw_icon: {
                        color: #f00,
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                    }

                    icon_walk: {
                        width: 12.5, height: Fit,
                        margin: 0.0
                    }
                }
                <LinkLabelIcon> {
                    text: "Click me!"
                    draw_icon: {
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                    }

                    icon_walk: {
                        width: 12.5,height: Fit,
                        margin: 0.0
                    }
                }
            }
        }
    }
}
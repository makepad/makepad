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
            <Markdown> { body: dep("crate://self/resources/linklabel.md") } 
        }
        demos = {
            <H4> { text: "Standard" }
            <UIZooRowH> {
                <LinkLabel> { text: "Click me!"}
            }

            <Hr> {}
            <H4> { text: "Standard, disabled" }
            <UIZooRowH> {
                <LinkLabel> {
                    text: "Click me!"
                    animator: {
                        disabled = {
                            default: on
                        }
                    }
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

            <Hr> {}
            <H4> { text: "Styling Attributes Reference" }
            <UIZooRowH> {
                <LinkLabel> {
                    label_walk: {
                        width: Fit,
                        height: Fit,
                        margin: { left: 10.}
                    }

                    draw_text: {
                        color: #A
                        color_hover: #C
                        color_down: #8
                        color_focus: #B
                        color_disabled: #3

                        text_style: {
                            font_size: 20.,
                            line_spacing: 1.4,
                            font_family:{ latin = font("crate://makepad_widgets/resources/IBMPlexSans-Italic.ttf", 0.0, 0.0) }
                        }
                        wrap: Word
                    }

                    draw_bg: {
                        color: #0A0
                        color_hover: #0C0
                        color_down: #080
                        color_focus: #0B0
                        color_disabled: #030
                    }

                    icon_walk: {
                        width: 20.
                        height: Fit,
                    }

                    draw_icon: {
                        color: #A00
                        color_hover: #C00
                        color_down: #800
                        color_focus: #B00
                        color_disabled: #300
                    
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                    }

                    text: "Click me!"
                }
            }

        }
    }
}
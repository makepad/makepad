use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoButton = <UIZooTabLayout_B> {
        desc = {
            <Markdown> { body: dep("crate://self/resources/button.md") } 
        }
        demos = {
            <H4> { text: "Standard"}
            <UIZooRowH> {
                basicbutton = <Button> { text: "<Button>" }

                iconbutton = <Button> {
                    draw_icon: {
                        color: #f00,
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                    }
                    text: "<Button>"
                }
            }

            <Hr> {}
            <H4> { text: "Standard, disabled"}
            <UIZooRowH> {
                <Button> {
                    text: "<Button>"
                    animator: {
                        disabled = {
                            default: on
                        }
                    }
                }
            }

            <Hr> {}
            <H4> { text: "GradientX"}
            <UIZooRowH> {
                <ButtonGradientX> { text: "<ButtonGradientX>" }
                <ButtonGradientX> {
                    draw_bg: {
                        border_radius: 1.0,
                        border_radius: 4.0

                        color_1: #C00
                        color_1_hover: #F0F
                        color_1_down: #800

                        color_2: #0CC
                        color_2_hover: #0FF
                        color_2_down: #088

                        border_color_1: #C
                        border_color_1_hover: #F
                        border_color_1_down: #0

                        border_color_2: #3
                        border_color_2_hover: #6
                        border_color_2_down: #8

                    }
                    text: "<ButtonGradientX>"
                }

            }

            <Hr> {}
            <H4> { text: "GradientY"}
            <UIZooRowH> {
                <ButtonGradientY> { text: "<ButtonGradientY>" }
                <ButtonGradientY> {
                    draw_bg: {
                        border_radius: 1.0,
                        border_radius: 4.0

                        color_1: #C00
                        color_1_hover: #F0F
                        color_1_down: #800

                        color_2: #0CC
                        color_2_hover: #0FF
                        color_2_down: #088

                        border_color_1: #C
                        border_color_1_hover: #F
                        border_color_1_down: #0

                        border_color_2: #3
                        border_color_2_hover: #6
                        border_color_2_down: #8

                    }
                    text: "<ButtonGradientY>"
                }

            }

            <Hr> {}
            <H4> { text: "Flat"}
            <UIZooRowH> {
                <ButtonFlat> {
                    draw_icon: {
                        color: #f00,
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                    }
                    text: "<ButtonFlat>"
                }

                <ButtonFlat> {
                    flow: Down,
                    icon_walk: { width: 15. }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                    }
                    text: "<ButtonFlat>"
                }
            }

            <Hr> {}
            <H4> { text: "Flatter"}
            <UIZooRowH> {
                <ButtonFlatter> {
                    draw_icon: {
                        color: #f00,
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                    }
                    text: "<ButtonFlatter>"
                }
            }

            <Hr> {}
            <H4> { text: "Styling Attributes Reference"}
            <UIZooRowH> {
                <Button> {
                    width: Fit
                    text: "<Button>"

                    draw_text: {
                        color: (THEME_COLOR_LABEL_INNER)
                        color_hover: (THEME_COLOR_LABEL_INNER_HOVER)
                        color_down: (THEME_COLOR_LABEL_INNER_DOWN)
                        color_focus: (THEME_COLOR_LABEL_INNER_FOCUS)
                        color_disabled: (THEME_COLOR_LABEL_INNER_DISABLED)

                        text_style: {
                            font_size: (THEME_FONT_SIZE_P)
                            font_family: {
                                latin = font("crate://makepad_widgets/resources/IBMPlexSans-Text.ttf", -0.1, 0.0),
                                chinese = font("crate://makepad_widgets/resources/LXGWWenKaiRegular.ttf", 0.0, 0.0)
                                emoji = font("crate://makepad_widgets/resources/NotoColorEmoji.ttf", 0.0, 0.0)
                            },
                            line_spacing: 1.2
                        }
                    }

                    icon_walk: {
                        width: (THEME_DATA_ICON_WIDTH),
                        height: Fit,
                    }

                    draw_icon: {
                        color: (THEME_COLOR_LABEL_INNER)
                        color_hover: (THEME_COLOR_LABEL_INNER_HOVER)
                        color_down: (THEME_COLOR_LABEL_INNER_DOWN)
                        color_focus: (THEME_COLOR_LABEL_INNER_FOCUS)
                        color_disabled: (THEME_COLOR_LABEL_INNER_DISABLED)
                    
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                    }

                    draw_bg: {
                        color_dither: 1.0

                        border_size: (THEME_BEVELING)
                        border_radius: (THEME_CORNER_RADIUS)

                        color: (THEME_COLOR_OUTSET)
                        color_hover: (THEME_COLOR_OUTSET_HOVER)
                        color_down: (THEME_COLOR_OUTSET_DOWN)
                        color_focus: (THEME_COLOR_OUTSET_FOCUS)
                        color_disabled: (THEME_COLOR_OUTSET_DISABLED)

                        border_color_1: (THEME_COLOR_BEVEL_OUTSET_1)
                        border_color_1_hover: (THEME_COLOR_BEVEL_OUTSET_1_HOVER)
                        border_color_1_down: (THEME_COLOR_BEVEL_OUTSET_1_DOWN)
                        border_color_1_focus: (THEME_COLOR_BEVEL_OUTSET_1_FOCUS)
                        border_color_1_disabled: (THEME_COLOR_BEVEL_OUTSET_1_DISABLED)

                        border_color_2: (THEME_COLOR_BEVEL_OUTSET_2)
                        border_color_2_hover: (THEME_COLOR_BEVEL_OUTSET_2_HOVER)
                        border_color_2_down: (THEME_COLOR_BEVEL_OUTSET_2_DOWN)
                        border_color_2_focus: (THEME_COLOR_BEVEL_OUTSET_2_FOCUS)
                        border_color_2_disabled: (THEME_COLOR_BEVEL_OUTSET_2_DISABLED)
                    }

                }
            }

        }
    }
}
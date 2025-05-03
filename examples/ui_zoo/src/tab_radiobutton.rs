use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoRadioButton = <UIZooTabLayout_B> {
        desc = {
            <Markdown> { body: dep("crate://self/resources/radiobutton.md") } 
        }
        demos = {
            <H4> { text: "Default"}
            <UIZooRowH> {
                radios_demo_1 = <View> {
                    spacing: (THEME_SPACE_2)
                    width: Fit, height: Fit,
                    radio1 = <RadioButton> { text: "Option 1" }
                    radio2 = <RadioButton> { text: "Option 2" }
                    radio3 = <RadioButton> { text: "Option 3" }
                    radio4 = <RadioButton> {
                        text: "Option 4, disabled"
                        animator: {
                            disabled = {
                                default: on
                            }
                        }
                    }
                }
            }

            <Hr> {}
            <H4> { text: "RadioButtonFlat"}
            <UIZooRowH> {
                radios_demo_2 = <View> {
                    spacing: (THEME_SPACE_2)
                    width: Fit, height: Fit,
                    radio1 = <RadioButtonFlat> { text: "Option 1" }
                    radio2 = <RadioButtonFlat> { text: "Option 2" }
                    radio3 = <RadioButtonFlat> { text: "Option 3" }
                    radio4 = <RadioButtonFlat> { text: "Option 4" }
                }
            }

            <Hr> {}
            <H4> { text: "RadioButtonFlatter"}
            <UIZooRowH> {
                radios_demo_3 = <View> {
                    spacing: (THEME_SPACE_2)
                    width: Fit, height: Fit,
                    radio1 = <RadioButtonFlatter> { text: "Option 1" }
                    radio2 = <RadioButtonFlatter> { text: "Option 2" }
                    radio3 = <RadioButtonFlatter> { text: "Option 3" }
                    radio4 = <RadioButtonFlatter> { text: "Option 4" }
                }
            }

            <Hr> {}
            <H4> { text: "RadioButtonGradientX"}
            <UIZooRowH> {
                radios_demo_4 = <View> {
                    spacing: (THEME_SPACE_2)
                    width: Fit, height: Fit,
                    radio1 = <RadioButtonGradientX> { text: "Option 1" }
                    radio2 = <RadioButtonGradientX> { text: "Option 2" }
                    radio3 = <RadioButtonGradientX> { text: "Option 3" }
                    radio4 = <RadioButtonGradientX> { text: "Option 4" }
                }
            }

            <Hr> {}
            <H4> { text: "RadioButtonGradientY"}
            <UIZooRowH> {
                radios_demo_5 = <View> {
                    spacing: (THEME_SPACE_2)
                    width: Fit, height: Fit,
                    radio1 = <RadioButtonGradientY> { text: "Option 1" }
                    radio2 = <RadioButtonGradientY> { text: "Option 2" }
                    radio3 = <RadioButtonGradientY> { text: "Option 3" }
                    radio4 = <RadioButtonGradientY> { text: "Option 4" }
                }
            }

            <Hr> {}
            <H4> { text: "Custom styled marker"}
            radios_demo_8 = <UIZooRowH> {
                radio1 = <RadioButtonCustom> {
                    text: "Option 1"

                    padding: { left: 20. }
                    align: { x: 0., y: 0.5}
                    icon_walk: { width: 12.5, height: Fit, margin: { left: 0. } }
                    draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }

                    label_walk: { margin: { left: 5. } }
                    label_align: { x: 0., y: 0. }
                    
                    draw_icon: {
                        color_1: #800
                        color_1_active: #f00
                        color_1_disabled: #4

                        color_2: #0
                        color_2_active: #f00
                        color_2_disabled: #4
                    }

                    draw_text: {
                        color: #0AA
                        color_hover: #0CC
                        color_down: #088
                        color_active: #f
                        color_focus: #0BB
                        color_disabled: #4

                        text_style: {
                            font_size: 8.,
                            line_spacing: 1.4,
                            font_family:{ latin = font("crate://makepad_widgets/resources/IBMPlexSans-Italic.ttf", 0.0, 0.0) }
                        }
                    }

                }
                radio2 = <RadioButton> {
                    text: "Option 2"

                    padding: { left: 20. }
                    align: { x: 0., y: 0.5}

                    label_walk: { margin: { left: 5. } }
                    label_align: { x: 0., y: 0. }

                    draw_text: {
                        color: #0AA
                        color_hover: #0CC
                        color_down: #088
                        color_active: #f
                        color_focus: #0BB
                        color_disabled: #4

                        text_style: {
                            font_size: 8.,
                            line_spacing: 1.4,
                            font_family:{ latin = font("crate://makepad_widgets/resources/IBMPlexSans-Italic.ttf", 0.0, 0.0) }
                        }
                    }

                    draw_bg: {
                        size: 15.0,

                        border_size: (THEME_BEVELING)
                        border_radius: (THEME_CORNER_RADIUS)

                        color_dither: 1.0

                        color: #A
                        color_hover: #C
                        color_down: #8
                        color_active: #A
                        color_focus: #B
                        color_disabled: #4

                        border_color_1: #A00
                        border_color_1_hover: #C00
                        border_color_1_down: #800
                        border_color_1_active: #A00
                        border_color_1_focus: #B00
                        border_color_1_disabled: #400

                        border_color_2: #0A0
                        border_color_2_hover: #0C0
                        border_color_2_down: #080
                        border_color_2_active: #0A0
                        border_color_2_focus: #0B0
                        border_color_2_disabled: #040

                        mark_color: #0000
                        mark_color_active: #0
                        mark_color_disabled: #0000

                    }

                }
            }

            <Hr> {}
            <H4> { text: "Textual"}
            <UIZooRowH> {
                radios_demo_9 = <View> {
                    width: Fit, height: Fit,
                    flow: Right,
                    spacing: (THEME_SPACE_2)
                    radio1 = <RadioButtonTextual> { text: "Option 1" }
                    radio2 = <RadioButtonTextual> { text: "Option 2" }
                    radio3 = <RadioButtonTextual> { text: "Option 3" }
                    radio4 = <RadioButtonTextual> { text: "Option 4" }
                }
            }

            <Hr> {}
            <H4> { text: "Textual Customized"}
            <UIZooRowH> {
                 radios_demo_10 = <View> {
                    width: Fit, height: Fit,
                    flow: Right,
                    spacing: (THEME_SPACE_2)
                    radio1 = <RadioButtonTextual> { 
                        text: "Option 1"

                        draw_text: {
                            color: #C80,
                            color_hover: #FC0,
                            color_active: #FF4,
                                
                            text_style: <THEME_FONT_REGULAR> {
                                font_size: (THEME_FONT_SIZE_P)
                            }
                        }
                    }
                    radio2 = <RadioButtonTextual> { 
                        text: "Option 2"

                        draw_text: {
                            color: #C80,
                            color_hover: #FC0,
                            color_active: #FF4,
                                
                            text_style: <THEME_FONT_REGULAR> {
                                font_size: (THEME_FONT_SIZE_P)
                            }
                        }
                    }
                    radio3 = <RadioButtonTextual> { 
                        text: "Option 3"

                        draw_text: {
                            color: #C80,
                            color_hover: #FC0,
                            color_active: #FF4,
                                
                            text_style: <THEME_FONT_REGULAR> {
                                font_size: (THEME_FONT_SIZE_P)
                            }
                        }
                    }
                    radio4 = <RadioButtonTextual> { 
                        text: "Option 4"

                        draw_text: {
                            color: #C80,
                            color_hover: #FC0,
                            color_active: #FF4,
                                
                            text_style: <THEME_FONT_REGULAR> {
                                font_size: (THEME_FONT_SIZE_P)
                            }
                        }
                    }
                }
            }

            <Hr> {}
            <H4> { text: "Button Group"}
            radios_demo_11 = <ButtonGroup> {
                radio1 = <RadioButtonTab> { text: "Option 1" }
                radio2 = <RadioButtonTab> { text: "Option 2" }
                radio3 = <RadioButtonTab> { text: "Option 3" }
                radio4 = <RadioButtonTab> {
                    text: "Option 4, disabled"
                    animator: {
                        disabled = {
                            default: on
                        }
                    }
                }
            }

            <Hr> {}
            <H4> { text: "Button Group Flat"}
            radios_demo_12 = <ButtonGroup> {
                radio1 = <RadioButtonTabFlat> { text: "Option 1" }
                radio2 = <RadioButtonTabFlat> { text: "Option 2" }
                radio3 = <RadioButtonTabFlat> { text: "Option 3" }
                radio4 = <RadioButtonTabFlat> { text: "Option 4" }
            }

            <Hr> {}
            <H4> { text: "Button Group Flatter"}
            radios_demo_13 = <ButtonGroup> {
                radio1 = <RadioButtonTabFlatter> { text: "Option 1" }
                radio2 = <RadioButtonTabFlatter> { text: "Option 2" }
                radio3 = <RadioButtonTabFlatter> { text: "Option 3" }
                radio4 = <RadioButtonTabFlatter> { text: "Option 4" }
            }

            <Hr> {}
            <H4> { text: "Button Group styled" }
            radios_demo_14 = <ButtonGroup> {
                radio1 = <RadioButtonTab> {
                    text: "Option 1"

                    icon_walk: {
                        margin: { left: (THEME_SPACE_3 * 1.5) } 
                        width: 12.5, height: Fit,
                    }
                    label_walk: {
                        margin: { left: (THEME_SPACE_1), right: 0. }
                    }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg")

                        color_1: #0
                        color_1_active: #BB0

                        color_2: #0
                        color_2_active: #B00
                    }

                    draw_text: {
                        color: #0
                        color_hover: #C
                        color_active: #F
                    }

                    draw_bg: {
                        border_size: 1.,
                        border_radius: 4.,

                        color_dither: 1.0
                        color: #F00
                        color_hover: #F44
                        color_active: #300

                        border_color_1: #0
                        border_color_1_hover: #F
                        border_color_1_active: #8

                        border_color_2: #0
                        border_color_2_hover: #F
                        border_color_2_active: #8
                    }
                }
                radio2 = <RadioButtonTab> {
                    text: "Option 2"

                    icon_walk: {
                        margin: { left: (THEME_SPACE_3 * 1.5) } 
                        width: 12.5, height: Fit,
                    }
                    label_walk: {
                        margin: { left: (THEME_SPACE_1), right: 0. }
                    }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg")

                        color_1: #0
                        color_1_active: #BB0

                        color_2: #0
                        color_2_active: #B00
                    }

                    draw_text: {
                        color: #0
                        color_hover: #C
                        color_active: #F
                    }

                    draw_bg: {
                        border_size: 1.,
                        border_radius: 4.,

                        color_dither: 1.0
                        color: #F00
                        color_hover: #F44
                        color_active: #300

                        border_color_1: #0
                        border_color_1_hover: #F
                        border_color_1_active: #8

                        border_color_2: #0
                        border_color_2_hover: #F
                        border_color_2_active: #8
                    }
                }
                radio3 = <RadioButtonTab> {
                    text: "Option 3"

                    icon_walk: {
                        margin: { left: (THEME_SPACE_3 * 1.5) } 
                        width: 12.5, height: Fit,
                    }
                    label_walk: {
                        margin: { left: (THEME_SPACE_1), right: 0. }
                    }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg")

                        color_1: #0
                        color_1_active: #BB0

                        color_2: #0
                        color_2_active: #B00
                    }

                    draw_text: {
                        color: #0
                        color_hover: #C
                        color_active: #F
                    }

                    draw_bg: {
                        border_size: 1.,
                        border_radius: 4.,

                        color_dither: 1.0
                        color: #F00
                        color_hover: #F44
                        color_active: #300

                        border_color_1: #0
                        border_color_1_hover: #F
                        border_color_1_active: #8

                        border_color_2: #0
                        border_color_2_hover: #F
                        border_color_2_active: #8
                    }
                } 
                radio4 = <RadioButtonTab> {
                    text: "Option 4"

                    icon_walk: {
                        margin: { left: (THEME_SPACE_3 * 1.5) } 
                        width: 12.5, height: Fit,
                    }
                    label_walk: {
                        margin: { left: (THEME_SPACE_1), right: 0. }
                    }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg")

                        color_1: #0
                        color_1_active: #BB0

                        color_2: #0
                        color_2_active: #B00
                    }

                    draw_text: {
                        color: #0
                        color_hover: #C
                        color_active: #F
                    }

                    draw_bg: {
                        border_size: 1.,
                        border_radius: 4.,

                        color_dither: 1.0
                        color: #F00
                        color_hover: #F44
                        color_active: #300

                        border_color_1: #0
                        border_color_1_hover: #F
                        border_color_1_active: #8

                        border_color_2: #0
                        border_color_2_hover: #F
                        border_color_2_active: #8
                    }
                }
            }

            <Hr> {}
            <H4> { text: "Button Group GradientY" }
            radios_demo_15 = <ButtonGroup> {
                width: Fit, height: Fit,
                radio1 = <RadioButtonTabGradientY> { text: "Option 1" }
                radio2 = <RadioButtonTabGradientY> { text: "Option 2" }
                radio3 = <RadioButtonTabGradientY> { text: "Option 3" }
                radio4 = <RadioButtonTabGradientY> { text: "Option 4" }
            }

            <Hr> {}
            <H4> { text: "Button Group GradientY styled" }
            radios_demo_16 = <ButtonGroup> {
                radio1 = <RadioButtonTabGradientY> {
                    text: "Option 1"

                    draw_text: {
                        color: #0
                        color_hover: #C
                        color_active: #F
                    }

                    draw_bg: {
                        border_size: (THEME_BEVELING)
                        border_radius: 6.

                        color_dither: 1.0

                        color: #F00
                        color_hover: #F44
                        color_active: #300

                        border_color_1: #0
                        border_color_1_hover: #F
                        border_color_1_active: #8

                        border_color_2: #0
                        border_color_2_hover: #F
                        border_color_2_active: #8
                    }
                }
                radio2 = <RadioButtonTabGradientY> {
                    text: "Option 2"

                    draw_text: {
                        color: #0
                        color_hover: #C
                        color_active: #F
                    }

                    draw_bg: {
                        border_size: (THEME_BEVELING)
                        border_radius: 6.

                        color_dither: 1.0

                        color: #F00
                        color_hover: #F44
                        color_active: #300

                        border_color_1: #0
                        border_color_1_hover: #F
                        border_color_1_active: #8

                        border_color_2: #0
                        border_color_2_hover: #F
                        border_color_2_active: #8
                    }
                }
                radio3 = <RadioButtonTabGradientY> {
                    text: "Option 3"

                    draw_text: {
                        color: #0
                        color_hover: #C
                        color_active: #F
                    }

                    draw_bg: {
                        border_size: (THEME_BEVELING)
                        border_radius: 6.

                        color_dither: 1.0

                        color: #F00
                        color_hover: #F44
                        color_active: #300

                        border_color_1: #0
                        border_color_1_hover: #F
                        border_color_1_active: #8

                        border_color_2: #0
                        border_color_2_hover: #F
                        border_color_2_active: #8
                    }
                }
                radio4 = <RadioButtonTabGradientY> {
                    text: "Option 4"

                    draw_text: {
                        color: #0
                        color_hover: #C
                        color_active: #F
                    }

                    draw_bg: {
                        border_size: (THEME_BEVELING)
                        border_radius: 6.

                        color_dither: 1.0

                        color: #F00
                        color_hover: #F44
                        color_active: #300

                        border_color_1: #0
                        border_color_1_hover: #F
                        border_color_1_active: #8

                        border_color_2: #0
                        border_color_2_hover: #F
                        border_color_2_active: #8
                    }
                }
            }

            <Hr> {}
            <H4> { text: "Button Group GradientX" }
            radios_demo_17 = <ButtonGroup> {
                radio1 = <RadioButtonTabGradientX> { text: "Option 1" }
                radio2 = <RadioButtonTabGradientX> { text: "Option 2" }
                radio3 = <RadioButtonTabGradientX> { text: "Option 3" }
                radio4 = <RadioButtonTabGradientX> { text: "Option 4" }
            }

            <Hr> {}
            <H4> { text: "Button Group GradientX" }
            radios_demo_18 = <ButtonGroup> {
                radio1 = <RadioButtonTabGradientX> {
                    text: "Option 1"

                    draw_text: {
                        color: #0
                        color_hover: #C
                        color_active: #F
                    }

                    draw_bg: {
                        border_size: (THEME_BEVELING)

                        color_dither: 1.0

                        color: #F00
                        color_hover: #F44
                        color_active: #300

                        border_color_1: #0
                        border_color_1_hover: #F
                        border_color_1_active: #8

                        border_color_2: #0
                        border_color_2_hover: #F
                        border_color_2_active: #8
                    }
                }
                radio2 = <RadioButtonTabGradientX> {
                    text: "Option 2"

                    draw_text: {
                        color: #0
                        color_hover: #C
                        color_active: #F
                    }

                    draw_bg: {
                        border_size: (THEME_BEVELING)

                        color_dither: 1.0

                        color: #F00
                        color_hover: #F44
                        color_active: #300

                        border_color_1: #0
                        border_color_1_hover: #F
                        border_color_1_active: #8

                        border_color_2: #0
                        border_color_2_hover: #F
                        border_color_2_active: #8
                    }
                }
                radio3 = <RadioButtonTabGradientX> {
                    text: "Option 3"

                    draw_text: {
                        color: #0
                        color_hover: #C
                        color_active: #F
                    }

                    draw_bg: {
                        border_size: (THEME_BEVELING)

                        color_dither: 1.0

                        color: #F00
                        color_hover: #F44
                        color_active: #300

                        border_color_1: #0
                        border_color_1_hover: #F
                        border_color_1_active: #8

                        border_color_2: #0
                        border_color_2_hover: #F
                        border_color_2_active: #8
                    }
                }
                radio4 = <RadioButtonTabGradientX> {
                    text: "Option 4"

                    draw_text: {
                        color: #0
                        color_hover: #C
                        color_active: #F
                    }

                    draw_bg: {
                        border_size: (THEME_BEVELING)

                        color_dither: 1.0

                        color: #F00
                        color_hover: #F44
                        color_active: #300

                        border_color_1: #0
                        border_color_1_hover: #F
                        border_color_1_active: #8

                        border_color_2: #0
                        border_color_2_hover: #F
                        border_color_2_active: #8
                    }
                }
            }

        }
    }
}
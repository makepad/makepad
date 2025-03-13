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
            <H3> { text: "<RadioButton>"}
        }
        demos = {
            <H4> { text: "Default"}
            <View> {
                height: Fit
                flow: Right
                align: { x: 0.0, y: 0.5 }
                radios_demo = <View> {
                    spacing: (THEME_SPACE_2)
                    width: Fit, height: Fit,
                    radio1 = <RadioButton> {
                        text: "Option 1"

                        label_walk: {
                            width: Fit, height: Fit,
                            margin: { left: 20. }
                        }

                        label_align: { y: 0.0 }
                        
                        draw_bg: {
                            border_size: (THEME_BEVELING)

                            color_dither: 1.0

                            color_1: #F00
                            color_1_hover: #F44
                            color_1_active: #F00

                            color_2: #F80
                            color_2_hover: #FA4
                            color_2_active: #F80

                            border_color_1: #0
                            border_color_1_hover: #F
                            border_color_1_active: #8

                            border_color_2: #0
                            border_color_2_hover: #F
                            border_color_2_active: #8

                            mark_color: #FFF0
                            mark_color_hover: #FFFF
                            mark_color_active: #FFFC
                            
                        }
                            
                        draw_text: {
                            color: #A
                            color_hover: #F
                            color_active: #C

                            text_style: <THEME_FONT_REGULAR> {
                                font_size: (THEME_FONT_SIZE_P)
                            }
                        }

                        icon_walk: { width: 13.0, height: Fit }
                            
                        draw_icon: {
                            color_1: #F00
                            color_1_hover: #F44
                            color_1_active: #F00

                            color_2: #F00
                            color_2_hover: #F44
                            color_2_active: #F00
                        }
                    }
                    radio2 = <RadioButton> { text: "Option 2" }
                    radio3 = <RadioButton> { text: "Option 3" }
                    radio4 = <RadioButton> { text: "Option 4" }
                }
            }

            <H4> { text: "Custom Radios"}
            <View> {
                height: Fit
                flow: Right
                align: { x: 0.0, y: 0.5 }
                iconradios_demo = <View> {
                    width: Fit, height: Fit,
                    spacing: (THEME_SPACE_2)
                    flow: Down,

                    radio1 = <RadioButtonCustom> {
                        text: "Option 1"
                        icon_walk: { width: 12.5, height: Fit }
                        draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }

                        label_align: { y: 0.0 }
                        
                        draw_text: {
                            color: #A
                            color_hover: #F
                            color_active: #C

                            text_style: <THEME_FONT_REGULAR> {
                                font_size: (THEME_FONT_SIZE_P)
                            }
                        }

                        draw_icon: {
                            color_1: #000
                            color_1_hover: #F44
                            color_1_active: #F00

                            color_2: #F00
                            color_2_hover: #F44
                            color_2_active: #F00
                        }
                    }
                    radio2 = <RadioButtonCustom> {
                        text: "Option 2"
                        icon_walk: {
                            width: 12.5, height: Fit,
                        }
                        draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                    }
                    radio3 = <RadioButtonCustom> {
                        text: "Option 3"
                        icon_walk: {
                            width: 12.5, height: Fit,
                        }
                        draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                    }
                    radio4 = <RadioButtonCustom> {
                        text: "Option 4"
                        icon_walk: {
                            width: 12.5, height: Fit,
                        }
                        draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                    }
                }
            }

            <H4> { text: "Text only"}
            <View> {
                height: Fit
                flow: Right
                align: { x: 0.0, y: 0.5 }
                textonlyradios_demo = <View> {
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
                    radio2 = <RadioButtonTextual> { text: "Option 2" }
                    radio3 = <RadioButtonTextual> { text: "Option 3" }
                    radio4 = <RadioButtonTextual> { text: "Option 4" }
                }
            }

            <H4> { text: "Button Group"}
            <ButtonGroup> {
                height: Fit
                flow: Right
                align: { x: 0.0, y: 0.5 }
                radiotabs_demo = <View> {
                    spacing: 5.
                    width: Fit, height: Fit,
                    radio1 = <RadioButtonTab> {
                        text: "Option 1"

                        icon_walk: {
                            width: 12.5, height: Fit,
                        }
                        label_walk: {
                            margin: { left: 5. }
                        }
                        draw_icon: {
                            svg_file: dep("crate://self/resources/Icon_Favorite.svg")

                            color_1: #0
                            color_1_hover: #FF0
                            color_1_active: #BB0

                            color_2: #0
                            color_2_hover: #F00
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
                    radio2 = <RadioButtonTab> { text: "Option 2" }
                    radio3 = <RadioButtonTab> { text: "Option 3" }
                    radio4 = <RadioButtonTab> { text: "Option 4" }
                }
            }

            <ButtonGroup> {
                height: Fit
                flow: Right
                align: { x: 0.0, y: 0.5 }
                radiotabs_demo = <View> {
                    spacing: 5.
                    width: Fit, height: Fit,
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

                            color_1: #F00
                            color_1_hover: #F44
                            color_1_active: #300

                            color_2: #F80
                            color_2_hover: #FA4
                            color_2_active: #310

                            border_color_1: #0
                            border_color_1_hover: #F
                            border_color_1_active: #8

                            border_color_2: #0
                            border_color_2_hover: #F
                            border_color_2_active: #8
                        }
                    }
                    radio2 = <RadioButtonTabGradientY> { text: "Option 2" }
                    radio3 = <RadioButtonTabGradientY> { text: "Option 3" }
                    radio4 = <RadioButtonTabGradientY> { text: "Option 4" }
                }
            }

            <ButtonGroup> {
                height: Fit
                flow: Right
                align: { x: 0.0, y: 0.5 }
                radiotabs_demo = <View> {
                    spacing: 5.
                    width: Fit, height: Fit,
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

                            color_1: #F00
                            color_1_hover: #F44
                            color_1_active: #300

                            color_2: #F80
                            color_2_hover: #FA4
                            color_2_active: #310

                            border_color_1: #0
                            border_color_1_hover: #F
                            border_color_1_active: #8

                            border_color_2: #0
                            border_color_2_hover: #F
                            border_color_2_active: #8
                        }
                    }
                    radio2 = <RadioButtonTabGradientX> { text: "Option 2" }
                    radio3 = <RadioButtonTabGradientX> { text: "Option 3" }
                    radio4 = <RadioButtonTabGradientX> { text: "Option 4" }
                }
            }

            <H4> { text: "Media"}
            <View> {
                height: Fit
                flow: Right
                align: { x: 0.0, y: 0.5 }
                mediaradios_demo = <View> {
                    width: Fit, height: Fit,
                    flow: Right,
                    spacing: (THEME_SPACE_2)
                    radio1 = <RadioButtonImage> {
                        width: 50, height: 50,
                        media: Image,
                        image: <Image> { source: dep("crate://self/resources/ducky.png" ) }
                    }
                    radio2 = <RadioButtonImage> {
                        width: 50, height: 50,
                        media: Image,
                        image: <Image> { source: dep("crate://self/resources/ducky.png" ) }
                    }
                    radio3 = <RadioButtonImage> {
                        width: 50, height: 50,
                        media: Image,
                        image: <Image> { source: dep("crate://self/resources/ducky.png" ) }
                    }
                    radio4 = <RadioButtonImage> {
                        width: 50, height: 50,
                        media: Image,
                        image: <Image> { source: dep("crate://self/resources/ducky.png" ) }
                    }
                }
            }
        }
    }
}
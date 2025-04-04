use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoCheckBox = <UIZooTabLayout_B> {
        desc = {
            <H3> { text: "Checkbox"}
            <P> {
                text: "The `CheckBox` widget provides a control for user input in the form of a checkbox. It allows users to select or deselect options."
            }

            <H4> { text: "Layouting"}
            <P> {
                text: "Complete layouting feature set support."
            }

            <H4> { text: "Draw Shaders"}
            <P> {
                text: "Complete layouting feature set support."
            }
        }
        demos = {
            <H4> { text: "Checkbox"}
            <CheckBox> {
                text:"Check me out!"
            }

            <Hr> {}
            <H4> { text: "CheckBoxFlat"}
            <CheckBoxFlat> { text:"Check me out!" }

            <Hr> {}
            <H4> { text: "CheckBoxFlatter"}
            <CheckBoxFlatter> { text:"Check me out!" }

            <Hr> {}
            <H4> { text: "Customized"}
            <UIZooRowH> {
                CheckBoxCustomized = <CheckBox> {
                    text:"Check me out!"

                    label_walk: {
                        width: Fit, height: Fit,
                        margin: <THEME_MSPACE_H_1> { left: 12.5 }
                    }

                    draw_bg: {
                        border_size: 1.0

                        color_1: #F40
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
                        mark_color_hover: #FFF0
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

                    draw_icon: {
                        color: #F00
                        color_hover: #F44
                        color_active: #F00
                    }

                    icon_walk: { width: 13.0, height: Fit }
                }

            }

            <Hr> {}
            <H4> { text: "CheckBoxGradientX"}
            <CheckBoxGradientX> { text:"Check me out!" }

            <Hr> {}
            <H4> { text: "CheckBoxGradientY"}
            <CheckBoxGradientY> { text:"Check me out!" }


            <Hr> {}
            <H4> { text: "Toggle"}
            <UIZooRowH> {
                <Toggle> {text:"Check me out!" }
            }

            <Hr> {}
            <H4> { text: "ToggleFlat"}
            <UIZooRowH> {
                <ToggleFlat> {text:"Check me out!" }
            }

            <Hr> {}
            <H4> { text: "ToggleFlatter"}
            <UIZooRowH> {
                <ToggleFlatter> {text:"Check me out!" }
            }

            <Hr> {}
            <H4> { text: "ToggleGradientX"}
            <UIZooRowH> {
                <ToggleGradientX> {text:"Check me out!" }
            }

            <Hr> {}
            <H4> { text: "ToggleGradientY"}
            <UIZooRowH> {
                <ToggleGradientY> {text:"Check me out!" }
            }

            <Hr> {}
            <H4> { text: "Toggle Customized"}
            <Toggle> {
                text:"Check me out!"

                draw_bg: {
                    border_size: 1.0

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

                    mark_color: #FFFF
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

                draw_icon: {
                    color: #F00
                    color_hover: #F44
                    color_active: #F00
                }

                icon_walk: { width: 13.0, height: Fit }

            }
            <Hr> {}

            <H4> { text: "Custom Icon Mode"}
            <UIZooRowH> {
                <CheckBoxCustom> {
                    text:"Check me out!"
                    draw_bg: { check_type: None }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                    }

                    label_walk: {
                        width: Fit, height: Fit,
                        margin: <THEME_MSPACE_H_1> { left: 12.5 }
                    }

                    draw_bg: {
                        border_size: 1.0

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
                        color: #330
                        color_hover: #8
                        color_active: #F80

                        text_style: <THEME_FONT_REGULAR> {
                            font_size: (THEME_FONT_SIZE_P)
                        }
                    }

                    draw_icon: {
                        color: #300
                        color_hover: #800
                        color_active: #F00
                    }

                    icon_walk: { width: 13.0, height: Fit }
                }
                <CheckBoxCustom> {
                    text:"Check me out!"
                    draw_bg: { check_type: None }
                    draw_icon: {
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                    }
                }
            }

            <Hr> {} 
            <H4> { text: "Output demo"}
            <UIZooRowH> {
                height: Fit
                flow: Right
                align: { x: 0.0, y: 0.5}
                simplecheckbox = <CheckBox> {text:"Check me out!"}
                simplecheckbox_output = <Label> { text:"hmm" }
            }
        }
    }
}
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
            <Markdown> { body: dep("crate://self/resources/checkbox.md") } 
        }
        demos = {
            <H4> { text: "Checkbox"}
            <CheckBox> { text:"<CheckBox>" }
            
            <Hr> {}
            <H4> { text: "Checkbox, disabled"}
            <CheckBox> {
                text:"<CheckBox>"
                animator: {
                    disabled = {
                        default: on
                    }
                }
            }

            <Hr> {}
            <H4> { text: "Standard, fully customized"}
            <UIZooRowH> {
                CheckBoxCustomized = <CheckBox> {
                    text:"<CheckBox>"
                    align: { x: 0., y: .5}
                    padding: { top: 0., left: 0., bottom: 0., right: 0.}
                    margin: { top: 0., left: 0., bottom: 0., right: 0.}

                    label_walk: {
                        width: Fit, height: Fit,
                        margin: <THEME_MSPACE_H_1> { left: 5.5 }
                    }

                    draw_bg: {
                        check_type: None
                        border_size: 1.0

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
                        color: #0AA
                        color_hover: #8ff
                        color_down: #088
                        color_focus: #0ff
                        color_disabled: #8

                        text_style: {
                            font_size: 8.,
                            line_spacing: 1.4,
                            font_family:{ latin = font("crate://makepad_widgets/resources/IBMPlexSans-Italic.ttf", 0.0, 0.0) }
                        }
                    }

                    icon_walk: {
                        width: 20.
                        height: Fit,
                    }

                    draw_icon: {
                        color: #0
                        color_active: #f00
                        color_disabled: #8
                    
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                    }

                    icon_walk: { width: 13.0, height: Fit }
                }

            }

            <Hr> {}
            <H4> { text: "CheckBoxFlat"}
            <CheckBoxFlat> { text:"<CheckBoxFlat>" }

            <Hr> {}
            <H4> { text: "CheckBoxFlatter"}
            <CheckBoxFlatter> { text:"<CheckBoxFlat>" }

            <Hr> {}
            <H4> { text: "CheckBoxGradientX"}
            <CheckBoxGradientX> { text:"<CheckBoxGradientX>" }

            <Hr> {}
            <H4> { text: "CheckBoxGradientY"}
            <CheckBoxGradientY> { text:"<CheckBoxGradientY>" }


            <Hr> {}
            <H4> { text: "Toggle"}
            <UIZooRowH> {
                <Toggle> {text:"<Toggle>" }
            }

            <Hr> {}
            <H4> { text: "ToggleFlat"}
            <UIZooRowH> {
                <ToggleFlat> {text:"<ToggleFlat>" }
            }

            <Hr> {}
            <H4> { text: "ToggleFlatter"}
            <UIZooRowH> {
                <ToggleFlatter> {text:"<ToggleFlatter>" }
            }

            <Hr> {}
            <H4> { text: "ToggleGradientX"}
            <UIZooRowH> {
                <ToggleGradientX> {text:"<ToggleGradientX>" }
            }

            <Hr> {}
            <H4> { text: "ToggleGradientY"}
            <UIZooRowH> {
                <ToggleGradientY> {text:"<ToggleGradientY>" }
            }

            <Hr> {} 
            <H4> { text: "Output demo"}
            <UIZooRowH> {
                height: Fit
                flow: Right
                align: { x: 0.0, y: 0.5}
                simplecheckbox = <CheckBox> {text:"<CheckBox>"}
                simplecheckbox_output = <Label> { text:"" }
            }

        }
    }
}
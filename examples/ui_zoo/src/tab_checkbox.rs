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

            <Hr> {}
            <H4> { text: "Custom Checkbox"}
            <UIZooRowH> {
                <CheckBox> {
                    text:"<CheckBox>"
                    align: { x: 0., y: .5}
                    padding: { top: 0., left: 0., bottom: 0., right: 0.}
                    margin: { top: 0., left: 0., bottom: 0., right: 0.}

                    label_walk: {
                        width: Fit, height: Fit,
                        margin: <THEME_MSPACE_H_1> { left: 5.5 }
                    }

                    draw_bg: { check_type: None }  

                    draw_icon: {
                        color: #0
                        color_active: #f00
                        color_disabled: #8
                    
                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                    }

                    icon_walk: {
                        width: 13.0,
                        height: Fit
                    }
                }

            }

            <Hr> {}
            <H4> { text: "Styling Attributes Reference"}
            <UIZooRowH> {
                <CheckBox> {
                    text:"<CheckBox>"

                    width: Fit, height: Fit,
                    padding: <THEME_MSPACE_2> {}
                    align: { x: 0., y: 0. }

                    label_walk: {
                        width: Fit, height: Fit,
                        margin: <THEME_MSPACE_H_1> { left: 13. }
                    }

                    draw_bg: {
                        check_type: Check
                        size: 14.0;

                        border_size: (THEME_BEVELING)
                        border_radius: (THEME_CORNER_RADIUS)

                        color_dither: 1.0

                        color: (THEME_COLOR_INSET)
                        color_hover: (THEME_COLOR_INSET_HOVER)
                        color_down: (THEME_COLOR_INSET_DOWN)
                        color_active: (THEME_COLOR_INSET_ACTIVE)
                        color_focus: (THEME_COLOR_INSET_FOCUS)
                        color_disabled: (THEME_COLOR_INSET_DISABLED)

                        border_color_1: (THEME_COLOR_BEVEL_INSET_2)
                        border_color_1_hover: (THEME_COLOR_BEVEL_INSET_2_HOVER)
                        border_color_1_down: (THEME_COLOR_BEVEL_INSET_2_DOWN)
                        border_color_1_active: (THEME_COLOR_BEVEL_INSET_2_ACTIVE)
                        border_color_1_focus: (THEME_COLOR_BEVEL_INSET_2_FOCUS)
                        border_color_1_disabled: (THEME_COLOR_BEVEL_INSET_2_DISABLED)

                        border_color_2: (THEME_COLOR_BEVEL_INSET_1)
                        border_color_2_hover: (THEME_COLOR_BEVEL_INSET_1_HOVER)
                        border_color_2_down: (THEME_COLOR_BEVEL_INSET_1_DOWN)
                        border_color_2_active: (THEME_COLOR_BEVEL_INSET_1_ACTIVE)
                        border_color_2_focus: (THEME_COLOR_BEVEL_INSET_1_FOCUS)
                        border_color_2_disabled: (THEME_COLOR_BEVEL_INSET_1_DISABLED)

                        mark_size: 0.65
                        mark_color: (THEME_COLOR_U_HIDDEN)
                        mark_color_hover: (THEME_COLOR_U_HIDDEN)
                        mark_color_down: (THEME_COLOR_U_HIDDEN)
                        mark_color_active: (THEME_COLOR_MARK_ACTIVE)
                        mark_color_active_hover: (THEME_COLOR_MARK_ACTIVE_HOVER)
                        mark_color_focus: (THEME_COLOR_MARK_FOCUS)
                        mark_color_disabled: (THEME_COLOR_MARK_DISABLED)
                    }  
                
                draw_text: {
                    color: (THEME_COLOR_LABEL_OUTER)
                    color_hover: (THEME_COLOR_LABEL_OUTER_HOVER)
                    color_down: (THEME_COLOR_LABEL_OUTER_DOWN)
                    color_focus: (THEME_COLOR_LABEL_OUTER_FOCUS)
                    color_active: (THEME_COLOR_LABEL_OUTER_ACTIVE)
                    color_disabled: (THEME_COLOR_LABEL_OUTER_DISABLED)

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

                }

            }

        }
    }
}
use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoTextInput = <UIZooTabLayout_B> {
        desc = {
            <Markdown> { body: dep("crate://self/resources/textinput.md") } 
        }
        demos = {
            <H4> { text: "TextInput" }
            <UIZooRowH> {
                simpletextinput = <TextInput> { }
                simpletextinput_outputbox = <P> {
                    text: "Output"
                }
            }

            <Hr> {}
            <H4> { text: "TextInput, Disabled" }
            <TextInput> {
                empty_text: "Inline Label"
                animator: {
                    disabled = {
                        default: on
                    }
                }
            }
            
            <Hr> {}
            <H4> { text: "TextInput, customized" }
            <TextInput> {
                empty_text: "Inline Label"

                width: Fill, height: Fit,
                padding: <THEME_MSPACE_1> { left: (THEME_SPACE_2), right: (THEME_SPACE_2) }
                margin: <THEME_MSPACE_V_1> {}
                flow: RightWrap,
                is_password: false,
                is_read_only: false,
                is_numeric_only: false
                empty_text: "Your text here",
                
                draw_bg: {
                    border_radius: (THEME_CORNER_RADIUS)
                    border_size: (THEME_BEVELING)

                    color_dither: 1.0

                    color: (THEME_COLOR_INSET)
                    color_hover: (THEME_COLOR_INSET_HOVER)
                    color_focus: (THEME_COLOR_INSET_FOCUS)
                    color_down: (THEME_COLOR_INSET_DOWN)
                    color_empty: (THEME_COLOR_INSET_EMPTY)
                    color_disabled: (THEME_COLOR_INSET_DISABLED)

                    border_color_1: (THEME_COLOR_BEVEL_INSET_2)
                    border_color_1_hover: (THEME_COLOR_BEVEL_INSET_2_HOVER)
                    border_color_1_focus: (THEME_COLOR_BEVEL_INSET_2_FOCUS)
                    border_color_1_down: (THEME_COLOR_BEVEL_INSET_2_DOWN)
                    border_color_1_empty: (THEME_COLOR_BEVEL_INSET_2_EMPTY)
                    border_color_1_disabled: (THEME_COLOR_BEVEL_INSET_2_DISABLED)

                    border_color_2: (THEME_COLOR_BEVEL_INSET_1)
                    border_color_2_hover: (THEME_COLOR_BEVEL_INSET_1_HOVER)
                    border_color_2_focus: (THEME_COLOR_BEVEL_INSET_1_FOCUS)
                    border_color_2_down: (THEME_COLOR_BEVEL_INSET_1_DOWN)
                    border_color_2_empty: (THEME_COLOR_BEVEL_INSET_1_EMPTY)
                    border_color_2_disabled: (THEME_COLOR_BEVEL_INSET_1_DISABLED)
                }

                draw_text: {
                    color: (THEME_COLOR_TEXT)
                    color_hover: (THEME_COLOR_TEXT_HOVER)
                    color_focus: (THEME_COLOR_TEXT_FOCUS)
                    color_down: (THEME_COLOR_TEXT_DOWN)
                    color_disabled: (THEME_COLOR_TEXT_DISABLED)
                    color_empty: (THEME_COLOR_TEXT_PLACEHOLDER)
                    color_empty_hover: (THEME_COLOR_TEXT_PLACEHOLDER_HOVER)
                    color_empty_focus: (THEME_COLOR_TEXT_FOCUS)

                    text_style: <THEME_FONT_REGULAR> {
                        line_spacing: (THEME_FONT_WDGT_LINE_SPACING),
                        font_size: (THEME_FONT_SIZE_P)
                    }
                }

                draw_selection: {
                    border_radius: (THEME_TEXTSELECTION_CORNER_RADIUS)

                    color: (THEME_COLOR_SELECTION)
                    color_hover: (THEME_COLOR_SELECTION_HOVER)
                    color_focus: (THEME_COLOR_SELECTION_FOCUS)
                    color_down: (THEME_COLOR_SELECTION_DOWN)
                    color_empty: (THEME_COLOR_SELECTION_EMPTY)
                    color_disabled: (THEME_COLOR_SELECTION_DISABLED)

                }

                draw_cursor: {
                    border_radius: 0.5
                    color: (THEME_COLOR_TEXT_CURSOR)
                }
            }

            <Hr> {}
            <H4> { text: "TextInput Inline Label" }
            <TextInput> { empty_text: "Inline Label" }

            <Hr> {}
            <H4> { text: "TextInput with content" }
            <TextInput> { text: "Some text" }

            <Hr> {}
            <H4> { text: "TextInputFlat" }
            <TextInputFlat> { empty_text: "Inline Label" }

            <Hr> {}
            <H4> { text: "TextInputFlatter" }
            <TextInputFlatter> { empty_text: "Inline Label" }

            <Hr> {}
            <H4> { text: "TextInputGradientX" }
            <TextInputGradientX> { empty_text: "Inline Label" }

            <Hr> {}
            <H4> { text: "TextInputGradientY" }
            <TextInputGradientY> { empty_text: "Inline Label" }

            <Hr> {}
            <H4> { text: "Styling Attributes Reference" }
            <TextInput> {
                width: Fill, height: Fit,
                padding: <THEME_MSPACE_1> { left: (THEME_SPACE_2), right: (THEME_SPACE_2) }
                margin: <THEME_MSPACE_V_1> {}
                flow: RightWrap,
                is_password: false,
                is_read_only: false,
                is_numeric_only: false

                empty_text: "Inline Label"

                draw_bg: {
                    border_radius: (THEME_CORNER_RADIUS)
                    border_size: (THEME_BEVELING)

                    color_dither: 1.0

                    color: (THEME_COLOR_INSET)
                    color_hover: (THEME_COLOR_INSET_HOVER)
                    color_focus: (THEME_COLOR_INSET_FOCUS)
                    color_down: (THEME_COLOR_INSET_DOWN)
                    color_empty: (THEME_COLOR_INSET_EMPTY)
                    color_disabled: (THEME_COLOR_INSET_DISABLED)

                    border_color_1: (THEME_COLOR_BEVEL_INSET_2)
                    border_color_1_hover: (THEME_COLOR_BEVEL_INSET_2_HOVER)
                    border_color_1_focus: (THEME_COLOR_BEVEL_INSET_2_FOCUS)
                    border_color_1_down: (THEME_COLOR_BEVEL_INSET_2_DOWN)
                    border_color_1_empty: (THEME_COLOR_BEVEL_INSET_2_EMPTY)
                    border_color_1_disabled: (THEME_COLOR_BEVEL_INSET_2_DISABLED)

                    border_color_2: (THEME_COLOR_BEVEL_INSET_1)
                    border_color_2_hover: (THEME_COLOR_BEVEL_INSET_1_HOVER)
                    border_color_2_focus: (THEME_COLOR_BEVEL_INSET_1_FOCUS)
                    border_color_2_down: (THEME_COLOR_BEVEL_INSET_1_DOWN)
                    border_color_2_empty: (THEME_COLOR_BEVEL_INSET_1_EMPTY)
                    border_color_2_disabled: (THEME_COLOR_BEVEL_INSET_1_DISABLED)
                }

                draw_text: {
                    color: (THEME_COLOR_TEXT)
                    color_hover: (THEME_COLOR_TEXT_HOVER)
                    color_focus: (THEME_COLOR_TEXT_FOCUS)
                    color_down: (THEME_COLOR_TEXT_DOWN)
                    color_disabled: (THEME_COLOR_TEXT_DISABLED)
                    color_empty: (THEME_COLOR_TEXT_PLACEHOLDER)
                    color_empty_hover: (THEME_COLOR_TEXT_PLACEHOLDER_HOVER)
                    color_empty_focus: (THEME_COLOR_TEXT_FOCUS)

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
                
                draw_selection: {
                    border_radius: (THEME_TEXTSELECTION_CORNER_RADIUS)

                    color: (THEME_COLOR_SELECTION)
                    color_hover: (THEME_COLOR_SELECTION_HOVER)
                    color_focus: (THEME_COLOR_SELECTION_FOCUS)
                    color_down: (THEME_COLOR_SELECTION_DOWN)
                    color_empty: (THEME_COLOR_SELECTION_EMPTY)
                    color_disabled: (THEME_COLOR_SELECTION_DISABLED)
                }

                draw_cursor: {
                    border_radius: 0.5
                    color: (THEME_COLOR_TEXT_CURSOR)
                }

            }

        }
    }
}
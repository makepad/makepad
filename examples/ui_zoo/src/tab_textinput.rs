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
            <H3> { text: "<TextInput>"}
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
            <H4> { text: "TextInput Inline Label" }
            <TextInput> { empty_text: "Inline Label" }

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
            <H4> { text: "TextInputGradientX styled" }
            <TextInputGradientX> {
                draw_bg: {
                    border_radius: 7.
                    border_size: 1.5

                    color_dither: 1.0

                    color_1: #F
                    color_1_hover: #F
                    color_1_focus: #F

                    color_2: #AA0
                    color_2_hover: #FF0
                    color_2_focus: #CC0

                    border_color_1: (THEME_COLOR_BEVEL_SHADOW)
                    border_color_1_hover: (THEME_COLOR_BEVEL_SHADOW)
                    border_color_1_focus: (THEME_COLOR_BEVEL_SHADOW)

                    border_color_2: (THEME_COLOR_BEVEL_LIGHT)
                    border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT)
                    border_color_2_focus: (THEME_COLOR_BEVEL_LIGHT)
                }

                draw_text: {
                    color: #3
                    color_hover: #484848
                    color_focus: #0
                    color_empty: #7
                    color_empty_focus: #6

                    wrap: Word,

                    text_style: <THEME_FONT_REGULAR> {
                        line_spacing: (THEME_FONT_LINE_SPACING),
                        font_size: (THEME_FONT_SIZE_P)
                    }

                    fn get_color(self) -> vec4 {
                        return
                        mix(
                            mix(
                                mix(self.color, self.color_hover, self.hover),
                                self.color_focus,
                                self.focus
                            ),
                            mix(self.color_empty, self.color_empty_focus, self.hover),
                            self.is_empty
                        )
                    }
                }

                draw_selection: {
                    color_1: (THEME_COLOR_BG_HIGHLIGHT_INLINE)
                    color_1_hover: (THEME_COLOR_BG_HIGHLIGHT_INLINE * 1.4)
                    color_1_focus: (THEME_COLOR_BG_HIGHLIGHT_INLINE * 1.2)

                    color_2: #0AA
                    color_2_hover: #0FF
                    color_2_focus: #0CC
                }

                draw_cursor: { color: #f00 }

                empty_text: "Inline Label"
            }

            <Hr> {}
            <H4> { text: "TextInputGradientY" }
            <TextInputGradientY> { empty_text: "Inline Label" }

            <Hr> {}
            <H4> { text: "TextInputGradientY styled"}
            <TextInputGradientY> {
                draw_bg: {
                    border_radius: 7.
                    border_size: 1.5

                    color_dither: 1.0

                    color_1: #F
                    color_1_hover: #F
                    color_1_focus: #F

                    color_2: #AA0
                    color_2_hover: #FF0
                    color_2_focus: #CC0

                    border_color_1: (THEME_COLOR_BEVEL_SHADOW)
                    border_color_1_hover: (THEME_COLOR_BEVEL_SHADOW)
                    border_color_1_focus: (THEME_COLOR_BEVEL_SHADOW)

                    border_color_2: (THEME_COLOR_BEVEL_LIGHT)
                    border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT)
                    border_color_2_focus: (THEME_COLOR_BEVEL_LIGHT)
                }

                draw_text: {
                    color: #3
                    color_hover: #484848
                    color_focus: #0
                    color_empty: #7
                    color_empty_focus: #6

                    wrap: Word,

                    text_style: <THEME_FONT_REGULAR> {
                        line_spacing: (THEME_FONT_LINE_SPACING),
                        font_size: (THEME_FONT_SIZE_P)
                    }

                    fn get_color(self) -> vec4 {
                        return
                        mix(
                            mix(
                                mix(self.color, self.color_hover, self.hover),
                                self.color_focus,
                                self.focus
                            ),
                            mix(self.color_empty, self.color_empty_focus, self.hover),
                            self.is_empty
                        )
                    }
                }

                draw_selection: {
                    color_1: (THEME_COLOR_BG_HIGHLIGHT_INLINE)
                    color_1_hover: (THEME_COLOR_BG_HIGHLIGHT_INLINE * 1.4)
                    color_1_focus: (THEME_COLOR_BG_HIGHLIGHT_INLINE * 1.2)

                    color_2: #0AA
                    color_2_hover: #0FF
                    color_2_focus: #0CC
                }

                draw_cursor: { color: #f00 }

                empty_text: "Inline Label"
            }
        }
    }
}
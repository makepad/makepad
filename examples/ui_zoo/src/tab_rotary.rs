use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoRotary = <UIZooTabLayout_B> {
        desc = {
            <Markdown> { body: dep("crate://self/resources/rotary.md") } 
        }
        demos = {
            <H4> { text: "<Rotary>"}
            <UIZooRowH> {
                align: { x: 0. , y: 0.}

                <Rotary> { text: "Label" }

                <Rotary> {
                    text: "Label",
                    draw_bg: {
                        gap: 0.,
                    }
                }

                <Rotary> {
                    text: "Label",
                    draw_bg: {
                        gap: 180.,
                    }
                }

                <Rotary> {
                    text: "Label"
                    animator: {
                        disabled = {
                            default: on
                        }
                    }
                }

                <Rotary> {
                    width: Fill,
                    height: 150
                    text: "Label",
                    draw_bg: {
                        val_size: 30.
                        val_padding: 20.,
                    }
                }

            }

            <Hr> {}
            <H4> { text: "<RotaryGradientY>"}
            <UIZooRowH> {
                align: { x: 0. , y: 0.}
                <RotaryGradientY> {
                    text: "Label",
                }
                <RotaryGradientY> {
                    text: "Label",
                    draw_bg: {
                        gap: 0.,
                    }
                }
                <RotaryGradientY> {
                    text: "Label",
                    draw_bg: {
                        gap: 180.,
                    }
                }
                <RotaryGradientY> {
                    text: "Label",
                    draw_bg: {
                        val_size: 30.
                    }
                }
                <RotaryGradientY> {
                    width: Fill,
                    height: 150
                    text: "Label",
                    draw_bg: {
                        val_size: 30.
                        val_padding: 20.,
                    }
                }
            }


            <Hr> {}
            <H4> { text: "RotaryFlat" }
            <UIZooRowH> {
                align: { x: 0. , y: 0.}
                <RotaryFlat> {
                    text: "Label",
                }
                <RotaryFlat> {
                    text: "Label",
                    draw_bg: {
                        gap: 0.,
                    }
                }
                <RotaryFlat> {
                    text: "Label",
                    draw_bg: {
                        gap: 180.,
                    }
                }
                <RotaryFlat> {
                    text: "Label",
                    draw_bg: {
                        val_size: 30.
                    }
                }
                <RotaryFlat> {
                    width: Fill,
                    height: 150
                    text: "Label",
                    draw_bg: {
                        val_size: 30.
                        val_padding: 20.,
                    }
                }
            }

            <Hr> {}
            <H4> { text: "RotaryFlatter" }
            <UIZooRowH> {
                align: { x: 0. , y: 0.}
                <RotaryFlatter> {
                    text: "Label",
                }
                <RotaryFlatter> {
                    text: "Label",
                    draw_bg: {
                        gap: 0.,
                    }
                }
                <RotaryFlatter> {
                    text: "Label",
                    draw_bg: {
                        gap: 180.,
                    }
                }
                <RotaryFlatter> {
                    text: "Label",
                    draw_bg: {
                        val_size: 30.
                    }
                }
                <RotaryFlatter> {
                    width: Fill,
                    height: 150
                    text: "Label",
                    draw_bg: {
                        val_size: 30.
                        val_padding: 20.,
                    }
                }
            }

            <Hr> {}
            <H4> { text: "Styling Attributes Reference" }
            <UIZooRowH> {
                <Rotary> {
                    text: "Label"

                    height: 95., width: 65.,
                    axis: Vertical,
                    flow: Right
                    align:{x:0.,y:0.0}

                    label_walk:{
                        margin:{top:0}
                        width: Fill
                    }

                    text_input:{ 
                        width: Fit
                    }

                    draw_text: {
                        color: (THEME_COLOR_LABEL_OUTER)
                        color_hover: (THEME_COLOR_LABEL_OUTER_HOVER)
                        color_drag: (THEME_COLOR_LABEL_OUTER_DRAG)
                        color_focus: (THEME_COLOR_LABEL_OUTER_FOCUS)
                        color_disabled: (THEME_COLOR_LABEL_OUTER_DISABLED)
                        color_empty: (THEME_COLOR_TEXT_PLACEHOLDER)

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

                    draw_bg: {
                        gap: 90.
                        val_padding: 10.

                        border_size: (THEME_BEVELING)
                        val_size: 20.

                        color_dither: 1.,
                        
                        color: (THEME_COLOR_INSET)
                        color_hover: (THEME_COLOR_INSET_HOVER)
                        color_focus: (THEME_COLOR_INSET_FOCUS)
                        color_disabled: (THEME_COLOR_INSET_DISABLED)
                        color_drag: (THEME_COLOR_INSET_DRAG)

                        border_color_1: (THEME_COLOR_BEVEL_INSET_1)
                        border_color_1_hover: (THEME_COLOR_BEVEL_INSET_1_HOVER)
                        border_color_1_drag: (THEME_COLOR_BEVEL_INSET_1_DRAG)
                        border_color_1_focus: (THEME_COLOR_BEVEL_INSET_1_FOCUS)
                        border_color_1_disabled: (THEME_COLOR_BEVEL_INSET_1_DISABLED)

                        border_color_2: (THEME_COLOR_BEVEL_INSET_2)
                        border_color_2_hover: (THEME_COLOR_BEVEL_INSET_2_HOVER)
                        border_color_2_drag: (THEME_COLOR_BEVEL_INSET_2_DRAG)
                        border_color_2_focus: (THEME_COLOR_BEVEL_INSET_2_FOCUS)
                        border_color_2_disabled: (THEME_COLOR_BEVEL_INSET_2_DISABLED)

                        handle_color: (THEME_COLOR_HANDLE);
                        handle_color_hover: (THEME_COLOR_HANDLE_HOVER);
                        handle_color_focus: (THEME_COLOR_HANDLE_FOCUS);
                        handle_color_disabled: (THEME_COLOR_HANDLE_DISABLED);
                        handle_color_drag: (THEME_COLOR_HANDLE_DRAG);

                        val_color_1: (THEME_COLOR_VAL_1);
                        val_color_1_hover: (THEME_COLOR_VAL_1);
                        val_color_1_focus: (THEME_COLOR_VAL_1);
                        val_color_1_disabled: (THEME_COLOR_VAL_1);
                        val_color_1_drag: (THEME_COLOR_VAL_1_DRAG);

                        val_color_2: (THEME_COLOR_VAL_2);
                        val_color_2_hover: (THEME_COLOR_VAL_2);
                        val_color_2_focus: (THEME_COLOR_VAL_2);
                        val_color_2_disabled: (THEME_COLOR_VAL_2);
                        val_color_2_drag: (THEME_COLOR_VAL_2_DRAG);
                    }
                }
            }

        }
    }
}
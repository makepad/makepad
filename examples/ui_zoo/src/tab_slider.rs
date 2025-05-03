use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    pub DemoSlider = <UIZooTabLayout_B> {
        desc = {
            <Markdown> { body: dep("crate://self/resources/slider.md") } 
        }
        demos = {
            <H4> { text: "Slider"}
            <Slider> { text: "Default" }

            <Slider> {
                text: "Standard, customized"

                min: 0.0, max: 1.0,
                step: 0.0,
                label_align: { x: 0., y: 0. }
                precision: 2,

                draw_text: {
                    color: #A,
                    color_hover: #C
                    color_focus: #B
                    color_drag: #8
                    color_empty: #9
                    color_disabled: #f

                    text_style: {
                        font_size: 8.,
                        line_spacing: 1.4,
                        font_family:{ latin = font("crate://makepad_widgets/resources/IBMPlexSans-Italic.ttf", 0.0, 0.0) }
                    }
                }

                label_walk: {
                    margin: { top: 0., bottom: (THEME_SPACE_1) },
                }

                text_input: <TextInput> {
                    empty_text: "0",
                    is_numeric_only: true,
                    is_read_only: false,

                    width: Fit,
                    label_align: {y: 0.},
                    margin: 0.
                    padding: 0.

                    draw_text: {
                        color: #A
                        color_hover: #C
                        color_focus: #B
                        color_down: #8
                        color_disabled: #3
                        color_empty: #6
                        color_empty_hover: #7
                        color_empty_focus: #9

                        text_style: {
                            font_size: 8.,
                            line_spacing: 1.4,
                            font_family:{ latin = font("crate://makepad_widgets/resources/IBMPlexSans-Italic.ttf", 0.0, 0.0) }
                        }
                    }

                    
                    draw_cursor: { color: #f00 }

                    draw_selection: {
                        border_radius: 1.

                        color: #0008
                        color_hover: #000A
                        color_focus: #000B
                        color_empty: #0000
                        color_disabled: #0003
                    }
                }

                draw_bg: {
                    handle_size: 20.
                    border_size: 0.75
                    val_size: 3.
                    bipolar: 0.0,

                    color_dither: 1.,
                    
                    color: (THEME_COLOR_INSET)
                    color_hover: (THEME_COLOR_INSET_HOVER)
                    color_focus: (THEME_COLOR_INSET_FOCUS)
                    color_disabled: (THEME_COLOR_INSET_DISABLED)
                    color_drag: (THEME_COLOR_INSET_DRAG)

                    handle_color_1: (THEME_COLOR_HANDLE_1)
                    handle_color_1_hover: (THEME_COLOR_HANDLE_1_HOVER)
                    handle_color_1_focus: (THEME_COLOR_HANDLE_1_FOCUS)
                    handle_color_1_disabled: (THEME_COLOR_HANDLE_1_DISABLED)
                    handle_color_1_drag: (THEME_COLOR_HANDLE_1_DRAG)

                    handle_color_2: (THEME_COLOR_HANDLE_2)
                    handle_color_2_hover: (THEME_COLOR_HANDLE_2_HOVER)
                    handle_color_2_focus: (THEME_COLOR_HANDLE_2_FOCUS)
                    handle_color_2_disabled: (THEME_COLOR_HANDLE_2_DISABLED)
                    handle_color_2_drag: (THEME_COLOR_HANDLE_2_DRAG)

                    border_color_1: (#088)
                    border_color_1_hover: (#0BB)
                    border_color_1_focus: (#0AA)
                    border_color_1_disabled: (#04)
                    border_color_1_drag: (#066)

                    border_color_2: (THEME_COLOR_BEVEL_INSET_1)
                    border_color_2_hover: (THEME_COLOR_BEVEL_INSET_1_HOVER)
                    border_color_2_focus: (THEME_COLOR_BEVEL_INSET_1_FOCUS)
                    border_color_2_disabled: (THEME_COLOR_BEVEL_INSET_1_DISABLED)
                    border_color_2_drag: (THEME_COLOR_BEVEL_INSET_1_DRAG)

                    val_color: #00A
                    val_color_hover: #00C
                    val_color_focus: #00B
                    val_color_disabled: #8
                    val_color_drag: #00F

                }
            }
            <Slider> {
                text: "Default, disabled"
                animator: {
                    disabled = {
                        default: on
                    }
                }
            }
            <Slider> { text: "label_align", label_align: { x: 0.5, y: 0. } }
            <Slider> { text: "min/max", min: 0., max: 100. }
            <Slider> { text: "precision", precision: 20 }
            <Slider> { text: "stepped", step: 0.1 }
            
            <H4> { text: "SliderGradientY"}
            <SliderGradientY> { text: "Default" }
            <SliderGradientY> { text: "label_align", label_align: { x: 0.5, y: 0. } }
            <SliderGradientY> { text: "min/max", min: 0., max: 100. }
            <SliderGradientY> { text: "precision", precision: 20 }
            <SliderGradientY> { text: "stepped", step: 0.1 }
            
            <Hr> {}
            <H4> { text: "SliderFlat"}
            <SliderFlat> { text: "Default" }
            <SliderFlat> { text: "label_align", label_align: { x: 0.5, y: 0. } }
            <SliderFlat> { text: "min/max", min: 0., max: 100. }
            <SliderFlat> { text: "precision", precision: 20 }
            <SliderFlat> { text: "stepped", step: 0.1 }

            <Hr> {}
            <H4> { text: "SliderFlatter"}
            <SliderFlatter> { text: "Default" }
            <SliderFlatter> { text: "label_align", label_align: { x: 0.5, y: 0. } }
            <SliderFlatter> { text: "min/max", min: 0., max: 100. }
            <SliderFlatter> { text: "precision", precision: 20 }
            <SliderFlatter> { text: "stepped", step: 0.1 }

            <H4> { text: "SliderMinimal"}
            <SliderMinimal> { text: "Default" }
            <SliderMinimal> {
                text: "Default, disabled"
                animator: {
                    disabled = {
                        default: on
                    }
                }
            }
            <SliderMinimal> {
                text: "Standard, customized"

                min: 0.0, max: 1.0,
                step: 0.0,
                label_align: { x: 0., y: 0. }
                precision: 2,

                draw_text: {
                    color: #A,
                    color_hover: #C
                    color_focus: #B
                    color_drag: #8
                    color_empty: #9
                    color_disabled: #f

                    text_style: {
                        font_size: 8.,
                        line_spacing: 1.4,
                        font_family:{ latin = font("crate://makepad_widgets/resources/IBMPlexSans-Italic.ttf", 0.0, 0.0) }
                    }
                }

                label_walk: {
                    margin: { top: 0., bottom: (THEME_SPACE_1) },
                }

                text_input: <TextInput> {
                    empty_text: "0",
                    is_numeric_only: true,
                    is_read_only: false,

                    width: Fit,
                    label_align: {y: 0.},
                    margin: 0.
                    padding: 0.

                    draw_text: {
                        color: #A
                        color_hover: #C
                        color_focus: #B
                        color_down: #8
                        color_disabled: #3
                        color_empty: #6
                        color_empty_hover: #7
                        color_empty_focus: #9

                        text_style: {
                            font_size: 8.,
                            line_spacing: 1.4,
                            font_family:{ latin = font("crate://makepad_widgets/resources/IBMPlexSans-Italic.ttf", 0.0, 0.0) }
                        }
                    }

                    
                    draw_cursor: { color: #f00 }

                    draw_selection: {
                        border_radius: 1.

                        color: #0008
                        color_hover: #000A
                        color_focus: #000B
                        color_empty: #0000
                        color_disabled: #0003
                    }
                }

                draw_bg: {
                    border_size: 0.75

                    color_1: (THEME_COLOR_INSET_1)
                    color_1_hover: (THEME_COLOR_INSET_1_HOVER)
                    color_1_focus: (THEME_COLOR_INSET_1_FOCUS)
                    color_1_disabled: (THEME_COLOR_INSET_1_DISABLED)
                    color_1_drag: (THEME_COLOR_INSET_1_DRAG)

                    color_2: (THEME_COLOR_INSET_2)
                    color_2_hover: (THEME_COLOR_INSET_2_HOVER)
                    color_2_focus: (THEME_COLOR_INSET_2_FOCUS)
                    color_2_disabled: (THEME_COLOR_INSET_2_DISABLED)
                    color_2_drag: (THEME_COLOR_INSET_2_DRAG)

                    handle_color: (THEME_COLOR_HANDLE)
                    handle_color_hover: (THEME_COLOR_HANDLE_HOVER)
                    handle_color_focus: (THEME_COLOR_HANDLE_FOCUS)
                    handle_color_drag: (THEME_COLOR_HANDLE_DRAG)
                    handle_color_disabled: (THEME_COLOR_HANDLE_DISABLED)

                    border_color_1: (#088)
                    border_color_1_hover: (#0BB)
                    border_color_1_focus: (#0AA)
                    border_color_1_disabled: (#04)
                    border_color_1_drag: (#066)

                    border_color_2: (THEME_COLOR_BEVEL_INSET_1)
                    border_color_2_hover: (THEME_COLOR_BEVEL_INSET_1_HOVER)
                    border_color_2_focus: (THEME_COLOR_BEVEL_INSET_1_FOCUS)
                    border_color_2_disabled: (THEME_COLOR_BEVEL_INSET_1_DISABLED)
                    border_color_2_drag: (THEME_COLOR_BEVEL_INSET_1_DRAG)

                    val_color: #00A
                    val_color_hover: #00C
                    val_color_focus: #00B
                    val_color_disabled: #8
                    val_color_drag: #00F

                }
            }

            <SliderMinimal> { text: "label_align", label_align: { x: 0.5, y: 0. } }
            <SliderMinimal> { text: "min/max", min: 0., max: 100. }
            <SliderMinimal> { text: "precision", precision: 20 }
            <SliderMinimal> { text: "stepped", step: 0.1 }

            <H4> { text: "SliderMinimalFlat"}
            <SliderMinimalFlat> { text: "Default" }
            <SliderMinimalFlat> { text: "label_align", label_align: { x: 0.5, y: 0. } }
            <SliderMinimalFlat> { text: "min/max", min: 0., max: 100. }
            <SliderMinimalFlat> { text: "precision", precision: 20 }
            <SliderMinimalFlat> { text: "stepped", step: 0.1 }

            <Hr> {}
            <H4> { text: "SliderRound"}
            <SliderRound> { text: "Default" }
            <SliderRound> {
                text: "Disabled"
                animator: {
                    disabled = {
                        default: on
                    }
                }
            }
            <SliderRound> {
                text: "Standard, customized"
                
                min: 0.0, max: 1.0,
                step: 0.0,
                label_align: { x: 0., y: 0. }
                precision: 2,

                draw_text: {
                    color: (THEME_COLOR_TEXT_VAL)
                    color_hover: (THEME_COLOR_TEXT_HOVER)
                    color_focus: (THEME_COLOR_TEXT_FOCUS)
                    color_drag: (THEME_COLOR_TEXT_DOWN)
                    color_disabled: (THEME_COLOR_TEXT_DISABLED)
                    color_empty: (THEME_COLOR_TEXT_PLACEHOLDER)

                    text_style: {
                        font_size: 8.,
                        line_spacing: 1.4,
                        font_family:{ latin = font("crate://makepad_widgets/resources/IBMPlexSans-Italic.ttf", 0.0, 0.0) }
                    }
                }

                label_walk: {
                    margin: { top: 0., bottom: (THEME_SPACE_1) },
                }

                text_input: <TextInput> {
                    empty_text: "0",
                    is_numeric_only: true,
                    is_read_only: false,

                    width: Fit,
                    label_align: {y: 0.},
                    margin: 0.
                    padding: 0.

                    draw_text: {
                        color: #A
                        color_hover: #C
                        color_focus: #B
                        color_down: #8
                        color_disabled: #3
                        color_empty: #6
                        color_empty_hover: #7
                        color_empty_focus: #9

                        text_style: {
                            font_size: 8.,
                            line_spacing: 1.4,
                            font_family:{ latin = font("crate://makepad_widgets/resources/IBMPlexSans-Italic.ttf", 0.0, 0.0) }
                        }
                    }

                    
                    draw_cursor: { color: #f00 }

                    draw_selection: {
                        border_radius: 1.

                        color: #0008
                        color_hover: #000A
                        color_focus: #000B
                        color_empty: #0000
                        color_disabled: #0003
                    }
                }

                draw_bg: {
                    label_size: 75.
                    val_heat: 10.
                    border_size: 0.75
                    border_radius: (THEME_CORNER_RADIUS * 2.)

                    val_color_1: #FFCC00
                    val_color_1_hover: #FF9944
                    val_color_1_focus: #FFCC44
                    val_color_1_drag: #FFAA00

                    val_color_2: #F00
                    val_color_2_hover: #F00
                    val_color_2_focus: #F00
                    val_color_2_drag: #F00

                    handle_color: #0000
                    handle_color_hover: #0008
                    handle_color_focus: #000C
                    handle_color_drag: #000F

                    color: (THEME_COLOR_INSET)
                    color_hover: (THEME_COLOR_INSET_HOVER)
                    color_focus: (THEME_COLOR_INSET_FOCUS)
                    color_disabled: (THEME_COLOR_INSET_DISABLED)
                    color_drag: (THEME_COLOR_INSET_DRAG)

                    handle_color: (THEME_COLOR_HANDLE)
                    handle_color_hover: (THEME_COLOR_HANDLE_HOVER)
                    handle_color_focus: (THEME_COLOR_HANDLE_FOCUS)
                    handle_color_drag: (THEME_COLOR_HANDLE_DRAG)
                    handle_color_disabled: (THEME_COLOR_HANDLE_DISABLED)

                    border_color_1: (#088)
                    border_color_1_hover: (#0BB)
                    border_color_1_focus: (#0AA)
                    border_color_1_disabled: (#04)
                    border_color_1_drag: (#066)

                    border_color_2: (THEME_COLOR_BEVEL_INSET_1)
                    border_color_2_hover: (THEME_COLOR_BEVEL_INSET_1_HOVER)
                    border_color_2_focus: (THEME_COLOR_BEVEL_INSET_1_FOCUS)
                    border_color_2_disabled: (THEME_COLOR_BEVEL_INSET_1_DISABLED)
                    border_color_2_drag: (THEME_COLOR_BEVEL_INSET_1_DRAG)

                }

            }
            <SliderRound> {
                text: "Solid",
                draw_text: {
                    color: #0ff;
                }
                draw_bg: {
                    val_color_1: #F08
                    val_color_1_hover: #F4A
                    val_color_1_focus: #C04
                    val_color_1_drag: #F08

                    val_color_2: #F08
                    val_color_2_hover: #F4A
                    val_color_2_focus: #C04
                    val_color_2_drag: #F08

                    handle_color: #F
                    handle_color_hover: #F
                    handle_color_focus: #F
                    handle_color_drag: #F
                }
            }
            <SliderRound> {
                text: "Solid",
                draw_bg: {
                    val_color_1: #6,
                    val_color_2: #6,
                    handle_color: #0,
                }
            }
            <SliderRound> { text: "min/max", min: 0., max: 100. }
            <SliderRound> { text: "precision", precision: 20 }
            <SliderRound> { text: "stepped", step: 0.1 }
            <SliderRound> {
                text: "label_size",
                draw_bg: {label_size: 150. },
            }

            <Hr> {}
            <H4> { text: "SliderRoundGradientY"}
            <SliderRoundGradientY> { text: "min/max", min: 0., max: 100. }
            <SliderRoundGradientY> { text: "precision", precision: 20 }
            <SliderRoundGradientY> { text: "stepped", step: 0.1 }

            <Hr> {}
            <H4> { text: "SliderRoundFlat"}
            <SliderRoundFlat> { text: "min/max", min: 0., max: 100. }
            <SliderRoundFlat> { text: "precision", precision: 20 }
            <SliderRoundFlat> { text: "stepped", step: 0.1 }

            <Hr> {}
            <H4> { text: "SliderRoundFlatter"}
            <SliderRoundFlatter> { text: "min/max", min: 0., max: 100. }
            <SliderRoundFlatter> { text: "precision", precision: 20 }
            <SliderRoundFlatter> { text: "stepped", step: 0.1 }
        }
    }
}
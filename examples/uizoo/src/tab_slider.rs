use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoSlider = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# Slider\n\nSliders allow selecting numeric values."}
        }
        demos +: {
            H4{text: "Slider"}
            Slider{text: "Default"}
            Slider{
                text: "Default, disabled"
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }
            Slider{text: "min/max" min: 0. max: 100.}
            Slider{text: "precision" precision: 20}
            Slider{text: "stepped" step: 0.1}

            Hr{}
            H4{text: "SliderGradientY"}
            SliderGradientY{text: "Default"}
            SliderGradientY{
                text: "disabled"
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }
            SliderGradientY{text: "min/max" min: 0. max: 100.}
            SliderGradientY{text: "precision" precision: 20}
            SliderGradientY{text: "stepped" step: 0.1}

            Hr{}
            H4{text: "SliderGradientX"}
            SliderGradientX{text: "Default"}
            SliderGradientX{
                text: "disabled"
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }
            SliderGradientX{text: "min/max" min: 0. max: 100.}
            SliderGradientX{text: "precision" precision: 20}
            SliderGradientX{text: "stepped" step: 0.1}

            Hr{}
            H4{text: "SliderFlat"}
            SliderFlat{text: "Default"}
            SliderFlat{
                text: "disabled"
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }
            SliderFlat{text: "min/max" min: 0. max: 100.}
            SliderFlat{text: "precision" precision: 20}
            SliderFlat{text: "stepped" step: 0.1}

            Hr{}
            H4{text: "SliderMinimal"}
            SliderMinimal{text: "Default"}
            SliderMinimal{
                text: "Default, disabled"
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }
            SliderMinimal{text: "min/max" min: 0. max: 100.}
            SliderMinimal{text: "precision" precision: 20}
            SliderMinimal{text: "stepped" step: 0.1}

            Hr{}
            H4{text: "SliderMinimalFlat"}
            SliderMinimalFlat{text: "Default"}
            SliderMinimalFlat{
                text: "disabled"
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }
            SliderMinimalFlat{text: "min/max" min: 0. max: 100.}
            SliderMinimalFlat{text: "precision" precision: 20}
            SliderMinimalFlat{text: "stepped" step: 0.1}

            Hr{}
            H4{text: "SliderRound"}
            SliderRound{text: "Default"}
            SliderRound{
                text: "Disabled"
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }
            SliderRound{
                text: "Solid"
                draw_text +: {
                    color: #0ff
                }
                draw_bg +: {
                    val_color: uniform(#xF08)
                    val_color_hover: uniform(#xF4A)
                    val_color_focus: uniform(#xC04)
                    val_color_drag: uniform(#xF08)

                    val_color_2: uniform(#xF08)
                    val_color_2_hover: uniform(#xF4A)
                    val_color_2_focus: uniform(#xC04)
                    val_color_2_drag: uniform(#xF08)

                    handle_color: uniform(#xF)
                    handle_color_hover: uniform(#xF)
                    handle_color_focus: uniform(#xF)
                    handle_color_drag: uniform(#xF)
                }
            }
            SliderRound{
                text: "Solid"
                draw_bg +: {
                    val_color: uniform(#6)
                    val_color_2: uniform(#6)
                    handle_color: uniform(#0)
                }
            }
            SliderRound{text: "min/max" min: 0. max: 100.}
            SliderRound{text: "precision" precision: 20}
            SliderRound{text: "stepped" step: 0.1}
            SliderRound{
                text: "label_size"
                draw_bg +: {label_size: 150.}
            }

            Hr{}
            H4{text: "SliderRoundGradientY"}
            SliderRoundGradientY{text: "min/max" min: 0. max: 100.}
            SliderRoundGradientY{
                text: "min/max"
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }
            SliderRoundGradientY{text: "precision" precision: 20}
            SliderRoundGradientY{text: "stepped" step: 0.1}

            Hr{}
            H4{text: "SliderRoundGradientX"}
            SliderRoundGradientX{text: "min/max" min: 0. max: 100.}
            SliderRoundGradientX{
                text: "min/max"
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }
            SliderRoundGradientX{text: "precision" precision: 20}
            SliderRoundGradientX{text: "stepped" step: 0.1}

            Hr{}
            H4{text: "SliderRoundFlat"}
            SliderRoundFlat{text: "min/max" min: 0. max: 100.}
            SliderRoundFlat{
                text: "disabled"
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }
            SliderRoundFlat{text: "precision" precision: 20}
            SliderRoundFlat{text: "stepped" step: 0.1}
        }
    }
}

use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoRotary = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# Rotary\n\nRotary controls allow selecting values with a circular dial."}
        }
        demos +: {
            H4{text: "Rotary"}
            UIZooRowH{
                align: Align{x: 0. y: 0.}

                Rotary{text: "Label"}

                Rotary{
                    text: "Label"
                    draw_bg +: {
                        val_size: 10.
                        val_padding: 2.
                        gap: 0.
                    }
                }

                Rotary{
                    text: "Label"
                    draw_bg +: {
                        val_size: 5.
                        val_padding: 2.5
                        gap: 180.
                    }
                }

                Rotary{
                    text: "Label"
                    draw_bg +: {
                        val_size: 5.
                        val_padding: 0.
                        gap: 180.
                    }
                    animator +: {
                        disabled: {
                            default: @on
                        }
                    }
                }

                Rotary{
                    width: Fill
                    height: 150
                    text: "Label"
                    draw_bg +: {
                        val_size: 10.
                        val_padding: 5.
                    }
                }
            }

            Hr{}
            H4{text: "RotaryGradientY"}
            UIZooRowH{
                align: Align{x: 0. y: 0.}
                RotaryGradientY{text: "Label"}
                RotaryGradientY{
                    text: "Label"
                    draw_bg +: {gap: 0.}
                }
                RotaryGradientY{
                    text: "Label"
                    draw_bg +: {gap: 180.}
                }
                RotaryGradientY{
                    text: "Label"
                    animator +: {
                        disabled: {
                            default: @on
                        }
                    }
                    draw_bg +: {val_size: 20.}
                }
                RotaryGradientY{
                    width: Fill
                    height: 150
                    text: "Label"
                    draw_bg +: {
                        val_size: 10.
                        val_padding: 5.
                    }
                }
            }

            Hr{}
            H4{text: "RotaryFlat"}
            UIZooRowH{
                align: Align{x: 0. y: 0.}
                RotaryFlat{text: "Label"}
                RotaryFlat{
                    text: "Label"
                    draw_bg +: {gap: 0.}
                }
                RotaryFlat{
                    text: "Label"
                    draw_bg +: {gap: 180.}
                }
                RotaryFlat{
                    text: "Label"
                    animator +: {
                        disabled: {
                            default: @on
                        }
                    }
                    draw_bg +: {val_size: 10.}
                }
                RotaryFlat{
                    width: Fill
                    height: 150
                    text: "Label"
                    draw_bg +: {
                        val_size: 10.
                        val_padding: 8.
                    }
                }
            }
        }
    }
}

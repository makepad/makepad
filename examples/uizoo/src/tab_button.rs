use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoButton = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# Button\n\nButtons trigger actions when clicked."}
        }
        demos +: {
            H4{text: "Standard"}
            UIZooRowH{
                Button{}
                Button{
                    draw_bg +: {
                        color_2: #f00
                        color_2_hover: #f00
                        color_2_down: #f00
                        color_2_focus: #f00
                        color_2_disabled: #f00

                        border_color_2: #f00
                        border_color_2_hover: #f00
                        border_color_2_down: #f00
                        border_color_2_focus: #f00
                        border_color_2_disabled: #f00
                    }
                }

                basicbutton := Button{}

                iconbutton := Button{
                    draw_icon +: {
                        gradient_fill_horizontal: instance(1.0)
                        color: #f00
                        color_2: #00f
                        svg: crate_resource("self:resources/Icon_Favorite.svg")
                    }
                    text: "Button"
                }
            }

            Hr{}
            H4{text: "Standard, disabled"}
            UIZooRowH{
                Button{
                    text: "Button"
                    animator +: {
                        disabled: {
                            default: @on
                        }
                    }
                }
            }

            Hr{}
            H4{text: "ButtonIcon"}
            UIZooRowH{
                ButtonIcon{
                    draw_icon +: {
                        gradient_fill_horizontal: instance(1.0)
                        color: #f00
                        color_2: #00f
                        svg: crate_resource("self:resources/Icon_Favorite.svg")
                    }
                }
            }

            Hr{}
            H4{text: "GradientX"}
            UIZooRowH{
                ButtonGradientX{text: "ButtonGradientX"}
                ButtonGradientX{
                    draw_bg +: {
                        border_radius: 4.0

                        color: #xC00
                        color_hover: #xF0F
                        color_down: #800

                        color_2: #x0CC
                        color_2_hover: #x0FF
                        color_2_down: #088

                        border_color: #xC
                        border_color_hover: #xF
                        border_color_down: #0

                        border_color_2: #3
                        border_color_2_hover: #6
                        border_color_2_down: #8
                    }
                    text: "ButtonGradientX"
                }
            }

            Hr{}
            H4{text: "ButtonGradientXIcon"}
            UIZooRowH{
                ButtonGradientXIcon{
                    draw_icon +: {
                        color: #f00
                        svg: crate_resource("self:resources/Icon_Favorite.svg")
                    }
                }
            }

            Hr{}
            H4{text: "GradientY"}
            UIZooRowH{
                ButtonGradientY{text: "ButtonGradientY"}
                ButtonGradientY{
                    draw_bg +: {
                        border_radius: 4.0

                        color: #xC00
                        color_hover: #xF0F
                        color_down: #800

                        color_2: #x0CC
                        color_2_hover: #x0FF
                        color_2_down: #088

                        border_color: #xC
                        border_color_hover: #xF
                        border_color_down: #0

                        border_color_2: #3
                        border_color_2_hover: #6
                        border_color_2_down: #8
                    }
                    text: "ButtonGradientY"
                }
            }

            Hr{}
            H4{text: "ButtonGradientYIcon"}
            UIZooRowH{
                ButtonGradientYIcon{
                    draw_icon +: {
                        color: #f00
                        svg: crate_resource("self:resources/Icon_Favorite.svg")
                    }
                }
            }

            Hr{}
            H4{text: "Flat"}
            UIZooRowH{
                ButtonFlat{
                    draw_icon +: {
                        color: #f00
                        svg: crate_resource("self:resources/Icon_Favorite.svg")
                    }
                    text: "ButtonFlat"
                }

                ButtonFlat{
                    flow: Down
                    icon_walk: Walk{width: 15.}
                    draw_icon +: {
                        svg: crate_resource("self:resources/Icon_Favorite.svg")
                    }
                    text: "ButtonFlat"
                }
            }

            Hr{}
            H4{text: "ButtonFlatIcon"}
            UIZooRowH{
                ButtonFlatIcon{
                    draw_icon +: {
                        color: #f00
                        svg: crate_resource("self:resources/Icon_Favorite.svg")
                    }
                }
            }

            Hr{}
            H4{text: "Flatter"}
            UIZooRowH{
                ButtonFlatter{
                    draw_icon +: {
                        color: #f00
                        svg: crate_resource("self:resources/Icon_Favorite.svg")
                    }
                    text: "ButtonFlatter"
                }
            }

            Hr{}
            H4{text: "ButtonFlatterIcon"}
            UIZooRowH{
                ButtonFlatterIcon{
                    draw_icon +: {
                        color: #f00
                        svg: crate_resource("self:resources/Icon_Favorite.svg")
                    }
                }
            }
        }
    }
}

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
                        color_2: uniform(#f00)
                        color_2_hover: uniform(#f00)
                        color_2_down: uniform(#f00)
                        color_2_focus: uniform(#f00)
                        color_2_disabled: uniform(#f00)

                        border_color_2: uniform(#f00)
                        border_color_2_hover: uniform(#f00)
                        border_color_2_down: uniform(#f00)
                        border_color_2_focus: uniform(#f00)
                        border_color_2_disabled: uniform(#f00)
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
                        border_radius: uniform(4.0)

                        color: uniform(#xC00)
                        color_hover: uniform(#xF0F)
                        color_down: uniform(#800)

                        color_2: uniform(#x0CC)
                        color_2_hover: uniform(#x0FF)
                        color_2_down: uniform(#088)

                        border_color: uniform(#xC)
                        border_color_hover: uniform(#xF)
                        border_color_down: uniform(#0)

                        border_color_2: uniform(#3)
                        border_color_2_hover: uniform(#6)
                        border_color_2_down: uniform(#8)
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
                        border_radius: uniform(4.0)

                        color: uniform(#xC00)
                        color_hover: uniform(#xF0F)
                        color_down: uniform(#800)

                        color_2: uniform(#x0CC)
                        color_2_hover: uniform(#x0FF)
                        color_2_down: uniform(#088)

                        border_color: uniform(#xC)
                        border_color_hover: uniform(#xF)
                        border_color_down: uniform(#0)

                        border_color_2: uniform(#3)
                        border_color_2_hover: uniform(#6)
                        border_color_2_down: uniform(#8)
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

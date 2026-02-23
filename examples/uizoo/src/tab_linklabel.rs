use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoLinkLabel = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# LinkLabel\n\nLinkLabels are clickable text links."}
        }
        demos +: {
            H4{text: "Standard"}
            UIZooRowH{
                LinkLabel{text: "Click me!"}
            }

            Hr{}
            H4{text: "Standard, disabled"}
            UIZooRowH{
                LinkLabel{
                    text: "Click me!"
                    animator +: {
                        disabled: {
                            default: @on
                        }
                    }
                }
            }

            Hr{}
            H4{text: "Styling Attributes Reference"}
            UIZooRowH{
                LinkLabel{
                    draw_text +: {
                        color: #xA
                        color_hover: #xC
                        color_down: #8
                        text_style +: {
                            font_size: 20.
                            line_spacing: 1.4
                        }

                    }

                    draw_bg +: {
                        color: uniform(#x0A0)
                        color_hover: uniform(#x0C0)
                        color_down: uniform(#080)
                    }

                    icon_walk: Walk{
                        width: 20.
                        height: Fit
                    }

                    draw_icon +: {
                        color: #xA00
                        color_hover: #xC00
                        color_down: #800
                        svg: crate_resource("self:resources/Icon_Favorite.svg")
                    }

                    text: "Click me!"
                }
            }
        }
    }
}

use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoIcon = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# Icon\n\nIcons display SVG vector graphics."}
        }
        demos +: {
            H4{text: "Standard"}
            Icon{
                draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
            }

            Hr{}
            H4{text: "IconGradientX"}
            IconGradientX{
                icon_walk: Walk{width: 100.}
                draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
            }

            Hr{}
            H4{text: "IconGradientY"}
            IconGradientY{
                icon_walk: Walk{width: 100.}
                draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
            }

            H4{text: "Styling Attributes Reference"}
            Icon{
                width: Fit
                height: Fit
                icon_walk: Walk{
                    width: 50.
                    margin: 10.
                }
                draw_bg +: {color: uniform(#f00)}
                draw_icon +: {
                    svg: crate_resource("self:resources/Icon_Favorite.svg")
                    color: #f0f
                    color_2: #ff0
                }
            }
        }
    }
}

use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoScrollBar = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# ScrollBar\n\nScrollBars enable scrolling through content."}
        }
        demos +: {
            GradientYView{
                height: 4000.
                width: Fill
                draw_bg +: {
                    // A plain value: a `uniform(..)` here redeclared the input and the red end never drew.
                    color_2: #f00
                }
            }
            scroll_bars: ScrollBars{
                scroll_bar_x.drag_scrolling: true
                scroll_bar_y.drag_scrolling: true
            }
        }
    }
}

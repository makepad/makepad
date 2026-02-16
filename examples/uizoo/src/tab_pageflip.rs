use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoPageFlip = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# PageFlip\n\nPageFlip switches between pages."}
        }
        demos +: {
            View{
                height: Fit width: Fill
                flow: Right
                spacing: theme.space_2
                pageflipbutton_a := Button{text: "Page A"}
                pageflipbutton_b := Button{text: "Page B"}
                pageflipbutton_c := Button{text: "Page C"}
            }

            page_flip := PageFlip{
                width: Fill height: Fill
                flow: Down
                active_page: @page_a

                page_a := View{
                    align: Align{x: 0.5 y: 0.5}
                    show_bg: true
                    draw_bg +: {color: uniform(#f00)}
                    width: Fill height: Fill
                    H3{width: Fit text: "Page A"}
                }

                page_b := View{
                    align: Align{x: 0.5 y: 0.5}
                    show_bg: true
                    draw_bg +: {color: uniform(#080)}
                    width: Fill height: Fill
                    H3{width: Fit text: "Page B"}
                }

                page_c := View{
                    align: Align{x: 0.5 y: 0.5}
                    show_bg: true
                    draw_bg +: {color: uniform(#008)}
                    width: Fill height: Fill
                    H3{width: Fit text: "Page C"}
                }
            }
        }
    }
}

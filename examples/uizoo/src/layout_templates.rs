use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*

    mod.widgets.UIZooTabLayout_B = View{
        height: Fill width: Fill
        flow: Right
        padding: 0
        spacing: 0.

        desc := RoundedView{
            width: 350. height: Fill
            show_bg: true
            draw_bg +: {
                color: theme.color_inset
                border_radius: uniform(theme.corner_radius)
            }
            padding: theme.mspace_3{top: 0. right: theme.space_2}
            margin: theme.mspace_v_2

            flow: Down
            spacing: theme.space_2
            scroll_bars: ScrollBars{show_scroll_x: false show_scroll_y: true}
        }

        demos := View{
            width: Fill height: Fill
            flow: Down
            spacing: theme.space_2
            padding: theme.mspace_3{right: (theme.space_2 * 3.0)}
            margin: theme.mspace_v_2
            scroll_bars: ScrollBars{show_scroll_x: false show_scroll_y: true}
        }
    }

    mod.widgets.UIZooRowH = View{
        height: Fit width: Fill
        spacing: theme.space_2
        flow: Right
        align: Align{x: 0. y: 0.5}
    }
}

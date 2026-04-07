use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let AlignScrollBox = RoundedView{
        show_bg: true
        draw_bg +: {
            color: uniform(#x0F02)
            border_size: uniform(1.)
            border_radius: uniform(0.)
            border_color: uniform(#xfff8)
        }
        padding: 3.
        align: Align{x: 0.5 y: 0.5}
    }

    let ScrollContainer = SolidView{
        draw_bg +: {
            color: (#x1a1a2e)
        }
    }

    mod.widgets.DemoAlignScroll = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# Align + Scroll\n\nDemonstrates that views with centered alignment (`align: {x: 0.5}`, `align: {y: 0.5}`) can still be scrolled when their content overflows.\n\nPreviously, per-walk alignment offsets could become negative when a child exceeded its container, shifting content in the wrong direction and fighting scroll.\n\nThe fix clamps per-walk alignment to zero when the child overflows, so alignment only applies when content fits and scrolling works unimpeded when it doesn't."}
        }
        demos +: {
            // ── Flow::Down with vertical centering + vertical scroll ──
            H4{text: "Flow: Down, align: {y: 0.5}, vertical scroll"}
            P{text: "Children are centered when they fit. Scroll vertically to see all content."}
            ScrollContainer{
                width: Fill height: 200.
                flow: Down
                align: Align{x: 0.5 y: 0.5}
                scroll_bars: ScrollBars{show_scroll_x: false show_scroll_y: true}
                AlignScrollBox{width: 120. height: 80. P{width: Fit text: "Box 1"}}
                AlignScrollBox{width: 120. height: 80. P{width: Fit text: "Box 2"}}
                AlignScrollBox{width: 120. height: 80. P{width: Fit text: "Box 3"}}
                AlignScrollBox{width: 120. height: 80. P{width: Fit text: "Box 4"}}
                AlignScrollBox{width: 120. height: 80. P{width: Fit text: "Box 5"}}
            }

            Hr{}

            // ── Flow::Down with horizontal centering + horizontal scroll ──
            H4{text: "Flow: Down, align: {x: 0.5}, horizontal scroll"}
            P{text: "Wide children should be scrollable horizontally even with x-centering."}
            ScrollContainer{
                width: 250. height: Fit
                flow: Down
                align: Align{x: 0.5 y: 0.0}
                scroll_bars: ScrollBars{show_scroll_x: true show_scroll_y: false}
                AlignScrollBox{width: 400. height: 40. P{width: Fit text: "Wide box 1 — 400px in a 250px container"}}
                AlignScrollBox{width: 150. height: 40. P{width: Fit text: "Narrow box 2"}}
                AlignScrollBox{width: 400. height: 40. P{width: Fit text: "Wide box 3 — also 400px wide"}}
            }

            Hr{}

            // ── Flow::Right with vertical centering + vertical scroll ──
            H4{text: "Flow: Right, align: {y: 0.5}, vertical scroll"}
            P{text: "Tall children in a horizontal flow should be scrollable vertically."}
            ScrollContainer{
                width: Fill height: 150.
                flow: Right
                align: Align{x: 0.0 y: 0.5}
                scroll_bars: ScrollBars{show_scroll_x: false show_scroll_y: true}
                AlignScrollBox{width: 100. height: 50. P{width: Fit text: "Short"}}
                AlignScrollBox{width: 100. height: 250. P{width: Fit text: "Tall (250px)"}}
                AlignScrollBox{width: 100. height: 50. P{width: Fit text: "Short"}}
            }

            Hr{}

            // ── Flow::Right with horizontal centering + horizontal scroll ──
            H4{text: "Flow: Right, align: {x: 0.5}, horizontal scroll"}
            P{text: "Many children in a horizontal flow should be scrollable horizontally."}
            ScrollContainer{
                width: 300. height: Fit
                flow: Right
                align: Align{x: 0.5 y: 0.5}
                scroll_bars: ScrollBars{show_scroll_x: true show_scroll_y: false}
                AlignScrollBox{width: 100. height: 60. P{width: Fit text: "A"}}
                AlignScrollBox{width: 100. height: 60. P{width: Fit text: "B"}}
                AlignScrollBox{width: 100. height: 60. P{width: Fit text: "C"}}
                AlignScrollBox{width: 100. height: 60. P{width: Fit text: "D"}}
                AlignScrollBox{width: 100. height: 60. P{width: Fit text: "E"}}
            }

            Hr{}

            // ── Flow::Overlay with centering + scroll ──
            H4{text: "Flow: Overlay, align: {x: 0.5, y: 0.5}, both scrolls"}
            P{text: "An oversized child in an Overlay layout should be scrollable in both axes."}
            ScrollContainer{
                width: 250. height: 150.
                flow: Overlay
                align: Align{x: 0.5 y: 0.5}
                scroll_bars: ScrollBars{show_scroll_x: true show_scroll_y: true}
                AlignScrollBox{
                    width: 400. height: 300.
                    P{width: Fit text: "400x300 box in a 250x150 container.\nScroll both directions to see all content."}
                }
            }

            Hr{}

            // ── Centering works when content fits ──
            H4{text: "Centering still works when content fits"}
            P{text: "When content is smaller than its container, it is still properly centered."}
            ScrollContainer{
                width: Fill height: 150.
                flow: Down
                align: Align{x: 0.5 y: 0.5}
                scroll_bars: ScrollBars{show_scroll_x: false show_scroll_y: true}
                AlignScrollBox{width: 120. height: 40. P{width: Fit text: "Centered"}}
            }
        }
    }
}

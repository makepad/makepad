//! The charts story: six plots of the same shape, and the thing an unfed
//! chart does instead of looking unfed.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Plot = View{
        width: Fill
        height: 180.
        flow: Down
        spacing: theme.space_1
        caption := Label{text: "" draw_text +: {color: theme.color_text_meta}}
    }

    mod.stories.ChartOverview = StoryPage{
        StoryNote{text: "Six ways of plotting a series, one small one for a cell, and one for a dashboard tile. They share a viewport and a set of colours, and differ in what they draw at each point."}

        StoryHeading{text: "The six full plots"}
        StoryNote{text: "Line and area over a series of points; bars and dots over the same; candles and open-high-low-close bars over a series of candles. Each one fits its own viewport to the data it holds."}
        StoryRow{
            Plot{caption: Label{text: "LineChart"} LineChart{}}
            Plot{caption: Label{text: "AreaChart"} AreaChart{}}
        }
        StoryRow{
            Plot{caption: Label{text: "BarChart"} BarChart{}}
            Plot{caption: Label{text: "ScatterChart"} ScatterChart{}}
        }
        StoryRow{
            Plot{caption: Label{text: "CandlestickChart"} CandlestickChart{}}
            Plot{caption: Label{text: "OhlcChart"} OhlcChart{}}
        }

        StoryHeading{text: "The two small ones"}
        StoryNote{text: "A Sparkline is meant for a table cell: no axes, no grid, no margins, just the shape. A TrendChart is the dashboard tile version, with a grid and a filled area under the line."}
        StoryRow{
            View{
                width: Fill height: 60.
                spark := Sparkline{}
            }
        }
        StoryRow{
            View{
                width: Fill height: 160.
                trend := TrendChart{}
            }
        }

        StoryHeading{text: "A chart with no data does not look like one"}
        StoryNote{text: "Every plot on this page was declared empty. On its first draw a chart that holds no data fabricates some — two hundred points of a sine wave with noise, or five hundred candles walked from a starting price — and draws that instead. It is a fixed seed, so it is the same invented series every time, which is why this page is stable enough to screenshot. It also means a chart whose data never arrived shows a plausible one, and looks exactly like a chart that is working."}
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "data-display/charts/overview",
    category: "Data display",
    component: "Charts",
    also: &[
        "ChartView",
        "LineChart",
        "AreaChart",
        "BarChart",
        "ScatterChart",
        "CandlestickChart",
        "OhlcChart",
        "Sparkline",
        "TrendChart",
    ],
    name: "Overview",
    dsl: "ChartOverview",
    added: "2026-02-12",
    tags: &[],
    doc: "# Charts

Eight plotting widgets over a shared viewport: `LineChart`, `AreaChart`, `BarChart` and `ScatterChart` over a series of points; `CandlestickChart` and `OhlcChart` over a series of candles; `Sparkline` for a table cell and `TrendChart` for a dashboard tile. `ChartView` is the surface underneath them all — the grid, the axes, the margins and the whole colour list live there.

Feed one with `set_series` for points, `set_candles` for candles, `set_values` for a sparkline. Each fits its viewport to what it holds the first time it draws.

**An unfed chart does not look unfed.** On its first draw, a chart holding no data calls one of the library's `generate_fake_*` functions and plots that: two hundred points of a sine wave with noise, or five hundred candles walked from a starting price. Every plot on this page is empty, and every plot on this page has a curve in it. The seed is fixed, so the invented series is the same every time — which is what makes this page stable enough to screenshot, and also what makes a chart whose data never arrived indistinguishable from one that is working. If you are wondering why your chart looks plausible but wrong, that is the first thing to check.

**The colours are literals, and they are dark.** `bg_color: #x1a1a2e`, `grid_color: #x2a2a3e`, the up and down greens and reds — none of it comes from the theme, so a chart is a dark panel whatever the page around it is doing. This page leaves them alone rather than restating them, because what a caller gets by writing `LineChart{}` is exactly what is drawn here. In a light theme these are eight dark rectangles, and that is the honest picture.",
    subject: "spark",
    feature: None,
    controls: &[],
    on_actions: None,
}];

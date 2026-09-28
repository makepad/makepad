//! The charts story: a dashboard tile with several lines, six plots of the
//! same shape, a sparkline, and the thing an unfed chart does instead of
//! looking unfed.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

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

    mod.storybook.StorySparklineBase = #(StorySparkline::register_widget(vm))

    // Cell sized, because a cell is what it is for.
    mod.storybook.StorySparkline = set_type_default() do mod.storybook.StorySparklineBase{
        width: 240.
        height: 40.
        spark := Sparkline{}
    }

    mod.stories.ChartOverview = StoryPage{
        StoryNote{text: "Six ways of plotting a series against axes, one small plot for a table cell, and one for a dashboard tile. They share a viewport and a set of colours, and differ in what they draw at each point. Pictures of how a whole divides are on the Shapes page."}

        StoryHeading{text: "A dashboard tile with several lines"}
        StoryNote{text: "A dashboard tile with two or three lines and a key is the most ordinary chart there is. TrendChart draws it: several lines over one plot, each in its own colour, a key at the top left when asked for, and a value axis that can be pinned so one tile compares with the next."}
        StoryRow{
            View{
                width: Fill height: 230.
                subject := TrendChart{
                    show_legend: true
                    series: [
                        "cpu 32 35 41 38 52 61 57 49 44 58 72 69 63 55 47 51"
                        "memory 61 62 62 64 66 65 67 70 71 71 73 74 74 76 77 78"
                        "disk 12 14 11 9 15 22 19 17 13 10 12 16 21 18 14 12"
                    ]
                }
            }
        }
        StoryNote{text: "One line of markup per plotted line — a name, then its numbers — read exactly as the chart shapes read theirs. Every line shares the one axis, which covers them all. The key names each line beside a swatch of its colour, on a panel of the chart's own background so the lines do not run through the words, and it is as wide as its names measure in the face they are drawn in. With several lines the dashed last-value rule takes each line's colour, because it has to say which line it belongs to."}

        StoryHeading{text: "An axis you can pin"}
        StoryNote{text: "Left, the axis fits the data, with a little room above and below. Middle and right, the same line over two consecutive frames with the axis pinned from nought to a hundred: the same value sits at the same height in both, so the eye reads the change between them rather than a rescale. A fitted axis would have redrawn the second frame taller and told the reader nothing."}
        StoryRow{
            Plot{
                caption: Label{text: "fitted"}
                TrendChart{
                    series: ["cpu 32 35 41 38 52 61 57 49 44 58 72 69 63 55 47 51"]
                }
            }
            Plot{
                caption: Label{text: "pinned 0 to 100"}
                TrendChart{
                    range_min: 0.0
                    range_max: 100.0
                    series: ["cpu 32 35 41 38 52 61 57 49 44 58 72 69 63 55 47 51"]
                }
            }
            Plot{
                caption: Label{text: "pinned 0 to 100, the next frame"}
                TrendChart{
                    range_min: 0.0
                    range_max: 100.0
                    series: ["cpu 35 41 38 52 61 57 49 44 58 72 69 63 55 47 51 46"]
                }
            }
        }

        StoryHeading{text: "The key is off unless asked for"}
        StoryNote{text: "The same two lines with show_legend left alone. A legend is a list of labels beside a picture, which is a layout decision belonging to whatever places the chart; a tile with no room beside it is the one case that earns the key above, and it has to ask."}
        StoryRow{
            View{
                width: Fill height: 170.
                TrendChart{
                    series: [
                        "cpu 32 35 41 38 52 61 57 49 44 58 72 69 63 55 47 51"
                        "memory 61 62 62 64 66 65 67 70 71 71 73 74 74 76 77 78"
                    ]
                }
            }
        }

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

        StoryHeading{text: "A plot for a table cell"}
        StoryNote{text: "A Sparkline has no axes, no grid and no margins, just the shape: a bar per number rising from the lowest of them, tinted by whether the series ended above where it began. Its numbers come from Rust with set_values, and this page hands it twenty-four."}
        StoryRow{
            spark_demo := mod.storybook.StorySparkline{}
        }

        StoryHeading{text: "A chart with no data"}
        StoryNote{text: "The six full plots above were declared empty. On its first draw each of them, holding no data, fabricates some — two hundred points of a sine wave with noise, or five hundred candles walked from a starting price — and draws that instead. It is a fixed seed, so it is the same invented series every time, which is why this page is stable enough to screenshot. It also means a chart whose data never arrived shows a plausible one, and looks exactly like a chart that is working."}
        StoryNote{text: "The small two do not. A TrendChart handed no lines draws its background and stops, and a Sparkline handed fewer than two numbers draws nothing at all."}
        StoryRow{
            Plot{
                height: 110.
                caption: Label{text: "TrendChart, no lines"}
                TrendChart{}
            }
            Plot{
                height: 110.
                caption: Label{text: "Sparkline, no numbers"}
                Sparkline{}
            }
        }
    }
}

/// The sparkline's numbers: a series that ends above where it began, so it
/// takes the rising colour.
const SPARK: &[f64] = &[
    12., 14., 13., 17., 16., 19., 22., 21., 24., 23., 26., 25., 28., 27., 31., 30., 29., 33., 35., 34., 37., 36., 39., 41.,
];

/// A sparkline with numbers in it. It has no markup for them, so they are
/// handed over from here before each draw.
#[derive(Script, ScriptHook, Widget)]
pub struct StorySparkline {
    #[deref]
    view: View,
}

impl Widget for StorySparkline {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if let Some(mut spark) = self.view.widget(cx, ids!(spark)).borrow_mut::<Sparkline>() {
            spark.set_values(SPARK);
        }
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
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
    tags: &["trend", "lines", "series", "legend", "key", "axis", "pin", "dashboard"],
    doc: "# Charts

Eight plotting widgets over a shared viewport: `LineChart`, `AreaChart`, `BarChart` and `ScatterChart` over a series of points; `CandlestickChart` and `OhlcChart` over a series of candles; `Sparkline` for a table cell and `TrendChart` for a dashboard tile. `ChartView` is the surface underneath the six full plots — the grid, the axes, the margins and the whole colour list live there. Pictures of how a whole divides are on the Shapes page.

Feed the six full plots with `set_data`, points for the first four and candles for the other two; a sparkline with `set_values`; and a trend tile with `series` markup, `set_rows`, or `set_series` for one line with no name. A full plot fits its viewport to what it holds the first time it draws after being fed; a trend tile fits its axis on every draw unless the axis is pinned.

## Several lines on a TrendChart

`TrendChart` is the dashboard tile: a grid, a right-hand gutter of tick labels, a filled area under a line and a dashed rule at its last value. It draws several lines over the one plot, a key naming them when asked, and a value axis that can be pinned. All three live on the tile that already owns the grid, the gutter and the colours, rather than in a widget or a shader of their own.

### The data is lines of markup

One line per plotted line: `\"cpu 32 35 41 38\"`. The words are the name and the numbers are the numbers, read by the same `parse_row` the chart shapes use, so a colon says exactly where a name stops and a name may hold a number of its own. `set_rows` takes the same thing from Rust; `set_series` is one line with no name.

Every line shares the one value axis, which covers them all, and the one spacing along the bottom: the longest line sets the spacing and a shorter one stops short. That is the true picture of fewer samples so far, and not a line stretched to fit.

### Colour

The first line is `color_line` over `color_fill`, so a tile with one line is styled by those two alone. The second, third and fourth are `color_line_2`, `_3` and `_4`, each over its own colour at `color_fill`'s opacity — a transparent fill is transparent for every line — and a fifth starts round again rather than fading past the fourth: two lines the same colour are honestly ambiguous, where two nearly the same colour look distinguishable and are not.

With one line the last-value rule and its number are `color_accent`. With several they have to say WHICH line, so each takes its line's colour.

### The key

`show_legend` draws a swatch and a name for every named line at the top left of the plot, on a panel of the chart's own background so the lines do not run through the words. **It is off unless asked for.** A legend is a list of labels beside a picture, which is a layout decision belonging to whatever places the chart. The one exception is a tile with no room beside it for anything, and that is the widget this is. A named line with no numbers yet is still in the key: it is a line the host wrote down.

The panel is as wide as its names measure in the face they are drawn in. A key sized by a count of characters is the right width for exactly one font, and clips or gapes in every other.

### A pinned axis

By default the axis fits the data on every draw, with eight per cent of the span as room above and below. `range_max` above `range_min` pins it to exactly that span, with no room added, because the pin is what the host asked for: two tiles pinned to the same numbers put the same value at the same height, which is what lets one frame be read against the next. A value beyond a pinned axis is drawn pegged to the edge it left by rather than off the chart. A top not above its bottom is no pin at all, so `0` and `0` — the defaults — mean fit.

### What it deliberately does not do

It does not stack. Three lines are three lines, each read against the axis; a stacked area is a different chart with a different question.

It does not wrap the key. It is one row, and a key too wide for its tile is a tile too small for its key.

## A chart with no data

**The six full plots do not look unfed.** On its first draw, a line, area, bar, scatter, candlestick or OHLC chart holding no data calls one of the library's `generate_fake_*` functions and plots that: two hundred points of a sine wave with noise, or five hundred candles walked from a starting price. Those six on this page are declared empty, and every one of them has a curve in it. The seed is fixed, so the invented series is the same every time — which is what makes this page stable enough to screenshot, and also what makes a chart whose data never arrived indistinguishable from one that is working. If you are wondering why your chart looks plausible but wrong, that is the first thing to check.

**The two small ones invent nothing.** A `TrendChart` handed no lines draws its background and stops, and a `Sparkline` with fewer than two numbers draws nothing at all.

## The colours are literals, and they are dark

`bg_color: #x1a1a2e`, `grid_color: #x2a2a3e`, the up and down greens and reds, and the trend tile's own background, grid and line colours — none of it comes from the theme, so a chart is a dark panel whatever the page around it is doing. This page leaves them alone rather than restating them, because what a caller gets by writing `LineChart{}` is exactly what is drawn here. In a light theme these are dark rectangles, and that is the honest picture.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Key", target: "subject", kind: ControlKind::Bool { prop: "show_legend", default: true } },
        Control { label: "Line width", target: "subject", kind: ControlKind::Number { prop: "line_width", min: 0.5, max: 6., step: 0.5, default: 2. } },
        Control { label: "Axis floor", target: "subject", kind: ControlKind::Number { prop: "range_min", min: 0., max: 100., step: 5., default: 0. } },
        Control { label: "Axis ceiling", target: "subject", kind: ControlKind::Number { prop: "range_max", min: 0., max: 200., step: 5., default: 0. } },
        Control { label: "Second line", target: "subject", kind: ControlKind::Color { prop: "color_line_2", default: 0xFF8A65FF } },
        Control { label: "Third line", target: "subject", kind: ControlKind::Color { prop: "color_line_3", default: 0x81C784FF } },
    ],
    on_actions: None,
}];

#[cfg(test)]
mod tests {
    use super::*;

    /// The overview is built from the DSL, which the compiler never reads.
    /// Building it and reaching the tile the controls panel addresses is
    /// what turns a mistake in it into a failed build; and the tile's markup
    /// has to be three named lines of sixteen numbers, or the page shows
    /// fewer lines than the note beside it counts.
    #[test]
    fn the_overview_builds_and_its_tile_carries_three_named_lines() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
        });
        let story = STORIES.iter().find(|story| story.dsl == "ChartOverview").unwrap();
        let page = cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {}", story.dsl);
            WidgetRef::script_from_value(vm, value)
        });
        assert!(!page.is_empty(), "{} built no widget", story.key);
        assert_eq!(makepad_platform::shader_error::take(), None, "a draw shader failed to compile");
        // The subject and every control target, or the panel moves a slider
        // that reaches nothing.
        for target in std::iter::once(story.subject)
            .chain(story.controls.iter().map(|c| c.target))
            .filter(|t| !t.is_empty())
        {
            assert!(!page.widget(&cx, &[LiveId::from_str(target)]).is_empty(), "no widget at {target}");
        }
        let subject = page.widget(&cx, &[LiveId::from_str(story.subject)]);
        let chart = subject.borrow::<TrendChart>().expect("the subject is a TrendChart");
        assert!(chart.show_legend, "the page's tile asks for its key");
        let rows = parse_rows(&chart.series);
        let names: Vec<&str> = rows.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(names, ["cpu", "memory", "disk"]);
        assert!(rows.iter().all(|row| row.values.len() == 16), "every line is sixteen samples");
    }

    /// A control's default is what the panel shows before anyone touches
    /// it, and what Reset goes back to; a default its slider cannot reach
    /// is a value the panel can show once and never again.
    #[test]
    fn number_controls_can_hold_their_own_defaults() {
        for story in STORIES {
            for control in story.controls {
                if let ControlKind::Number { prop, min, max, step, default } = &control.kind {
                    assert!(min < max, "{} {}: min {} is not below max {}", story.key, prop, min, max);
                    assert!(*step > 0., "{} {}: step {} does not move", story.key, prop, step);
                    assert!(
                        min <= default && default <= max,
                        "{} {}: default {} is outside {}..{}",
                        story.key, prop, default, min, max
                    );
                }
            }
        }
    }
}
